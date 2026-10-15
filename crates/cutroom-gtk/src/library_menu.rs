//! Right-click menu and dialogs for items in the media panel, following what
//! video editors usually offer: add to the timeline, show where it is used,
//! rename, relink, reveal in the file manager, copy the path, properties,
//! and remove from the project.

use crate::app::App;
use crate::dialogs::{buttons, caption, content_box, dialog_window};
use cutroom_core::home_logic::{format_duration, format_size};
use cutroom_core::state::{media_label, MediaKind};
use cutroom_timeline::{FrameRate, MediaId};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gdk, gio};
use std::rc::Rc;

/// What the menu needs to know about the item to decide what to offer.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemState {
    pub missing: bool,
    pub used_by: usize,
    pub in_project: bool,
    pub has_session: bool,
    pub proxy: ProxyState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyState {
    /// Audio, pictures and videos not read yet have no proxy.
    NotApplicable,
    None,
    Creating,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    AddToTimeline,
    ShowInTimeline,
    Rename,
    Relink,
    CreateProxy,
    DeleteProxy,
    Reveal,
    CopyPath,
    Properties,
    Remove,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    pub command: Command,
    pub label: String,
    pub enabled: bool,
    pub danger: bool,
    /// A separator goes before this item.
    pub separator_before: bool,
}

/// The menu for one item. Missing media can't be added or revealed, and it
/// gets "Relink..." first, since that is the thing to do about it.
pub fn menu_items(state: &ItemState) -> Vec<MenuItem> {
    let item = |command, label: &str, enabled, danger, separator_before| MenuItem {
        command,
        label: label.to_owned(),
        enabled: enabled && state.has_session,
        danger,
        separator_before,
    };
    let used = match state.used_by {
        0 => "Show in Timeline".to_owned(),
        1 => "Show in Timeline (1 clip)".to_owned(),
        n => format!("Show in Timeline ({n} clips)"),
    };
    let mut items = Vec::new();
    if state.missing {
        items.push(item(Command::Relink, "Relink Media...", true, false, false));
        items.push(item(
            Command::AddToTimeline,
            "Add to Timeline",
            false,
            false,
            false,
        ));
    } else {
        items.push(item(
            Command::AddToTimeline,
            "Add to Timeline",
            true,
            false,
            false,
        ));
    }
    items.push(item(
        Command::ShowInTimeline,
        &used,
        state.used_by > 0,
        false,
        false,
    ));
    items.push(item(Command::Rename, "Rename...", true, false, true));
    if !state.missing {
        items.push(item(Command::Relink, "Relink Media...", true, false, false));
    }
    match state.proxy {
        ProxyState::NotApplicable => {}
        ProxyState::None => items.push(item(
            Command::CreateProxy,
            "Create Proxy",
            !state.missing,
            false,
            true,
        )),
        ProxyState::Creating => items.push(item(
            Command::CreateProxy,
            "Creating Proxy...",
            false,
            false,
            true,
        )),
        ProxyState::Ready => items.push(item(
            Command::DeleteProxy,
            "Delete Proxy",
            true,
            false,
            true,
        )),
    }
    items.push(item(
        Command::Reveal,
        "Show in File Manager",
        !state.missing,
        false,
        true,
    ));
    items.push(item(
        Command::CopyPath,
        "Copy File Path",
        true,
        false,
        false,
    ));
    items.push(item(
        Command::Properties,
        "Properties...",
        true,
        false,
        false,
    ));
    items.push(item(
        Command::Remove,
        "Remove from Project...",
        true,
        true,
        true,
    ));
    items
}

fn state_of(app: &Rc<App>, id: MediaId) -> ItemState {
    let st = app.state.borrow();
    match st.session.as_ref() {
        Some(s) => ItemState {
            missing: s.media_is_missing(id),
            used_by: s.media_usage(id),
            in_project: s.media_in_project(id),
            has_session: true,
            proxy: if app.proxy_jobs.borrow().contains(&id) {
                ProxyState::Creating
            } else if s.existing_proxy(id).is_some() {
                ProxyState::Ready
            } else if s.can_have_proxy(id) {
                ProxyState::None
            } else {
                ProxyState::NotApplicable
            },
        },
        None => ItemState {
            missing: false,
            used_by: 0,
            in_project: false,
            has_session: false,
            proxy: ProxyState::NotApplicable,
        },
    }
}

/// Remove the menu opened earlier, if any.
pub fn dismiss(app: &Rc<App>) {
    let old = app.state.borrow_mut().library_popover.take();
    if let Some(old) = old {
        old.popdown();
        old.unparent();
    }
}

/// Open the menu next to the pointer.
pub fn show(app: &Rc<App>, anchor: &gtk::Widget, id: MediaId, x: f64, y: f64) {
    let popover = gtk::Popover::new();
    popover.set_parent(anchor);
    popover.set_has_arrow(false);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    // Only one menu exists at a time: the previous one is taken down here (or
    // when the library is rebuilt), never while it is still closing.
    dismiss(app);
    app.state.borrow_mut().library_popover = Some(popover.clone());
    // The item's own tooltip would sit on top of the menu.
    anchor.set_has_tooltip(false);
    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for item in menu_items(&state_of(app, id)) {
        if item.separator_before {
            menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }
        let button = gtk::Button::with_label(&item.label);
        button.add_css_class("flat");
        button.set_sensitive(item.enabled);
        if item.danger {
            button.add_css_class("destructive-action");
        }
        if let Some(label) = button.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
            label.set_xalign(0.0);
        }
        let (app, popover) = (app.clone(), popover.clone());
        button.connect_clicked(move |_| {
            popover.popdown();
            run(&app, id, item.command);
        });
        menu.append(&button);
    }
    popover.set_child(Some(&menu));
    popover.popup();
}

pub fn run(app: &Rc<App>, id: MediaId, command: Command) {
    match command {
        Command::AddToTimeline => {
            if let Some(s) = app.state.borrow_mut().session.as_mut() {
                s.add_media_to_timeline(id);
            }
            app.sync_preview(true);
            app.refresh();
        }
        Command::ShowInTimeline => {
            if let Some(s) = app.state.borrow_mut().session.as_mut() {
                s.find_media_in_timeline(id);
            }
            app.sync_preview(true);
            app.refresh();
        }
        Command::Rename => rename(app, id),
        Command::Relink => relink(app, id),
        Command::CreateProxy => app.create_proxy(id),
        Command::DeleteProxy => app.delete_proxy(id),
        Command::Reveal => reveal(app, id),
        Command::CopyPath => {
            let path = app
                .state
                .borrow()
                .session
                .as_ref()
                .and_then(|s| s.media_path(id));
            if let Some(path) = path {
                app.window.clipboard().set_text(&path.display().to_string());
                app.set_status("Copied the file path");
                app.refresh();
            }
        }
        Command::Properties => properties(app, id),
        Command::Remove => remove(app, id),
    }
}

fn reveal(app: &Rc<App>, id: MediaId) {
    let Some(path) = app
        .state
        .borrow()
        .session
        .as_ref()
        .and_then(|s| s.media_path(id))
    else {
        return;
    };
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&path)));
    let app = app.clone();
    launcher.open_containing_folder(
        Some(&app.window.clone()),
        gio::Cancellable::NONE,
        move |result| {
            if let Err(error) = result {
                app.set_status(format!("Could not open the file manager: {error}"));
                app.refresh();
            }
        },
    );
}

fn rename(app: &Rc<App>, id: MediaId) {
    let current = {
        let st = app.state.borrow();
        let Some(session) = st.session.as_ref() else {
            return;
        };
        match session.doc.media_item(id) {
            Ok(item) => media_label(item),
            Err(_) => return,
        }
    };
    let window = dialog_window(app, "Rename", 420);
    let body = content_box();
    body.append(&caption(
        "Name shown in the media panel and on clips (the file itself keeps its name). Leave empty to use the file name.",
    ));
    let entry = gtk::Entry::new();
    entry.set_text(&current);
    entry.select_region(0, -1);
    body.append(&entry);
    let ok = gtk::Button::with_label("Rename");
    ok.add_css_class("suggested-action");
    let cancel = gtk::Button::with_label("Cancel");
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    let apply = {
        let (app, window, entry) = (app.clone(), window.clone(), entry.clone());
        Rc::new(move || {
            if let Some(s) = app.state.borrow_mut().session.as_mut() {
                s.rename_media(id, entry.text().as_str());
            }
            window.close();
            app.refresh();
        })
    };
    {
        let apply = apply.clone();
        ok.connect_clicked(move |_| apply());
    }
    entry.connect_activate(move |_| apply());
    body.append(&buttons(&ok, &cancel));
    window.set_child(Some(&body));
    window.present();
    entry.grab_focus();
}

fn relink(app: &Rc<App>, id: MediaId) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Media files"));
    for ext in cutroom_core::state::MEDIA_EXTENSIONS {
        filter.add_suffix(ext);
    }
    let dialog = gtk::FileDialog::builder()
        .title("Choose the file to link to")
        .default_filter(&filter)
        .build();
    let app = app.clone();
    dialog.open(
        Some(&app.window.clone()),
        gio::Cancellable::NONE,
        move |result| {
            let Ok(file) = result else { return };
            let Some(path) = file.path() else { return };
            let outcome = {
                let mut st = app.state.borrow_mut();
                st.session.as_mut().map(|s| s.relink_media(id, &path))
            };
            match outcome {
                Some(Ok(())) => {
                    app.set_status(format!("Linked to {}", path.display()));
                    app.reprobe(id);
                }
                Some(Err(error)) => app.set_status(format!("Could not relink: {error}")),
                None => {}
            }
            app.refresh();
        },
    );
}

fn remove(app: &Rc<App>, id: MediaId) {
    let (name, used, in_project) = {
        let st = app.state.borrow();
        let Some(session) = st.session.as_ref() else {
            return;
        };
        let Ok(item) = session.doc.media_item(id) else {
            return;
        };
        (
            media_label(item),
            session.media_usage(id),
            session.media_in_project(id),
        )
    };
    let mut detail = String::new();
    if used > 0 {
        detail.push_str(&format!(
            "It is used by {used} clip(s) on the timeline. Those clips are removed too, and this can't be undone.\n\n"
        ));
    }
    detail.push_str(if in_project {
        "The file is stored inside the project. You can keep it on disk or delete it."
    } else {
        "The original file is not touched."
    });
    let mut choices = vec!["Cancel", "Remove from project"];
    if in_project {
        choices.push("Remove and delete file");
    }
    let alert = gtk::AlertDialog::builder()
        .message(format!("Remove \"{name}\" from the project?"))
        .detail(detail)
        .buttons(choices)
        .cancel_button(0)
        .default_button(0)
        .build();
    let app = app.clone();
    alert.choose(
        Some(&app.window.clone()),
        gio::Cancellable::NONE,
        move |answer| {
            let delete_file = match answer {
                Ok(1) => false,
                Ok(2) => true,
                _ => return,
            };
            let outcome = {
                let mut st = app.state.borrow_mut();
                st.session.as_mut().map(|s| s.remove_media(id, delete_file))
            };
            match outcome {
                Some(Ok(clips)) => {
                    let mut message = format!("Removed \"{name}\" from the project");
                    if clips > 0 {
                        message.push_str(&format!(" and {clips} clip(s)"));
                    }
                    app.set_status(message);
                }
                Some(Err(error)) => app.set_status(error),
                None => {}
            }
            app.sync_preview(true);
            app.refresh();
        },
    );
}

/// "29.97", "30", "23.976".
pub fn fps_text(rate: FrameRate) -> String {
    if rate.den == 0 {
        return "unknown".into();
    }
    let fps = f64::from(rate.num) / f64::from(rate.den);
    let text = format!("{fps:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// The lines of the Properties window: (label, value).
pub fn property_lines(app: &Rc<App>, id: MediaId) -> Vec<(String, String)> {
    let st = app.state.borrow();
    let Some(session) = st.session.as_ref() else {
        return Vec::new();
    };
    let Ok(item) = session.doc.media_item(id) else {
        return Vec::new();
    };
    let kind = match session.media_kind_of(id) {
        MediaKind::Video => "Video",
        MediaKind::Audio => "Audio",
        MediaKind::Image => "Picture",
    };
    let path = session.media_path(id);
    let mut lines = vec![
        ("Name".to_owned(), media_label(item)),
        ("Type".to_owned(), kind.to_owned()),
    ];
    if let Some(path) = &path {
        let file = path.file_name().map(|n| n.to_string_lossy().into_owned());
        lines.push(("File".to_owned(), file.unwrap_or_default()));
        lines.push((
            "Folder".to_owned(),
            path.parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        ));
    }
    lines.push((
        "Stored".to_owned(),
        if session.media_in_project(id) {
            "Inside the project (a copy)"
        } else {
            "Linked to the original file"
        }
        .to_owned(),
    ));
    lines.push((
        "Status".to_owned(),
        if session.media_is_missing(id) {
            "File not found. Use Relink Media."
        } else {
            "OK"
        }
        .to_owned(),
    ));
    if let Some(size) = item.size {
        lines.push(("Size".to_owned(), format_size(size)));
    }
    if let Some(duration) = item.duration {
        lines.push(("Duration".to_owned(), format_duration(duration)));
    }
    match &item.probe {
        Some(probe) => {
            if probe.has_video {
                let mut video = format!("{} x {}", probe.width, probe.height);
                if let Some(rate) = probe.frame_rate {
                    video.push_str(&format!(", {} fps", fps_text(rate)));
                }
                lines.push((
                    if session.media_kind_of(id) == MediaKind::Image {
                        "Picture"
                    } else {
                        "Video"
                    }
                    .to_owned(),
                    video,
                ));
            }
            lines.push((
                "Audio".to_owned(),
                if probe.has_audio { "Yes" } else { "No sound" }.to_owned(),
            ));
        }
        None => lines.push(("Streams".to_owned(), "Not read yet".to_owned())),
    }
    if session.can_have_proxy(id) {
        lines.push((
            "Proxy".to_owned(),
            match session.existing_proxy(id) {
                Some(p) => match std::fs::metadata(&p) {
                    Ok(m) => format!("Ready ({})", format_size(m.len())),
                    Err(_) => "Ready".to_owned(),
                },
                None => "None".to_owned(),
            },
        ));
    }
    lines.push((
        "Used on timeline".to_owned(),
        match session.media_usage(id) {
            0 => "Not used".to_owned(),
            1 => "1 clip".to_owned(),
            n => format!("{n} clips"),
        },
    ));
    if let Some(hash) = &item.hash {
        lines.push(("Fingerprint".to_owned(), hash.chars().take(16).collect()));
    }
    lines
}

fn properties(app: &Rc<App>, id: MediaId) {
    let window = dialog_window(app, "Properties", 520);
    let body = content_box();
    let grid = gtk::Grid::new();
    grid.set_row_spacing(6);
    grid.set_column_spacing(16);
    let lines = property_lines(app, id);
    let line_count = lines.len() as i32;
    for (row, (label, value)) in lines.into_iter().enumerate() {
        grid.attach(&caption(&label), 0, row as i32, 1, 1);
        let v = gtk::Label::new(Some(&value));
        v.set_xalign(0.0);
        v.set_selectable(true);
        v.set_wrap(true);
        v.set_hexpand(true);
        grid.attach(&v, 1, row as i32, 1, 1);
    }
    // Which decoder plays this file takes a moment to find out.
    let (path, is_video) = {
        let st = app.state.borrow();
        let session = st.session.as_ref();
        (
            session
                .and_then(|s| s.media_path(id))
                .filter(|p| p.is_file()),
            session.is_some_and(|s| s.media_kind_of(id) == MediaKind::Video),
        )
    };
    if let (Some(path), true) = (path, is_video) {
        let row = line_count;
        grid.attach(&caption("Decoder"), 0, row, 1, 1);
        let value = gtk::Label::new(Some("Checking..."));
        value.set_xalign(0.0);
        value.set_selectable(true);
        grid.attach(&value, 1, row, 1, 1);
        gtk::glib::spawn_future_local(async move {
            let info = gio::spawn_blocking(move || crate::analysis::decoder_in_use(&path))
                .await
                .ok()
                .flatten();
            value.set_text(&decoder_text(info.as_ref()));
        });
    }
    body.append(&grid);
    let close = gtk::Button::with_label("Close");
    {
        let window = window.clone();
        close.connect_clicked(move |_| window.close());
    }
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::End);
    row.append(&close);
    body.append(&row);
    window.set_child(Some(&body));
    window.present();
    // Otherwise the first line starts out highlighted.
    close.grab_focus();
}

/// "vah264dec (hardware)", "avdec_h264 (software)" or a plain note.
pub fn decoder_text(info: Option<&crate::analysis::DecoderInfo>) -> String {
    match info {
        Some(d) if d.hardware => format!("{} (hardware)", d.factory),
        Some(d) => format!("{} (software)", d.factory),
        None => "Could not be determined".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(missing: bool, used_by: usize, in_project: bool) -> ItemState {
        ItemState {
            missing,
            used_by,
            in_project,
            has_session: true,
            proxy: ProxyState::NotApplicable,
        }
    }

    fn find(items: &[MenuItem], command: Command) -> &MenuItem {
        items.iter().find(|i| i.command == command).unwrap()
    }

    #[test]
    fn a_normal_item_offers_the_usual_commands_in_order() {
        let items = menu_items(&state(false, 0, false));
        let commands: Vec<Command> = items.iter().map(|i| i.command).collect();
        assert_eq!(
            commands,
            [
                Command::AddToTimeline,
                Command::ShowInTimeline,
                Command::Rename,
                Command::Relink,
                Command::Reveal,
                Command::CopyPath,
                Command::Properties,
                Command::Remove
            ]
        );
        assert!(find(&items, Command::AddToTimeline).enabled);
        assert!(
            !find(&items, Command::ShowInTimeline).enabled,
            "unused media is nowhere to show"
        );
        assert!(find(&items, Command::Remove).danger);
        assert!(find(&items, Command::Remove).separator_before);
    }

    #[test]
    fn videos_offer_to_create_a_proxy_then_to_delete_it() {
        let with = |proxy| {
            let mut s = state(false, 0, false);
            s.proxy = proxy;
            menu_items(&s)
        };
        let none = with(ProxyState::None);
        assert!(find(&none, Command::CreateProxy).enabled);
        assert_eq!(find(&none, Command::CreateProxy).label, "Create Proxy");
        let creating = with(ProxyState::Creating);
        assert!(
            !find(&creating, Command::CreateProxy).enabled,
            "one at a time"
        );
        let ready = with(ProxyState::Ready);
        assert_eq!(find(&ready, Command::DeleteProxy).label, "Delete Proxy");
        assert!(ready.iter().all(|i| i.command != Command::CreateProxy));
        let missing = {
            let mut s = state(true, 0, false);
            s.proxy = ProxyState::None;
            menu_items(&s)
        };
        assert!(
            !find(&missing, Command::CreateProxy).enabled,
            "nothing to make a proxy from"
        );
    }

    #[test]
    fn show_in_timeline_says_how_many_clips() {
        let label = |n| {
            menu_items(&state(false, n, false))
                .into_iter()
                .find(|i| i.command == Command::ShowInTimeline)
                .unwrap()
                .label
        };
        assert_eq!(label(0), "Show in Timeline");
        assert_eq!(label(1), "Show in Timeline (1 clip)");
        assert_eq!(label(3), "Show in Timeline (3 clips)");
    }

    #[test]
    fn missing_media_leads_with_relink_and_cannot_be_added_or_revealed() {
        let items = menu_items(&state(true, 2, false));
        assert_eq!(items[0].command, Command::Relink);
        assert!(!find(&items, Command::AddToTimeline).enabled);
        assert!(!find(&items, Command::Reveal).enabled);
        assert_eq!(
            items
                .iter()
                .filter(|i| i.command == Command::Relink)
                .count(),
            1,
            "offered once"
        );
        assert!(
            find(&items, Command::ShowInTimeline).enabled,
            "its clips are still on the timeline"
        );
        assert!(find(&items, Command::Remove).enabled);
    }

    #[test]
    fn nothing_is_enabled_without_an_open_project() {
        let mut s = state(false, 0, false);
        s.has_session = false;
        assert!(menu_items(&s).iter().all(|i| !i.enabled));
    }

    #[test]
    fn frame_rates_read_naturally() {
        assert_eq!(fps_text(FrameRate::FPS_30), "30");
        assert_eq!(fps_text(FrameRate::NTSC_29_97), "29.97");
        assert_eq!(fps_text(FrameRate::NTSC_23_976), "23.976");
        assert_eq!(fps_text(FrameRate::new(30, 0)), "unknown");
    }

    #[test]
    fn decoder_text_says_hardware_or_software() {
        use crate::analysis::DecoderInfo;
        let hw = DecoderInfo {
            factory: "vah264dec".into(),
            hardware: true,
        };
        let sw = DecoderInfo {
            factory: "avdec_h264".into(),
            hardware: false,
        };
        assert_eq!(decoder_text(Some(&hw)), "vah264dec (hardware)");
        assert_eq!(decoder_text(Some(&sw)), "avdec_h264 (software)");
        assert_eq!(decoder_text(None), "Could not be determined");
    }
}
