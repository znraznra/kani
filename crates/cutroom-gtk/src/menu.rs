//! The menu bar and its actions.

use crate::app::App;
use cutroom_core::settings::ThemeChoice;
use cutroom_core::shortcuts::{Action, BINDINGS};
use cutroom_timeline::TrackKind;
use gtk4 as gtk;
use gtk4::gio;
use gtk4::prelude::*;
use std::rc::Rc;

pub struct MenuUi {
    pub bar: gtk::PopoverMenuBar,
    actions: Vec<(Action, gio::SimpleAction)>,
    theme: gio::SimpleAction,
    autosave: gio::SimpleAction,
}

/// GTK's name for the action, used in menu models.
fn name_of(action: Action) -> Option<&'static str> {
    Some(match action {
        Action::NewProject => "new-project",
        Action::OpenProject => "open-project",
        Action::ImportMedia => "import-media",
        Action::ImportFolder => "import-folder",
        Action::Paste => "paste",
        Action::Copy => "copy",
        Action::Cut => "cut",
        Action::CopyAttributes => "copy-attributes",
        Action::PasteAttributes => "paste-attributes",
        Action::Quit => "quit",
        Action::Save => "save",
        Action::MoveProject => "move-project",
        Action::CloseProject => "close-project",
        Action::Undo => "undo",
        Action::Redo => "redo",
        Action::Split => "split",
        Action::SplitAll => "split-all",
        Action::DeleteLeft => "delete-left",
        Action::DeleteRight => "delete-right",
        Action::Delete => "delete",
        Action::RippleDelete => "ripple-delete",
        Action::AddTrack(TrackKind::Video) => "add-video-track",
        Action::AddTrack(TrackKind::Audio) => "add-audio-track",
        Action::SelectTool => "select-tool",
        Action::SplitTool => "split-tool",
        Action::ZoomIn => "zoom-in",
        Action::ZoomOut => "zoom-out",
        Action::FitTimeline => "fit-timeline",
        Action::ShowAbout => "about",
        Action::ShowShortcuts => "show-shortcuts",
        Action::Export => "export",
        Action::ExportRange => "export-range",
        _ => return None,
    })
}

/// `<Control><Shift>z` style text for showing a shortcut beside a menu item.
fn accel_for(action: Action) -> Option<String> {
    let binding = BINDINGS.iter().find(|b| b.action == action)?;
    if binding.key == "plus" {
        return None;
    }
    Some(format!(
        "{}{}{}",
        if binding.ctrl { "<Control>" } else { "" },
        if binding.shift { "<Shift>" } else { "" },
        binding.key
    ))
}

fn item(menu: &gio::Menu, label: &str, action: Action) {
    let Some(name) = name_of(action) else { return };
    let entry = gio::MenuItem::new(Some(label), Some(&format!("win.{name}")));
    if let Some(accel) = accel_for(action) {
        entry.set_attribute_value("accel", Some(&accel.to_variant()));
    }
    menu.append_item(&entry);
}

pub fn build(app: &Rc<App>) -> MenuUi {
    let mut actions = Vec::new();
    let mut register = |action: Action| {
        let Some(name) = name_of(action) else { return };
        let simple = gio::SimpleAction::new(name, None);
        let handler_app = app.clone();
        simple.connect_activate(move |_, _| handler_app.run_action(action));
        app.window.add_action(&simple);
        actions.push((action, simple));
    };
    for action in [
        Action::NewProject,
        Action::OpenProject,
        Action::ImportMedia,
        Action::ImportFolder,
        Action::Paste,
        Action::Copy,
        Action::Cut,
        Action::CopyAttributes,
        Action::PasteAttributes,
        Action::Quit,
        Action::Save,
        Action::MoveProject,
        Action::CloseProject,
        Action::Undo,
        Action::Redo,
        Action::Split,
        Action::SplitAll,
        Action::DeleteLeft,
        Action::DeleteRight,
        Action::Delete,
        Action::RippleDelete,
        Action::AddTrack(TrackKind::Video),
        Action::AddTrack(TrackKind::Audio),
        Action::SelectTool,
        Action::SplitTool,
        Action::ZoomIn,
        Action::ZoomOut,
        Action::FitTimeline,
        Action::ShowAbout,
        Action::ShowShortcuts,
        Action::Export,
        Action::ExportRange,
    ] {
        register(action);
    }

    let theme = gio::SimpleAction::new_stateful(
        "theme",
        Some(gtk::glib::VariantTy::STRING),
        &"system".to_variant(),
    );
    {
        let app = app.clone();
        theme.connect_activate(move |_, param| {
            let Some(text) = param.and_then(|p| p.get::<String>()) else {
                return;
            };
            let choice = match text.as_str() {
                "dark" => ThemeChoice::Dark,
                "light" => ThemeChoice::Light,
                _ => ThemeChoice::System,
            };
            app.set_theme(choice);
        });
    }
    app.window.add_action(&theme);

    let autosave = gio::SimpleAction::new_stateful(
        "autosave",
        Some(gtk::glib::VariantTy::STRING),
        &"10".to_variant(),
    );
    {
        let app = app.clone();
        autosave.connect_activate(move |_, param| {
            if let Some(minutes) = param
                .and_then(|p| p.get::<String>())
                .and_then(|s| s.parse::<u32>().ok())
            {
                app.set_autosave_minutes(minutes);
                crate::menu::update(&app);
            }
        });
    }
    app.window.add_action(&autosave);

    let model = gio::Menu::new();

    let file = gio::Menu::new();
    item(&file, "New project...", Action::NewProject);
    item(&file, "Open project...", Action::OpenProject);
    let import = gio::Menu::new();
    item(&import, "Import media...", Action::ImportMedia);
    item(&import, "Import folder...", Action::ImportFolder);
    file.append_section(None, &import);
    let project = gio::Menu::new();
    item(&project, "Save", Action::Save);
    item(&project, "Move project...", Action::MoveProject);
    item(
        &project,
        "Close project (back to home)",
        Action::CloseProject,
    );
    file.append_section(None, &project);
    let output = gio::Menu::new();
    item(&output, "Export video...", Action::Export);
    item(&output, "Export range...", Action::ExportRange);
    file.append_section(None, &output);
    let quit = gio::Menu::new();
    item(&quit, "Quit", Action::Quit);
    file.append_section(None, &quit);
    model.append_submenu(Some("File"), &file);

    let edit = gio::Menu::new();
    let history = gio::Menu::new();
    item(&history, "Undo", Action::Undo);
    item(&history, "Redo", Action::Redo);
    edit.append_section(None, &history);
    let clipboard = gio::Menu::new();
    item(&clipboard, "Cut", Action::Cut);
    item(&clipboard, "Copy", Action::Copy);
    item(&clipboard, "Paste", Action::Paste);
    item(&clipboard, "Copy attributes", Action::CopyAttributes);
    item(&clipboard, "Paste attributes", Action::PasteAttributes);
    edit.append_section(None, &clipboard);
    let cutting = gio::Menu::new();
    item(&cutting, "Split at playhead", Action::Split);
    item(&cutting, "Split all tracks at playhead", Action::SplitAll);
    item(&cutting, "Delete left of playhead", Action::DeleteLeft);
    item(&cutting, "Delete right of playhead", Action::DeleteRight);
    item(&cutting, "Delete", Action::Delete);
    item(&cutting, "Ripple delete", Action::RippleDelete);
    edit.append_section(None, &cutting);
    let tools = gio::Menu::new();
    item(&tools, "Select tool", Action::SelectTool);
    item(&tools, "Split tool", Action::SplitTool);
    edit.append_section(None, &tools);
    let tracks = gio::Menu::new();
    item(
        &tracks,
        "Add video track",
        Action::AddTrack(TrackKind::Video),
    );
    item(
        &tracks,
        "Add audio track",
        Action::AddTrack(TrackKind::Audio),
    );
    edit.append_section(None, &tracks);
    model.append_submenu(Some("Edit"), &edit);

    let view = gio::Menu::new();
    item(&view, "Zoom in", Action::ZoomIn);
    item(&view, "Zoom out", Action::ZoomOut);
    item(&view, "Fit timeline", Action::FitTimeline);
    model.append_submenu(Some("View"), &view);

    let settings = gio::Menu::new();
    let appearance = gio::Menu::new();
    appearance.append(Some("Follow system"), Some("win.theme::system"));
    appearance.append(Some("Dark"), Some("win.theme::dark"));
    appearance.append(Some("Light"), Some("win.theme::light"));
    settings.append_submenu(Some("Appearance"), &appearance);
    let autosave_menu = gio::Menu::new();
    for (minutes, label) in [
        ("0", "Off"),
        ("1", "Every minute"),
        ("5", "Every 5 minutes"),
        ("10", "Every 10 minutes"),
        ("30", "Every 30 minutes"),
    ] {
        autosave_menu.append(Some(label), Some(&format!("win.autosave::{minutes}")));
    }
    settings.append_submenu(Some("Autosave"), &autosave_menu);
    model.append_submenu(Some("Settings"), &settings);

    let help = gio::Menu::new();
    item(&help, "Keyboard shortcuts", Action::ShowShortcuts);
    item(&help, "About Kani...", Action::ShowAbout);
    model.append_submenu(Some("Help"), &help);

    let bar = gtk::PopoverMenuBar::from_model(Some(&model));
    MenuUi {
        bar,
        actions,
        theme,
        autosave,
    }
}

/// Enable or disable items to match what can be done right now.
pub fn update(app: &Rc<App>) {
    let ui = app.ui();
    for (action, simple) in &ui.menu.actions {
        simple.set_enabled(app.is_action_enabled(*action));
    }
    let st = app.state.borrow();
    let theme = match st.settings.theme {
        ThemeChoice::System => "system",
        ThemeChoice::Dark => "dark",
        ThemeChoice::Light => "light",
    };
    ui.menu.theme.set_state(&theme.to_variant());
    ui.menu
        .autosave
        .set_state(&st.settings.autosave_minutes.to_string().to_variant());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_accelerators_are_written_in_gtk_syntax() {
        assert_eq!(accel_for(Action::Split).as_deref(), Some("<Control>b"));
        assert_eq!(
            accel_for(Action::Redo).as_deref(),
            Some("<Control><Shift>z")
        );
        assert_eq!(accel_for(Action::TogglePlay).as_deref(), Some("space"));
        assert_eq!(accel_for(Action::ShowAbout), None);
    }

    #[test]
    fn every_menu_action_has_a_unique_name() {
        let actions = [
            Action::NewProject,
            Action::OpenProject,
            Action::ImportMedia,
            Action::ImportFolder,
            Action::Paste,
            Action::Copy,
            Action::Cut,
            Action::CopyAttributes,
            Action::PasteAttributes,
            Action::Quit,
            Action::Save,
            Action::MoveProject,
            Action::CloseProject,
            Action::Undo,
            Action::Redo,
            Action::Split,
            Action::SplitAll,
            Action::DeleteLeft,
            Action::DeleteRight,
            Action::Delete,
            Action::RippleDelete,
            Action::AddTrack(TrackKind::Video),
            Action::AddTrack(TrackKind::Audio),
            Action::SelectTool,
            Action::SplitTool,
            Action::ZoomIn,
            Action::ZoomOut,
            Action::FitTimeline,
            Action::ShowAbout,
            Action::ShowShortcuts,
            Action::Export,
            Action::ExportRange,
        ];
        let mut seen = Vec::new();
        for action in actions {
            let name = name_of(action).expect("named");
            assert!(!seen.contains(&name), "duplicate {name}");
            seen.push(name);
        }
        assert!(name_of(Action::TogglePlay).is_none());
    }
}
