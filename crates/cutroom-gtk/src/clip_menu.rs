//! Builds the timeline clip's right-click menu (see `cutroom_core::clip_menu`)
//! as a native popover menu with sub-menus, and runs the commands.

use crate::app::App;
use cutroom_core::clip_menu::{clip_menu, ClipCommand, ClipMenuState, MenuNode};
use cutroom_timeline::ClipId;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Remove the menu opened earlier, if any.
pub fn dismiss(app: &Rc<App>) {
    let old = app.state.borrow_mut().clip_popover.take();
    if let Some(old) = old {
        old.popdown();
        old.unparent();
    }
}

type Submenus = Rc<RefCell<Vec<gtk::Popover>>>;

/// Close the sub-menus opened from `depth` downwards.
fn close_submenus(open: &Submenus, depth: usize) {
    loop {
        let popover = {
            let mut open = open.borrow_mut();
            if open.len() <= depth {
                return;
            }
            open.pop()
        };
        if let Some(popover) = popover {
            popover.popdown();
            popover.unparent();
        }
    }
}

/// One level of the menu: a column of rows. Hovering a row with a sub-menu
/// opens it as a flyout beside the row; hovering any other row closes the
/// flyouts that were open at this level.
fn build_level(
    nodes: &[MenuNode],
    depth: usize,
    open: &Submenus,
    close_all: &Rc<dyn Fn()>,
    on_run: &Rc<dyn Fn(ClipCommand)>,
) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.set_width_request(210);
    for node in nodes {
        let (row, button) = match node {
            MenuNode::Separator => {
                column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
                continue;
            }
            MenuNode::Item { label, enabled, .. } => {
                let button = gtk::Button::with_label(label);
                button.set_sensitive(*enabled);
                if let Some(l) = button.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
                    l.set_xalign(0.0);
                }
                (gtk::Box::new(gtk::Orientation::Horizontal, 0), button)
            }
            MenuNode::Submenu { label, .. } => {
                let inner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let text = gtk::Label::new(Some(label));
                text.set_xalign(0.0);
                text.set_hexpand(true);
                inner.append(&text);
                inner.append(&gtk::Image::from_icon_name("pan-end-symbolic"));
                let button = gtk::Button::new();
                button.set_child(Some(&inner));
                (gtk::Box::new(gtk::Orientation::Horizontal, 0), button)
            }
        };
        button.add_css_class("flat");
        button.set_hexpand(true);
        row.append(&button);

        // Hovering the row (even a greyed one) dismisses flyouts of this level.
        let motion = gtk::EventControllerMotion::new();
        let open_submenu = {
            let (open, close_all, on_run) = (open.clone(), close_all.clone(), on_run.clone());
            let (button, node) = (button.clone(), node.clone());
            move || {
                close_submenus(&open, depth);
                if let MenuNode::Submenu { items, .. } = &node {
                    let flyout = gtk::Popover::new();
                    flyout.set_parent(&button);
                    flyout.set_has_arrow(false);
                    flyout.set_autohide(false);
                    flyout.set_position(gtk::PositionType::Right);
                    flyout.add_css_class("cutroom-compact-menu");
                    flyout.set_child(Some(&build_level(
                        items,
                        depth + 1,
                        &open,
                        &close_all,
                        &on_run,
                    )));
                    open.borrow_mut().push(flyout.clone());
                    flyout.popup();
                }
            }
        };
        let hover = Rc::new(open_submenu);
        {
            let hover = hover.clone();
            motion.connect_enter(move |_, _, _| hover());
        }
        row.add_controller(motion);

        match node {
            MenuNode::Item { command, .. } => {
                let (close_all, on_run, command) = (close_all.clone(), on_run.clone(), *command);
                button.connect_clicked(move |_| {
                    close_all();
                    on_run(command);
                });
            }
            MenuNode::Submenu { .. } => {
                let hover = hover.clone();
                button.connect_clicked(move |_| hover());
            }
            MenuNode::Separator => {}
        }
        column.append(&row);
    }
    column
}

/// Open the menu for `clip` next to the pointer.
pub fn show(app: &Rc<App>, anchor: &gtk::Widget, clip: ClipId, x: f64, y: f64) {
    let state = {
        let st = app.state.borrow();
        let Some(session) = st.session.as_ref() else {
            return;
        };
        let file_present = session
            .doc
            .timeline
            .find_clip(clip)
            .and_then(|(_, c)| session.media_path(c.media))
            .is_some_and(|p| p.is_file());
        ClipMenuState {
            can_extract_audio: session.can_extract_audio(clip),
            file_present,
            can_transform: session.can_transform(clip),
            can_paste_attributes: session.can_paste_attributes(clip),
            deactivated: session
                .doc
                .timeline
                .find_clip(clip)
                .is_some_and(|(_, c)| c.disabled),
        }
    };
    let nodes = clip_menu(state);
    let on_run: Rc<dyn Fn(ClipCommand)> = {
        let app = app.clone();
        Rc::new(move |command| run(&app, clip, command))
    };

    dismiss(app);
    let root = gtk::Popover::new();
    root.set_parent(anchor);
    root.set_has_arrow(false);
    root.add_css_class("cutroom-compact-menu");
    // The menu is long: open it on whichever side of the pointer has more room.
    let window_y = anchor
        .compute_point(&app.window, &gtk::graphene::Point::new(x as f32, y as f32))
        .map_or(y, |p| f64::from(p.y()));
    root.set_position(if window_y > f64::from(app.window.height()) / 2.0 {
        gtk::PositionType::Top
    } else {
        gtk::PositionType::Bottom
    });
    root.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));

    let open: Submenus = Rc::new(RefCell::new(Vec::new()));
    let close_all: Rc<dyn Fn()> = {
        let (open, root) = (open.clone(), root.clone());
        Rc::new(move || {
            close_submenus(&open, 0);
            root.popdown();
        })
    };
    let column = build_level(&nodes, 0, &open, &close_all, &on_run);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_propagate_natural_height(true);
    scroller.set_max_content_height((app.window.height() - 60).max(200));
    scroller.set_child(Some(&column));
    root.set_child(Some(&scroller));
    {
        // When the menu closes (Escape, a click elsewhere) its flyouts go too.
        let open = open.clone();
        root.connect_closed(move |_| {
            let open = open.clone();
            gtk::glib::idle_add_local_once(move || close_submenus(&open, 0));
        });
    }
    app.state.borrow_mut().clip_popover = Some(root.clone());
    root.popup();
}

/// Run a command from the menu on `clip`.
pub fn run(app: &Rc<App>, clip: ClipId, command: ClipCommand) {
    match command {
        ClipCommand::Copy | ClipCommand::Cut => {
            if let Some(session) = app.state.borrow_mut().session.as_mut() {
                session.selected = Some(clip);
            }
            app.run_action(if command == ClipCommand::Copy {
                cutroom_core::shortcuts::Action::Copy
            } else {
                cutroom_core::shortcuts::Action::Cut
            });
        }
        ClipCommand::CopyAttributes => app.copy_attributes_of(clip),
        ClipCommand::PasteAttributes => app.paste_attributes_on(clip),
        ClipCommand::Delete => {
            if let Some(session) = app.state.borrow_mut().session.as_mut() {
                session.selected = Some(clip);
                session.delete_selected(false);
            }
            app.sync_preview(true);
            app.refresh();
        }
        ClipCommand::ExtractAudio => {
            let result = app
                .state
                .borrow_mut()
                .session
                .as_mut()
                .map(|s| s.extract_audio(clip));
            if let Some(Err(message)) = result {
                app.set_status(message);
            }
            app.sync_preview(true);
            app.refresh();
        }
        ClipCommand::OpenFileLocation => {
            let media = app
                .state
                .borrow()
                .session
                .as_ref()
                .and_then(|s| s.doc.timeline.find_clip(clip).map(|(_, c)| c.media));
            if let Some(media) = media {
                crate::library_menu::run(app, media, crate::library_menu::Command::Reveal);
            }
        }
        ClipCommand::Deactivate => {
            if let Some(session) = app.state.borrow_mut().session.as_mut() {
                let ids = session.selection();
                let ids = if ids.contains(&clip) { ids } else { vec![clip] };
                let off = ids.iter().any(|id| {
                    session
                        .doc
                        .timeline
                        .find_clip(*id)
                        .is_some_and(|(_, c)| !c.disabled)
                });
                session.set_clips_disabled(&ids, off);
            }
            app.sync_preview(true);
            app.refresh();
        }
        ClipCommand::Mirror | ClipCommand::Rotate => {
            if let Some(session) = app.state.borrow_mut().session.as_mut() {
                if command == ClipCommand::Mirror {
                    session.mirror_clip(clip);
                } else {
                    session.rotate_clip(clip);
                }
            }
            app.refresh();
        }
        // Everything else belongs to a later phase and is greyed out.
        _ => {
            app.set_status("That isn't available yet.");
            app.refresh();
        }
    }
}
