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

/// How captions look in an Advanced SubStation Alpha (.ass) file.
#[derive(Debug, Clone, PartialEq)]
pub struct AssStyle {
    pub font: String,
    /// Font size in pixels of the picture the file is made for.
    pub size: u32,
    pub bold: bool,
    pub italic: bool,
    /// Colours as red, green, blue.
    pub text: [u8; 3],
    pub outline: [u8; 3],
    /// Outline thickness and shadow distance, in pixels.
    pub outline_px: f32,
    pub shadow_px: f32,
    /// Space between the bottom edge and the text, in pixels.
    pub margin_bottom: u32,
}

impl Default for AssStyle {
    fn default() -> Self {
        AssStyle {
            font: "Arial".into(),
            size: 48,
            bold: true,
            italic: false,
            text: [255, 255, 255],
            outline: [0, 0, 0],
            outline_px: 2.0,
            shadow_px: 0.0,
            margin_bottom: 60,
        }
    }
}

fn ass_stamp(secs: f64) -> String {
    let cs = (secs.max(0.0) * 100.0).round() as u64;
    format!(
        "{}:{:02}:{:02}.{:02}",
        cs / 360_000,
        cs / 6000 % 60,
        cs / 100 % 60,
        cs % 100
    )
}

/// ASS colours are blue, green, red.
fn ass_colour(rgb: [u8; 3]) -> String {
    format!("&H00{:02X}{:02X}{:02X}", rgb[2], rgb[1], rgb[0])
}

/// A caption file in the Advanced SubStation Alpha format, for a picture
/// `width` x `height` pixels in size.
pub fn to_ass(cues: &[Cue], style: &AssStyle, width: u32, height: u32) -> String {
    let mut out = String::from("[Script Info]\nTitle: Kani captions\nScriptType: v4.00+\nWrapStyle: 0\nScaledBorderAndShadow: yes\n");
    out.push_str(&format!("PlayResX: {width}\nPlayResY: {height}\n\n"));
    out.push_str("[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n");
    out.push_str(&format!(
        "Style: Default,{},{},{},{},{},&H80000000,{},{},0,0,100,100,0,0,1,{:.1},{:.1},2,40,40,{},1\n\n",
        style.font.replace(',', " "),
        style.size,
        ass_colour(style.text),
        ass_colour(style.text),
        ass_colour(style.outline),
        if style.bold { -1 } else { 0 },
        if style.italic { -1 } else { 0 },
        style.outline_px,
        style.shadow_px,
        style.margin_bottom
    ));
    out.push_str("[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
    for (a, b, text) in cues {
        // Line breaks are \N; braces would start an override block.
        let text = text
            .replace('{', "(")
            .replace('}', ")")
            .replace("\r\n", "\\N")
            .replace('\n', "\\N");
        out.push_str(&format!(
            "Dialogue: 0,{},{},Default,,0,0,0,,{}\n",
            ass_stamp(*a),
            ass_stamp(*b),
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

    #[test]
    fn ass_has_a_style_and_one_dialogue_line_per_cue() {
        let cues = vec![
            (0.0, 1.5, "Hi {there}".to_string()),
            (3723.25, 3725.0, "Two\nlines".to_string()),
        ];
        let style = AssStyle {
            text: [255, 200, 0],
            ..AssStyle::default()
        };
        let ass = to_ass(&cues, &style, 1080, 1920);
        assert!(ass.starts_with("[Script Info]"));
        assert!(ass.contains("PlayResX: 1080\nPlayResY: 1920"));
        assert!(ass.contains("Style: Default,Arial,48,&H0000C8FF,"), "{ass}");
        assert!(ass.contains("Dialogue: 0,0:00:00.00,0:00:01.50,Default,,0,0,0,,Hi (there)"));
        assert!(ass.contains("Dialogue: 0,1:02:03.25,1:02:05.00,Default,,0,0,0,,Two\\Nlines"));
        assert_eq!(ass.matches("Dialogue:").count(), 2);
    }
}
