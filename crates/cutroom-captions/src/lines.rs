//! Group words into caption lines that read well on screen.

use crate::TimedWord;

#[derive(Debug, Clone, Copy)]
pub struct LineRules {
    /// Longest line, in characters.
    pub max_chars: usize,
    /// Longest a line stays up, in seconds.
    pub max_secs: f64,
    /// A pause this long ends the line.
    pub gap: f64,
}

impl Default for LineRules {
    fn default() -> Self {
        Self {
            max_chars: 32,
            max_secs: 4.0,
            gap: 0.8,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub start: f64,
    pub end: f64,
    pub words: Vec<TimedWord>,
}

impl Line {
    pub fn text(&self) -> String {
        let s: String = self.words.iter().map(|w| w.text.as_str()).collect();
        s.trim().to_string()
    }
}

fn ends_sentence(w: &str) -> bool {
    matches!(
        w.trim_end().chars().last(),
        Some('.' | '!' | '?' | '…' | '。' | '！' | '？')
    )
}

fn only_punctuation(w: &str) -> bool {
    let t = w.trim();
    !t.is_empty() && t.chars().all(|c| !c.is_alphanumeric())
}

/// Join stray punctuation tokens onto the word before them, drop empty
/// tokens and bracketed sound notes such as `[MUSIC]`.
pub fn tidy(words: &[TimedWord]) -> Vec<TimedWord> {
    let mut out: Vec<TimedWord> = Vec::new();
    for w in words {
        let t = w.text.trim();
        if t.is_empty()
            || (t.starts_with('[') && t.ends_with(']'))
            || (t.starts_with('(') && t.ends_with(')'))
        {
            continue;
        }
        if only_punctuation(&w.text) {
            if let Some(last) = out.last_mut() {
                last.text.push_str(t);
                last.end = last.end.max(w.end);
                continue;
            }
        }
        let mut w = w.clone();
        let cjk_join = out.last().is_some_and(|l| {
            l.text.chars().last().is_some_and(crate::align::is_cjk)
                || w.text.chars().next().is_some_and(crate::align::is_cjk)
        });
        if !w.text.starts_with(char::is_whitespace) && !cjk_join {
            // Whisper marks a new word with a leading space; a token without
            // one continues the word before it.
            if let Some(last) = out.last_mut() {
                last.text.push_str(&w.text);
                last.end = last.end.max(w.end);
                continue;
            }
        }
        if w.end < w.start {
            w.end = w.start;
        }
        out.push(w);
    }
    out
}

pub fn group(words: &[TimedWord], rules: LineRules) -> Vec<Line> {
    let words = tidy(words);
    let mut lines: Vec<Line> = Vec::new();
    let mut cur: Vec<TimedWord> = Vec::new();
    let mut chars = 0usize;
    let flush = |cur: &mut Vec<TimedWord>, lines: &mut Vec<Line>| {
        if let (Some(f), Some(l)) = (cur.first(), cur.last()) {
            lines.push(Line {
                start: f.start,
                end: l.end,
                words: std::mem::take(cur),
            });
        }
    };
    for w in words {
        if let Some(last) = cur.last() {
            let len = w.text.trim().chars().count() + 1;
            let too_long = chars + len > rules.max_chars;
            let too_slow = w.end - cur[0].start > rules.max_secs;
            let paused = w.start - last.end >= rules.gap;
            if too_long || too_slow || paused || ends_sentence(&last.text) {
                flush(&mut cur, &mut lines);
                chars = 0;
            }
        }
        chars += w.text.trim().chars().count() + 1;
        cur.push(w);
    }
    flush(&mut cur, &mut lines);
    // Give each line a hair of tail so it does not flick off mid-breath, but
    // never into the next line.
    for i in 0..lines.len() {
        let limit = lines.get(i + 1).map_or(f64::MAX, |n| n.start);
        lines[i].end = (lines[i].end + 0.15).min(limit).max(lines[i].end);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(t: &str, a: f64, b: f64) -> TimedWord {
        TimedWord {
            text: t.into(),
            start: a,
            end: b,
        }
    }

    #[test]
    fn a_pause_or_a_full_stop_starts_a_new_line() {
        let words = vec![
            w(" Hello", 0.0, 0.4),
            w(" there.", 0.4, 0.9),
            w(" How", 1.0, 1.2),
            w(" are", 1.2, 1.4),
            w(" you", 3.0, 3.3),
        ];
        let lines = group(&words, LineRules::default());
        let text: Vec<_> = lines.iter().map(|l| l.text()).collect();
        assert_eq!(text, ["Hello there.", "How are", "you"]);
    }

    #[test]
    fn long_runs_are_cut_at_the_character_limit() {
        let words: Vec<_> = (0..12)
            .map(|i| w(" wordy", i as f64 * 0.3, i as f64 * 0.3 + 0.25))
            .collect();
        let lines = group(&words, LineRules::default());
        assert!(lines.len() >= 2);
        assert!(lines.iter().all(|l| l.text().chars().count() <= 33));
    }

    #[test]
    fn stray_punctuation_and_sound_notes_are_tidied() {
        let words = vec![
            w(" [MUSIC]", 0.0, 1.0),
            w(" Hi", 1.0, 1.2),
            w(",", 1.2, 1.25),
            w(" you", 1.3, 1.5),
        ];
        let lines = group(&words, LineRules::default());
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text(), "Hi, you");
    }

    #[test]
    fn a_token_without_a_leading_space_continues_the_word() {
        let t = tidy(&[w(" un", 0.0, 0.1), w("believable", 0.1, 0.4)]);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].text, " unbelievable");
    }
}
