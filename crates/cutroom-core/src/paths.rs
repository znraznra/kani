//! Turning pasted or dropped text into file paths (`file://` URIs, quoted
//! paths, one per line).

use std::path::{Path, PathBuf};

/// Percent-decoding for `file://` URIs (`%20` becomes a space, and so on).
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(value) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `file:///home/me/a%20b.mp4` -> `/home/me/a b.mp4`; `file:///C:/x/a.mp4` -> `C:/x/a.mp4`.
fn path_from_file_uri(rest: &str) -> Option<PathBuf> {
    let slash = rest.find('/')?;
    let host = &rest[..slash];
    if !host.is_empty() && host != "localhost" {
        return None; // a file on another machine
    }
    let mut path = percent_decode(&rest[slash..]);
    let b = path.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        path.remove(0); // Windows drive letter
    }
    Some(PathBuf::from(path))
}

/// Paths found in clipboard or dropped text: one per line, optionally quoted
/// (Windows "Copy as path") or `file://` URIs. Lines like `copy`/`cut` written
/// by some Linux file managers are ignored.
pub fn paths_from_text(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(|line| line.trim().trim_matches(|c| c == '"' || c == '\''))
        .filter(|line| !line.is_empty() && !matches!(*line, "copy" | "cut"))
        .filter_map(|line| match line.strip_prefix("file://") {
            Some(rest) => path_from_file_uri(rest),
            None => Some(PathBuf::from(line)),
        })
        .collect()
}

/// Keep only paths that exist (so random pasted text isn't treated as files).
pub fn existing(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .filter(|p| Path::new(p).exists())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn uris_are_decoded() {
        assert_eq!(
            paths_from_text("file:///home/me/My%20Videos/a%2Bb.mp4"),
            [PathBuf::from("/home/me/My Videos/a+b.mp4")]
        );
        assert_eq!(
            paths_from_text("file://localhost/tmp/x.mp4"),
            [PathBuf::from("/tmp/x.mp4")]
        );
        assert!(paths_from_text("file://otherhost/tmp/x.mp4").is_empty());
    }

    #[test]
    fn windows_drive_uris_lose_the_leading_slash() {
        assert_eq!(
            paths_from_text("file:///C:/Users/me/clip.mp4"),
            [PathBuf::from("C:/Users/me/clip.mp4")]
        );
    }

    #[test]
    fn quoted_paths_lines_and_crlf_are_handled() {
        let text = "\"C:\\Users\\me\\a.mp4\"\r\n'/tmp/b.mov'\r\n\r\n  /tmp/c.png  \r\n";
        assert_eq!(
            paths_from_text(text),
            [
                PathBuf::from("C:\\Users\\me\\a.mp4"),
                PathBuf::from("/tmp/b.mov"),
                PathBuf::from("/tmp/c.png"),
            ]
        );
    }

    #[test]
    fn gnome_style_copy_markers_are_ignored() {
        let text = "copy\nfile:///tmp/a.mp4\nfile:///tmp/b.mp4";
        assert_eq!(paths_from_text(text).len(), 2);
        assert_eq!(paths_from_text("cut\nfile:///tmp/a.mp4").len(), 1);
    }

    #[test]
    fn broken_percent_escapes_are_left_alone() {
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a%zzb"), "a%zzb");
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("%41%42"), "AB");
        assert_eq!(percent_decode("%E6%97%A5"), "日");
    }

    #[test]
    fn only_paths_that_exist_are_kept() {
        let tmp = tempdir().unwrap();
        let real = tmp.path().join("real.mp4");
        std::fs::write(&real, b"x").unwrap();
        let kept = existing(vec![real.clone(), tmp.path().join("nope.mp4")]);
        assert_eq!(kept, [real]);
        assert!(existing(paths_from_text("just some words")).is_empty());
    }

    #[test]
    fn plain_text_with_a_real_path_round_trips() {
        let tmp = tempdir().unwrap();
        let real = tmp.path().join("with space.mp4");
        std::fs::write(&real, b"x").unwrap();
        let text = format!("\"{}\"", real.display());
        assert_eq!(existing(paths_from_text(&text)), [real]);
    }
}
