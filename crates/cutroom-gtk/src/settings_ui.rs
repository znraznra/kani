//! The Settings window: every preference of the app in one place, as a list
//! of sections down the left and the chosen section on the right.
//!
//! Choices are staged and only take effect with Save (Cancel, Esc or closing
//! the window drops them). Rows for things Kani cannot do yet are shown but
//! disabled, with the phase that brings them. Settings that belong to one
//! place of work (the Export window's format and quality, the Auto-Captions
//! panel's language, model and look) stay where they are used and are
//! described in their tab instead of being copied here.

use crate::app::App;
use cutroom_core::settings::{Settings, ThemeChoice};
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// The tabs, in order: (id, title).
pub const TABS: [(&str, &str); 9] = [
    ("general", "General"),
    ("draft", "Draft"),
    ("edit", "Edit"),
    ("playback", "Playback"),
    ("performance", "Performance"),
    ("export", "Export"),
    ("captions", "Captions"),
    ("libraries", "Libraries"),
    ("advanced", "Advanced"),
];

/// What the open window holds (public for the tests).
#[cfg_attr(not(test), allow(dead_code))]
pub struct Dialog {
    pub window: gtk::Window,
    pub stack: gtk::Stack,
    pub draft: Rc<RefCell<Settings>>,
    pub theme: gtk::DropDown,
    pub autosave: gtk::Switch,
    pub autosave_minutes: gtk::SpinButton,
    pub proxies: gtk::Switch,
    pub add_dropped: gtk::Switch,
    pub acceleration: gtk::DropDown,
    pub save: gtk::Button,
    pub cancel: gtk::Button,
    /// Every row built disabled: (tab, label, phase that brings it).
    pub planned: Vec<(String, String, String)>,
}

thread_local! {
    static OPEN: RefCell<Option<Rc<Dialog>>> = const { RefCell::new(None) };
}

/// The open Settings window, if any.
pub fn active() -> Option<Rc<Dialog>> {
    OPEN.with(|o| o.borrow().clone()).filter(|d| d.window.is_visible())
}

pub fn open(app: &Rc<App>, tab: Option<&str>) {
    if let Some(d) = active() {
        if let Some(t) = tab {
            d.stack.set_visible_child_name(t);
        }
        d.window.present();
        return;
    }
    let draft = app.state.borrow().settings.clone();
    build(app, draft, tab);
}

fn drop_down(items: &[&str], selected: u32) -> gtk::DropDown {
    let d = gtk::DropDown::from_strings(items);
    d.set_selected(selected);
    d.set_valign(gtk::Align::Center);
    d
}

fn switch(on: bool) -> gtk::Switch {
    let s = gtk::Switch::new();
    s.set_active(on);
    s.set_valign(gtk::Align::Center);
    s
}

/// One line: the name on the left, the control on the right, an optional note below.
fn row(label: &str, control: &impl IsA<gtk::Widget>, note: Option<&str>) -> gtk::Box {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let name = gtk::Label::new(Some(label));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_wrap(true);
    name.set_width_chars(22);
    name.set_max_width_chars(30);
    line.append(&name);
    control.set_halign(gtk::Align::End);
    line.append(control);
    outer.append(&line);
    if let Some(note) = note {
        let n = gtk::Label::new(Some(note));
        n.set_xalign(0.0);
        n.set_wrap(true);
        n.add_css_class("dim-label");
        n.add_css_class("caption");
        outer.append(&n);
    }
    outer.set_margin_top(6);
    outer.set_margin_bottom(6);
    outer
}

fn note(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.add_css_class("dim-label");
    l
}

/// What a disabled row shows.
enum Planned {
    Switch(bool),
    Choice(&'static [&'static str], u32),
    Number(f64, f64, f64, &'static str),
    Button(&'static str),
    Text(&'static str),
}

fn planned_row(
    page: &gtk::Box,
    list: &mut Vec<(String, String, String)>,
    tab: &str,
    label: &str,
    what: Planned,
    phase: &str,
) {
    let control: gtk::Widget = match what {
        Planned::Switch(on) => switch(on).upcast(),
        Planned::Choice(items, i) => drop_down(items, i).upcast(),
        Planned::Number(v, lo, hi, unit) => {
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let spin = gtk::SpinButton::with_range(lo, hi, 1.0);
            spin.set_value(v);
            b.append(&spin);
            if !unit.is_empty() {
                b.append(&gtk::Label::new(Some(unit)));
            }
            b.upcast()
        }
        Planned::Button(text) => gtk::Button::with_label(text).upcast(),
        Planned::Text(text) => {
            let l = gtk::Label::new(Some(text));
            l.add_css_class("dim-label");
            l.upcast()
        }
    };
    control.set_valign(gtk::Align::Center);
    let r = row(label, &control, Some(&format!("Not available yet - {phase}")));
    r.set_sensitive(false);
    r.set_tooltip_text(Some(&format!("Comes with {phase}")));
    page.append(&r);
    list.push((tab.to_string(), label.to_string(), phase.to_string()));
}

fn page() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.set_margin_top(8);
    b.set_margin_bottom(8);
    b.set_margin_start(18);
    b.set_margin_end(18);
    b
}

fn separator(page: &gtk::Box) {
    page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
}

/// A folder choice: the folder (or the default's name), Change and Use default.
fn folder_row(
    app: &Rc<App>,
    draft: &Rc<RefCell<Settings>>,
    label: &str,
    title: &'static str,
    default_text: String,
    get: fn(&Settings) -> Option<PathBuf>,
    set: fn(&mut Settings, Option<PathBuf>),
) -> gtk::Box {
    let shown = gtk::Label::new(None);
    shown.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    shown.set_max_width_chars(28);
    shown.set_selectable(true);
    shown.add_css_class("caption");
    let change = gtk::Button::with_label("Change...");
    let reset = gtk::Button::with_label("Use default");
    reset.add_css_class("flat");
    let refresh = {
        let (shown, reset, draft) = (shown.clone(), reset.clone(), draft.clone());
        Rc::new(move || {
            let chosen = get(&draft.borrow());
            shown.set_text(
                &chosen
                    .as_ref()
                    .map_or_else(|| default_text.clone(), |p| p.display().to_string()),
            );
            reset.set_visible(chosen.is_some());
        })
    };
    refresh();
    {
        let (app, draft, refresh) = (app.clone(), draft.clone(), refresh.clone());
        change.connect_clicked(move |_| {
            let (draft, refresh) = (draft.clone(), refresh.clone());
            app.choose_folder(title, move |dir| {
                set(&mut draft.borrow_mut(), Some(dir));
                refresh();
            });
        });
    }
    {
        let (draft, refresh) = (draft.clone(), refresh.clone());
        reset.connect_clicked(move |_| {
            set(&mut draft.borrow_mut(), None);
            refresh();
        });
    }
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    controls.append(&shown);
    controls.append(&change);
    controls.append(&reset);
    row(label, &controls, None)
}

fn theme_index(t: ThemeChoice) -> u32 {
    ThemeChoice::ALL.iter().position(|c| *c == t).unwrap_or(0) as u32
}

fn acceleration_index(mode: &str) -> u32 {
    match mode {
        "on" => 1,
        "off" => 2,
        _ => 0,
    }
}

fn acceleration_value(i: u32) -> &'static str {
    match i {
        1 => "on",
        2 => "off",
        _ => "auto",
    }
}

fn build(app: &Rc<App>, initial: Settings, tab: Option<&str>) {
    let draft = Rc::new(RefCell::new(initial));
    let window = crate::dialogs::dialog_window(app, "Settings", 820);
    window.set_resizable(true);
    window.set_default_size(820, 600);
    let mut planned: Vec<(String, String, String)> = Vec::new();
    let stack = gtk::Stack::new();
    stack.set_vexpand(true);

    // ----- General -------------------------------------------------------
    let general = page();
    let theme = drop_down(
        &ThemeChoice::ALL.map(|t| t.label()),
        theme_index(draft.borrow().theme),
    );
    {
        let draft = draft.clone();
        theme.connect_selected_notify(move |d| {
            draft.borrow_mut().theme = ThemeChoice::ALL[(d.selected() as usize).min(2)];
        });
    }
    general.append(&row("Appearance", &theme, None));
    planned_row(&general, &mut planned, "general", "Language", Planned::Choice(&["English"], 0), "Phase 11");
    let shortcuts = gtk::Button::with_label("View...");
    {
        let app = app.clone();
        shortcuts.connect_clicked(move |_| crate::dialogs::shortcuts(&app));
    }
    general.append(&row(
        "Keyboard shortcuts",
        &shortcuts,
        Some("Ctrl+, opens these settings."),
    ));
    planned_row(&general, &mut planned, "general", "Check for updates automatically", Planned::Switch(false), "Phase 11");
    stack.add_titled(&scroll(&general), Some("general"), "General");

    // ----- Draft ---------------------------------------------------------
    let draft_page = page();
    let default_dir = app.state.borrow().default_projects_dir.display().to_string();
    draft_page.append(&folder_row(
        app,
        &draft,
        "Projects folder",
        "Where should Kani keep your projects?",
        default_dir,
        |s| s.projects_dir.clone(),
        |s, v| s.projects_dir = v,
    ));
    let autosave = switch(draft.borrow().autosave_every() > 0);
    let autosave_minutes = gtk::SpinButton::with_range(1.0, 120.0, 1.0);
    autosave_minutes.set_value(f64::from(match draft.borrow().autosave_minutes {
        0 => 10,
        m => m,
    }));
    autosave_minutes.set_sensitive(autosave.is_active());
    {
        let (draft, minutes) = (draft.clone(), autosave_minutes.clone());
        autosave.connect_active_notify(move |s| {
            let mut d = draft.borrow_mut();
            d.autosave_enabled = s.is_active();
            if d.autosave_minutes == 0 {
                d.autosave_minutes = 10;
            }
            minutes.set_sensitive(s.is_active());
        });
    }
    {
        let draft = draft.clone();
        autosave_minutes.connect_value_changed(move |s| {
            draft.borrow_mut().autosave_minutes = s.value() as u32;
        });
    }
    draft_page.append(&row(
        "Autosave",
        &autosave,
        Some("Saves a project in the background while you work, and keeps a rotating backup."),
    ));
    let every = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    every.append(&autosave_minutes);
    every.append(&gtk::Label::new(Some("minutes")));
    draft_page.append(&row("Autosave interval", &every, None));
    separator(&draft_page);
    planned_row(&draft_page, &mut planned, "draft", "Import material in new projects", Planned::Choice(&["Keep in original place", "Copy into the project"], 0), "Project settings");
    planned_row(&draft_page, &mut planned, "draft", "Frame rate for new projects", Planned::Choice(&["30.00 fps"], 0), "Project settings");
    planned_row(&draft_page, &mut planned, "draft", "Resolution for new projects", Planned::Choice(&["1080 x 1920"], 0), "Project settings");
    planned_row(&draft_page, &mut planned, "draft", "Colour space for new projects", Planned::Choice(&["Rec. 709"], 0), "Project settings");
    planned_row(&draft_page, &mut planned, "draft", "Keep backup copies of projects", Planned::Switch(true), "Phase 11");
    stack.add_titled(&scroll(&draft_page), Some("draft"), "Draft");

    // ----- Edit ----------------------------------------------------------
    let edit = page();
    let add_dropped = switch(draft.borrow().add_dropped_to_timeline);
    {
        let draft = draft.clone();
        add_dropped.connect_active_notify(move |s| {
            draft.borrow_mut().add_dropped_to_timeline = s.is_active();
        });
    }
    edit.append(&row(
        "Add dropped files to the timeline",
        &add_dropped,
        Some("Files dropped on the window go into the library, and also onto the timeline when this is on."),
    ));
    separator(&edit);
    planned_row(&edit, &mut planned, "edit", "Step forward", Planned::Number(10.0, 1.0, 100.0, "frames"), "Phase 11");
    planned_row(&edit, &mut planned, "edit", "Adjust values by", Planned::Number(10.0, 1.0, 100.0, "units"), "Phase 11");
    planned_row(&edit, &mut planned, "edit", "Image duration", Planned::Number(5.0, 1.0, 600.0, "s"), "Phase 11");
    planned_row(&edit, &mut planned, "edit", "Target loudness", Planned::Choice(&["Default (-23 LUFS)"], 0), "Phase 11");
    planned_row(&edit, &mut planned, "edit", "Play sound while dragging the playhead", Planned::Switch(true), "Phase 3");
    planned_row(&edit, &mut planned, "edit", "Layers can be reordered in new projects", Planned::Switch(false), "Phase 11");
    planned_row(&edit, &mut planned, "edit", "Snap to clips and the playhead", Planned::Switch(true), "Phase 11");
    planned_row(&edit, &mut planned, "edit", "Time code", Planned::Choice(&["HH:MM:SS+frame"], 0), "Phase 11");
    stack.add_titled(&scroll(&edit), Some("edit"), "Edit");

    // ----- Playback ------------------------------------------------------
    let playback = page();
    let proxies = switch(draft.borrow().use_proxies);
    {
        let draft = draft.clone();
        proxies.connect_active_notify(move |s| {
            draft.borrow_mut().use_proxies = s.is_active();
        });
    }
    playback.append(&row(
        "Use proxies in the preview",
        &proxies,
        Some("Proxies are small copies made with Create Proxy in a media item's right-click menu. The preview plays them; export always uses the originals."),
    ));
    separator(&playback);
    planned_row(&playback, &mut planned, "playback", "Create proxies automatically", Planned::Switch(false), "Phase 3");
    planned_row(&playback, &mut planned, "playback", "Proxy quality", Planned::Choice(&["Small and fast"], 0), "Phase 3");
    planned_row(&playback, &mut planned, "playback", "Decode with the graphics chip", Planned::Switch(true), "Phase 3");
    planned_row(&playback, &mut planned, "playback", "Remux unsupported files on import", Planned::Switch(false), "Phase 3");
    planned_row(&playback, &mut planned, "playback", "Preview resolution", Planned::Choice(&["Original"], 0), "Phase 3");
    stack.add_titled(&scroll(&playback), Some("playback"), "Playback");

    // ----- Performance ---------------------------------------------------
    let performance = page();
    let acceleration = drop_down(
        &["Automatic (recommended)", "Graphics chip", "Processor"],
        acceleration_index(&draft.borrow().export_graphics_card),
    );
    {
        let draft = draft.clone();
        acceleration.connect_selected_notify(move |d| {
            draft.borrow_mut().export_graphics_card = acceleration_value(d.selected()).to_string();
        });
    }
    let card_state = match draft.borrow().graphics_card_available.or(draft.borrow().graphics_card_ok) {
        Some(true) => "The last check found a graphics chip that works.",
        Some(false) => "The last check found no graphics chip that works here.",
        None => "The graphics chip has not been checked yet (the Export window checks it).",
    };
    performance.append(&row(
        "Compositor acceleration",
        &acceleration,
        Some(&format!(
            "Where pictures are put together when exporting. Automatic uses the graphics chip only when it is faster. The Export window's Acceleration row is this same setting. {card_state}"
        )),
    ));
    separator(&performance);
    planned_row(&performance, &mut planned, "performance", "Preview compositing on the graphics chip", Planned::Switch(false), "Phase 4 (GPU backend)");
    planned_row(&performance, &mut planned, "performance", "Processor threads", Planned::Choice(&["Automatic"], 0), "Phase 11");
    planned_row(&performance, &mut planned, "performance", "Memory limit", Planned::Choice(&["Automatic"], 0), "Phase 11");
    planned_row(&performance, &mut planned, "performance", "Cache folder", Planned::Text("Default"), "Phase 11");
    planned_row(&performance, &mut planned, "performance", "Clear the cache", Planned::Button("Clear"), "Phase 11");
    stack.add_titled(&scroll(&performance), Some("performance"), "Performance");

    // ----- Export --------------------------------------------------------
    let export = page();
    export.append(&folder_row(
        app,
        &draft,
        "Export folder",
        "Where should exported videos go?",
        "The Videos folder".to_string(),
        |s| s.export_folder.clone(),
        |s, v| s.export_folder = v,
    ));
    export.append(&note(
        "Format, quality, sound and caption files are chosen in the Export window, which remembers them.",
    ));
    separator(&export);
    planned_row(&export, &mut planned, "export", "Play a sound when an export finishes", Planned::Switch(false), "Phase 11");
    planned_row(&export, &mut planned, "export", "Open the folder when an export finishes", Planned::Switch(false), "Phase 11");
    stack.add_titled(&scroll(&export), Some("export"), "Export");

    // ----- Captions ------------------------------------------------------
    let captions = page();
    captions.append(&note(
        "The spoken language, speech model and caption look are chosen in the Auto-Captions panel, where they are used, and remembered.",
    ));
    separator(&captions);
    planned_row(&captions, &mut planned, "captions", "Label speakers", Planned::Switch(false), "Phase 7 (open item)");
    planned_row(&captions, &mut planned, "captions", "Translate captions", Planned::Choice(&["Off"], 0), "Phase 7 (open item)");
    planned_row(&captions, &mut planned, "captions", "Recognise speech on the graphics chip", Planned::Switch(false), "Phase 7 (open item)");
    planned_row(&captions, &mut planned, "captions", "Speech model folder", Planned::Text("Default"), "Phase 7 (open item)");
    stack.add_titled(&scroll(&captions), Some("captions"), "Captions");

    // ----- Libraries -----------------------------------------------------
    let libraries = page();
    planned_row(&libraries, &mut planned, "libraries", "Asset library folder", Planned::Text("Default"), "Phase 8");
    planned_row(&libraries, &mut planned, "libraries", "Installed asset packs", Planned::Button("Manage..."), "Phase 8");
    planned_row(&libraries, &mut planned, "libraries", "Colour look (LUT) folder", Planned::Text("Default"), "Phase 9");
    planned_row(&libraries, &mut planned, "libraries", "Default grading colour space", Planned::Choice(&["Rec. 709"], 0), "Phase 9");
    planned_row(&libraries, &mut planned, "libraries", "Sticker folder", Planned::Text("Default"), "Phase 10");
    stack.add_titled(&scroll(&libraries), Some("libraries"), "Libraries");

    // ----- Advanced ------------------------------------------------------
    let advanced = page();
    let config = app
        .state
        .borrow()
        .config_dir
        .as_ref()
        .map_or_else(|| "not available".to_string(), |p| p.display().to_string());
    let config_label = gtk::Label::new(Some(&config));
    config_label.set_selectable(true);
    config_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    config_label.set_max_width_chars(34);
    config_label.add_css_class("caption");
    advanced.append(&row("Settings are stored in", &config_label, None));
    let restore = gtk::Button::with_label("Restore defaults");
    advanced.append(&row(
        "Restore defaults",
        &restore,
        Some("Puts every setting in this window back to how Kani starts. Nothing changes until you press Save."),
    ));
    separator(&advanced);
    planned_row(&advanced, &mut planned, "advanced", "Write a diagnostic log", Planned::Switch(false), "Phase 11");
    planned_row(&advanced, &mut planned, "advanced", "Send crash reports", Planned::Switch(false), "Phase 11");
    stack.add_titled(&scroll(&advanced), Some("advanced"), "Advanced");

    // ----- Sidebar -------------------------------------------------------
    // The sections run down the left; the chosen one shows on the right.
    let sidebar = gtk::ListBox::new();
    sidebar.set_selection_mode(gtk::SelectionMode::Single);
    sidebar.add_css_class("navigation-sidebar");
    for (_, title) in TABS {
        let label = gtk::Label::new(Some(title));
        label.set_xalign(0.0);
        label.set_margin_top(8);
        label.set_margin_bottom(8);
        label.set_margin_start(10);
        label.set_margin_end(10);
        sidebar.append(&label);
    }
    {
        let stack = stack.clone();
        sidebar.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                if let Some((id, _)) = TABS.get(row.index() as usize) {
                    stack.set_visible_child_name(id);
                }
            }
        });
    }
    // Keep the list in step when something else changes the section.
    {
        let sidebar = sidebar.clone();
        stack.connect_visible_child_name_notify(move |s| {
            let name = s.visible_child_name();
            if let Some(i) = TABS.iter().position(|(id, _)| Some(*id) == name.as_deref()) {
                if let Some(row) = sidebar.row_at_index(i as i32) {
                    if !row.is_selected() {
                        sidebar.select_row(Some(&row));
                    }
                }
            }
        });
    }
    let start = tab
        .filter(|t| TABS.iter().any(|(id, _)| id == t))
        .unwrap_or("general");
    stack.set_visible_child_name(start);
    if let Some(i) = TABS.iter().position(|(id, _)| *id == start) {
        sidebar.select_row(sidebar.row_at_index(i as i32).as_ref());
    }
    let side_scroll = gtk::ScrolledWindow::new();
    side_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    side_scroll.set_min_content_width(170);
    side_scroll.set_child(Some(&sidebar));
    side_scroll.set_margin_start(12);
    side_scroll.set_margin_top(12);
    side_scroll.set_margin_bottom(8);

    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    let cancel = gtk::Button::with_label("Cancel");
    let bottom = crate::dialogs::buttons(&save, &cancel);
    bottom.set_margin_start(18);
    bottom.set_margin_end(18);
    bottom.set_margin_bottom(14);
    bottom.set_margin_top(8);

    let frame = gtk::Frame::new(None);
    frame.set_child(Some(&stack));
    frame.set_margin_start(12);
    frame.set_margin_end(18);
    frame.set_margin_top(12);
    frame.set_margin_bottom(8);
    frame.set_hexpand(true);
    frame.set_vexpand(true);

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.append(&side_scroll);
    body.append(&frame);
    body.set_vexpand(true);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&body);
    root.append(&bottom);
    window.set_child(Some(&root));

    let dialog = Rc::new(Dialog {
        window: window.clone(),
        stack: stack.clone(),
        draft: draft.clone(),
        theme: theme.clone(),
        autosave: autosave.clone(),
        autosave_minutes: autosave_minutes.clone(),
        proxies: proxies.clone(),
        add_dropped: add_dropped.clone(),
        acceleration: acceleration.clone(),
        save: save.clone(),
        cancel: cancel.clone(),
        planned,
    });

    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    {
        let (app, dialog) = (app.clone(), dialog.clone());
        save.connect_clicked(move |_| {
            apply(&app, &dialog.draft.borrow());
            dialog.window.close();
        });
    }
    {
        // Back to the starting values, shown at once (and saved only by Save).
        let (app, dialog) = (app.clone(), dialog.clone());
        restore.connect_clicked(move |_| {
            let mut fresh = Settings::default();
            {
                // What is not a setting of this window is kept.
                let d = dialog.draft.borrow();
                fresh.recent_projects = d.recent_projects.clone();
                fresh.home_view = d.home_view;
                fresh.library_view = d.library_view;
                fresh.caption_language = d.caption_language.clone();
                fresh.caption_model = d.caption_model.clone();
                fresh.caption_model_path = d.caption_model_path.clone();
                fresh.caption_style = d.caption_style.clone();
                fresh.export_quality = d.export_quality.clone();
                fresh.export_encoder = d.export_encoder.clone();
                fresh.export_video = d.export_video;
                fresh.export_audio = d.export_audio;
                fresh.export_codec = d.export_codec.clone();
                fresh.export_container = d.export_container.clone();
                fresh.export_audio_format = d.export_audio_format.clone();
                fresh.export_audio_kbps = d.export_audio_kbps;
                fresh.export_channels = d.export_channels.clone();
                fresh.export_custom_kbps = d.export_custom_kbps;
                fresh.export_fill = d.export_fill;
                fresh.export_caption_files = d.export_caption_files.clone();
                fresh.graphics_card_ok = d.graphics_card_ok;
                fresh.graphics_card_available = d.graphics_card_available;
                fresh.graphics_card_checked = d.graphics_card_checked.clone();
            }
            let tab = dialog.stack.visible_child_name().map(|s| s.to_string());
            dialog.window.close();
            build(&app, fresh, tab.as_deref());
        });
    }
    {
        let dialog = dialog.clone();
        window.connect_close_request(move |_| {
            OPEN.with(|o| {
                let mut o = o.borrow_mut();
                if o.as_ref().is_some_and(|d| Rc::ptr_eq(d, &dialog)) {
                    *o = None;
                }
            });
            gtk::glib::Propagation::Proceed
        });
    }
    OPEN.with(|o| *o.borrow_mut() = Some(dialog));
    window.present();
}

fn scroll(page: &gtk::Box) -> gtk::ScrolledWindow {
    let s = gtk::ScrolledWindow::new();
    s.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    s.set_vexpand(true);
    s.set_min_content_height(360);
    s.set_child(Some(page));
    s
}

/// Make the staged choices the real ones.
fn apply(app: &Rc<App>, new: &Settings) {
    let (theme_changed, projects_changed, proxies_changed) = {
        let st = app.state.borrow();
        let old = &st.settings;
        (
            old.theme != new.theme,
            old.projects_dir != new.projects_dir,
            old.use_proxies != new.use_proxies,
        )
    };
    {
        let mut st = app.state.borrow_mut();
        let s = &mut st.settings;
        s.autosave_enabled = new.autosave_enabled;
        s.autosave_minutes = new.autosave_minutes;
        s.add_dropped_to_timeline = new.add_dropped_to_timeline;
        s.export_graphics_card = new.export_graphics_card.clone();
        s.export_folder = new.export_folder.clone();
        s.use_proxies = new.use_proxies;
        st.save_settings();
    }
    if theme_changed {
        app.set_theme(new.theme);
    }
    if projects_changed {
        app.change_projects_dir(new.projects_dir.clone());
    }
    if proxies_changed {
        app.sync_preview(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acceleration_choices_map_to_the_saved_words() {
        for (i, word) in [(0, "auto"), (1, "on"), (2, "off")] {
            assert_eq!(acceleration_value(i), word);
            assert_eq!(acceleration_index(word), i);
        }
        assert_eq!(acceleration_index("something else"), 0);
    }

    #[test]
    fn every_tab_has_a_unique_id() {
        let mut ids: Vec<_> = TABS.iter().map(|t| t.0).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), TABS.len());
    }
}
