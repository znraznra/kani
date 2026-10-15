//! Subtitle files.

/// A cue: start and end in seconds, and its text.
pub type Cue = (f64, f64, String);

fn stamp(secs: f64, comma: bool) -> String {
    let ms = (secs.max(0.0) * 1000.0).round() as u64;
    let (h, m, s, ms) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    let sep = if comma { ',' } else { '.' };
    format!("{h:02}:{m:02}:{s:02}{sep}{ms:03}")
}

pub fn to_srt(cues: &[Cue]) -> String {
    let mut out = String::new();
    for (i, (a, b, text)) in cues.iter().enumerate() {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            stamp(*a, true),
            stamp(*b, true),
            text
        ));
    }
    out
}

pub fn to_vtt(cues: &[Cue]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for (a, b, text) in cues {
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            stamp(*a, false),
            stamp(*b, false),
            text
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_and_vtt_have_the_right_shape() {
        let cues = vec![
            (0.0, 1.5, "Hi".to_string()),
            (3723.25, 3725.0, "Later".to_string()),
        ];
        assert_eq!(
            to_srt(&cues),
            "1\n00:00:00,000 --> 00:00:01,500\nHi\n\n2\n01:02:03,250 --> 01:02:05,000\nLater\n\n"
        );
        assert!(to_vtt(&cues).starts_with("WEBVTT\n\n00:00:00.000 --> 00:00:01.500\nHi"));
    }
}
