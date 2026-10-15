//! Toolkit-independent logic for the home screen: searching, sorting and
//! formatting project information.

use crate::new_project::aspect_label;
use cutroom_project::{ProjectEntry, ProjectSummary};
use cutroom_timeline::Ticks;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    Modified,
    Name,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Modified => "Last modified",
            SortKey::Name => "Name",
        }
    }
}

// ----- pure helpers ----------------------------------------------------------

/// Entries matching `query` (case-insensitive, against the name and folder
/// name), in the chosen order.
pub fn filter_and_sort<'a>(
    entries: &'a [ProjectEntry],
    query: &str,
    sort: SortKey,
) -> Vec<&'a ProjectEntry> {
    let needle = query.trim().to_lowercase();
    let mut shown: Vec<&ProjectEntry> = entries
        .iter()
        .filter(|e| {
            needle.is_empty() || {
                let folder = e
                    .path()
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                e.display_name().to_lowercase().contains(&needle) || folder.contains(&needle)
            }
        })
        .collect();
    match sort {
        SortKey::Modified => shown.sort_by(|a, b| b.modified().cmp(&a.modified())),
        SortKey::Name => shown.sort_by_key(|e| e.display_name().to_lowercase()),
    }
    shown
}

/// `0:42`, `12:05`, or `1:02:03`.
pub fn format_duration(t: Ticks) -> String {
    let total = t.as_seconds_f64().round().max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `just now`, `5 minutes ago`, `3 days ago`...
pub fn ago_text(modified: Option<SystemTime>, now: SystemTime) -> String {
    let Some(modified) = modified else {
        return "unknown".into();
    };
    let secs = now
        .duration_since(modified)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    let plural = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => plural(secs / 60, "minute"),
        3600..=86_399 => plural(secs / 3600, "hour"),
        86_400..=2_591_999 => plural(secs / 86_400, "day"),
        2_592_000..=31_535_999 => plural(secs / 2_592_000, "month"),
        _ => plural(secs / 31_536_000, "year"),
    }
}

/// `2026-10-10 14:05 UTC`.
pub fn format_utc(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rem / 3600,
        (rem / 60) % 60
    )
}

/// A stable, pleasant colour for a project's placeholder cover.
pub fn cover_tint(name: &str) -> (u8, u8, u8) {
    let mut hash: u32 = 2_166_136_261;
    for byte in name.bytes() {
        hash = (hash ^ u32::from(byte)).wrapping_mul(16_777_619);
    }
    hsv_to_rgb((hash % 360) as f32, 0.45, 0.55)
}

/// Hue in degrees, saturation and value in 0..=1.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to_byte = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (to_byte(r), to_byte(g), to_byte(b))
}

pub fn meta_line(summary: &ProjectSummary) -> String {
    format!(
        "{}  |  {:.3} fps  |  {}",
        aspect_label(summary.width, summary.height),
        summary.frame_rate.as_f64(),
        format_duration(summary.duration)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutroom_project::{
        rename_project, scan_projects, ProjectDoc, ProjectFolder, ProjectSettings,
    };
    use cutroom_timeline::FrameRate;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn make(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        let doc = ProjectDoc::new(name, ProjectSettings::vertical(FrameRate::FPS_30));
        ProjectFolder::create(&path, &doc).unwrap();
        path
    }

    fn names(entries: &[&ProjectEntry]) -> Vec<String> {
        entries.iter().map(|e| e.display_name()).collect()
    }

    struct Listing {
        entries: Vec<ProjectEntry>,
    }

    fn state_with(root: &Path) -> Listing {
        Listing {
            entries: scan_projects(root),
        }
    }

    // --- helpers

    #[test]
    fn search_matches_names_case_insensitively_and_trims() {
        let tmp = tempdir().unwrap();
        for n in ["Spring Promo", "Autumn sale", "Promo Reel"] {
            make(tmp.path(), n);
        }
        let state = state_with(tmp.path());
        let hits = filter_and_sort(&state.entries, "  PROMO ", SortKey::Name);
        assert_eq!(names(&hits), ["Promo Reel", "Spring Promo"]);
        assert_eq!(filter_and_sort(&state.entries, "", SortKey::Name).len(), 3);
        assert!(filter_and_sort(&state.entries, "zzz", SortKey::Name).is_empty());
    }

    #[test]
    fn search_also_matches_the_folder_name() {
        let tmp = tempdir().unwrap();
        let path = make(tmp.path(), "folder-name");
        rename_project(&path, "Totally Different", false).unwrap();
        let state = state_with(tmp.path());
        assert_eq!(
            filter_and_sort(&state.entries, "folder-name", SortKey::Name).len(),
            1
        );
        assert_eq!(
            filter_and_sort(&state.entries, "different", SortKey::Name).len(),
            1
        );
    }

    #[test]
    fn name_sort_ignores_case() {
        let tmp = tempdir().unwrap();
        for n in ["beta", "Alpha", "gamma"] {
            make(tmp.path(), n);
        }
        let state = state_with(tmp.path());
        let all = filter_and_sort(&state.entries, "", SortKey::Name);
        assert_eq!(names(&all), ["Alpha", "beta", "gamma"]);
    }

    #[test]
    fn modified_sort_puts_the_newest_first() {
        let t = |s| Some(UNIX_EPOCH + Duration::from_secs(s));
        let entry = |name: &str, m| ProjectEntry::Broken {
            path: PathBuf::from(name),
            error: String::new(),
            modified: m,
        };
        let entries = vec![
            entry("old", t(5)),
            entry("new", t(500)),
            entry("none", None),
        ];
        let sorted = filter_and_sort(&entries, "", SortKey::Modified);
        assert_eq!(names(&sorted), ["new", "old", "none"]);
    }

    #[test]
    fn durations_sizes_and_times_read_naturally() {
        assert_eq!(format_duration(Ticks::from_seconds_f64(42.0)), "0:42");
        assert_eq!(format_duration(Ticks::from_seconds_f64(725.0)), "12:05");
        assert_eq!(format_duration(Ticks::from_seconds_f64(3723.0)), "1:02:03");
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(5 * 1024 * 1024 * 1024), "5.0 GB");

        let now = UNIX_EPOCH + Duration::from_secs(10_000_000);
        let ago = |s| ago_text(Some(now - Duration::from_secs(s)), now);
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(60), "1 minute ago");
        assert_eq!(ago(7200), "2 hours ago");
        assert_eq!(ago(86_400), "1 day ago");
        assert_eq!(ago(86_400 * 3), "3 days ago");
        assert_eq!(ago(2_592_000 * 2), "2 months ago");
        assert_eq!(ago_text(None, now), "unknown");
        assert_eq!(
            ago_text(Some(now + Duration::from_secs(99)), now),
            "just now",
            "clock skew is harmless"
        );
    }

    #[test]
    fn utc_formatting_is_correct_across_edge_cases() {
        assert_eq!(format_utc(UNIX_EPOCH), "1970-01-01 00:00 UTC");
        // 2026-10-10 14:05:00 UTC
        assert_eq!(
            format_utc(UNIX_EPOCH + Duration::from_secs(1_791_641_100)),
            "2026-10-10 14:05 UTC"
        );
        // Leap day: 2024-02-29 12:00:00 UTC
        assert_eq!(
            format_utc(UNIX_EPOCH + Duration::from_secs(1_709_208_000)),
            "2024-02-29 12:00 UTC"
        );
    }

    #[test]
    fn covers_have_stable_distinct_colours() {
        assert_eq!(cover_tint("Promo"), cover_tint("Promo"));
        assert_ne!(cover_tint("Promo"), cover_tint("Sale"));
    }
}
