//! User preferences, stored in the OS config directory (never in a project).

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "settings.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    /// Follow the operating system's light/dark setting.
    #[default]
    System,
    Dark,
    Light,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Dark, ThemeChoice::Light];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "Follow system",
            ThemeChoice::Dark => "Dark",
            ThemeChoice::Light => "Light",
        }
    }
}

/// Thumbnail grid or compact list; used by the home screen and the media library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    #[default]
    Thumbnails,
    List,
}

/// Longest list of remembered projects kept outside the default folder.
const MAX_RECENT: usize = 30;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// When files are dropped onto the window, also place them on the timeline.
    pub add_dropped_to_timeline: bool,
    /// Play proxy files in the preview when they exist (export always uses the originals).
    pub use_proxies: bool,
    /// Where new projects are created and listed. `None` means the default
    /// (`Documents/Kani`).
    pub projects_dir: Option<PathBuf>,
    pub home_view: ViewMode,
    pub library_view: ViewMode,
    /// Minutes between background autosaves; 0 turns autosave off.
    pub autosave_minutes: u32,
    /// Projects opened from outside the projects folder, newest first.
    pub recent_projects: Vec<PathBuf>,
    /// Auto-captions: spoken language (Whisper code, empty = detect).
    pub caption_language: String,
    /// Auto-captions: model id ("tiny", "base", "small") or "custom".
    pub caption_model: String,
    /// The model file used when `caption_model` is "custom".
    pub caption_model_path: Option<PathBuf>,
    /// Auto-captions: look, by name.
    pub caption_style: String,
    /// Export: picture quality by name ("Recommended" ...).
    pub export_quality: String,
    /// Export: the video encoder's id, or "auto" (the best one that works).
    pub export_encoder: String,
    /// Export: the folder the last video was saved in.
    pub export_folder: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeChoice::System,
            add_dropped_to_timeline: false,
            use_proxies: true,
            projects_dir: None,
            home_view: ViewMode::Thumbnails,
            library_view: ViewMode::List,
            autosave_minutes: 10,
            recent_projects: Vec::new(),
            caption_language: String::new(),
            caption_model: "base".into(),
            caption_model_path: None,
            caption_style: "Pop".into(),
            export_quality: "Recommended".into(),
            export_encoder: "auto".into(),
            export_folder: None,
        }
    }
}

impl Settings {
    /// The folder projects live in: the user's choice, else `default`.
    pub fn projects_dir_or(&self, default: &Path) -> PathBuf {
        self.projects_dir
            .clone()
            .unwrap_or_else(|| default.to_path_buf())
    }

    /// Remember a project opened from anywhere, most recent first.
    pub fn remember_project(&mut self, path: &Path) {
        self.recent_projects.retain(|p| p != path);
        self.recent_projects.insert(0, path.to_path_buf());
        self.recent_projects.truncate(MAX_RECENT);
    }

    pub fn forget_project(&mut self, path: &Path) {
        self.recent_projects.retain(|p| p != path);
    }

    /// Missing or unreadable settings fall back to defaults; they are never fatal.
    pub fn load(config_dir: &Path) -> Settings {
        fs::read(config_dir.join(FILE_NAME))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Atomic write: temp file, flush, rename.
    pub fn save(&self, config_dir: &Path) -> std::io::Result<()> {
        fs::create_dir_all(config_dir)?;
        let tmp = config_dir.join(format!("{FILE_NAME}.tmp"));
        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            file.sync_all()?;
        }
        fs::rename(tmp, config_dir.join(FILE_NAME))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn defaults_follow_the_system_theme() {
        let settings = Settings::default();
        assert_eq!(settings.theme, ThemeChoice::System);
        assert!(!settings.add_dropped_to_timeline);
        assert!(settings.use_proxies, "proxies are used when they exist");
    }

    #[test]
    fn save_then_load_roundtrips() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("config/cutroom");
        let settings = Settings {
            theme: ThemeChoice::Light,
            add_dropped_to_timeline: true,
            ..Settings::default()
        };
        settings.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), settings);
        assert!(!dir.join("settings.json.tmp").exists());
    }

    #[test]
    fn missing_file_gives_defaults() {
        let tmp = tempdir().unwrap();
        assert_eq!(Settings::load(tmp.path()), Settings::default());
    }

    #[test]
    fn corrupt_file_gives_defaults_instead_of_failing() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("settings.json"), b"{ not json").unwrap();
        assert_eq!(Settings::load(tmp.path()), Settings::default());
    }

    #[test]
    fn partial_and_unknown_fields_are_tolerated() {
        let tmp = tempdir().unwrap();
        fs::write(
            tmp.path().join("settings.json"),
            br#"{"theme":"dark","from_a_future_version":42}"#,
        )
        .unwrap();
        let loaded = Settings::load(tmp.path());
        assert_eq!(loaded.theme, ThemeChoice::Dark);
        assert!(!loaded.add_dropped_to_timeline);
    }

    #[test]
    fn every_choice_has_a_label() {
        for choice in ThemeChoice::ALL {
            assert!(!choice.label().is_empty());
        }
    }

    #[test]
    fn new_settings_have_sensible_defaults() {
        let s = Settings::default();
        assert_eq!(s.autosave_minutes, 10);
        assert_eq!(s.home_view, ViewMode::Thumbnails);
        assert_eq!(s.library_view, ViewMode::List);
        assert!(s.projects_dir.is_none() && s.recent_projects.is_empty());
    }

    #[test]
    fn an_old_settings_file_keeps_its_values_and_gets_new_defaults() {
        let tmp = tempdir().unwrap();
        fs::write(
            tmp.path().join("settings.json"),
            br#"{"theme":"light","add_dropped_to_timeline":true}"#,
        )
        .unwrap();
        let loaded = Settings::load(tmp.path());
        assert_eq!(loaded.theme, ThemeChoice::Light);
        assert!(loaded.add_dropped_to_timeline);
        assert_eq!(
            loaded.autosave_minutes, 10,
            "missing fields use real defaults, not zero"
        );
    }

    #[test]
    fn projects_dir_falls_back_to_the_default() {
        let mut s = Settings::default();
        let default = Path::new("/docs/Kani");
        assert_eq!(s.projects_dir_or(default), default);
        s.projects_dir = Some(PathBuf::from("/elsewhere"));
        assert_eq!(s.projects_dir_or(default), PathBuf::from("/elsewhere"));
    }

    #[test]
    fn recent_projects_are_deduplicated_newest_first_and_capped() {
        let mut s = Settings::default();
        s.remember_project(Path::new("/a"));
        s.remember_project(Path::new("/b"));
        s.remember_project(Path::new("/a"));
        assert_eq!(
            s.recent_projects,
            [PathBuf::from("/a"), PathBuf::from("/b")]
        );
        for i in 0..50 {
            s.remember_project(Path::new(&format!("/p{i}")));
        }
        assert_eq!(s.recent_projects.len(), 30);
        assert_eq!(s.recent_projects[0], PathBuf::from("/p49"));
        s.forget_project(Path::new("/p49"));
        assert_eq!(s.recent_projects[0], PathBuf::from("/p48"));
    }

    #[test]
    fn the_full_settings_roundtrip() {
        let tmp = tempdir().unwrap();
        let mut s = Settings {
            theme: ThemeChoice::Dark,
            projects_dir: Some(PathBuf::from("/my/projects")),
            home_view: ViewMode::List,
            library_view: ViewMode::Thumbnails,
            autosave_minutes: 5,
            ..Settings::default()
        };
        s.remember_project(Path::new("/x"));
        s.save(tmp.path()).unwrap();
        assert_eq!(Settings::load(tmp.path()), s);
    }
}
