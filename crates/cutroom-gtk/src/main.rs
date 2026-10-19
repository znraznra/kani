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
mod hwcheck;
mod hwdecode;
mod icon;
mod icons;
mod library_menu;
mod media;
mod menu;
mod navigator;
mod preview;
mod preview_gpu;
mod preview_tools;
mod project_settings_ui;
mod range_select;
mod replace_ui;
mod sample;
mod settings_ui;
mod sound;
mod startup;
mod strips;
mod text_render;
mod text_ui;
mod theme;
mod thumbs;
mod timeline;
mod transform_overlay;
mod transform_ui;
mod ui;
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
                startup::step("building the window");
                let app = app::App::new(application, files);
                startup::step("window built");
                {
                    let note = |w: &gtk::ApplicationWindow| {
                        let renderer = w
                            .renderer()
                            .map(|r| r.type_().name().to_string())
                            .unwrap_or_else(|| "?".into());
                        startup::step(&format!("window on screen, drawn by {renderer}"));
                    };
                    if app.window.is_mapped() {
                        note(&app.window);
                    } else {
                        app.window.connect_map(move |w| note(w));
                    }
                }
                *slot.borrow_mut() = Some(app);
            }
        }
    });
}

fn main() -> glib::ExitCode {
    // Before anything else: bundled prerequisites, safe mode, the start-up trace.
    {
        let config = cutroom_platform::AppPaths::detect().map(|p| p.config_dir);
        let safe_setting = config
            .as_deref()
            .map(cutroom_core::settings::Settings::load)
            .is_some_and(|s| s.safe_mode);
        startup::prepare(config.as_deref(), safe_setting);
    }
    // Before GTK starts, so the toolkit sees the bundled typeface.
    fonts::install();
    startup::step("starting GStreamer");
    if let Err(e) = gst::init() {
        eprintln!("Could not start GStreamer: {e}");
        startup::step(&format!("GStreamer did not start: {e}"));
        return glib::ExitCode::FAILURE;
    }
    startup::step(&format!(
        "{} started, {} plug-ins",
        gst::version_string(),
        gst::Registry::get().plugins().len()
    ));
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
    startup::step("starting GTK");
    let code = application.run();
    startup::clean_exit();
    code
}
