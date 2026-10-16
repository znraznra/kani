//! The Export window: a picture of the project on the left; on the right where
//! to save, and the Video, Audio and Captions to make, then the encoder and
//! hardware acceleration. While exporting, the same window stays (same size,
//! same picture, which follows the frames as they are made) and the right-hand
//! side shows progress and a log; Cancel stays where it was.

use crate::app::App;
use crate::export::{self, Encoder, ExportError, Job, Progress, Report, Source};
use crate::export_options::{
    self as opts, AudioFormat, BitratePreset, Channels, Container, Resolution, VideoCodec,
    AUDIO_BITRATES,
};
use cutroom_core::state::{MediaKind, Session};
use cutroom_timeline::{FrameRate, MediaId, Ticks};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Outcome = Arc<Mutex<Option<Result<Report, ExportError>>>>;

pub struct Dialog {
    pub window: gtk::Window,
    pub name: gtk::Entry,
    pub folder: gtk::Label,
    ext: gtk::Label,
    // What to make.
    pub video_on: gtk::CheckButton,
    pub audio_on: gtk::CheckButton,
    pub captions_on: gtk::CheckButton,
    // Video.
    pub aspect: gtk::DropDown,
    /// The size (resolution) list.
    pub size: gtk::DropDown,
    fit: gtk::DropDown,
    fit_row: Vec<gtk::Widget>,
    pub bitrate: gtk::DropDown,
    custom_kbps: gtk::SpinButton,
    pub codec: gtk::DropDown,
    pub format: gtk::DropDown,
    pub rate: gtk::DropDown,
    video_box: gtk::Box,
    // Audio.
    pub audio_format: gtk::DropDown,
    pub audio_bitrate: gtk::DropDown,
    pub channels: gtk::DropDown,
    audio_box: gtk::Box,
    // Captions.
    caption_srt: gtk::CheckButton,
    caption_vtt: gtk::CheckButton,
    caption_ass: gtk::CheckButton,
    #[allow(dead_code)]
    caption_box: gtk::Box,
    // Encoder and acceleration.
    pub encoder: gtk::DropDown,
    /// Automatic, always or never put pictures together on the graphics card.
    pub gpu_mode: gtk::DropDown,
    // Left side and bottom.
    preview: gtk::Picture,
    preview_note: gtk::Label,
    files: gtk::Label,
    pub summary: gtk::Label,
    pub export: gtk::Button,
    pub cancel: gtk::Button,
    /// The right-hand side: the choices, or the progress while exporting.
    stack: gtk::Stack,
    bar: gtk::ProgressBar,
    note: gtk::Label,
    log: gtk::TextBuffer,
    /// The export has ended: Export now reads "Show in folder" and Cancel reads "Close".
    finished: Cell<bool>,
    folder_path: RefCell<PathBuf>,
    // What the project is, and what is offered for it.
    project: (u32, u32),
    #[allow(dead_code)]
    project_rate: FrameRate,
    aspects: Vec<(String, (u32, u32))>,
    resolutions: RefCell<Vec<Resolution>>,
    rates: Vec<FrameRate>,
    containers: RefCell<Vec<Container>>,
    audio_formats: RefCell<Vec<AudioFormat>>,
    encoders: RefCell<Vec<Encoder>>,
    timeline: cutroom_timeline::Timeline,
    sources: HashMap<MediaId, Source>,
    cues: Vec<(f64, f64, String)>,
    caption_look: Option<(cutroom_timeline::TextSpec, cutroom_timeline::Transform)>,
    playhead: Ticks,
    seconds: f64,
    sample_rate: u32,
    /// What is running right now, if anything.
    running: RefCell<Option<Running>>,
    /// The finished file, once there is one (the video, else the first file).
    pub saved: RefCell<Option<PathBuf>>,
    dimmed: RefCell<Option<gtk::Widget>>,
    /// The encoders for the chosen video format have been found.
    ready: Cell<bool>,
    /// Which encoder search is the current one (older answers are dropped).
    search: Cell<u32>,
    /// Set while the window changes its own lists, so that does not count as a choice.
    busy: Cell<bool>,
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

/// Replace what a dropdown lists, keeping the window from treating that as a choice.
fn set_items(d: &Dialog, dropdown: &gtk::DropDown, items: &[String], selected: u32) {
    d.busy.set(true);
    dropdown.set_model(Some(&gtk::StringList::new(
        &items.iter().map(String::as_str).collect::<Vec<_>>(),
    )));
    dropdown.set_selected(selected.min(items.len().saturating_sub(1) as u32));
    d.busy.set(false);
}

fn select(d: &Dialog, dropdown: &gtk::DropDown, index: u32) {
    d.busy.set(true);
    dropdown.set_selected(index);
    d.busy.set(false);
}

/// A section heading with a tick box that switches the section on or off.
fn section_check(title: &str, on: bool) -> gtk::CheckButton {
    let check = gtk::CheckButton::with_label(title);
    check.set_active(on);
    check.add_css_class("heading");
    check
}

fn grid_label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_width_chars(10);
    l.add_css_class("dim-label");
    l
}

fn new_grid() -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(10);
    grid
}

thread_local! {
    static STYLED: Cell<bool> = const { Cell::new(false) };
}

/// The Export window is denser than the rest of the app (small controls, tight
/// rows, small type): a compact dialog with a picture beside it.
fn add_compact_style() {
    if STYLED.with(|s| s.replace(true)) {
        return;
    }
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        "window.kani-export { font-size: 10pt; }\
         window.kani-export button, window.kani-export entry, window.kani-export dropdown > button, window.kani-export spinbutton { min-height: 22px; padding-top: 2px; padding-bottom: 2px; }\
         window.kani-export .heading { font-size: 10.5pt; font-weight: 700; }\
         window.kani-export .caption { font-size: 9pt; }\
         window.kani-export checkbutton { min-height: 20px; }\
         window.kani-export .export-log { font-size: 9pt; }",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn dropdown(items: &[&str]) -> gtk::DropDown {
    let d = gtk::DropDown::from_strings(items);
    d.set_hexpand(true);
    d
}

/// Open the Export window.
pub fn open(app: &Rc<App>) {
    let clicked = Instant::now();
    if let Some(existing) = active() {
        existing.window.present();
        return;
    }
    let (project_name, canvas, project_rate, sample_rate, seconds, sources, timeline, cues, look, playhead) = {
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
            session.caption_cues(),
            session.caption_look(),
            session.playhead,
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
    let settings = app.state.borrow().settings.clone();

    let window = gtk::Window::builder()
        .title("Export")
        .transient_for(&app.window)
        .modal(true)
        .destroy_with_parent(true)
        .default_width(740)
        .default_height(560)
        .build();
    add_compact_style();
    window.add_css_class("kani-export");

    // ---- left: the picture and the list of files
    let preview = gtk::Picture::new();
    #[allow(deprecated)]
    preview.set_keep_aspect_ratio(true);
    preview.set_can_shrink(true);
    // A fixed-size box for the picture, whatever shape the video is.
    let preview_box = gtk::ScrolledWindow::new();
    preview_box.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
    preview_box.set_size_request(300, 300);
    preview_box.set_hexpand(false);
    preview_box.set_vexpand(false);
    preview_box.set_child(Some(&preview));
    let preview_frame = gtk::Frame::new(None);
    preview_frame.set_child(Some(&preview_box));
    let preview_note = gtk::Label::new(Some("Preparing the picture ..."));
    preview_note.add_css_class("dim-label");
    preview_note.add_css_class("caption");
    preview_note.set_xalign(0.0);
    let files = gtk::Label::new(None);
    files.set_xalign(0.0);
    files.set_wrap(true);
    files.set_max_width_chars(36);
    files.set_selectable(false);
    files.add_css_class("caption");
    let left = gtk::Box::new(gtk::Orientation::Vertical, 6);
    left.set_valign(gtk::Align::Start);
    left.set_width_request(300);
    left.append(&preview_frame);
    left.append(&preview_note);
    left.append(&files);

    // ---- right: where to save
    let right = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let top = new_grid();
    let name = gtk::Entry::new();
    name.set_hexpand(true);
    name.set_width_chars(8);
    name.set_text(&clean_name(&project_name));
    let ext = gtk::Label::new(Some(".mp4"));
    ext.add_css_class("dim-label");
    let name_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    name_row.append(&name);
    name_row.append(&ext);
    top.attach(&grid_label("Name"), 0, 0, 1, 1);
    top.attach(&name_row, 1, 0, 1, 1);
    let folder_path = default_folder(app);
    let folder = gtk::Label::new(Some(&folder_path.display().to_string()));
    folder.set_xalign(0.0);
    folder.set_hexpand(true);
    folder.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    folder.set_tooltip_text(Some(&folder_path.display().to_string()));
    let change = gtk::Button::from_icon_name("folder-open-symbolic");
    change.set_tooltip_text(Some("Choose the folder"));
    let folder_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    folder_row.append(&folder);
    folder_row.append(&change);
    top.attach(&grid_label("Export to"), 0, 1, 1, 1);
    top.attach(&folder_row, 1, 1, 1, 1);
    right.append(&top);
    right.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // ---- Video
    let video_on = section_check("Video", settings.export_video);
    video_on.set_tooltip_text(Some(
        "Make a video file. It carries the project's sound.",
    ));
    let video_grid = new_grid();
    let aspects = opts::aspect_options(canvas);
    let aspect = dropdown(&aspects.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>());
    let size = dropdown(&["..."]);
    let fit = dropdown(&["Black bars around the picture", "Crop the picture to fill"]);
    fit.set_selected(u32::from(settings.export_fill));
    let fit_label = grid_label("When the shape differs");
    let bitrate = dropdown(&["..."]);
    let custom_kbps = gtk::SpinButton::with_range(500.0, 100_000.0, 500.0);
    custom_kbps.set_value(f64::from(settings.export_custom_kbps.max(500)));
    custom_kbps.set_visible(false);
    custom_kbps.set_tooltip_text(Some("Video bit rate in kilobits per second"));
    let bitrate_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bitrate_row.append(&bitrate);
    bitrate_row.append(&custom_kbps);
    let codec = dropdown(&VideoCodec::ALL.iter().map(|c| c.label()).collect::<Vec<_>>());
    let format = dropdown(&["..."]);
    let rates = opts::frame_rates(project_rate);
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
    let rate = dropdown(&rate_labels.iter().map(String::as_str).collect::<Vec<_>>());
    let colour = gtk::Label::new(Some("Rec. 709 SDR"));
    colour.set_xalign(0.0);
    colour.set_tooltip_text(Some(
        "Kani works in Rec. 709 standard dynamic range, the usual choice for phones and the web.",
    ));
    let rows: [(&str, gtk::Widget); 7] = [
        ("Aspect ratio", aspect.clone().upcast()),
        ("Resolution", size.clone().upcast()),
        ("", gtk::Box::new(gtk::Orientation::Horizontal, 0).upcast()),
        ("Bit rate", bitrate_row.clone().upcast()),
        ("Codec", codec.clone().upcast()),
        ("Format", format.clone().upcast()),
        ("Frame rate", rate.clone().upcast()),
    ];
    let mut row = 0;
    let mut fit_row: Vec<gtk::Widget> = Vec::new();
    for (i, (label, widget)) in rows.iter().enumerate() {
        if i == 2 {
            // The "when the shape differs" row, shown only when it matters.
            video_grid.attach(&fit_label, 0, row, 1, 1);
            video_grid.attach(&fit, 1, row, 1, 1);
            fit_row = vec![fit_label.clone().upcast(), fit.clone().upcast()];
            row += 1;
            continue;
        }
        video_grid.attach(&grid_label(label), 0, row, 1, 1);
        video_grid.attach(widget, 1, row, 1, 1);
        row += 1;
    }
    video_grid.attach(&grid_label("Color space"), 0, row, 1, 1);
    video_grid.attach(&colour, 1, row, 1, 1);
    let video_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    video_box.append(&video_on);
    video_box.append(&video_grid);
    right.append(&video_box);
    right.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // ---- Audio
    let audio_on = section_check("Audio", settings.export_audio);
    audio_on.set_tooltip_text(Some(
        "Make a file of the project's sound alone. (With Video ticked too, the video file has its sound as well.)",
    ));
    let audio_grid = new_grid();
    let audio_format = dropdown(&["..."]);
    let audio_bitrate = dropdown(
        &AUDIO_BITRATES
            .iter()
            .map(|_| "")
            .collect::<Vec<_>>(),
    );
    let channels = dropdown(&Channels::ALL.iter().map(|c| c.label()).collect::<Vec<_>>());
    channels.set_tooltip_text(Some(
        "Kani mixes the project's sound to stereo, so mono and stereo are on offer; there is no surround source to keep.",
    ));
    audio_grid.attach(&grid_label("Format"), 0, 0, 1, 1);
    audio_grid.attach(&audio_format, 1, 0, 1, 1);
    audio_grid.attach(&grid_label("Bit rate"), 0, 1, 1, 1);
    audio_grid.attach(&audio_bitrate, 1, 1, 1, 1);
    audio_grid.attach(&grid_label("Channels"), 0, 2, 1, 1);
    audio_grid.attach(&channels, 1, 2, 1, 1);
    let audio_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    audio_box.append(&audio_on);
    audio_box.append(&audio_grid);
    right.append(&audio_box);
    right.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // ---- Captions
    let captions_on = section_check("Captions", !settings.export_caption_files.is_empty() && !cues.is_empty());
    captions_on.set_tooltip_text(Some("Save the caption text as subtitle files."));
    let caption_srt = gtk::CheckButton::with_label("SubRip (.srt)");
    let caption_vtt = gtk::CheckButton::with_label("WebVTT (.vtt)");
    let caption_ass = gtk::CheckButton::with_label("Advanced SubStation Alpha (.ass)");
    let wanted = |id: &str| settings.export_caption_files.iter().any(|f| f == id);
    caption_srt.set_active(wanted("srt") || settings.export_caption_files.is_empty());
    caption_vtt.set_active(wanted("vtt"));
    caption_ass.set_active(wanted("ass"));
    let caption_grid = gtk::Box::new(gtk::Orientation::Vertical, 4);
    caption_grid.set_margin_start(20);
    caption_grid.append(&caption_srt);
    caption_grid.append(&caption_vtt);
    caption_grid.append(&caption_ass);
    let caption_note = gtk::Label::new(Some("There are no captions in this project yet."));
    caption_note.set_xalign(0.0);
    caption_note.add_css_class("dim-label");
    caption_note.add_css_class("caption");
    caption_note.set_margin_start(20);
    caption_note.set_visible(cues.is_empty());
    let caption_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    caption_box.append(&captions_on);
    caption_box.append(&caption_grid);
    caption_box.append(&caption_note);
    right.append(&caption_box);
    right.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // ---- Encoder and rendering
    let heading = gtk::Label::new(Some("Encoder"));
    heading.set_xalign(0.0);
    heading.add_css_class("heading");
    let tech_grid = new_grid();
    let encoder = dropdown(&["Looking for encoders..."]);
    encoder.set_sensitive(false);
    encoder.set_tooltip_text(Some(
        "Automatic uses the best encoder that works on this computer (a graphics chip when it has one, else the processor).",
    ));
    tech_grid.attach(&grid_label("Encoder"), 0, 0, 1, 1);
    tech_grid.attach(&encoder, 1, 0, 1, 1);
    // Rendering: Automatic, Hardware (the graphics card puts the pictures
    // together) or Software (the processor does). Automatic follows the last
    // check, which Kani makes by itself the first time and after each update.
    let gpu_mode = dropdown(&["Automatic (recommended)", "Hardware", "Software"]);
    gpu_mode.set_selected(match settings.export_graphics_card.as_str() {
        "on" => 1,
        "off" => 2,
        _ => 0,
    });
    gpu_mode.set_tooltip_text(Some(
        "Automatic: the graphics card builds the pictures when a check found it works and is clearly faster, otherwise the processor does. Hardware: use the graphics card whenever it can (anything it cannot do falls back to the processor). Software: the processor does it all.",
    ));
    tech_grid.attach(&grid_label("Rendering"), 0, 1, 1, 1);
    tech_grid.attach(&gpu_mode, 1, 1, 1, 1);
    let gpu_button = gtk::Button::with_label("Check");
    gpu_button.set_tooltip_text(Some("Test the graphics card again"));
    let gpu_note = gtk::Label::new(Some(&card_note(&settings)));
    gpu_note.set_xalign(0.0);
    gpu_note.set_hexpand(true);
    let gpu_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    gpu_row.append(&gpu_note);
    gpu_row.append(&gpu_button);
    tech_grid.attach(&grid_label("Hardware"), 0, 2, 1, 1);
    tech_grid.attach(&gpu_row, 1, 2, 1, 1);
    right.append(&heading);
    right.append(&tech_grid);
    {
        let (app, note) = (app.clone(), gpu_note.clone());
        gpu_button.connect_clicked(move |button| check_card(&app, button, &note));
    }
    if settings.graphics_card_needs_check(env!("CARGO_PKG_VERSION")) {
        check_card(app, &gpu_button, &gpu_note);
    }

    let scroll = gtk::ScrolledWindow::new();
    // The right-hand side is a fixed width whatever the lists say.
    scroll.set_hscrollbar_policy(gtk::PolicyType::External);
    scroll.set_min_content_height(380);
    scroll.set_min_content_width(390);
    scroll.set_max_content_width(390);
    scroll.set_hexpand(true);
    scroll.set_child(Some(&right));
    right.set_margin_end(12);

    // While exporting, the right-hand side shows this instead of the choices:
    // the window, its size and the picture on the left all stay as they were.
    let bar = gtk::ProgressBar::new();
    bar.set_show_text(true);
    let status_title = gtk::Label::new(Some("Exporting"));
    status_title.set_xalign(0.0);
    status_title.add_css_class("heading");
    let note = gtk::Label::new(None);
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.add_css_class("dim-label");
    let log = gtk::TextBuffer::new(None);
    let log_view = gtk::TextView::with_buffer(&log);
    log_view.set_editable(false);
    log_view.set_cursor_visible(false);
    log_view.set_wrap_mode(gtk::WrapMode::WordChar);
    log_view.add_css_class("export-log");
    log_view.set_left_margin(6);
    log_view.set_right_margin(6);
    log_view.set_top_margin(4);
    let log_scroll = gtk::ScrolledWindow::new();
    log_scroll.set_vexpand(true);
    log_scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    log_scroll.set_min_content_height(250);
    log_scroll.set_child(Some(&log_view));
    let log_frame = gtk::Frame::new(None);
    log_frame.set_child(Some(&log_scroll));
    let run = gtk::Box::new(gtk::Orientation::Vertical, 8);
    run.append(&status_title);
    run.append(&bar);
    run.append(&note);
    run.append(&log_frame);
    run.set_margin_end(12);
    run.set_width_request(390);

    let stack = gtk::Stack::new();
    stack.set_hhomogeneous(true);
    stack.set_vhomogeneous(true);
    stack.set_hexpand(false);
    stack.set_width_request(390);
    stack.add_named(&scroll, Some("form"));
    stack.add_named(&run, Some("run"));
    stack.set_visible_child_name("form");

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    content.append(&left);
    content.append(&stack);

    // The buttons are the same size and place throughout: Export then Cancel at
    // the lower right. While exporting Export is hidden and Cancel stays put; when
    // it is over they read "Show in folder" and "Close".
    let summary = gtk::Label::new(None);
    summary.set_xalign(0.0);
    summary.set_hexpand(true);
    summary.set_wrap(true);
    summary.add_css_class("dim-label");
    let cancel = gtk::Button::with_label("Cancel");
    let export = gtk::Button::with_label("Export");
    for b in [&cancel, &export] {
        b.set_size_request(96, -1);
    }
    export.add_css_class("suggested-action");
    export.set_sensitive(false);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.append(&summary);
    buttons.append(&export);
    buttons.append(&cancel);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.set_margin_top(16);
    root.set_margin_bottom(14);
    root.set_margin_start(18);
    root.set_margin_end(18);
    root.append(&content);
    root.append(&buttons);
    window.set_child(Some(&root));

    let dialog = Rc::new(Dialog {
        window,
        name,
        folder,
        ext,
        video_on,
        audio_on,
        captions_on,
        aspect,
        size,
        fit,
        fit_row,
        bitrate,
        custom_kbps,
        codec,
        format,
        rate,
        video_box,
        audio_format,
        audio_bitrate,
        channels,
        audio_box,
        caption_srt,
        caption_vtt,
        caption_ass,
        caption_box,
        encoder,
        gpu_mode,
        preview,
        preview_note,
        files,
        summary,
        export,
        cancel,
        stack,
        bar,
        note,
        log,
        finished: Cell::new(false),
        folder_path: RefCell::new(folder_path),
        project: canvas,
        project_rate,
        aspects,
        resolutions: RefCell::new(Vec::new()),
        rates,
        containers: RefCell::new(Vec::new()),
        audio_formats: RefCell::new(Vec::new()),
        encoders: RefCell::new(Vec::new()),
        timeline,
        sources,
        cues,
        caption_look: look,
        playhead,
        seconds,
        sample_rate,
        running: RefCell::new(None),
        saved: RefCell::new(None),
        dimmed: RefCell::new(None),
        ready: Cell::new(false),
        search: Cell::new(0),
        busy: Cell::new(false),
        gone: Cell::new(false),
    });
    ACTIVE.with(|a| *a.borrow_mut() = Rc::downgrade(&dialog));
    restore_choices(&dialog, &settings);
    connect(app, &dialog);
    refresh_all(&dialog);
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
                    start_preview(&d);
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


/// Put the lists in order for what is chosen, starting from the saved choices.
fn restore_choices(d: &Dialog, settings: &cutroom_core::settings::Settings) {
    // Size: starts on the project's own.
    refresh_resolutions(d);
    // Bit rate presets.
    let preset = BitratePreset::from_id(&settings.export_quality).unwrap_or(BitratePreset::Recommended);
    refresh_bitrate_labels(d, preset);
    // Codec, then the formats that hold it.
    let codec = VideoCodec::from_id(&settings.export_codec).unwrap_or(VideoCodec::H264);
    select(
        d,
        &d.codec,
        VideoCodec::ALL.iter().position(|c| *c == codec).unwrap_or(0) as u32,
    );
    refresh_formats(d, Container::from_id(&settings.export_container));
    // Sound.
    let channels = Channels::from_id(&settings.export_channels).unwrap_or(Channels::Stereo);
    select(
        d,
        &d.channels,
        Channels::ALL.iter().position(|c| *c == channels).unwrap_or(0) as u32,
    );
    refresh_audio_formats(d, AudioFormat::from_id(&settings.export_audio_format));
    let labels: Vec<String> = AUDIO_BITRATES.iter().map(|k| format!("{k} kbps")).collect();
    let at = AUDIO_BITRATES
        .iter()
        .position(|k| *k == settings.export_audio_kbps)
        .unwrap_or(3) as u32;
    set_items(d, &d.audio_bitrate, &labels, at);
}

/// Everything that follows from the current choices.
fn refresh_all(d: &Dialog) {
    refresh_enabled(d);
    update_summary(d);
}

fn chosen_aspect(d: &Dialog) -> (u32, u32) {
    d.aspects[(d.aspect.selected() as usize).min(d.aspects.len() - 1)].1
}

/// Rebuild the size list for the chosen shape and pick the best start.
fn refresh_resolutions(d: &Dialog) {
    let list = opts::resolutions(chosen_aspect(d), d.project);
    let labels: Vec<String> = list.iter().map(|r| r.label.clone()).collect();
    let at = opts::default_resolution(&list, d.project) as u32;
    *d.resolutions.borrow_mut() = list;
    set_items(d, &d.size, &labels, at);
}

fn chosen_resolution(d: &Dialog) -> (u32, u32) {
    let list = d.resolutions.borrow();
    let r = &list[(d.size.selected() as usize).min(list.len().saturating_sub(1))];
    (r.width, r.height)
}

fn chosen_rate(d: &Dialog) -> FrameRate {
    d.rates[(d.rate.selected() as usize).min(d.rates.len() - 1)]
}

fn chosen_codec(d: &Dialog) -> VideoCodec {
    VideoCodec::ALL[(d.codec.selected() as usize).min(VideoCodec::ALL.len() - 1)]
}

fn chosen_container(d: &Dialog) -> Container {
    let list = d.containers.borrow();
    list.get(d.format.selected() as usize)
        .or(list.first())
        .copied()
        .unwrap_or(Container::Mp4)
}

fn chosen_preset(d: &Dialog) -> BitratePreset {
    BitratePreset::ALL[(d.bitrate.selected() as usize).min(BitratePreset::ALL.len() - 1)]
}

fn chosen_video_kbps(d: &Dialog) -> u32 {
    let preset = chosen_preset(d);
    if preset == BitratePreset::Custom {
        return d.custom_kbps.value() as u32;
    }
    let (w, h) = chosen_resolution(d);
    opts::video_kbps(chosen_codec(d), w, h, chosen_rate(d).as_f64(), preset)
}

/// The bit rate names, with what each comes to for the chosen size, frame rate and codec.
fn refresh_bitrate_labels(d: &Dialog, keep: BitratePreset) {
    let (w, h) = chosen_resolution_or(d);
    let (codec, fps) = (chosen_codec(d), chosen_rate(d).as_f64());
    let labels: Vec<String> = BitratePreset::ALL
        .iter()
        .map(|p| {
            if *p == BitratePreset::Custom {
                p.name().to_string()
            } else {
                format!("{} - {}", p.name(), opts::mbps(opts::video_kbps(codec, w, h, fps, *p)))
            }
        })
        .collect();
    let at = BitratePreset::ALL.iter().position(|p| *p == keep).unwrap_or(1) as u32;
    set_items(d, &d.bitrate, &labels, at);
}

/// The chosen size, or the project's while the list is still being filled.
fn chosen_resolution_or(d: &Dialog) -> (u32, u32) {
    if d.resolutions.borrow().is_empty() {
        d.project
    } else {
        chosen_resolution(d)
    }
}

/// The file formats that can hold the chosen video format (and are installed).
fn refresh_formats(d: &Dialog, prefer: Option<Container>) {
    let keep = prefer.or_else(|| d.containers.borrow().get(d.format.selected() as usize).copied());
    let list: Vec<Container> = chosen_codec(d)
        .containers()
        .iter()
        .copied()
        .filter(|c| export::container_available(*c))
        .collect();
    let at = keep
        .and_then(|k| list.iter().position(|c| *c == k))
        .unwrap_or(0) as u32;
    let labels: Vec<String> = list.iter().map(|c| c.label().to_string()).collect();
    *d.containers.borrow_mut() = list;
    set_items(d, &d.format, &labels, at);
}

/// The sound formats on offer: the ones the video file's format can carry
/// when a video is made, otherwise all that are installed.
fn refresh_audio_formats(d: &Dialog, prefer: Option<AudioFormat>) {
    let keep = prefer.or_else(|| d.audio_formats.borrow().get(d.audio_format.selected() as usize).copied());
    let allowed: Vec<AudioFormat> = if d.video_on.is_active() {
        chosen_container(d).audio_formats().to_vec()
    } else {
        AudioFormat::ALL.to_vec()
    };
    let list: Vec<AudioFormat> = allowed
        .into_iter()
        .filter(|f| export::audio_available(*f))
        .collect();
    let at = keep
        .and_then(|k| list.iter().position(|f| *f == k))
        .unwrap_or(0) as u32;
    let labels: Vec<String> = list.iter().map(|f| f.label().to_string()).collect();
    *d.audio_formats.borrow_mut() = list;
    set_items(d, &d.audio_format, &labels, at);
}

fn chosen_audio_format(d: &Dialog) -> AudioFormat {
    let list = d.audio_formats.borrow();
    list.get(d.audio_format.selected() as usize)
        .or(list.first())
        .copied()
        .unwrap_or(AudioFormat::Aac)
}

fn chosen_channels(d: &Dialog) -> Channels {
    Channels::ALL[(d.channels.selected() as usize).min(Channels::ALL.len() - 1)]
}

fn chosen_audio_kbps(d: &Dialog) -> u32 {
    let format = chosen_audio_format(d);
    if format.is_lossy() {
        AUDIO_BITRATES[(d.audio_bitrate.selected() as usize).min(AUDIO_BITRATES.len() - 1)]
    } else {
        opts::lossless_kbps(format, chosen_channels(d), d.sample_rate)
    }
}

fn caption_formats(d: &Dialog) -> Vec<&'static str> {
    if !d.captions_on.is_active() || d.cues.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if d.caption_srt.is_active() {
        out.push("srt");
    }
    if d.caption_vtt.is_active() {
        out.push("vtt");
    }
    if d.caption_ass.is_active() {
        out.push("ass");
    }
    out
}

/// Switch sections on and off, and say whether Export can be pressed.
fn refresh_enabled(d: &Dialog) {
    let video = d.video_on.is_active();
    let audio = d.audio_on.is_active();
    set_section_sensitive(&d.video_box, &d.video_on, video);
    set_section_sensitive(&d.audio_box, &d.audio_on, audio);
    let has_captions = !d.cues.is_empty();
    d.captions_on.set_sensitive(has_captions);
    for c in [&d.caption_srt, &d.caption_vtt, &d.caption_ass] {
        c.set_sensitive(has_captions && d.captions_on.is_active());
    }
    // Shape differs from the project's: ask what to do about it.
    let differs = !opts::same_shape(chosen_aspect(d), d.project);
    for w in &d.fit_row {
        w.set_visible(differs);
    }
    d.custom_kbps
        .set_visible(chosen_preset(d) == BitratePreset::Custom);
    d.encoder.set_sensitive(video && !d.encoders.borrow().is_empty());
    d.audio_bitrate
        .set_sensitive(audio_format_is_lossy(d));
    let anything = video || audio || !caption_formats(d).is_empty();
    let video_ready = !video || (d.ready.get() && !d.encoders.borrow().is_empty());
    d.export.set_sensitive(anything && video_ready);
}

fn audio_format_is_lossy(d: &Dialog) -> bool {
    chosen_audio_format(d).is_lossy()
}

/// Dim a section's choices when its tick box is off (the box itself stays usable).
fn set_section_sensitive(section: &gtk::Box, header: &gtk::CheckButton, on: bool) {
    let mut child = section.first_child();
    while let Some(w) = child {
        if w.downcast_ref::<gtk::CheckButton>().is_none_or(|c| c != header) {
            w.set_sensitive(on);
        }
        child = w.next_sibling();
    }
}

/// The video file's path, from the name and folder in the window.
pub fn output_path(d: &Dialog) -> PathBuf {
    let stem = clean_name(d.name.text().as_str());
    d.folder_path
        .borrow()
        .join(format!("{stem}.{}", chosen_container(d).extension()))
}

/// The sound file's path.
fn audio_path(d: &Dialog) -> PathBuf {
    let stem = clean_name(d.name.text().as_str());
    let ext = chosen_audio_format(d).file_extension();
    d.folder_path.borrow().join(format!("{stem}.{ext}"))
}

fn caption_path(d: &Dialog, ext: &str) -> PathBuf {
    let stem = clean_name(d.name.text().as_str());
    d.folder_path.borrow().join(format!("{stem}.{ext}"))
}

/// Every file this export would make, in order, with a few words on each.
fn planned_files(d: &Dialog) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    if d.video_on.is_active() {
        let (w, h) = chosen_resolution(d);
        out.push((
            output_path(d),
            format!(
                "video, {w} x {h}, {}, with {} sound",
                chosen_codec(d).name(),
                chosen_audio_format(d).label().split(' ').next().unwrap_or("")
            ),
        ));
    }
    if d.audio_on.is_active() {
        out.push((
            audio_path(d),
            format!("sound only, {}", chosen_audio_format(d).label().split(' ').next().unwrap_or("")),
        ));
    }
    for f in caption_formats(d) {
        out.push((caption_path(d, f), format!("captions, .{f}")));
    }
    out
}

/// What the window says at the bottom and on the left.
fn update_summary(d: &Dialog) {
    if d.busy.get() {
        return;
    }
    d.ext.set_text(&format!(
        ".{}",
        if d.video_on.is_active() {
            chosen_container(d).extension().to_string()
        } else if d.audio_on.is_active() {
            chosen_audio_format(d).file_extension().to_string()
        } else {
            "srt".to_string()
        }
    ));
    let files = planned_files(d);
    d.files.set_text(&if files.is_empty() {
        "Nothing is switched on to export.".to_string()
    } else {
        let mut text = String::from("This will make:");
        for (path, what) in &files {
            let name = path.file_name().map_or(String::new(), |n| n.to_string_lossy().to_string());
            text.push_str(&format!("\n{name}\n   {what}"));
        }
        text
    });
    let video_kbps = d.video_on.is_active().then(|| chosen_video_kbps(d));
    let audio_kbps = (d.video_on.is_active() || d.audio_on.is_active()).then(|| chosen_audio_kbps(d));
    let mut bytes = 0.0;
    if let Some(v) = video_kbps {
        bytes += opts::estimated_bytes(d.seconds, Some(v), audio_kbps);
    }
    if d.audio_on.is_active() {
        bytes += opts::estimated_bytes(d.seconds, None, audio_kbps);
    }
    let mut text = format!("Duration: {}", clock(d.seconds));
    if bytes > 0.0 {
        text.push_str(&format!(" | Size: about {}", megabytes(bytes)));
    }
    if d.video_on.is_active() && d.ready.get() && d.encoders.borrow().is_empty() {
        text = format!(
            "No {} video encoder was found. Pick another video format, or install GStreamer's plug-in for it.",
            chosen_codec(d).name()
        );
    }
    d.summary.set_text(&text);
}

/// Test the encoders for the chosen video format on a worker thread (a second
/// or two), then offer them.
fn find_encoders(app: &Rc<App>, dialog: &Rc<Dialog>) {
    let codec = chosen_codec(dialog);
    let generation = dialog.search.get() + 1;
    dialog.search.set(generation);
    dialog.ready.set(false);
    dialog.encoder.set_sensitive(false);
    set_items(dialog, &dialog.encoder, &["Looking for encoders...".to_string()], 0);
    refresh_enabled(dialog);
    let found: Arc<Mutex<Option<Vec<Encoder>>>> = Arc::new(Mutex::new(None));
    {
        let found = found.clone();
        std::thread::spawn(move || {
            let list = export::working_encoders(codec).to_vec();
            *found.lock().unwrap() = Some(list);
        });
    }
    let (app, dialog) = (app.clone(), dialog.clone());
    glib::timeout_add_local(Duration::from_millis(100), move || {
        if dialog.gone.get() || (dialog.window.surface().is_none() && !dialog.window.is_visible()) {
            return glib::ControlFlow::Break;
        }
        let Some(list) = found.lock().unwrap().take() else {
            return glib::ControlFlow::Continue;
        };
        if dialog.search.get() != generation {
            // The video format was changed meanwhile; that search has its own answer.
            return glib::ControlFlow::Break;
        }
        let mut labels = vec![match list.first() {
            Some(best) => format!("Automatic ({})", best.label),
            None => "None found".to_string(),
        }];
        labels.extend(list.iter().map(|e| e.label.to_string()));
        let wanted = app.state.borrow().settings.export_encoder.clone();
        let index = list
            .iter()
            .position(|e| e.id == wanted)
            .map_or(0, |i| i as u32 + 1);
        set_items(&dialog, &dialog.encoder, &labels, index);
        *dialog.encoders.borrow_mut() = list;
        dialog.ready.set(true);
        refresh_all(&dialog);
        glib::ControlFlow::Break
    });
}

/// The encoders to try: the one picked, then (for "Automatic") all the others.
fn chosen_encoders(d: &Dialog) -> Vec<Encoder> {
    let all = d.encoders.borrow().clone();
    if all.is_empty() {
        return all;
    }
    match d.encoder.selected() {
        0 => all,
        n => {
            let mut list = vec![all[(n as usize - 1).min(all.len() - 1)]];
            // If the one picked fails, fall back to software rather than give up.
            let first = list[0];
            list.extend(all.iter().filter(|e| !e.hardware && **e != first).copied());
            list
        }
    }
}

/// The job for what is chosen. `video` and `audio` say which files to make.
fn make_job(d: &Dialog, gpu: bool) -> Job {
    let (width, height) = chosen_resolution(d);
    let fill = d.fit.selected() == 1;
    let (cw, ch) = opts::canvas_inside((width, height), d.project, fill);
    let video = d.video_on.is_active();
    Job {
        timeline: d.timeline.clone(),
        sources: d.sources.clone(),
        width,
        height,
        canvas_width: cw,
        canvas_height: ch,
        fps: chosen_rate(d),
        sample_rate: d.sample_rate,
        video_kbps: chosen_video_kbps(d),
        audio_format: chosen_audio_format(d),
        audio_kbps: chosen_audio_kbps(d),
        channels: chosen_channels(d),
        container: chosen_container(d),
        encoders: if video { chosen_encoders(d) } else { Vec::new() },
        output: video.then(|| output_path(d)),
        audio_output: d.audio_on.is_active().then(|| audio_path(d)),
        limit_seconds: test_limit(),
        gpu,
    }
}

/// Draw one picture of the project at the playhead on the left, on a worker thread.
fn start_preview(d: &Rc<Dialog>) {
    let job = make_job(d, false);
    let at = d.playhead.min(d.timeline.duration());
    let found: Arc<Mutex<Option<Option<(u32, u32, Vec<u8>)>>>> = Arc::new(Mutex::new(None));
    {
        let found = found.clone();
        std::thread::spawn(move || {
            let result = export::preview_frame(&job, at, 520);
            *found.lock().unwrap() = Some(result);
        });
    }
    let d = d.clone();
    glib::timeout_add_local(Duration::from_millis(150), move || {
        if d.gone.get() {
            return glib::ControlFlow::Break;
        }
        let Some(result) = found.lock().unwrap().take() else {
            return glib::ControlFlow::Continue;
        };
        match result {
            Some((w, h, pixels)) => {
                let texture = gtk::gdk::MemoryTexture::new(
                    w as i32,
                    h as i32,
                    gtk::gdk::MemoryFormat::R8g8b8a8,
                    &glib::Bytes::from_owned(pixels),
                    w as usize * 4,
                );
                d.preview.set_paintable(Some(&texture));
                d.preview_note
                    .set_text(&format!("The picture at {} on the timeline", clock(d.playhead.as_seconds_f64())));
            }
            None => d.preview_note.set_text("No preview picture could be made."),
        }
        glib::ControlFlow::Break
    });
}

fn connect(app: &Rc<App>, d: &Rc<Dialog>) {
    // Anything that changes the summary only.
    for dd in [&d.size, &d.rate, &d.audio_bitrate, &d.channels, &d.fit] {
        let d = d.clone();
        dd.connect_selected_notify(move |_| {
            if d.busy.get() {
                return;
            }
            refresh_bitrate_labels(&d, chosen_preset(&d));
            refresh_all(&d);
        });
    }
    {
        let d = d.clone();
        d.aspect.clone().connect_selected_notify(move |_| {
            if d.busy.get() {
                return;
            }
            refresh_resolutions(&d);
            refresh_bitrate_labels(&d, chosen_preset(&d));
            refresh_all(&d);
        });
    }
    {
        let d = d.clone();
        d.bitrate.clone().connect_selected_notify(move |dd| {
            if d.busy.get() {
                return;
            }
            if chosen_preset(&d) == BitratePreset::Custom && dd.selected() as usize == BitratePreset::ALL.len() - 1 {
                // Start the custom number from what Recommended comes to.
                let (w, h) = chosen_resolution(&d);
                let start = opts::video_kbps(chosen_codec(&d), w, h, chosen_rate(&d).as_f64(), BitratePreset::Recommended);
                if d.custom_kbps.value() < 501.0 {
                    d.custom_kbps.set_value(f64::from(start));
                }
            }
            refresh_all(&d);
        });
    }
    {
        let d = d.clone();
        d.custom_kbps.clone().connect_value_changed(move |_| update_summary(&d));
    }
    {
        let (app, d) = (app.clone(), d.clone());
        d.codec.clone().connect_selected_notify(move |_| {
            if d.busy.get() {
                return;
            }
            refresh_formats(&d, None);
            refresh_audio_formats(&d, None);
            refresh_bitrate_labels(&d, chosen_preset(&d));
            find_encoders(&app, &d);
            refresh_all(&d);
        });
    }
    {
        let d = d.clone();
        d.format.clone().connect_selected_notify(move |_| {
            if d.busy.get() {
                return;
            }
            refresh_audio_formats(&d, None);
            refresh_all(&d);
        });
    }
    {
        let d = d.clone();
        d.audio_format.clone().connect_selected_notify(move |_| {
            if d.busy.get() {
                return;
            }
            refresh_all(&d);
        });
    }
    for check in [&d.video_on, &d.audio_on, &d.captions_on, &d.caption_srt, &d.caption_vtt, &d.caption_ass] {
        let d = d.clone();
        check.connect_toggled(move |_| {
            if d.busy.get() {
                return;
            }
            // The sound formats depend on whether a video file is made.
            refresh_audio_formats(&d, None);
            refresh_all(&d);
        });
    }
    {
        let d = d.clone();
        d.name.clone().connect_changed(move |_| update_summary(&d));
    }
    {
        // Cancel stops a running export; otherwise (before, or after it is over) it closes.
        let d2 = d.clone();
        d.cancel.connect_clicked(move |_| {
            if d2.running.borrow().is_some() {
                stop_export(&d2);
            } else {
                close(&d2);
            }
        });
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
        let change = d
            .folder
            .parent()
            .and_then(|p| p.last_child())
            .and_downcast::<gtk::Button>();
        if let Some(change) = change {
            change.connect_clicked(move |_| {
                let dialog = gtk::FileDialog::builder()
                    .title("Save the files in")
                    .build();
                dialog.set_initial_folder(Some(&gio::File::for_path(&*d2.folder_path.borrow())));
                let d3 = d2.clone();
                dialog.select_folder(Some(&d2.window), gio::Cancellable::NONE, move |result| {
                    if let Some(path) = result.ok().and_then(|f| f.path()) {
                        d3.folder.set_text(&path.display().to_string());
                        d3.folder
                            .set_tooltip_text(Some(&path.display().to_string()));
                        *d3.folder_path.borrow_mut() = path;
                        update_summary(&d3);
                    }
                });
            });
        }
    }
    {
        let (app, d2) = (app.clone(), d.clone());
        d.export.connect_clicked(move |_| {
            if d2.finished.get() {
                // Once the export is over this button shows the files.
                let first = d2.saved.borrow().clone();
                if let Some(parent) = first.as_ref().and_then(|p| p.parent()) {
                    let uri = gio::File::for_path(parent).uri();
                    let _ = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
                }
            } else {
                begin(&app, &d2);
            }
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

/// Add a line to the log shown while exporting.
fn log_line(d: &Dialog, line: &str) {
    let mut end = d.log.end_iter();
    if d.log.char_count() > 0 {
        d.log.insert(&mut end, "\n");
    }
    let mut end = d.log.end_iter();
    d.log.insert(&mut end, line);
}

fn stop_export(d: &Rc<Dialog>) {
    if let Some(run) = d.running.borrow().as_ref() {
        run.progress.cancel.store(true, Ordering::Relaxed);
    }
    d.cancel.set_sensitive(false);
    d.note.set_text("Cancelling...");
    log_line(d, "Cancelling ...");
}

fn begin(app: &Rc<App>, d: &Rc<Dialog>) {
    let existing: Vec<String> = planned_files(d)
        .into_iter()
        .filter(|(p, _)| p.exists())
        .map(|(p, _)| p.file_name().map_or(String::new(), |n| n.to_string_lossy().to_string()))
        .collect();
    if existing.is_empty() {
        start(app, d);
        return;
    }
    let alert = gtk::AlertDialog::builder()
        .message(if existing.len() == 1 {
            format!("{} already exists.", existing[0])
        } else {
            format!("{} files already exist.", existing.len())
        })
        .detail(if existing.len() == 1 {
            "Replace it?".to_string()
        } else {
            format!("{}\n\nReplace them?", existing.join("\n"))
        })
        .buttons(["Cancel", "Replace"])
        .cancel_button(0)
        .default_button(0)
        .build();
    let (app, d2) = (app.clone(), d.clone());
    alert.choose(Some(&d.window), gio::Cancellable::NONE, move |answer| {
        if answer == Ok(1) {
            start(&app, &d2);
        }
    });
}

/// What the window says about hardware acceleration from the last check.
fn card_note(settings: &cutroom_core::settings::Settings) -> String {
    match settings.graphics_card_available.or(settings.graphics_card_ok) {
        Some(true) => "Available".into(),
        Some(false) => "Not available".into(),
        None => "Checking ...".into(),
    }
}

/// Check the graphics card in the background (it takes a few seconds), show
/// what was found and remember it for automatic mode.
fn check_card(app: &Rc<App>, button: &gtk::Button, note: &gtk::Label) {
    button.set_sensitive(false);
    note.set_text("Checking ...");
    let found: Arc<Mutex<Option<crate::gpu::GpuReport>>> = Arc::new(Mutex::new(None));
    {
        let found = found.clone();
        std::thread::spawn(move || {
            *found.lock().unwrap() = Some(crate::gpu::test());
        });
    }
    let (app, button, note) = (app.clone(), button.clone(), note.clone());
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
            if report.recommended {
                "recommended"
            } else if report.usable {
                "usable, not recommended"
            } else {
                "not usable"
            }
        );
        {
            let mut st = app.state.borrow_mut();
            st.settings.graphics_card_ok = Some(report.recommended);
            st.settings.graphics_card_available = Some(report.available);
            st.settings.graphics_card_checked = env!("CARGO_PKG_VERSION").to_string();
            st.save_settings();
        }
        note.set_text(if report.available { "Available" } else { "Not available" });
        button.set_sensitive(true);
        glib::ControlFlow::Break
    });
}

/// The look of the caption text for an .ass file, from the first caption.
fn ass_style(d: &Dialog, height: u32) -> cutroom_captions::srt::AssStyle {
    ass_style_for(&d.caption_look, height)
}

fn ass_style_for(
    look: &Option<(cutroom_timeline::TextSpec, cutroom_timeline::Transform)>,
    height: u32,
) -> cutroom_captions::srt::AssStyle {
    let mut style = cutroom_captions::srt::AssStyle::default();
    let Some((spec, transform)) = look else {
        return style;
    };
    let h = f64::from(height);
    let size = (f64::from(spec.size) / 1000.0 * h * 0.8).round().max(8.0);
    if !spec.font.is_empty() {
        style.font = spec.font.clone();
    }
    style.size = size as u32;
    style.bold = spec.bold;
    style.italic = spec.italic;
    style.text = [spec.color[0], spec.color[1], spec.color[2]];
    style.outline = [spec.outline_color[0], spec.outline_color[1], spec.outline_color[2]];
    style.outline_px = if spec.outline_on {
        (f64::from(spec.outline) / 1000.0 * size) as f32
    } else {
        0.0
    };
    style.shadow_px = if spec.shadow_on {
        (f64::from(spec.shadow) / 1000.0 * size) as f32
    } else {
        0.0
    };
    // The caption sits where its transform put it: its middle is that far
    // down the picture; the margin is the gap below its bottom edge.
    let middle = h / 2.0 + f64::from(transform.y) / 1000.0 * h;
    style.margin_bottom = (h - (middle + size * 0.6)).max(10.0) as u32;
    style
}

/// Write the caption files; returns the ones saved, and what went wrong.
fn write_captions(d: &Dialog) -> (Vec<PathBuf>, Vec<String>) {
    let (mut saved, mut problems) = (Vec::new(), Vec::new());
    let (w, h) = if d.video_on.is_active() {
        chosen_resolution(d)
    } else {
        d.project
    };
    for ext in caption_formats(d) {
        let text = match ext {
            "srt" => cutroom_captions::srt::to_srt(&d.cues),
            "vtt" => cutroom_captions::srt::to_vtt(&d.cues),
            _ => cutroom_captions::srt::to_ass(&d.cues, &ass_style(d, h), w, h),
        };
        let path = caption_path(d, ext);
        match std::fs::write(&path, text) {
            Ok(()) => saved.push(path),
            Err(e) => problems.push(format!("The .{ext} captions could not be saved: {e}")),
        }
    }
    (saved, problems)
}

fn start(app: &Rc<App>, d: &Rc<Dialog>) {
    // Remember the choices.
    {
        let mut st = app.state.borrow_mut();
        let preset = chosen_preset(d);
        st.settings.export_quality = preset.id().to_string();
        if preset == BitratePreset::Custom {
            st.settings.export_custom_kbps = d.custom_kbps.value() as u32;
        }
        st.settings.export_video = d.video_on.is_active();
        st.settings.export_audio = d.audio_on.is_active();
        st.settings.export_codec = chosen_codec(d).id().to_string();
        st.settings.export_container = chosen_container(d).id().to_string();
        st.settings.export_audio_format = chosen_audio_format(d).id().to_string();
        st.settings.export_audio_kbps = AUDIO_BITRATES
            [(d.audio_bitrate.selected() as usize).min(AUDIO_BITRATES.len() - 1)];
        st.settings.export_channels = chosen_channels(d).id().to_string();
        st.settings.export_fill = d.fit.selected() == 1;
        // Remember the caption files wanted even while Captions is switched off.
        let mut wanted = Vec::new();
        for (check, id) in [(&d.caption_srt, "srt"), (&d.caption_vtt, "vtt"), (&d.caption_ass, "ass")] {
            if check.is_active() && d.captions_on.is_active() {
                wanted.push(id.to_string());
            }
        }
        st.settings.export_caption_files = wanted;
        st.settings.export_encoder = match d.encoder.selected() {
            0 => "auto".into(),
            n => d
                .encoders
                .borrow()
                .get(n as usize - 1)
                .map_or("auto".into(), |e| e.id.to_string()),
        };
        st.settings.export_folder = Some(d.folder_path.borrow().clone());
        st.settings.export_graphics_card = match d.gpu_mode.selected() {
            1 => "on",
            2 => "off",
            _ => "auto",
        }
        .into();
        st.save_settings();
    }
    let gpu = app.state.borrow().settings.use_graphics_card();
    let job = make_job(d, gpu);
    let planned: Vec<PathBuf> = planned_files(d).into_iter().map(|(p, _)| p).collect();
    // Captions are quick: write them now.
    let (_, caption_problems) = write_captions(d);
    let progress = Arc::new(Progress::default());
    let outcome: Outcome = Arc::new(Mutex::new(None));
    if job.output.is_none() && job.audio_output.is_none() {
        // Only captions were asked for: nothing to render.
        *outcome.lock().unwrap() = Some(Ok(Report {
            encoder: "captions".into(),
            warnings: caption_problems.clone(),
            ..Report::default()
        }));
    } else {
        let (progress, outcome) = (progress.clone(), outcome.clone());
        std::thread::spawn(move || {
            let result = export::export(&job, &progress);
            *outcome.lock().unwrap() = Some(result);
        });
    }
    *d.running.borrow_mut() = Some(Running {
        progress: progress.clone(),
        outcome,
        started: Instant::now(),
    });
    d.stack.set_visible_child_name("run");
    d.export.set_visible(false);
    d.cancel.set_sensitive(true);
    d.window.set_title(Some("Exporting..."));
    d.bar.set_fraction(0.0);
    d.bar.set_text(Some("Starting..."));
    d.note.set_text("Starting ...");
    d.log.set_text("");
    d.preview_note.set_text("The picture being made");
    log_line(d, "Making:");
    for p in &planned {
        log_line(d, &format!("  {}", p.display()));
    }
    log_line(d, "Starting ...");

    let (app, d) = (app.clone(), d.clone());
    let logged_sound = Cell::new(false);
    glib::timeout_add_local(Duration::from_millis(120), move || {
        if d.gone.get() {
            return glib::ControlFlow::Break;
        }
        let (fraction, done, total, elapsed, finished, sound_pass) = {
            let running = d.running.borrow();
            let Some(r) = running.as_ref() else {
                return glib::ControlFlow::Break;
            };
            // The picture on the left follows the frames as they are made.
            if let Some(live) = r.progress.live.lock().unwrap().take() {
                let texture = gtk::gdk::MemoryTexture::new(
                    live.width as i32,
                    live.height as i32,
                    gtk::gdk::MemoryFormat::R8g8b8a8,
                    &glib::Bytes::from_owned(live.rgba),
                    live.width as usize * 4,
                );
                d.preview.set_paintable(Some(&texture));
                let total = r.progress.frames_total.load(Ordering::Relaxed);
                d.preview_note
                    .set_text(&format!("Frame {} of {total}", live.frame));
            }
            (
                r.progress.fraction(),
                r.progress.frames_done.load(Ordering::Relaxed),
                r.progress.frames_total.load(Ordering::Relaxed),
                r.started.elapsed().as_secs_f64(),
                {
                    let result = r.outcome.lock().unwrap().take();
                    result
                },
                r.progress.audio_pass.load(Ordering::Relaxed),
            )
        };
        match finished {
            None => {
                d.bar.set_fraction(fraction);
                d.bar.set_text(Some(&format!("{:.0}%", fraction * 100.0)));
                let left = if fraction > 0.02 {
                    format!(
                        ", about {} left",
                        clock(elapsed * (1.0 - fraction) / fraction)
                    )
                } else {
                    String::new()
                };
                let what = if sound_pass {
                    "Making the sound file".to_string()
                } else if total > 0 {
                    format!("Frame {done} of {total}")
                } else {
                    "Starting".to_string()
                };
                d.note
                    .set_text(&format!("{what}\n{} elapsed{left}", clock(elapsed)));
                if sound_pass && !logged_sound.replace(true) {
                    log_line(&d, "Making the sound file ...");
                }
                glib::ControlFlow::Continue
            }
            Some(result) => {
                d.running.borrow_mut().take();
                // Which of the planned files really exist now.
                let made: Vec<PathBuf> = planned
                    .iter()
                    .filter(|p| p.is_file())
                    .cloned()
                    .collect();
                finish(&app, &d, result, &made, &caption_problems);
                glib::ControlFlow::Break
            }
        }
    });
}

fn finish(
    app: &Rc<App>,
    d: &Rc<Dialog>,
    result: Result<Report, ExportError>,
    files: &[PathBuf],
    caption_problems: &[String],
) {
    match result {
        Err(ExportError::Cancelled) => {
            app.set_status("Export cancelled.");
            app.update_status();
            close(d);
        }
        Err(ExportError::Failed(message)) => {
            eprintln!("kani: the export failed: {message}");
            d.bar.set_fraction(0.0);
            d.bar.set_text(Some("The export failed"));
            d.note.set_text("The export failed");
            log_line(d, &message);
            d.window.set_title(Some("Export failed"));
            end_buttons(d, false);
            app.set_status(format!("Export failed: {message}"));
            app.update_status();
        }
        Ok(report) => {
            d.bar.set_fraction(1.0);
            d.bar.set_text(Some("Done"));
            d.window.set_title(Some("Export finished"));
            d.note.set_text(&match report.elapsed {
                e if e > Duration::ZERO => format!("Done\nTook {} (time elapsed)", elapsed_words(e.as_secs_f64())),
                _ => "Done".to_string(),
            });
            log_line(d, "Saved:");
            for path in files.iter().filter(|p| p.is_file()) {
                let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                log_line(d, &format!("  {} ({})", path.display(), megabytes(size as f64)));
            }
            if report.frames > 0 {
                log_line(d, &format!("{} frames, {}.", report.frames, report.encoder));
            }
            if report.elapsed > Duration::ZERO {
                log_line(
                    d,
                    &format!("Took {} (time elapsed).", elapsed_words(report.elapsed.as_secs_f64())),
                );
            }
            for warning in report.warnings.iter().chain(caption_problems.iter()) {
                log_line(d, &format!("Note: {warning}"));
            }
            let first = files.iter().find(|p| p.is_file()).cloned();
            *d.saved.borrow_mut() = first.clone();
            app.set_status(match &first {
                Some(p) => format!("Exported {}", p.display()),
                None => "Export finished.".to_string(),
            });
            app.update_status();
            end_buttons(d, first.is_some());
        }
    }
}

/// The export is over: the same two buttons now read "Show in folder" (when
/// there is something to show) and "Close".
fn end_buttons(d: &Dialog, can_show: bool) {
    d.finished.set(true);
    d.cancel.set_label("Close");
    d.cancel.set_sensitive(true);
    d.export.set_label("Show in folder");
    d.export.set_visible(can_show);
    d.export.set_sensitive(can_show);
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

impl Dialog {
    /// Is an export running in this window right now? (tests)
    #[cfg(test)]
    pub fn running_for_tests(&self) -> bool {
        self.running.borrow().is_some()
    }
}

/// Point the window at another folder (tests do not use the Videos folder).
#[cfg(test)]
pub fn set_folder_for_tests(d: &Dialog, folder: &std::path::Path) {
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

    #[test]
    fn caption_files_take_their_look_from_the_first_caption() {
        let mut spec = cutroom_timeline::TextSpec::default();
        spec.font = "Inter".into();
        spec.size = 50;
        spec.bold = true;
        spec.color = [250, 240, 20, 255];
        let look = Some((
            spec,
            cutroom_timeline::Transform {
                y: 300,
                ..cutroom_timeline::Transform::default()
            },
        ));
        let style = ass_style_for(&look, 1920);
        assert_eq!(style.font, "Inter");
        assert_eq!(style.size, 77, "50/1000 of 1920, times 0.8");
        assert_eq!(style.text, [250, 240, 20]);
        // The caption's middle is 30% below the centre: 1536 px down; its
        // bottom edge about 46 px lower; the rest of the picture is the margin.
        assert!((330..350).contains(&style.margin_bottom), "{}", style.margin_bottom);
        // No captions yet: a plain default look.
        assert_eq!(ass_style_for(&None, 1080), cutroom_captions::srt::AssStyle::default());
    }
}
