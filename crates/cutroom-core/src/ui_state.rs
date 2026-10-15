//! What the editor looked like when the project was last closed, so reopening
//! it puts everything back: zoom and scroll, playhead, tool, selection and the
//! size of the panels. Kept in `ui-state.json` beside the project file, apart
//! from the project itself so that it can never damage it.

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FILE_NAME: &str = "ui-state.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    pub pixels_per_second: f64,
    pub scroll_seconds: f64,
    /// How far the track list is scrolled down, in pixels.
    pub track_scroll: f64,
    pub playhead_ticks: i64,
    /// "select" or "split".
    pub tool: String,
    pub snap: bool,
    pub selection: Vec<u64>,
    /// The library tab.
    pub category: usize,
    /// Divider positions: library | preview, preview | inspector, top | timeline.
    pub panes: [i32; 3],
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            pixels_per_second: 80.0,
            scroll_seconds: 0.0,
            track_scroll: 0.0,
            playhead_ticks: 0,
            tool: "select".into(),
            snap: true,
            selection: Vec::new(),
            category: 0,
            panes: [0, 0, 0],
        }
    }
}

impl UiState {
    /// The saved state of a project folder, if there is a readable one.
    pub fn load(folder: &Path) -> Option<UiState> {
        let text = std::fs::read_to_string(folder.join(FILE_NAME)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Write it next to the project; failures are ignored (it is only a convenience).
    pub fn save(&self, folder: &Path) {
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let tmp = folder.join(format!("{FILE_NAME}.tmp"));
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, folder.join(FILE_NAME));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_missing_or_broken_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(UiState::load(dir.path()).is_none());
        let s = UiState {
            pixels_per_second: 123.0,
            tool: "split".into(),
            selection: vec![4, 5],
            panes: [300, 700, 460],
            ..UiState::default()
        };
        s.save(dir.path());
        assert_eq!(UiState::load(dir.path()), Some(s));
        std::fs::write(dir.path().join(FILE_NAME), "{ nope").unwrap();
        assert!(UiState::load(dir.path()).is_none());
        // Older files with fewer fields still load.
        std::fs::write(dir.path().join(FILE_NAME), r#"{"snap": false}"#).unwrap();
        assert!(!UiState::load(dir.path()).unwrap().snap);
    }
}
