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

use crate::media::{path_to_uri, sample_to_thumb};
use crate::preview::{find_targets, LayerSource};
use cutroom_compose::{compose, Frame, Placement};
use cutroom_core::state::MediaKind;
use cutroom_platform::{HwBackend, Os};
use cutroom_timeline::{ClipId, FrameRate, MediaId, Ticks, Timeline};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

// ---------------------------------------------------------------- settings

/// How good the picture should be; sets the bit rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    Small,
    Recommended,
    High,
    Maximum,
}

impl Quality {
    pub const ALL: [Quality; 4] = [
        Quality::Small,
        Quality::Recommended,
        Quality::High,
        Quality::Maximum,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Quality::Small => "Small file",
            Quality::Recommended => "Recommended",
            Quality::High => "High",
            Quality::Maximum => "Maximum",
        }
    }

    /// Bits per pixel per frame.
    fn bits_per_pixel(self) -> f64 {
        match self {
            Quality::Small => 0.07,
            Quality::Recommended => 0.12,
            Quality::High => 0.18,
            Quality::Maximum => 0.28,
        }
    }
}

/// Video bit rate, in kilobits per second, for a picture size, frame rate and quality.
pub fn bitrate_kbps(width: u32, height: u32, fps: f64, quality: Quality) -> u32 {
    let bits = f64::from(width) * f64::from(height) * fps * quality.bits_per_pixel();
    ((bits / 1000.0).round() as u32).clamp(1_500, 80_000)
}

/// Picture sizes on offer for a canvas: its own size, then smaller ones that
/// keep its shape (the shorter side 1440, 1080, 720 or 480 — never bigger than
/// the project). Both sides are even, which H.264 needs.
pub fn size_options(width: u32, height: u32) -> Vec<(u32, u32)> {
    let even = |v: f64| ((v.round() as u32).max(2) / 2) * 2;
    let short = width.min(height).max(2);
    let mut out = vec![(even(f64::from(width)), even(f64::from(height)))];
    for side in [1440u32, 1080, 720, 480] {
        if side < short {
            let k = f64::from(side) / f64::from(short);
            let size = (even(f64::from(width) * k), even(f64::from(height) * k));
            if !out.contains(&size) {
                out.push(size);
            }
        }
    }
    out
}

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
    pub hardware: bool,
    /// How many of the element's bit-rate units make a kilobit/s.
    bitrate_unit: u32,
}

const fn enc(
    id: &'static str,
    label: &'static str,
    element: &'static str,
    hardware: bool,
    bitrate_unit: u32,
) -> Encoder {
    Encoder {
        id,
        label,
        element,
        hardware,
        bitrate_unit,
    }
}

const NVENC: &[Encoder] = &[enc("nvenc", "NVIDIA NVENC", "nvh264enc", true, 1)];
const QSV: &[Encoder] = &[enc("qsv", "Intel Quick Sync", "qsvh264enc", true, 1)];
const VAAPI: &[Encoder] = &[
    enc("vaapi-lp", "VA-API (low power)", "vah264lpenc", true, 1),
    enc("vaapi", "VA-API", "vah264enc", true, 1),
    enc(
        "vaapi-old",
        "VA-API (older plug-in)",
        "vaapih264enc",
        true,
        1,
    ),
];
const AMF: &[Encoder] = &[enc("amf", "AMD AMF", "amfh264enc", true, 1)];
const VIDEOTOOLBOX: &[Encoder] = &[enc(
    "videotoolbox",
    "Apple VideoToolbox",
    "vtenc_h264",
    true,
    1,
)];
const WINDOWS_MF: &[Encoder] = &[enc("mf", "Windows Media Foundation", "mfh264enc", true, 1)];
const SOFTWARE: &[Encoder] = &[
    enc("x264", "Software (x264)", "x264enc", false, 1),
    enc(
        "openh264",
        "Software (OpenH264)",
        "openh264enc",
        false,
        1000,
    ),
];

/// Every encoder worth trying on this system, best first: the graphics
/// hardware in the order the platform prefers, then software.
pub fn candidates() -> Vec<Encoder> {
    let os = Os::current();
    let mut out = Vec::new();
    for backend in os.default_backend_order() {
        out.extend_from_slice(match backend {
            HwBackend::Nvenc => NVENC,
            HwBackend::Qsv => QSV,
            HwBackend::Vaapi => VAAPI,
            HwBackend::Amf => AMF,
            HwBackend::VideoToolbox => VIDEOTOOLBOX,
            HwBackend::Software => SOFTWARE,
        });
    }
    if os == Os::Windows {
        out.extend_from_slice(WINDOWS_MF);
    }
    out.extend_from_slice(SOFTWARE);
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
    set_number(element, "bitrate", kbps * encoder.bitrate_unit);
    let gop = (fps.as_f64() * 2.0).round().max(1.0) as u32;
    set_number(element, "key-int-max", gop);
    set_number(element, "gop-size", gop);
    if encoder.element == "x264enc" {
        set_text(element, "speed-preset", "veryfast");
        // Widest compatibility (phones, social sites): no B-frame tricks needed.
        set_number(element, "bframes", 2);
    }
}

/// The video half of the pipeline, from raw pictures to H.264.
fn video_chain(encoder: &Encoder) -> String {
    format!(
        "videoconvert ! capsfilter caps=\"video/x-raw,colorimetry=bt709\" ! {} name=venc ! h264parse",
        encoder.element
    )
}

/// Does this encoder really work on this machine? Encodes a few test frames.
fn probe(encoder: Encoder) -> bool {
    if !has_element(encoder.element) || !has_element("h264parse") {
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

static WORKING: OnceLock<Vec<Encoder>> = OnceLock::new();

/// The encoders that work on this machine, best first. The first call tests
/// them (a second or two), so call it from a worker thread.
pub fn working_encoders() -> &'static [Encoder] {
    WORKING.get_or_init(|| {
        if gst::init().is_err() {
            return Vec::new();
        }
        candidates().into_iter().filter(|e| probe(*e)).collect()
    })
}

fn audio_encoder() -> Option<&'static str> {
    ["avenc_aac", "voaacenc", "fdkaacenc", "faac"]
        .into_iter()
        .find(|name| has_element(name))
}

// ---------------------------------------------------------------- progress

/// Shared between the export thread and the window showing it.
#[derive(Default)]
pub struct Progress {
    pub frames_done: AtomicU64,
    pub frames_total: AtomicU64,
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn fraction(&self) -> f64 {
        let total = self.frames_total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        (self.frames_done.load(Ordering::Relaxed) as f64 / total as f64).min(1.0)
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
    pub width: u32,
    pub height: u32,
    pub fps: FrameRate,
    pub sample_rate: u32,
    pub video_kbps: u32,
    pub audio_kbps: u32,
    /// Try these, in order, until one works.
    pub encoders: Vec<Encoder>,
    pub output: PathBuf,
    /// Export only this many seconds from the start (used by tests; `None` = all of it).
    pub limit_seconds: Option<f64>,
}

impl Job {
    /// How much of the timeline is exported.
    fn duration(&self) -> Ticks {
        let all = self.timeline.duration();
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
}

// -------------------------------------------------------- reading pictures

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
}

impl VideoReader {
    fn open(uri: &str, from: f64, max_edge: u32) -> Result<VideoReader> {
        let playbin = gst::ElementFactory::make("playbin")
            .property("uri", uri)
            .build()
            .map_err(|e| ExportError::Failed(e.to_string()))?;
        let bin = gst::parse::bin_from_description(
            &format!(
                "videoconvert ! videoscale ! capsfilter caps=\"video/x-raw,format=RGBA,width=[1,{m}],height=[1,{m}],pixel-aspect-ratio=1/1\" \
                 ! appsink name=exportsink sync=false max-buffers=3 drop=false",
                m = max_edge
            ),
            true,
        )
        .map_err(|e| ExportError::Failed(e.to_string()))?;
        let sink = bin
            .by_name("exportsink")
            .and_then(|e| e.downcast::<gst_app::AppSink>().ok())
            .ok_or_else(|| ExportError::Failed("no picture sink".into()))?;
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
        })
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
            self.job.width,
            self.job.height,
        ));
        self.texts.push((spec.clone(), frame.clone()));
        if self.texts.len() > 8 {
            self.texts.remove(0);
        }
        frame
    }

    /// The finished picture at timeline time `at`.
    fn picture(&mut self, at: Ticks) -> Vec<u8> {
        let (width, height) = (self.job.width, self.job.height);
        let targets = {
            let resolve = self.resolve();
            find_targets(&self.job.timeline, at, &resolve)
        };
        let mut picked: Vec<(Arc<Frame>, Placement)> = Vec::new();
        let mut used: HashSet<ClipId> = HashSet::new();
        // Topmost first in the list; the compositor wants the bottom first.
        for layer in targets.layers.iter().rev() {
            used.insert(layer.clip);
            let (frame, natural) = match &layer.source {
                LayerSource::Text(spec) => (Some(self.text(spec)), true),
                LayerSource::Still(path) => (self.still(path), false),
                LayerSource::Video(target) => (self.video_frame(layer.clip, target), false),
            };
            if let Some(frame) = frame {
                picked.push((
                    frame,
                    Placement {
                        natural,
                        ..Placement::from(&layer.transform.clamped())
                    },
                ));
            }
        }
        // Free the readers of clips that are no longer showing.
        self.readers.retain(|clip, _| used.contains(clip));
        let started = std::time::Instant::now();
        let canvas = compose(width, height, picked.iter().map(|(f, p)| (&**f, p)));
        self.compose_time += started.elapsed();
        canvas.rgba
    }

    fn video_frame(&mut self, clip: ClipId, target: &crate::preview::Target) -> Option<Arc<Frame>> {
        if self.broken.contains(&clip) {
            return None;
        }
        let t = target.source_time.as_seconds_f64();
        if self.readers.get(&clip).is_some_and(|r| !r.can_reach(t)) {
            self.readers.remove(&clip);
        }
        if !self.readers.contains_key(&clip) {
            let edge = self.job.width.max(self.job.height);
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
        let frame = self.readers.get_mut(&clip)?.frame_at(t);
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

/// Render and save one file with one encoder.
fn run_with(job: &Job, encoder: &Encoder, progress: &Progress, temp: &Path) -> Result<Report> {
    let Some(aac) = audio_encoder() else {
        return fail("No AAC sound encoder is installed (GStreamer needs avenc_aac or voaacenc).");
    };
    if !has_element("mp4mux") {
        return fail("The MP4 muxer (mp4mux) is missing from GStreamer.");
    }
    let unlimited = "max-size-buffers=0 max-size-bytes=0 max-size-time=0";
    let description = format!(
        "mp4mux name=mux ! filesink name=out \
         appsrc name=vsrc ! queue {unlimited} ! {video} ! queue {unlimited} ! mux. \
         appsrc name=asrc ! queue {unlimited} ! audioconvert ! audioresample ! {aac} name=aenc ! aacparse ! queue {unlimited} ! mux.",
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
    set_number(&get("aenc")?, "bitrate", job.audio_kbps * 1000);
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
            let at = frame_time(i, job.fps);
            let t0 = std::time::Instant::now();
            let pixels = renderer.picture(at);
            picture_time += t0.elapsed();
            let mut buffer = gst::Buffer::from_mut_slice(pixels);
            {
                let b = buffer.get_mut().ok_or("buffer")?;
                b.set_pts(gst::ClockTime::from_nseconds(frame_ns(i)));
                b.set_duration(gst::ClockTime::from_nseconds(
                    frame_ns(i + 1).saturating_sub(frame_ns(i)),
                ));
            }
            let t1 = std::time::Instant::now();
            push(&vsrc, &bus, buffer)?;
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
        while audio_block < blocks_total {
            if progress.cancel.load(Ordering::Relaxed) {
                return Err(ExportError::Cancelled);
            }
            push_audio(&asrc, &bus, &mut renderer, audio_block, block, rate)?;
            audio_block += 1;
        }
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
    let samples = renderer.sound(start as f64 / f64::from(rate), block);
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

/// Export the job. Blocks; run it on a worker thread. The file appears at
/// `job.output` only if everything worked (it is written under a temporary
/// name first); a cancelled or failed export leaves nothing behind.
pub fn export(job: &Job, progress: &Progress) -> Result<Report> {
    gst::init().map_err(|e| ExportError::Failed(e.to_string()))?;
    if job.timeline.duration() <= Ticks::ZERO {
        return fail("There is nothing on the timeline to export.");
    }
    if job.encoders.is_empty() {
        return fail("No H.264 video encoder is available. Install GStreamer's x264 plug-in (gst-plugins-ugly) or a hardware encoder plug-in.");
    }
    if let Some(parent) = job.output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let temp = {
        let mut name = job
            .output
            .file_stem()
            .map(|s| s.to_os_string())
            .unwrap_or_default();
        name.push(".exporting.mp4");
        job.output.with_file_name(name)
    };
    let mut notes = Vec::new();
    let mut outcome = fail("No encoder worked.");
    for encoder in &job.encoders {
        let _ = std::fs::remove_file(&temp);
        match run_with(job, encoder, progress, &temp) {
            Ok(mut report) => {
                report.warnings.splice(0..0, notes);
                std::fs::rename(&temp, &job.output)
                    .map_err(|e| format!("could not save {}: {e}", job.output.display()))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_rates_follow_size_rate_and_quality() {
        let a = bitrate_kbps(1080, 1920, 30.0, Quality::Recommended);
        assert!((7_000..8_000).contains(&a), "{a}");
        assert!(bitrate_kbps(1080, 1920, 30.0, Quality::Small) < a);
        assert!(bitrate_kbps(1080, 1920, 30.0, Quality::Maximum) > a);
        assert_eq!(bitrate_kbps(64, 64, 24.0, Quality::Small), 1_500);
        assert_eq!(bitrate_kbps(7680, 4320, 60.0, Quality::Maximum), 80_000);
    }

    #[test]
    fn sizes_keep_the_shape_stay_even_and_never_grow() {
        let sizes = size_options(1080, 1920);
        assert_eq!(sizes[0], (1080, 1920));
        assert!(sizes.contains(&(720, 1280)));
        assert!(sizes.contains(&(480, 852)), "{sizes:?}");
        assert!(!sizes.iter().any(|&(w, _)| w > 1080));
        for (w, h) in size_options(1921, 1081) {
            assert!(w % 2 == 0 && h % 2 == 0, "{w}x{h}");
        }
    }

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
            fps: FrameRate::FPS_30,
            sample_rate: 48_000,
            video_kbps: 1_500,
            audio_kbps: 128,
            encoders: candidates()
                .into_iter()
                .filter(|e| e.element == "x264enc")
                .collect(),
            output: dir.join("out").join("result.mp4"),
            limit_seconds: None,
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
        assert!(job.output.is_file());
        assert!(
            !job.output.with_file_name("result.exporting.mp4").exists(),
            "the temporary file is gone"
        );
        if let Some(info) = ffprobe(
            &job.output,
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
        assert!(!job.output.exists());
        let leftovers: Vec<_> = std::fs::read_dir(job.output.parent().unwrap())
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
            job.output.to_str().unwrap(),
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
            job.output.to_str().unwrap(),
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
                job.output.to_str().unwrap(),
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
            fps: FrameRate::FPS_30,
            sample_rate: 48_000,
            video_kbps: bitrate_kbps(1080, 1920, 30.0, Quality::Recommended),
            audio_kbps: 192,
            encoders: candidates()
                .into_iter()
                .filter(|e| e.element == "x264enc")
                .collect(),
            output: out,
            limit_seconds: None,
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

    #[test]
    fn every_platform_lists_software_last_and_hardware_first() {
        let list = candidates();
        assert!(list.iter().any(|e| e.element == "x264enc"));
        let first_software = list
            .iter()
            .position(|e| !e.hardware && e.element == "x264enc");
        let last_hardware = list.iter().rposition(|e| e.hardware);
        if let (Some(s), Some(h)) = (first_software, last_hardware) {
            assert!(h < s, "hardware encoders come before software");
        }
    }
}
