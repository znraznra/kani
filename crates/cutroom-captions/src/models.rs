//! The speech models: where they live, which exist, and fetching them.

use crate::CaptionError;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    pub id: &'static str,
    pub label: &'static str,
    pub file: &'static str,
    pub approx_mb: u32,
}

pub const MODELS: &[Model] = &[
    Model {
        id: "tiny",
        label: "Tiny: fastest, roughest",
        file: "ggml-tiny.bin",
        approx_mb: 78,
    },
    Model {
        id: "base",
        label: "Base: quick, decent",
        file: "ggml-base.bin",
        approx_mb: 148,
    },
    Model {
        id: "small",
        label: "Small: slower, better",
        file: "ggml-small.bin",
        approx_mb: 488,
    },
    Model {
        id: "medium",
        label: "Medium (compressed): accurate, slow",
        file: "ggml-medium-q5_0.bin",
        approx_mb: 539,
    },
    Model {
        id: "turbo",
        label: "Large v3 Turbo (compressed): most accurate",
        file: "ggml-large-v3-turbo-q5_0.bin",
        approx_mb: 574,
    },
];

/// The small model that tells speech from music and silence (Silero VAD).
pub const VOICE_DETECTOR: Model = Model {
    id: "vad",
    label: "Voice detector",
    file: "ggml-silero-v5.1.2.bin",
    approx_mb: 1,
};

pub fn by_id(id: &str) -> Model {
    MODELS
        .iter()
        .copied()
        .find(|m| m.id == id)
        .unwrap_or(MODELS[1])
}

impl Model {
    pub fn url(&self) -> String {
        let repo = if self.id == "vad" {
            "ggml-org/whisper-vad"
        } else {
            "ggerganov/whisper.cpp"
        };
        format!("https://huggingface.co/{repo}/resolve/main/{}", self.file)
    }

    pub fn path_in(&self, dir: &Path) -> PathBuf {
        dir.join(self.file)
    }

    pub fn installed_in(&self, dir: &Path) -> bool {
        std::fs::metadata(self.path_in(dir)).map_or(false, |m| m.len() > 500_000)
    }
}

/// Download `url` to `dest` (via a `.part` file, so a stopped download never
/// looks like a model). `progress` gets bytes so far and the total if known.
pub fn download_from(
    url: &str,
    dest: &Path,
    mut progress: impl FnMut(u64, Option<u64>),
    cancel: &AtomicBool,
) -> Result<(), CaptionError> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut response = ureq::get(url)
        .call()
        .map_err(|e| CaptionError::Message(format!("download failed: {e}")))?;
    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let part = dest.with_extension("part");
    let mut file = std::fs::File::create(&part)?;
    let mut reader = response.body_mut().with_config().limit(u64::MAX).reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let result = (|| -> Result<(), CaptionError> {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(CaptionError::Cancelled);
            }
            let n = reader
                .read(&mut buf)
                .map_err(|e| CaptionError::Message(format!("download interrupted: {e}")))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])?;
            done += n as u64;
            progress(done, total);
        }
        file.flush()?;
        if let Some(t) = total {
            if t != done {
                return Err(CaptionError::Message("download was cut short".into()));
            }
        }
        Ok(())
    })();
    drop(file);
    match result {
        Ok(()) => {
            std::fs::rename(&part, dest)?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

pub fn download(
    model: Model,
    dir: &Path,
    progress: impl FnMut(u64, Option<u64>),
    cancel: &AtomicBool,
) -> Result<PathBuf, CaptionError> {
    let dest = model.path_in(dir);
    download_from(&model.url(), &dest, progress, cancel)?;
    Ok(dest)
}

/// The voice detector, fetched if it is not there yet. `None` when it cannot
/// be had (offline, say): captions then fall back to plain loudness.
pub fn ensure_voice_detector(dir: &Path, cancel: &AtomicBool) -> Option<PathBuf> {
    let model = VOICE_DETECTOR;
    if !model.installed_in(dir) {
        let _ = std::fs::create_dir_all(dir);
        download(model, dir, |_, _| {}, cancel).ok()?;
    }
    model.installed_in(dir).then(|| model.path_in(dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    fn serve(body: Vec<u8>, lie_about_length: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                while r.read_line(&mut line).map_or(false, |n| n > 2) {
                    line.clear();
                }
                let claimed = if lie_about_length {
                    body.len() + 10
                } else {
                    body.len()
                };
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Length: {claimed}\r\nConnection: close\r\n\r\n"
                );
                let _ = s.write_all(&body);
            }
        });
        format!("http://{addr}/model.bin")
    }

    #[test]
    fn a_download_lands_whole_and_reports_progress() {
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let url = serve(body.clone(), false);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("models").join("ggml-test.bin");
        let mut last = (0, None);
        download_from(&url, &dest, |d, t| last = (d, t), &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(last, (300_000, Some(300_000)));
        assert!(!dest.with_extension("part").exists());
    }

    #[test]
    fn a_cut_short_download_leaves_no_model_behind() {
        let url = serve(vec![7; 50_000], true);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("ggml-test.bin");
        assert!(download_from(&url, &dest, |_, _| {}, &AtomicBool::new(false)).is_err());
        assert!(!dest.exists());
        assert!(!dest.with_extension("part").exists());
    }

    #[test]
    fn cancelling_stops_and_cleans_up() {
        let url = serve(vec![1; 500_000], false);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("ggml-test.bin");
        let cancel = AtomicBool::new(true);
        let err = download_from(&url, &dest, |_, _| {}, &cancel).unwrap_err();
        assert!(matches!(err, CaptionError::Cancelled));
        assert!(!dest.exists() && !dest.with_extension("part").exists());
    }

    #[test]
    fn installed_means_a_real_sized_file() {
        let dir = tempfile::tempdir().unwrap();
        let m = by_id("tiny");
        assert!(!m.installed_in(dir.path()));
        std::fs::write(m.path_in(dir.path()), b"x").unwrap();
        assert!(!m.installed_in(dir.path()));
        std::fs::write(m.path_in(dir.path()), vec![0u8; 2_000_000]).unwrap();
        assert!(m.installed_in(dir.path()));
    }
}
