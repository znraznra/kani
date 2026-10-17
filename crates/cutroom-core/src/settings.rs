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
    /// Background autosave on or off.
    pub autosave_enabled: bool,
    /// Minutes between background autosaves (0 also means off).
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
    /// Export: put pictures together on the graphics card? "auto" (when the
    /// last check found it works and is faster), "on" or "off".
    pub export_graphics_card: String,
    /// What the last graphics card check found: `Some(true)` it works and is
    /// faster than the processor, `Some(false)` it does not, `None` not checked.
    pub graphics_card_ok: Option<bool>,
    /// Whether the last check found a real graphics chip that works (what the
    /// Export window calls "Available"); `None` not checked.
    pub graphics_card_available: Option<bool>,
    /// The Kani version that made that check (it is made again after an update).
    pub graphics_card_checked: String,
    /// Export: make a video file / a sound-only file.
    pub export_video: bool,
    pub export_audio: bool,
    /// Export: video format by id ("h264", "h265", "vp9", "av1") and file format ("mp4" ...).
    pub export_codec: String,
    pub export_container: String,
    /// Export: sound format ("aac" ...), its bit rate in kbps and channels ("stereo", "mono").
    pub export_audio_format: String,
    pub export_audio_kbps: u32,
    pub export_channels: String,
    /// Export: your own video bit rate in kbps, used by the Custom preset.
    pub export_custom_kbps: u32,
    /// Export: crop to fill when the shape differs from the project's (else black bars).
    pub export_fill: bool,
    /// Export: caption files to save next to the video ("srt", "vtt", "ass").
    pub export_caption_files: Vec<String>,
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
            autosave_enabled: true,
            autosave_minutes: 10,
            recent_projects: Vec::new(),
            caption_language: String::new(),
            caption_model: "base".into(),
            caption_model_path: None,
            caption_style: "Pop".into(),
            export_quality: "Recommended".into(),
            export_encoder: "auto".into(),
            export_folder: None,
            export_graphics_card: "auto".into(),
            graphics_card_ok: None,
            graphics_card_available: None,
            graphics_card_checked: String::new(),
            export_video: true,
            export_audio: false,
            export_codec: "h264".into(),
            export_container: "mp4".into(),
            export_audio_format: "aac".into(),
            export_audio_kbps: 192,
            export_channels: "stereo".into(),
            export_custom_kbps: 0,
            export_fill: false,
            export_caption_files: Vec::new(),
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

    /// Minutes between autosaves, or 0 when autosave is off.
    pub fn autosave_every(&self) -> u32 {
        if self.autosave_enabled {
            self.autosave_minutes
        } else {
            0
        }
    }

    /// Should export put pictures together on the graphics card? "on" and "off"
    /// are the user's word; "auto" (or anything else) follows the last check,
    /// and stays on the processor until a check has said the card is better.
    pub fn use_graphics_card(&self) -> bool {
        match self.export_graphics_card.as_str() {
            "on" => true,
            "off" => false,
            _ => self.graphics_card_ok == Some(true),
        }
    }

    /// Does the graphics card check need making (never made, or by an older version)?
    pub fn graphics_card_needs_check(&self, version: &str) -> bool {
        self.graphics_card_ok.is_none() || self.graphics_card_checked != version
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
    fn autosave_can_be_switched_off_without_losing_the_interval() {
        let mut s = Settings::default();
        assert_eq!(s.autosave_every(), 10);
        s.autosave_enabled = false;
        assert_eq!(s.autosave_every(), 0);
        assert_eq!(s.autosave_minutes, 10, "the interval is kept");
        s.autosave_enabled = true;
        s.autosave_minutes = 0;
        assert_eq!(s.autosave_every(), 0, "an old file saying 0 still means off");
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
    fn the_graphics_card_is_used_when_asked_or_when_the_check_liked_it() {
        let mut s = Settings::default();
        assert_eq!(s.export_graphics_card, "auto");
        assert!(!s.use_graphics_card(), "not checked yet: the processor");
        assert!(s.graphics_card_needs_check("0.5.15"));
        s.graphics_card_ok = Some(true);
        s.graphics_card_checked = "0.5.15".into();
        assert!(s.use_graphics_card());
        assert!(!s.graphics_card_needs_check("0.5.15"));
        assert!(s.graphics_card_needs_check("0.5.16"), "checked again after an update");
        s.graphics_card_ok = Some(false);
        assert!(!s.use_graphics_card());
        s.export_graphics_card = "on".into();
        assert!(s.use_graphics_card(), "the user's word beats the check");
        s.graphics_card_ok = Some(true);
        s.export_graphics_card = "off".into();
        assert!(!s.use_graphics_card());
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
