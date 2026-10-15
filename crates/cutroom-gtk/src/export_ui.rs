//! The Export window: where to save, how big, how fast, how good, which
//! encoder; then progress with a Cancel button, and the result.

use crate::app::App;
use crate::export::{self, Encoder, ExportError, Job, Progress, Quality, Report, Source};
use cutroom_core::state::{MediaKind, Session};
use cutroom_timeline::{FrameRate, MediaId};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Outcome = Arc<Mutex<Option<Result<Report, ExportError>>>>;

pub struct Dialog {
    pub window: gtk::Window,
    pub name: gtk::Entry,
    pub folder: gtk::Label,
    pub size: gtk::DropDown,
    pub rate: gtk::DropDown,
    pub quality: gtk::DropDown,
    pub encoder: gtk::DropDown,
    pub summary: gtk::Label,
    pub export: gtk::Button,
    pub cancel: gtk::Button,
    form: gtk::Box,
    run: gtk::Box,
    bar: gtk::ProgressBar,
    note: gtk::Label,
    stop: gtk::Button,
    folder_path: RefCell<PathBuf>,
    sizes: Vec<(u32, u32)>,
    rates: Vec<FrameRate>,
    encoders: RefCell<Vec<Encoder>>,
    seconds: f64,
    sample_rate: u32,
    /// What is running right now, if anything.
    running: RefCell<Option<Running>>,
    /// The finished file, once there is one.
    pub saved: RefCell<Option<PathBuf>>,
    dimmed: RefCell<Option<gtk::Widget>>,
    ready: Cell<bool>,
    /// Set once the window has been closed.
    gone: Cell<bool>,
}

struct Running {
    progress: Arc<Progress>,
    outcome: Outcome,
    started: Instant,
}

thread_local! {
    static ACTIVE: RefCell<Weak<Dialog>> = RefCell::new(Weak::new());
}

/// The Export window, while it is open.
pub fn active() -> Option<Rc<Dialog>> {
    // The window's own signal handlers keep the dialog alive, so "still
    // exists" is not "still open": a destroyed window must never be handed out.
    ACTIVE
        .with(|a| a.borrow().upgrade())
        .filter(|d| !d.gone.get())
}

/// Forget the open window (it has been closed).
fn forget(d: &Dialog) {
    d.gone.set(true);
    ACTIVE.with(|a| {
        let mut slot = a.borrow_mut();
        if std::ptr::eq(slot.as_ptr(), d) {
            *slot = Weak::new();
        }
    });
}

fn clean_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if "/\\:*?\"<>|".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() {
        "Kani video".into()
    } else {
        cleaned
    }
}

fn rate_label(rate: FrameRate) -> String {
    let fps = rate.as_f64();
    if (fps - fps.round()).abs() < 0.001 {
        format!("{} fps", fps.round() as u32)
    } else {
        format!("{fps:.2} fps")
    }
}

fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// "6 min 03 s", "42 s" or "1 h 02 min 10 s".
fn elapsed_words(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{} h {:02} min {:02} s", s / 3600, (s / 60) % 60, s % 60)
    } else if s >= 60 {
        format!("{} min {:02} s", s / 60, s % 60)
    } else {
        format!("{s} s")
    }
}

fn megabytes(bytes: f64) -> String {
    let mb = bytes / 1_000_000.0;
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1000.0)
    } else if mb >= 10.0 {
        format!("{mb:.0} MB")
    } else {
        format!("{mb:.1} MB")
    }
}

/// Everything about the open project an export needs, or why it cannot be done.
fn gather(session: &Session) -> Result<(HashMap<MediaId, Source>, Vec<String>), String> {
    let mut sources = HashMap::new();
    let mut missing = Vec::new();
    for track in &session.doc.timeline.tracks {
        for clip in &track.clips {
            if clip.text.is_some() || clip.media == MediaId(0) || sources.contains_key(&clip.media)
            {
                continue;
            }
            let Some(path) = session.media_path(clip.media).filter(|p| p.is_file()) else {
                let name = session
                    .doc
                    .media_item(clip.media)
                    .ok()
                    .map(|m| match &m.path {
                        cutroom_project::MediaPath::Relative(p)
                        | cutroom_project::MediaPath::Absolute(p) => p.clone(),
                    })
                    .unwrap_or_else(|| "a file".into());
                if !missing.contains(&name) {
                    missing.push(name);
                }
                continue;
            };
            let kind = session.media_kind_of(clip.media);
            let has_audio = session
                .doc
                .media_item(clip.media)
                .ok()
                .and_then(|m| m.probe.as_ref())
                .is_none_or(|p| p.has_audio);
            sources.insert(
                clip.media,
                Source {
                    path,
                    kind,
                    has_audio: has_audio && kind != MediaKind::Image,
                },
            );
        }
    }
    if missing.is_empty() {
        Ok((sources, missing))
    } else {
        Err(format!(
            "Cannot export: {} not found. Relink or remove {} first.",
            missing.join(", "),
            if missing.len() == 1 { "it" } else { "them" }
        ))
    }
}

fn default_folder(app: &App) -> PathBuf {
    app.state
        .borrow()
        .settings
        .export_folder
        .clone()
        .filter(|p| p.is_dir())
        .or_else(|| glib::user_special_dir(glib::UserDirectory::Videos).filter(|p| p.is_dir()))
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Open the Export window.
pub fn open(app: &Rc<App>) {
    let clicked = Instant::now();
    if let Some(existing) = active() {
        existing.window.present();
        return;
    }
    let (project_name, canvas, project_rate, sample_rate, seconds, sources, timeline) = {
        let st = app.state.borrow();
        let Some(session) = st.session.as_ref() else {
            return;
        };
        let duration = session.doc.timeline.duration();
        if duration <= cutroom_timeline::Ticks::ZERO {
            drop(st);
            app.set_status("There is nothing on the timeline to export yet.");
            app.update_status();
            return;
        }
        let sources = match gather(session) {
            Ok((sources, _)) => sources,
            Err(message) => {
                drop(st);
                app.set_status(message);
                app.update_status();
                return;
            }
        };
        (
            session.doc.name.clone(),
            (session.doc.settings.width, session.doc.settings.height),
            session.doc.settings.frame_rate,
            session.doc.settings.sample_rate,
            duration.as_seconds_f64(),
            sources,
            session.doc.timeline.clone(),
        )
    };
    // Stop playback: the preview would only compete for the processor.
    {
        let mut st = app.state.borrow_mut();
        if let Some(s) = st.session.as_mut() {
            s.playing = false;
        }
    }
    app.sync_preview(false);

    let window = gtk::Window::builder()
        .title("Export video")
        .transient_for(&app.window)
        .modal(true)
        .destroy_with_parent(true)
        .default_width(460)
        .default_height(380)
        .build();

    let form = gtk::Box::new(gtk::Orientation::Vertical, 10);
    form.set_margin_top(18);
    form.set_margin_bottom(16);
    form.set_margin_start(20);
    form.set_margin_end(20);

    let grid = gtk::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(12);
    let label = |text: &str| {
        let l = gtk::Label::new(Some(text));
        l.set_xalign(0.0);
        l.add_css_class("dim-label");
        l
    };

    let name = gtk::Entry::new();
    name.set_hexpand(true);
    name.set_text(&clean_name(&project_name));
    let name_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    name_row.append(&name);
    let ext = gtk::Label::new(Some(".mp4"));
    ext.add_css_class("dim-label");
    name_row.append(&ext);
    grid.attach(&label("Name"), 0, 0, 1, 1);
    grid.attach(&name_row, 1, 0, 1, 1);

    let folder_path = default_folder(app);
    let folder = gtk::Label::new(Some(&folder_path.display().to_string()));
    folder.set_xalign(0.0);
    folder.set_hexpand(true);
    folder.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    folder.set_tooltip_text(Some(&folder_path.display().to_string()));
    let change = gtk::Button::with_label("Change...");
    let folder_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    folder_row.append(&folder);
    folder_row.append(&change);
    grid.attach(&label("Save in"), 0, 1, 1, 1);
    grid.attach(&folder_row, 1, 1, 1, 1);

    let sizes = export::size_options(canvas.0, canvas.1);
    let size_labels: Vec<String> = sizes
        .iter()
        .enumerate()
        .map(|(i, (w, h))| {
            if i == 0 {
                format!("{w} x {h} (project size)")
            } else {
                format!("{w} x {h}")
            }
        })
        .collect();
    let size =
        gtk::DropDown::from_strings(&size_labels.iter().map(String::as_str).collect::<Vec<_>>());
    grid.attach(&label("Size"), 0, 2, 1, 1);
    grid.attach(&size, 1, 2, 1, 1);

    let mut rates = vec![project_rate];
    for r in [
        FrameRate::FPS_24,
        FrameRate::FPS_25,
        FrameRate::FPS_30,
        FrameRate::FPS_50,
        FrameRate::FPS_60,
    ] {
        if (r.as_f64() - project_rate.as_f64()).abs() > 0.01 {
            rates.push(r);
        }
    }
    let rate_labels: Vec<String> = rates
        .iter()
        .enumerate()
        .map(|(i, r)| {
            if i == 0 {
                format!("{} (project)", rate_label(*r))
            } else {
                rate_label(*r)
            }
        })
        .collect();
    let rate =
        gtk::DropDown::from_strings(&rate_labels.iter().map(String::as_str).collect::<Vec<_>>());
    grid.attach(&label("Frame rate"), 0, 3, 1, 1);
    grid.attach(&rate, 1, 3, 1, 1);

    let quality_labels: Vec<&str> = Quality::ALL.iter().map(|q| q.label()).collect();
    let quality = gtk::DropDown::from_strings(&quality_labels);
    let wanted = app.state.borrow().settings.export_quality.clone();
    quality.set_selected(
        Quality::ALL
            .iter()
            .position(|q| q.label() == wanted)
            .unwrap_or(1) as u32,
    );
    grid.attach(&label("Quality"), 0, 4, 1, 1);
    grid.attach(&quality, 1, 4, 1, 1);

    let encoder = gtk::DropDown::from_strings(&["Looking for encoders..."]);
    encoder.set_sensitive(false);
    grid.attach(&label("Encoder"), 0, 5, 1, 1);
    grid.attach(&encoder, 1, 5, 1, 1);

    // Does the graphics card work for compositing? (Export does not use it
    // yet; this button measures it and says.)
    let gpu_button = gtk::Button::with_label("Test GPU compositing");
    gpu_button.set_halign(gtk::Align::Start);
    let gpu_note = gtk::Label::new(Some(if crate::gpu::available() {
        "OpenGL plug-ins found. Not used for export yet."
    } else {
        "OpenGL plug-ins not found."
    }));
    gpu_note.set_xalign(0.0);
    gpu_note.set_wrap(true);
    gpu_note.set_max_width_chars(48);
    gpu_note.add_css_class("dim-label");
    gpu_note.add_css_class("caption");
    let gpu_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    gpu_box.append(&gpu_button);
    gpu_box.append(&gpu_note);
    grid.attach(&label("Graphics card"), 0, 6, 1, 1);
    grid.attach(&gpu_box, 1, 6, 1, 1);
    {
        let note = gpu_note.clone();
        gpu_button.connect_clicked(move |button| {
            button.set_sensitive(false);
            note.set_text("Testing ...");
            let found: Arc<Mutex<Option<crate::gpu::GpuReport>>> = Arc::new(Mutex::new(None));
            {
                let found = found.clone();
                std::thread::spawn(move || {
                    *found.lock().unwrap() = Some(crate::gpu::test());
                });
            }
            let (button, note) = (button.clone(), note.clone());
            glib::timeout_add_local(Duration::from_millis(150), move || {
                let Some(report) = found.lock().unwrap().take() else {
                    return glib::ControlFlow::Continue;
                };
                for line in &report.details {
                    eprintln!("kani: GPU compositing test: {line}");
                }
                eprintln!(
                    "kani: GPU compositing test: {} ({})",
                    report.summary,
                    if report.usable {
                        "usable"
                    } else {
                        "not usable"
                    }
                );
                note.set_text(&report.summary);
                button.set_sensitive(true);
                glib::ControlFlow::Break
            });
        });
    }
    form.append(&grid);

    let summary = gtk::Label::new(None);
    summary.set_xalign(0.0);
    summary.set_wrap(true);
    summary.add_css_class("dim-label");
    summary.add_css_class("caption");
    form.append(&summary);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let export = gtk::Button::with_label("Export");
    export.add_css_class("suggested-action");
    export.set_sensitive(false);
    buttons.append(&cancel);
    buttons.append(&export);
    form.append(&buttons);

    // The progress page, shown instead of the form while exporting.
    let run = gtk::Box::new(gtk::Orientation::Vertical, 10);
    run.set_margin_top(18);
    run.set_margin_bottom(16);
    run.set_margin_start(20);
    run.set_margin_end(20);
    let bar = gtk::ProgressBar::new();
    bar.set_show_text(true);
    let note = gtk::Label::new(None);
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.set_max_width_chars(60);
    note.add_css_class("dim-label");
    note.add_css_class("caption");
    let stop = gtk::Button::with_label("Cancel");
    stop.set_halign(gtk::Align::End);
    run.append(&bar);
    run.append(&note);
    run.append(&stop);
    run.set_visible(false);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&form);
    root.append(&run);
    window.set_child(Some(&root));

    let dialog = Rc::new(Dialog {
        window,
        name,
        folder,
        size,
        rate,
        quality,
        encoder,
        summary,
        export,
        cancel,
        form,
        run,
        bar,
        note,
        stop,
        folder_path: RefCell::new(folder_path),
        sizes,
        rates,
        encoders: RefCell::new(Vec::new()),
        seconds,
        sample_rate,
        running: RefCell::new(None),
        saved: RefCell::new(None),
        dimmed: RefCell::new(None),
        ready: Cell::new(false),
        gone: Cell::new(false),
    });
    ACTIVE.with(|a| *a.borrow_mut() = Rc::downgrade(&dialog));
    connect(app, &dialog, timeline, sources);
    update_summary(&dialog);
    // Dim the workspace only once the window is really on screen, so a window
    // that fails to appear can never leave the app greyed out and stuck.
    {
        let (app, d) = (app.clone(), dialog.clone());
        dialog.window.connect_map(move |_| {
            if d.dimmed.borrow().is_none() {
                let dimmed = app.window.child();
                if let Some(w) = &dimmed {
                    w.set_opacity(0.4);
                }
                *d.dimmed.borrow_mut() = dimmed;
            }
        });
    }
    {
        // Esc closes it (or cancels a running export).
        let d = dialog.clone();
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                if d.running.borrow().is_some() {
                    stop_export(&d);
                } else {
                    close(&d);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        dialog.window.add_controller(keys);
    }
    {
        // However it goes away, the workspace comes back to full strength.
        let d = dialog.clone();
        dialog.window.connect_destroy(move |_| {
            forget(&d);
            if let Some(w) = d.dimmed.borrow_mut().take() {
                w.set_opacity(1.0);
            }
        });
    }
    {
        // If the window has not appeared after a few seconds, try once more as
        // a plain window (no parent, not modal); if that fails too, say so
        // instead of leaving the app stuck.
        let (app, d) = (app.clone(), Rc::downgrade(&dialog));
        glib::timeout_add_local_once(Duration::from_secs(3), move || {
            let Some(strong) = d.upgrade() else { return };
            if strong.window.is_mapped() {
                return;
            }
            eprintln!(
                "kani: the Export window did not appear within 3 s ({:.1} s since the click); trying it as a plain window",
                clicked.elapsed().as_secs_f64()
            );
            strong.window.set_modal(false);
            strong.window.set_transient_for(None::<&gtk::Window>);
            strong.window.present();
            let app = app.clone();
            glib::timeout_add_local_once(Duration::from_secs(3), move || {
                let Some(strong) = d.upgrade() else { return };
                if !strong.window.is_mapped() {
                    eprintln!(
                        "kani: the Export window still did not appear ({:.1} s since the click); closing it",
                        clicked.elapsed().as_secs_f64()
                    );
                    app.set_status("The Export window could not be shown. Run kani from a terminal and send me any messages it prints.");
                    app.update_status();
                    close(&strong);
                }
            });
        });
    }
    {
        // Report any time the interface freezes while the window opens.
        let (d, last) = (Rc::downgrade(&dialog), Rc::new(Cell::new(Instant::now())));
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let gap = last.get().elapsed().as_secs_f64();
            last.set(Instant::now());
            if gap > 1.0 {
                eprintln!("kani: the interface was frozen for {gap:.1} s");
            }
            match d.upgrade() {
                Some(d) if d.window.is_visible() || clicked.elapsed().as_secs() < 10 => {
                    glib::ControlFlow::Continue
                }
                _ => glib::ControlFlow::Break,
            }
        });
    }
    eprintln!(
        "kani: opening the Export window (prepared in {:.0} ms)",
        clicked.elapsed().as_secs_f64() * 1000.0
    );
    // The encoder test starts once the window is on screen (or, failing that,
    // after a few seconds), so it does not compete with the window for the
    // graphics driver while it opens.
    {
        let started = Rc::new(Cell::new(false));
        let begin = {
            let (app, d, started) = (app.clone(), dialog.clone(), started.clone());
            Rc::new(move |delay_ms: u64| {
                if started.replace(true) {
                    return;
                }
                let (app, d) = (app.clone(), d.clone());
                glib::timeout_add_local_once(Duration::from_millis(delay_ms), move || {
                    find_encoders(&app, &d);
                });
            })
        };
        {
            let begin = begin.clone();
            dialog.window.connect_map(move |_| {
                eprintln!(
                    "kani: the Export window is on screen ({:.1} s since the click)",
                    clicked.elapsed().as_secs_f64()
                );
                begin(400);
            });
        }
        glib::timeout_add_local_once(Duration::from_secs(4), move || begin(0));
    }
    dialog.window.present();
    dialog.name.grab_focus();
}

/// Test the encoders on a worker thread (a second or two), then offer them.
fn find_encoders(app: &Rc<App>, dialog: &Rc<Dialog>) {
    let found: Arc<Mutex<Option<Vec<Encoder>>>> = Arc::new(Mutex::new(None));
    {
        let found = found.clone();
        std::thread::spawn(move || {
            let list = export::working_encoders().to_vec();
            *found.lock().unwrap() = Some(list);
        });
    }
    let (app, dialog) = (app.clone(), dialog.clone());
    glib::timeout_add_local(Duration::from_millis(100), move || {
        if dialog.window.surface().is_none() && !dialog.window.is_visible() {
            return glib::ControlFlow::Break;
        }
        let Some(list) = found.lock().unwrap().take() else {
            return glib::ControlFlow::Continue;
        };
        let mut labels = vec![match list.first() {
            Some(best) => format!("Automatic ({})", best.label),
            None => "None found".to_string(),
        }];
        labels.extend(list.iter().map(|e| e.label.to_string()));
        dialog.encoder.set_model(Some(&gtk::StringList::new(
            &labels.iter().map(String::as_str).collect::<Vec<_>>(),
        )));
        let wanted = app.state.borrow().settings.export_encoder.clone();
        let index = list
            .iter()
            .position(|e| e.id == wanted)
            .map_or(0, |i| i as u32 + 1);
        dialog.encoder.set_selected(index);
        dialog.encoder.set_sensitive(!list.is_empty());
        dialog.export.set_sensitive(!list.is_empty());
        if list.is_empty() {
            dialog.summary.set_text(
                "No H.264 video encoder was found. Install GStreamer's x264 plug-in (gst-plugins-ugly), or on Windows run the Kani setup again.",
            );
        }
        *dialog.encoders.borrow_mut() = list;
        dialog.ready.set(true);
        update_summary(&dialog);
        glib::ControlFlow::Break
    });
}

fn chosen_size(d: &Dialog) -> (u32, u32) {
    d.sizes[(d.size.selected() as usize).min(d.sizes.len() - 1)]
}

fn chosen_rate(d: &Dialog) -> FrameRate {
    d.rates[(d.rate.selected() as usize).min(d.rates.len() - 1)]
}

fn chosen_quality(d: &Dialog) -> Quality {
    Quality::ALL[(d.quality.selected() as usize).min(Quality::ALL.len() - 1)]
}

/// The encoders to try: the one picked, then (for "Automatic") all the others.
fn chosen_encoders(d: &Dialog) -> Vec<Encoder> {
    let all = d.encoders.borrow().clone();
    match d.encoder.selected() {
        0 => all,
        n => {
            let mut list = vec![all[(n as usize - 1).min(all.len().saturating_sub(1))]];
            // If the one picked fails, fall back to software rather than give up.
            let first = list[0];
            list.extend(all.iter().filter(|e| !e.hardware && **e != first).copied());
            list
        }
    }
}

fn update_summary(d: &Dialog) {
    if !d.ready.get() {
        d.summary.set_text("");
        return;
    }
    let (w, h) = chosen_size(d);
    let fps = chosen_rate(d).as_f64();
    let kbps = export::bitrate_kbps(w, h, fps, chosen_quality(d));
    let bytes = f64::from(kbps + 192) * 1000.0 / 8.0 * d.seconds;
    d.summary.set_text(&format!(
        "{} long. About {} as MP4 (H.264 video, AAC sound).",
        clock(d.seconds),
        megabytes(bytes)
    ));
}

fn connect(
    app: &Rc<App>,
    d: &Rc<Dialog>,
    timeline: cutroom_timeline::Timeline,
    sources: HashMap<MediaId, Source>,
) {
    for dropdown in [&d.size, &d.rate, &d.quality, &d.encoder] {
        let d = d.clone();
        dropdown.connect_selected_notify(move |_| update_summary(&d));
    }
    {
        let d2 = d.clone();
        d.cancel.connect_clicked(move |_| close(&d2));
    }
    {
        let d2 = d.clone();
        d.window.connect_close_request(move |_| {
            if d2.running.borrow().is_some() {
                // Closing the window mid-export is the same as Cancel.
                stop_export(&d2);
            } else {
                close(&d2);
            }
            glib::Propagation::Stop
        });
    }
    {
        let d2 = d.clone();
        d.stop.connect_clicked(move |_| stop_export(&d2));
    }
    {
        let (app, d2) = (app.clone(), d.clone());
        let change = d
            .folder
            .parent()
            .and_then(|p| p.last_child())
            .and_downcast::<gtk::Button>();
        if let Some(change) = change {
            change.connect_clicked(move |_| {
                let dialog = gtk::FileDialog::builder()
                    .title("Save the video in")
                    .build();
                dialog.set_initial_folder(Some(&gio::File::for_path(&*d2.folder_path.borrow())));
                let d3 = d2.clone();
                let _ = &app;
                dialog.select_folder(Some(&d2.window), gio::Cancellable::NONE, move |result| {
                    if let Some(path) = result.ok().and_then(|f| f.path()) {
                        d3.folder.set_text(&path.display().to_string());
                        d3.folder
                            .set_tooltip_text(Some(&path.display().to_string()));
                        *d3.folder_path.borrow_mut() = path;
                    }
                });
            });
        }
    }
    {
        let (app, d2) = (app.clone(), d.clone());
        d.export.connect_clicked(move |_| {
            begin(&app, &d2, timeline.clone(), sources.clone());
        });
    }
    {
        let d2 = d.clone();
        d.name.connect_activate(move |_| {
            if d2.export.is_sensitive() {
                d2.export.emit_clicked();
            }
        });
    }
}

fn close(d: &Rc<Dialog>) {
    forget(d);
    if let Some(w) = d.dimmed.borrow_mut().take() {
        w.set_opacity(1.0);
    }
    d.window.destroy();
}

fn stop_export(d: &Rc<Dialog>) {
    if let Some(run) = d.running.borrow().as_ref() {
        run.progress.cancel.store(true, Ordering::Relaxed);
    }
    d.stop.set_sensitive(false);
    d.note.set_text("Cancelling...");
}

/// The file the export will write, from the name and folder in the window.
pub fn output_path(d: &Dialog) -> PathBuf {
    let stem = clean_name(d.name.text().as_str());
    d.folder_path.borrow().join(format!("{stem}.mp4"))
}

fn begin(
    app: &Rc<App>,
    d: &Rc<Dialog>,
    timeline: cutroom_timeline::Timeline,
    sources: HashMap<MediaId, Source>,
) {
    let output = output_path(d);
    if output.exists() {
        let alert = gtk::AlertDialog::builder()
            .message(format!(
                "{} already exists.",
                output
                    .file_name()
                    .map_or(String::new(), |n| n.to_string_lossy().to_string())
            ))
            .detail("Replace it?")
            .buttons(["Cancel", "Replace"])
            .cancel_button(0)
            .default_button(0)
            .build();
        let (app, d2) = (app.clone(), d.clone());
        alert.choose(Some(&d.window), gio::Cancellable::NONE, move |answer| {
            if answer == Ok(1) {
                start(&app, &d2, timeline, sources, output);
            }
        });
    } else {
        start(app, d, timeline, sources, output);
    }
}

fn start(
    app: &Rc<App>,
    d: &Rc<Dialog>,
    timeline: cutroom_timeline::Timeline,
    sources: HashMap<MediaId, Source>,
    output: PathBuf,
) {
    let (width, height) = chosen_size(d);
    let fps = chosen_rate(d);
    let quality = chosen_quality(d);
    let encoders = chosen_encoders(d);
    // Remember the choices.
    {
        let mut st = app.state.borrow_mut();
        st.settings.export_quality = quality.label().to_string();
        st.settings.export_encoder = match d.encoder.selected() {
            0 => "auto".into(),
            n => d
                .encoders
                .borrow()
                .get(n as usize - 1)
                .map_or("auto".into(), |e| e.id.to_string()),
        };
        st.settings.export_folder = Some(d.folder_path.borrow().clone());
        st.save_settings();
    }
    let job = Job {
        timeline,
        sources,
        width,
        height,
        fps,
        sample_rate: d.sample_rate,
        video_kbps: export::bitrate_kbps(width, height, fps.as_f64(), quality),
        audio_kbps: 192,
        encoders,
        output: output.clone(),
        limit_seconds: test_limit(),
    };
    let progress = Arc::new(Progress::default());
    let outcome: Outcome = Arc::new(Mutex::new(None));
    {
        let (progress, outcome) = (progress.clone(), outcome.clone());
        std::thread::spawn(move || {
            let result = export::export(&job, &progress);
            *outcome.lock().unwrap() = Some(result);
        });
    }
    *d.running.borrow_mut() = Some(Running {
        progress,
        outcome,
        started: Instant::now(),
    });
    d.form.set_visible(false);
    d.run.set_visible(true);
    d.stop.set_sensitive(true);
    d.window.set_title(Some("Exporting..."));
    d.bar.set_fraction(0.0);
    d.bar.set_text(Some("Starting..."));
    d.note.set_text(&format!("{}", output.display()));

    let (app, d) = (app.clone(), d.clone());
    glib::timeout_add_local(Duration::from_millis(150), move || {
        let (fraction, done, total, elapsed, finished) = {
            let running = d.running.borrow();
            let Some(r) = running.as_ref() else {
                return glib::ControlFlow::Break;
            };
            (
                r.progress.fraction(),
                r.progress.frames_done.load(Ordering::Relaxed),
                r.progress.frames_total.load(Ordering::Relaxed),
                r.started.elapsed().as_secs_f64(),
                {
                    let result = r.outcome.lock().unwrap().take();
                    result
                },
            )
        };
        match finished {
            None => {
                d.bar.set_fraction(fraction);
                let left = if fraction > 0.02 {
                    format!(
                        " - about {} left",
                        clock(elapsed * (1.0 - fraction) / fraction)
                    )
                } else {
                    String::new()
                };
                d.bar.set_text(Some(&format!(
                    "{:.0}% - {} elapsed{left}",
                    fraction * 100.0,
                    clock(elapsed)
                )));
                if total > 0 {
                    d.note
                        .set_text(&format!("Frame {done} of {total}. {}", output.display()));
                }
                glib::ControlFlow::Continue
            }
            Some(result) => {
                d.running.borrow_mut().take();
                finish(&app, &d, result, &output);
                glib::ControlFlow::Break
            }
        }
    });
}

fn finish(app: &Rc<App>, d: &Rc<Dialog>, result: Result<Report, ExportError>, output: &Path) {
    match result {
        Err(ExportError::Cancelled) => {
            app.set_status("Export cancelled.");
            app.update_status();
            close(d);
        }
        Err(ExportError::Failed(message)) => {
            d.bar.set_fraction(0.0);
            d.bar.set_text(Some("The export failed"));
            d.note.set_text(&message);
            d.window.set_title(Some("Export failed"));
            d.stop.set_label("Close");
            d.stop.set_sensitive(true);
            let d2 = d.clone();
            d.stop.connect_clicked(move |_| close(&d2));
            app.set_status(format!("Export failed: {message}"));
            app.update_status();
        }
        Ok(report) => {
            let size = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
            d.bar.set_fraction(1.0);
            d.bar.set_text(Some("Done"));
            d.window.set_title(Some("Export finished"));
            let mut text = format!(
                "Saved {} ({}, {} frames, {}).",
                output.display(),
                megabytes(size as f64),
                report.frames,
                report.encoder
            );
            text.push_str(&format!(
                "\nTook {} (time elapsed).",
                elapsed_words(report.elapsed.as_secs_f64())
            ));
            for warning in &report.warnings {
                text.push_str(&format!("\nNote: {warning}"));
            }
            d.note.set_text(&text);
            *d.saved.borrow_mut() = Some(output.to_path_buf());
            app.set_status(format!("Exported {}", output.display()));
            app.update_status();
            // Replace the Cancel button by "Show in folder" and "Close".
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.set_halign(gtk::Align::End);
            let show = gtk::Button::with_label("Show in folder");
            let done = gtk::Button::with_label("Close");
            done.add_css_class("suggested-action");
            row.append(&show);
            row.append(&done);
            d.run.remove(&d.stop);
            d.run.append(&row);
            {
                let output = output.to_path_buf();
                show.connect_clicked(move |_| {
                    if let Some(parent) = output.parent() {
                        let uri = gio::File::for_path(parent).uri();
                        let _ =
                            gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
                    }
                });
            }
            let d2 = d.clone();
            done.connect_clicked(move |_| close(&d2));
        }
    }
}

thread_local! {
    static TEST_LIMIT: Cell<Option<f64>> = const { Cell::new(None) };
}

/// Tests export only the first few seconds.
#[cfg(test)]
pub fn limit_for_tests(seconds: f64) {
    TEST_LIMIT.with(|l| l.set(Some(seconds)));
}

fn test_limit() -> Option<f64> {
    TEST_LIMIT.with(|l| l.get())
}

/// Point the window at another folder (tests do not use the Videos folder).
#[cfg(test)]
pub fn set_folder_for_tests(d: &Dialog, folder: &Path) {
    d.folder.set_text(&folder.display().to_string());
    *d.folder_path.borrow_mut() = folder.to_path_buf();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_time_is_written_in_words() {
        assert_eq!(elapsed_words(42.4), "42 s");
        assert_eq!(elapsed_words(363.0), "6 min 03 s");
        assert_eq!(elapsed_words(3730.0), "1 h 02 min 10 s");
    }

    #[test]
    fn file_names_are_made_safe() {
        assert_eq!(clean_name("My: video/final?"), "My_ video_final_");
        assert_eq!(clean_name("   "), "Kani video");
        assert_eq!(clean_name("..."), "Kani video");
        assert_eq!(clean_name("Tako's reel"), "Tako's reel");
    }

    #[test]
    fn times_and_sizes_read_naturally() {
        assert_eq!(clock(5.0), "0:05");
        assert_eq!(clock(125.0), "2:05");
        assert_eq!(clock(3725.0), "1:02:05");
        assert_eq!(megabytes(2_500_000.0), "2.5 MB");
        assert_eq!(megabytes(48_000_000.0), "48 MB");
        assert_eq!(megabytes(2_400_000_000.0), "2.4 GB");
    }

    #[test]
    fn frame_rates_have_plain_names() {
        assert_eq!(rate_label(FrameRate::FPS_30), "30 fps");
        assert_eq!(rate_label(FrameRate::NTSC_29_97), "29.97 fps");
    }
}
