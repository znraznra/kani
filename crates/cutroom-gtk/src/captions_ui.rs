//! The Captions panel: pick a language, a speech model and a look, press
//! Generate, and the words are heard, timed and put on the timeline, lit up
//! as they are said.

use crate::app::App;
use cutroom_captions::{lines, models, srt};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Spoken languages offered (Whisper code, name). The first means "detect".
pub const LANGUAGES: &[(&str, &str)] = &[
    ("", "Detect automatically"),
    ("en", "English"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("tr", "Turkish"),
    ("ru", "Russian"),
    ("uk", "Ukrainian"),
    ("ar", "Arabic"),
    ("hi", "Hindi"),
    ("id", "Indonesian"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh", "Chinese"),
    ("tl", "Tagalog"),
];

const CUSTOM: &str = "custom";

/// What a worker thread reports back.
#[derive(Default)]
pub struct Shared {
    /// 0.0 to 1.0
    fraction: f64,
    message: String,
    finished: Option<Result<Finished, String>>,
}

enum Finished {
    Downloaded,
    Lines(Vec<lines::Line>),
}

/// A running download or transcription: its shared progress and its cancel flag.
pub type Job = (Arc<Mutex<Shared>>, Arc<AtomicBool>);

pub struct Panel {
    pub root: gtk::Box,
    pub language: gtk::DropDown,
    pub model: gtk::DropDown,
    pub model_status: gtk::Label,
    pub download: gtk::Button,
    pub choose_file: gtk::Button,
    pub style: gtk::DropDown,
    pub style_blurb: gtk::Label,
    pub generate: gtk::Button,
    pub cancel: gtk::Button,
    pub progress: gtk::ProgressBar,
    pub message: gtk::Label,
    pub export_srt: gtk::Button,
    pub import: gtk::Button,
    pub export_vtt: gtk::Button,
    pub source_note: gtk::Label,
    pub job: RefCell<Option<Job>>,
}

fn row(title: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let l = gtk::Label::new(Some(title));
    l.set_xalign(0.0);
    l.add_css_class("dim-label");
    l.add_css_class("caption");
    b.append(&l);
    b.append(widget);
    b
}

pub fn build() -> Rc<Panel> {
    let root = gtk::Box::new(gtk::Orientation::Vertical, crate::ui::GAP + 2);
    crate::ui::pad(&root);

    let title = gtk::Label::new(Some("Auto captions"));
    title.add_css_class("panel-title");
    title.set_xalign(0.0);
    root.append(&title);
    let intro = gtk::Label::new(Some(
        "Listens to your clips on this computer (nothing is uploaded), then adds one caption per line on a Captions track.",
    ));
    intro.set_wrap(true);
    intro.set_xalign(0.0);
    intro.add_css_class("dim-label");
    root.append(&intro);
    if !cutroom_captions::TRANSCRIPTION_BUILT_IN {
        let w = gtk::Label::new(Some("This build was made without speech recognition. Rebuild with the \"whisper\" feature to generate captions."));
        w.set_wrap(true);
        w.set_xalign(0.0);
        root.append(&w);
    }

    let language = gtk::DropDown::from_strings(&LANGUAGES.iter().map(|l| l.1).collect::<Vec<_>>());
    root.append(&row("Spoken language", &language));

    let mut names: Vec<&str> = models::MODELS.iter().map(|m| m.label).collect();
    names.push("Choose a model file…");
    let model = gtk::DropDown::from_strings(&names);
    root.append(&row("Speech model", &model));
    let model_status = gtk::Label::new(None);
    model_status.set_xalign(0.0);
    model_status.set_wrap(true);
    model_status.add_css_class("caption");
    root.append(&model_status);
    let model_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let download = gtk::Button::with_label("Download");
    let choose_file = gtk::Button::with_label("Choose file…");
    model_buttons.append(&download);
    model_buttons.append(&choose_file);
    root.append(&model_buttons);
    let model_hint = gtk::Label::new(Some(
        "Tiny and Base often misspell or miss words, especially in noisy audio or languages other than English. Medium and Turbo are much more accurate but slower.",
    ));
    model_hint.set_wrap(true);
    model_hint.set_xalign(0.0);
    model_hint.add_css_class("dim-label");
    model_hint.add_css_class("caption");
    root.append(&model_hint);

    let styles = cutroom_timeline::caption_styles();
    let style = gtk::DropDown::from_strings(&styles.iter().map(|s| s.name).collect::<Vec<_>>());
    root.append(&row("Look", &style));
    let style_blurb = gtk::Label::new(None);
    style_blurb.set_xalign(0.0);
    style_blurb.add_css_class("dim-label");
    style_blurb.add_css_class("caption");
    root.append(&style_blurb);

    let source_note = gtk::Label::new(None);
    source_note.set_wrap(true);
    source_note.set_xalign(0.0);
    source_note.add_css_class("caption");
    root.append(&source_note);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let generate = gtk::Button::with_label("Generate captions");
    generate.add_css_class("suggested-action");
    let cancel = gtk::Button::with_label("Cancel");
    cancel.set_visible(false);
    buttons.append(&generate);
    buttons.append(&cancel);
    root.append(&buttons);
    let progress = gtk::ProgressBar::new();
    progress.set_visible(false);
    root.append(&progress);
    let message = gtk::Label::new(None);
    message.set_wrap(true);
    message.set_xalign(0.0);
    root.append(&message);

    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let import_label = gtk::Label::new(Some(
        "Bring in a subtitle file (.srt, .vtt, .ass) as captions on the Captions track, in the look chosen above",
    ));
    import_label.set_xalign(0.0);
    import_label.set_wrap(true);
    import_label.add_css_class("dim-label");
    import_label.add_css_class("caption");
    root.append(&import_label);
    let import = gtk::Button::with_label("Import caption file…");
    import.set_halign(gtk::Align::Start);
    root.append(&import);
    let export_label =
        gtk::Label::new(Some("Save the captions on the timeline as a subtitle file"));
    export_label.set_xalign(0.0);
    export_label.set_wrap(true);
    export_label.add_css_class("dim-label");
    export_label.add_css_class("caption");
    root.append(&export_label);
    let exports = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let export_srt = gtk::Button::with_label("Export .srt");
    let export_vtt = gtk::Button::with_label("Export .vtt");
    exports.append(&export_srt);
    exports.append(&export_vtt);
    root.append(&exports);

    Rc::new(Panel {
        root,
        language,
        model,
        model_status,
        download,
        choose_file,
        style,
        style_blurb,
        generate,
        cancel,
        progress,
        message,
        export_srt,
        import,
        export_vtt,
        source_note,
        job: RefCell::new(None),
    })
}

fn models_dir(app: &App) -> Option<PathBuf> {
    app.state
        .borrow()
        .config_dir
        .as_ref()
        .map(|d| d.join("models"))
}

/// The model file chosen right now, if there is one on disk.
fn chosen_model_path(app: &App, panel: &Panel) -> Option<PathBuf> {
    let st = app.state.borrow();
    if st.settings.caption_model == CUSTOM {
        return st
            .settings
            .caption_model_path
            .clone()
            .filter(|p| p.is_file());
    }
    let m = models::by_id(&st.settings.caption_model);
    let dir = st.config_dir.as_ref()?.join("models");
    let _ = panel;
    m.installed_in(&dir).then(|| m.path_in(&dir))
}

/// Bring every control in line with the settings and the project.
pub fn refresh(app: &Rc<App>, panel: &Rc<Panel>) {
    let (lang, model_id, style_name) = {
        let st = app.state.borrow();
        (
            st.settings.caption_language.clone(),
            st.settings.caption_model.clone(),
            st.settings.caption_style.clone(),
        )
    };
    panel
        .language
        .set_selected(LANGUAGES.iter().position(|l| l.0 == lang).unwrap_or(0) as u32);
    panel.model.set_selected(if model_id == CUSTOM {
        models::MODELS.len() as u32
    } else {
        models::MODELS
            .iter()
            .position(|m| m.id == model_id)
            .unwrap_or(1) as u32
    });
    let styles = cutroom_timeline::caption_styles();
    let si = styles
        .iter()
        .position(|s| s.name == style_name)
        .unwrap_or(0);
    panel.style.set_selected(si as u32);
    panel.style_blurb.set_text(styles[si].blurb);

    let busy = panel.job.borrow().is_some();
    let have = chosen_model_path(app, panel);
    let custom = model_id == CUSTOM;
    panel.download.set_visible(!custom);
    panel.choose_file.set_visible(custom);
    panel.download.set_sensitive(!busy && have.is_none());
    panel.model_status.set_text(&if custom {
        match &have {
            Some(p) => format!("Using {}", p.display()),
            None => "No model file chosen yet.".to_string(),
        }
    } else {
        let m = models::by_id(&model_id);
        if have.is_some() {
            "Installed on this computer.".to_string()
        } else {
            format!(
                "Not downloaded yet (about {} MB, fetched once).",
                m.approx_mb
            )
        }
    });
    let sources = app
        .state
        .borrow()
        .session
        .as_ref()
        .map(|s| (s.caption_sources().len(), s.selection().len()))
        .unwrap_or((0, 0));
    panel.source_note.set_text(&match sources {
        (0, _) => "Nothing with sound to caption yet.".to_string(),
        (n, 0) => format!("Will listen to everything with sound ({n} clip{}). Select clips to caption only those.", if n == 1 { "" } else { "s" }),
        (n, _) => format!("Will listen to the {n} selected clip{} with sound.", if n == 1 { "" } else { "s" }),
    });
    let has_captions = app
        .state
        .borrow()
        .session
        .as_ref()
        .is_some_and(|s| !s.caption_cues().is_empty());
    panel.export_srt.set_sensitive(has_captions);
    panel.export_vtt.set_sensitive(has_captions);
    panel.generate.set_sensitive(
        !busy && cutroom_captions::TRANSCRIPTION_BUILT_IN && have.is_some() && sources.0 > 0,
    );
}

fn show_progress(panel: &Panel, fraction: f64, message: &str) {
    panel.progress.set_visible(true);
    panel.progress.set_fraction(fraction.clamp(0.0, 1.0));
    panel.message.set_text(message);
}

fn end_job(app: &Rc<App>, panel: &Rc<Panel>) {
    panel.job.borrow_mut().take();
    panel.progress.set_visible(false);
    panel.cancel.set_visible(false);
    refresh(app, panel);
}

/// The "Generating captions" window: the workspace behind it is dimmed and
/// cannot be used until it closes.
struct Busy {
    window: gtk::Window,
    bar: gtk::ProgressBar,
    note: gtk::Label,
    dimmed: Option<gtk::Widget>,
}

impl Busy {
    fn open(app: &Rc<App>, title: &str, cancel: Arc<AtomicBool>) -> Busy {
        let window = gtk::Window::builder()
            .title(title)
            .transient_for(&app.window)
            .modal(true)
            .resizable(false)
            .destroy_with_parent(true)
            .default_width(420)
            .build();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
        body.set_margin_top(18);
        body.set_margin_bottom(16);
        body.set_margin_start(20);
        body.set_margin_end(20);
        let bar = gtk::ProgressBar::new();
        bar.set_show_text(true);
        let note = gtk::Label::new(None);
        note.set_xalign(0.0);
        note.set_wrap(true);
        note.add_css_class("dim-label");
        note.add_css_class("caption");
        let stop = gtk::Button::with_label("Cancel");
        stop.set_halign(gtk::Align::End);
        {
            let cancel = cancel.clone();
            stop.connect_clicked(move |b| {
                cancel.store(true, Ordering::Relaxed);
                b.set_sensitive(false);
            });
        }
        // Closing the window is the same as Cancel.
        window.connect_close_request(move |_| {
            cancel.store(true, Ordering::Relaxed);
            glib::Propagation::Stop
        });
        body.append(&bar);
        body.append(&note);
        body.append(&stop);
        window.set_child(Some(&body));
        let dimmed = app.window.child();
        if let Some(w) = &dimmed {
            w.set_opacity(0.4);
        }
        window.present();
        Busy {
            window,
            bar,
            note,
            dimmed,
        }
    }

    fn show(&self, fraction: f64, note: &str) {
        let f = fraction.clamp(0.0, 1.0);
        self.bar.set_fraction(f);
        self.bar.set_text(Some(&format!("{:.0}%", f * 100.0)));
        self.note.set_text(note);
    }

    fn close(self) {
        if let Some(w) = &self.dimmed {
            w.set_opacity(1.0);
        }
        self.window.destroy();
    }
}

/// Watch a worker until it finishes, then hand the result to `done`. With a
/// `window_title`, a progress window covers the app meanwhile.
fn watch(
    app: &Rc<App>,
    panel: &Rc<Panel>,
    shared: Arc<Mutex<Shared>>,
    cancel: Arc<AtomicBool>,
    window_title: Option<&str>,
    done: impl Fn(&Rc<App>, &Rc<Panel>, Result<Finished, String>) + 'static,
) {
    *panel.job.borrow_mut() = Some((shared.clone(), cancel.clone()));
    panel.cancel.set_visible(true);
    panel.cancel.set_sensitive(true);
    refresh(app, panel);
    let busy = window_title.map(|t| Busy::open(app, t, cancel));
    let busy = RefCell::new(busy);
    let (app, panel) = (app.clone(), panel.clone());
    glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
        let (fraction, message, finished) = {
            let mut s = shared.lock().unwrap();
            (s.fraction, s.message.clone(), s.finished.take())
        };
        match finished {
            None => {
                show_progress(&panel, fraction, &message);
                if let Some(b) = busy.borrow().as_ref() {
                    b.show(fraction, &message);
                }
                glib::ControlFlow::Continue
            }
            Some(result) => {
                if let Some(b) = busy.borrow_mut().take() {
                    b.close();
                }
                end_job(&app, &panel);
                done(&app, &panel, result);
                glib::ControlFlow::Break
            }
        }
    });
}

fn start_download(app: &Rc<App>, panel: &Rc<Panel>) {
    let Some(dir) = models_dir(app) else {
        panel
            .message
            .set_text("There is no place to keep models on this system.");
        return;
    };
    let model = models::by_id(&app.state.borrow().settings.caption_model);
    let shared = Arc::new(Mutex::new(Shared {
        message: format!("Downloading the {} model…", model.id),
        ..Shared::default()
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let (shared, cancel) = (shared.clone(), cancel.clone());
        std::thread::spawn(move || {
            let result = models::download(
                model,
                &dir,
                |done, total| {
                    let mut s = shared.lock().unwrap();
                    let mb = |b: u64| b as f64 / 1_048_576.0;
                    s.fraction = total.map_or(0.0, |t| done as f64 / t as f64);
                    s.message = match total {
                        Some(t) => format!(
                            "Downloading the {} model… {:.0} of {:.0} MB",
                            model.id,
                            mb(done),
                            mb(t)
                        ),
                        None => format!("Downloading the {} model… {:.0} MB", model.id, mb(done)),
                    };
                },
                &cancel,
            );
            let mut s = shared.lock().unwrap();
            s.finished = Some(match result {
                Ok(_) => Ok(Finished::Downloaded),
                Err(cutroom_captions::CaptionError::Cancelled) => Err("Download cancelled.".into()),
                Err(e) => Err(format!("The download did not work: {e}. Check your connection, or use \"Choose a model file…\" with a ggml model you already have.")),
            });
        });
    }
    watch(app, panel, shared, cancel, None, |_, panel, result| {
        panel.message.set_text(&match result {
            Ok(_) => "Model ready.".to_string(),
            Err(e) => e,
        });
    });
}

#[cfg(feature = "whisper")]
fn start_generate(app: &Rc<App>, panel: &Rc<Panel>) {
    let Some(model_path) = chosen_model_path(app, panel) else {
        panel.message.set_text("Get a speech model first.");
        return;
    };
    let sources = match app.state.borrow().session.as_ref() {
        Some(s) => s.caption_sources(),
        None => return,
    };
    if sources.is_empty() {
        panel.message.set_text("Nothing with sound to caption.");
        return;
    }
    let language = app.state.borrow().settings.caption_language.clone();
    let models = models_dir(app);
    let notes_path = debug_path(app);
    let shared = Arc::new(Mutex::new(Shared {
        message: "Reading the sound…".into(),
        ..Shared::default()
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let (shared, cancel) = (shared.clone(), cancel.clone());
        std::thread::spawn(move || {
            let total = sources.len() as f64;
            let result = (|| -> Result<Vec<lines::Line>, String> {
                // A small helper model that tells speech from music and silence.
                shared.lock().unwrap().message = "Getting the voice detector ready…".into();
                let detector = models
                    .as_deref()
                    .and_then(|dir| cutroom_captions::models::ensure_voice_detector(dir, &cancel));
                let mut words = Vec::new();
                for (i, src) in sources.iter().enumerate() {
                    {
                        let mut s = shared.lock().unwrap();
                        s.message = format!("Reading the sound… ({} of {})", i + 1, sources.len());
                        s.fraction = i as f64 / total;
                    }
                    let mut samples = crate::captions_audio::decode_range(
                        &src.path,
                        src.source_in,
                        src.duration,
                        &cancel,
                    )?;
                    if samples.is_empty() {
                        continue;
                    }
                    // Whisper wants at least a second to chew on.
                    if samples.len() < crate::captions_audio::RATE + 1600 {
                        samples.resize(crate::captions_audio::RATE + 1600, 0.0);
                    }
                    shared.lock().unwrap().message = format!(
                        "Loading the speech model… (clip {} of {})",
                        i + 1,
                        sources.len()
                    );
                    let lang = (!language.is_empty()).then_some(language.as_str());
                    let sh = shared.clone();
                    let heard = cutroom_captions::whisper::transcribe(
                        &model_path,
                        detector.as_deref(),
                        &samples,
                        lang,
                        move |p| {
                            let mut s = sh.lock().unwrap();
                            s.message = format!(
                                "Listening and writing it down… clip {} of {}",
                                i + 1,
                                total as usize
                            );
                            s.fraction = (i as f64 + f64::from(p) / 100.0) / total;
                        },
                        cancel.clone(),
                    );
                    if let Some(path) = &notes_path {
                        let mut text = format!(
                            "Kani caption notes, clip {} of {}\n{}\n",
                            i + 1,
                            sources.len(),
                            cutroom_captions::whisper::debug_report()
                        );
                        if let Err(e) = &heard {
                            text.push_str(&format!("\nERROR: {e}\n"));
                        }
                        let _ = std::fs::write(path, text);
                    }
                    let heard = heard.map_err(|e| match e {
                        cutroom_captions::CaptionError::Cancelled => "Cancelled.".to_string(),
                        e => e.to_string(),
                    })?;
                    for w in heard {
                        if w.start >= src.duration {
                            continue;
                        }
                        words.push(cutroom_captions::TimedWord {
                            text: w.text,
                            start: src.start + w.start,
                            end: src.start + w.end.min(src.duration),
                        });
                    }
                }
                {
                    let mut s = shared.lock().unwrap();
                    s.fraction = 1.0;
                    s.message = "Placing the words and breaking them into lines…".into();
                }
                Ok(lines::group(&words, lines::LineRules::default()))
            })();
            shared.lock().unwrap().finished = Some(result.map(Finished::Lines));
        });
    }
    watch(
        app,
        panel,
        shared,
        cancel,
        Some("Generating captions"),
        |app, panel, result| match result {
            Err(e) => panel.message.set_text(&e),
            Ok(Finished::Downloaded) => {}
            Ok(Finished::Lines(found)) => {
                if found.is_empty() {
                    panel.message.set_text("No speech was heard. Try a bigger model, or pick the spoken language instead of detecting it.");
                    return;
                }
                let notes = debug_path(app).map_or(String::new(), |p| {
                    format!(" Notes on what was heard: {}", p.display())
                });
                let style = style_spec(app);
                let style_name = app.state.borrow().settings.caption_style.clone();
                let added = app
                    .state
                    .borrow_mut()
                    .session
                    .as_mut()
                    .map_or(0, |s| s.add_captions(&found, &style, &style_name));
                panel.message.set_text(&format!(
                "Added {added} captions. Click one on the timeline to fix a word or change its look; undo (Ctrl+Z) removes them all.{notes}"
            ));
                app.sync_preview(false);
                app.refresh();
            }
        },
    );
}

#[cfg(not(feature = "whisper"))]
fn start_generate(_app: &Rc<App>, panel: &Rc<Panel>) {
    panel
        .message
        .set_text("This build has no speech recognition.");
}

pub fn style_spec(app: &App) -> cutroom_timeline::TextSpec {
    let name = app.state.borrow().settings.caption_style.clone();
    let styles = cutroom_timeline::caption_styles();
    styles
        .into_iter()
        .find(|s| s.name == name)
        .map(|s| s.spec)
        .unwrap_or_default()
}

/// Where the notes on the latest caption run are kept (next to the models).
fn debug_path(app: &App) -> Option<PathBuf> {
    models_dir(app)?
        .parent()
        .map(|p| p.join("caption-debug.txt"))
}

/// Put the cues of a subtitle file on the Captions track in the chosen look.
pub fn import_file(app: &Rc<App>, path: &std::path::Path) -> Result<usize, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read the file: {e}"))?;
    let text = String::from_utf8_lossy(&bytes);
    let cues = srt::parse_subtitles(&text)?;
    let style = style_spec(app);
    let style_name = app.state.borrow().settings.caption_style.clone();
    let added = app
        .state
        .borrow_mut()
        .session
        .as_mut()
        .map_or(0, |s| s.import_captions(&cues, &style, &style_name));
    app.sync_preview(false);
    app.refresh();
    Ok(added)
}

fn choose_and_import(app: &Rc<App>, panel: &Rc<Panel>) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Subtitle files (.srt, .vtt, .ass, .ssa)"));
    for ext in ["srt", "vtt", "ass", "ssa"] {
        filter.add_suffix(ext);
    }
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder()
        .title("Import a caption file")
        .filters(&filters)
        .build();
    let (app2, panel) = (app.clone(), panel.clone());
    dialog.open(Some(&app.window), gio::Cancellable::NONE, move |result| {
        let Some(path) = result.ok().and_then(|f| f.path()) else {
            return;
        };
        match import_file(&app2, &path) {
            Ok(n) => panel.message.set_text(&format!(
                "Imported {n} captions. Click one on the timeline to change its words or look; undo (Ctrl+Z) removes them all."
            )),
            Err(e) => panel.message.set_text(&e),
        }
    });
}

fn export(app: &Rc<App>, vtt: bool) {
    let cues = match app.state.borrow().session.as_ref() {
        Some(s) => s.caption_cues(),
        None => return,
    };
    if cues.is_empty() {
        return;
    }
    let dialog = gtk::FileDialog::builder()
        .title(if vtt {
            "Save captions (.vtt)"
        } else {
            "Save captions (.srt)"
        })
        .initial_name(if vtt { "captions.vtt" } else { "captions.srt" })
        .build();
    let app2 = app.clone();
    dialog.save(Some(&app.window), gio::Cancellable::NONE, move |result| {
        let Some(path) = result.ok().and_then(|f| f.path()) else {
            return;
        };
        let text = if vtt {
            srt::to_vtt(&cues)
        } else {
            srt::to_srt(&cues)
        };
        match std::fs::write(&path, text) {
            Ok(()) => app2.set_status(format!(
                "Saved {} captions to {}",
                cues.len(),
                path.display()
            )),
            Err(e) => app2.set_status(format!("Could not save the captions: {e}")),
        }
        app2.update_status();
    });
}

/// Wire the controls. Called once, after the window exists.
pub fn connect(app: &Rc<App>, panel: &Rc<Panel>) {
    {
        let (app, panel) = (app.clone(), panel.clone());
        panel.clone().language.connect_selected_notify(move |dd| {
            let code = LANGUAGES.get(dd.selected() as usize).map_or("", |l| l.0);
            let mut st = app.state.borrow_mut();
            if st.settings.caption_language != code {
                st.settings.caption_language = code.to_string();
                st.save_settings();
            }
            let _ = &panel;
        });
    }
    {
        let (app, panel) = (app.clone(), panel.clone());
        panel.clone().model.connect_selected_notify(move |dd| {
            let i = dd.selected() as usize;
            let id = models::MODELS.get(i).map_or(CUSTOM, |m| m.id);
            let changed = {
                let mut st = app.state.borrow_mut();
                let changed = st.settings.caption_model != id;
                if changed {
                    st.settings.caption_model = id.to_string();
                    st.save_settings();
                }
                changed
            };
            if changed {
                refresh(&app, &panel);
            }
        });
    }
    {
        let (app, panel) = (app.clone(), panel.clone());
        panel.clone().style.connect_selected_notify(move |dd| {
            let styles = cutroom_timeline::caption_styles();
            let Some(s) = styles.get(dd.selected() as usize) else {
                return;
            };
            panel.style_blurb.set_text(s.blurb);
            let mut st = app.state.borrow_mut();
            if st.settings.caption_style != s.name {
                st.settings.caption_style = s.name.to_string();
                st.save_settings();
            }
        });
    }
    {
        let (app, panel) = (app.clone(), panel.clone());
        panel
            .clone()
            .download
            .connect_clicked(move |_| start_download(&app, &panel));
    }
    {
        let (app, panel) = (app.clone(), panel.clone());
        panel
            .clone()
            .import
            .connect_clicked(move |_| choose_and_import(&app, &panel));
    }
    {
        let (app, panel) = (app.clone(), panel.clone());
        panel.clone().choose_file.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder()
                .title("Choose a Whisper (ggml) model file")
                .build();
            let (app, panel) = (app.clone(), panel.clone());
            let window = app.window.clone();
            dialog.open(Some(&window), gio::Cancellable::NONE, move |result| {
                if let Some(path) = result.ok().and_then(|f| f.path()) {
                    {
                        let mut st = app.state.borrow_mut();
                        st.settings.caption_model = CUSTOM.to_string();
                        st.settings.caption_model_path = Some(path);
                        st.save_settings();
                    }
                    refresh(&app, &panel);
                }
            });
        });
    }
    {
        let (app, panel) = (app.clone(), panel.clone());
        panel
            .clone()
            .generate
            .connect_clicked(move |_| start_generate(&app, &panel));
    }
    {
        let panel = panel.clone();
        panel.clone().cancel.connect_clicked(move |b| {
            if let Some((_, cancel)) = panel.job.borrow().as_ref() {
                cancel.store(true, Ordering::Relaxed);
            }
            b.set_sensitive(false);
        });
    }
    {
        let app = app.clone();
        panel
            .export_srt
            .connect_clicked(move |_| export(&app, false));
    }
    {
        let app = app.clone();
        panel
            .export_vtt
            .connect_clicked(move |_| export(&app, true));
    }
}
