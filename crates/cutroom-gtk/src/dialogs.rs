//! Dialog windows: new project, rename, project info, delete, system
//! information and the keyboard shortcut list.

use crate::app::App;
use cutroom_core::home_logic::{ago_text, format_duration, format_size, format_utc};
use cutroom_core::new_project::{
    aspect_label, CanvasChoice, NewProjectForm, CANVAS_PRESETS, CUSTOM_MAX, CUSTOM_MIN, FRAME_RATES,
};
use cutroom_core::shortcuts::{self, Origin, BINDINGS, PLANNED};
use cutroom_project::{
    delete_project as delete_project_files, dir_size, folder_name_problem,
    rename_project as rename_project_files, ProjectEntry,
};
use gstreamer as gst;
use gstreamer::prelude::*;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::SystemTime;

pub(crate) fn dialog_window(app: &Rc<App>, title: &str, width: i32) -> gtk::Window {
    let window = gtk::Window::builder()
        .title(title)
        .modal(true)
        .transient_for(&app.window)
        .destroy_with_parent(true)
        .default_width(width)
        .resizable(false)
        .build();
    let keys = gtk::EventControllerKey::new();
    {
        let window = window.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                window.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
    window.add_controller(keys);
    window
}

pub(crate) fn content_box() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 10);
    b.set_margin_top(16);
    b.set_margin_bottom(16);
    b.set_margin_start(18);
    b.set_margin_end(18);
    b
}

pub(crate) fn buttons(primary: &gtk::Button, cancel: &gtk::Button) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::End);
    row.append(cancel);
    row.append(primary);
    row
}

pub(crate) fn caption(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.add_css_class("dim-label");
    l
}

pub(crate) fn error_label() -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.add_css_class("error");
    l
}

// ----- new project ---------------------------------------------------------------

pub fn new_project(app: &Rc<App>) {
    let form = Rc::new(RefCell::new(NewProjectForm::new(
        app.state.borrow().projects_dir(),
    )));
    let window = dialog_window(app, "New project", 480);
    let body = content_box();
    let grid = gtk::Grid::new();
    grid.set_row_spacing(10);
    grid.set_column_spacing(12);

    let name = gtk::Entry::new();
    name.set_text(&form.borrow().name);
    name.set_hexpand(true);
    grid.attach(&caption("Name"), 0, 0, 1, 1);
    grid.attach(&name, 1, 0, 1, 1);

    let location = gtk::Label::new(None);
    location.set_xalign(0.0);
    location.set_hexpand(true);
    location.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    location.add_css_class("caption");
    let choose = gtk::Button::with_label("Choose...");
    let use_default = gtk::Button::with_label("Use default");
    let location_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    location_row.append(&location);
    location_row.append(&choose);
    location_row.append(&use_default);
    grid.attach(&caption("Save in"), 0, 1, 1, 1);
    grid.attach(&location_row, 1, 1, 1, 1);

    let mut labels: Vec<&str> = CANVAS_PRESETS.iter().map(|p| p.label).collect();
    labels.push("Custom size...");
    let canvas = gtk::DropDown::from_strings(&labels);
    grid.attach(&caption("Canvas"), 0, 2, 1, 1);
    grid.attach(&canvas, 1, 2, 1, 1);

    let width = gtk::SpinButton::with_range(f64::from(CUSTOM_MIN), f64::from(CUSTOM_MAX), 2.0);
    let height = gtk::SpinButton::with_range(f64::from(CUSTOM_MIN), f64::from(CUSTOM_MAX), 2.0);
    width.set_value(f64::from(form.borrow().custom_width));
    height.set_value(f64::from(form.borrow().custom_height));
    let size_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    size_row.append(&width);
    size_row.append(&gtk::Label::new(Some("x")));
    size_row.append(&height);
    size_row.append(&gtk::Label::new(Some("px")));
    let size_caption = caption("Size");
    grid.attach(&size_caption, 0, 3, 1, 1);
    grid.attach(&size_row, 1, 3, 1, 1);

    let fps_labels: Vec<String> = FRAME_RATES
        .iter()
        .map(|(l, _)| format!("{l} fps"))
        .collect();
    let fps_refs: Vec<&str> = fps_labels.iter().map(String::as_str).collect();
    let fps = gtk::DropDown::from_strings(&fps_refs);
    fps.set_selected(form.borrow().rate_index as u32);
    grid.attach(&caption("Frame rate"), 0, 4, 1, 1);
    grid.attach(&fps, 1, 4, 1, 1);
    body.append(&grid);

    let summary = caption("");
    let problem = error_label();
    body.append(&summary);
    body.append(&problem);

    let cancel = gtk::Button::with_label("Cancel");
    let create = gtk::Button::with_label("Create");
    create.add_css_class("suggested-action");
    body.append(&buttons(&create, &cancel));
    window.set_child(Some(&body));

    // One function brings every dependent widget in line with the form.
    let update: Rc<dyn Fn()> = {
        let (form, location, use_default, size_row, size_caption, summary, problem, create) = (
            form.clone(),
            location.clone(),
            use_default.clone(),
            size_row.clone(),
            size_caption.clone(),
            summary.clone(),
            problem.clone(),
            create.clone(),
        );
        Rc::new(move || {
            let f = form.borrow();
            location.set_text(&f.location.display().to_string());
            use_default.set_visible(!f.is_default_location());
            let custom = f.canvas == CanvasChoice::Custom;
            size_row.set_visible(custom);
            size_caption.set_visible(custom);
            let (w, h) = f.size();
            let rate = FRAME_RATES[f.rate_index.min(FRAME_RATES.len() - 1)].0;
            let mut text = format!("{w} x {h} ({}) at {rate} fps", aspect_label(w, h));
            if custom {
                text.push_str(&format!(
                    ". Custom sizes stay even and between {CUSTOM_MIN} and {CUSTOM_MAX} px."
                ));
            }
            summary.set_text(&text);
            let message = if let Some(p) = f.name_problem() {
                p.to_owned()
            } else if f.folder_taken() {
                "A project with that name already exists in this folder.".to_owned()
            } else if let Some(e) = &f.error {
                e.clone()
            } else {
                String::new()
            };
            problem.set_text(&message);
            problem.set_visible(!message.is_empty());
            create.set_sensitive(f.can_create());
        })
    };

    {
        let (form, update) = (form.clone(), update.clone());
        name.connect_changed(move |e| {
            form.borrow_mut().name = e.text().to_string();
            form.borrow_mut().error = None;
            update();
        });
    }
    {
        let (form, update) = (form.clone(), update.clone());
        canvas.connect_selected_notify(move |d| {
            let i = d.selected() as usize;
            form.borrow_mut().canvas = if i < CANVAS_PRESETS.len() {
                CanvasChoice::Preset(i)
            } else {
                CanvasChoice::Custom
            };
            update();
        });
    }
    {
        let (form, update) = (form.clone(), update.clone());
        width.connect_value_changed(move |s| {
            form.borrow_mut().custom_width = s.value() as u32;
            update();
        });
    }
    {
        let (form, update) = (form.clone(), update.clone());
        height.connect_value_changed(move |s| {
            form.borrow_mut().custom_height = s.value() as u32;
            update();
        });
    }
    {
        let (form, update) = (form.clone(), update.clone());
        fps.connect_selected_notify(move |d| {
            form.borrow_mut().rate_index = d.selected() as usize;
            update();
        });
    }
    {
        let (form, update, app) = (form.clone(), update.clone(), app.clone());
        choose.connect_clicked(move |_| {
            let (form, update) = (form.clone(), update.clone());
            app.choose_folder("Where should the project be saved?", move |dir| {
                form.borrow_mut().location = dir;
                update();
            });
        });
    }
    {
        let (form, update) = (form.clone(), update.clone());
        use_default.connect_clicked(move |_| {
            let default = form.borrow().default_location.clone();
            form.borrow_mut().location = default;
            update();
        });
    }
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    {
        let (form, update, app, window) =
            (form.clone(), update.clone(), app.clone(), window.clone());
        create.connect_clicked(move |_| {
            let snapshot = form.borrow().clone();
            match app.create_project(&snapshot) {
                Ok(()) => window.close(),
                Err(message) => {
                    form.borrow_mut().error = Some(message);
                    update();
                }
            }
        });
    }
    {
        let create = create.clone();
        name.connect_activate(move |_| {
            if create.is_sensitive() {
                create.emit_clicked();
            }
        });
    }
    update();
    window.present();
    name.grab_focus();
    name.select_region(0, -1);
}

// ----- rename ----------------------------------------------------------------------

pub fn rename_project(app: &Rc<App>, entry: &ProjectEntry) {
    let path = entry.path().to_path_buf();
    let window = dialog_window(app, "Rename project", 420);
    let body = content_box();
    body.append(&caption("New name"));
    let name = gtk::Entry::new();
    name.set_text(&entry.display_name());
    body.append(&name);
    let rename_folder = gtk::CheckButton::with_label("Also rename the project's folder");
    rename_folder.set_active(true);
    body.append(&rename_folder);
    let problem = error_label();
    body.append(&problem);
    let cancel = gtk::Button::with_label("Cancel");
    let rename = gtk::Button::with_label("Rename");
    rename.add_css_class("suggested-action");
    body.append(&buttons(&rename, &cancel));
    window.set_child(Some(&body));

    let validate: Rc<dyn Fn()> = {
        let (name, rename_folder, problem, rename, path) = (
            name.clone(),
            rename_folder.clone(),
            problem.clone(),
            rename.clone(),
            path.clone(),
        );
        Rc::new(move || {
            let text = name.text().to_string();
            let target = path.parent().map(|p| p.join(text.trim()));
            let message = if let Some(p) = folder_name_problem(&text) {
                Some(p.to_owned())
            } else if rename_folder.is_active()
                && target.as_ref().is_some_and(|t| *t != path && t.exists())
            {
                Some("A folder with that name already exists.".to_owned())
            } else {
                None
            };
            problem.set_text(message.as_deref().unwrap_or(""));
            problem.set_visible(message.is_some());
            rename.set_sensitive(message.is_none());
        })
    };
    {
        let validate = validate.clone();
        name.connect_changed(move |_| validate());
    }
    {
        let validate = validate.clone();
        rename_folder.connect_toggled(move |_| validate());
    }
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    {
        let (app, window, name, rename_folder, problem, path) = (
            app.clone(),
            window.clone(),
            name.clone(),
            rename_folder.clone(),
            problem.clone(),
            path.clone(),
        );
        rename.connect_clicked(move |_| {
            let new_name = name.text().to_string();
            match rename_project_files(&path, &new_name, rename_folder.is_active()) {
                Ok(new_path) => {
                    {
                        let mut st = app.state.borrow_mut();
                        if st.settings.recent_projects.contains(&path) {
                            st.settings.forget_project(&path);
                            st.settings.remember_project(&new_path);
                            st.save_settings();
                        }
                        if st.home.selected.as_deref() == Some(path.as_path()) {
                            st.home.selected = Some(new_path);
                        }
                        st.notice = Some(format!("Renamed to \"{}\"", new_name.trim()));
                    }
                    window.close();
                    app.refresh_home_entries();
                    app.refresh();
                }
                Err(e) => {
                    problem.set_text(&e.to_string());
                    problem.set_visible(true);
                }
            }
        });
    }
    {
        let rename = rename.clone();
        name.connect_activate(move |_| {
            if rename.is_sensitive() {
                rename.emit_clicked();
            }
        });
    }
    validate();
    window.present();
    name.grab_focus();
    name.select_region(0, -1);
}

// ----- info -------------------------------------------------------------------------

pub fn project_info(app: &Rc<App>, entry: &ProjectEntry) {
    let window = dialog_window(app, "Project info", 520);
    let body = content_box();
    let grid = gtk::Grid::new();
    grid.set_row_spacing(6);
    grid.set_column_spacing(16);
    let mut row = 0;
    let mut add = |label: &str, value: String| {
        grid.attach(&caption(label), 0, row, 1, 1);
        let v = gtk::Label::new(Some(&value));
        v.set_xalign(0.0);
        v.set_selectable(true);
        v.set_wrap(true);
        v.set_max_width_chars(48);
        grid.attach(&v, 1, row, 1, 1);
        row += 1;
    };
    let path = entry.path().to_path_buf();
    add("Name", entry.display_name());
    add("Location", path.display().to_string());
    match entry {
        ProjectEntry::Ok(s) => {
            add(
                "Canvas",
                format!(
                    "{} x {}  ({})",
                    s.width,
                    s.height,
                    aspect_label(s.width, s.height)
                ),
            );
            add("Frame rate", format!("{:.3} fps", s.frame_rate.as_f64()));
            add("Length", format_duration(s.duration));
            add("Media files", s.media_count.to_string());
            add("Clips on timeline", s.clip_count.to_string());
        }
        ProjectEntry::Broken { error, .. } => add("Problem", error.clone()),
    }
    if let Some(modified) = entry.modified() {
        add(
            "Last modified",
            format!(
                "{}  ({})",
                ago_text(Some(modified), SystemTime::now()),
                format_utc(modified)
            ),
        );
    }
    add("Size on disk", format_size(dir_size(&path)));
    add(
        "Media copied into project",
        format_size(dir_size(&path.join("media"))),
    );
    body.append(&grid);
    let close = gtk::Button::with_label("Close");
    {
        let window = window.clone();
        close.connect_clicked(move |_| window.close());
    }
    let spacer = gtk::Button::new();
    spacer.set_visible(false);
    body.append(&buttons(&close, &spacer));
    window.set_child(Some(&body));
    window.present();
}

// ----- delete -----------------------------------------------------------------------

pub fn delete_project(app: &Rc<App>, entry: &ProjectEntry) {
    let path = entry.path().to_path_buf();
    let name = entry.display_name();
    let detail = format!(
        "{}\n\nThis permanently deletes the project folder and everything inside it ({}), including {} of media copied into the project. Original files that the project only links to are not touched. This can't be undone.",
        path.display(),
        format_size(dir_size(&path)),
        format_size(dir_size(&path.join("media"))),
    );
    let alert = gtk::AlertDialog::builder()
        .message(format!("Delete \"{name}\"?"))
        .detail(detail)
        .buttons(["Cancel", "Delete permanently"])
        .cancel_button(0)
        .default_button(0)
        .modal(true)
        .build();
    let app = app.clone();
    let parent = app.window.clone();
    alert.choose(Some(&parent), gio::Cancellable::NONE, move |answer| {
        if answer.ok() != Some(1) {
            return;
        }
        match delete_project_files(&path) {
            Ok(()) => {
                {
                    let mut st = app.state.borrow_mut();
                    st.settings.forget_project(&path);
                    st.save_settings();
                    st.notice = Some(format!("Deleted \"{name}\""));
                }
                app.refresh_home_entries();
                app.refresh();
            }
            Err(e) => {
                app.set_status(format!("Could not delete: {e}"));
                app.refresh();
            }
        }
    });
}

// ----- system information -----------------------------------------------------------

/// Hardware-accelerated codecs GStreamer found, so we can see what this machine offers.
pub fn hardware_codec_lines() -> Vec<String> {
    const HINTS: [&str; 10] = [
        "vaapi", "nvdec", "nvh26", "nvav1", "d3d11", "d3d12", "qsv", "msdk", "v4l2", "vulkan",
    ];
    let mut lines = Vec::new();
    for (label, kind) in [
        ("decoder", gst::ElementFactoryType::DECODER),
        ("encoder", gst::ElementFactoryType::ENCODER),
    ] {
        let factories = gst::ElementFactory::factories_with_type(
            kind | gst::ElementFactoryType::MEDIA_VIDEO,
            gst::Rank::NONE,
        );
        let mut names: Vec<String> = factories
            .iter()
            .map(|f| f.name().to_string())
            .filter(|n| HINTS.iter().any(|h| n.contains(h)) || n.starts_with("va") && n.len() > 2)
            .collect();
        names.sort();
        names.dedup();
        for name in names {
            lines.push(format!("  {label}: {name}"));
        }
    }
    if lines.is_empty() {
        lines.push("  none found (software codecs only)".to_owned());
    }
    lines
}

/// `Version 0.5.11 (built 2026-10-13, commit abc1234)`.
pub fn version_line() -> String {
    let commit = env!("CUTROOM_GIT_HASH");
    let mut line = format!(
        "Version {} (built {}",
        env!("CARGO_PKG_VERSION"),
        env!("CUTROOM_BUILD_DATE")
    );
    if !commit.is_empty() {
        line.push_str(&format!(", commit {commit}"));
    }
    line.push(')');
    line
}

/// The build date as yymmdd, for the About heading.
fn short_build_date() -> String {
    let digits: String = env!("CUTROOM_BUILD_DATE")
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    if digits.len() >= 8 {
        digits[2..8].to_owned()
    } else {
        digits
    }
}

pub fn author_line() -> String {
    format!("By {}", env!("CARGO_PKG_AUTHORS").replace(':', ", "))
}

/// Everything the system information section reports.
pub struct SystemFacts<'a> {
    pub display: &'a str,
    pub session_type: &'a str,
    pub wayland_display: &'a str,
    pub x11_display: &'a str,
    pub gtk_version: &'a str,
    pub gstreamer_version: &'a str,
    pub dark: bool,
    pub config_dir: &'a str,
    pub projects_dir: &'a str,
    pub hardware: &'a [String],
    /// Per codec, the decoders GStreamer would pick (best first).
    pub decoders: &'a [String],
}

pub fn system_info_text(f: &SystemFacts) -> String {
    let mut text = String::new();
    text.push_str(&format!(
        "GTK {}\nGStreamer {}\n",
        f.gtk_version, f.gstreamer_version
    ));
    text.push_str(&format!("Window system: {}\n", f.display));
    text.push_str(&format!(
        "XDG_SESSION_TYPE={}  WAYLAND_DISPLAY={}  DISPLAY={}\n",
        f.session_type, f.wayland_display, f.x11_display
    ));
    text.push_str(&format!(
        "Appearance: {}\n\n",
        if f.dark { "dark" } else { "light" }
    ));
    text.push_str(&format!(
        "Settings folder: {}\nProjects folder: {}\n\n",
        f.config_dir, f.projects_dir
    ));
    text.push_str("Hardware video codecs available to GStreamer:\n");
    for line in f.hardware {
        text.push_str(line);
        text.push('\n');
    }
    text.push_str("\nVideo decoders GStreamer will pick, best first:\n");
    for line in f.decoders {
        text.push_str(line);
        text.push('\n');
    }
    text
}

pub fn about(app: &Rc<App>) {
    let window = dialog_window(app, "About Kani", 420);
    // Not modal: it can be moved aside and the editor stays usable.
    window.set_modal(false);
    window.set_resizable(true);
    window.set_default_size(420, 546);
    window.add_css_class("about-compact");

    let display = gtk::gdk::Display::default()
        .map(|d| d.type_().name().to_owned())
        .unwrap_or_else(|| "unknown".into());
    let env = |name: &str| std::env::var(name).unwrap_or_else(|_| "(not set)".into());
    let (config, projects, dark) = {
        let st = app.state.borrow();
        (
            st.config_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            st.projects_dir().display().to_string(),
            st.dark,
        )
    };
    let hardware = hardware_codec_lines();
    let decoders = crate::analysis::decoder_lines(&crate::analysis::decoder_choices());
    let gtk_version = format!(
        "{}.{}.{}",
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version()
    );
    let gstreamer_version = gst::version_string().to_string();
    let (session, wayland, x11) = (
        env("XDG_SESSION_TYPE"),
        env("WAYLAND_DISPLAY"),
        env("DISPLAY"),
    );
    let system_text = system_info_text(&SystemFacts {
        display: &display,
        session_type: &session,
        wayland_display: &wayland,
        x11_display: &x11,
        gtk_version: &gtk_version,
        gstreamer_version: &gstreamer_version,
        dark,
        config_dir: &config,
        projects_dir: &projects,
        hardware: &hardware,
        decoders: &decoders,
    });

    let body = content_box();
    body.set_spacing(6);

    // Heading: the icon on the left, the name and details on the right.
    let heading = gtk::Box::new(gtk::Orientation::Horizontal, 15);
    heading.set_margin_bottom(8);
    let icon = gtk::Picture::for_paintable(&crate::icon::texture());
    icon.set_size_request(78, 78);
    icon.set_can_shrink(true);
    icon.set_valign(gtk::Align::Center);
    heading.append(&icon);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    text.set_valign(gtk::Align::Center);
    text.set_hexpand(true);
    let name = gtk::Label::new(None);
    name.set_markup("<span size=\"17pt\" weight=\"bold\">Kani</span>");
    name.set_xalign(0.0);
    text.append(&name);
    let tagline = gtk::Label::new(None);
    tagline.set_markup("<span size=\"8.5pt\">Time-bending edits made simple.</span>");
    tagline.set_xalign(0.0);
    tagline.set_wrap(true);
    tagline.set_margin_bottom(4);
    text.append(&tagline);
    let details = gtk::Label::new(None);
    details.set_markup(&format!(
        "<span size=\"6.5pt\">Version {} (build {})\nby <a href=\"https://github.com/znraznra\">znraznra</a>\nLicensed under {}</span>",
        glib::markup_escape_text(env!("CARGO_PKG_VERSION")),
        glib::markup_escape_text(&short_build_date()),
        glib::markup_escape_text(cutroom_core::credits::LICENSE_NAME),
    ));
    details.set_xalign(0.0);
    text.append(&details);
    heading.append(&text);
    body.append(&heading);

    let notebook = gtk::Notebook::new();
    notebook.set_vexpand(true);

    // ---- System Info tab
    let system_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    system_page.set_margin_top(8);
    let label = gtk::Label::new(Some(&system_text));
    label.set_selectable(true);
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&label));
    scroll.set_min_content_height(300);
    scroll.set_vexpand(true);
    system_page.append(&scroll);
    let copy = gtk::Button::with_label("Copy system information");
    copy.set_halign(gtk::Align::Start);
    {
        let window = window.clone();
        let text = format!(
            "Kani {}\n{}\n\n{}",
            version_line(),
            author_line(),
            system_text
        );
        copy.connect_clicked(move |_| window.clipboard().set_text(&text));
    }
    system_page.append(&copy);
    notebook.append_page(&system_page, Some(&gtk::Label::new(Some("System Info"))));

    // ---- Credits tab
    let credits_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    credits_page.set_margin_top(8);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
    list.set_margin_end(10);
    let licence = gtk::Label::new(Some(cutroom_core::credits::LICENSE_SUMMARY));
    licence.set_wrap(true);
    licence.set_xalign(0.0);
    licence.set_selectable(true);
    list.append(&licence);
    for section in cutroom_core::credits::SECTIONS {
        let title = gtk::Label::new(Some(section.title));
        title.set_xalign(0.0);
        title.set_margin_top(10);
        title.add_css_class("heading");
        list.append(&title);
        let note = gtk::Label::new(Some(section.note));
        note.set_xalign(0.0);
        note.set_wrap(true);
        note.add_css_class("dim-label");
        list.append(&note);
        for e in section.entries {
            let line = gtk::Label::new(None);
            let esc = |t: &str| glib::markup_escape_text(t).to_string();
            let mut markup = format!(
                "<b>{}</b>  {}\n<small>{}</small>",
                esc(e.name),
                esc(e.what),
                esc(e.license)
            );
            if !e.url.is_empty() {
                markup.push_str(&format!(
                    "  <small><a href=\"{0}\">{0}</a></small>",
                    esc(e.url)
                ));
            }
            line.set_markup(&markup);
            line.set_xalign(0.0);
            line.set_wrap(true);
            line.set_margin_top(4);
            list.append(&line);
        }
    }
    let credits_scroll = gtk::ScrolledWindow::new();
    credits_scroll.set_child(Some(&list));
    credits_scroll.set_min_content_height(300);
    credits_scroll.set_vexpand(true);
    credits_page.append(&credits_scroll);
    let copy_credits = gtk::Button::with_label("Copy credits");
    copy_credits.set_halign(gtk::Align::Start);
    {
        let window = window.clone();
        copy_credits.connect_clicked(move |_| {
            window
                .clipboard()
                .set_text(&cutroom_core::credits::as_text())
        });
    }
    credits_page.append(&copy_credits);
    notebook.append_page(&credits_page, Some(&gtk::Label::new(Some("Credits"))));
    body.append(&notebook);

    let close = gtk::Button::with_label("Close");
    close.set_halign(gtk::Align::End);
    {
        let window = window.clone();
        close.connect_clicked(move |_| window.close());
    }
    body.append(&close);
    window.set_child(Some(&body));
    window.present();
}

// ----- shortcuts ---------------------------------------------------------------------

pub fn shortcuts(app: &Rc<App>) {
    let window = dialog_window(app, "Keyboard shortcuts", 560);
    window.set_resizable(true);
    let grid = gtk::Grid::new();
    grid.set_row_spacing(4);
    grid.set_column_spacing(18);
    for (col, title) in ["Keys", "Action", "Source"].iter().enumerate() {
        let l = gtk::Label::new(Some(title));
        l.set_xalign(0.0);
        l.add_css_class("heading");
        grid.attach(&l, col as i32, 0, 1, 1);
    }
    for (i, binding) in BINDINGS.iter().enumerate() {
        let row = i as i32 + 1;
        let keys = gtk::Label::new(Some(&shortcuts::display(binding)));
        keys.set_xalign(0.0);
        let action = gtk::Label::new(Some(binding.label));
        action.set_xalign(0.0);
        let origin = gtk::Label::new(Some(match binding.origin {
            Origin::CapCut => "CapCut",
            Origin::Convention => "Convention",
        }));
        origin.set_xalign(0.0);
        origin.add_css_class("dim-label");
        grid.attach(&keys, 0, row, 1, 1);
        grid.attach(&action, 1, row, 1, 1);
        grid.attach(&origin, 2, row, 1, 1);
    }
    let body = content_box();
    let note = caption("Defaults follow CapCut's desktop shortcuts where we could confirm them. \"Convention\" means a common editor shortcut we haven't verified against CapCut.");
    note.set_wrap(true);
    body.append(&note);
    body.append(&grid);
    let planned_title = gtk::Label::new(Some("Not enabled yet"));
    planned_title.set_xalign(0.0);
    planned_title.add_css_class("heading");
    body.append(&planned_title);
    for planned in PLANNED {
        let l = gtk::Label::new(Some(&format!(
            "{}: {} (needs {})",
            planned.keys, planned.what, planned.needs
        )));
        l.set_xalign(0.0);
        l.set_wrap(true);
        body.append(&l);
    }
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&body));
    scroll.set_min_content_height(480);
    window.set_child(Some(&scroll));
    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hardware_codec_list_is_never_empty() {
        let _ = gst::init();
        let lines = hardware_codec_lines();
        assert!(!lines.is_empty());
        assert!(lines.iter().all(|l| l.starts_with("  ")));
    }

    #[test]
    fn the_version_line_names_the_version_and_build_date() {
        let line = version_line();
        assert!(
            line.starts_with(&format!("Version {}", env!("CARGO_PKG_VERSION"))),
            "{line}"
        );
        assert!(line.contains("built 20") && line.ends_with(')'), "{line}");
    }

    #[test]
    fn the_author_comes_from_the_package_metadata() {
        let line = author_line();
        assert!(line.starts_with("By ") && line.len() > 3, "{line}");
    }

    #[test]
    fn the_system_text_lists_everything() {
        let hardware = vec!["  decoder: vah264dec".to_owned()];
        let text = system_info_text(&SystemFacts {
            display: "GdkWaylandDisplay",
            session_type: "wayland",
            wayland_display: "wayland-0",
            x11_display: ":0",
            gtk_version: "4.14.5",
            gstreamer_version: "GStreamer 1.24.2",
            dark: true,
            config_dir: "/home/me/.config/cutroom",
            projects_dir: "/home/me/Documents/Kani",
            hardware: &hardware,
            decoders: &["  H.264: vah264dec (hardware), avdec_h264".to_owned()],
        });
        for needle in [
            "GTK 4.14.5",
            "GdkWaylandDisplay",
            "wayland-0",
            "dark",
            "/home/me/.config/cutroom",
            "vah264dec",
            "Video decoders GStreamer will pick",
        ] {
            assert!(text.contains(needle), "missing {needle}: {text}");
        }
    }
}
