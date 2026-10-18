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

// ----- reading subtitle files ------------------------------------------------------

/// Seconds from "01:02:03,456", "01:02:03.456", "02:03.456" (WebVTT) or
/// "1:02:03.45" (Advanced SubStation).
fn parse_stamp(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, sec] => (
            h.parse::<f64>().ok()?,
            m.parse::<f64>().ok()?,
            sec.parse::<f64>().ok()?,
        ),
        [m, sec] => (0.0, m.parse::<f64>().ok()?, sec.parse::<f64>().ok()?),
        _ => return None,
    };
    Some(h * 3600.0 + m * 60.0 + sec)
}

/// Text without markup: `<i>`, `<b>`, `<c.x>`, `<00:01.000>` and
/// `{\an8}`-style blocks go; Advanced SubStation's `\N` becomes a new line.
fn clean(text: &str) -> String {
    let mut out = String::new();
    let mut depth_angle = false;
    let mut depth_brace = false;
    for c in text.chars() {
        match c {
            '<' => depth_angle = true,
            '>' if depth_angle => depth_angle = false,
            '{' => depth_brace = true,
            '}' if depth_brace => depth_brace = false,
            _ if depth_angle || depth_brace => {}
            _ => out.push(c),
        }
    }
    let out = out
        .replace("\\N", "\n")
        .replace("\\n", "\n")
        .replace("\\h", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ");
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Cues from SubRip (.srt) or WebVTT (.vtt): a "start --> end" line, then
/// the text up to a blank line.
fn parse_timed_blocks(text: &str) -> Vec<Cue> {
    let mut cues = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((a, rest)) = line.split_once("-->") else {
            continue;
        };
        // WebVTT may put settings after the end time ("00:01.000 align:start").
        let b = rest.split_whitespace().next().unwrap_or("");
        let (Some(start), Some(end)) = (parse_stamp(a), parse_stamp(b)) else {
            continue;
        };
        let mut body = Vec::new();
        while let Some(next) = lines.peek() {
            if next.trim().is_empty() {
                break;
            }
            body.push(lines.next().unwrap_or_default());
        }
        let text = clean(&body.join("\n"));
        if !text.is_empty() && end > start {
            cues.push((start, end, text));
        }
    }
    cues
}

/// Cues from Advanced SubStation Alpha (.ass / .ssa): the Dialogue lines of
/// the [Events] section, read by its Format line.
fn parse_ass(text: &str) -> Vec<Cue> {
    let mut cues = Vec::new();
    let mut in_events = false;
    let mut format: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_events = line.eq_ignore_ascii_case("[events]");
            continue;
        }
        if !in_events {
            continue;
        }
        if let Some(f) = line.strip_prefix("Format:") {
            format = f.split(',').map(|s| s.trim().to_lowercase()).collect();
            continue;
        }
        let Some(d) = line.strip_prefix("Dialogue:") else {
            continue;
        };
        if format.is_empty() {
            format = [
                "layer", "start", "end", "style", "name", "marginl", "marginr", "marginv",
                "effect", "text",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
        }
        let fields: Vec<&str> = d.splitn(format.len(), ',').map(str::trim).collect();
        let at = |name: &str| {
            format
                .iter()
                .position(|f| f == name)
                .and_then(|i| fields.get(i).copied())
        };
        let (Some(a), Some(b), Some(t)) = (at("start"), at("end"), at("text")) else {
            continue;
        };
        let (Some(start), Some(end)) = (parse_stamp(a), parse_stamp(b)) else {
            continue;
        };
        let text = clean(t);
        if !text.is_empty() && end > start {
            cues.push((start, end, text));
        }
    }
    cues.sort_by(|x, y| x.0.total_cmp(&y.0));
    cues
}

/// Read a subtitle file (.srt, .vtt, .ass or .ssa, told apart by what is in
/// it). Cues come back in time order.
pub fn parse_subtitles(text: &str) -> Result<Vec<Cue>, String> {
    let text = text.trim_start_matches('\u{feff}');
    let lower = text.to_lowercase();
    let mut cues = if lower.contains("[events]") {
        parse_ass(text)
    } else {
        parse_timed_blocks(text)
    };
    cues.sort_by(|x, y| x.0.total_cmp(&y.0));
    if cues.is_empty() {
        return Err(
            "No captions were found in this file. Kani reads .srt, .vtt, .ass and .ssa.".into(),
        );
    }
    Ok(cues)
}

/// One cue as a caption line, its words spread over the cue's time by their
/// length (subtitle files have no per-word times), so word lighting works.
pub fn cue_to_line(cue: &Cue) -> crate::lines::Line {
    let (start, end, text) = cue;
    let mut pieces: Vec<String> = Vec::new();
    for (row, line) in text.lines().enumerate() {
        for (i, word) in line.split_whitespace().enumerate() {
            let sep = if pieces.is_empty() {
                ""
            } else if i == 0 && row > 0 {
                "\n"
            } else {
                " "
            };
            pieces.push(format!("{sep}{word}"));
        }
    }
    let weight: usize = pieces.iter().map(|p| p.trim().chars().count() + 1).sum();
    let span = (end - start).max(0.0);
    let mut t = *start;
    let words = pieces
        .into_iter()
        .map(|p| {
            let share = (p.trim().chars().count() + 1) as f64 / weight.max(1) as f64 * span;
            let w = crate::TimedWord {
                text: p,
                start: t,
                end: t + share,
            };
            t += share;
            w
        })
        .collect();
    crate::lines::Line {
        start: *start,
        end: *end,
        words,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_vtt_and_ass_files_are_read() {
        let srt = "1\n00:00:01,000 --> 00:00:02,500\n<i>Hello</i> there\nfriend\n\n2\n00:00:03,000 --> 00:00:04,000\n{\\an8}Bye\n";
        let cues = parse_subtitles(srt).unwrap();
        assert_eq!(
            cues,
            vec![
                (1.0, 2.5, "Hello there\nfriend".into()),
                (3.0, 4.0, "Bye".into())
            ]
        );
        let vtt = "\u{feff}WEBVTT\n\nNOTE hi\n\n00:01.000 --> 00:02.000 align:start\nOne &amp; two\n\nx\n01:00:00.000 --> 01:00:01.000\nLate\n";
        let cues = parse_subtitles(vtt).unwrap();
        assert_eq!(
            cues,
            vec![
                (1.0, 2.0, "One & two".into()),
                (3600.0, 3601.0, "Late".into())
            ]
        );
        let ass = "[Script Info]\nTitle: x\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:05.00,0:00:06.50,Default,,0,0,0,,{\\b1}Big{\\b0}, bold\\Nline\nDialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,First\n";
        let cues = parse_subtitles(ass).unwrap();
        assert_eq!(
            cues,
            vec![
                (1.0, 2.0, "First".into()),
                (5.0, 6.5, "Big, bold\nline".into())
            ]
        );
        assert!(parse_subtitles("nothing here").is_err());
    }

    #[test]
    fn our_own_files_read_back_the_same() {
        let cues: Vec<Cue> = vec![(0.5, 1.25, "a b".into()), (2.0, 3.0, "c".into())];
        assert_eq!(parse_subtitles(&to_srt(&cues)).unwrap(), cues);
        assert_eq!(parse_subtitles(&to_vtt(&cues)).unwrap(), cues);
    }

    #[test]
    fn a_cue_becomes_a_line_with_words_spread_over_its_time() {
        let line = cue_to_line(&(1.0, 3.0, "ab cd\nef".into()));
        let texts: Vec<&str> = line.words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["ab", " cd", "\nef"]);
        assert_eq!(line.words[0].start, 1.0);
        assert!((line.words[2].end - 3.0).abs() < 1e-9);
        assert!(line
            .words
            .windows(2)
            .all(|w| (w[0].end - w[1].start).abs() < 1e-9));
    }

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
