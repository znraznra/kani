//! The right-click menu of a clip on the timeline. Every entry of the full
//! editor menu is listed so the layout is final; the ones that need features
//! from later phases are disabled until those exist.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClipCommand {
    Copy,
    Cut,
    CopyAttributes,
    PasteAttributes,
    Delete,
    Freeze,
    Reverse,
    Mirror,
    Rotate,
    Crop,
    SplitScenes,
    Transcript,
    GenerateCaptions,
    DontIsolateVoice,
    RemoveVocal,
    KeepVocal,
    ExtractAudio,
    AutoRemoval,
    ChromaKey,
    CustomRemoval,
    NormalizeLoudness,
    EnhanceVoice,
    ReduceNoise,
    Stabilize,
    OpticalFlow,
    AutoAdjust,
    ColorMatch,
    ColorCorrection,
    MotionBlur,
    CameraTracking,
    Group,
    PreCompose,
    SavePreset,
    ExportSelection,
    Deactivate,
    Replace,
    Trim,
    OpenFileLocation,
    RenderWholeTimeline,
    RenderIn,
    RenderOut,
}

impl ClipCommand {
    /// A stable name for menu actions, such as `extractaudio`.
    pub fn action_name(self) -> String {
        format!("{self:?}").to_lowercase()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuNode {
    Item {
        command: ClipCommand,
        label: &'static str,
        enabled: bool,
    },
    Submenu {
        label: &'static str,
        items: Vec<MenuNode>,
    },
    Separator,
}

/// What decides which entries are available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipMenuState {
    /// The clip is a video with sound that has not been extracted yet.
    pub can_extract_audio: bool,
    /// The clip's source file exists on disk.
    pub file_present: bool,
    /// The clip has a picture that can be turned or mirrored.
    pub can_transform: bool,
    /// The clip is currently deactivated (the menu then offers "Activate").
    pub deactivated: bool,
    /// Attributes were copied from a clip of the same type as this one.
    pub can_paste_attributes: bool,
}

/// The whole menu, in the order the editor shows it.
pub fn clip_menu(state: ClipMenuState) -> Vec<MenuNode> {
    use ClipCommand::*;
    let item = |command, label, enabled| MenuNode::Item {
        command,
        label,
        enabled,
    };
    let off = |command, label| item(command, label, false);
    let sub = |label, items| MenuNode::Submenu { label, items };
    vec![
        item(Copy, "Copy", true),
        item(Cut, "Cut", true),
        item(CopyAttributes, "Copy attributes", true),
        item(
            PasteAttributes,
            "Paste attributes",
            state.can_paste_attributes,
        ),
        item(Delete, "Delete", true),
        MenuNode::Separator,
        sub(
            "Edit",
            vec![
                off(Freeze, "Freeze"),
                off(Reverse, "Reverse"),
                item(Mirror, "Mirror", state.can_transform),
                item(Rotate, "Rotate", state.can_transform),
                off(Crop, "Crop"),
            ],
        ),
        off(SplitScenes, "Split scenes"),
        MenuNode::Separator,
        off(Transcript, "Transcript"),
        off(GenerateCaptions, "Generate captions"),
        MenuNode::Separator,
        sub(
            "Isolate voice",
            vec![
                off(DontIsolateVoice, "Don't isolate voice"),
                off(RemoveVocal, "Remove vocal"),
                off(KeepVocal, "Keep vocal"),
            ],
        ),
        item(ExtractAudio, "Extract audio", state.can_extract_audio),
        MenuNode::Separator,
        sub(
            "Remove background",
            vec![
                off(AutoRemoval, "Auto removal"),
                off(ChromaKey, "Chroma key"),
                off(CustomRemoval, "Custom removal"),
            ],
        ),
        sub(
            "Enhance audio",
            vec![
                off(NormalizeLoudness, "Normalize loudness"),
                off(EnhanceVoice, "Enhance voice"),
                off(ReduceNoise, "Reduce noise"),
            ],
        ),
        sub(
            "Enhance visuals",
            vec![
                off(Stabilize, "Stabilize"),
                off(OpticalFlow, "Optical flow"),
            ],
        ),
        sub(
            "Adjust color",
            vec![
                off(AutoAdjust, "Auto adjust"),
                off(ColorMatch, "Color match"),
                off(ColorCorrection, "Color correction"),
            ],
        ),
        sub(
            "Enhance motion",
            vec![
                off(MotionBlur, "Motion blur"),
                off(CameraTracking, "Camera tracking"),
            ],
        ),
        MenuNode::Separator,
        off(Group, "Group"),
        off(PreCompose, "Pre-compose"),
        off(SavePreset, "Save preset"),
        off(ExportSelection, "Export selection"),
        item(
            Deactivate,
            if state.deactivated {
                "Activate"
            } else {
                "Deactivate"
            },
            true,
        ),
        MenuNode::Separator,
        sub(
            "Clip actions",
            vec![
                off(Replace, "Replace"),
                off(Trim, "Trim"),
                item(OpenFileLocation, "Open file location", state.file_present),
            ],
        ),
        sub(
            "Render",
            vec![
                off(RenderWholeTimeline, "Whole timeline"),
                off(RenderIn, "In"),
                off(RenderOut, "Out"),
            ],
        ),
    ]
}

/// Every item, however deeply nested, as (command, label, enabled).
pub fn flatten(nodes: &[MenuNode]) -> Vec<(ClipCommand, &'static str, bool)> {
    let mut out = Vec::new();
    for node in nodes {
        match node {
            MenuNode::Item {
                command,
                label,
                enabled,
            } => out.push((*command, *label, *enabled)),
            MenuNode::Submenu { items, .. } => out.extend(flatten(items)),
            MenuNode::Separator => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const FULL: ClipMenuState = ClipMenuState {
        deactivated: false,
        can_extract_audio: true,
        file_present: true,
        can_transform: true,
        can_paste_attributes: true,
    };

    fn top_level(nodes: &[MenuNode]) -> Vec<&'static str> {
        nodes
            .iter()
            .filter_map(|n| match n {
                MenuNode::Item { label, .. } | MenuNode::Submenu { label, .. } => Some(*label),
                MenuNode::Separator => None,
            })
            .collect()
    }

    fn submenu<'a>(nodes: &'a [MenuNode], name: &str) -> Vec<&'static str> {
        nodes
            .iter()
            .find_map(|n| match n {
                MenuNode::Submenu { label, items } if *label == name => Some(flatten(items)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no submenu {name}"))
            .into_iter()
            .map(|(_, label, _)| label)
            .collect()
    }

    #[test]
    fn the_top_level_follows_the_requested_order() {
        assert_eq!(
            top_level(&clip_menu(FULL)),
            [
                "Copy",
                "Cut",
                "Copy attributes",
                "Paste attributes",
                "Delete",
                "Edit",
                "Split scenes",
                "Transcript",
                "Generate captions",
                "Isolate voice",
                "Extract audio",
                "Remove background",
                "Enhance audio",
                "Enhance visuals",
                "Adjust color",
                "Enhance motion",
                "Group",
                "Pre-compose",
                "Save preset",
                "Export selection",
                "Deactivate",
                "Clip actions",
                "Render",
            ]
        );
    }

    #[test]
    fn submenus_hold_the_requested_entries() {
        let menu = clip_menu(FULL);
        assert_eq!(
            submenu(&menu, "Edit"),
            ["Freeze", "Reverse", "Mirror", "Rotate", "Crop"]
        );
        assert_eq!(
            submenu(&menu, "Isolate voice"),
            ["Don't isolate voice", "Remove vocal", "Keep vocal"]
        );
        assert_eq!(
            submenu(&menu, "Remove background"),
            ["Auto removal", "Chroma key", "Custom removal"]
        );
        assert_eq!(
            submenu(&menu, "Enhance audio"),
            ["Normalize loudness", "Enhance voice", "Reduce noise"]
        );
        assert_eq!(
            submenu(&menu, "Enhance visuals"),
            ["Stabilize", "Optical flow"]
        );
        assert_eq!(
            submenu(&menu, "Adjust color"),
            ["Auto adjust", "Color match", "Color correction"]
        );
        assert_eq!(
            submenu(&menu, "Enhance motion"),
            ["Motion blur", "Camera tracking"]
        );
        assert_eq!(
            submenu(&menu, "Clip actions"),
            ["Replace", "Trim", "Open file location"]
        );
        assert_eq!(submenu(&menu, "Render"), ["Whole timeline", "In", "Out"]);
    }

    #[test]
    fn only_what_exists_in_this_phase_is_enabled() {
        let enabled: HashSet<ClipCommand> = flatten(&clip_menu(FULL))
            .into_iter()
            .filter(|(_, _, on)| *on)
            .map(|(c, _, _)| c)
            .collect();
        let expected: HashSet<ClipCommand> = [
            ClipCommand::Copy,
            ClipCommand::Cut,
            ClipCommand::CopyAttributes,
            ClipCommand::PasteAttributes,
            ClipCommand::Delete,
            ClipCommand::ExtractAudio,
            ClipCommand::Deactivate,
            ClipCommand::OpenFileLocation,
            ClipCommand::Mirror,
            ClipCommand::Rotate,
        ]
        .into_iter()
        .collect();
        assert_eq!(enabled, expected);
    }

    #[test]
    fn extract_audio_and_open_location_follow_the_clip() {
        let find = |state: ClipMenuState, command| {
            flatten(&clip_menu(state))
                .into_iter()
                .find(|(c, _, _)| *c == command)
                .unwrap()
                .2
        };
        let silent_missing = ClipMenuState {
            deactivated: false,
            can_extract_audio: false,
            file_present: false,
            can_transform: false,
            can_paste_attributes: false,
        };
        assert!(!find(silent_missing, ClipCommand::ExtractAudio));
        assert!(!find(silent_missing, ClipCommand::OpenFileLocation));
        assert!(find(silent_missing, ClipCommand::Copy), "copy always works");
        assert!(
            !find(silent_missing, ClipCommand::Mirror),
            "a clip without a picture cannot be mirrored"
        );
        assert!(find(FULL, ClipCommand::Rotate));
    }

    #[test]
    fn every_command_appears_exactly_once_with_a_unique_action_name() {
        let items = flatten(&clip_menu(FULL));
        assert_eq!(items.len(), 41);
        let commands: HashSet<ClipCommand> = items.iter().map(|(c, _, _)| *c).collect();
        assert_eq!(commands.len(), items.len());
        let names: HashSet<String> = items.iter().map(|(c, _, _)| c.action_name()).collect();
        assert_eq!(names.len(), items.len());
        assert!(names
            .iter()
            .all(|n| n.chars().all(|c| c.is_ascii_alphanumeric())));
    }

    #[test]
    fn separators_split_the_groups_and_never_lead_or_trail() {
        let menu = clip_menu(FULL);
        assert!(!matches!(menu.first(), Some(MenuNode::Separator)));
        assert!(!matches!(menu.last(), Some(MenuNode::Separator)));
        assert!(menu
            .windows(2)
            .all(|w| !(w[0] == MenuNode::Separator && w[1] == MenuNode::Separator)));
    }
}
