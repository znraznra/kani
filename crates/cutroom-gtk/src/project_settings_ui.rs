//! Project settings window (Info tab > "Project settings"). Two tabs:
//! Details (name, location, imported material, canvas, frame rate, colour
//! space) and Performance (placeholders until those settings are defined).
//! Changes are staged and applied together on Save.

use crate::app::App;
use crate::dialogs::{buttons, caption, content_box, dialog_window};
use cutroom_core::new_project::{CANVAS_PRESETS, CUSTOM_MAX, CUSTOM_MIN, FRAME_RATES};
use cutroom_project::ColorSpace;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// Index of the "Custom" entry in the canvas dropdown.
const CUSTOM: usize = CANVAS_PRESETS.len();

#[allow(dead_code)] // fields are driven by the UI test
pub struct Dialog {
    pub window: gtk::Window,
    pub name: gtk::Entry,
    pub canvas: gtk::DropDown,
    pub width: gtk::SpinButton,
    pub height: gtk::SpinButton,
    pub rate: gtk::DropDown,
    pub color: gtk::DropDown,
    pub copy: gtk::DropDown,
    pub relink: gtk::CheckButton,
    pub save: gtk::Button,
    pub cancel: gtk::Button,
    pub destination: Rc<RefCell<Option<PathBuf>>>,
}

fn strings(items: impl IntoIterator<Item = String>) -> gtk::StringList {
    let list = gtk::StringList::new(&[]);
    for i in items {
        list.append(&i);
    }
    list
}

fn row(grid: &gtk::Grid, r: i32, label: &str, widget: &impl IsA<gtk::Widget>) {
    let l = caption(label);
    l.set_valign(gtk::Align::Center);
    grid.attach(&l, 0, r, 1, 1);
    widget.set_hexpand(true);
    grid.attach(widget, 1, r, 1, 1);
}

fn grid() -> gtk::Grid {
    let g = gtk::Grid::new();
    g.set_row_spacing(10);
    g.set_column_spacing(14);
    g.set_margin_top(12);
    g.set_margin_bottom(8);
    g
}

pub fn open(app: &Rc<App>) -> Option<Rc<Dialog>> {
    let (name, root, settings, copy_now) = {
        let st = app.state.borrow();
        let s = st.session.as_ref()?;
        (
            s.doc.name.clone(),
            s.folder.root().to_path_buf(),
            s.doc.settings.clone(),
            s.doc.storage.import_copy,
        )
    };
    let window = dialog_window(app, "Project settings", 560);
    let body = content_box();
    let tabs = gtk::Notebook::new();

    // ---- Details -----------------------------------------------------------
    let g = grid();
    let name_entry = gtk::Entry::new();
    name_entry.set_text(&name);
    row(&g, 0, "Project name", &name_entry);

    let path_label = gtk::Label::new(Some(&root.display().to_string()));
    path_label.set_xalign(0.0);
    path_label.set_wrap(true);
    path_label.set_selectable(true);
    path_label.add_css_class("caption");
    let move_btn = gtk::Button::with_label("Move...");
    move_btn.set_tooltip_text(Some(
        "Move the whole project folder to another place or drive",
    ));
    let path_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    path_label.set_hexpand(true);
    path_row.append(&path_label);
    path_row.append(&move_btn);
    row(&g, 1, "Project path", &path_row);
    let destination: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    {
        let (app, path_label, destination) =
            (app.clone(), path_label.clone(), destination.clone());
        move_btn.connect_clicked(move |_| {
            let (path_label, destination) = (path_label.clone(), destination.clone());
            app.choose_folder(
                "Move project to (choose a new or empty folder)",
                move |dest| {
                    path_label.set_text(&format!("{}  (will move on Save)", dest.display()));
                    *destination.borrow_mut() = Some(dest);
                },
            );
        });
    }

    let copy = gtk::DropDown::new(
        Some(strings([
            "Keep in original place".to_owned(),
            "Copy into the project".to_owned(),
        ])),
        gtk::Expression::NONE,
    );
    copy.set_selected(copy_now as u32);
    row(&g, 2, "Import material", &copy);
    let relink = gtk::CheckButton::with_label(
        "Also copy the media already in this project and relink it to the project's media folder",
    );
    relink.set_active(true);
    relink.set_sensitive(copy_now);
    if let Some(l) = relink.last_child().and_downcast::<gtk::Label>() {
        l.set_wrap(true);
    }
    g.attach(&relink, 1, 3, 1, 1);
    {
        let relink = relink.clone();
        copy.connect_selected_notify(move |c| relink.set_sensitive(c.selected() == 1));
    }

    let canvas_items = CANVAS_PRESETS
        .iter()
        .map(|p| p.label.to_owned())
        .chain(["Custom".to_owned()]);
    let canvas = gtk::DropDown::new(Some(strings(canvas_items)), gtk::Expression::NONE);
    let preset = CANVAS_PRESETS
        .iter()
        .position(|p| p.width == settings.width && p.height == settings.height)
        .unwrap_or(CUSTOM);
    canvas.set_selected(preset as u32);
    row(&g, 4, "Resolution", &canvas);
    let width = gtk::SpinButton::with_range(CUSTOM_MIN as f64, CUSTOM_MAX as f64, 2.0);
    let height = gtk::SpinButton::with_range(CUSTOM_MIN as f64, CUSTOM_MAX as f64, 2.0);
    width.set_value(settings.width as f64);
    height.set_value(settings.height as f64);
    let size_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    size_row.append(&width);
    size_row.append(&gtk::Label::new(Some("x")));
    size_row.append(&height);
    size_row.set_visible(preset == CUSTOM);
    g.attach(&size_row, 1, 5, 1, 1);
    {
        let (width, height, size_row) = (width.clone(), height.clone(), size_row.clone());
        canvas.connect_selected_notify(move |c| {
            let i = c.selected() as usize;
            size_row.set_visible(i == CUSTOM);
            if let Some(p) = CANVAS_PRESETS.get(i) {
                width.set_value(p.width as f64);
                height.set_value(p.height as f64);
            }
        });
    }

    let rate = gtk::DropDown::new(
        Some(strings(FRAME_RATES.iter().map(|(l, _)| format!("{l} fps")))),
        gtk::Expression::NONE,
    );
    rate.set_selected(
        FRAME_RATES
            .iter()
            .position(|(_, r)| *r == settings.frame_rate)
            .unwrap_or(4) as u32,
    );
    row(&g, 6, "Frame rate", &rate);

    let color = gtk::DropDown::new(
        Some(strings(ColorSpace::ALL.iter().map(|c| c.label().to_owned()))),
        gtk::Expression::NONE,
    );
    color.set_selected(
        ColorSpace::ALL
            .iter()
            .position(|c| *c == settings.color_space)
            .unwrap_or(0) as u32,
    );
    row(&g, 7, "Color space", &color);
    let note = caption(
        "Changing resolution or frame rate keeps every clip where it is. \
         Colour space is recorded in the project; colour-managed rendering arrives in a later phase.",
    );
    note.set_wrap(true);
    note.set_max_width_chars(60);
    g.attach(&note, 0, 8, 2, 1);
    tabs.append_page(&g, Some(&gtk::Label::new(Some("Details"))));

    // ---- Performance (placeholders) -------------------------------------------
    let p = grid();
    for (i, label) in ["Preview resolution", "Render cache", "Proxy files", "Hardware decoding"]
        .iter()
        .enumerate()
    {
        let l = gtk::Label::new(Some("Not available yet"));
        l.set_xalign(0.0);
        l.add_css_class("dim-label");
        l.set_sensitive(false);
        row(&p, i as i32, label, &l);
    }
    tabs.append_page(&p, Some(&gtk::Label::new(Some("Performance"))));

    body.append(&tabs);
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    body.append(&buttons(&save, &cancel));
    window.set_child(Some(&body));
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }

    let dialog = Rc::new(Dialog {
        window: window.clone(),
        name: name_entry.clone(),
        canvas,
        width,
        height,
        rate,
        color,
        copy,
        relink,
        save: save.clone(),
        cancel,
        destination,
    });
    {
        let (app, d) = (app.clone(), dialog.clone());
        save.connect_clicked(move |_| apply(&app, &d));
    }
    {
        let save = save.clone();
        name_entry.connect_changed(move |e| {
            save.set_sensitive(cutroom_project::folder_name_problem(&e.text()).is_none());
        });
    }
    window.present();
    Some(dialog)
}

/// Apply every staged change, then close.
pub fn apply(app: &Rc<App>, d: &Dialog) {
    let (w, h) = (d.width.value() as u32, d.height.value() as u32);
    let rate = FRAME_RATES[(d.rate.selected() as usize).min(FRAME_RATES.len() - 1)].1;
    let color = ColorSpace::ALL[(d.color.selected() as usize).min(ColorSpace::ALL.len() - 1)];
    let copy = d.copy.selected() == 1;
    let relink = d.relink.is_active();
    let dest = d.destination.borrow().clone();
    let mut notes: Vec<String> = Vec::new();
    {
        let mut st = app.state.borrow_mut();
        let Some(s) = st.session.as_mut() else { return };
        if s.set_name(d.name.text().as_str()) {
            notes.push("renamed".into());
        }
        if s.set_format(w, h, rate, color) {
            notes.push("format changed".into());
        }
        match s.set_import_copy(copy, relink) {
            Ok(n) if n > 0 => notes.push(format!("{n} file(s) copied into the project")),
            Ok(_) => {}
            Err(e) => notes.push(format!("copy failed: {e}")),
        }
        if let Some(dest) = dest {
            match s.move_project(&dest) {
                Ok(()) => notes.push("project moved".into()),
                Err(e) => notes.push(format!("move failed: {e}")),
            }
        }
    }
    if !notes.is_empty() {
        app.set_status(format!("Project settings: {}", notes.join(", ")));
    }
    d.window.close();
    app.sync_preview(true);
    app.refresh();
}
