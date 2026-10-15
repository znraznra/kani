//! Making word times trustworthy. Speech models time whole phrases well but
//! single words roughly, and they sometimes "hear" words in silence. These
//! helpers spread words over their phrase sensibly and check the phrase
//! against the actual loudness of the sound.

use crate::TimedWord;

/// A piece of recognised text (usually a token) and the times the model
/// guessed for it, in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub text: String,
    pub t0: Option<f64>,
    pub t1: Option<f64>,
}

pub(crate) fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF   // hiragana, katakana
        | 0x3400..=0x4DBF // han extension A
        | 0x4E00..=0x9FFF // han
        | 0xF900..=0xFAFF
        | 0xFF66..=0xFF9F)
}

/// Words from pieces, timed inside the phrase `[start, end]`.
pub fn words_from_pieces(start: f64, end: f64, pieces: &[Piece]) -> Vec<TimedWord> {
    struct Raw {
        text: String,
        t0: Option<f64>,
        t1: Option<f64>,
    }
    let mut raws: Vec<Raw> = Vec::new();
    for p in pieces {
        if p.text.is_empty() {
            continue;
        }
        let first = p.text.chars().next().unwrap_or(' ');
        let continues = raws.last().is_some_and(|r| {
            !first.is_whitespace() && !(is_cjk(first) || r.text.chars().last().is_some_and(is_cjk))
        });
        if continues {
            let r = raws.last_mut().unwrap();
            r.text.push_str(&p.text);
            r.t1 = p.t1.or(r.t1);
        } else {
            raws.push(Raw {
                text: p.text.clone(),
                t0: p.t0,
                t1: p.t1,
            });
        }
    }
    // Sound-only notes and blank words are not speech.
    raws.retain(|r| !r.text.trim().is_empty());
    if raws.is_empty() || end <= start {
        return Vec::new();
    }

    let span = end - start;
    let times: Option<Vec<(f64, f64)>> = raws
        .iter()
        .map(|r| Some((r.t0?, r.t1?)))
        .collect::<Option<Vec<_>>>();
    let trusted = times.as_ref().is_some_and(|t| {
        let ordered = t.windows(2).all(|w| w[1].0 + 0.02 >= w[0].0);
        let inside = t
            .iter()
            .all(|(a, b)| *a >= start - 0.1 && *b <= end + 0.1 && b >= a);
        let covered = t.last().map_or(0.0, |l| l.1) - t.first().map_or(0.0, |f| f.0);
        ordered && inside && covered >= 0.6 * span
    });
    if trusted {
        let t = times.unwrap();
        let mut out: Vec<TimedWord> = raws
            .into_iter()
            .zip(t)
            .map(|(r, (a, b))| TimedWord {
                text: r.text,
                start: a.clamp(start, end),
                end: b.clamp(start, end),
            })
            .collect();
        // A word with no length lasts until the next begins.
        for i in 0..out.len() {
            let next = out.get(i + 1).map_or(end, |n| n.start);
            if out[i].end <= out[i].start {
                out[i].end = next.max(out[i].start);
            }
        }
        return out;
    }

    // Otherwise: by the number of letters, which tracks speaking time well.
    let weight = |s: &str| s.chars().filter(|c| !c.is_whitespace()).count().max(1) as f64;
    let total: f64 = raws.iter().map(|r| weight(&r.text)).sum();
    let mut at = start;
    raws.into_iter()
        .map(|r| {
            let len = span * weight(&r.text) / total;
            let w = TimedWord {
                text: r.text,
                start: at,
                end: at + len,
            };
            at += len;
            w
        })
        .collect()
}

/// Where the sound is loud enough to be speech.
pub struct Voice {
    voiced: Vec<bool>,
}

const FRAME: f64 = 0.01;

impl Voice {
    /// `samples` at `rate` Hz, mono.
    pub fn analyse(samples: &[f32], rate: usize) -> Voice {
        let hop = (rate as f64 * FRAME) as usize;
        if hop == 0 || samples.len() < hop {
            return Voice { voiced: Vec::new() };
        }
        let rms: Vec<f32> = samples
            .chunks(hop)
            .map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt())
            .collect();
        let mut sorted = rms.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize];
        let (floor, loud) = (at(0.2), at(0.9));
        let threshold = (floor * 3.0).min(loud * 0.3).max(0.003);
        let mut voiced: Vec<bool> = rms.iter().map(|r| *r > threshold).collect();
        // Close short gaps (between syllables), then drop short blips (clicks).
        let n = voiced.len();
        let mut i = 0;
        while i < n {
            if !voiced[i] {
                let j = (i..n).find(|&j| voiced[j]).unwrap_or(n);
                if i > 0 && j < n && j - i <= 12 {
                    voiced[i..j].iter_mut().for_each(|v| *v = true);
                }
                i = j.max(i + 1);
            } else {
                i += 1;
            }
        }
        let mut i = 0;
        while i < n {
            if voiced[i] {
                let j = (i..n).find(|&j| !voiced[j]).unwrap_or(n);
                if j - i < 3 {
                    voiced[i..j].iter_mut().for_each(|v| *v = false);
                }
                i = j;
            } else {
                i += 1;
            }
        }
        Voice { voiced }
    }

    fn range(&self, from: f64, to: f64) -> std::ops::Range<usize> {
        let a = (from.max(0.0) / FRAME) as usize;
        let b = ((to.max(0.0) / FRAME).ceil() as usize).min(self.voiced.len());
        a.min(b)..b
    }

    /// The stretches of sound between `from` and `to`, with pauses shorter
    /// than `merge` seconds closed up.
    pub fn runs(&self, from: f64, to: f64, merge: f64) -> Vec<(f64, f64)> {
        let r = self.range(from, to);
        let start = r.start;
        let mut out: Vec<(f64, f64)> = Vec::new();
        for (i, v) in self.voiced[r].iter().enumerate() {
            if !*v {
                continue;
            }
            let (a, b) = ((start + i) as f64 * FRAME, (start + i + 1) as f64 * FRAME);
            match out.last_mut() {
                Some(last) if a - last.1 < merge => last.1 = b,
                _ => out.push((a, b)),
            }
        }
        out
    }

    pub fn first_voiced(&self, from: f64, to: f64) -> Option<f64> {
        let r = self.range(from, to);
        let start = r.start;
        self.voiced[r]
            .iter()
            .position(|v| *v)
            .map(|i| (start + i) as f64 * FRAME)
    }

    pub fn last_voiced(&self, from: f64, to: f64) -> Option<f64> {
        let r = self.range(from, to);
        let start = r.start;
        self.voiced[r]
            .iter()
            .rposition(|v| *v)
            .map(|i| (start + i + 1) as f64 * FRAME)
    }
}

/// Pull a phrase's words into the part that is actually spoken. Returns
/// `None` when the phrase lies in silence (the model made it up).
pub fn fit_to_speech(words: Vec<TimedWord>, voice: &Voice) -> Option<Vec<TimedWord>> {
    let (start, end) = (words.first()?.start, words.last()?.end);
    if voice.voiced.is_empty() {
        return Some(words);
    }
    // Some leeway: models are early or late by a fraction of a second.
    let onset = voice.first_voiced((start - 0.25).max(0.0), end)?;
    let offset = voice.last_voiced(start, end + 0.35)?;
    let new_start = onset.max(start).min(end);
    let new_end = (offset + 0.1).min(end).max(new_start + 0.05);
    if (new_start - start).abs() < 0.02 && (new_end - end).abs() < 0.02 {
        return Some(skip_pauses(words, voice, start, end));
    }
    let scale = (new_end - new_start) / (end - start).max(1e-6);
    let words: Vec<TimedWord> = words
        .into_iter()
        .map(|w| TimedWord {
            start: new_start + (w.start - start) * scale,
            end: new_start + (w.end - start) * scale,
            text: w.text,
        })
        .collect();
    Some(skip_pauses(words, voice, new_start, new_end))
}

/// A pause in the middle of a phrase (longer than `PAUSE` seconds) has no
/// words in it: the words are moved onto the stretches where there is sound,
/// so the line stops before the pause and the next one starts after it.
const PAUSE: f64 = 0.6;

fn skip_pauses(words: Vec<TimedWord>, voice: &Voice, start: f64, end: f64) -> Vec<TimedWord> {
    let runs = voice.runs(start, end, PAUSE);
    if runs.len() < 2 {
        return words;
    }
    let in_sound = |t: f64| runs.iter().any(|(a, b)| t >= *a - 0.05 && t <= *b + 0.05);
    if words.iter().all(|w| in_sound((w.start + w.end) / 2.0)) {
        return words;
    }
    // Lay the words along the sound only, in proportion to where they were.
    let total: f64 = runs.iter().map(|(a, b)| b - a).sum();
    let span = (end - start).max(1e-6);
    // (time, which stretch of sound). A word's start belongs to the later
    // stretch when it falls exactly between two.
    let place = |frac: f64, is_start: bool| -> (f64, usize) {
        let mut left = frac.clamp(0.0, 1.0) * total;
        for (i, (a, b)) in runs.iter().enumerate() {
            let len = b - a;
            if left < len || (!is_start && left <= len) {
                return (a + left, i);
            }
            left -= len;
        }
        (runs.last().map_or(end, |r| r.1), runs.len() - 1)
    };
    words
        .into_iter()
        .map(|w| {
            let (s, si) = place((w.start - start) / span, true);
            let (e, ei) = place((w.end - start) / span, false);
            // A word does not stretch across a pause.
            let e = if ei == si { e } else { runs[si].1 };
            TimedWord {
                start: s,
                end: e.max(s),
                text: w.text,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(t: &str, a: Option<f64>, b: Option<f64>) -> Piece {
        Piece {
            text: t.into(),
            t0: a,
            t1: b,
        }
    }

    #[test]
    fn tokens_join_into_words_and_spread_by_length_without_times() {
        let p = vec![
            piece(" un", None, None),
            piece("believ", None, None),
            piece("able", None, None),
            piece(" day", None, None),
        ];
        let w = words_from_pieces(1.0, 3.0, &p);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].text, " unbelievable");
        assert!((w[0].start - 1.0).abs() < 1e-9 && (w[1].end - 3.0).abs() < 1e-9);
        // "unbelievable" has 12 letters, "day" 3: it takes four fifths of the time.
        assert!((w[0].end - 2.6).abs() < 1e-9, "{}", w[0].end);
    }

    #[test]
    fn sensible_token_times_are_used_and_silly_ones_are_not() {
        let good = vec![
            piece(" one", Some(1.0), Some(1.5)),
            piece(" two", Some(1.6), Some(2.2)),
            piece(" three", Some(2.3), Some(3.0)),
        ];
        let w = words_from_pieces(1.0, 3.0, &good);
        assert!((w[1].start - 1.6).abs() < 1e-9);
        // All crammed at the start of a long phrase: ignored, spread evenly instead.
        let bad = vec![
            piece(" one", Some(1.0), Some(1.1)),
            piece(" two", Some(1.1), Some(1.2)),
            piece(" tri", Some(1.2), Some(1.3)),
        ];
        let w = words_from_pieces(1.0, 4.0, &bad);
        assert!(w[2].end > 3.9, "{w:?}");
    }

    #[test]
    fn japanese_gets_one_word_per_character_group_not_one_giant_word() {
        let p = vec![
            piece("今", None, None),
            piece("日", None, None),
            piece("は", None, None),
        ];
        assert_eq!(words_from_pieces(0.0, 3.0, &p).len(), 3);
    }

    fn tone(secs: &[(f64, f64)], total: f64) -> Vec<f32> {
        let rate = 16_000usize;
        (0..(total * rate as f64) as usize)
            .map(|i| {
                let t = i as f64 / rate as f64;
                if secs.iter().any(|(a, b)| t >= *a && t < *b) {
                    (0.3 * (t * 440.0 * std::f64::consts::TAU).sin()) as f32
                } else {
                    0.0005 * ((i * 7919) % 100) as f32 / 100.0
                }
            })
            .collect()
    }

    #[test]
    fn a_phrase_that_starts_too_early_is_pulled_to_where_the_speech_is() {
        let voice = Voice::analyse(&tone(&[(2.0, 4.0)], 6.0), 16_000);
        let words = vec![
            TimedWord {
                text: " early".into(),
                start: 0.5,
                end: 1.5,
            },
            TimedWord {
                text: " bird".into(),
                start: 1.5,
                end: 5.5,
            },
        ];
        let fitted = fit_to_speech(words, &voice).unwrap();
        assert!((fitted[0].start - 2.0).abs() < 0.05, "{fitted:?}");
        assert!(fitted[1].end <= 4.2, "{fitted:?}");
        assert!(fitted[0].end <= fitted[1].start + 1e-9);
    }

    #[test]
    fn a_pause_inside_a_phrase_has_no_words_in_it() {
        // Speech 1.0-2.0 s and 5.0-6.0 s; the model spread eight words over 1.0-6.0.
        let voice = Voice::analyse(&tone(&[(1.0, 2.0), (5.0, 6.0)], 7.0), 16_000);
        let words: Vec<TimedWord> = (0..8)
            .map(|i| TimedWord {
                text: format!(" w{i}"),
                start: 1.0 + i as f64 * 0.625,
                end: 1.0 + (i + 1) as f64 * 0.625,
            })
            .collect();
        let out = fit_to_speech(words, &voice).unwrap();
        for w in &out {
            let mid = (w.start + w.end) / 2.0;
            assert!(
                (1.0..=2.05).contains(&mid) || (4.95..=6.05).contains(&mid),
                "{w:?} sits in the pause"
            );
        }
        assert!(
            out[4].start - out[3].end > 2.0,
            "the gap shows between the halves"
        );
    }

    #[test]
    fn a_phrase_in_silence_is_dropped() {
        let voice = Voice::analyse(&tone(&[(5.0, 6.0)], 8.0), 16_000);
        let words = vec![TimedWord {
            text: " Thanks".into(),
            start: 0.5,
            end: 1.5,
        }];
        assert!(fit_to_speech(words, &voice).is_none());
    }

    #[test]
    fn well_timed_speech_is_left_alone() {
        let voice = Voice::analyse(&tone(&[(1.0, 3.0)], 4.0), 16_000);
        let words = vec![
            TimedWord {
                text: " fine".into(),
                start: 1.0,
                end: 2.0,
            },
            TimedWord {
                text: " words".into(),
                start: 2.0,
                end: 3.0,
            },
        ];
        let fitted = fit_to_speech(words.clone(), &voice).unwrap();
        for (a, b) in fitted.iter().zip(&words) {
            assert!(
                (a.start - b.start).abs() < 0.12 && (a.end - b.end).abs() < 0.12,
                "{fitted:?}"
            );
        }
    }
}
