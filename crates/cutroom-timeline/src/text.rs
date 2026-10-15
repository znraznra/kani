//! Text clips: words drawn by the app instead of picture or sound taken from
//! a file. The clip keeps only the description below; the renderer turns it
//! into a transparent picture the size of the canvas, which then goes through
//! the same move / scale / turn / fade steps as any other layer.
//!
//! Sizes are relative so a project looks the same at every resolution:
//! `size` is in thousandths of the canvas height, and the effect sizes
//! (outline, shadow, padding, letter spacing) are in thousandths of `size`.

use serde::{Deserialize, Serialize};

/// A colour as red, green, blue, alpha (straight alpha).
pub type Rgba = [u8; 4];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

/// How a caption's words are lit up as they are spoken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HighlightMode {
    /// Only the word being spoken.
    #[default]
    Word,
    /// Every word spoken so far, like a karaoke line filling up.
    Karaoke,
}

/// One spoken word of a caption: its letters (with the space before it, if
/// the language uses spaces) and when it is said, measured from the start of
/// the clip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    pub start: crate::time::Ticks,
    pub end: crate::time::Ticks,
}

/// The words of a caption joined into the text that is drawn.
pub fn join_words(words: &[Word]) -> String {
    let joined: String = words.iter().map(|w| w.text.as_str()).collect();
    joined.trim_start().to_string()
}

/// The byte range of every word inside [`join_words`]'s result.
pub fn word_ranges(words: &[Word]) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(words.len());
    let mut at = 0usize;
    let mut first = true;
    for w in words {
        let text = if first {
            w.text.trim_start()
        } else {
            w.text.as_str()
        };
        first = false;
        // The space before a word is not part of it.
        let lead = text.len() - text.trim_start().len();
        out.push((at + lead, at + text.len()));
        at += text.len();
    }
    out
}

/// New words for edited text: when the number of words is unchanged the
/// timings stay; otherwise the words are spread over `length` in proportion
/// to their letters.
pub fn retime(old: &[Word], content: &str, length: crate::time::Ticks) -> Vec<Word> {
    // Each word with the whitespace before it.
    let mut pieces: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    for ch in content.chars() {
        if ch.is_whitespace() {
            if in_word {
                pieces.push(std::mem::take(&mut current));
                in_word = false;
            }
            current.push(ch);
        } else {
            current.push(ch);
            in_word = true;
        }
    }
    if in_word {
        pieces.push(current);
    }
    if pieces.len() == old.len() {
        return pieces
            .into_iter()
            .zip(old)
            .map(|(text, w)| Word {
                text,
                start: w.start,
                end: w.end,
            })
            .collect();
    }
    let total: usize = pieces.iter().map(|p| p.trim().chars().count().max(1)).sum();
    let mut done = 0usize;
    pieces
        .into_iter()
        .map(|text| {
            let n = text.trim().chars().count().max(1);
            let start = crate::time::Ticks(length.0 * done as i64 / total.max(1) as i64);
            done += n;
            let end = crate::time::Ticks(length.0 * done as i64 / total.max(1) as i64);
            Word { text, start, end }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextSpec {
    pub content: String,
    /// Font family name; empty means the app's own font.
    pub font: String,
    /// Height of a line of text, in thousandths of the canvas height.
    pub size: i32,
    pub bold: bool,
    pub italic: bool,
    pub color: Rgba,
    pub align: TextAlign,
    /// Outline thickness, in thousandths of `size` (0 = none).
    pub outline: i32,
    pub outline_color: Rgba,
    /// Drop shadow distance, in thousandths of `size` (0 = none).
    pub shadow: i32,
    /// How soft the shadow is, in thousandths of `size`.
    pub shadow_blur: i32,
    pub shadow_color: Rgba,
    /// Box behind the text; fully transparent means none.
    pub background: Rgba,
    /// Space between the text and the edge of its box, in thousandths of `size`.
    pub padding: i32,
    /// How round the box's corners are, in thousandths of the box height (500 = pill).
    pub corner: i32,
    /// Extra space between letters, in thousandths of `size` (may be negative).
    pub spacing: i32,
    /// Colour of the word being spoken (captions); fully transparent means
    /// the words are not lit up.
    pub highlight: Rgba,
    pub highlight_mode: HighlightMode,
    /// The effects can be switched off without losing their settings.
    #[serde(default = "yes")]
    pub outline_on: bool,
    #[serde(default = "yes")]
    pub shadow_on: bool,
    #[serde(default = "yes")]
    pub box_on: bool,
    /// Which part of the text is lit right now, as a byte range. Worked out
    /// for the moment being drawn ([`crate::Clip::text_at`]), never saved.
    #[serde(skip)]
    pub active: Option<(usize, usize)>,
}

fn yes() -> bool {
    true
}

impl TextSpec {
    /// Effects start switched off; a ready-made look that uses one switches
    /// it on.
    pub fn with_effects_flagged(mut self) -> TextSpec {
        self.outline_on = self.outline > 0;
        self.shadow_on = self.shadow > 0;
        self.box_on = self.background[3] > 0;
        self
    }

    /// This look with every look setting that differs between `old` and `new`
    /// set to `new`'s value (the words and everything unchanged stay as they
    /// are): how one change is shared with other clips.
    pub fn with_changes(&self, old: &TextSpec, new: &TextSpec) -> TextSpec {
        let mut out = self.clone();
        macro_rules! share {
            ($($f:ident),*) => { $( if old.$f != new.$f { out.$f = new.$f.clone(); } )* };
        }
        share!(
            font,
            size,
            bold,
            italic,
            color,
            align,
            outline,
            outline_color,
            shadow,
            shadow_blur,
            shadow_color,
            background,
            padding,
            corner,
            spacing,
            highlight,
            highlight_mode,
            outline_on,
            shadow_on,
            box_on
        );
        out
    }

    /// The look as drawn: effects that are switched off count as zero.
    pub fn effective(&self) -> TextSpec {
        let mut s = self.clone();
        if !s.outline_on {
            s.outline = 0;
        }
        if !s.shadow_on {
            s.shadow = 0;
        }
        if !s.box_on {
            s.background[3] = 0;
            s.padding = 0;
        }
        s
    }
}

impl Default for TextSpec {
    fn default() -> TextSpec {
        TextSpec {
            content: "Your text".into(),
            font: String::new(),
            size: 80,
            bold: true,
            italic: false,
            color: [255, 255, 255, 255],
            align: TextAlign::Center,
            outline: 0,
            outline_color: [0, 0, 0, 255],
            shadow: 0,
            shadow_blur: 0,
            shadow_color: [0, 0, 0, 160],
            background: [0, 0, 0, 0],
            padding: 0,
            corner: 0,
            spacing: 0,
            highlight: [0, 0, 0, 0],
            highlight_mode: HighlightMode::Word,
            outline_on: false,
            shadow_on: false,
            box_on: false,
            active: None,
        }
    }
}

impl TextSpec {
    pub const SIZE_MIN: i32 = 10;
    pub const SIZE_MAX: i32 = 600;

    /// The same text with every number brought into its legal range.
    pub fn clamped(mut self) -> TextSpec {
        // The lit part is worked out at draw time; never stored.
        self.active = None;
        self.size = self.size.clamp(Self::SIZE_MIN, Self::SIZE_MAX);
        self.outline = self.outline.clamp(0, 400);
        self.shadow = self.shadow.clamp(0, 600);
        self.shadow_blur = self.shadow_blur.clamp(0, 600);
        self.padding = self.padding.clamp(0, 1000);
        self.corner = self.corner.clamp(0, 500);
        self.spacing = self.spacing.clamp(-200, 1000);
        self
    }

    /// A short label for timelines and lists: the first line, trimmed.
    pub fn label(&self) -> String {
        let first = self.content.lines().next().unwrap_or("").trim();
        if first.is_empty() {
            "Text".into()
        } else {
            first.chars().take(40).collect()
        }
    }
}

/// A ready-made look, shown in the Text library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub name: &'static str,
    pub blurb: &'static str,
    pub spec: TextSpec,
}

/// The built-in text styles. They use only the app's own font (Inter, OFL),
/// so they look the same on every machine.
pub fn templates() -> Vec<Template> {
    raw_templates()
        .into_iter()
        .map(|mut t| {
            t.spec = t.spec.with_effects_flagged();
            t
        })
        .collect()
}

fn raw_templates() -> Vec<Template> {
    let base = TextSpec::default();
    let t = |name, blurb, spec| Template { name, blurb, spec };
    vec![
        t(
            "Plain",
            "White, bold, nothing else",
            TextSpec {
                content: "Your text".into(),
                ..base.clone()
            },
        ),
        t(
            "Big title",
            "Large with a soft shadow",
            TextSpec {
                content: "Big title".into(),
                size: 130,
                shadow: 45,
                shadow_blur: 60,
                shadow_color: [0, 0, 0, 200],
                ..base.clone()
            },
        ),
        t(
            "Outlined",
            "Yellow letters, black outline",
            TextSpec {
                content: "Outlined".into(),
                size: 110,
                color: [255, 214, 10, 255],
                outline: 70,
                ..base.clone()
            },
        ),
        t(
            "Subtitle box",
            "White on a dark box",
            TextSpec {
                content: "A line of subtitle".into(),
                size: 55,
                bold: false,
                background: [0, 0, 0, 180],
                padding: 300,
                corner: 200,
                ..base.clone()
            },
        ),
        t(
            "Lower third",
            "Name and role on a coloured bar",
            TextSpec {
                content: "Name\nRole or place".into(),
                size: 52,
                align: TextAlign::Left,
                background: [20, 118, 128, 235],
                padding: 350,
                ..base.clone()
            },
        ),
        t(
            "Neon",
            "Cyan glow",
            TextSpec {
                content: "NEON".into(),
                size: 130,
                color: [190, 255, 255, 255],
                shadow: 1,
                shadow_blur: 380,
                shadow_color: [0, 220, 255, 255],
                spacing: 120,
                ..base.clone()
            },
        ),
        t(
            "Comic pop",
            "Thick outline, tilted feel",
            TextSpec {
                content: "POW!".into(),
                size: 150,
                italic: true,
                color: [255, 240, 80, 255],
                outline: 110,
                outline_color: [200, 30, 30, 255],
                shadow: 60,
                shadow_color: [0, 0, 0, 255],
                ..base.clone()
            },
        ),
        t(
            "Minimal",
            "Thin, wide-spaced caption",
            TextSpec {
                content: "minimal".into(),
                size: 60,
                bold: false,
                spacing: 220,
                ..base.clone()
            },
        ),
        t(
            "Quote",
            "Italic, with a shadow for contrast",
            TextSpec {
                content: "\u{201c}A quote worth reading\u{201d}".into(),
                size: 75,
                bold: false,
                italic: true,
                shadow: 30,
                shadow_blur: 50,
                shadow_color: [0, 0, 0, 220],
                ..base
            },
        ),
    ]
}

/// A look for generated captions.
pub struct CaptionStyle {
    pub name: &'static str,
    pub blurb: &'static str,
    pub spec: TextSpec,
}

/// Where a caption sits: thousandths of the canvas below its centre.
pub const CAPTION_Y: i32 = 340;

/// The built-in caption looks. Highlight colour alpha 0 means no word lighting.
pub fn caption_styles() -> Vec<CaptionStyle> {
    raw_caption_styles()
        .into_iter()
        .map(|mut c| {
            c.spec = c.spec.with_effects_flagged();
            c
        })
        .collect()
}

fn raw_caption_styles() -> Vec<CaptionStyle> {
    let base = TextSpec {
        content: String::new(),
        size: 58,
        ..TextSpec::default()
    };
    let s = |name, blurb, spec| CaptionStyle { name, blurb, spec };
    vec![
        s(
            "Classic",
            "White on a dark box",
            TextSpec {
                bold: false,
                background: [0, 0, 0, 180],
                padding: 300,
                corner: 200,
                ..base.clone()
            },
        ),
        s(
            "Karaoke",
            "Words fill with yellow as they are said",
            TextSpec {
                outline: 70,
                highlight: [255, 214, 10, 255],
                highlight_mode: HighlightMode::Karaoke,
                ..base.clone()
            },
        ),
        s(
            "Pop",
            "Outlined, the spoken word lights up",
            TextSpec {
                size: 70,
                outline: 90,
                shadow: 30,
                shadow_blur: 20,
                shadow_color: [0, 0, 0, 200],
                highlight: [255, 214, 10, 255],
                ..base.clone()
            },
        ),
        s(
            "Clean",
            "White with a soft shadow",
            TextSpec {
                shadow: 40,
                shadow_blur: 60,
                shadow_color: [0, 0, 0, 210],
                ..base.clone()
            },
        ),
        s(
            "Bold highlight",
            "Dark box, the spoken word in green",
            TextSpec {
                size: 64,
                background: [0, 0, 0, 200],
                padding: 320,
                corner: 250,
                highlight: [90, 230, 120, 255],
                ..base.clone()
            },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::Ticks;

    fn w(text: &str, a: i64, b: i64) -> Word {
        Word {
            text: text.into(),
            start: Ticks(a),
            end: Ticks(b),
        }
    }

    #[test]
    fn words_join_and_find_their_place() {
        let words = vec![w(" Hello", 0, 10), w(" big", 10, 20), w(" world", 20, 30)];
        let text = join_words(&words);
        assert_eq!(text, "Hello big world");
        let r = word_ranges(&words);
        assert_eq!(&text[r[0].0..r[0].1], "Hello");
        assert_eq!(&text[r[1].0..r[1].1], "big");
        assert_eq!(&text[r[2].0..r[2].1], "world");
    }

    #[test]
    fn retime_keeps_times_when_only_spelling_changes() {
        let old = vec![w("Hello", 0, 10), w(" wrld", 10, 20)];
        let new = retime(&old, "Hello world", Ticks(20));
        assert_eq!(new.len(), 2);
        assert_eq!((new[1].start, new[1].end), (Ticks(10), Ticks(20)));
        assert_eq!(join_words(&new), "Hello world");
    }

    #[test]
    fn retime_spreads_new_words_across_the_clip() {
        let old = vec![w("Hi", 0, 10)];
        let new = retime(&old, "one two three", Ticks(300));
        assert_eq!(new.len(), 3);
        assert_eq!(new[0].start, Ticks(0));
        assert_eq!(new[2].end, Ticks(300));
        assert!(new[0].end <= new[1].start + Ticks(1));
        assert_eq!(join_words(&new), "one two three");
    }

    #[test]
    fn numbers_are_kept_in_range() {
        let t = TextSpec {
            size: 5000,
            outline: -4,
            spacing: -999,
            ..TextSpec::default()
        }
        .clamped();
        assert_eq!((t.size, t.outline, t.spacing), (600, 0, -200));
    }

    #[test]
    fn the_label_is_the_first_line() {
        let t = TextSpec {
            content: "  Hello\nworld".into(),
            ..TextSpec::default()
        };
        assert_eq!(t.label(), "Hello");
        let empty = TextSpec {
            content: "  ".into(),
            ..TextSpec::default()
        };
        assert_eq!(empty.label(), "Text");
    }

    #[test]
    fn old_projects_without_these_fields_still_load() {
        let t: TextSpec = serde_json::from_str(r#"{"content":"Hi"}"#).unwrap();
        assert_eq!(t.content, "Hi");
        assert_eq!(t.size, 80);
    }

    #[test]
    fn templates_have_unique_names_and_legal_numbers() {
        let all = templates();
        assert!(all.len() >= 8);
        let mut names: Vec<_> = all.iter().map(|t| t.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), all.len());
        for t in all {
            assert_eq!(t.spec.clone().clamped(), t.spec, "{} is in range", t.name);
            assert!(!t.spec.content.trim().is_empty());
        }
    }
}
