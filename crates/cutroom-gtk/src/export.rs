//! Export (Phase 5): renders the timeline frame by frame, as fast as the
//! machine allows, and writes an MP4 with H.264 video and AAC sound.
//!
//! The pictures come from the same compositor and the same "what is on
//! screen at this moment" logic as the preview ([`find_targets`]), so what you
//! export is what you saw. Each video file is read by its own pipeline from
//! the clip's start, one frame after another. The sound of every clip that is
//! audible is mixed here, in small blocks, and fed to the AAC encoder.
//!
//! The video encoder is chosen per machine: the graphics card's encoder when
//! one works (NVIDIA NVENC, Intel Quick Sync, VA-API, AMD AMF, Apple
//! VideoToolbox), otherwise software (x264). Every candidate is test-encoded
//! once before it is offered, so a driver that is installed but broken is not
//! picked.

use crate::export_options::{AudioFormat, Channels, Container, VideoCodec};
use crate::media::{path_to_uri, sample_to_thumb};
use crate::preview::{find_targets, LayerSource};
use cutroom_compose::{
    covers_canvas, is_plain_copy, plain_copy_origin, render_layer, Canvas, Frame, Layer, Placement,
};
use cutroom_core::state::MediaKind;
use cutroom_platform::{HwBackend, Os};
use cutroom_timeline::{ClipId, FrameRate, MediaId, Ticks, Timeline};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

// ---------------------------------------------------------------- settings

/// The nearest frame at time `index / fps`.
fn frame_time(index: u64, fps: FrameRate) -> Ticks {
    Ticks::from_frames(index as i64, fps).unwrap_or_else(|| {
        Ticks::from_seconds_f64(index as f64 * f64::from(fps.den) / f64::from(fps.num))
    })
}

/// How many frames cover `duration`.
pub fn frame_count(duration: Ticks, fps: FrameRate) -> u64 {
    let frames = duration.as_seconds_f64() * fps.as_f64();
    (frames - 1e-6).ceil().max(0.0) as u64
}

// ---------------------------------------------------------------- encoders

/// A video encoder GStreamer may have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoder {
    /// Short stable name, kept in settings.
    pub id: &'static str,
    pub label: &'static str,
    /// The GStreamer element.
    pub element: &'static str,
    /// The video format it makes.
    pub codec: VideoCodec,
    pub hardware: bool,
    /// The element's bit-rate property, and how many of its units make a kilobit/s.
    bitrate_prop: &'static str,
    bitrate_unit: u32,
}

const fn enc(
    codec: VideoCodec,
    id: &'static str,
    label: &'static str,
    element: &'static str,
    hardware: bool,
) -> Encoder {
    enc_with(codec, id, label, element, hardware, "bitrate", 1)
}

const fn enc_with(
    codec: VideoCodec,
    id: &'static str,
    label: &'static str,
    element: &'static str,
    hardware: bool,
    bitrate_prop: &'static str,
    bitrate_unit: u32,
) -> Encoder {
    Encoder {
        id,
        label,
        element,
        codec,
        hardware,
        bitrate_prop,
        bitrate_unit,
    }
}

use VideoCodec::{Av1, Vp9, H264, H265};

const H264_NVENC: &[Encoder] = &[enc(H264, "nvenc", "NVIDIA NVENC", "nvh264enc", true)];
const H264_QSV: &[Encoder] = &[enc(H264, "qsv", "Intel Quick Sync", "qsvh264enc", true)];
const H264_VAAPI: &[Encoder] = &[
    enc(H264, "vaapi-lp", "VA-API (low power)", "vah264lpenc", true),
    enc(H264, "vaapi", "VA-API", "vah264enc", true),
    enc(
        H264,
        "vaapi-old",
        "VA-API (older plug-in)",
        "vaapih264enc",
        true,
    ),
];
const H264_AMF: &[Encoder] = &[enc(H264, "amf", "AMD AMF", "amfh264enc", true)];
const H264_VT: &[Encoder] = &[enc(
    H264,
    "videotoolbox",
    "Apple VideoToolbox",
    "vtenc_h264",
    true,
)];
const H264_MF: &[Encoder] = &[enc(
    H264,
    "mf",
    "Windows Media Foundation",
    "mfh264enc",
    true,
)];
const H264_SOFT: &[Encoder] = &[
    enc(H264, "x264", "Software (x264)", "x264enc", false),
    enc_with(
        H264,
        "openh264",
        "Software (OpenH264)",
        "openh264enc",
        false,
        "bitrate",
        1000,
    ),
];

const H265_NVENC: &[Encoder] = &[enc(H265, "nvenc-h265", "NVIDIA NVENC", "nvh265enc", true)];
const H265_QSV: &[Encoder] = &[enc(
    H265,
    "qsv-h265",
    "Intel Quick Sync",
    "qsvh265enc",
    true,
)];
const H265_VAAPI: &[Encoder] = &[
    enc(
        H265,
        "vaapi-lp-h265",
        "VA-API (low power)",
        "vah265lpenc",
        true,
    ),
    enc(H265, "vaapi-h265", "VA-API", "vah265enc", true),
    enc(
        H265,
        "vaapi-old-h265",
        "VA-API (older plug-in)",
        "vaapih265enc",
        true,
    ),
];
const H265_AMF: &[Encoder] = &[enc(H265, "amf-h265", "AMD AMF", "amfh265enc", true)];
const H265_VT: &[Encoder] = &[enc(
    H265,
    "videotoolbox-h265",
    "Apple VideoToolbox",
    "vtenc_h265",
    true,
)];
const H265_MF: &[Encoder] = &[enc(
    H265,
    "mf-h265",
    "Windows Media Foundation",
    "mfh265enc",
    true,
)];
const H265_SOFT: &[Encoder] = &[enc(H265, "x265", "Software (x265)", "x265enc", false)];

const VP9_VAAPI: &[Encoder] = &[
    enc_with(
        Vp9,
        "vaapi-lp-vp9",
        "VA-API (low power)",
        "vavp9lpenc",
        true,
        "bitrate",
        1,
    ),
    enc_with(Vp9, "vaapi-vp9", "VA-API", "vavp9enc", true, "bitrate", 1),
];
const VP9_QSV: &[Encoder] = &[enc(Vp9, "qsv-vp9", "Intel Quick Sync", "qsvvp9enc", true)];
const VP9_SOFT: &[Encoder] = &[enc_with(
    Vp9,
    "libvpx-vp9",
    "Software (libvpx)",
    "vp9enc",
    false,
    "target-bitrate",
    1000,
)];

const AV1_NVENC: &[Encoder] = &[enc(Av1, "nvenc-av1", "NVIDIA NVENC", "nvav1enc", true)];
const AV1_QSV: &[Encoder] = &[enc(Av1, "qsv-av1", "Intel Quick Sync", "qsvav1enc", true)];
const AV1_VAAPI: &[Encoder] = &[
    enc(
        Av1,
        "vaapi-lp-av1",
        "VA-API (low power)",
        "vaav1lpenc",
        true,
    ),
    enc(Av1, "vaapi-av1", "VA-API", "vaav1enc", true),
];
const AV1_AMF: &[Encoder] = &[enc(Av1, "amf-av1", "AMD AMF", "amfav1enc", true)];
const AV1_SOFT: &[Encoder] = &[
    enc_with(
        Av1,
        "svtav1",
        "Software (SVT-AV1)",
        "svtav1enc",
        false,
        "target-bitrate",
        1,
    ),
    enc_with(
        Av1,
        "libaom",
        "Software (libaom)",
        "av1enc",
        false,
        "target-bitrate",
        1,
    ),
];

/// The graphics hardware encoders of one kind for one video format.
fn hardware_encoders(backend: HwBackend, codec: VideoCodec) -> &'static [Encoder] {
    match (codec, backend) {
        (H264, HwBackend::Nvenc) => H264_NVENC,
        (H264, HwBackend::Qsv) => H264_QSV,
        (H264, HwBackend::Vaapi) => H264_VAAPI,
        (H264, HwBackend::Amf) => H264_AMF,
        (H264, HwBackend::VideoToolbox) => H264_VT,
        (H265, HwBackend::Nvenc) => H265_NVENC,
        (H265, HwBackend::Qsv) => H265_QSV,
        (H265, HwBackend::Vaapi) => H265_VAAPI,
        (H265, HwBackend::Amf) => H265_AMF,
        (H265, HwBackend::VideoToolbox) => H265_VT,
        (Vp9, HwBackend::Vaapi) => VP9_VAAPI,
        (Vp9, HwBackend::Qsv) => VP9_QSV,
        (Av1, HwBackend::Nvenc) => AV1_NVENC,
        (Av1, HwBackend::Qsv) => AV1_QSV,
        (Av1, HwBackend::Vaapi) => AV1_VAAPI,
        (Av1, HwBackend::Amf) => AV1_AMF,
        _ => &[],
    }
}

fn software_encoders(codec: VideoCodec) -> &'static [Encoder] {
    match codec {
        H264 => H264_SOFT,
        H265 => H265_SOFT,
        Vp9 => VP9_SOFT,
        Av1 => AV1_SOFT,
    }
}

/// Every encoder worth trying on this system for a video format, best first:
/// the graphics hardware in the order the platform prefers, then software.
pub fn candidates(codec: VideoCodec) -> Vec<Encoder> {
    let os = Os::current();
    let mut out: Vec<Encoder> = Vec::new();
    let mut add = |list: &[Encoder]| {
        for e in list {
            if !out.contains(e) {
                out.push(*e);
            }
        }
    };
    for backend in os.default_backend_order() {
        add(match backend {
            HwBackend::Software => software_encoders(codec),
            other => hardware_encoders(*other, codec),
        });
    }
    if os == Os::Windows {
        add(match codec {
            H264 => H264_MF,
            H265 => H265_MF,
            _ => &[],
        });
    }
    add(software_encoders(codec));
    out
}

fn has_element(name: &str) -> bool {
    gst::ElementFactory::find(name).is_some()
}

/// Set a number property if the element has it.
fn set_number(element: &gst::Element, name: &str, value: u32) {
    if element.find_property(name).is_some() {
        element.set_property_from_str(name, &value.to_string());
    }
}

/// Set a text/enum property if the element has it.
fn set_text(element: &gst::Element, name: &str, value: &str) {
    if element.find_property(name).is_some() {
        element.set_property_from_str(name, value);
    }
}

fn configure_encoder(element: &gst::Element, encoder: &Encoder, kbps: u32, fps: FrameRate) {
    set_number(element, encoder.bitrate_prop, kbps * encoder.bitrate_unit);
    let gop = (fps.as_f64() * 2.0).round().max(1.0) as u32;
    set_number(element, "key-int-max", gop);
    set_number(element, "gop-size", gop);
    set_number(element, "keyframe-max-dist", gop);
    set_number(element, "intra-period-length", gop);
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get().min(8)) as u32;
    match encoder.element {
        "x264enc" => {
            set_text(element, "speed-preset", "veryfast");
            // Widest compatibility (phones, social sites): no B-frame tricks needed.
            set_number(element, "bframes", 2);
        }
        "x265enc" => set_text(element, "speed-preset", "veryfast"),
        "vp9enc" => {
            // Good quality (not the realtime setting), at a speed that suits a
            // phone-length video.
            set_number(element, "deadline", 1_000_000);
            set_number(element, "cpu-used", 4);
            set_number(element, "threads", threads);
            set_text(element, "row-mt", "true");
            set_number(element, "lag-in-frames", 8);
        }
        "svtav1enc" => set_number(element, "preset", 10),
        "av1enc" => {
            set_number(element, "cpu-used", 6);
            set_number(element, "threads", threads);
            set_text(element, "row-mt", "true");
            set_number(element, "lag-in-frames", 8);
        }
        _ => {}
    }
}

/// How many threads picture conversion may use (colour conversion of a
/// 1080x1920 picture is a real share of an export's time).
fn convert_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 4)
}

/// The parser that follows an encoder of this video format.
fn video_parser(codec: VideoCodec) -> &'static str {
    match codec {
        H264 => "h264parse",
        H265 => "h265parse",
        Vp9 => "vp9parse",
        Av1 => "av1parse",
    }
}

/// The video half of the pipeline, from raw pictures to the encoded format.
fn video_chain(encoder: &Encoder) -> String {
    format!(
        "videoconvert n-threads={} ! capsfilter caps=\"video/x-raw,colorimetry=bt709\" ! {} name=venc ! {}",
        convert_threads(),
        encoder.element,
        video_parser(encoder.codec)
    )
}

/// Does this encoder really work on this machine? Encodes a few test frames.
fn probe(encoder: Encoder) -> bool {
    if !has_element(encoder.element) || !has_element(video_parser(encoder.codec)) {
        return false;
    }
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let description = format!(
            "videotestsrc num-buffers=10 ! video/x-raw,format=RGBA,width=640,height=360,framerate=30/1 ! {} ! fakesink",
            video_chain(&encoder)
        );
        let ok = (|| {
            let pipeline = gst::parse::launch(&description).ok()?;
            if let Some(element) = pipeline
                .downcast_ref::<gst::Bin>()
                .and_then(|b| b.by_name("venc"))
            {
                configure_encoder(&element, &encoder, 2000, FrameRate::FPS_30);
            }
            pipeline.set_state(gst::State::Playing).ok()?;
            let bus = pipeline.bus()?;
            let message = bus.timed_pop_filtered(
                gst::ClockTime::from_seconds(10),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            );
            let _ = pipeline.set_state(gst::State::Null);
            Some(matches!(
                message.as_ref().map(|m| m.view()),
                Some(gst::MessageView::Eos(_))
            ))
        })();
        let _ = send.send(ok.unwrap_or(false));
    });
    receive
        .recv_timeout(Duration::from_secs(15))
        .unwrap_or(false)
}

static WORKING: [OnceLock<Vec<Encoder>>; 4] = [
    OnceLock::new(),
    OnceLock::new(),
    OnceLock::new(),
    OnceLock::new(),
];

impl Encoder {
    /// The kind of encoder this is, as kept in settings: "software", or the
    /// hardware maker ("vaapi", "nvenc", ...). Variants of one maker share it.
    pub fn family(&self) -> &'static str {
        if !self.hardware {
            return "software";
        }
        for key in ["vaapi", "nvenc", "qsv", "amf", "videotoolbox", "mf"] {
            if self.id.starts_with(key) {
                return key;
            }
        }
        "hardware"
    }
}

/// What the Encoder list calls one kind of encoder.
pub fn family_label(family: &str) -> &'static str {
    match family {
        "software" => "Processor",
        "vaapi" => "Graphics chip (VA-API)",
        "nvenc" => "Graphics chip (NVIDIA NVENC)",
        "qsv" => "Graphics chip (Intel Quick Sync)",
        "amf" => "Graphics chip (AMD AMF)",
        "videotoolbox" => "Graphics chip (Apple VideoToolbox)",
        "mf" => "Graphics chip (Windows Media Foundation)",
        _ => "Graphics chip",
    }
}

/// Every encoder that works on this machine, for every video format. Slow the
/// first time (each is test-encoded), so call it from a worker thread.
pub fn all_working_encoders() -> Vec<Encoder> {
    VideoCodec::ALL
        .iter()
        .flat_map(|c| working_encoders(*c).iter().copied())
        .collect()
}

/// The encoders for one video format that work on this machine, best first.
/// The first call for each format tests them (a second or two), so call it
/// from a worker thread.
pub fn working_encoders(codec: VideoCodec) -> &'static [Encoder] {
    let slot = VideoCodec::ALL
        .iter()
        .position(|c| *c == codec)
        .unwrap_or(0);
    WORKING[slot].get_or_init(|| {
        if gst::init().is_err() {
            return Vec::new();
        }
        candidates(codec)
            .into_iter()
            .filter(|e| {
                let t = std::time::Instant::now();
                let ok = probe(*e);
                eprintln!(
                    "kani: tested {} encoder {}: {} ({:.1} s)",
                    codec.name(),
                    e.label,
                    if ok {
                        "works".to_string()
                    } else if !has_element(e.element) {
                        format!("not usable - GStreamer has no '{}' plug-in", e.element)
                    } else {
                        format!(
                            "not usable - '{}' is installed but failed a test encode",
                            e.element
                        )
                    },
                    t.elapsed().as_secs_f64()
                );
                ok
            })
            .collect()
    })
}

fn audio_encoder() -> Option<&'static str> {
    ["avenc_aac", "voaacenc", "fdkaacenc", "faac"]
        .into_iter()
        .find(|name| has_element(name))
}

// ------------------------------------------------------------------ sound

/// Can this machine make sound in this format?
pub fn audio_available(format: AudioFormat) -> bool {
    match format {
        AudioFormat::Aac => audio_encoder().is_some(),
        AudioFormat::Mp3 => has_element("lamemp3enc"),
        AudioFormat::Opus => has_element("opusenc"),
        AudioFormat::Flac => has_element("flacenc"),
        AudioFormat::Wav => true,
    }
}

/// Can this machine write this file format?
pub fn container_available(container: Container) -> bool {
    has_element(container.muxer())
}

/// The sound half of a pipeline, from the mixed stereo sound to `format`.
/// `in_video` is for sound inside a video file (a parser the muxer wants,
/// raw sound for WAV); otherwise it is for a file of its own. The encoder is
/// named `aenc` (WAV has none).
fn audio_chain(format: AudioFormat, channels: Channels, in_video: bool) -> Option<String> {
    let n = channels.count();
    let convert = "audioconvert ! audioresample";
    Some(match format {
        AudioFormat::Aac => format!(
            "{convert} ! capsfilter caps=\"audio/x-raw,channels={n}\" ! {} name=aenc ! aacparse",
            audio_encoder()?
        ),
        AudioFormat::Mp3 => format!(
            "{convert} ! capsfilter caps=\"audio/x-raw,channels={n}\" ! lamemp3enc name=aenc{}",
            if in_video { " ! mpegaudioparse" } else { "" }
        ),
        AudioFormat::Opus => format!(
            "{convert} ! capsfilter caps=\"audio/x-raw,rate=48000,channels={n}\" ! opusenc name=aenc{}",
            if in_video { " ! opusparse" } else { "" }
        ),
        AudioFormat::Flac => format!(
            "{convert} ! capsfilter caps=\"audio/x-raw,channels={n}\" ! flacenc name=aenc{}",
            if in_video { " ! flacparse" } else { "" }
        ),
        AudioFormat::Wav => format!(
            "{convert} ! capsfilter caps=\"audio/x-raw,format=S16LE,channels={n}\"{}",
            if in_video { "" } else { " ! wavenc" }
        ),
    })
}

/// Set the bit rate on the sound encoder `aenc`, if the format has one.
fn configure_audio(element: &gst::Element, format: AudioFormat, kbps: u32) {
    match format {
        AudioFormat::Aac | AudioFormat::Opus => set_number(element, "bitrate", kbps * 1000),
        AudioFormat::Mp3 => {
            set_text(element, "target", "bitrate");
            set_text(element, "cbr", "true");
            set_number(element, "bitrate", kbps);
        }
        AudioFormat::Flac | AudioFormat::Wav => {}
    }
}

// ---------------------------------------------------------------- progress

/// Shared between the export thread and the window showing it.
#[derive(Default)]
pub struct Progress {
    pub frames_done: AtomicU64,
    pub frames_total: AtomicU64,
    pub cancel: AtomicBool,
    /// Sound made so far, and in all, in 10 ms blocks (for the sound file).
    pub audio_done: AtomicU64,
    pub audio_total: AtomicU64,
    /// Set while the sound file is being made.
    pub audio_pass: AtomicBool,
    /// Set when there is no video, only a sound file.
    pub audio_only: AtomicBool,
    /// The newest picture handed to the encoder, small, for the window to show
    /// while the export runs (taken by the window, so each is shown once).
    pub live: Mutex<Option<LivePicture>>,
    /// When the picture above was last updated (so it is not made for every frame).
    live_at: Mutex<Option<std::time::Instant>>,
}

/// A small copy of a picture being exported (RGBA).
pub struct LivePicture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Which frame it is (counting from 1).
    pub frame: u64,
}

impl Progress {
    /// Offer a finished picture to the window. Only about eight a second are
    /// kept (the rest cost time for nothing), but `force` always keeps one.
    pub fn show(&self, width: u32, height: u32, pixels: &[u8], frame: u64, force: bool) {
        const LONG_EDGE: u32 = 360;
        {
            let mut at = self.live_at.lock().unwrap();
            let due = at.is_none_or(|t| t.elapsed() >= Duration::from_millis(120));
            if !(due || force) {
                return;
            }
            *at = Some(std::time::Instant::now());
        }
        let (w, h) = (width as usize, height as usize);
        if w == 0 || h == 0 || pixels.len() < w * h * 4 {
            return;
        }
        let shrink = f64::from(LONG_EDGE) / w.max(h) as f64;
        let (ow, oh) = if shrink < 1.0 {
            (
                ((w as f64 * shrink).round() as usize).max(1),
                ((h as f64 * shrink).round() as usize).max(1),
            )
        } else {
            (w, h)
        };
        let mut rgba = Vec::with_capacity(ow * oh * 4);
        for y in 0..oh {
            let sy = y * h / oh;
            for x in 0..ow {
                let o = (sy * w + x * w / ow) * 4;
                rgba.extend_from_slice(&pixels[o..o + 4]);
            }
        }
        *self.live.lock().unwrap() = Some(LivePicture {
            width: ow as u32,
            height: oh as u32,
            rgba,
            frame,
        });
    }

    pub fn fraction(&self) -> f64 {
        let (done, total) = if self.audio_only.load(Ordering::Relaxed) {
            (
                self.audio_done.load(Ordering::Relaxed),
                self.audio_total.load(Ordering::Relaxed),
            )
        } else {
            (
                self.frames_done.load(Ordering::Relaxed),
                self.frames_total.load(Ordering::Relaxed),
            )
        };
        if total == 0 {
            return 0.0;
        }
        (done as f64 / total as f64).min(1.0)
    }
}

#[derive(Debug)]
pub enum ExportError {
    Cancelled,
    Failed(String),
}

impl From<String> for ExportError {
    fn from(message: String) -> Self {
        ExportError::Failed(message)
    }
}

impl From<&str> for ExportError {
    fn from(message: &str) -> Self {
        ExportError::Failed(message.to_string())
    }
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::Cancelled => write!(f, "Export cancelled."),
            ExportError::Failed(m) => write!(f, "{m}"),
        }
    }
}

type Result<T> = std::result::Result<T, ExportError>;

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(ExportError::Failed(message.into()))
}

// -------------------------------------------------------------------- job

/// A media file as the exporter needs to know it.
#[derive(Debug, Clone)]
pub struct Source {
    pub path: PathBuf,
    pub kind: MediaKind,
    pub has_audio: bool,
}

/// Everything an export needs, copied out of the project so it can run on its own thread.
#[derive(Clone)]
pub struct Job {
    pub timeline: Timeline,
    pub sources: HashMap<MediaId, Source>,
    /// The size of the exported picture.
    pub width: u32,
    pub height: u32,
    /// The size the project's picture is put together at; it sits in the middle
    /// of the exported picture (black bars around it if smaller, cropped if
    /// bigger). The same as `width` x `height` when the shapes match.
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub fps: FrameRate,
    pub sample_rate: u32,
    pub video_kbps: u32,
    /// Sound: the format (in the video file and in the sound file), bit rate and channels.
    pub audio_format: AudioFormat,
    pub audio_kbps: u32,
    pub channels: Channels,
    /// The video file's format.
    pub container: Container,
    /// Try these, in order, until one works.
    pub encoders: Vec<Encoder>,
    /// The video file (with its sound), or `None` for no video.
    pub output: Option<PathBuf>,
    /// A file of the project's sound alone, or `None` for none.
    pub audio_output: Option<PathBuf>,
    /// Where on the timeline the export begins (zero for the whole project), and
    /// where it ends (`None` = the end of the timeline).
    pub range_start: Ticks,
    pub range_end: Option<Ticks>,
    /// Export only this many seconds from the start (used by tests; `None` = all of it).
    pub limit_seconds: Option<f64>,
    /// Put video layers together on the graphics card (falls back to the
    /// processor when that is not possible).
    pub gpu: bool,
}

impl Job {
    /// How much of the timeline is exported.
    fn duration(&self) -> Ticks {
        let end = self.range_end.map_or(self.timeline.duration(), |e| {
            e.min(self.timeline.duration())
        });
        let all = if end > self.range_start {
            end - self.range_start
        } else {
            Ticks::ZERO
        };
        match self.limit_seconds {
            Some(limit) => all.min(Ticks::from_seconds_f64(limit)),
            None => all,
        }
    }
}

/// What an export did, besides saving the file.
#[derive(Debug, Default, Clone)]
pub struct Report {
    pub encoder: String,
    pub frames: u64,
    /// Things that did not go perfectly (a clip that could not be read ...).
    pub warnings: Vec<String>,
    /// Real time the export took, start to finish.
    pub elapsed: Duration,
    /// Where that time went: waiting for decoded video, compositing, handing
    /// pictures to the encoder.
    pub decode: Duration,
    pub compose: Duration,
    pub handoff: Duration,
    /// Layer pictures left out because a solid layer covered them.
    pub hidden_skipped: u64,
    /// Video pictures that GStreamer was asked to shrink while decoding.
    pub shrunk_frames: u64,
    /// Layer pictures put together on the graphics card.
    pub gpu_layers: u64,
    /// Whole pictures put together on the card, and ones it handed to the processor.
    pub gpu_frames: u64,
    pub gpu_skipped_frames: u64,
    /// Set when the card was asked for but could not be used.
    pub gpu_problem: Option<String>,
    /// The sound-only file was saved.
    pub audio_saved: bool,
}

// -------------------------------------------------------- reading pictures

/// A line about the state of the machine, printed at the start of every export,
/// so renders that took different times can be told apart (power saving,
/// battery, other work running).
fn machine_state() -> String {
    let read = |path: &str| {
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim().to_string())
    };
    let threads = std::thread::available_parallelism().map_or(0, |n| n.get());
    let governor = read("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor");
    let preference = read("/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference");
    let mhz = read("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq")
        .and_then(|v| v.parse::<f64>().ok())
        .map(|khz| khz / 1000.0);
    let load =
        read("/proc/loadavg").map(|l| l.split_whitespace().take(3).collect::<Vec<_>>().join(" "));
    let battery = std::fs::read_dir("/sys/class/power_supply")
        .ok()
        .and_then(|dir| {
            dir.flatten().find_map(|entry| {
                let path = entry.path();
                let kind = std::fs::read_to_string(path.join("type")).ok()?;
                (kind.trim() == "Mains")
                    .then(|| std::fs::read_to_string(path.join("online")).ok())
                    .flatten()
                    .map(|v| v.trim() == "1")
            })
        });
    format!(
        "{threads} threads, governor {}, power preference {}, core 0 at {} MHz, load {}, mains power {}",
        governor.as_deref().unwrap_or("unknown"),
        preference.as_deref().unwrap_or("unknown"),
        mhz.map_or("unknown".to_string(), |m| format!("{m:.0}")),
        load.as_deref().unwrap_or("unknown"),
        battery.map_or("unknown", |on| if on { "yes" } else { "NO (on battery)" })
    )
}

fn seconds_of(ts: Option<gst::ClockTime>) -> f64 {
    ts.map_or(0.0, |t| t.nseconds() as f64 / 1e9)
}

fn wait_ready(pipeline: &gst::Element) -> Result<()> {
    match pipeline.state(gst::ClockTime::from_seconds(20)).0 {
        Ok(_) => Ok(()),
        Err(_) => fail("a media file could not be opened"),
    }
}

/// Reads the pictures of one video file, in order, from one moment on.
struct VideoReader {
    pipeline: gst::Element,
    sink: gst_app::AppSink,
    current: Option<(f64, Arc<Frame>)>,
    next: Option<(f64, Arc<Frame>)>,
    ended: bool,
    /// The size limit the pictures are being scaled to inside GStreamer.
    filter: gst::Element,
    edge: u32,
    /// Size of the pictures before any scaling by us.
    native: Option<(u32, u32)>,
}

fn reader_caps(edge: u32) -> String {
    format!("video/x-raw,format=RGBA,width=[1,{edge}],height=[1,{edge}],pixel-aspect-ratio=1/1")
}

impl VideoReader {
    fn open(uri: &str, from: f64, max_edge: u32) -> Result<VideoReader> {
        let playbin = gst::ElementFactory::make("playbin")
            .property("uri", uri)
            .build()
            .map_err(|e| ExportError::Failed(e.to_string()))?;
        let bin = gst::parse::bin_from_description(
            &format!(
                "videoconvert n-threads={t} ! videoscale n-threads={t} ! capsfilter name=limit caps=\"{caps}\" \
                 ! appsink name=exportsink sync=false max-buffers=3 drop=false",
                caps = reader_caps(max_edge),
                t = convert_threads()
            ),
            true,
        )
        .map_err(|e| ExportError::Failed(e.to_string()))?;
        let sink = bin
            .by_name("exportsink")
            .and_then(|e| e.downcast::<gst_app::AppSink>().ok())
            .ok_or_else(|| ExportError::Failed("no picture sink".into()))?;
        let filter = bin
            .by_name("limit")
            .ok_or_else(|| ExportError::Failed("no size filter".into()))?;
        playbin.set_property("video-sink", &bin);
        // Nothing else may be built for the streams we do not read.
        for name in ["audio-sink", "text-sink"] {
            if let Ok(fake) = gst::ElementFactory::make("fakesink").build() {
                playbin.set_property(name, &fake);
            }
        }
        playbin.set_property_from_str("flags", "video");
        playbin
            .set_state(gst::State::Paused)
            .map_err(|_| ExportError::Failed("could not start reading a video".into()))?;
        wait_ready(&playbin)?;
        if from > 0.02 {
            let _ = playbin.seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::ClockTime::from_nseconds((from * 1e9) as u64),
            );
        }
        playbin
            .set_state(gst::State::Playing)
            .map_err(|_| ExportError::Failed("could not start reading a video".into()))?;
        Ok(VideoReader {
            pipeline: playbin,
            sink,
            current: None,
            next: None,
            ended: false,
            filter,
            edge: max_edge,
            native: None,
        })
    }

    /// Ask GStreamer to hand over smaller pictures (or full size again).
    fn set_edge(&mut self, edge: u32) {
        if let Ok(caps) = reader_caps(edge).parse::<gst::Caps>() {
            self.filter.set_property("caps", &caps);
            self.edge = edge;
        }
    }

    /// A layer shown smaller than its picture does not need every pixel: let
    /// GStreamer (which works alongside the compositor) do the shrinking. Only
    /// for flat layers (no slant or perspective), and sized so the result is
    /// drawn as large as before - the placement is relative to the fitted size.
    fn tune(&mut self, frame: &Frame, scale: f32, flat: bool, canvas: (u32, u32)) {
        let full = canvas.0.max(canvas.1);
        if self.native.is_none() {
            if self.edge != full {
                return;
            }
            self.native = Some((frame.width, frame.height));
        }
        let Some((nw, nh)) = self.native else { return };
        let fit = (canvas.0 as f32 / nw as f32).min(canvas.1 as f32 / nh as f32);
        let shown = fit * scale;
        let want = if flat && shown < 0.95 {
            ((nw.max(nh) as f32 * shown).ceil() as u32).clamp(16, full)
        } else {
            full
        };
        if want > self.edge || (want as f32) < self.edge as f32 * 0.9 {
            self.set_edge(want);
        }
    }

    fn pull(&mut self) -> Option<(f64, Arc<Frame>)> {
        let sample = self.sink.try_pull_sample(gst::ClockTime::from_seconds(8))?;
        let pts = seconds_of(sample.buffer().and_then(|b| b.pts()));
        let thumb = sample_to_thumb(&sample)?;
        Some((
            pts,
            Arc::new(Frame {
                width: thumb.width as u32,
                height: thumb.height as u32,
                rgba: thumb.rgba,
            }),
        ))
    }

    /// The picture showing at source time `t` (seconds): the last one that
    /// starts at or before it. Time must not go backwards.
    fn frame_at(&mut self, t: f64) -> Option<Arc<Frame>> {
        loop {
            if self.next.is_none() && !self.ended {
                match self.pull() {
                    Some(found) => self.next = Some(found),
                    None => self.ended = true,
                }
            }
            match &self.next {
                Some((pts, _)) if *pts <= t + 0.0005 => self.current = self.next.take(),
                _ => break,
            }
        }
        if self.current.is_none() {
            self.current = self.next.take();
        }
        self.current.as_ref().map(|(_, f)| f.clone())
    }

    /// Is `t` somewhere this reader can get to without starting over?
    fn can_reach(&self, t: f64) -> bool {
        match &self.current {
            Some((pts, _)) => t >= *pts - 0.001 && t <= *pts + 2.0,
            None => true,
        }
    }
}

impl Drop for VideoReader {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

/// Reads the sound of one file as stereo floats, in order, from one moment on.
struct AudioReader {
    pipeline: gst::Element,
    sink: gst_app::AppSink,
    rate: u32,
    /// Source sample the next unread sample is.
    position: i64,
    buffer: Vec<f32>,
    head: usize,
    ended: bool,
}

impl AudioReader {
    fn open(uri: &str, from_sample: i64, rate: u32) -> Result<AudioReader> {
        let playbin = gst::ElementFactory::make("playbin")
            .property("uri", uri)
            .build()
            .map_err(|e| ExportError::Failed(e.to_string()))?;
        let bin = gst::parse::bin_from_description(
            &format!(
                "audioconvert ! audioresample ! capsfilter caps=\"audio/x-raw,format=F32LE,layout=interleaved,rate={rate},channels=2\" \
                 ! appsink name=exportsink sync=false max-buffers=16 drop=false"
            ),
            true,
        )
        .map_err(|e| ExportError::Failed(e.to_string()))?;
        let sink = bin
            .by_name("exportsink")
            .and_then(|e| e.downcast::<gst_app::AppSink>().ok())
            .ok_or_else(|| ExportError::Failed("no sound sink".into()))?;
        playbin.set_property("audio-sink", &bin);
        for name in ["video-sink", "text-sink"] {
            if let Ok(fake) = gst::ElementFactory::make("fakesink").build() {
                playbin.set_property(name, &fake);
            }
        }
        playbin.set_property_from_str("flags", "audio");
        playbin
            .set_state(gst::State::Paused)
            .map_err(|_| ExportError::Failed("could not start reading sound".into()))?;
        wait_ready(&playbin)?;
        if from_sample > 64 {
            let _ = playbin.seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::ClockTime::from_nseconds((from_sample as f64 / f64::from(rate) * 1e9) as u64),
            );
        }
        playbin
            .set_state(gst::State::Playing)
            .map_err(|_| ExportError::Failed("could not start reading sound".into()))?;
        Ok(AudioReader {
            pipeline: playbin,
            sink,
            rate,
            position: from_sample.max(0),
            buffer: Vec::new(),
            head: 0,
            ended: false,
        })
    }

    /// The next `frames` stereo samples (interleaved); silence once the file ends.
    fn read(&mut self, frames: usize) -> Vec<f32> {
        let want = frames * 2;
        while self.buffer.len() - self.head < want && !self.ended {
            match self.sink.try_pull_sample(gst::ClockTime::from_seconds(8)) {
                Some(sample) => {
                    if let Some(map) = sample.buffer().and_then(|b| b.map_readable().ok()) {
                        if self.head > 0 && self.head == self.buffer.len() {
                            self.buffer.clear();
                            self.head = 0;
                        }
                        self.buffer.extend(
                            map.chunks_exact(4)
                                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                        );
                    }
                }
                None => self.ended = true,
            }
        }
        let have = (self.buffer.len() - self.head).min(want);
        let mut out = self.buffer[self.head..self.head + have].to_vec();
        self.head += have;
        out.resize(want, 0.0);
        if self.head > 1 << 16 {
            self.buffer.drain(..self.head);
            self.head = 0;
        }
        self.position += frames as i64;
        out
    }

    fn can_reach(&self, sample: i64) -> bool {
        // Within about 40 ms of where this reader is.
        (sample - self.position).abs() <= i64::from(self.rate) / 25
    }
}

impl Drop for AudioReader {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

// ---------------------------------------------------------------- render

struct Renderer<'a> {
    job: &'a Job,
    readers: HashMap<ClipId, VideoReader>,
    stills: Vec<(PathBuf, Arc<Frame>)>,
    texts: Vec<(cutroom_timeline::TextSpec, Arc<Frame>)>,
    sounds: HashMap<ClipId, AudioReader>,
    silent_uris: HashSet<String>,
    broken: HashSet<ClipId>,
    warnings: Vec<String>,
    decode_time: Duration,
    compose_time: Duration,
    layer_kinds: Vec<(u32, u32, bool)>,
    /// Layers not drawn (or decoded) because something solid covered them.
    hidden_skipped: u64,
    /// Pictures the readers were asked to shrink while decoding.
    shrunk_frames: u64,
    /// The graphics card compositor, once started (`None` until needed).
    gpu: Option<crate::gpu_compose::GpuCompositor>,
    /// The card was asked for and could not be used; why.
    gpu_problem: Option<String>,
    /// Layer pictures put together on the card.
    gpu_layers: u64,
    /// Whole pictures put together on the card, and ones the card passed to the processor.
    gpu_frames: u64,
    gpu_skipped_frames: u64,
    /// Finished pictures not yet handed to the encoder, oldest first. With the
    /// card on, the newest ones are still on their way back from it.
    ready: VecDeque<Vec<u8>>,
    /// Time spent making the empty (black) canvas for each picture.
    canvas_time: Duration,
    /// Stills and text that stay where they are from frame to frame, drawn once.
    static_layers: HashMap<ClipId, StaticLayer>,
    /// Compositing time per kind of layer: (kind, width, height, scale in
    /// thousandths, cached) -> (time, frames).
    layer_time: HashMap<(&'static str, u32, u32, i32, bool, u8), (Duration, u64)>,
}

/// A still or text layer that has been seen in the same place on consecutive frames.
struct StaticLayer {
    frame: Arc<Frame>,
    placement: Placement,
    seen: u32,
    layer: Option<Arc<Layer>>,
}

/// What `picture` lays down, bottom first.
enum Item {
    Draw(Arc<Frame>, Placement),
    Cached(Arc<Layer>),
}

impl<'a> Renderer<'a> {
    fn new(job: &'a Job) -> Renderer<'a> {
        let silent_uris = job
            .sources
            .values()
            .filter(|s| !s.has_audio || s.kind == MediaKind::Image)
            .map(|s| path_to_uri(&s.path))
            .collect();
        Renderer {
            job,
            readers: HashMap::new(),
            stills: Vec::new(),
            texts: Vec::new(),
            sounds: HashMap::new(),
            silent_uris,
            broken: HashSet::new(),
            warnings: Vec::new(),
            decode_time: Duration::ZERO,
            compose_time: Duration::ZERO,
            layer_kinds: Vec::new(),
            hidden_skipped: 0,
            shrunk_frames: 0,
            gpu: None,
            gpu_problem: None,
            gpu_layers: 0,
            gpu_frames: 0,
            gpu_skipped_frames: 0,
            ready: VecDeque::new(),
            canvas_time: Duration::ZERO,
            static_layers: HashMap::new(),
            layer_time: HashMap::new(),
        }
    }

    fn resolve(&self) -> impl Fn(MediaId) -> Option<(PathBuf, MediaKind)> + '_ {
        move |id| self.job.sources.get(&id).map(|s| (s.path.clone(), s.kind))
    }

    fn warn(&mut self, clip: ClipId, message: String) {
        if self.broken.insert(clip) {
            self.warnings.push(message);
        }
    }

    fn still(&mut self, path: &Path) -> Option<Arc<Frame>> {
        if let Some(i) = self.stills.iter().position(|(p, _)| p == path) {
            return Some(self.stills[i].1.clone());
        }
        match crate::preview::load_still_frame(path) {
            Ok(frame) => {
                let frame = Arc::new(frame);
                self.stills.push((path.to_path_buf(), frame.clone()));
                if self.stills.len() > 6 {
                    self.stills.remove(0);
                }
                Some(frame)
            }
            Err(error) => {
                self.warnings
                    .push(format!("Could not read {}: {error}", path.display()));
                None
            }
        }
    }

    fn text(&mut self, spec: &cutroom_timeline::TextSpec) -> Arc<Frame> {
        if let Some((_, frame)) = self.texts.iter().find(|(s, _)| s == spec) {
            return frame.clone();
        }
        let frame = Arc::new(crate::text_render::render(
            spec,
            self.job.canvas_width,
            self.job.canvas_height,
        ));
        self.texts.push((spec.clone(), frame.clone()));
        if self.texts.len() > 8 {
            self.texts.remove(0);
        }
        frame
    }

    /// Put together the picture at timeline time `at`. It joins `ready` now, or
    /// (with the card on) a couple of pictures later, always in order.
    fn picture(&mut self, at: Ticks) -> Result<()> {
        let (width, height) = (self.job.canvas_width, self.job.canvas_height);
        let targets = {
            let resolve = self.resolve();
            find_targets(&self.job.timeline, at, &resolve)
        };
        let mut picked: Vec<(Item, (&'static str, u32, u32, i32, bool, u8))> = Vec::new();
        let mut used: HashSet<ClipId> = HashSet::new();
        // Topmost first: once something solid covers the canvas, everything
        // under it is left out (and never decoded).
        let count = targets.layers.len();
        for (index, layer) in targets.layers.iter().enumerate() {
            used.insert(layer.clip);
            let (frame, natural, still_or_text) = match &layer.source {
                LayerSource::Text(spec) => (Some(self.text(spec)), true, true),
                LayerSource::Still(path) => (self.still(path), false, true),
                LayerSource::Video(target) => {
                    let t = layer.transform.clamped();
                    let flat = t.skew_x == 0
                        && t.skew_y == 0
                        && t.persp_x == 0
                        && t.persp_y == 0
                        && t.warp.is_identity();
                    (
                        self.video_frame(layer.clip, target, t.scale as f32 / 1000.0, flat),
                        false,
                        false,
                    )
                }
            };
            let Some(frame) = frame else { continue };
            let seen = (frame.width, frame.height, natural);
            if !self.layer_kinds.contains(&seen) {
                self.layer_kinds.push(seen);
                let t = layer.transform.clamped();
                eprintln!(
                    "kani: export layer: {} picture {}x{} on a {}x{} canvas (scale {:.3}, position {:.3},{:.3})",
                    if natural { "text" } else { "video/image" },
                    frame.width,
                    frame.height,
                    width,
                    height,
                    t.scale,
                    t.x,
                    t.y
                );
            }
            let placement = Placement {
                natural,
                ..Placement::from(&layer.transform.clamped())
            };
            let kind = if natural {
                "text"
            } else if still_or_text {
                "still"
            } else {
                "video"
            };
            let t = layer.transform.clamped();
            let milli = t.scale;
            // What else is special about this layer (shown in the timing table).
            let flags = u8::from(t.rotation != 0)
                | u8::from(
                    t.skew_x != 0
                        || t.skew_y != 0
                        || t.persp_x != 0
                        || t.persp_y != 0
                        || !t.warp.is_identity(),
                ) << 1
                | u8::from(t.opacity < 1000) << 2
                | u8::from(t.flip_h || t.flip_v) << 3;
            let mut cached = None;
            if still_or_text {
                // The same picture in the same place as on the last frame: draw
                // it once and lay that down from now on.
                let entry = self
                    .static_layers
                    .entry(layer.clip)
                    .or_insert_with(|| StaticLayer {
                        frame: frame.clone(),
                        placement,
                        seen: 0,
                        layer: None,
                    });
                if !Arc::ptr_eq(&entry.frame, &frame) || entry.placement != placement {
                    *entry = StaticLayer {
                        frame: frame.clone(),
                        placement,
                        seen: 0,
                        layer: None,
                    };
                }
                entry.seen += 1;
                if entry.layer.is_none() && entry.seen >= 2 {
                    entry.layer = render_layer(width, height, &frame, &placement).map(Arc::new);
                }
                cached = entry.layer.clone();
            }
            picked.push(match cached {
                Some(layer) => (
                    Item::Cached(layer),
                    (kind, frame.width, frame.height, milli, true, flags),
                ),
                None => (
                    Item::Draw(frame.clone(), placement),
                    (kind, frame.width, frame.height, milli, false, flags),
                ),
            });
            if kind == "video"
                && covers_canvas(width, height, frame.width, frame.height, &placement)
            {
                self.hidden_skipped += (count - index - 1) as u64;
                break;
            }
        }
        picked.reverse();
        // Free the readers of clips that are no longer showing.
        self.readers.retain(|clip, _| used.contains(clip));
        self.static_layers.retain(|clip, _| used.contains(clip));
        let started = std::time::Instant::now();
        if self.job.gpu && self.gpu_problem.is_none() {
            let started = std::time::Instant::now();
            let done = self.picture_on_card(&picked)?;
            self.compose_time += started.elapsed();
            if done {
                return Ok(());
            }
        }
        // The processor does this one; the ones still on their way back from the
        // card come first.
        self.drain_card()?;
        let canvas_started = std::time::Instant::now();
        let mut canvas = Canvas::new(width, height);
        self.canvas_time += canvas_started.elapsed();
        for (item, key) in &picked {
            let t = std::time::Instant::now();
            match item {
                Item::Draw(frame, placement) => canvas.draw(frame, placement),
                Item::Cached(layer) => canvas.blit(layer),
            }
            let entry = self.layer_time.entry(*key).or_default();
            entry.0 += t.elapsed();
            entry.1 += 1;
        }
        self.compose_time += started.elapsed();
        self.ready.push_back(canvas.rgba);
        Ok(())
    }

    /// Take every picture still on its way back from the card.
    fn drain_card(&mut self) -> Result<()> {
        let Some(card) = self.gpu.as_mut() else {
            return Ok(());
        };
        let started = std::time::Instant::now();
        let mut taken = Vec::new();
        let mut lost = None;
        while card.pictures_waiting() > 0 {
            match card.take_picture() {
                Ok(pixels) => taken.push(pixels),
                Err(why) => {
                    lost = Some(why);
                    break;
                }
            }
        }
        self.ready.extend(taken);
        self.record_read_back(started.elapsed());
        match lost {
            Some(why) => fail(format!("the graphics card lost a picture: {why}")),
            None => Ok(()),
        }
    }

    /// Everything left in the pipeline: call after the last picture.
    fn flush(&mut self) -> Result<()> {
        let started = std::time::Instant::now();
        self.drain_card()?;
        self.compose_time += started.elapsed();
        Ok(())
    }

    fn record_read_back(&mut self, time: Duration) {
        let key = (
            "read-back from the card",
            self.job.canvas_width,
            self.job.canvas_height,
            0,
            false,
            0,
        );
        let entry = self.layer_time.entry(key).or_default();
        entry.0 += time;
        entry.1 += 1;
    }

    /// The whole picture put together on the graphics card: the canvas stays
    /// there while every layer is drawn over it, and it comes back once. `None`
    /// when the card cannot do this picture (the processor does it).
    fn picture_on_card(
        &mut self,
        picked: &[(Item, (&'static str, u32, u32, i32, bool, u8))],
    ) -> Result<bool> {
        let (width, height) = (self.job.canvas_width, self.job.canvas_height);
        if self.gpu.is_none() {
            match crate::gpu_compose::GpuCompositor::new(width, height) {
                Ok(card) => {
                    eprintln!(
                        "kani: compositing whole pictures on the graphics card ({})",
                        card.renderer
                    );
                    self.gpu = Some(card);
                }
                Err(why) => {
                    eprintln!("kani: graphics card compositing could not start: {why}; using the processor");
                    self.gpu_problem = Some(why);
                    return Ok(false);
                }
            }
        }
        let Some(card) = self.gpu.as_mut() else {
            return Ok(false);
        };
        card.begin_picture();
        let mut outcome: std::result::Result<(), String> = Ok(());
        for (item, key) in picked {
            let started = std::time::Instant::now();
            let drawn = match item {
                Item::Draw(frame, placement) => {
                    if is_plain_copy(width, height, frame.width, frame.height, placement) {
                        let (left, top) =
                            plain_copy_origin(width, height, frame.width, frame.height, placement);
                        card.draw_pixels(frame.clone(), frame, left, top);
                        Ok(())
                    } else {
                        card.draw(frame, placement)
                    }
                }
                Item::Cached(layer) => {
                    card.draw_pixels(layer.clone(), &layer.frame, layer.left, layer.top);
                    Ok(())
                }
            };
            let entry = self.layer_time.entry(*key).or_default();
            entry.0 += started.elapsed();
            entry.1 += 1;
            if let Err(why) = drawn {
                outcome = Err(why);
                break;
            }
        }
        // Ask for the picture and carry on; the one before comes back while the
        // card works on this one.
        let started = std::time::Instant::now();
        let asked = outcome.and_then(|()| card.start_picture_read());
        let mut taken = Vec::new();
        let mut late = Ok(());
        if asked.is_ok() {
            while card.pictures_waiting() > 2 {
                match card.take_picture() {
                    Ok(pixels) => taken.push(pixels),
                    Err(why) => {
                        late = Err(why);
                        break;
                    }
                }
            }
        }
        let spent = started.elapsed();
        self.ready.extend(taken);
        self.record_read_back(spent);
        if let Err(why) = late {
            return fail(format!("the graphics card lost a picture: {why}"));
        }
        match asked {
            Ok(()) => {
                self.gpu_frames += 1;
                self.gpu_layers += picked.len() as u64;
                Ok(true)
            }
            Err(why) if why.starts_with(crate::gpu_compose::UNSUPPORTED) => {
                // One picture the card path cannot place: the processor does it.
                self.gpu_skipped_frames += 1;
                Ok(false)
            }
            Err(why) => {
                eprintln!("kani: graphics card compositing failed: {why}; using the processor from here on");
                self.gpu_problem = Some(why);
                // Pictures already on their way come first, then the card goes.
                self.drain_card()?;
                self.gpu = None;
                Ok(false)
            }
        }
    }

    /// The kinds of layer that cost the most compositing time, as text.
    fn slowest_layers(&self) -> String {
        let mut rows: Vec<_> = self.layer_time.iter().collect();
        rows.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
        let mut lines: Vec<String> = rows
            .iter()
            .take(8)
            .map(|((kind, w, h, milli, cached, flags), (time, frames))| {
                let mut notes = String::new();
                for (bit, name) in [
                    (1, "turned"),
                    (2, "skewed/perspective"),
                    (4, "see-through"),
                    (8, "mirrored"),
                ] {
                    if flags & bit != 0 {
                        notes.push_str(&format!(", {name}"));
                    }
                }
                format!(
                    "  {kind} {w}x{h} at scale {:.3}{notes}{}: {:.1} s over {frames} frames ({:.1} ms each)",
                    f64::from(*milli) / 1000.0,
                    if *cached { " (drawn once)" } else { "" },
                    time.as_secs_f64(),
                    time.as_secs_f64() * 1000.0 / (*frames).max(1) as f64
                )
            })
            .collect();
        let rest: Duration = rows.iter().skip(8).map(|(_, (t, _))| *t).sum();
        let shown: Duration = rows.iter().take(8).map(|(_, (t, _))| *t).sum();
        lines.push(format!(
            "  {} other kinds of layer: {:.1} s; the eight above: {:.1} s; making the empty canvas: {:.1} s",
            rows.len().saturating_sub(8),
            rest.as_secs_f64(),
            shown.as_secs_f64(),
            self.canvas_time.as_secs_f64()
        ));
        lines.join("\n")
    }

    fn video_frame(
        &mut self,
        clip: ClipId,
        target: &crate::preview::Target,
        scale: f32,
        flat: bool,
    ) -> Option<Arc<Frame>> {
        if self.broken.contains(&clip) {
            return None;
        }
        let t = target.source_time.as_seconds_f64();
        if self.readers.get(&clip).is_some_and(|r| !r.can_reach(t)) {
            self.readers.remove(&clip);
        }
        if !self.readers.contains_key(&clip) {
            let edge = self.job.canvas_width.max(self.job.canvas_height);
            match VideoReader::open(&target.uri, t, edge) {
                Ok(reader) => {
                    self.readers.insert(clip, reader);
                }
                Err(error) => {
                    self.warn(clip, format!("A video could not be read ({error})."));
                    return None;
                }
            }
        }
        let started = std::time::Instant::now();
        let canvas = (self.job.canvas_width, self.job.canvas_height);
        let reader = self.readers.get_mut(&clip)?;
        let frame = reader.frame_at(t);
        if let Some(f) = &frame {
            reader.tune(f, scale, flat, canvas);
            if reader.edge < canvas.0.max(canvas.1) {
                self.shrunk_frames += 1;
            }
        }
        self.decode_time += started.elapsed();
        if frame.is_none() {
            self.warn(clip, "A video gave no pictures.".into());
        }
        frame
    }

    /// `frames` stereo samples of sound starting at timeline time `start` seconds.
    fn sound(&mut self, start: f64, frames: usize) -> Vec<f32> {
        let rate = f64::from(self.job.sample_rate);
        let half = frames as f64 / 2.0 / rate;
        let at = Ticks::from_seconds_f64(start + half);
        let targets = {
            let resolve = self.resolve();
            find_targets(&self.job.timeline, at, &resolve)
        };
        let mut sources: Vec<&crate::preview::Target> = targets.audio.iter().collect();
        for layer in &targets.layers {
            if let LayerSource::Video(t) = &layer.source {
                if !t.muted {
                    sources.push(t);
                }
            }
        }
        let mut mix = vec![0.0f32; frames * 2];
        let mut used: HashSet<ClipId> = HashSet::new();
        for target in sources {
            if target.volume <= 0.0 || self.silent_uris.contains(&target.uri) {
                continue;
            }
            used.insert(target.clip);
            if self.broken.contains(&target.clip) {
                continue;
            }
            let first = ((target.source_time.as_seconds_f64() - half) * rate)
                .round()
                .max(0.0) as i64;
            if self
                .sounds
                .get(&target.clip)
                .is_some_and(|r| !r.can_reach(first))
            {
                self.sounds.remove(&target.clip);
            }
            if !self.sounds.contains_key(&target.clip) {
                match AudioReader::open(&target.uri, first, self.job.sample_rate) {
                    Ok(reader) => {
                        self.sounds.insert(target.clip, reader);
                    }
                    Err(error) => {
                        let clip = target.clip;
                        self.warn(clip, format!("Sound could not be read ({error})."));
                        continue;
                    }
                }
            }
            let Some(reader) = self.sounds.get_mut(&target.clip) else {
                continue;
            };
            let samples = reader.read(frames);
            let gain = target.volume as f32;
            for (m, s) in mix.iter_mut().zip(samples) {
                *m += s * gain;
            }
        }
        self.sounds.retain(|clip, _| used.contains(clip));
        for m in &mut mix {
            *m = m.clamp(-1.0, 1.0);
        }
        mix
    }
}

/// One picture of the project at `at`, `long_edge` pixels on its longer side,
/// as straight RGBA: what the Export window shows. `None` if it cannot be made.
pub fn preview_frame(job: &Job, at: Ticks, long_edge: u32) -> Option<(u32, u32, Vec<u8>)> {
    let (pw, ph) = (job.canvas_width.max(2), job.canvas_height.max(2));
    let k = f64::from(long_edge) / f64::from(pw.max(ph));
    let size = |v: u32| (((f64::from(v) * k) / 2.0).round() as u32).max(1) * 2;
    let (w, h) = (size(pw), size(ph));
    let mut small = job.clone();
    small.width = w;
    small.height = h;
    small.canvas_width = w;
    small.canvas_height = h;
    small.gpu = false;
    let mut renderer = Renderer::new(&small);
    renderer.picture(at).ok()?;
    renderer.ready.pop_front().map(|pixels| (w, h, pixels))
}

// ---------------------------------------------------------------- the run

fn bus_error(bus: &gst::Bus) -> Option<String> {
    let message = bus.pop_filtered(&[gst::MessageType::Error])?;
    if let gst::MessageView::Error(e) = message.view() {
        let debug = e.debug().map(|d| d.to_string()).unwrap_or_default();
        return Some(format!("{} {debug}", e.error()).trim().to_string());
    }
    None
}

fn push(src: &gst_app::AppSrc, bus: &gst::Bus, buffer: gst::Buffer) -> Result<()> {
    src.push_buffer(buffer).map(|_| ()).map_err(|e| {
        ExportError::Failed(bus_error(bus).unwrap_or_else(|| format!("the encoder stopped ({e})")))
    })
}

/// Put the project's picture (`canvas_width` x `canvas_height`) in the middle of
/// the exported frame (`width` x `height`): black bars where it is smaller, the
/// overhang cut off where it is bigger. Nothing to do when the sizes match.
fn into_frame(job: &Job, pixels: Vec<u8>) -> Vec<u8> {
    center_in_frame(
        pixels,
        (job.canvas_width as usize, job.canvas_height as usize),
        (job.width as usize, job.height as usize),
    )
}

/// `pixels` (RGBA, `canvas` size) centred in a frame of `frame` size.
fn center_in_frame(pixels: Vec<u8>, canvas: (usize, usize), frame: (usize, usize)) -> Vec<u8> {
    let ((cw, ch), (w, h)) = (canvas, frame);
    if (cw, ch) == (w, h) {
        return pixels;
    }
    let mut out = vec![0u8; w * h * 4];
    for px in out.chunks_exact_mut(4) {
        px[3] = 255;
    }
    // Where the picture's top left corner lands in the frame (may be negative).
    let ox = (w as i64 - cw as i64) / 2;
    let oy = (h as i64 - ch as i64) / 2;
    let x0 = ox.max(0) as usize;
    let x1 = ((ox + cw as i64).min(w as i64)).max(0) as usize;
    if x1 <= x0 {
        return out;
    }
    for y in 0..h {
        let sy = y as i64 - oy;
        if sy < 0 || sy >= ch as i64 {
            continue;
        }
        let sx = (x0 as i64 - ox) as usize;
        let len = (x1 - x0) * 4;
        let from = (sy as usize * cw + sx) * 4;
        let to = (y * w + x0) * 4;
        out[to..to + len].copy_from_slice(&pixels[from..from + len]);
    }
    out
}

/// Hand one finished picture to the encoder as frame number `index`.
fn push_video(
    vsrc: &gst_app::AppSrc,
    bus: &gst::Bus,
    pixels: Vec<u8>,
    index: u64,
    frame_ns: &dyn Fn(u64) -> u64,
) -> Result<()> {
    let mut buffer = gst::Buffer::from_mut_slice(pixels);
    {
        let b = buffer.get_mut().ok_or("buffer")?;
        b.set_pts(gst::ClockTime::from_nseconds(frame_ns(index)));
        b.set_duration(gst::ClockTime::from_nseconds(
            frame_ns(index + 1).saturating_sub(frame_ns(index)),
        ));
    }
    push(vsrc, bus, buffer)
}

/// Render and save one file with one encoder.
fn run_with(job: &Job, encoder: &Encoder, progress: &Progress, temp: &Path) -> Result<Report> {
    if !audio_available(job.audio_format) {
        return fail(format!(
            "No {} sound encoder is installed in GStreamer.",
            job.audio_format.label()
        ));
    }
    if !container_available(job.container) {
        return fail(format!(
            "The {} file writer ({}) is missing from GStreamer.",
            job.container.label(),
            job.container.muxer()
        ));
    }
    let Some(audio) = audio_chain(job.audio_format, job.channels, true) else {
        return fail("No sound encoder is installed in GStreamer.");
    };
    let unlimited = "max-size-buffers=0 max-size-bytes=0 max-size-time=0";
    // Pictures waiting for the encoder are raw and large (8 MB each at 1080x1920),
    // and H.265, VP9 and AV1 encode far slower than Kani draws pictures, so this
    // queue must be small: when it is full the drawing waits for the encoder.
    // (An unlimited queue here once piled up gigabytes and made the system stop
    // the app on an 8 GB computer.) Only the small encoded data may queue freely.
    let waiting = "max-size-buffers=4 max-size-bytes=0 max-size-time=0";
    let description = format!(
        "{mux} name=mux ! filesink name=out \
         appsrc name=vsrc ! queue {waiting} ! {video} ! queue {unlimited} ! mux. \
         appsrc name=asrc ! queue {unlimited} ! {audio} ! queue {unlimited} ! mux.",
        mux = job.container.muxer(),
        video = video_chain(encoder),
    );
    let pipeline = gst::parse::launch(&description)
        .map_err(|e| ExportError::Failed(format!("could not build the export pipeline: {e}")))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| ExportError::Failed("export pipeline".into()))?;
    let get = |name: &str| {
        pipeline
            .by_name(name)
            .ok_or_else(|| ExportError::Failed(format!("export pipeline has no {name}")))
    };
    let vsrc = get("vsrc")?
        .downcast::<gst_app::AppSrc>()
        .map_err(|_| ExportError::Failed("vsrc".into()))?;
    let asrc = get("asrc")?
        .downcast::<gst_app::AppSrc>()
        .map_err(|_| ExportError::Failed("asrc".into()))?;
    get("out")?.set_property("location", temp.to_string_lossy().to_string());
    configure_encoder(&get("venc")?, encoder, job.video_kbps, job.fps);
    if let Some(aenc) = pipeline.by_name("aenc") {
        configure_audio(&aenc, job.audio_format, job.audio_kbps);
    }
    set_text(&get("mux")?, "faststart", "true");

    let (width, height, rate) = (job.width, job.height, job.sample_rate);
    vsrc.set_caps(Some(
        &gst::Caps::builder("video/x-raw")
            .field("format", "RGBA")
            .field("width", width as i32)
            .field("height", height as i32)
            .field(
                "framerate",
                gst::Fraction::new(job.fps.num as i32, job.fps.den as i32),
            )
            .build(),
    ));
    vsrc.set_format(gst::Format::Time);
    vsrc.set_block(true);
    vsrc.set_max_bytes(u64::from(width) * u64::from(height) * 4 * 6);
    asrc.set_caps(Some(
        &gst::Caps::builder("audio/x-raw")
            .field("format", "F32LE")
            .field("layout", "interleaved")
            .field("rate", rate as i32)
            .field("channels", 2i32)
            .build(),
    ));
    asrc.set_format(gst::Format::Time);
    asrc.set_block(true);
    asrc.set_max_bytes(1 << 22);

    let bus = pipeline
        .bus()
        .ok_or_else(|| ExportError::Failed("no bus".into()))?;
    if pipeline.set_state(gst::State::Playing).is_err() {
        let reason = bus_error(&bus).unwrap_or_else(|| "it would not start".into());
        let _ = pipeline.set_state(gst::State::Null);
        return fail(format!("{} could not start: {reason}", encoder.label));
    }

    let duration = job.duration();
    let frames = frame_count(duration, job.fps);
    progress.frames_total.store(frames, Ordering::Relaxed);
    progress.frames_done.store(0, Ordering::Relaxed);
    let total_seconds = duration.as_seconds_f64();
    let block = (rate as usize / 100).max(1); // 10 ms
    let blocks_total = (total_seconds * f64::from(rate) / block as f64).ceil() as u64;
    let frame_ns = |i: u64| (frame_time(i, job.fps).as_seconds_f64() * 1e9).round() as u64;

    let mut renderer = Renderer::new(job);
    let mut audio_block = 0u64;
    let mut pushed = 0u64;
    let (mut picture_time, mut push_time) = (Duration::ZERO, Duration::ZERO);
    let finish = |result: Result<()>, pipeline: &gst::Pipeline| {
        if result.is_err() {
            let _ = pipeline.set_state(gst::State::Null);
        }
        result
    };
    let mut run = || -> Result<()> {
        for i in 0..frames {
            if progress.cancel.load(Ordering::Relaxed) {
                return Err(ExportError::Cancelled);
            }
            let at = job.range_start + frame_time(i, job.fps);
            let t0 = std::time::Instant::now();
            renderer.picture(at)?;
            picture_time += t0.elapsed();
            let t1 = std::time::Instant::now();
            while let Some(pixels) = renderer.ready.pop_front() {
                let pixels = into_frame(job, pixels);
                progress.show(
                    job.width,
                    job.height,
                    &pixels,
                    pushed + 1,
                    pushed + 1 >= frames,
                );
                push_video(&vsrc, &bus, pixels, pushed, &frame_ns)?;
                pushed += 1;
            }
            push_time += t1.elapsed();
            // Sound up to the end of this frame.
            let until = frame_time(i + 1, job.fps)
                .as_seconds_f64()
                .min(total_seconds);
            while audio_block < blocks_total
                && (audio_block * block as u64) as f64 / f64::from(rate) < until
            {
                push_audio(&asrc, &bus, &mut renderer, audio_block, block, rate)?;
                audio_block += 1;
            }
            progress.frames_done.store(i + 1, Ordering::Relaxed);
        }
        // The last pictures still on their way back from the card.
        let t0 = std::time::Instant::now();
        renderer.flush()?;
        picture_time += t0.elapsed();
        while let Some(pixels) = renderer.ready.pop_front() {
            let pixels = into_frame(job, pixels);
            progress.show(
                job.width,
                job.height,
                &pixels,
                pushed + 1,
                pushed + 1 >= frames,
            );
            push_video(&vsrc, &bus, pixels, pushed, &frame_ns)?;
            pushed += 1;
        }
        while audio_block < blocks_total {
            if progress.cancel.load(Ordering::Relaxed) {
                return Err(ExportError::Cancelled);
            }
            push_audio(&asrc, &bus, &mut renderer, audio_block, block, rate)?;
            audio_block += 1;
        }
        eprintln!(
            "kani: compositing time by kind of layer:\n{}",
            renderer.slowest_layers()
        );
        eprintln!(
            "kani: skipped {} layer pictures hidden under a solid layer; {} video pictures were shrunk while decoding",
            renderer.hidden_skipped, renderer.shrunk_frames
        );
        if std::env::var_os("KANI_EXPORT_TIMING").is_some() {
            eprintln!(
                "export timing: pictures {picture_time:?} (waiting for decoded video {:?}, compositing {:?}), handing to the encoder {push_time:?}",
                renderer.decode_time, renderer.compose_time
            );
        }
        let _ = vsrc.end_of_stream();
        let _ = asrc.end_of_stream();
        // The encoder and muxer finish their last work.
        loop {
            if progress.cancel.load(Ordering::Relaxed) {
                return Err(ExportError::Cancelled);
            }
            match bus.timed_pop_filtered(
                gst::ClockTime::from_mseconds(200),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            ) {
                Some(m) => match m.view() {
                    gst::MessageView::Eos(_) => return Ok(()),
                    gst::MessageView::Error(e) => {
                        return fail(format!("{} {}", e.error(), e.debug().unwrap_or_default()))
                    }
                    _ => {}
                },
                None => {}
            }
        }
    };
    let result = finish(run(), &pipeline);
    let _ = pipeline.set_state(gst::State::Null);
    result?;
    Ok(Report {
        encoder: encoder.label.to_string(),
        frames,
        warnings: renderer.warnings,
        elapsed: Duration::ZERO, // filled in by export()
        decode: renderer.decode_time,
        compose: renderer.compose_time,
        handoff: push_time,
        hidden_skipped: renderer.hidden_skipped,
        shrunk_frames: renderer.shrunk_frames,
        gpu_layers: renderer.gpu_layers,
        gpu_frames: renderer.gpu_frames,
        gpu_skipped_frames: renderer.gpu_skipped_frames,
        gpu_problem: renderer.gpu_problem.clone(),
        audio_saved: false,
    })
}

fn push_audio(
    asrc: &gst_app::AppSrc,
    bus: &gst::Bus,
    renderer: &mut Renderer,
    index: u64,
    block: usize,
    rate: u32,
) -> Result<()> {
    let start = index * block as u64;
    // The sound is read from the timeline at `range_start` onwards, but stamped
    // from zero: the exported file starts at its own beginning.
    let offset = renderer.job.range_start.as_seconds_f64();
    let samples = renderer.sound(offset + start as f64 / f64::from(rate), block);
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    let ns = |n: u64| n * 1_000_000_000 / u64::from(rate);
    let mut buffer = gst::Buffer::from_mut_slice(bytes);
    {
        let b = buffer.get_mut().ok_or("buffer")?;
        b.set_pts(gst::ClockTime::from_nseconds(ns(start)));
        b.set_duration(gst::ClockTime::from_nseconds(
            ns(start + block as u64) - ns(start),
        ));
    }
    push(asrc, bus, buffer)
}

/// A temporary name next to `output`, with the same ending, for the file
/// while it is being written.
fn temp_name(output: &Path) -> PathBuf {
    let mut name = output
        .file_stem()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(".exporting.");
    name.push(output.extension().unwrap_or_default());
    output.with_file_name(name)
}

/// Make the file of the project's sound alone.
fn run_audio_file(job: &Job, progress: &Progress, temp: &Path) -> Result<()> {
    let format = job.audio_format;
    if !audio_available(format) {
        return fail(format!(
            "No {} sound encoder is installed in GStreamer.",
            format.label()
        ));
    }
    let Some(chain) = audio_chain(format, job.channels, false) else {
        return fail("No sound encoder is installed in GStreamer.");
    };
    let unlimited = "max-size-buffers=0 max-size-bytes=0 max-size-time=0";
    let tail = match format {
        AudioFormat::Aac => "mp4mux faststart=true ! filesink name=out",
        AudioFormat::Mp3 if has_element("id3v2mux") => "id3v2mux ! filesink name=out",
        AudioFormat::Opus if has_element("oggmux") => "oggmux ! filesink name=out",
        _ => "filesink name=out",
    };
    let description = format!("appsrc name=asrc ! queue {unlimited} ! {chain} ! {tail}");
    let pipeline = gst::parse::launch(&description)
        .map_err(|e| ExportError::Failed(format!("could not build the sound pipeline: {e}")))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| ExportError::Failed("sound pipeline".into()))?;
    let get = |name: &str| {
        pipeline
            .by_name(name)
            .ok_or_else(|| ExportError::Failed(format!("sound pipeline has no {name}")))
    };
    let asrc = get("asrc")?
        .downcast::<gst_app::AppSrc>()
        .map_err(|_| ExportError::Failed("asrc".into()))?;
    get("out")?.set_property("location", temp.to_string_lossy().to_string());
    if let Some(aenc) = pipeline.by_name("aenc") {
        configure_audio(&aenc, format, job.audio_kbps);
    }
    let rate = job.sample_rate;
    asrc.set_caps(Some(
        &gst::Caps::builder("audio/x-raw")
            .field("format", "F32LE")
            .field("layout", "interleaved")
            .field("rate", rate as i32)
            .field("channels", 2i32)
            .build(),
    ));
    asrc.set_format(gst::Format::Time);
    asrc.set_block(true);
    asrc.set_max_bytes(1 << 22);
    let bus = pipeline
        .bus()
        .ok_or_else(|| ExportError::Failed("no bus".into()))?;
    if pipeline.set_state(gst::State::Playing).is_err() {
        let reason = bus_error(&bus).unwrap_or_else(|| "it would not start".into());
        let _ = pipeline.set_state(gst::State::Null);
        return fail(format!("the sound file could not start: {reason}"));
    }
    let block = (rate as usize / 100).max(1); // 10 ms
    let total_seconds = job.duration().as_seconds_f64();
    let blocks_total = (total_seconds * f64::from(rate) / block as f64).ceil() as u64;
    progress.audio_done.store(0, Ordering::Relaxed);
    progress.audio_total.store(blocks_total, Ordering::Relaxed);
    let mut renderer = Renderer::new(job);
    let mut run = || -> Result<()> {
        for index in 0..blocks_total {
            if progress.cancel.load(Ordering::Relaxed) {
                return Err(ExportError::Cancelled);
            }
            push_audio(&asrc, &bus, &mut renderer, index, block, rate)?;
            progress.audio_done.store(index + 1, Ordering::Relaxed);
        }
        let _ = asrc.end_of_stream();
        loop {
            if progress.cancel.load(Ordering::Relaxed) {
                return Err(ExportError::Cancelled);
            }
            if let Some(m) = bus.timed_pop_filtered(
                gst::ClockTime::from_mseconds(200),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            ) {
                match m.view() {
                    gst::MessageView::Eos(_) => return Ok(()),
                    gst::MessageView::Error(e) => {
                        return fail(format!("{} {}", e.error(), e.debug().unwrap_or_default()))
                    }
                    _ => {}
                }
            }
        }
    };
    let result = run();
    let _ = pipeline.set_state(gst::State::Null);
    result
}

/// Make the video file (with its sound), trying each encoder in turn.
fn export_video(
    job: &Job,
    progress: &Progress,
    output: &Path,
    started: std::time::Instant,
) -> Result<Report> {
    if job.encoders.is_empty() {
        return fail(
            "No video encoder is available for this video format. Install GStreamer's plug-in for it, or pick another video format.",
        );
    }
    let temp = temp_name(output);
    let mut notes = Vec::new();
    let mut outcome = fail("No encoder worked.");
    for encoder in &job.encoders {
        let _ = std::fs::remove_file(&temp);
        match run_with(job, encoder, progress, &temp) {
            Ok(mut report) => {
                report.warnings.splice(0..0, notes);
                std::fs::rename(&temp, output)
                    .map_err(|e| format!("could not save {}: {e}", output.display()))?;
                report.elapsed = started.elapsed();
                let video = job.duration().as_seconds_f64();
                eprintln!(
                    "kani: exported {video:.1} s of video in {:.1} s ({:.2}x real time) with {}; waiting for decoded video {:.1} s, compositing {:.1} s, handing to the encoder {:.1} s; {} hidden layer pictures skipped, {} video pictures shrunk while decoding",
                    report.elapsed.as_secs_f64(),
                    video / report.elapsed.as_secs_f64().max(0.001),
                    report.encoder,
                    report.decode.as_secs_f64(),
                    report.compose.as_secs_f64(),
                    report.handoff.as_secs_f64(),
                    report.hidden_skipped,
                    report.shrunk_frames,
                );
                if job.gpu {
                    match &report.gpu_problem {
                        Some(why) => eprintln!("kani: GPU compositing was on but not used fully: {why}"),
                        None => eprintln!(
                            "kani: GPU compositing was on: {} whole pictures ({} layer draws) put together on the graphics card, {} left to the processor",
                            report.gpu_frames, report.gpu_layers, report.gpu_skipped_frames
                        ),
                    }
                }
                return Ok(report);
            }
            Err(ExportError::Cancelled) => {
                let _ = std::fs::remove_file(&temp);
                return Err(ExportError::Cancelled);
            }
            Err(ExportError::Failed(message)) => {
                notes.push(format!(
                    "{} did not work ({message}); tried the next one.",
                    encoder.label
                ));
                outcome = Err(ExportError::Failed(format!("{}: {message}", encoder.label)));
            }
        }
    }
    let _ = std::fs::remove_file(&temp);
    outcome
}

/// Make the file of the project's sound alone, written under a temporary name
/// and moved into place only when it is complete.
fn make_sound_file(job: &Job, progress: &Progress, path: &Path) -> Result<()> {
    let temp = temp_name(path);
    let _ = std::fs::remove_file(&temp);
    let started = std::time::Instant::now();
    match run_audio_file(job, progress, &temp) {
        Ok(()) => match std::fs::rename(&temp, path) {
            Ok(()) => {
                eprintln!(
                    "kani: saved the sound file ({}) in {:.1} s",
                    job.audio_format.label(),
                    started.elapsed().as_secs_f64()
                );
                Ok(())
            }
            Err(e) => {
                let _ = std::fs::remove_file(&temp);
                fail(format!("could not save {}: {e}", path.display()))
            }
        },
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e)
        }
    }
}

/// Export the job. Blocks; run it on a worker thread. Each file appears at its
/// path only if everything worked (it is written under a temporary name
/// first); a cancelled or failed export leaves nothing behind.
pub fn export(job: &Job, progress: &Progress) -> Result<Report> {
    gst::init().map_err(|e| ExportError::Failed(e.to_string()))?;
    if job.timeline.duration() <= Ticks::ZERO {
        return fail("There is nothing on the timeline to export.");
    }
    if job.duration() <= Ticks::ZERO {
        return fail("The chosen part of the timeline is empty.");
    }
    if job.output.is_none() && job.audio_output.is_none() {
        return fail("Nothing to export: choose video or audio.");
    }
    eprintln!("kani: export starting - {}", machine_state());
    for path in job.output.iter().chain(job.audio_output.iter()) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
    }
    progress
        .audio_only
        .store(job.output.is_none(), Ordering::Relaxed);
    let started = std::time::Instant::now();
    // The video file and the sound file are made at the same time, each on its
    // own thread: the sound is a light job next to the pictures, so it comes
    // for free instead of adding its time to the end.
    let (video_result, sound_result) = match (&job.output, &job.audio_output) {
        (Some(video), Some(sound)) => std::thread::scope(|scope| {
            let sound_thread = scope.spawn(|| make_sound_file(job, progress, sound));
            let video_result = export_video(job, progress, video, started);
            if video_result.is_err() {
                // The video failed or was cancelled: stop the sound too.
                progress.cancel.store(true, Ordering::Relaxed);
            }
            let sound_result = sound_thread
                .join()
                .unwrap_or_else(|_| fail("the sound file stopped unexpectedly"));
            (Some(video_result), Some(sound_result))
        }),
        (Some(video), None) => (Some(export_video(job, progress, video, started)), None),
        (None, Some(sound)) => {
            progress.audio_pass.store(true, Ordering::Relaxed);
            (None, Some(make_sound_file(job, progress, sound)))
        }
        (None, None) => (None, None),
    };
    let mut report = match video_result {
        Some(result) => result?,
        None => Report {
            encoder: "sound only".into(),
            ..Report::default()
        },
    };
    match sound_result {
        Some(Ok(())) => report.audio_saved = true,
        Some(Err(ExportError::Cancelled)) => return Err(ExportError::Cancelled),
        Some(Err(ExportError::Failed(message))) => {
            if job.output.is_some() {
                report
                    .warnings
                    .push(format!("The sound file could not be made: {message}"));
            } else {
                return fail(message);
            }
        }
        None => {}
    }
    report.elapsed = started.elapsed();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_counted_up_to_the_end() {
        let second = Ticks::from_seconds_f64(1.0);
        assert_eq!(frame_count(second, FrameRate::FPS_30), 30);
        assert_eq!(
            frame_count(Ticks::from_seconds_f64(1.01), FrameRate::FPS_30),
            31
        );
        assert_eq!(frame_count(Ticks::ZERO, FrameRate::FPS_30), 0);
        assert_eq!(frame_count(second, FrameRate::NTSC_29_97), 30);
    }

    // ----- real exports (need GStreamer's test plug-ins and x264) -------------

    use crate::media::test_media;
    use cutroom_timeline::{apply, Edit, TrackKind};

    fn s(t: f64) -> Ticks {
        Ticks::from_seconds_f64(t)
    }

    /// V1: a video (media 1) from 1 s in, shown 0..2 s; A1: a sound (media 2) at 1..3 s.
    fn small_job(dir: &Path) -> Option<Job> {
        if !test_media::init()
            || !has_element("x264enc")
            || audio_encoder().is_none()
            || !has_element("mp4mux")
        {
            eprintln!("encoders unavailable: skipping");
            return None;
        }
        let (video, audio) = (dir.join("v.webm"), dir.join("a.wav"));
        test_media::make_video(&video, 4);
        test_media::make_wav(&audio, 3);
        let mut tl = Timeline::new();
        let vt = tl.new_track(TrackKind::Video, "V1");
        let vid = vt.id;
        apply(
            &mut tl,
            Edit::InsertTrack {
                index: 0,
                track: vt,
            },
        )
        .unwrap();
        let at = tl.new_track(TrackKind::Audio, "A1");
        let aid = at.id;
        apply(
            &mut tl,
            Edit::InsertTrack {
                index: 1,
                track: at,
            },
        )
        .unwrap();
        let c = tl.new_clip(MediaId(1), s(1.0), s(0.0), s(2.0));
        apply(
            &mut tl,
            Edit::InsertClip {
                track: vid,
                clip: c,
            },
        )
        .unwrap();
        let c = tl.new_clip(MediaId(2), s(0.0), s(1.0), s(2.0));
        apply(
            &mut tl,
            Edit::InsertClip {
                track: aid,
                clip: c,
            },
        )
        .unwrap();
        let mut sources = HashMap::new();
        sources.insert(
            MediaId(1),
            Source {
                path: video,
                kind: MediaKind::Video,
                has_audio: true,
            },
        );
        sources.insert(
            MediaId(2),
            Source {
                path: audio,
                kind: MediaKind::Audio,
                has_audio: true,
            },
        );
        Some(Job {
            timeline: tl,
            sources,
            width: 160,
            height: 90,
            canvas_width: 160,
            canvas_height: 90,
            container: Container::Mp4,
            audio_format: AudioFormat::Aac,
            channels: Channels::Stereo,
            audio_output: None,
            fps: FrameRate::FPS_30,
            sample_rate: 48_000,
            video_kbps: 1_500,
            audio_kbps: 128,
            encoders: candidates(VideoCodec::H264)
                .into_iter()
                .filter(|e| e.element == "x264enc")
                .collect(),
            output: Some(dir.join("out").join("result.mp4")),
            range_start: Ticks::ZERO,
            range_end: None,
            limit_seconds: None,
            gpu: false,
        })
    }

    fn ffprobe(path: &Path, args: &[&str]) -> Option<String> {
        let out = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-of", "default=nw=1"])
            .args(args)
            .arg(path)
            .output()
            .ok()?;
        Some(String::from_utf8_lossy(&out.stdout).to_string())
    }

    #[test]
    fn a_timeline_is_exported_as_h264_and_aac_in_an_mp4() {
        let dir = tempfile::tempdir().unwrap();
        let Some(job) = small_job(dir.path()) else {
            return;
        };
        let progress = Progress::default();
        let report = export(&job, &progress).expect("export");
        assert_eq!(report.frames, 90, "three seconds at 30 fps");
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!((progress.fraction() - 1.0).abs() < 1e-9);
        assert!(job.output.as_ref().unwrap().is_file());
        assert!(
            !job.output
                .as_ref()
                .unwrap()
                .with_file_name("result.exporting.mp4")
                .exists(),
            "the temporary file is gone"
        );
        if let Some(info) = ffprobe(
            job.output.as_ref().unwrap(),
            &[
                "-show_entries",
                "stream=codec_name,width,height:format=duration",
            ],
        ) {
            if !info.is_empty() {
                assert!(info.contains("codec_name=h264"), "{info}");
                assert!(info.contains("codec_name=aac"), "{info}");
                assert!(
                    info.contains("width=160") && info.contains("height=90"),
                    "{info}"
                );
                let secs: f64 = info
                    .lines()
                    .find_map(|l| l.strip_prefix("duration="))
                    .and_then(|v| v.parse().ok())
                    .expect("duration");
                assert!((2.9..3.3).contains(&secs), "{secs} s");
            }
        }
    }

    #[test]
    fn a_layer_hidden_under_a_solid_one_is_not_drawn_and_a_small_one_is_shrunk() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut job) = small_job(dir.path()) else {
            return;
        };
        // A second video track on top, enlarged until it covers the canvas.
        let top = job.timeline.new_track(TrackKind::Video, "V2");
        let top_id = top.id;
        apply(
            &mut job.timeline,
            Edit::InsertTrack {
                index: 0,
                track: top,
            },
        )
        .unwrap();
        let clip = job.timeline.new_clip(MediaId(1), s(1.0), s(0.0), s(2.0));
        let clip_id = clip.id;
        apply(
            &mut job.timeline,
            Edit::InsertClip {
                track: top_id,
                clip,
            },
        )
        .unwrap();
        let big = cutroom_timeline::Transform {
            scale: 1500,
            ..cutroom_timeline::Transform::default()
        };
        apply(
            &mut job.timeline,
            Edit::SetClipTransform {
                clip: clip_id,
                transform: big,
            },
        )
        .unwrap();
        let report = export(&job, &Progress::default()).expect("export");
        assert!(
            report.hidden_skipped > 50,
            "{} skipped",
            report.hidden_skipped
        );
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);

        // A small picture is decoded smaller.
        let small = cutroom_timeline::Transform {
            scale: 400,
            ..cutroom_timeline::Transform::default()
        };
        apply(
            &mut job.timeline,
            Edit::SetClipTransform {
                clip: clip_id,
                transform: small,
            },
        )
        .unwrap();
        let report = export(&job, &Progress::default()).expect("export");
        assert_eq!(report.hidden_skipped, 0);
        assert!(report.shrunk_frames > 20, "{} shrunk", report.shrunk_frames);
        assert!(job.output.as_ref().unwrap().is_file());
    }

    #[test]
    fn video_can_be_put_together_on_the_graphics_card_or_not() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut job) = small_job(dir.path()) else {
            return;
        };
        // A slanted, scaled picture: the kind of layer that goes to the card.
        let clip = job.timeline.tracks[0].clips[0].id;
        let slanted = cutroom_timeline::Transform {
            scale: 800,
            skew_x: 80,
            ..cutroom_timeline::Transform::default()
        };
        apply(
            &mut job.timeline,
            Edit::SetClipTransform {
                clip,
                transform: slanted,
            },
        )
        .unwrap();
        let off = export(&job, &Progress::default()).expect("export without the card");
        assert_eq!(off.gpu_layers, 0);
        job.gpu = true;
        let on = export(&job, &Progress::default()).expect("export with the card");
        match &on.gpu_problem {
            Some(why) => eprintln!("no usable graphics card here ({why}): fell back"),
            None => assert!(on.gpu_frames > 30, "{} pictures on the card", on.gpu_frames),
        }
        assert_eq!(on.frames, off.frames);
        assert!(on.warnings.is_empty(), "{:?}", on.warnings);
    }

    #[test]
    fn a_cancelled_export_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let Some(job) = small_job(dir.path()) else {
            return;
        };
        let progress = Progress::default();
        progress.cancel.store(true, Ordering::Relaxed);
        assert!(matches!(
            export(&job, &progress),
            Err(ExportError::Cancelled)
        ));
        assert!(!job.output.as_ref().unwrap().exists());
        let leftovers: Vec<_> = std::fs::read_dir(job.output.as_ref().unwrap().parent().unwrap())
            .unwrap()
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn an_empty_timeline_and_a_missing_encoder_are_explained() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut job) = small_job(dir.path()) else {
            return;
        };
        job.encoders.clear();
        let error = export(&job, &Progress::default()).unwrap_err().to_string();
        assert!(error.contains("encoder"), "{error}");
        job.timeline = Timeline::new();
        let error = export(&job, &Progress::default()).unwrap_err().to_string();
        assert!(error.contains("nothing"), "{error}");
    }

    #[test]
    fn the_exported_picture_and_sound_are_really_there() {
        let dir = tempfile::tempdir().unwrap();
        let Some(job) = small_job(dir.path()) else {
            return;
        };
        export(&job, &Progress::default()).expect("export");
        let ffmpeg = |args: &[&str]| {
            std::process::Command::new("ffmpeg")
                .args(["-v", "error"])
                .args(args)
                .output()
                .ok()
        };
        // A frame from the middle of the video part has colour in it ...
        let Some(frame) = ffmpeg(&[
            "-ss",
            "0.5",
            "-i",
            job.output.as_ref().unwrap().to_str().unwrap(),
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ]) else {
            return;
        };
        assert_eq!(frame.stdout.len(), 160 * 90 * 3);
        assert!(frame.stdout.iter().any(|&b| b > 60), "not black");
        // ... a frame after the video clip ends (2..3 s) is black ...
        let late = ffmpeg(&[
            "-ss",
            "2.5",
            "-i",
            job.output.as_ref().unwrap().to_str().unwrap(),
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ])
        .unwrap();
        assert!(late.stdout.iter().all(|&b| b < 30), "black after the clip");
        // ... and there is sound: the clip's own at the start, the other file at the end.
        for (from, to) in [("0.2", "0.8"), ("2.2", "2.8")] {
            let out = ffmpeg(&[
                "-ss",
                from,
                "-t",
                &format!(
                    "{:.1}",
                    to.parse::<f64>().unwrap() - from.parse::<f64>().unwrap()
                ),
                "-i",
                job.output.as_ref().unwrap().to_str().unwrap(),
                "-vn",
                "-ac",
                "1",
                "-f",
                "f32le",
                "-",
            ])
            .unwrap();
            let loud = out
                .stdout
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]).abs())
                .fold(0.0f32, f32::max);
            assert!(loud > 0.05, "sound between {from} and {to} s: peak {loud}");
        }
    }

    /// Not run by default: exports a mixed timeline of real clips found in
    /// `KANI_FOOTAGE` to `KANI_REAL_OUT`, and prints how long it took.
    #[test]
    #[ignore]
    fn real_footage_exports() {
        let (Some(dir), Some(out)) = (
            std::env::var_os("KANI_FOOTAGE").map(PathBuf::from),
            std::env::var_os("KANI_REAL_OUT").map(PathBuf::from),
        ) else {
            return;
        };
        gst::init().unwrap();
        let files = [
            "01-Ginuwine_-_Pony__Lyrics__1080p.mp4",
            "01-Edible_danger_fried_Mac_and_cheese_1080p.mp4",
            "01-The_Heils_-_Aryans__Official_Music_Video_.mp4",
            "01-asamimi_1.mp4",
            "01-sbr.mp4",
        ];
        let mut sources = HashMap::new();
        for (i, f) in files.iter().enumerate() {
            sources.insert(
                MediaId(i as u64 + 1),
                Source {
                    path: dir.join(f),
                    kind: MediaKind::Video,
                    has_audio: true,
                },
            );
        }
        let mut tl = Timeline::new();
        let top = tl.new_track(TrackKind::Video, "V2");
        let top_id = top.id;
        apply(
            &mut tl,
            Edit::InsertTrack {
                index: 0,
                track: top,
            },
        )
        .unwrap();
        let v = tl.new_track(TrackKind::Video, "V1");
        let vid = v.id;
        apply(&mut tl, Edit::InsertTrack { index: 1, track: v }).unwrap();
        let a = tl.new_track(TrackKind::Audio, "A1");
        let aid = a.id;
        apply(&mut tl, Edit::InsertTrack { index: 2, track: a }).unwrap();
        // Four consecutive pieces from four different files (H.264 landscape,
        // H.264 portrait, AV1, HEVC), 2.5 s each, starting a little way in.
        for (i, (media, from)) in [(1u64, 2.0), (2, 1.0), (3, 4.0), (4, 0.5)]
            .iter()
            .enumerate()
        {
            let c = tl.new_clip(MediaId(*media), s(*from), s(2.5 * i as f64), s(2.5));
            apply(
                &mut tl,
                Edit::InsertClip {
                    track: vid,
                    clip: c,
                },
            )
            .unwrap();
        }
        // Music under all of it: the sound of another file.
        let c = tl.new_clip(MediaId(5), s(1.0), s(0.0), s(10.0));
        apply(
            &mut tl,
            Edit::InsertClip {
                track: aid,
                clip: c,
            },
        )
        .unwrap();
        // A title over the middle.
        let mut t = tl.new_clip(MediaId(0), s(0.0), s(3.0), s(4.0));
        let mut spec = cutroom_timeline::TextSpec::default();
        spec.content = "Kani export test".into();
        t.text = Some(spec);
        apply(
            &mut tl,
            Edit::InsertClip {
                track: top_id,
                clip: t,
            },
        )
        .unwrap();
        let job = Job {
            timeline: tl,
            sources,
            width: 1080,
            height: 1920,
            canvas_width: 1080,
            canvas_height: 1920,
            container: Container::Mp4,
            audio_format: AudioFormat::Aac,
            channels: Channels::Stereo,
            audio_output: None,
            fps: FrameRate::FPS_30,
            sample_rate: 48_000,
            video_kbps: crate::export_options::video_kbps(
                VideoCodec::H264,
                1080,
                1920,
                30.0,
                crate::export_options::BitratePreset::Recommended,
            ),
            audio_kbps: 192,
            encoders: candidates(VideoCodec::H264)
                .into_iter()
                .filter(|e| e.element == "x264enc")
                .collect(),
            output: Some(out),
            range_start: Ticks::ZERO,
            range_end: None,
            limit_seconds: None,
            gpu: false,
        };
        let started = std::time::Instant::now();
        let report = export(&job, &Progress::default()).expect("export");
        eprintln!(
            "exported {} frames with {} in {:.1} s; warnings {:?}",
            report.frames,
            report.encoder,
            started.elapsed().as_secs_f64(),
            report.warnings
        );
    }

    fn env_number(name: &str, default: u32) -> u32 {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    /// Speed check: one plain 1080x1920 clip with its sound, as a vertical
    /// phone project is. `KANI_CLIP` is the file, `KANI_REAL_OUT` the result.
    #[test]
    #[ignore]
    fn timed_vertical_clip() {
        let (Some(clip), Some(out)) = (
            std::env::var_os("KANI_CLIP").map(PathBuf::from),
            std::env::var_os("KANI_REAL_OUT").map(PathBuf::from),
        ) else {
            return;
        };
        let seconds: f64 = std::env::var("KANI_SECONDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20.0);
        gst::init().unwrap();
        let mut sources = HashMap::new();
        sources.insert(
            MediaId(1),
            Source {
                path: clip,
                kind: MediaKind::Video,
                has_audio: true,
            },
        );
        let mut tl = Timeline::new();
        let v = tl.new_track(TrackKind::Video, "V1");
        let vid = v.id;
        apply(&mut tl, Edit::InsertTrack { index: 0, track: v }).unwrap();
        let a = tl.new_track(TrackKind::Audio, "A1");
        let aid = a.id;
        apply(&mut tl, Edit::InsertTrack { index: 1, track: a }).unwrap();
        for track in [vid, aid] {
            let c = tl.new_clip(MediaId(1), s(0.0), s(0.0), s(seconds));
            apply(&mut tl, Edit::InsertClip { track, clip: c }).unwrap();
        }
        // KANI_STILL=<image> adds that picture over the video, zoomed like the slow project.
        if let Some(image) = std::env::var_os("KANI_STILL") {
            sources.insert(
                MediaId(2),
                Source {
                    path: PathBuf::from(image),
                    kind: MediaKind::Image,
                    has_audio: false,
                },
            );
            let layer = tl.new_track(TrackKind::Video, "V3");
            let layer_id = layer.id;
            apply(
                &mut tl,
                Edit::InsertTrack {
                    index: 0,
                    track: layer,
                },
            )
            .unwrap();
            let mut c = tl.new_clip(MediaId(2), s(0.0), s(0.0), s(seconds));
            c.transform.scale = 1642;
            apply(
                &mut tl,
                Edit::InsertClip {
                    track: layer_id,
                    clip: c,
                },
            )
            .unwrap();
        }
        // KANI_CAPTION=1 adds a caption-like text clip at the captions' usual place.
        if std::env::var_os("KANI_CAPTION").is_some() {
            let top = tl.new_track(TrackKind::Video, "V2");
            let top_id = top.id;
            apply(
                &mut tl,
                Edit::InsertTrack {
                    index: 0,
                    track: top,
                },
            )
            .unwrap();
            let mut t = tl.new_clip(MediaId(0), s(0.0), s(0.0), s(seconds));
            let mut spec = cutroom_timeline::TextSpec::default();
            spec.content = "Caption test".into();
            t.text = Some(spec);
            t.transform.y = cutroom_timeline::text::CAPTION_Y;
            if std::env::var_os("KANI_WARP").is_some() {
                // Skewed and in perspective, like the project that was slow.
                t.transform.skew_x = 100;
                t.transform.persp_x = 200;
                t.transform.scale = 663;
            }
            apply(
                &mut tl,
                Edit::InsertClip {
                    track: top_id,
                    clip: t,
                },
            )
            .unwrap();
        }
        let job = Job {
            timeline: tl,
            sources,
            width: env_number("KANI_W", 1080),
            height: env_number("KANI_H", 1920),
            canvas_width: env_number("KANI_W", 1080),
            canvas_height: env_number("KANI_H", 1920),
            container: Container::Mp4,
            audio_format: AudioFormat::Aac,
            channels: Channels::Stereo,
            audio_output: None,
            fps: FrameRate::FPS_30,
            sample_rate: 48_000,
            video_kbps: crate::export_options::video_kbps(
                VideoCodec::H264,
                1080,
                1920,
                30.0,
                crate::export_options::BitratePreset::Low,
            ),
            audio_kbps: 192,
            encoders: candidates(VideoCodec::H264)
                .into_iter()
                .filter(|e| e.element == "x264enc")
                .collect(),
            output: Some(out),
            range_start: Ticks::ZERO,
            range_end: None,
            limit_seconds: None,
            gpu: false,
        };
        let report = export(&job, &Progress::default()).expect("export");
        eprintln!(
            "RESULT: {seconds} s of video in {:.1} s elapsed; decode {:.1} s, compose {:.1} s, encoder hand-off {:.1} s",
            report.elapsed.as_secs_f64(),
            report.decode.as_secs_f64(),
            report.compose.as_secs_f64(),
            report.handoff.as_secs_f64()
        );
    }

    #[test]
    fn every_platform_lists_software_last_and_hardware_first() {
        let list = candidates(VideoCodec::H264);
        assert!(list.iter().any(|e| e.element == "x264enc"));
        let first_software = list
            .iter()
            .position(|e| !e.hardware && e.element == "x264enc");
        let last_hardware = list.iter().rposition(|e| e.hardware);
        if let (Some(s), Some(h)) = (first_software, last_hardware) {
            assert!(h < s, "hardware encoders come before software");
        }
    }

    #[test]
    fn a_picture_is_centred_with_bars_or_cropped_to_the_frame() {
        let red = |w: usize, h: usize| {
            [255u8, 0, 0, 255]
                .iter()
                .copied()
                .cycle()
                .take(w * h * 4)
                .collect::<Vec<u8>>()
        };
        // Narrow picture in a wide frame: black bars at the sides.
        let framed = center_in_frame(red(2, 2), (2, 2), (6, 2));
        for y in 0..2 {
            for (x, want) in [(0, 0u8), (1, 0), (2, 255), (3, 255), (4, 0), (5, 0)] {
                let px = &framed[(y * 6 + x) * 4..][..4];
                assert_eq!(px, &[want, 0, 0, 255], "pixel {x},{y}");
            }
        }
        // Wide picture in a narrow frame: the sides are cut off.
        let mut wide = red(6, 2);
        for y in 0..2 {
            for x in 0..6 {
                let o = (y * 6 + x) * 4;
                wide[o + 1] = (x * 40) as u8; // green tells the columns apart
            }
        }
        let cut = center_in_frame(wide, (6, 2), (2, 2));
        assert_eq!(cut.len(), 2 * 2 * 4);
        assert_eq!(cut[1], 80, "column 2 comes first");
        assert_eq!(cut[5], 120, "then column 3");
        // Same size: untouched.
        let same = red(4, 4);
        assert_eq!(center_in_frame(same.clone(), (4, 4), (4, 4)), same);
        // Taller and wider than the frame in each direction: nothing breaks.
        assert_eq!(center_in_frame(red(8, 8), (8, 8), (2, 6)).len(), 2 * 6 * 4);
    }

    /// Probe the streams of a file: "codec_name=..." lines.
    fn codecs_of(path: &Path) -> Option<String> {
        ffprobe(path, &["-show_entries", "stream=codec_name,channels"])
    }

    #[test]
    fn every_video_format_in_every_file_format_it_can_go_in_is_made_and_readable() {
        let dir = tempfile::tempdir().unwrap();
        let Some(base) = small_job(dir.path()) else {
            return;
        };
        let mut tried = 0;
        for codec in VideoCodec::ALL {
            let encoders: Vec<Encoder> = candidates(codec)
                .into_iter()
                .filter(|e| !e.hardware && has_element(e.element))
                .collect();
            if encoders.is_empty() {
                eprintln!("no software {} encoder: skipping", codec.name());
                continue;
            }
            for container in codec.containers() {
                if !container_available(*container) {
                    continue;
                }
                let mut job = base.clone();
                job.limit_seconds = Some(1.0);
                job.encoders = encoders.clone();
                job.container = *container;
                job.audio_format = container
                    .audio_formats()
                    .iter()
                    .copied()
                    .find(|f| audio_available(*f))
                    .unwrap();
                let path = dir.path().join(format!(
                    "{}-{}.{}",
                    codec.id(),
                    container.id(),
                    container.extension()
                ));
                job.output = Some(path.clone());
                export(&job, &Progress::default())
                    .unwrap_or_else(|e| panic!("{} in {}: {e:?}", codec.name(), container.label()));
                assert!(path.is_file(), "{} in {}", codec.name(), container.label());
                if let Some(info) = codecs_of(&path) {
                    if !info.is_empty() {
                        let want = match codec {
                            VideoCodec::H264 => "h264",
                            VideoCodec::H265 => "hevc",
                            VideoCodec::Vp9 => "vp9",
                            VideoCodec::Av1 => "av1",
                        };
                        assert!(
                            info.contains(&format!("codec_name={want}")),
                            "{} in {}: {info}",
                            codec.name(),
                            container.label()
                        );
                    }
                }
                tried += 1;
            }
        }
        eprintln!("{tried} video and file format pairings made");
        assert!(tried >= 3, "at least the H.264 formats were tried");
    }

    #[test]
    fn every_sound_format_works_in_the_files_that_can_carry_it() {
        let dir = tempfile::tempdir().unwrap();
        let Some(base) = small_job(dir.path()) else {
            return;
        };
        for container in Container::ALL {
            if !container_available(container) {
                continue;
            }
            // A codec this file format holds, with a software encoder here.
            let Some((codec, encoders)) = VideoCodec::ALL.iter().find_map(|c| {
                if !c.containers().contains(&container) {
                    return None;
                }
                let list: Vec<Encoder> = candidates(*c)
                    .into_iter()
                    .filter(|e| !e.hardware && has_element(e.element))
                    .collect();
                (!list.is_empty()).then_some((*c, list))
            }) else {
                continue;
            };
            for format in container.audio_formats() {
                if !audio_available(*format) {
                    continue;
                }
                let mut job = base.clone();
                job.limit_seconds = Some(1.0);
                job.encoders = encoders.clone();
                job.container = container;
                job.audio_format = *format;
                job.channels = Channels::Mono;
                let path = dir.path().join(format!(
                    "{}-{}-{}.{}",
                    codec.id(),
                    container.id(),
                    format.id(),
                    container.extension()
                ));
                job.output = Some(path.clone());
                export(&job, &Progress::default()).unwrap_or_else(|e| {
                    panic!("{} sound in {}: {e:?}", format.label(), container.label())
                });
                if let Some(info) = codecs_of(&path) {
                    if !info.is_empty() {
                        assert!(
                            info.contains("channels=1"),
                            "mono sound in {}: {info}",
                            container.label()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_sound_only_file_is_made_in_each_format_and_in_mono_or_stereo() {
        let dir = tempfile::tempdir().unwrap();
        let Some(base) = small_job(dir.path()) else {
            return;
        };
        for format in AudioFormat::ALL {
            if !audio_available(format) {
                eprintln!("no {} encoder: skipping", format.label());
                continue;
            }
            for channels in Channels::ALL {
                let mut job = base.clone();
                job.output = None;
                job.audio_format = format;
                job.channels = channels;
                let path = dir.path().join(format!(
                    "sound-{}-{}.{}",
                    format.id(),
                    channels.id(),
                    format.file_extension()
                ));
                job.audio_output = Some(path.clone());
                let progress = Progress::default();
                let report = export(&job, &progress)
                    .unwrap_or_else(|e| panic!("{} sound only: {e:?}", format.label()));
                assert!(report.audio_saved && path.is_file(), "{}", format.label());
                assert_eq!(report.frames, 0, "no pictures were made");
                assert!((progress.fraction() - 1.0).abs() < 1e-9);
                assert!(
                    std::fs::metadata(&path).unwrap().len() > 500,
                    "{} file has content",
                    format.label()
                );
                let leftovers: Vec<_> = std::fs::read_dir(dir.path())
                    .unwrap()
                    .flatten()
                    .filter(|e| e.file_name().to_string_lossy().contains(".exporting."))
                    .collect();
                assert!(leftovers.is_empty(), "{leftovers:?}");
                if let Some(info) = codecs_of(&path) {
                    if !info.is_empty() {
                        let want = match format {
                            AudioFormat::Aac => "aac",
                            AudioFormat::Mp3 => "mp3",
                            AudioFormat::Opus => "opus",
                            AudioFormat::Flac => "flac",
                            AudioFormat::Wav => "pcm_s16le",
                        };
                        assert!(info.contains(&format!("codec_name={want}")), "{info}");
                        assert!(
                            info.contains(&format!("channels={}", channels.count())),
                            "{info}"
                        );
                        assert!(!info.contains("h264"), "no picture in a sound file: {info}");
                    }
                }
            }
        }
    }

    #[test]
    fn video_and_a_separate_sound_file_are_made_together() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut job) = small_job(dir.path()) else {
            return;
        };
        job.limit_seconds = Some(1.0);
        let sound = dir.path().join("out").join("both.m4a");
        job.audio_output = Some(sound.clone());
        let video = job.output.clone().unwrap();
        let report = export(&job, &Progress::default()).expect("export");
        assert!(video.is_file() && sound.is_file());
        assert!(report.audio_saved);
        assert!(report.frames > 0);
        if let Some(info) = codecs_of(&video) {
            if !info.is_empty() {
                assert!(info.contains("h264") && info.contains("aac"), "{info}");
            }
        }
        if let Some(info) = codecs_of(&sound) {
            if !info.is_empty() {
                assert!(!info.contains("h264"), "{info}");
            }
        }
    }

    /// Peak memory of this process so far, in megabytes (Linux).
    fn peak_megabytes() -> f64 {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("VmHWM:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|kb| kb.parse::<f64>().ok())
            })
            .map_or(0.0, |kb| kb / 1024.0)
    }

    /// A slow encoder must make the renderer wait, not pile pictures up in
    /// memory. Run alone: `cargo test slow_encoders -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn slow_encoders_do_not_fill_the_memory_with_waiting_pictures() {
        let dir = tempfile::tempdir().unwrap();
        let Some(base) = small_job(dir.path()) else {
            return;
        };
        let frames: u32 = std::env::var("KANI_MEM_FPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);
        let only = std::env::var("KANI_MEM_CODEC").unwrap_or_default();
        for codec in [
            VideoCodec::H264,
            VideoCodec::H265,
            VideoCodec::Vp9,
            VideoCodec::Av1,
        ] {
            if !only.is_empty() && only != codec.id() {
                continue;
            }
            let encoders: Vec<Encoder> = candidates(codec)
                .into_iter()
                .filter(|e| !e.hardware && has_element(e.element))
                .collect();
            if encoders.is_empty() {
                continue;
            }
            let mut job = base.clone();
            job.width = 1080;
            job.height = 1920;
            job.canvas_width = 1080;
            job.canvas_height = 1920;
            job.fps = FrameRate::new(frames, 1);
            job.encoders = encoders;
            job.container = codec.containers()[0];
            job.audio_format = job
                .container
                .audio_formats()
                .iter()
                .copied()
                .find(|f| audio_available(*f))
                .unwrap();
            let path = dir
                .path()
                .join(format!("big.{}", job.container.extension()));
            job.output = Some(path);
            let before = peak_megabytes();
            let started = std::time::Instant::now();
            export(&job, &Progress::default()).expect("export");
            eprintln!(
                "MEMORY {}: peak {:.0} MB (was {before:.0} MB), {:.1} s",
                codec.name(),
                peak_megabytes(),
                started.elapsed().as_secs_f64()
            );
        }
    }

    #[test]
    fn the_window_is_offered_small_pictures_at_a_steady_pace() {
        let progress = Progress::default();
        let big = vec![200u8; 1080 * 1920 * 4];
        progress.show(1080, 1920, &big, 1, false);
        let first = progress
            .live
            .lock()
            .unwrap()
            .take()
            .expect("the first picture is kept");
        assert_eq!((first.width, first.height), (203, 360));
        assert_eq!(first.rgba.len(), 203 * 360 * 4);
        assert_eq!(first.frame, 1);
        // Straight after, an ordinary picture is skipped but a forced one is kept.
        progress.show(1080, 1920, &big, 2, false);
        assert!(progress.live.lock().unwrap().is_none());
        progress.show(1080, 1920, &big, 3, true);
        assert_eq!(progress.live.lock().unwrap().take().unwrap().frame, 3);
        // A small picture is not enlarged.
        progress.show(100, 50, &vec![0u8; 100 * 50 * 4], 4, true);
        let small = progress.live.lock().unwrap().take().unwrap();
        assert_eq!((small.width, small.height), (100, 50));
    }

    #[test]
    fn only_the_chosen_part_of_the_timeline_is_exported() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut job) = small_job(dir.path()) else {
            return;
        };
        // The timeline is 3 s long; take 1.0 s to 2.5 s.
        job.range_start = Ticks::from_seconds_f64(1.0);
        job.range_end = Some(Ticks::from_seconds_f64(2.5));
        let sound = dir.path().join("out").join("part.m4a");
        job.audio_output = Some(sound.clone());
        let video = job.output.clone().unwrap();
        let report = export(&job, &Progress::default()).expect("export");
        assert_eq!(report.frames, 45, "1.5 s at 30 fps");
        for (path, what) in [(&video, "video"), (&sound, "sound")] {
            if let Some(info) = ffprobe(path, &["-show_entries", "format=duration"]) {
                if let Some(seconds) = info
                    .lines()
                    .find_map(|l| l.strip_prefix("duration="))
                    .and_then(|v| v.parse::<f64>().ok())
                {
                    assert!(
                        (seconds - 1.5).abs() < 0.2,
                        "{what} is {seconds} s, not 1.5"
                    );
                }
            }
        }
        // A part past the end is refused, not made empty.
        job.range_start = Ticks::from_seconds_f64(5.0);
        job.range_end = None;
        match export(&job, &Progress::default()) {
            Err(ExportError::Failed(m)) => assert!(m.contains("empty"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_picture_of_another_shape_is_exported_at_the_chosen_size_with_bars() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut job) = small_job(dir.path()) else {
            return;
        };
        job.limit_seconds = Some(1.0);
        // The project's picture fits inside a square frame: bars above and below.
        job.width = 160;
        job.height = 160;
        job.canvas_width = 160;
        job.canvas_height = 90;
        let report = export(&job, &Progress::default()).expect("export");
        assert!(report.frames > 0);
        let path = job.output.clone().unwrap();
        if let Some(info) = ffprobe(&path, &["-show_entries", "stream=width,height"]) {
            if !info.is_empty() {
                assert!(
                    info.contains("width=160") && info.contains("height=160"),
                    "{info}"
                );
            }
        }
    }

    #[test]
    fn nothing_chosen_is_explained_and_the_old_h264_message_names_the_format() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut job) = small_job(dir.path()) else {
            return;
        };
        job.output = None;
        job.audio_output = None;
        match export(&job, &Progress::default()) {
            Err(ExportError::Failed(m)) => assert!(m.contains("Nothing to export"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn encoders_are_listed_for_every_video_format_with_software_last() {
        for codec in VideoCodec::ALL {
            let list = candidates(codec);
            assert!(!list.is_empty(), "{}", codec.name());
            assert!(list.iter().all(|e| e.codec == codec));
            assert!(!list.last().unwrap().hardware, "software comes last");
            let mut ids: Vec<_> = list.iter().map(|e| e.id).collect();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), list.len(), "no repeats for {}", codec.name());
        }
        // Encoder ids are unique across formats too (settings keep one).
        let mut all: Vec<_> = VideoCodec::ALL
            .iter()
            .flat_map(|c| candidates(*c))
            .map(|e| e.id)
            .collect();
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n);
    }
}
