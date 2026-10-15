//! Local speech recognition with whisper.cpp.

use crate::align::{self, Piece, Voice};
use crate::{CaptionError, TimedWord};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// `samples` are 16 kHz mono floats. `language` is a Whisper code such as
/// "en", or None to detect it. `progress` gets 0..=100 as the work runs
/// (on the calling thread).
pub fn transcribe(
    model: &Path,
    voice_detector: Option<&Path>,
    samples: &[f32],
    language: Option<&str>,
    progress: impl FnMut(i32) + 'static,
    cancel: Arc<AtomicBool>,
) -> Result<Vec<TimedWord>, CaptionError> {
    // Sound that is not speech (music, noise, silence) is silenced first, so
    // the model has nothing to invent captions from, and the timing step
    // below sees only the spoken parts.
    clear_debug();
    let gated = voice_detector.and_then(|v| speech_only(v, samples));
    let voice_gate = gated.is_some();
    note(format!(
        "audio {:.1}s, language {language:?}, voice detector {}",
        samples.len() as f64 / 16_000.0,
        if voice_gate { "used" } else { "not used" }
    ));
    let samples: &[f32] = gated.as_deref().unwrap_or(samples);
    let fail =
        |what: &str, e: &dyn std::fmt::Display| CaptionError::Message(format!("{what}: {e}"));
    let mut cp = WhisperContextParameters::default();
    cp.use_gpu(false);
    let ctx = WhisperContext::new_with_params(model, cp)
        .map_err(|e| fail("could not load the speech model", &e))?;
    keep_log();

    // Try the careful settings first; if the engine fails part-way (some
    // machines do, for reasons that only show in its own log), try again with
    // plainer ones before giving up.
    unsafe extern "C" fn should_abort(flag: *mut std::ffi::c_void) -> bool {
        (*(flag as *const AtomicBool)).load(Ordering::Relaxed)
    }
    let progress = Arc::new(std::sync::Mutex::new(
        Box::new(progress) as Box<dyn FnMut(i32)>
    ));
    let max_threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut state = None;
    let mut failure = String::new();
    for attempt in 0..3 {
        let mut s = ctx
            .create_state()
            .map_err(|e| fail("could not start recognition", &e))?;
        let careful = attempt == 0;
        let mut params = if careful {
            // Beam search reads more carefully than the quick greedy pass.
            // Word times come from each phrase (see `align`), not from asking
            // the model to cut at every word, which makes words early or late.
            FullParams::new(SamplingStrategy::BeamSearch {
                beam_size: 5,
                patience: -1.0,
            })
        } else {
            FullParams::new(SamplingStrategy::Greedy { best_of: 1 })
        };
        params.set_language(language);
        params.set_translate(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        params.set_no_context(true);
        params.set_token_timestamps(careful);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_no_speech_thold(0.6);
        params.set_temperature(0.0);
        params.set_temperature_inc(0.2);
        let threads = match attempt {
            0 => max_threads.min(8),
            1 => max_threads.min(4),
            _ => 1,
        };
        params.set_n_threads(threads as i32);
        {
            let progress = progress.clone();
            params.set_progress_callback_safe(move |p: i32| {
                if let Ok(mut f) = progress.lock() {
                    f(p.clamp(0, 100));
                }
            });
        }
        // `cancel` outlives the call to `full` below, which is the only user.
        // (whisper-rs's own "safe" abort hook is unsound, so this is the
        // plain C hook with the flag itself.)
        if attempt < 2 {
            unsafe {
                params.set_abort_callback(Some(should_abort));
                params.set_abort_callback_user_data(Arc::as_ptr(&cancel) as *mut std::ffi::c_void);
            }
        }
        clear_log();
        let result = s.full(params, samples);
        if cancel.load(Ordering::Relaxed) {
            return Err(CaptionError::Cancelled);
        }
        match result {
            Ok(_) => {
                state = Some(s);
                break;
            }
            Err(e) => {
                failure = format!(
                    "{e} (attempt {}, {threads} threads)\n{}",
                    attempt + 1,
                    log_tail()
                );
            }
        }
    }
    let Some(state) = state else {
        return Err(CaptionError::Message(format!(
            "recognition failed: {failure}"
        )));
    };

    let eot = ctx.token_eot();
    let voice = Voice::analyse(samples, 16_000);
    let mut words: Vec<TimedWord> = Vec::new();
    let mut last_text = String::new();
    let mut repeats = 0;
    for seg in state.as_iter() {
        let (start, end) = (
            seg.start_timestamp() as f64 / 100.0,
            seg.end_timestamp() as f64 / 100.0,
        );
        let no_speech = seg.no_speech_probability();
        // How sure the model was of the words themselves.
        let n = seg.n_tokens();
        let probs: Vec<f32> = (0..n)
            .filter_map(|i| seg.get_token(i))
            .map(|t| t.token_data())
            .filter(|d| d.id < eot)
            .map(|d| d.p)
            .collect();
        let mean_p = probs.iter().sum::<f32>() / probs.len().max(1) as f32;
        let seg_text = seg
            .to_str_lossy()
            .map(|c| c.trim().to_string())
            .unwrap_or_default();
        let head =
            format!("{start:7.2}-{end:7.2}s no_speech {no_speech:.2} p {mean_p:.2} {seg_text:?}");
        // The model says "no speech here" with some confidence. With the
        // voice detector's word on it, only the surest cases are dropped.
        let limit = if voice_gate { 0.9 } else { 0.75 };
        if no_speech > limit {
            note(format!("{head} -> dropped: no speech"));
            continue;
        }
        // Unsure of both "is this speech" and the words (without the voice
        // detector to say so): music or noise the model is guessing at.
        if !voice_gate && no_speech > 0.4 && mean_p < 0.3 {
            note(format!("{head} -> dropped: unsure"));
            continue;
        }
        // Stock phrases models make up out of silence and noise.
        if (no_speech > 0.3 || mean_p < 0.5) && is_stock_phrase(&seg_text) {
            note(format!("{head} -> dropped: stock phrase"));
            continue;
        }
        let mut pieces = Vec::new();
        for i in 0..seg.n_tokens() {
            let Some(token) = seg.get_token(i) else {
                continue;
            };
            let data = token.token_data();
            // Markers (start, end, timestamps, language) are not speech.
            if data.id >= eot {
                continue;
            }
            let text = token
                .to_str_lossy()
                .map_err(|e| fail("could not read the result", &e))?;
            if text.starts_with("[_") {
                continue;
            }
            let timed = data.t0 >= 0 && data.t1 >= 0;
            pieces.push(Piece {
                text: text.to_string(),
                t0: timed.then(|| data.t0 as f64 / 100.0),
                t1: timed.then(|| data.t1 as f64 / 100.0),
            });
        }
        let phrase: Vec<TimedWord> = align::words_from_pieces(start, end, &pieces);
        let text: String = phrase
            .iter()
            .map(|w| w.text.as_str())
            .collect::<String>()
            .trim()
            .to_lowercase();
        if text.is_empty() {
            note(format!("{head} -> dropped: empty"));
            continue;
        }
        // A model stuck in a loop says the same thing again and again.
        if text == last_text {
            repeats += 1;
            if repeats >= 2 {
                note(format!("{head} -> dropped: repeat"));
                continue;
            }
        } else {
            repeats = 0;
            last_text = text;
        }
        // Keep it only where there is sound to say it in.
        let count = phrase.len();
        match align::fit_to_speech(phrase, &voice) {
            Some(fitted) => {
                note(format!(
                    "{head} -> kept {count} words, {:.2}-{:.2}s",
                    fitted.first().map_or(0.0, |w| w.start),
                    fitted.last().map_or(0.0, |w| w.end)
                ));
                words.extend(fitted);
            }
            None => note(format!("{head} -> dropped: no sound there")),
        }
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tiny test model from whisper.cpp has random weights: it cannot hear
    /// anything, but it proves the model loads and the pipeline runs through.
    #[test]
    fn the_pipeline_runs_end_to_end_on_silence() {
        let model =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/for-tests-ggml-tiny.en.bin");
        let samples = vec![0.0f32; 16_000 * 2];
        let seen = Arc::new(std::sync::atomic::AtomicI32::new(-1));
        let s2 = seen.clone();
        let out = transcribe(
            &model,
            None,
            &samples,
            Some("en"),
            move |p| s2.store(p, Ordering::Relaxed),
            Arc::new(AtomicBool::new(false)),
        );
        assert!(out.is_ok(), "{out:?}");
        for w in out.unwrap() {
            assert!(w.end >= w.start);
        }
    }

    /// Loud sound reaches the encoder, which used to "fail" because of a broken
    /// abort hook in the binding (error -6).
    #[test]
    fn real_sound_gets_through_the_encoder() {
        let model =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/for-tests-ggml-tiny.en.bin");
        let samples: Vec<f32> = (0..16_000 * 3)
            .map(|i| 0.3 * (i as f32 * 0.05).sin())
            .collect();
        let out = transcribe(
            &model,
            None,
            &samples,
            Some("en"),
            |_| {},
            Arc::new(AtomicBool::new(false)),
        );
        assert!(out.is_ok(), "{out:?}");
    }

    #[test]
    fn a_missing_model_is_an_error_not_a_crash() {
        let out = transcribe(
            Path::new("/nonexistent/model.bin"),
            None,
            &[0.0; 16000],
            None,
            |_| {},
            Arc::new(AtomicBool::new(false)),
        );
        assert!(matches!(out, Err(CaptionError::Message(_))));
    }

    #[test]
    fn cancelling_before_starting_returns_cancelled() {
        let model =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/for-tests-ggml-tiny.en.bin");
        let out = transcribe(
            &model,
            None,
            &vec![0.0; 16000 * 2],
            Some("en"),
            |_| {},
            Arc::new(AtomicBool::new(true)),
        );
        assert!(matches!(out, Err(CaptionError::Cancelled)), "{out:?}");
    }
}

/// `samples` with everything the voice detector does not call speech set to
/// silence. `None` if the detector cannot run.
fn speech_only(detector: &Path, samples: &[f32]) -> Option<Vec<f32>> {
    use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};
    let mut cp = WhisperVadContextParams::new();
    cp.set_use_gpu(false);
    let mut vad = WhisperVadContext::new(detector.to_str()?, cp).ok()?;
    let mut params = WhisperVadParams::new();
    // Generous: better a little music let through (the later steps trim it)
    // than speech cut away.
    params.set_threshold(0.35);
    params.set_min_speech_duration(100);
    params.set_min_silence_duration(300);
    params.set_speech_pad(200);
    let segments = vad.segments_from_samples(params, samples).ok()?;
    let rate = 16_000.0_f64;
    let mut out = vec![0.0f32; samples.len()];
    let mut kept = 0usize;
    for seg in segments {
        note(format!(
            "voice {:.2}-{:.2}s",
            f64::from(seg.start) / 100.0,
            f64::from(seg.end) / 100.0
        ));
        // The detector counts in hundredths of a second.
        let a = ((f64::from(seg.start) / 100.0 * rate) as usize).min(samples.len());
        let b = ((f64::from(seg.end) / 100.0 * rate) as usize).min(samples.len());
        if b > a {
            out[a..b].copy_from_slice(&samples[a..b]);
            kept += b - a;
        }
    }
    // A detector that hears next to nothing in a long recording has probably
    // failed: do not throw the sound away on its word.
    if samples.len() > 16_000 * 3 && kept * 50 < samples.len() {
        note("voice detector found almost no speech: ignored".into());
        return None;
    }
    Some(out)
}

/// What the model hears and what is done with it, kept as plain text so a
/// bad result can be looked into.
static DEBUG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn note(line: String) {
    if let Ok(mut d) = DEBUG.lock() {
        if d.len() < 2000 {
            d.push(line);
        }
    }
}

fn clear_debug() {
    if let Ok(mut d) = DEBUG.lock() {
        d.clear();
    }
}

/// The notes of the latest run.
pub fn debug_report() -> String {
    DEBUG.lock().map(|d| d.join("\n")).unwrap_or_default()
}

/// Lines models are known to invent out of silence, music and noise.
fn is_stock_phrase(text: &str) -> bool {
    let t: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    matches!(
        t.as_str(),
        "thank you"
            | "thanks for watching"
            | "thank you for watching"
            | "thanks for watching and ill see you in the next video"
            | "you"
            | "bye"
            | "bye bye"
            | "please subscribe"
            | "like and subscribe"
            | "subtitles by the amaraorg community"
            | "music"
    )
}

// ----- the engine's own messages, kept so a failure can say why --------------------------

static LOG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

unsafe extern "C" fn on_log(
    _level: whisper_rs_sys::ggml_log_level,
    text: *const std::os::raw::c_char,
    _user: *mut std::ffi::c_void,
) {
    if text.is_null() {
        return;
    }
    let line = std::ffi::CStr::from_ptr(text).to_string_lossy();
    for part in line.split('\n') {
        let part = part.trim();
        // Per-token chatter would bury the useful lines.
        if part.is_empty() || part.contains("token =") || part.contains(" id = ") {
            continue;
        }
        if let Ok(mut log) = LOG.lock() {
            if log.len() >= 24 {
                log.remove(0);
            }
            log.push(part.chars().take(160).collect());
        }
    }
}

fn keep_log() {
    unsafe { whisper_rs_sys::whisper_log_set(Some(on_log), std::ptr::null_mut()) };
}

fn clear_log() {
    if let Ok(mut log) = LOG.lock() {
        log.clear();
    }
}

fn log_tail() -> String {
    LOG.lock()
        .map(|l| {
            l.iter()
                .rev()
                .take(8)
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
