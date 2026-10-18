//! "Is decoding on the graphics chip really working, on my footage?"
//!
//! Decodes the start of one real file twice, as fast as it can go: once with
//! the graphics chip's decoders allowed and once with only the processor's.
//! It reports which decoder element each run used, how many pictures came
//! out, how long it took and how much processor time it cost, and says in
//! plain words what that means. The numbers also go to the console.

use crate::app::App;
use crate::dialogs::{buttons, caption, content_box, dialog_window};
use crate::hwdecode::{apply, is_hardware_decoder};
use gstreamer as gst;
use gstreamer::prelude::*;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long each run may decode.
const LIMIT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq)]
pub struct Decoder {
    pub name: String,
    /// What it decodes, e.g. "video/x-h264".
    pub codec: String,
    pub hardware: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    pub decoders: Vec<Decoder>,
    pub frames: u64,
    pub seconds: f64,
    /// Processor time the program used meanwhile (Linux only).
    pub cpu: Option<f64>,
    pub error: Option<String>,
}

impl Run {
    fn fps(&self) -> f64 {
        if self.seconds > 0.0 {
            self.frames as f64 / self.seconds
        } else {
            0.0
        }
    }

    fn used_hardware(&self) -> bool {
        self.decoders.iter().any(|d| d.hardware)
    }

    fn names(&self) -> String {
        if self.decoders.is_empty() {
            return "none found".into();
        }
        self.decoders
            .iter()
            .map(|d| {
                format!(
                    "{} ({})",
                    d.name,
                    if d.hardware {
                        "graphics chip"
                    } else {
                        "processor"
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub file: String,
    /// Every graphics-chip decoder GStreamer has, whether or not it took this file.
    pub installed: Vec<String>,
    pub hardware: Run,
    pub software: Run,
}

/// Processor time this program has used so far, in seconds.
#[cfg(target_os = "linux")]
fn process_cpu_seconds() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // After the "(name)" the fields go: state, ... utime (14th overall), stime (15th).
    let rest = stat.rsplit_once(')')?.1;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let ticks: f64 = fields.get(11)?.parse::<f64>().ok()? + fields.get(12)?.parse::<f64>().ok()?;
    Some(ticks / 100.0)
}

#[cfg(not(target_os = "linux"))]
fn process_cpu_seconds() -> Option<f64> {
    None
}

/// The graphics-chip video decoders GStreamer knows, by name.
pub fn installed_hardware_decoders() -> Vec<String> {
    if gst::init().is_err() {
        return Vec::new();
    }
    let mut names: Vec<String> = gst::Registry::get()
        .features(gst::ElementFactory::static_type())
        .iter()
        .filter(|f| {
            let klass = f
                .downcast_ref::<gst::ElementFactory>()
                .map(|f| f.klass().to_string());
            klass.is_some_and(|k| k.contains("Decoder") && k.contains("Video"))
                && is_hardware_decoder(&f.name())
        })
        .map(|f| f.name().to_string())
        .collect();
    names.sort();
    names
}

/// Decode the start of `path` as fast as possible and count what comes out.
fn decode_run(path: &Path) -> Run {
    let mut run = Run::default();
    let Ok(playbin) = gst::ElementFactory::make("playbin")
        .property("uri", crate::media::path_to_uri(path))
        .build()
    else {
        run.error = Some("GStreamer's playbin is missing".into());
        return run;
    };
    // Pictures only, handed on as they are (no conversion that would copy them).
    playbin.set_property_from_str("flags", "video+native-video");
    let Ok(sink) = gst::ElementFactory::make("fakesink")
        .property("sync", false)
        .build()
    else {
        run.error = Some("GStreamer's fakesink is missing".into());
        return run;
    };
    playbin.set_property("video-sink", &sink);
    let frames = Arc::new(AtomicU64::new(0));
    if let Some(pad) = sink.static_pad("sink") {
        let frames = frames.clone();
        pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
            frames.fetch_add(1, Ordering::Relaxed);
            gst::PadProbeReturn::Ok
        });
    }
    let found: Arc<Mutex<Vec<gst::Element>>> = Arc::new(Mutex::new(Vec::new()));
    if let Some(bin) = playbin.downcast_ref::<gst::Bin>() {
        let found = found.clone();
        bin.connect_deep_element_added(move |_, _, element| {
            let is_decoder = element.factory().is_some_and(|f| {
                let klass = f.klass();
                klass.contains("Decoder") && klass.contains("Video")
            });
            if is_decoder {
                if let Ok(mut list) = found.lock() {
                    list.push(element.clone());
                }
            }
        });
    }
    let (began, cpu_before) = (Instant::now(), process_cpu_seconds());
    if let Err(e) = playbin.set_state(gst::State::Playing) {
        run.error = Some(format!("could not start: {e}"));
        let _ = playbin.set_state(gst::State::Null);
        return run;
    }
    if let Some(bus) = playbin.bus() {
        while began.elapsed() < LIMIT {
            let Some(message) = bus.timed_pop_filtered(
                gst::ClockTime::from_mseconds(100),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            ) else {
                continue;
            };
            if let gst::MessageView::Error(e) = message.view() {
                run.error = Some(e.error().to_string());
            }
            break;
        }
    }
    run.seconds = began.elapsed().as_secs_f64();
    run.cpu = cpu_before
        .zip(process_cpu_seconds())
        .map(|(a, b)| (b - a).max(0.0));
    run.frames = frames.load(Ordering::Relaxed);
    if let Ok(list) = found.lock() {
        for element in list.iter() {
            let name = element
                .factory()
                .map(|f| f.name().to_string())
                .unwrap_or_default();
            let codec = element
                .static_pad("sink")
                .and_then(|p| p.current_caps())
                .and_then(|c| c.structure(0).map(|s| s.name().to_string()))
                .unwrap_or_default();
            run.decoders.push(Decoder {
                hardware: is_hardware_decoder(&name),
                name,
                codec,
            });
        }
    }
    let _ = playbin.set_state(gst::State::Null);
    run
}

/// The whole check. Blocks for up to about twenty seconds: run it on a thread.
/// `hw_setting` is the user's "Decode with the graphics chip", put back afterwards.
pub fn run_check(path: &Path, hw_setting: bool) -> Report {
    let _ = gst::init();
    let mut report = Report {
        file: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        installed: installed_hardware_decoders(),
        ..Report::default()
    };
    apply(true);
    report.hardware = decode_run(path);
    apply(false);
    report.software = decode_run(path);
    apply(hw_setting);
    report
}

/// What the numbers mean, in plain words.
pub fn explain(r: &Report) -> String {
    let (hw, sw) = (&r.hardware, &r.software);
    let codec = hw
        .decoders
        .iter()
        .chain(sw.decoders.iter())
        .map(|d| d.codec.as_str())
        .find(|c| !c.is_empty())
        .unwrap_or("this video");
    if let Some(e) = &hw.error {
        return format!("Decoding with the graphics chip failed: {e}. Turn off \"Decode with the graphics chip\" in Settings > Performance if the preview misbehaves with this kind of file.");
    }
    if hw.frames == 0 {
        return "Nothing could be decoded from this file, with or without the graphics chip."
            .into();
    }
    if !hw.used_hardware() {
        return if r.installed.is_empty() {
            "No graphics-chip video decoder is installed for GStreamer on this machine (VA-API, NVIDIA, Direct3D and Quick Sync were looked for), so the processor decodes everything. On Linux the VA plug-in is in gst-plugins-bad, with your graphics driver's VA-API support.".into()
        } else {
            format!(
                "None of the installed graphics-chip decoders took this file ({codec}), so the processor decoded it. The chip's decoders here are: {}. They may not support this codec, profile or colour depth.",
                r.installed.join(", ")
            )
        };
    }
    let mut text = format!(
        "Decoding on the graphics chip works for this file ({}).",
        hw.names()
    );
    match (hw.cpu, sw.cpu) {
        (Some(a), Some(b)) if b > 0.05 => {
            let saved = ((1.0 - a / b) * 100.0).round();
            if saved >= 10.0 {
                text += &format!(
                    " It used {a:.1} s of processor time against {b:.1} s on the processor alone: about {saved:.0}% less."
                );
            } else {
                text += &format!(
                    " It used {a:.1} s of processor time against {b:.1} s on the processor alone, so little is saved with this file."
                );
            }
        }
        _ => {}
    }
    if sw.fps() > 0.0 && hw.fps() < sw.fps() * 0.6 {
        text += " It was slower than the processor, though: the chip may be copying pictures back for the preview. If playback stutters, turn the setting off.";
    }
    text
}

/// The report as plain text (also what the Copy button copies).
pub fn text(r: &Report) -> String {
    let line = |label: &str, run: &Run| {
        let cpu = run
            .cpu
            .map_or(String::new(), |c| format!(", {c:.1} s of processor time"));
        match &run.error {
            Some(e) => format!("{label}: failed ({e})"),
            None => format!(
                "{label}: {} - {} pictures in {:.1} s ({:.0} per second){cpu}",
                run.names(),
                run.frames,
                run.seconds,
                run.fps()
            ),
        }
    };
    format!(
        "File: {}\nGraphics-chip decoders installed: {}\n\n{}\n{}\n\n{}",
        r.file,
        if r.installed.is_empty() {
            "none".to_string()
        } else {
            r.installed.join(", ")
        },
        line("Graphics chip allowed", &r.hardware),
        line("Processor only", &r.software),
        explain(r)
    )
}

/// Open the window, run the check on `path` and show what it found.
pub fn open(app: &Rc<App>, path: PathBuf) {
    let window = dialog_window(app, "Check hardware decoding", 560);
    let body = content_box();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    body.append(&caption(&format!("Decoding the start of \"{name}\" twice: with the graphics chip, then with the processor only.")));
    let busy = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let spinner = gtk::Spinner::new();
    spinner.start();
    busy.append(&spinner);
    busy.append(&gtk::Label::new(Some(
        "Testing... this takes up to 20 seconds. Leave playback stopped meanwhile.",
    )));
    body.append(&busy);
    let result = gtk::Label::new(None);
    result.set_selectable(true);
    result.set_wrap(true);
    result.set_xalign(0.0);
    result.set_max_width_chars(70);
    result.set_visible(false);
    body.append(&result);
    let copy = gtk::Button::with_label("Copy report");
    copy.set_sensitive(false);
    let close = gtk::Button::with_label("Close");
    body.append(&buttons(&close, &copy));
    window.set_child(Some(&body));
    {
        let window = window.clone();
        close.connect_clicked(move |_| window.close());
    }
    window.present();

    let hw_setting = app.state.borrow().settings.hw_decode;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run_check(&path, hw_setting));
    });
    let weak = window.downgrade();
    let app = app.clone();
    glib::timeout_add_local(Duration::from_millis(200), move || {
        let Some(window) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        match rx.try_recv() {
            Ok(report) => {
                let report_text = text(&report);
                eprintln!("kani: hardware decoding check\n{report_text}");
                spinner.stop();
                busy.set_visible(false);
                result.set_text(&report_text);
                result.set_visible(true);
                copy.set_sensitive(true);
                let app = app.clone();
                copy.connect_clicked(move |_| {
                    window.clipboard().set_text(&report_text);
                    app.set_status("Copied the hardware decoding report");
                });
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(_) => glib::ControlFlow::Break,
        }
    });
}

/// Choose a video file, then check it (the Settings button).
pub fn choose_and_open(app: &Rc<App>) {
    let dialog = gtk::FileDialog::new();
    dialog.set_title("Choose a video to test");
    let app2 = app.clone();
    dialog.open(
        Some(&app.window),
        gtk::gio::Cancellable::NONE,
        move |picked| {
            if let Some(path) = picked.ok().and_then(|f| f.path()) {
                open(&app2, path);
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(decoder: &str, hardware: bool, frames: u64, cpu: f64) -> Run {
        Run {
            decoders: vec![Decoder {
                name: decoder.into(),
                codec: "video/x-h264".into(),
                hardware,
            }],
            frames,
            seconds: 2.0,
            cpu: Some(cpu),
            error: None,
        }
    }

    #[test]
    fn the_verdict_says_what_happened() {
        let mut r = Report {
            file: "a.mp4".into(),
            installed: vec![],
            hardware: run("avdec_h264", false, 300, 4.0),
            software: run("avdec_h264", false, 300, 4.0),
        };
        assert!(explain(&r).contains("No graphics-chip video decoder is installed"));
        r.installed = vec!["vah265dec".into()];
        assert!(explain(&r).contains("None of the installed"));
        r.installed = vec!["vah264dec".into()];
        r.hardware = run("vah264dec", true, 300, 1.0);
        let text = explain(&r);
        assert!(
            text.contains("works for this file") && text.contains("75% less"),
            "{text}"
        );
        r.hardware.frames = 100;
        assert!(explain(&r).contains("slower than the processor"));
        r.hardware.error = Some("boom".into());
        assert!(explain(&r).contains("failed: boom"));
        r.hardware.error = None;
        r.hardware.frames = 0;
        assert!(explain(&r).contains("Nothing could be decoded"));
    }

    /// A real (tiny) file through both runs: here only the processor decodes.
    #[test]
    fn a_real_file_decodes_in_both_runs() {
        if gst::init().is_err()
            || gst::ElementFactory::find("x264enc").is_none()
            || gst::ElementFactory::find("mp4mux").is_none()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("clip.mp4");
        let launch = format!(
            "videotestsrc num-buffers=60 ! video/x-raw,width=320,height=240,framerate=30/1 ! x264enc ! mp4mux ! filesink location={}",
            file.display()
        );
        let pipeline = gst::parse::launch(&launch).unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        let bus = pipeline.bus().unwrap();
        bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(20),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        );
        pipeline.set_state(gst::State::Null).unwrap();
        let report = run_check(&file, true);
        assert_eq!(report.hardware.frames, 60, "{report:?}");
        assert_eq!(report.software.frames, 60, "{report:?}");
        assert!(report.software.decoders.iter().all(|d| !d.hardware));
        assert!(text(&report).contains("clip.mp4"));
    }
}
