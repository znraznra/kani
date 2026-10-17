//! Keyboard map, toolkit-independent: keys are GDK/X11 key names ("b",
//! "space", "Delete"). The defaults follow CapCut's desktop shortcuts where
//! published guides agree, limited to what Kani can do at this stage.
//!
//! * `Origin::CapCut`: listed in published CapCut desktop shortcut guides.
//! * `Origin::Convention`: a common editor/OS convention, not verified
//!   against CapCut.
//!
//! Shortcuts for features that don't exist yet live in [`PLANNED`] and fire
//! nothing until the feature does.

use cutroom_timeline::TrackKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    NewProject,
    OpenProject,
    ImportMedia,
    ImportFolder,
    Paste,
    Copy,
    Cut,
    CopyAttributes,
    PasteAttributes,
    Quit,
    Save,
    MoveProject,
    CloseProject,
    Undo,
    Redo,
    Split,
    SplitAll,
    DeleteLeft,
    DeleteRight,
    Delete,
    RippleDelete,
    AddTrack(TrackKind),
    TogglePlay,
    StepFrames(i64),
    GoToStart,
    GoToEnd,
    ZoomIn,
    ZoomOut,
    SelectTool,
    SplitTool,
    ToggleActive,
    AddText,
    FitTimeline,
    ShowAbout,
    ShowShortcuts,
    ShowSettings,
    Export,
    ExportRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    CapCut,
    Convention,
}

#[derive(Debug, Clone, Copy)]
pub struct Binding {
    /// GDK key name, lowercase for letters.
    pub key: &'static str,
    pub ctrl: bool,
    pub shift: bool,
    pub action: Action,
    pub label: &'static str,
    pub origin: Origin,
    /// Still fires while a text box has focus (file-level commands only).
    pub while_typing: bool,
}

pub struct Planned {
    pub keys: &'static str,
    pub what: &'static str,
    pub needs: &'static str,
}

pub const PLANNED: &[Planned] = &[
    Planned {
        keys: "Ctrl+A",
        what: "Select all clips",
        needs: "multi-selection",
    },
    Planned {
        keys: "Full-screen preview",
        what: "Full-screen preview (confirm the key in CapCut's panel)",
        needs: "the compositor (Phase 4)",
    },
];

const fn b(
    key: &'static str,
    ctrl: bool,
    shift: bool,
    action: Action,
    label: &'static str,
    origin: Origin,
    while_typing: bool,
) -> Binding {
    Binding {
        key,
        ctrl,
        shift,
        action,
        label,
        origin,
        while_typing,
    }
}

use Action::*;
use Origin::{CapCut, Convention};

/// Every active binding. Lookups require the exact Ctrl and Shift state, so
/// the order does not matter.
pub const BINDINGS: &[Binding] = &[
    b("z", true, true, Redo, "Redo", CapCut, false),
    b("y", true, false, Redo, "Redo", Convention, false),
    b("z", true, false, Undo, "Undo", CapCut, false),
    b(
        "b",
        true,
        true,
        SplitAll,
        "Split all tracks at the playhead",
        CapCut,
        false,
    ),
    b(
        "b",
        true,
        false,
        Split,
        "Split the selected clip at the playhead",
        CapCut,
        false,
    ),
    b(
        "Delete",
        false,
        true,
        RippleDelete,
        "Delete and close the gap",
        Convention,
        false,
    ),
    b(
        "BackSpace",
        false,
        true,
        RippleDelete,
        "Delete and close the gap",
        Convention,
        false,
    ),
    b(
        "Delete",
        false,
        false,
        Delete,
        "Delete the selected clip",
        CapCut,
        false,
    ),
    b(
        "BackSpace",
        false,
        false,
        Delete,
        "Delete the selected clip",
        Convention,
        false,
    ),
    b(
        "q",
        false,
        false,
        DeleteLeft,
        "Delete everything left of the playhead",
        CapCut,
        false,
    ),
    b(
        "w",
        false,
        false,
        DeleteRight,
        "Delete everything right of the playhead",
        CapCut,
        false,
    ),
    b("a", false, false, SelectTool, "Select tool", CapCut, false),
    b(
        "v",
        false,
        false,
        ToggleActive,
        "Deactivate / activate the selected clips",
        CapCut,
        false,
    ),
    b(
        "t",
        false,
        false,
        AddText,
        "Add a text clip (default style) at the playhead",
        CapCut,
        false,
    ),
    b(
        "b",
        false,
        false,
        SplitTool,
        "Split tool (click a clip to cut)",
        CapCut,
        false,
    ),
    b(
        "space",
        false,
        false,
        TogglePlay,
        "Play / pause",
        CapCut,
        false,
    ),
    b(
        "Left",
        false,
        false,
        StepFrames(-1),
        "Previous frame",
        CapCut,
        false,
    ),
    b(
        "Right",
        false,
        false,
        StepFrames(1),
        "Next frame",
        CapCut,
        false,
    ),
    b(
        "Home",
        false,
        false,
        GoToStart,
        "Go to start",
        Convention,
        false,
    ),
    b("End", false, false, GoToEnd, "Go to end", Convention, false),
    b(
        "equal",
        true,
        false,
        ZoomIn,
        "Zoom in (the timeline; the preview when the pointer is over it)",
        Convention,
        false,
    ),
    b(
        "plus",
        true,
        false,
        ZoomIn,
        "Zoom in (the timeline; the preview when the pointer is over it)",
        Convention,
        false,
    ),
    b(
        "minus",
        true,
        false,
        ZoomOut,
        "Zoom out (the timeline; the preview when the pointer is over it)",
        Convention,
        false,
    ),
    b(
        "i",
        true,
        false,
        ImportMedia,
        "Import media",
        Convention,
        true,
    ),
    b(
        "c",
        true,
        false,
        Copy,
        "Copy the selected clip",
        Convention,
        false,
    ),
    b(
        "x",
        true,
        false,
        Cut,
        "Cut the selected clip",
        Convention,
        false,
    ),
    b(
        "v",
        true,
        false,
        Paste,
        "Paste a clip, or files and images from the clipboard, at the playhead",
        Convention,
        false,
    ),
    b(
        "c",
        true,
        true,
        CopyAttributes,
        "Copy the look, position and size of the selected clip",
        Convention,
        false,
    ),
    b(
        "v",
        true,
        true,
        PasteAttributes,
        "Paste copied attributes on the selected clip (same type only)",
        Convention,
        false,
    ),
    b(
        "q",
        true,
        false,
        Quit,
        "Quit (saves the open project first)",
        Convention,
        true,
    ),
    b("s", true, false, Save, "Save", Convention, true),
    b("e", true, false, Export, "Export the video", CapCut, false),
    b("comma", true, false, ShowSettings, "Settings", Convention, true),
    b(
        "x",
        true,
        true,
        ExportRange,
        "Pick a range on the timeline to export",
        Convention,
        false,
    ),
    b(
        "n",
        true,
        false,
        NewProject,
        "New project",
        Convention,
        true,
    ),
    b(
        "o",
        true,
        false,
        OpenProject,
        "Open project",
        Convention,
        true,
    ),
];

/// The action bound to `key` with exactly this Ctrl/Shift state.
/// `typing` is true while a text box has focus.
pub fn lookup(key: &str, ctrl: bool, shift: bool, typing: bool) -> Option<Action> {
    BINDINGS
        .iter()
        .find(|b| b.key == key && b.ctrl == ctrl && b.shift == shift)
        .filter(|b| !typing || b.while_typing)
        .map(|b| b.action)
}

/// Human text for a binding, e.g. `Ctrl+Shift+Z`.
pub fn display(binding: &Binding) -> String {
    let key = match binding.key {
        "space" => "Space".to_owned(),
        "equal" => "=".to_owned(),
        "plus" => "+".to_owned(),
        "minus" => "-".to_owned(),
        k if k.chars().count() == 1 => k.to_uppercase(),
        k => k.to_owned(),
    };
    let mut out = String::new();
    if binding.ctrl {
        out.push_str("Ctrl+");
    }
    if binding.shift {
        out.push_str("Shift+");
    }
    out + &key
}

/// Shortcut text for an action (first binding), empty if it has none.
pub fn text_for(action: Action) -> String {
    BINDINGS
        .iter()
        .find(|b| b.action == action)
        .map(display)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capcut_core_shortcuts() {
        assert_eq!(lookup("b", true, false, false), Some(Split));
        assert_eq!(lookup("b", true, true, false), Some(SplitAll));
        assert_eq!(lookup("space", false, false, false), Some(TogglePlay));
        assert_eq!(lookup("Delete", false, false, false), Some(Delete));
        assert_eq!(lookup("q", false, false, false), Some(DeleteLeft));
        assert_eq!(lookup("w", false, false, false), Some(DeleteRight));
        assert_eq!(lookup("a", false, false, false), Some(SelectTool));
        assert_eq!(lookup("b", false, false, false), Some(SplitTool));
        assert_eq!(lookup("Left", false, false, false), Some(StepFrames(-1)));
        assert_eq!(lookup("Right", false, false, false), Some(StepFrames(1)));
    }

    #[test]
    fn undo_and_redo_do_not_collide() {
        assert_eq!(lookup("z", true, false, false), Some(Undo));
        assert_eq!(lookup("z", true, true, false), Some(Redo));
        assert_eq!(lookup("y", true, false, false), Some(Redo));
    }

    #[test]
    fn shift_delete_is_ripple_not_plain_delete() {
        assert_eq!(lookup("Delete", false, true, false), Some(RippleDelete));
    }

    #[test]
    fn plain_keys_do_not_fire_with_ctrl_held() {
        assert_eq!(
            lookup("q", true, false, false),
            Some(Quit),
            "Ctrl+Q is quit, not delete-left"
        );
        assert_eq!(lookup("space", true, false, false), None);
        assert_eq!(lookup("w", true, false, false), None);
    }

    #[test]
    fn typing_blocks_editing_shortcuts_but_not_file_ones() {
        for (key, ctrl) in [
            ("space", false),
            ("q", false),
            ("Delete", false),
            ("BackSpace", false),
            ("z", true),
            ("b", true),
        ] {
            assert_eq!(
                lookup(key, ctrl, false, true),
                None,
                "{key} fired while typing"
            );
        }
        assert_eq!(lookup("s", true, false, true), Some(Save));
        assert_eq!(lookup("i", true, false, true), Some(ImportMedia));
        assert_eq!(lookup("c", true, false, false), Some(Copy));
        assert_eq!(lookup("x", true, false, false), Some(Cut));
        assert_eq!(lookup("v", true, false, false), Some(Paste));
        assert_eq!(
            lookup("v", true, false, true),
            None,
            "text fields keep their own paste"
        );
        assert_eq!(lookup("c", true, true, false), Some(CopyAttributes));
        assert_eq!(lookup("v", true, true, false), Some(PasteAttributes));
        assert_eq!(lookup("q", true, false, true), Some(Quit));
    }

    #[test]
    fn no_two_bindings_share_the_same_keys() {
        let mut seen = Vec::new();
        for binding in BINDINGS {
            let keys = (binding.key, binding.ctrl, binding.shift);
            assert!(
                !seen.contains(&keys),
                "duplicate binding for {}",
                binding.label
            );
            seen.push(keys);
        }
    }

    #[test]
    fn display_text_reads_naturally() {
        assert_eq!(text_for(Split), "Ctrl+B");
        assert_eq!(text_for(Redo), "Ctrl+Shift+Z");
        assert_eq!(text_for(TogglePlay), "Space");
        assert_eq!(text_for(ZoomIn), "Ctrl+=");
        assert_eq!(text_for(ShowAbout), "");
    }

    #[test]
    fn planned_shortcuts_are_not_bound() {
        assert!(!PLANNED.is_empty());
        for key in ["a"] {
            assert_eq!(
                lookup(key, true, false, false),
                None,
                "Ctrl+{key} must stay inactive"
            );
        }
    }

    #[test]
    fn ctrl_e_exports() {
        assert_eq!(lookup("e", true, false, false), Some(Export));
        assert_eq!(lookup("e", false, false, false), None);
        assert_eq!(lookup("x", true, true, false), Some(ExportRange));
        assert_eq!(lookup("x", true, false, false), Some(Cut), "Ctrl+X stays Cut");
    }

    #[test]
    fn t_adds_a_text_clip_but_not_while_typing_or_with_ctrl() {
        assert_eq!(lookup("t", false, false, false), Some(AddText));
        assert_eq!(lookup("t", false, false, true), None);
        assert_eq!(lookup("t", true, false, false), None);
    }
}
