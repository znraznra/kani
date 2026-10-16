//! Kani: Time-bending edits made simple. A local-first short-form video editor on GTK4 and GStreamer.
#![cfg_attr(not(feature = "whisper"), allow(dead_code))]

mod analysis;
mod app;
mod captions_audio;
mod captions_ui;
mod clip_menu;
mod crossfade_ui;
mod dialogs;
mod editor;
mod export;
mod export_options;
mod export_ui;
mod fonts;
mod gpu;
mod gpu_compose;
mod home;
mod icon;
mod icons;
mod library_menu;
mod media;
mod menu;
mod navigator;
mod preview;
mod sample;
mod strips;
mod text_render;
mod text_ui;
mod theme;
mod thumbs;
mod timeline;
mod transform_overlay;
mod transform_ui;
mod ui_test;

use gstreamer as gst;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

thread_local! {
    static APP: RefCell<Option<Rc<app::App>>> = const { RefCell::new(None) };
}

fn activate(application: &gtk::Application, files: Vec<PathBuf>) {
    APP.with(|slot| {
        let existing = slot.borrow().clone();
        match existing {
            Some(app) => {
                app.window.present();
                if !files.is_empty() {
                    app.import_startup_files(files);
                }
            }
            None => {
                icon::install_as_default();
                *slot.borrow_mut() = Some(app::App::new(application, files));
            }
        }
    });
}

fn main() -> glib::ExitCode {
    // Before GTK starts, so the toolkit sees the bundled typeface.
    fonts::install();
    if let Err(e) = gst::init() {
        eprintln!("Could not start GStreamer: {e}");
        return glib::ExitCode::FAILURE;
    }
    // The picture sink that draws video inside a GTK widget is built into Kani.
    if let Err(e) = gstgtk4::plugin_register_static() {
        eprintln!("Could not register the GTK video sink: {e}");
    }
    let application = gtk::Application::builder()
        .application_id("dev.cutroom.Kani")
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    application.connect_activate(|a| activate(a, Vec::new()));
    application.connect_open(|a, files, _| {
        let paths = files.iter().filter_map(|f| f.path()).collect();
        activate(a, paths);
    });
    application.run()
}
