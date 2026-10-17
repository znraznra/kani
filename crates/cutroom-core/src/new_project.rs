//! The "New project" form: canvas presets, custom sizes, frame rates, location.

use cutroom_project::{folder_name_problem, ProjectSettings};
use cutroom_timeline::FrameRate;
use std::path::PathBuf;

pub struct CanvasPreset {
    pub label: &'static str,
    pub width: u32,
    pub height: u32,
}

pub const CANVAS_PRESETS: &[CanvasPreset] = &[
    CanvasPreset {
        label: "Vertical 9:16  1080 x 1920  (Reels, Shorts, TikTok)",
        width: 1080,
        height: 1920,
    },
    CanvasPreset {
        label: "Vertical 9:16  2160 x 3840  (4K)",
        width: 2160,
        height: 3840,
    },
    CanvasPreset {
        label: "Portrait 4:5  1080 x 1350  (Instagram feed)",
        width: 1080,
        height: 1350,
    },
    CanvasPreset {
        label: "Portrait 3:4  1080 x 1440",
        width: 1080,
        height: 1440,
    },
    CanvasPreset {
        label: "Square 1:1  1080 x 1080",
        width: 1080,
        height: 1080,
    },
    CanvasPreset {
        label: "Horizontal 16:9  1920 x 1080  (HD)",
        width: 1920,
        height: 1080,
    },
    CanvasPreset {
        label: "Horizontal 16:9  3840 x 2160  (4K)",
        width: 3840,
        height: 2160,
    },
    CanvasPreset {
        label: "Standard 4:3  1440 x 1080",
        width: 1440,
        height: 1080,
    },
    CanvasPreset {
        label: "Cinematic 21:9  2560 x 1080",
        width: 2560,
        height: 1080,
    },
];

pub const FRAME_RATES: [(&str, FrameRate); 10] = [
    ("23.976", FrameRate::NTSC_23_976),
    ("24", FrameRate::FPS_24),
    ("25", FrameRate::FPS_25),
    ("29.97", FrameRate::NTSC_29_97),
    ("30", FrameRate::FPS_30),
    ("48", FrameRate::new(48, 1)),
    ("50", FrameRate::FPS_50),
    ("59.94", FrameRate::NTSC_59_94),
    ("60", FrameRate::FPS_60),
    ("120", FrameRate::new(120, 1)),
];

/// Smallest and largest custom canvas edge, in pixels.
pub const CUSTOM_MIN: u32 = 64;
pub const CUSTOM_MAX: u32 = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasChoice {
    Preset(usize),
    Custom,
}

#[derive(Clone)]
pub struct NewProjectForm {
    pub name: String,
    /// Folder the project folder will be created in.
    pub location: PathBuf,
    /// The default projects folder, so "Use default" can go back to it.
    pub default_location: PathBuf,
    pub canvas: CanvasChoice,
    pub custom_width: u32,
    pub custom_height: u32,
    pub rate_index: usize,
    pub error: Option<String>,
}

/// Round to the nearest even number within the allowed range. Most video
/// codecs need even dimensions.
pub fn clamp_even(value: u32) -> u32 {
    let clamped = value.clamp(CUSTOM_MIN, CUSTOM_MAX);
    clamped - (clamped % 2)
}

/// `9:16`, `16:9`, `4:5`; very unusual shapes fall back to a decimal like `2.37:1`.
pub fn aspect_label(width: u32, height: u32) -> String {
    fn gcd(a: u32, b: u32) -> u32 {
        if b == 0 {
            a
        } else {
            gcd(b, a % b)
        }
    }
    if width == 0 || height == 0 {
        return "-".into();
    }
    let g = gcd(width, height);
    let (w, h) = (width / g, height / g);
    if w <= 32 && h <= 32 {
        format!("{w}:{h}")
    } else if width >= height {
        format!("{:.2}:1", width as f64 / height as f64)
    } else {
        format!("1:{:.2}", height as f64 / width as f64)
    }
}

impl NewProjectForm {
    pub fn new(default_location: PathBuf) -> Self {
        NewProjectForm {
            name: "My project".into(),
            location: default_location.clone(),
            default_location,
            canvas: CanvasChoice::Preset(0),
            custom_width: 1080,
            custom_height: 1920,
            rate_index: 4, // 30 fps
            error: None,
        }
    }

    /// Final canvas size after clamping and rounding.
    pub fn size(&self) -> (u32, u32) {
        match self.canvas {
            CanvasChoice::Preset(i) => {
                let preset = &CANVAS_PRESETS[i.min(CANVAS_PRESETS.len() - 1)];
                (preset.width, preset.height)
            }
            CanvasChoice::Custom => (
                clamp_even(self.custom_width),
                clamp_even(self.custom_height),
            ),
        }
    }

    pub fn frame_rate(&self) -> FrameRate {
        FRAME_RATES[self.rate_index.min(FRAME_RATES.len() - 1)].1
    }

    pub fn settings(&self) -> ProjectSettings {
        let (width, height) = self.size();
        ProjectSettings {
            width,
            height,
            frame_rate: self.frame_rate(),
            sample_rate: 48_000,
            color_space: Default::default(),
        }
    }

    pub fn project_root(&self) -> PathBuf {
        self.location.join(self.name.trim())
    }

    pub fn name_problem(&self) -> Option<&'static str> {
        folder_name_problem(&self.name)
    }

    pub fn folder_taken(&self) -> bool {
        self.name_problem().is_none() && self.project_root().exists()
    }

    pub fn can_create(&self) -> bool {
        self.name_problem().is_none() && !self.folder_taken()
    }

    pub fn is_default_location(&self) -> bool {
        self.location == self.default_location
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn form() -> NewProjectForm {
        NewProjectForm::new(PathBuf::from("/projects"))
    }

    #[test]
    fn defaults_are_vertical_30fps_in_the_default_folder() {
        let f = form();
        assert_eq!(f.size(), (1080, 1920));
        assert_eq!(f.frame_rate(), FrameRate::FPS_30);
        assert!(f.is_default_location());
        assert_eq!(f.settings().sample_rate, 48_000);
    }

    #[test]
    fn every_preset_has_even_dimensions_and_a_label() {
        for preset in CANVAS_PRESETS {
            assert!(
                preset.width % 2 == 0 && preset.height % 2 == 0,
                "{}",
                preset.label
            );
            assert!(preset.width >= CUSTOM_MIN && preset.height >= CUSTOM_MIN);
            assert!(!preset.label.is_empty());
        }
    }

    #[test]
    fn preset_labels_match_their_sizes() {
        for preset in CANVAS_PRESETS {
            let text = format!("{} x {}", preset.width, preset.height);
            assert!(preset.label.contains(&text), "{} vs {text}", preset.label);
        }
    }

    #[test]
    fn every_frame_rate_has_an_exact_tick_length() {
        for (label, rate) in FRAME_RATES {
            assert!(rate.ticks_per_frame().is_some(), "{label} fps is not exact");
        }
    }

    #[test]
    fn frame_rate_labels_match_the_values() {
        for (label, rate) in FRAME_RATES {
            let shown: f64 = label.parse().unwrap();
            assert!((rate.as_f64() - shown).abs() < 0.01, "{label}");
        }
    }

    #[test]
    fn custom_sizes_are_clamped_and_made_even() {
        assert_eq!(clamp_even(1081), 1080);
        assert_eq!(clamp_even(1920), 1920);
        assert_eq!(clamp_even(3), CUSTOM_MIN);
        assert_eq!(clamp_even(100_000), CUSTOM_MAX);
        let mut f = form();
        f.canvas = CanvasChoice::Custom;
        f.custom_width = 1001;
        f.custom_height = 1999;
        assert_eq!(f.size(), (1000, 1998));
    }

    #[test]
    fn choosing_a_preset_uses_its_size() {
        let mut f = form();
        f.canvas = CanvasChoice::Preset(5);
        assert_eq!(f.size(), (1920, 1080));
        f.canvas = CanvasChoice::Preset(999); // out of range is safe
        assert_eq!(f.size(), {
            let last = CANVAS_PRESETS.last().unwrap();
            (last.width, last.height)
        });
    }

    #[test]
    fn aspect_labels() {
        assert_eq!(aspect_label(1080, 1920), "9:16");
        assert_eq!(aspect_label(1920, 1080), "16:9");
        assert_eq!(aspect_label(1080, 1080), "1:1");
        assert_eq!(aspect_label(1080, 1350), "4:5");
        assert_eq!(aspect_label(1440, 1080), "4:3");
        assert_eq!(aspect_label(2560, 1080), "2.37:1");
        assert_eq!(aspect_label(1000, 3001), "1:3.00");
        assert_eq!(aspect_label(0, 10), "-");
    }

    #[test]
    fn frame_rate_selection_is_applied() {
        let mut f = form();
        f.rate_index = 0;
        assert_eq!(f.frame_rate(), FrameRate::NTSC_23_976);
        f.rate_index = FRAME_RATES.len() - 1;
        assert_eq!(f.settings().frame_rate, FrameRate::new(120, 1));
        f.rate_index = 999;
        assert_eq!(
            f.frame_rate(),
            FrameRate::new(120, 1),
            "out of range is safe"
        );
    }

    #[test]
    fn names_follow_the_shared_rules_and_existing_folders_block_creation() {
        let tmp = tempdir().unwrap();
        let mut f = NewProjectForm::new(tmp.path().to_path_buf());
        f.name = "Promo".into();
        assert!(f.can_create());
        std::fs::create_dir_all(tmp.path().join("Promo")).unwrap();
        assert!(f.folder_taken());
        assert!(!f.can_create());

        f.name = "bad/name".into();
        assert!(f.name_problem().is_some());
        assert!(!f.can_create());
        assert!(
            !f.folder_taken(),
            "an invalid name is reported as invalid, not as taken"
        );
    }

    #[test]
    fn changing_the_location_is_remembered_as_non_default() {
        let mut f = form();
        f.location = PathBuf::from("/elsewhere");
        assert!(!f.is_default_location());
        assert_eq!(f.project_root(), PathBuf::from("/elsewhere/My project"));
    }
}
