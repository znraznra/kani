//! Preview playback. Every clip under the playhead has its own GStreamer
//! pipeline (picture and sound, or sound only), driven by the UI's playhead.
//! The pictures of the video clips are handed to the compositor, which stacks
//! them as the clips' transforms say and shows the result.

use crate::media::path_to_uri;
use cutroom_compose::{compose, preview_size, Frame, Placement};
use cutroom_core::state::MediaKind;
use cutroom_timeline::transition::{self, Level};
use cutroom_timeline::{ClipId, MediaId, Ticks, Timeline, TrackKind, Transform};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gtk4::{gdk, glib};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How far a playing pipeline may drift from the playhead before we re-seek.
const DRIFT: Ticks = Ticks(cutroom_timeline::TICKS_PER_SECOND / 4);
/// How far a paused pipeline may be from the playhead before we re-seek.
/// How long the playhead must rest after a scrub before the exact frame is fetched.
const SETTLE_AFTER: Duration = Duration::from_millis(150);
const FRAME_SLACK: Ticks = Ticks(cutroom_timeline::TICKS_PER_SECOND / 60);

fn to_clock(t: Ticks) -> gst::ClockTime {
    gst::ClockTime::from_nseconds((t.as_seconds_f64().max(0.0) * 1_000_000_000.0) as u64)
}

fn from_clock(t: gst::ClockTime) -> Ticks {
    Ticks::from_seconds_f64(t.nseconds() as f64 / 1_000_000_000.0)
}

/// A clip that should be heard or seen right now.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub clip: ClipId,
    pub uri: String,
    /// Time inside the source file that corresponds to the playhead.
    pub source_time: Ticks,
    pub muted: bool,
    /// Loudness, 0.0 to 1.0 (less during a crossfade).
    pub volume: f64,
}

/// What a picture layer shows.
#[derive(Debug, Clone, PartialEq)]
pub enum LayerSource {
    /// A video file, played by the clip's own pipeline.
    Video(Target),
    /// A photo.
    Still(PathBuf),
    /// Words drawn by the app.
    Text(cutroom_timeline::TextSpec),
}

/// One picture on the canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerTarget {
    pub clip: ClipId,
    pub source: LayerSource,
    pub transform: Transform,
}

/// Everything that should be seen or heard at one moment.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Targets {
    /// Picture layers, topmost first (the order of the tracks). A video
    /// layer's own sound plays through its pipeline (muted if its track or the
    /// clip is muted).
    pub layers: Vec<LayerTarget>,
    /// For look-ahead results: when (on the timeline) these targets start.
    pub at: Option<Ticks>,
    /// Sound-only sources, mixed together by the audio device: clips on audio
    /// tracks (a video file there plays as sound only). Clips on muted tracks
    /// are left out.
    pub audio: Vec<Target>,
}

/// Work out what to show and play at `playhead`. Tracks are ordered top to bottom.
pub fn find_targets(
    tl: &Timeline,
    playhead: Ticks,
    resolve: &dyn Fn(MediaId) -> Option<(PathBuf, MediaKind)>,
) -> Targets {
    let mut targets = Targets::default();
    for track in tl.tracks.iter().filter(|t| !t.hidden) {
        let Some(under) = track
            .clips
            .iter()
            .position(|c| c.start <= playhead && playhead < c.end())
        else {
            continue;
        };
        // The clips to show on this track now, topmost first, each with how
        // strongly: normally just the one under the playhead, but around a
        // crossfade it is the two clips on either side of the cut.
        let level = |opacity: f64, volume: f64| Level { opacity, volume };
        let mut shown: Vec<(usize, Level)> = vec![(under, level(1.0, 1.0))];
        let crossfade = transition::mix_at(track, under, playhead)
            .map(|m| (under - 1, under, m))
            .or_else(|| {
                transition::mix_at(track, under + 1, playhead).map(|m| (under, under + 1, m))
            });
        if let Some((first, second, mix)) = crossfade {
            shown.clear();
            // The second clip is drawn over the first.
            if let Some(l) = mix.second {
                shown.push((second, l));
            }
            if let Some(l) = mix.first {
                shown.push((first, l));
            }
        }
        for (index, lv) in shown {
            let clip = &track.clips[index];
            if clip.disabled {
                continue;
            }
            if let (Some(text), TrackKind::Video) = (clip.text_at(playhead), track.kind) {
                let mut transform = clip.transform_at(playhead);
                transform.opacity = (f64::from(transform.opacity) * lv.opacity).round() as i32;
                targets.layers.push(LayerTarget {
                    clip: clip.id,
                    source: LayerSource::Text(text),
                    transform,
                });
                continue;
            }
            let Some((path, media_kind)) = resolve(clip.media) else {
                continue;
            };
            let is_image = media_kind == MediaKind::Image;
            let target = |muted| Target {
                clip: clip.id,
                uri: path_to_uri(&path),
                source_time: (clip.source_in + (playhead - clip.start)).max(Ticks::ZERO),
                muted,
                volume: lv.volume * f64::from(clip.volume) / 1000.0,
            };
            match track.kind {
                TrackKind::Video => {
                    let mut transform = clip.transform_at(playhead);
                    transform.opacity = (f64::from(transform.opacity) * lv.opacity).round() as i32;
                    targets.layers.push(LayerTarget {
                        clip: clip.id,
                        source: if is_image {
                            LayerSource::Still(path.clone())
                        } else {
                            LayerSource::Video(target(track.muted || clip.audio_muted))
                        },
                        transform,
                    });
                }
                // Sound only, never from photos or muted tracks.
                TrackKind::Audio if !is_image && !track.muted && !clip.audio_muted => {
                    targets.audio.push(target(false));
                }
                TrackKind::Audio => {}
            }
        }
    }
    targets
}

/// How far ahead of the playhead the next clip is made ready.
const LOOKAHEAD: Ticks = Ticks(cutroom_timeline::TICKS_PER_SECOND * 6 / 5);

/// What will be on screen and audible at the next moment something starts
/// (a cut), if that is within [`LOOKAHEAD`] of the playhead.
pub fn targets_ahead(
    tl: &Timeline,
    playhead: Ticks,
    resolve: &dyn Fn(MediaId) -> Option<(PathBuf, MediaKind)>,
) -> Targets {
    // When each clip first shows: at its start, or earlier if a crossfade
    // brings its picture in before the cut.
    let next = tl
        .tracks
        .iter()
        .filter(|t| !t.hidden)
        .flat_map(|t| {
            t.clips
                .iter()
                .enumerate()
                .map(move |(i, c)| match transition::window(t, i) {
                    Some((from, _))
                        if transition::mix_at(t, i, from).is_some_and(|m| m.overlapping) =>
                    {
                        from
                    }
                    _ => c.start,
                })
        })
        .filter(|start| *start > playhead && *start - playhead <= LOOKAHEAD)
        .min();
    match next {
        Some(at) => {
            let mut targets = find_targets(tl, at, resolve);
            targets.at = Some(at);
            targets
        }
        None => Targets::default(),
    }
}

/// Decode a photo for the compositor: upright (EXIF), no bigger than 2048 px, with an alpha channel.
pub(crate) fn load_still_frame(path: &std::path::Path) -> Result<Frame, String> {
    let pixbuf = gtk4::gdk_pixbuf::Pixbuf::from_file_at_scale(path, 2048, 2048, true)
        .map_err(|e| e.to_string())?;
    let pixbuf = pixbuf.apply_embedded_orientation().unwrap_or(pixbuf);
    let pixbuf = if pixbuf.has_alpha() {
        pixbuf
    } else {
        pixbuf
            .add_alpha(false, 0, 0, 0)
            .map_err(|e| e.to_string())?
    };
    let (width, height) = (pixbuf.width() as usize, pixbuf.height() as usize);
    let stride = pixbuf.rowstride() as usize;
    let bytes = pixbuf.read_pixel_bytes();
    let mut rgba = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        rgba.extend_from_slice(&bytes[row * stride..row * stride + width * 4]);
    }
    Ok(Frame {
        width: width as u32,
        height: height as u32,
        rgba,
    })
}

/// Most sound-only pipelines kept alive at once.
const MAX_AUDIO_PLAYERS: usize = 16;
/// Most video pipelines kept alive at once.
const MAX_LAYER_PLAYERS: usize = 8;

/// How long before a cut a prepared clip starts running (silently).
const PRERUN: Ticks = Ticks(cutroom_timeline::TICKS_PER_SECOND / 8);

/// Longest side, in pixels, of the pictures decoded for the preview.
const DEFAULT_EDGE: u32 = 960;
/// How long a scrub sound lasts after each move of the playhead.
const BLIP: Duration = Duration::from_millis(90);

#[derive(Default)]
struct Slot {
    frame: Option<Arc<Frame>>,
    serial: u64,
}

type FrameSlot = Arc<Mutex<Slot>>;

fn store_frame(slot: &FrameSlot, sample: &gst::Sample) {
    if let Some(thumb) = crate::media::sample_to_thumb(sample) {
        if let Ok(mut slot) = slot.lock() {
            slot.frame = Some(Arc::new(Frame {
                width: thumb.width as u32,
                height: thumb.height as u32,
                rgba: thumb.rgba,
            }));
            slot.serial += 1;
        }
    }
}

struct Player {
    playbin: gst::Element,
    uri: Option<String>,
    clip: Option<ClipId>,
    /// The pipeline has prerolled and can be seeked.
    ready: bool,
    pending_seek: Option<Ticks>,
    /// The queued seek may land on the nearest keyframe (a scrub in progress).
    pending_fast: bool,
    /// A seek has been sent and has not finished yet.
    seeking: bool,
    seek_started: Option<Instant>,
    /// After scrubbing stops, jump to this exact frame at this time.
    settle: Option<(Instant, Ticks)>,
    want_play: bool,
    audio_enabled: bool,
    has_video: bool,
    /// The newest picture this player has produced (layer players only).
    frames: Option<FrameSlot>,
    /// How many times a clip was loaded from scratch (tests watch this).
    loads: u32,
    /// Where in its file the preloaded clip begins.
    preloaded_at: Option<Ticks>,
    /// Running silently ahead of its cut, so its sound device is already awake.
    early: bool,
    /// Playing a short piece of sound for a scrub, until this moment.
    blip: Option<Instant>,
    /// The sound output element this pipeline uses (see sound.rs).
    sound_out: Option<String>,
}

impl Player {
    /// A pipeline for sound only.
    fn new_audio() -> Option<Player> {
        let fake = gst::ElementFactory::make("fakesink").build().ok()?;
        Player::with_sink(fake, false, None)
    }

    /// A pipeline whose pictures go to the compositor as RGBA frames, at most
    /// `edge` pixels on the longer side.
    fn new_layer(edge: u32) -> Option<Player> {
        let description = format!(
            "videoconvert ! videoscale ! capsfilter caps=\"video/x-raw,format=RGBA,width=[1,{m}],height=[1,{m}],pixel-aspect-ratio=1/1\" \
             ! appsink name=layersink sync=true max-buffers=1 drop=true",
            m = edge
        );
        let bin = gst::parse::bin_from_description(&description, true).ok()?;
        let appsink = bin
            .by_name("layersink")?
            .downcast::<gst_app::AppSink>()
            .ok()?;
        let slot: FrameSlot = Arc::new(Mutex::new(Slot::default()));
        let (a, b) = (slot.clone(), slot.clone());
        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    if let Ok(sample) = sink.pull_sample() {
                        store_frame(&a, &sample);
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .new_preroll(move |sink| {
                    if let Ok(sample) = sink.pull_preroll() {
                        store_frame(&b, &sample);
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        Player::with_sink(bin.upcast(), true, Some(slot))
    }

    fn with_sink(sink: gst::Element, has_video: bool, frames: Option<FrameSlot>) -> Option<Player> {
        let playbin = gst::ElementFactory::make("playbin").build().ok()?;
        playbin.set_property("video-sink", &sink);
        // Name the sound output ourselves (see sound.rs).
        let sound_out = crate::sound::make_sink().map(|audio| {
            playbin.set_property("audio-sink", &audio);
            audio
                .factory()
                .map(|f| f.name().to_string())
                .unwrap_or_default()
        });
        let player = Player {
            sound_out,
            playbin,
            uri: None,
            clip: None,
            ready: false,
            pending_seek: None,
            pending_fast: false,
            seeking: false,
            seek_started: None,
            settle: None,
            want_play: false,
            audio_enabled: true,
            has_video,
            frames,
            loads: 0,
            preloaded_at: None,
            early: false,
            blip: None,
        };
        player.apply_flags();
        Some(player)
    }

    /// The newest picture and its serial number (bumped with every new frame).
    fn latest_frame(&self) -> Option<(Arc<Frame>, u64)> {
        let slot = self.frames.as_ref()?.lock().ok()?;
        slot.frame.clone().map(|f| (f, slot.serial))
    }

    /// Forget the picture of the previous clip.
    fn clear_frame(&self) {
        if let Some(slot) = &self.frames {
            if let Ok(mut slot) = slot.lock() {
                slot.frame = None;
            }
        }
    }

    /// Send sound to a fake sink (tests must not open a sound device).
    fn use_fake_audio(&mut self) {
        self.sound_out = None;
        if let Ok(fake) = gst::ElementFactory::make("fakesink")
            .property("sync", true)
            .build()
        {
            self.playbin.set_property("audio-sink", &fake);
        }
    }

    fn apply_flags(&self) {
        let flags = match (self.has_video, self.audio_enabled) {
            (true, true) => "video+audio+soft-volume",
            (true, false) => "video",
            (false, true) => "audio+soft-volume",
            (false, false) => "soft-volume",
        };
        self.playbin.set_property_from_str("flags", flags);
    }

    fn load(&mut self, target: &Target, play: bool) {
        if self.loads == 0 {
            crate::startup::step(&format!("preview: first file opened ({})", target.uri));
        }
        self.loads += 1;
        self.clear_frame();
        self.preloaded_at = None;
        self.early = false;
        let _ = self.playbin.set_state(gst::State::Null);
        self.playbin.set_property("uri", &target.uri);
        self.apply_flags();
        let _ = self.playbin.set_state(gst::State::Paused);
        self.uri = Some(target.uri.clone());
        self.clip = Some(target.clip);
        self.ready = false;
        self.seeking = false;
        self.settle = None;
        self.pending_fast = false;
        self.pending_seek = Some(target.source_time);
        self.want_play = play;
        self.set_muted(target.muted);
        self.set_volume(target.volume);
    }

    fn set_muted(&self, muted: bool) {
        self.playbin.set_property("mute", muted);
    }

    fn set_volume(&self, volume: f64) {
        self.playbin.set_property("volume", volume.clamp(0.0, 2.0));
    }

    fn position(&self) -> Option<Ticks> {
        self.playbin
            .query_position::<gst::ClockTime>()
            .map(from_clock)
    }

    /// Seek to the exact frame.
    fn seek(&mut self, to: Ticks) {
        self.seek_with(to, true);
    }

    /// While the user drags the playhead only the nearest keyframe is needed,
    /// which is much quicker; the exact frame follows when the dragging stops.
    fn scrub_to(&mut self, to: Ticks) {
        self.seek_with(to, false);
        self.settle = Some((Instant::now() + SETTLE_AFTER, to));
    }

    /// Only one seek is in flight at a time. A newer request replaces any
    /// queued one, so a fast drag does not pile up work.
    fn seek_with(&mut self, to: Ticks, accurate: bool) {
        if !self.ready {
            self.pending_seek = Some(to);
            self.pending_fast = !accurate;
            return;
        }
        let stuck = self
            .seek_started
            .is_some_and(|t| t.elapsed() > Duration::from_millis(1500));
        if self.seeking && !stuck {
            self.pending_seek = Some(to);
            self.pending_fast = !accurate;
            return;
        }
        let flags = gst::SeekFlags::FLUSH
            | if accurate {
                gst::SeekFlags::ACCURATE
            } else {
                gst::SeekFlags::KEY_UNIT
            };
        if self.playbin.seek_simple(flags, to_clock(to)).is_ok() {
            self.seeking = true;
            self.seek_started = Some(Instant::now());
        } else {
            self.pending_seek = Some(to);
            self.pending_fast = !accurate;
        }
    }

    fn set_playing(&mut self, play: bool) {
        self.want_play = play;
        if self.ready {
            let state = if play {
                gst::State::Playing
            } else {
                gst::State::Paused
            };
            let _ = self.playbin.set_state(state);
        }
    }

    fn release(&mut self) {
        self.early = false;
        if self.clip.is_some() {
            let _ = self.playbin.set_state(gst::State::Paused);
            self.want_play = false;
            self.clip = None;
        }
    }

    /// Get a clip ready (loaded, paused, at its start) before it is needed.
    fn preload(&mut self, target: &Target) {
        if self.clip == Some(target.clip) && self.uri.as_deref() == Some(&target.uri) {
            return;
        }
        self.load(target, false);
        self.preloaded_at = Some(target.source_time);
    }

    /// Start a prepared clip a little before its cut, silently and from just
    /// early enough that it is exactly at its beginning when the cut arrives.
    /// Starting a pipeline (and its sound device) takes a few tens of
    /// milliseconds; doing it ahead of time keeps the cut seamless.
    fn start_early(&mut self, remaining: Ticks) {
        let Some(begin) = self.preloaded_at else {
            return;
        };
        if self.early || self.want_play || !self.ready || begin < remaining {
            return;
        }
        self.set_muted(true);
        self.seek(begin - remaining);
        self.set_playing(true);
        self.early = true;
    }

    /// Follow `target`: load it if it is a different clip, otherwise keep the
    /// pipeline within reach of the playhead.
    fn follow(&mut self, target: &Target, playing: bool, playhead_moved: bool, scrub_audio: bool) {
        if self.uri.as_deref() != Some(&target.uri) {
            self.load(target, playing);
            return;
        }
        // Another clip cut from the same file (a split, or a trimmed piece): keep
        // the pipeline and just move it, instead of tearing it down and reloading.
        self.clip = Some(target.clip);
        self.set_muted(target.muted);
        self.set_volume(target.volume);
        self.early = false;
        if playing {
            self.blip = None;
            if !self.want_play {
                // A pipeline prepared in advance already sits at the start.
                let off = self
                    .position()
                    .is_none_or(|p| (p - target.source_time).abs() > FRAME_SLACK);
                if off {
                    self.seek(target.source_time);
                }
                self.set_playing(true);
            } else if let Some(position) = self.position() {
                let drift = (position - target.source_time).abs();
                if drift > DRIFT {
                    self.seek(target.source_time);
                }
            }
        } else {
            if self.want_play && self.blip.is_none() {
                self.set_playing(false);
            }
            if playhead_moved {
                let off = self
                    .position()
                    .map_or(true, |p| (p - target.source_time).abs() > FRAME_SLACK);
                if off {
                    self.scrub_to(target.source_time);
                    if scrub_audio && self.audio_enabled && !target.muted {
                        // A short piece of sound, so dragging the playhead can be heard.
                        self.blip = Some(Instant::now() + BLIP);
                        self.set_playing(true);
                    }
                }
            }
        }
    }

    /// Put a new sound output on this pipeline and load its clip again.
    fn reset_sound_output(&mut self) {
        let _ = self.playbin.set_state(gst::State::Null);
        if let Some(audio) = crate::sound::make_sink() {
            self.playbin.set_property("audio-sink", &audio);
        }
        let (uri, clip) = (self.uri.clone(), self.clip);
        let (play, position) = (self.want_play, self.pending_seek.or(self.position()));
        if let (Some(uri), Some(clip)) = (uri, clip) {
            let target = Target {
                clip,
                uri,
                source_time: position.unwrap_or(Ticks::ZERO),
                muted: false,
                volume: 1.0,
            };
            self.load(&target, play);
        }
    }

    /// Handle the pipeline's messages. Returns an error text if it failed,
    /// and whether it came from the sound side.
    fn poll(&mut self) -> Option<(String, bool)> {
        if self.blip.is_some_and(|end| Instant::now() >= end) {
            self.blip = None;
            self.set_playing(false);
        }
        if let Some((at, to)) = self.settle {
            if Instant::now() >= at {
                self.settle = None;
                self.seek(to);
            }
        }
        let bus = self.playbin.bus()?;
        let mut error = None;
        while let Some(message) = bus.pop() {
            match message.view() {
                gst::MessageView::AsyncDone(_) => {
                    self.ready = true;
                    self.seeking = false;
                    if let Some(to) = self.pending_seek.take() {
                        let accurate = !std::mem::take(&mut self.pending_fast);
                        self.seek_with(to, accurate);
                    }
                    if self.want_play {
                        let _ = self.playbin.set_state(gst::State::Playing);
                    }
                }
                gst::MessageView::Eos(_) => self.want_play = false,
                gst::MessageView::Error(e) => {
                    let sound = crate::sound::is_sound_error(e.src());
                    let text = format!("{} ({})", e.error(), e.debug().unwrap_or_default());
                    eprintln!(
                        "kani: playback error from {}: {text}",
                        e.src().map_or("?".into(), |s| s.path_string().to_string())
                    );
                    error = Some((text, sound));
                }
                _ => {}
            }
        }
        error
    }

    fn shutdown(&self) {
        let _ = self.playbin.set_state(gst::State::Null);
    }
}

impl Drop for Player {
    /// GStreamer wants elements in the NULL state before they are dropped.
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// One picture layer of the current moment.
struct CurrentLayer {
    clip: ClipId,
    still: Option<PathBuf>,
    text: Option<cutroom_timeline::TextSpec>,
    transform: Transform,
}

/// What the last rendered picture was made from, to skip identical redraws.
#[derive(Debug, Clone, PartialEq)]
struct RenderKey {
    size: (u32, u32),
    layers: Vec<(ClipId, u64, Transform)>,
}

pub struct Preview {
    /// Pipelines that play video clips, one per clip that is showing or about to.
    layers: Vec<Player>,
    /// Sound-only pipelines, one per clip that is playing or about to.
    audio: Vec<Player>,
    /// The picture layers under the playhead, bottom first.
    current: Vec<CurrentLayer>,
    /// Clips that start in the next moment (loaded ahead of time).
    upcoming: Vec<ClipId>,
    /// Decoded photos, most recently used last.
    stills: Vec<(PathBuf, Arc<Frame>)>,
    /// Recently drawn text pictures, by what they were drawn from and at what size.
    texts: Vec<((cutroom_timeline::TextSpec, (u32, u32)), Arc<Frame>)>,
    last_render: Option<RenderKey>,
    /// Test mode: nothing may open a real sound device.
    fake_audio: bool,
    /// The last playback problem, for the status bar.
    pub problem: Option<String>,
    /// Longest side of the pictures decoded and drawn for the preview.
    edge: u32,
    /// Play a short piece of sound while the playhead is dragged.
    scrub_audio: bool,
    /// Put the picture together on the graphics chip (when it can).
    gpu_wanted: bool,
    gpu: Option<crate::preview_gpu::PreviewGpu>,
    /// Recently bent pictures (see [`Preview::bent`]).
    bends: Vec<Bend>,
    /// The picture the chip is drawing now (live player only).
    in_flight: Option<Sent>,
    /// How the chip has been doing since playback began.
    stats: ChipStats,
}

/// A picture handed to the chip and not yet back.
struct Sent {
    key: RenderKey,
    size: (u32, u32),
    /// Kept so the processor can draw it if the chip cannot.
    layers: Vec<(Arc<Frame>, Placement)>,
}

/// Counts for the console, so slowness can be pinned on a step.
#[derive(Default)]
struct ChipStats {
    pictures: u32,
    layers: u32,
    draw: Duration,
    read: Duration,
    slowest: Duration,
    /// Times a new picture had to wait because the chip was still busy.
    waited: u32,
}

impl ChipStats {
    fn report(&mut self) {
        if self.pictures >= 20 {
            let n = f64::from(self.pictures);
            let ms = |d: Duration| d.as_secs_f64() * 1000.0;
            eprintln!(
                "kani: preview on the graphics chip: {} pictures, {:.1} layers each; per picture {:.1} ms uploading and drawing, {:.1} ms reading back; slowest {:.1} ms; {} times a newer picture waited for the chip",
                self.pictures,
                f64::from(self.layers) / n,
                ms(self.draw) / n,
                ms(self.read) / n,
                ms(self.slowest),
                self.waited
            );
        }
        *self = ChipStats::default();
    }
}

/// A bent copy of a picture, kept while it is still wanted.
struct Bend {
    source: Arc<Frame>,
    placement: Placement,
    size: (u32, u32),
    bent: Arc<Frame>,
    straight: Placement,
}

impl Preview {
    pub fn new() -> Preview {
        Preview {
            layers: Vec::new(),
            audio: Vec::new(),
            current: Vec::new(),
            upcoming: Vec::new(),
            stills: Vec::new(),
            texts: Vec::new(),
            last_render: None,
            fake_audio: false,
            problem: None,
            edge: DEFAULT_EDGE,
            scrub_audio: false,
            gpu_wanted: false,
            gpu: None,
            bends: Vec::new(),
            in_flight: None,
            stats: ChipStats::default(),
        }
    }

    /// Apply the preview settings. A new picture size starts the pipelines
    /// again (they are made for one size); the next sync fills them.
    pub fn configure(&mut self, edge: u32, scrub_audio: bool, gpu: bool) {
        let edge = edge.clamp(240, 4096);
        if edge != self.edge {
            self.edge = edge;
            self.layers.clear();
            self.texts.clear();
            self.invalidate();
        }
        self.scrub_audio = scrub_audio;
        if gpu != self.gpu_wanted {
            self.gpu_wanted = gpu;
            self.gpu = None;
            self.in_flight = None;
            self.invalidate();
        }
    }

    /// `frame` placed by `placement` as a picture that needs no bending: the
    /// same picture for plain layers, else a bent copy (reused while the
    /// picture, the bend and the size stay the same).
    fn bent(
        &mut self,
        frame: &Arc<Frame>,
        placement: &Placement,
        size: (u32, u32),
    ) -> (Arc<Frame>, Placement) {
        if !placement.has_distort() {
            return (frame.clone(), *placement);
        }
        if let Some(hit) = self
            .bends
            .iter()
            .find(|b| Arc::ptr_eq(&b.source, frame) && b.placement == *placement && b.size == size)
        {
            return (hit.bent.clone(), hit.straight);
        }
        match cutroom_compose::prebend(frame, placement, size.0, size.1) {
            Some((bent, straight)) => {
                let bent = Arc::new(bent);
                self.bends.push(Bend {
                    source: frame.clone(),
                    placement: *placement,
                    size,
                    bent: bent.clone(),
                    straight,
                });
                if self.bends.len() > 8 {
                    self.bends.remove(0);
                }
                (bent, straight)
            }
            None => (
                frame.clone(),
                Placement {
                    pin: [0.0; 8],
                    mesh: [0.0; 32],
                    ..*placement
                },
            ),
        }
    }

    /// Is the picture being put together on the graphics chip right now?
    #[cfg(test)]
    pub fn gpu_in_use(&self) -> bool {
        self.gpu_wanted && self.gpu.is_some()
    }

    #[cfg(test)]
    pub fn for_tests() -> Preview {
        Preview {
            fake_audio: true,
            ..Preview::new()
        }
    }

    /// Make the next [`Preview::render`] draw again (a new project, a new canvas).
    pub fn invalidate(&mut self) {
        self.last_render = None;
    }

    fn new_player(&self, layer: bool) -> Option<Player> {
        let mut player = if layer {
            Player::new_layer(self.edge)
        } else {
            Player::new_audio()
        }?;
        if self.fake_audio {
            player.use_fake_audio();
        }
        Some(player)
    }

    /// The pipeline that should handle `target`: the one that already has the
    /// clip, else an idle one that has the same file (no reload), else any idle
    /// one, else a new one.
    fn player_for(&mut self, layer: bool, target: &Target) -> Option<&mut Player> {
        let pool = if layer { &self.layers } else { &self.audio };
        let index = pool
            .iter()
            .position(|p| p.clip == Some(target.clip))
            .or_else(|| {
                pool.iter()
                    .position(|p| p.clip.is_none() && p.uri.as_deref() == Some(&target.uri))
            })
            .or_else(|| pool.iter().position(|p| p.clip.is_none()));
        let index = match index {
            Some(i) => i,
            None if pool.len()
                < if layer {
                    MAX_LAYER_PLAYERS
                } else {
                    MAX_AUDIO_PLAYERS
                } =>
            {
                let player = self.new_player(layer)?;
                let pool = if layer {
                    &mut self.layers
                } else {
                    &mut self.audio
                };
                pool.push(player);
                pool.len() - 1
            }
            None => return None,
        };
        let pool = if layer {
            &mut self.layers
        } else {
            &mut self.audio
        };
        pool.get_mut(index)
    }

    /// Point the pipelines at what is under the playhead, and while playing
    /// get whatever starts in the next moment ready (and running silently just
    /// before its cut), so cuts do not stutter.
    pub fn sync(
        &mut self,
        tl: &Timeline,
        resolve: &dyn Fn(MediaId) -> Option<(PathBuf, MediaKind)>,
        playhead: Ticks,
        playing: bool,
        playhead_moved: bool,
    ) {
        let scrub_audio = self.scrub_audio;
        let targets = find_targets(tl, playhead, resolve);
        let ahead = if playing {
            targets_ahead(tl, playhead, resolve)
        } else {
            Targets::default()
        };
        let video_of = |t: &Targets| -> Vec<Target> {
            t.layers
                .iter()
                .filter_map(|l| match &l.source {
                    LayerSource::Video(target) => Some(target.clone()),
                    LayerSource::Still(_) | LayerSource::Text(_) => None,
                })
                .collect()
        };
        let (now_video, ahead_video) = (video_of(&targets), video_of(&ahead));

        self.current = targets
            .layers
            .iter()
            .rev()
            .map(|l| CurrentLayer {
                clip: l.clip,
                still: match &l.source {
                    LayerSource::Still(path) => Some(path.clone()),
                    LayerSource::Video(_) | LayerSource::Text(_) => None,
                },
                text: match &l.source {
                    LayerSource::Text(t) => Some(t.clone()),
                    _ => None,
                },
                transform: l.transform,
            })
            .collect();
        self.upcoming = ahead_video
            .iter()
            .map(|t| t.clip)
            .chain(ahead.audio.iter().map(|t| t.clip))
            .filter(|c| {
                !self.current.iter().any(|l| l.clip == *c)
                    && !targets.audio.iter().any(|t| t.clip == *c)
            })
            .collect();

        for (layer, now, ahead_t, tracks) in [
            (true, &now_video, &ahead_video, ()),
            (false, &targets.audio, &ahead.audio, ()),
        ]
        .map(|(l, n, a, t)| (l, n, a, t))
        {
            let _ = tracks;
            let pool = if layer {
                &mut self.layers
            } else {
                &mut self.audio
            };
            // Free the pipelines whose clip is no longer needed.
            for player in pool.iter_mut() {
                if let Some(clip) = player.clip {
                    if !now.iter().any(|t| t.clip == clip)
                        && !ahead_t.iter().any(|t| t.clip == clip)
                    {
                        player.release();
                    }
                }
            }
            for target in now {
                if let Some(player) = self.player_for(layer, target) {
                    player.follow(target, playing, playhead_moved, scrub_audio);
                }
            }
            for target in ahead_t
                .iter()
                .filter(|a| !now.iter().any(|t| t.clip == a.clip))
            {
                let remaining = ahead.at.map(|at| at - playhead);
                if let Some(player) = self.player_for(layer, target) {
                    player.preload(target);
                    if let Some(remaining) = remaining.filter(|r| playing && *r <= PRERUN) {
                        player.start_early(remaining);
                    }
                }
            }
        }
    }

    fn all_players(&mut self) -> impl Iterator<Item = &mut Player> {
        self.layers.iter_mut().chain(self.audio.iter_mut())
    }

    /// Call regularly: finishes seeks and notices failures.
    pub fn poll(&mut self) {
        let mut problem = None;
        let fake = self.fake_audio;
        for player in self.all_players() {
            if let Some((error, sound)) = player.poll() {
                if sound && !fake && player.audio_enabled && player.uri.is_some() {
                    // The sound output failed: try the next one before giving up on sound.
                    let failed = player.sound_out.clone();
                    let now = crate::sound::current_name();
                    let next = if failed.is_some() && failed == now {
                        crate::sound::next_after_failure(&error)
                    } else {
                        now
                    };
                    if next.is_some() {
                        player.sound_out = next;
                        player.reset_sound_output();
                        continue;
                    }
                }
                if player.audio_enabled && player.uri.is_some() {
                    // Often "no sound device": keep the picture, drop the sound.
                    player.audio_enabled = false;
                    let (uri, clip) = (player.uri.clone(), player.clip);
                    let (play, position) = (player.want_play, player.pending_seek);
                    if let (Some(uri), Some(clip)) = (uri, clip) {
                        let target = Target {
                            clip,
                            uri,
                            source_time: position.unwrap_or(Ticks::ZERO),
                            muted: true,
                            volume: 1.0,
                        };
                        player.load(&target, play);
                    }
                    problem = Some(format!("Sound unavailable: {error}"));
                } else {
                    problem = Some(format!("Playback problem: {error}"));
                }
            }
        }
        if problem.is_some() {
            self.problem = problem;
        }
    }

    pub fn pause(&mut self) {
        self.stats.report();
        for player in self.all_players() {
            player.set_playing(false);
        }
    }

    pub fn shutdown(&mut self) {
        for player in self.all_players() {
            player.shutdown();
        }
    }

    /// A photo's frame, decoded once and kept for a while.
    fn text_frame(&mut self, spec: &cutroom_timeline::TextSpec, size: (u32, u32)) -> Arc<Frame> {
        if let Some(i) = self
            .texts
            .iter()
            .position(|((s, z), _)| s == spec && *z == size)
        {
            let entry = self.texts.remove(i);
            let frame = entry.1.clone();
            self.texts.push(entry);
            return frame;
        }
        let frame = Arc::new(crate::text_render::render(spec, size.0, size.1));
        self.texts.push(((spec.clone(), size), frame.clone()));
        if self.texts.len() > 8 {
            self.texts.remove(0);
        }
        frame
    }

    fn still_frame(&mut self, path: &std::path::Path) -> Option<Arc<Frame>> {
        if let Some(i) = self.stills.iter().position(|(p, _)| p == path) {
            let entry = self.stills.remove(i);
            let frame = entry.1.clone();
            self.stills.push(entry);
            return Some(frame);
        }
        match load_still_frame(path) {
            Ok(frame) => {
                let frame = Arc::new(frame);
                self.stills.push((path.to_path_buf(), frame.clone()));
                if self.stills.len() > 6 {
                    self.stills.remove(0);
                }
                Some(frame)
            }
            Err(error) => {
                self.problem = Some(format!("Could not show {}: {error}", path.display()));
                None
            }
        }
    }

    /// Stack the pictures under the playhead into the canvas, if anything
    /// changed since the last time. `override_transform` previews a change that
    /// is still being dragged. `None` means "keep showing what is on screen":
    /// nothing changed, or a layer's first frame has not arrived yet.
    pub fn render(
        &mut self,
        canvas: (u32, u32),
        override_transform: Option<(ClipId, Transform)>,
    ) -> Option<gdk::Texture> {
        // On the chip the picture comes back a moment later, and the window
        // goes on meanwhile; otherwise it is drawn right here.
        let canvas = if self.gpu_wanted {
            self.compose_live(canvas, override_transform)?
        } else {
            self.compose_now(canvas, override_transform)?
        };
        let (width, height) = (canvas.width, canvas.height);
        let bytes = glib::Bytes::from_owned(canvas.rgba);
        Some(
            gdk::MemoryTexture::new(
                width as i32,
                height as i32,
                gdk::MemoryFormat::R8g8b8a8,
                &bytes,
                width as usize * 4,
            )
            .upcast(),
        )
    }

    /// The layers to draw now, bottom first, with what they were made from;
    /// `None` while a clip's first picture is awaited or nothing changed.
    fn pick_layers(
        &mut self,
        canvas: (u32, u32),
        override_transform: Option<(ClipId, Transform)>,
    ) -> Option<((u32, u32), RenderKey, Vec<(Arc<Frame>, Placement)>)> {
        let size = preview_size(canvas.0.max(2), canvas.1.max(2), self.edge);
        let mut picked: Vec<(Arc<Frame>, Placement, (ClipId, u64, Transform))> = Vec::new();
        let mut waiting = false;
        let current: Vec<(
            ClipId,
            Option<PathBuf>,
            Option<cutroom_timeline::TextSpec>,
            Transform,
        )> = self
            .current
            .iter()
            .map(|l| (l.clip, l.still.clone(), l.text.clone(), l.transform))
            .collect();
        for (clip, still, text, transform) in current {
            let transform = match override_transform {
                Some((id, t)) if id == clip => t.clamped(),
                _ => transform,
            };
            let found = match (&still, &text) {
                (_, Some(spec)) => {
                    let serial = {
                        use std::hash::{Hash, Hasher};
                        let mut h = std::collections::hash_map::DefaultHasher::new();
                        format!("{spec:?}").hash(&mut h);
                        h.finish()
                    };
                    Some((self.text_frame(spec, size), serial))
                }
                (Some(path), None) => self.still_frame(path).map(|f| (f, 0)),
                (None, None) => self
                    .layers
                    .iter()
                    .find(|p| p.clip == Some(clip))
                    .and_then(|p| p.latest_frame()),
            };
            match found {
                Some((frame, serial)) => picked.push((
                    frame,
                    Placement {
                        // Words keep their own size; pictures are fitted.
                        natural: text.is_some(),
                        ..Placement::from(&transform)
                    },
                    (clip, serial, transform),
                )),
                None => waiting = true,
            }
        }
        if waiting {
            return None;
        }
        let key = RenderKey {
            size,
            layers: picked.iter().map(|(_, _, k)| *k).collect(),
        };
        if self.last_render.as_ref() == Some(&key) {
            return None;
        }
        let layers = picked.into_iter().map(|(f, p, _)| (f, p)).collect();
        Some((size, key, layers))
    }

    /// Draw with the processor (bent layers come from a small cache, so a
    /// bent photo or text is not bent again for every picture of the video
    /// around it).
    fn cpu_compose(
        &mut self,
        layers: &[(Arc<Frame>, Placement)],
        size: (u32, u32),
    ) -> cutroom_compose::Canvas {
        let layers: Vec<(Arc<Frame>, Placement)> =
            layers.iter().map(|(f, p)| self.bent(f, p, size)).collect();
        compose(size.0, size.1, layers.iter().map(|(f, p)| (&**f, p)))
    }

    /// The picture itself, as pixels, waiting for the chip if it is used
    /// (see [`Preview::render`] for the live player, which does not wait).
    pub fn compose_now(
        &mut self,
        canvas: (u32, u32),
        override_transform: Option<(ClipId, Transform)>,
    ) -> Option<cutroom_compose::Canvas> {
        let (size, key, layers) = self.pick_layers(canvas, override_transform)?;
        let on_card = if self.gpu_wanted {
            let gpu = self
                .gpu
                .get_or_insert_with(crate::preview_gpu::PreviewGpu::start);
            match gpu.compose(size, layers.clone()) {
                Ok(Some(rgba)) => cutroom_compose::Canvas::from_rgba(size.0, size.1, rgba),
                Ok(None) => None,
                Err(why) => {
                    self.gpu_failed(&why);
                    None
                }
            }
        } else {
            None
        };
        let canvas = match on_card {
            Some(c) => c,
            None => self.cpu_compose(&layers, size),
        };
        self.last_render = Some(key);
        Some(canvas)
    }

    fn gpu_failed(&mut self, why: &str) {
        eprintln!("kani: the preview could not use the graphics chip: {why}");
        self.problem = Some(format!(
            "The preview could not use the graphics chip ({why}); the processor does it now."
        ));
        self.gpu_wanted = false;
        self.gpu = None;
        self.in_flight = None;
        self.last_render = None;
    }

    /// The live player's picture with the chip: collect the one that came
    /// back, and send the newest layers for the next. Never waits.
    fn compose_live(
        &mut self,
        canvas: (u32, u32),
        override_transform: Option<(ClipId, Transform)>,
    ) -> Option<cutroom_compose::Canvas> {
        use crate::preview_gpu::{Reply, STUCK_AFTER};
        let gpu = self
            .gpu
            .get_or_insert_with(crate::preview_gpu::PreviewGpu::start);
        let mut finished = None;
        match gpu.poll() {
            Some(Reply::Picture(rgba, timing)) => {
                if let Some(sent) = self.in_flight.take() {
                    self.stats.pictures += 1;
                    self.stats.layers += sent.layers.len() as u32;
                    self.stats.draw += timing.draw;
                    self.stats.read += timing.read;
                    self.stats.slowest = self.stats.slowest.max(timing.draw + timing.read);
                    finished = cutroom_compose::Canvas::from_rgba(sent.size.0, sent.size.1, rgba);
                    self.last_render = Some(sent.key);
                }
            }
            Some(Reply::Unsupported) => {
                // Something the chip cannot place: the processor draws this one.
                if let Some(sent) = self.in_flight.take() {
                    finished = Some(self.cpu_compose(&sent.layers, sent.size));
                    self.last_render = Some(sent.key);
                }
            }
            Some(Reply::Failed(why)) => {
                self.gpu_failed(&why);
                return None;
            }
            None => {
                if gpu.busy_for().is_some_and(|t| t > STUCK_AFTER) {
                    self.gpu_failed("the graphics chip took too long");
                    return None;
                }
            }
        }
        let busy = self.gpu.as_ref().is_some_and(|g| g.busy_for().is_some());
        if let Some((size, key, layers)) = self.pick_layers(canvas, override_transform) {
            if busy {
                self.stats.waited += 1;
            } else if let Some(gpu) = self.gpu.as_ref() {
                match gpu.submit(size, layers.clone()) {
                    Ok(()) => self.in_flight = Some(Sent { key, size, layers }),
                    Err(why) => self.gpu_failed(&why),
                }
            }
        }
        finished
    }

    // ---- what tests look at

    /// Make the next render draw even if nothing changed.
    #[cfg(test)]
    pub fn forget_last_render(&mut self) {
        self.last_render = None;
    }

    /// The top video layer's pipeline: (its clip, prepared, position).
    #[cfg(test)]
    fn video_state(&self) -> Option<(Option<ClipId>, bool, Option<Ticks>)> {
        let top = self
            .current
            .iter()
            .rev()
            .find(|l| l.still.is_none() && l.text.is_none())?;
        self.layers
            .iter()
            .find(|p| p.clip == Some(top.clip))
            .map(|p| (p.clip, p.ready, p.position()))
    }

    /// The pipeline loaded for a clip that starts soon: (clip, prepared, position).
    #[cfg(test)]
    fn standby_state(&self) -> Option<(Option<ClipId>, bool, Option<Ticks>)> {
        self.layers
            .iter()
            .find(|p| p.clip.is_some_and(|c| self.upcoming.contains(&c)))
            .map(|p| (p.clip, p.ready, p.position()))
    }

    /// Is the soon-to-start clip already running silently? (running early, muted)
    #[cfg(test)]
    fn standby_running_early(&self) -> Option<(bool, bool)> {
        self.layers
            .iter()
            .find(|p| p.clip.is_some_and(|c| self.upcoming.contains(&c)))
            .map(|p| (p.early, p.playbin.property::<bool>("mute")))
    }

    /// Pipelines loaded from scratch so far, over all of them.
    #[cfg(test)]
    fn total_loads(&self) -> u32 {
        self.layers
            .iter()
            .chain(self.audio.iter())
            .map(|p| p.loads)
            .sum()
    }

    /// Clips that currently have a sound-only pipeline assigned.
    #[cfg(test)]
    fn audio_clips(&self) -> Vec<ClipId> {
        let mut clips: Vec<ClipId> = self.audio.iter().filter_map(|p| p.clip).collect();
        clips.sort();
        clips
    }

    /// Clips that currently have a picture pipeline assigned.
    #[cfg(test)]
    fn layer_clips(&self) -> Vec<ClipId> {
        let mut clips: Vec<ClipId> = self.layers.iter().filter_map(|p| p.clip).collect();
        clips.sort();
        clips
    }

    /// Is a photo one of the current layers?
    #[cfg(test)]
    pub fn shows_still(&self) -> bool {
        self.current.iter().any(|l| l.still.is_some())
    }

    /// Does the layer for this clip have a picture yet?
    #[cfg(test)]
    fn has_frame(&self, clip: ClipId) -> bool {
        self.layers
            .iter()
            .find(|p| p.clip == Some(clip))
            .is_some_and(|p| p.latest_frame().is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::test_media::*;
    use cutroom_timeline::{apply, Edit};
    use tempfile::tempdir;

    fn s(t: f64) -> Ticks {
        Ticks::from_seconds_f64(t)
    }

    fn add_track(
        tl: &mut Timeline,
        kind: TrackKind,
        name: &str,
        index: usize,
    ) -> cutroom_timeline::TrackId {
        let track = tl.new_track(kind, name);
        let id = track.id;
        apply(tl, Edit::InsertTrack { index, track }).unwrap();
        id
    }

    fn add_clip(
        tl: &mut Timeline,
        track: cutroom_timeline::TrackId,
        media: u64,
        source_in: f64,
        start: f64,
        duration: f64,
    ) -> ClipId {
        let clip = tl.new_clip(MediaId(media), s(source_in), s(start), s(duration));
        let id = clip.id;
        apply(tl, Edit::InsertClip { track, clip }).unwrap();
        id
    }

    /// V1 holds clip A (media 1, a video) at 0..4 s with a 1 s source offset;
    /// A1 holds clip B (media 2, a sound) at 2..5 s.
    fn timeline() -> (Timeline, ClipId, ClipId) {
        let mut tl = Timeline::new();
        let v = add_track(&mut tl, TrackKind::Video, "V1", 0);
        let a = add_track(&mut tl, TrackKind::Audio, "A1", 1);
        let ca = add_clip(&mut tl, v, 1, 1.0, 0.0, 4.0);
        let cb = add_clip(&mut tl, a, 2, 0.0, 2.0, 3.0);
        (tl, ca, cb)
    }

    fn resolver(
        video: PathBuf,
        audio: PathBuf,
    ) -> impl Fn(MediaId) -> Option<(PathBuf, MediaKind)> {
        move |id| match id.0 {
            1 => Some((video.clone(), MediaKind::Video)),
            2 => Some((audio.clone(), MediaKind::Audio)),
            _ => None,
        }
    }

    fn video_of(layer: &LayerTarget) -> &Target {
        match &layer.source {
            LayerSource::Video(t) => t,
            LayerSource::Still(_) | LayerSource::Text(_) => panic!("expected a video layer"),
        }
    }

    fn wait_for(preview: &mut Preview, mut done: impl FnMut(&Preview) -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(8) {
            preview.poll();
            if done(preview) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn files() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempdir().unwrap();
        let (video, audio) = (tmp.path().join("v.webm"), tmp.path().join("a.wav"));
        make_video(&video, 6);
        make_wav(&audio, 4);
        (tmp, video, audio)
    }

    // ----- what to show and play (no GStreamer needed) ---------------------------

    #[test]
    fn the_target_maps_the_playhead_into_the_source_file() {
        let (tl, clip_a, clip_b) = timeline();
        let resolve = resolver(PathBuf::from("/m/v.webm"), PathBuf::from("/m/a.wav"));
        let at_1_5 = find_targets(&tl, s(1.5), &resolve);
        assert_eq!(at_1_5.layers.len(), 1);
        let video = video_of(&at_1_5.layers[0]);
        assert_eq!(video.clip, clip_a);
        assert_eq!(
            video.source_time,
            s(2.5),
            "1 s offset plus 1.5 s into the clip"
        );
        assert_eq!(video.uri, "file:///m/v.webm");
        assert!(at_1_5.audio.is_empty(), "no sound clip yet at 1.5 s");
        let at_3 = find_targets(&tl, s(3.0), &resolve);
        assert_eq!(at_3.audio.len(), 1);
        assert_eq!(
            (at_3.audio[0].clip, at_3.audio[0].source_time),
            (clip_b, s(1.0))
        );
    }

    #[test]
    fn a_text_clip_is_a_layer_that_needs_no_media_and_follows_its_transform() {
        let (mut tl, _, _) = timeline();
        let new_track = tl.new_track(TrackKind::Video, "Text");
        let track = new_track.id;
        apply(
            &mut tl,
            Edit::InsertTrack {
                index: 0,
                track: new_track,
            },
        )
        .unwrap();
        let mut clip = tl.new_clip(MediaId(0), Ticks::ZERO, s(1.0), s(2.0));
        clip.text = Some(cutroom_timeline::TextSpec::default());
        clip.transform.x = 250;
        let id = clip.id;
        apply(&mut tl, Edit::InsertClip { track, clip }).unwrap();
        // Nothing resolves, yet the words are still a layer.
        let targets = find_targets(&tl, s(1.5), &|_| None);
        assert_eq!(targets.layers.len(), 1);
        assert_eq!(targets.layers[0].clip, id);
        assert!(matches!(targets.layers[0].source, LayerSource::Text(_)));
        assert_eq!(targets.layers[0].transform.x, 250);
        assert!(targets.audio.is_empty());
    }

    #[test]
    fn nothing_is_targeted_outside_clips_or_for_unresolvable_media() {
        let (tl, _, _) = timeline();
        let resolve = resolver(PathBuf::from("/v"), PathBuf::from("/a"));
        assert!(
            find_targets(&tl, s(4.0), &resolve).layers.is_empty(),
            "clip ends are exclusive"
        );
        assert!(find_targets(&tl, s(1.0), &resolve).audio.is_empty());
        assert_eq!(find_targets(&tl, s(1.0), &|_| None), Targets::default());
    }

    #[test]
    fn a_photo_is_a_still_layer_with_no_sound_and_muted_tracks_keep_their_picture() {
        let (mut tl, _, _) = timeline();
        let still = |_: MediaId| Some((PathBuf::from("/m/p.png"), MediaKind::Image));
        let targets = find_targets(&tl, s(1.0), &still);
        assert_eq!(targets.layers.len(), 1);
        assert_eq!(
            targets.layers[0].source,
            LayerSource::Still(PathBuf::from("/m/p.png"))
        );
        assert!(targets.audio.is_empty(), "a photo has no sound");

        let video_track = tl.tracks[0].id;
        apply(
            &mut tl,
            Edit::SetTrackFlags {
                track: video_track,
                muted: true,
                locked: false,
            },
        )
        .unwrap();
        let resolve = resolver(PathBuf::from("/v"), PathBuf::from("/a"));
        let layers = find_targets(&tl, s(1.0), &resolve).layers;
        assert!(
            video_of(&layers[0]).muted,
            "the picture shows, the sound is off"
        );
    }

    #[test]
    fn every_video_clip_under_the_playhead_is_a_layer_topmost_first() {
        let (mut tl, clip_a, _) = timeline();
        let upper = add_track(&mut tl, TrackKind::Video, "V2", 0);
        let top = add_clip(&mut tl, upper, 7, 0.0, 0.0, 3.0);
        let resolve = |id: MediaId| match id.0 {
            7 => Some((PathBuf::from("/m/p.png"), MediaKind::Image)),
            1 => Some((PathBuf::from("/m/v.webm"), MediaKind::Video)),
            _ => Some((PathBuf::from("/m/a.wav"), MediaKind::Audio)),
        };
        let layers = find_targets(&tl, s(1.5), &resolve).layers;
        assert_eq!(
            layers.iter().map(|l| l.clip).collect::<Vec<_>>(),
            [top, clip_a]
        );
        assert!(matches!(layers[0].source, LayerSource::Still(_)));
        assert!(
            matches!(layers[1].source, LayerSource::Video(_)),
            "the video below is a layer too"
        );
        let later = find_targets(&tl, s(3.5), &resolve).layers;
        assert_eq!(
            later.iter().map(|l| l.clip).collect::<Vec<_>>(),
            [clip_a],
            "past the photo's end"
        );
    }

    #[test]
    fn a_layer_carries_the_clips_transform() {
        let (mut tl, clip_a, _) = timeline();
        let moved = Transform {
            x: 100,
            scale: 500,
            flip_h: true,
            ..Transform::default()
        };
        apply(
            &mut tl,
            Edit::SetClipTransform {
                clip: clip_a,
                transform: moved,
            },
        )
        .unwrap();
        let resolve = resolver(PathBuf::from("/v"), PathBuf::from("/a"));
        assert_eq!(
            find_targets(&tl, s(1.0), &resolve).layers[0].transform,
            moved
        );
    }

    /// Two clips of one file side by side on V1: the second starts 4 s into the file.
    fn touching_pair(crossfade: f64, second_source_in: f64) -> (Timeline, ClipId, ClipId) {
        let mut tl = Timeline::new();
        let track = tl.new_track(TrackKind::Video, "V1");
        let tid = track.id;
        apply(&mut tl, Edit::InsertTrack { index: 0, track }).unwrap();
        let a = tl.new_clip(MediaId(1), Ticks::ZERO, Ticks::ZERO, s(4.0));
        let b = tl.new_clip(MediaId(1), s(second_source_in), s(4.0), s(4.0));
        let (ia, ib) = (a.id, b.id);
        apply(
            &mut tl,
            Edit::InsertClip {
                track: tid,
                clip: a,
            },
        )
        .unwrap();
        apply(
            &mut tl,
            Edit::InsertClip {
                track: tid,
                clip: b,
            },
        )
        .unwrap();
        apply(
            &mut tl,
            Edit::SetClipCrossfade {
                clip: ib,
                crossfade: s(crossfade),
            },
        )
        .unwrap();
        (tl, ia, ib)
    }

    #[test]
    fn a_crossfade_shows_both_clips_with_the_second_fading_in_over_the_first() {
        let (tl, a, b) = touching_pair(2.0, 4.0);
        let resolve = resolver(PathBuf::from("/v"), PathBuf::from("/a"));
        // Before the window and after it: one clip, fully opaque.
        assert_eq!(find_targets(&tl, s(2.9), &resolve).layers.len(), 1);
        assert_eq!(find_targets(&tl, s(5.1), &resolve).layers.len(), 1);
        // Half way: two layers, the second (B) on top at half opacity.
        let mid = find_targets(&tl, s(4.0), &resolve);
        assert_eq!(mid.layers.len(), 2);
        assert_eq!(mid.layers[0].clip, b);
        assert_eq!(mid.layers[1].clip, a);
        assert_eq!(mid.layers[0].transform.opacity, 500);
        assert_eq!(mid.layers[1].transform.opacity, 1000);
        // The first clip runs on past its end; the second starts before its beginning.
        assert_eq!(video_of(&mid.layers[1]).source_time, s(4.0));
        assert_eq!(video_of(&mid.layers[0]).source_time, s(4.0));
        let early = find_targets(&tl, s(3.0), &resolve);
        assert_eq!(video_of(&early.layers[0]).source_time, s(3.0));
        assert!(
            (video_of(&mid.layers[0]).volume.powi(2) + video_of(&mid.layers[1]).volume.powi(2)
                - 1.0)
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn without_spare_footage_a_crossfade_fades_each_clip_through_black() {
        let (tl, a, b) = touching_pair(2.0, 0.0);
        let resolve = resolver(PathBuf::from("/v"), PathBuf::from("/a"));
        let before = find_targets(&tl, s(3.5), &resolve);
        assert_eq!(before.layers.len(), 1);
        assert_eq!(before.layers[0].clip, a);
        assert_eq!(before.layers[0].transform.opacity, 500);
        let after = find_targets(&tl, s(4.5), &resolve);
        assert_eq!(after.layers.len(), 1);
        assert_eq!(after.layers[0].clip, b);
        assert_eq!(after.layers[0].transform.opacity, 500);
    }

    #[test]
    fn the_second_clip_is_made_ready_before_an_overlapping_crossfade_begins() {
        let (tl, _, b) = touching_pair(2.0, 4.0);
        let resolve = resolver(PathBuf::from("/v"), PathBuf::from("/a"));
        let ahead = targets_ahead(&tl, s(2.5), &resolve);
        assert_eq!(ahead.at, Some(s(3.0)));
        assert!(ahead.layers.iter().any(|l| l.clip == b));
    }

    #[test]
    fn a_hidden_track_shows_and_plays_nothing() {
        let (mut tl, _, _) = timeline();
        let ids: Vec<_> = tl.tracks.iter().map(|t| t.id).collect();
        for track in ids {
            apply(
                &mut tl,
                Edit::SetTrackHidden {
                    track,
                    hidden: true,
                },
            )
            .unwrap();
        }
        let resolve = resolver(PathBuf::from("/v"), PathBuf::from("/a"));
        let t = find_targets(&tl, s(1.0), &resolve);
        assert!(t.layers.is_empty() && t.audio.is_empty());
    }

    #[test]
    fn a_deactivated_clip_shows_and_plays_nothing() {
        let (mut tl, clip_a, _) = timeline();
        apply(
            &mut tl,
            Edit::SetClipDisabled {
                clip: clip_a,
                disabled: true,
            },
        )
        .unwrap();
        let resolve = resolver(PathBuf::from("/v"), PathBuf::from("/a"));
        let t = find_targets(&tl, s(1.0), &resolve);
        assert!(t.layers.is_empty() && t.audio.is_empty());
    }

    #[test]
    fn a_clip_whose_sound_was_extracted_is_silent_but_still_shows() {
        let (mut tl, clip_a, _) = timeline();
        apply(
            &mut tl,
            Edit::SetClipAudioMuted {
                clip: clip_a,
                muted: true,
            },
        )
        .unwrap();
        let resolve = resolver(PathBuf::from("/v"), PathBuf::from("/a"));
        assert!(video_of(&find_targets(&tl, s(1.0), &resolve).layers[0]).muted);
    }

    #[test]
    fn a_video_file_on_an_audio_track_is_sound_only() {
        let mut tl = Timeline::new();
        let a = add_track(&mut tl, TrackKind::Audio, "A1", 0);
        let clip = add_clip(&mut tl, a, 1, 0.0, 0.0, 2.0);
        let resolve = |_: MediaId| Some((PathBuf::from("/m/v.webm"), MediaKind::Video));
        let targets = find_targets(&tl, s(0.5), &resolve);
        assert!(targets.layers.is_empty(), "no picture from an audio track");
        assert_eq!(
            targets.audio.iter().map(|t| t.clip).collect::<Vec<_>>(),
            [clip]
        );
    }

    #[test]
    fn every_unmuted_audio_track_is_mixed_and_muted_ones_are_not() {
        let (mut tl, _, clip_b) = timeline();
        let second = add_track(&mut tl, TrackKind::Audio, "A2", 2);
        let other = add_clip(&mut tl, second, 2, 0.0, 2.5, 1.0);
        let resolve = resolver(PathBuf::from("/v"), PathBuf::from("/a"));
        let mut heard: Vec<_> = find_targets(&tl, s(3.0), &resolve)
            .audio
            .iter()
            .map(|t| t.clip)
            .collect();
        heard.sort();
        let mut expected = vec![clip_b, other];
        expected.sort();
        assert_eq!(heard, expected, "both audio tracks play together");
        apply(
            &mut tl,
            Edit::SetTrackFlags {
                track: second,
                muted: true,
                locked: false,
            },
        )
        .unwrap();
        let heard: Vec<_> = find_targets(&tl, s(3.0), &resolve)
            .audio
            .iter()
            .map(|t| t.clip)
            .collect();
        assert_eq!(heard, [clip_b]);
    }

    #[test]
    fn targets_ahead_only_looks_a_moment_forward_and_picks_the_next_start() {
        let (tl, _, clip_b) = timeline();
        let resolve = resolver(PathBuf::from("/v"), PathBuf::from("/a"));
        assert_eq!(
            targets_ahead(&tl, s(0.5), &resolve),
            Targets::default(),
            "1.5 s away is too far"
        );
        let near = targets_ahead(&tl, s(1.0), &resolve);
        assert_eq!(
            near.audio.iter().map(|t| t.clip).collect::<Vec<_>>(),
            [clip_b]
        );
        assert_eq!(
            near.audio[0].source_time,
            s(0.0),
            "positioned at the start of its source"
        );
        assert_eq!(near.at, Some(s(2.0)));
        assert_eq!(targets_ahead(&tl, s(3.0), &resolve), Targets::default());
    }

    // ----- pipelines and pictures (real media) -------------------------------------

    #[test]
    fn a_layer_pipeline_delivers_pictures_no_larger_than_the_limit() {
        if !init() {
            return;
        }
        let (_tmp, video, audio) = files();
        let (tl, clip_a, _) = timeline();
        let resolve = resolver(video, audio);
        let mut preview = Preview::for_tests();
        preview.sync(&tl, &resolve, s(1.0), false, true);
        assert!(
            wait_for(&mut preview, |p| p.has_frame(clip_a)),
            "a picture arrives"
        );
        let (frame, _) = preview.layers[0].latest_frame().unwrap();
        assert_eq!(
            (frame.width, frame.height),
            (320, 240),
            "small pictures are not enlarged"
        );
        assert_eq!(frame.rgba.len(), 320 * 240 * 4);
        preview.shutdown();
    }

    #[test]
    fn large_pictures_are_shrunk_and_keep_their_shape() {
        if !init() {
            return;
        }
        let tmp = tempdir().unwrap();
        let big = tmp.path().join("big.webm");
        let status = gst::parse::launch(&format!(
            "webmmux name=mux ! filesink location=\"{}\" videotestsrc num-buffers=30 ! video/x-raw,width=1080,height=1920,framerate=30/1 ! videoconvert ! vp8enc deadline=1 ! mux.",
            big.display()
        ))
        .unwrap();
        status.set_state(gst::State::Playing).unwrap();
        status
            .bus()
            .unwrap()
            .timed_pop_filtered(gst::ClockTime::from_seconds(30), &[gst::MessageType::Eos]);
        status.set_state(gst::State::Null).unwrap();

        let mut tl = Timeline::new();
        let v = add_track(&mut tl, TrackKind::Video, "V1", 0);
        let clip = add_clip(&mut tl, v, 1, 0.0, 0.0, 1.0);
        let resolve = move |_: MediaId| Some((big.clone(), MediaKind::Video));
        let mut preview = Preview::for_tests();
        preview.sync(&tl, &resolve, s(0.2), false, true);
        assert!(wait_for(&mut preview, |p| p.has_frame(clip)));
        let (frame, _) = preview.layers[0].latest_frame().unwrap();
        assert_eq!(
            (frame.width, frame.height),
            (540, 960),
            "1080x1920 became 540x960"
        );
        preview.shutdown();
    }

    #[test]
    fn the_preview_resolution_limits_the_pictures_and_restarts_the_pipelines() {
        if !init() {
            return;
        }
        let (_tmp, video, audio) = files();
        let (tl, clip_a, _) = timeline();
        let resolve = resolver(video, audio);
        let mut preview = Preview::for_tests();
        preview.configure(240, false, false);
        preview.sync(&tl, &resolve, s(1.0), false, true);
        assert!(wait_for(&mut preview, |p| p.has_frame(clip_a)));
        let (frame, _) = preview.layers[0].latest_frame().unwrap();
        assert_eq!(
            (frame.width, frame.height),
            (240, 180),
            "a 320x240 picture is cut to a 240 px edge"
        );
        preview.configure(960, false, false);
        assert!(
            preview.layers.is_empty(),
            "a new size starts the pipelines again"
        );
        preview.shutdown();
    }

    #[test]
    fn the_graphics_chip_preview_draws_what_the_processor_draws_or_falls_back() {
        if !init() {
            return;
        }
        let tmp = tempdir().unwrap();
        let png = tmp.path().join("p.png");
        make_png(&png, 100, 100);
        let mut tl = Timeline::new();
        let v = add_track(&mut tl, TrackKind::Video, "V1", 0);
        add_clip(&mut tl, v, 1, 0.0, 0.0, 2.0);
        let resolve = move |_: MediaId| Some((png.clone(), MediaKind::Image));
        let mut cpu = Preview::for_tests();
        cpu.sync(&tl, &resolve, s(0.5), false, true);
        let want = cpu.compose_now((100, 200), None).unwrap();
        let mut card = Preview::for_tests();
        card.configure(960, false, true);
        card.sync(&tl, &resolve, s(0.5), false, true);
        let got = card.compose_now((100, 200), None).expect("a picture");
        assert_eq!((got.width, got.height), (want.width, want.height));
        if card.gpu_in_use() {
            // Same picture, within rounding.
            let worst = got
                .rgba
                .iter()
                .zip(&want.rgba)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(worst <= 3, "the card's picture differs by {worst}");
        } else {
            assert!(card.problem.is_some(), "falling back says why");
            assert_eq!(got.rgba, want.rgba, "the processor drew it");
        }
        cpu.shutdown();
        card.shutdown();
    }

    #[test]
    fn dragging_the_playhead_plays_a_short_sound_when_asked() {
        if !init() {
            return;
        }
        let (_tmp, video, audio) = files();
        let (tl, _, _) = timeline();
        let resolve = resolver(video, audio);
        for (asked, expect) in [(true, true), (false, false)] {
            let mut preview = Preview::for_tests();
            preview.configure(960, asked, false);
            preview.sync(&tl, &resolve, s(1.0), false, true);
            assert!(wait_for(&mut preview, |p| p.total_loads() > 0
                && p.layers.iter().all(|l| l.ready)));
            preview.sync(&tl, &resolve, s(1.6), false, true);
            let heard = preview.layers.iter().any(|l| l.blip.is_some());
            assert_eq!(heard, expect, "scrub sound {asked}");
            if asked {
                assert!(
                    wait_for(&mut preview, |p| p.layers.iter().all(|l| l.blip.is_none())),
                    "the sound stops by itself"
                );
            }
            preview.shutdown();
        }
    }

    #[test]
    fn the_compositor_places_a_layer_as_its_transform_says() {
        if !init() {
            return;
        }
        let tmp = tempdir().unwrap();
        let png = tmp.path().join("p.png");
        make_png(&png, 100, 100); // blue (0x33, 0x66, 0xcc)
        let mut tl = Timeline::new();
        let v = add_track(&mut tl, TrackKind::Video, "V1", 0);
        let clip = add_clip(&mut tl, v, 1, 0.0, 0.0, 2.0);
        let resolve = move |_: MediaId| Some((png.clone(), MediaKind::Image));
        let mut preview = Preview::for_tests();
        preview.sync(&tl, &resolve, s(0.5), false, true);

        let canvas = (100, 200);
        let full = preview.compose_now(canvas, None).expect("drawn");
        assert_eq!((full.width, full.height), (100, 200));
        assert_eq!(
            full.pixel(50, 100),
            [0x33, 0x66, 0xcc, 255],
            "the picture is in the middle"
        );
        assert_eq!(
            full.pixel(50, 10),
            [0, 0, 0, 255],
            "the bars above are black"
        );
        assert!(
            preview.compose_now(canvas, None).is_none(),
            "nothing changed, nothing redrawn"
        );

        let small = Transform {
            scale: 300,
            x: 300,
            ..Transform::default()
        };
        let moved = preview
            .compose_now(canvas, Some((clip, small)))
            .expect("a preview of the change is drawn");
        assert_eq!(
            moved.pixel(50, 100),
            [0, 0, 0, 255],
            "moved away from the middle"
        );
        assert_eq!(
            moved.pixel(80, 100),
            [0x33, 0x66, 0xcc, 255],
            "and to the right"
        );

        let faded = Transform {
            opacity: 500,
            ..Transform::default()
        };
        let half = preview.compose_now(canvas, Some((clip, faded))).unwrap();
        assert_eq!(half.pixel(50, 100)[0], 0x1a, "half transparent over black");
        preview.shutdown();
    }

    #[test]
    fn an_empty_playhead_is_a_black_canvas_not_a_frozen_frame() {
        if !init() {
            return;
        }
        let (_tmp, video, audio) = files();
        let (tl, clip_a, _) = timeline();
        let resolve = resolver(video, audio);
        let mut preview = Preview::for_tests();
        preview.sync(&tl, &resolve, s(1.0), false, true);
        assert!(wait_for(&mut preview, |p| p.has_frame(clip_a)));
        let with_video = preview.compose_now((90, 160), None).expect("drawn");
        assert!(with_video
            .rgba
            .chunks_exact(4)
            .any(|p| p[0] > 0 || p[1] > 0 || p[2] > 0));

        preview.sync(&tl, &resolve, s(4.5), false, true); // past the last clip
        let blank = preview
            .compose_now((90, 160), None)
            .expect("redrawn at once");
        assert!(
            blank.rgba.chunks_exact(4).all(|p| p == [0, 0, 0, 255]),
            "nothing but black"
        );
        preview.shutdown();
    }

    #[test]
    fn the_old_picture_stays_until_the_new_clips_first_frame_arrives() {
        if !init() {
            return;
        }
        let (_tmp, video, audio) = files();
        let (tl, clip_a, _) = timeline();
        let resolve = resolver(video, audio);
        let mut preview = Preview::for_tests();
        preview.sync(&tl, &resolve, s(1.0), false, true);
        assert!(wait_for(&mut preview, |p| p.has_frame(clip_a)));
        assert!(preview.compose_now((90, 160), None).is_some());
        // Jump somewhere else in the same clip: the pipeline is re-seeked and
        // the next picture is not here yet.
        preview.layers[0].clear_frame();
        assert!(
            preview.compose_now((90, 160), None).is_none(),
            "no flash of black while waiting"
        );
        preview.shutdown();
    }

    #[test]
    fn scrubbing_seeks_the_video_pipeline_to_the_right_frame() {
        if !init() {
            return;
        }
        let (_tmp, video, audio) = files();
        let (tl, clip_a, _) = timeline();
        let resolve = resolver(video, audio);
        let mut preview = Preview::for_tests();
        preview.sync(&tl, &resolve, s(0.5), false, true);
        assert!(wait_for(&mut preview, |p| p
            .video_state()
            .is_some_and(|(c, ready, _)| c == Some(clip_a) && ready)));
        preview.sync(&tl, &resolve, s(2.5), false, true); // 3.5 s into the file
        assert!(wait_for(&mut preview, |p| p
            .video_state()
            .and_then(|(_, _, pos)| pos)
            .is_some_and(|pos| (pos.as_seconds_f64() - 3.5).abs() < 0.1)));
        preview.shutdown();
    }

    #[test]
    fn leaving_a_clip_releases_its_pipeline() {
        if !init() {
            return;
        }
        let (_tmp, video, audio) = files();
        let (tl, clip_a, clip_b) = timeline();
        let resolve = resolver(video, audio);
        let mut preview = Preview::for_tests();
        preview.sync(&tl, &resolve, s(3.0), false, true);
        assert_eq!(preview.layer_clips(), vec![clip_a]);
        assert_eq!(preview.audio_clips(), vec![clip_b]);
        preview.sync(&tl, &resolve, s(4.5), false, true); // the video ended at 4 s
        assert!(preview.layer_clips().is_empty());
        assert_eq!(preview.audio_clips(), vec![clip_b]);
        preview.shutdown();
    }

    #[test]
    fn overlapping_clips_each_get_their_own_pipeline() {
        if !init() {
            return;
        }
        let (_tmp, video, audio) = files();
        let (mut tl, clip_a, clip_b) = timeline();
        let a2 = add_track(&mut tl, TrackKind::Audio, "A2", 2);
        let clip_c = add_clip(&mut tl, a2, 2, 0.0, 2.5, 2.0);
        let v2 = add_track(&mut tl, TrackKind::Video, "V2", 0);
        let top = add_clip(&mut tl, v2, 1, 0.0, 0.0, 5.0);
        let resolve = resolver(video, audio);
        let mut preview = Preview::for_tests();
        preview.sync(&tl, &resolve, s(3.0), true, true);
        let mut audio_expected = vec![clip_b, clip_c];
        audio_expected.sort();
        assert_eq!(
            preview.audio_clips(),
            audio_expected,
            "both audio clips are heard"
        );
        let mut layers_expected = vec![clip_a, top];
        layers_expected.sort();
        assert_eq!(
            preview.layer_clips(),
            layers_expected,
            "both pictures are decoded, the top one drawn above"
        );

        preview.sync(&tl, &resolve, s(4.8), true, true); // clip C and clip A have ended
        assert_eq!(preview.audio_clips(), vec![clip_b][..]);
        assert_eq!(preview.layer_clips(), vec![top][..]);
        preview.shutdown();
    }

    // ----- cuts ---------------------------------------------------------------------

    /// Video track with clip X at 0..2 s (source 0) and clip Y at 2..4 s whose
    /// source starts at `y_source`.
    fn two_cuts(y_source: f64) -> (Timeline, ClipId, ClipId) {
        let mut tl = Timeline::new();
        let v = add_track(&mut tl, TrackKind::Video, "V1", 0);
        let x = add_clip(&mut tl, v, 1, 0.0, 0.0, 2.0);
        let y = add_clip(&mut tl, v, 1, y_source, 2.0, 2.0);
        (tl, x, y)
    }

    #[test]
    fn a_fast_drag_does_not_pile_up_seeks_and_ends_on_the_exact_frame() {
        if !init() {
            return;
        }
        let (_tmp, video, audio) = files();
        let (tl, clip_a, _) = timeline();
        let resolve = resolver(video, audio);
        let mut preview = Preview::for_tests();
        preview.sync(&tl, &resolve, s(1.0), false, true);
        assert!(wait_for(&mut preview, |p| p
            .video_state()
            .is_some_and(|(c, ready, _)| c == Some(clip_a) && ready)));
        for step in 0..60 {
            let at = 1.0 + f64::from(step) * 0.04;
            preview.sync(&tl, &resolve, Ticks::from_seconds_f64(at), false, true);
            preview.poll();
        }
        let want = 1.0 + 59.0 * 0.04 + 1.0;
        assert!(
            wait_for(&mut preview, |p| p
                .video_state()
                .and_then(|(_, _, pos)| pos)
                .is_some_and(|pos| (pos.as_seconds_f64() - want).abs() < 0.05)),
            "settled on {want:.2}s, got {:?}",
            preview.video_state()
        );
        preview.shutdown();
    }

    #[test]
    fn the_next_clip_is_ready_and_running_silently_just_before_its_cut() {
        if !init() {
            return;
        }
        let (_tmp, video, audio) = files();
        let (tl, x, y) = two_cuts(4.0); // a piece of the middle was cut away
        let resolve = resolver(video, audio);
        let mut preview = Preview::for_tests();
        preview.sync(&tl, &resolve, s(0.5), true, true);
        assert!(wait_for(&mut preview, |p| p
            .video_state()
            .is_some_and(|(c, ready, _)| c == Some(x) && ready)));

        // 1.0 s: Y is less than the look-ahead away, so it is loaded.
        preview.sync(&tl, &resolve, s(1.0), true, false);
        assert!(
            wait_for(&mut preview, |p| p.standby_state().is_some_and(
                |(c, ready, _)| c == Some(y) && ready && p.has_frame(y)
            )),
            "standby: {:?}",
            preview.standby_state()
        );
        let loads = preview.total_loads();

        preview.sync(&tl, &resolve, s(1.5), true, false);
        assert_eq!(
            preview.standby_running_early(),
            Some((false, false)),
            "too early to start"
        );
        preview.sync(&tl, &resolve, s(1.92), true, false);
        assert_eq!(
            preview.standby_running_early(),
            Some((true, true)),
            "running early, and muted"
        );
        for _ in 0..4 {
            std::thread::sleep(Duration::from_millis(27));
            preview.poll();
        }

        preview.sync(&tl, &resolve, s(2.03), true, false); // the cut
        let (clip, ready, position) = preview.video_state().unwrap();
        assert_eq!(clip, Some(y));
        assert!(
            ready && preview.has_frame(y),
            "a picture is there from the first moment"
        );
        let at = position.unwrap().as_seconds_f64();
        assert!((at - 4.0).abs() < 0.3, "positioned at its start, not {at}");
        assert_eq!(
            preview.total_loads(),
            loads,
            "nothing was loaded at the cut"
        );
        assert_eq!(
            preview
                .layers
                .iter()
                .find(|p| p.clip == Some(y))
                .map(|p| p.playbin.property::<bool>("mute")),
            Some(false),
            "and it is audible once it is the current clip"
        );
        preview.shutdown();
    }

    #[test]
    fn a_split_in_the_same_file_keeps_playing_across_the_cut() {
        if !init() {
            return;
        }
        let (_tmp, video, audio) = files();
        let (tl, x, y) = two_cuts(2.0); // Y continues exactly where X stops
        let resolve = resolver(video, audio);
        let mut preview = Preview::for_tests();
        preview.sync(&tl, &resolve, s(1.5), true, true);
        assert!(wait_for(&mut preview, |p| p
            .video_state()
            .is_some_and(|(c, ready, _)| c == Some(x) && ready)));
        preview.sync(&tl, &resolve, s(1.6), true, false);
        assert!(wait_for(&mut preview, |p| p
            .standby_state()
            .is_some_and(|(c, ready, _)| c == Some(y) && ready)));
        let loads = preview.total_loads();
        preview.sync(&tl, &resolve, s(2.05), true, false);
        assert_eq!(preview.video_state().unwrap().0, Some(y));
        assert_eq!(
            preview.total_loads(),
            loads,
            "no pipeline was built at the cut"
        );
        preview.shutdown();
    }

    #[test]
    fn sound_that_starts_soon_is_loaded_before_it_starts() {
        if !init() {
            return;
        }
        let (_tmp, video, audio) = files();
        let (tl, _, clip_b) = timeline(); // the sound starts at 2.0 s
        let resolve = resolver(video, audio);
        let mut preview = Preview::for_tests();
        preview.sync(&tl, &resolve, s(0.5), true, true);
        assert!(
            !preview.audio_clips().contains(&clip_b),
            "too early to prepare"
        );
        preview.sync(&tl, &resolve, s(1.2), true, false);
        assert!(
            preview.audio_clips().contains(&clip_b),
            "within the look-ahead it is loaded"
        );
        preview.sync(&tl, &resolve, s(2.1), true, false);
        assert!(preview.audio_clips().contains(&clip_b));
        preview.shutdown();
    }

    /// Plays a real file (set `CUTROOM_SAMPLE_VIDEO`, for example to Big Buck
    /// Bunny) across jump cuts in real time and checks every cut arrives
    /// prepared: ready, with a picture, at the right place.
    #[test]
    fn playing_real_footage_across_jump_cuts_arrives_prepared() {
        let Some(sample) = std::env::var_os("CUTROOM_SAMPLE_VIDEO").map(PathBuf::from) else {
            return;
        };
        if !init() || !sample.is_file() {
            return;
        }
        let mut tl = Timeline::new();
        let v = add_track(&mut tl, TrackKind::Video, "V1", 0);
        let pieces = [0.0, 5.0, 2.0, 7.0];
        let clips: Vec<(ClipId, f64)> = pieces
            .iter()
            .enumerate()
            .map(|(i, source)| {
                (
                    add_clip(&mut tl, v, 1, *source, i as f64 * 1.5, 1.5),
                    *source,
                )
            })
            .collect();
        let resolve = {
            let sample = sample.clone();
            move |_: MediaId| Some((sample.clone(), MediaKind::Video))
        };
        let mut preview = Preview::for_tests();
        preview.sync(&tl, &resolve, s(0.0), true, true);
        assert!(wait_for(&mut preview, |p| p
            .video_state()
            .is_some_and(|(c, ready, _)| c == Some(clips[0].0) && ready)));

        let start = Instant::now();
        let (mut crossed, mut last_clip) = (0, clips[0].0);
        let mut check: Option<(usize, f64)> = None;
        while start.elapsed() < Duration::from_millis(6000) {
            let playhead = Ticks::from_seconds_f64(start.elapsed().as_secs_f64());
            preview.sync(&tl, &resolve, playhead, true, true);
            preview.poll();
            let (clip, ready, position) = preview.video_state().unwrap();
            if let (Some((index, expected)), Some(position)) = (check, position) {
                let at = position.as_seconds_f64();
                assert!(
                    (at - expected).abs() < 0.6,
                    "after cut {index}: at {at:.2}s, wanted about {expected:.2}s"
                );
                check = None;
            }
            if clip != Some(last_clip) {
                let index = clips.iter().position(|(id, _)| Some(*id) == clip).unwrap();
                assert!(ready, "cut {index} arrived before its pipeline was ready");
                assert!(
                    preview.has_frame(clip.unwrap()),
                    "cut {index} had no picture yet"
                );
                check = Some((index, clips[index].1));
                last_clip = clip.unwrap();
                crossed += 1;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        assert_eq!(crossed, 3, "three cuts were crossed");
        preview.shutdown();
    }
}
