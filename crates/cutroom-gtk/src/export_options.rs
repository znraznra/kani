//! What the Export window offers, and which choices go together: aspect
//! ratios and the sizes that belong to each, codecs and the formats that can
//! hold them, audio formats, bit rates. Plain data and rules, no GStreamer, so
//! it is all tested without any plug-ins.

use cutroom_timeline::FrameRate;

// ------------------------------------------------------------ video codecs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoCodec {
    H264,
    H265,
    Vp9,
    Av1,
}

impl VideoCodec {
    pub const ALL: [VideoCodec; 4] = [
        VideoCodec::H264,
        VideoCodec::H265,
        VideoCodec::Vp9,
        VideoCodec::Av1,
    ];

    /// Kept in settings.
    pub fn id(self) -> &'static str {
        match self {
            VideoCodec::H264 => "h264",
            VideoCodec::H265 => "h265",
            VideoCodec::Vp9 => "vp9",
            VideoCodec::Av1 => "av1",
        }
    }

    pub fn from_id(id: &str) -> Option<VideoCodec> {
        VideoCodec::ALL.into_iter().find(|c| c.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264 (works everywhere)",
            VideoCodec::H265 => "H.265 / HEVC (smaller files)",
            VideoCodec::Vp9 => "VP9",
            VideoCodec::Av1 => "AV1 (smallest, slow)",
        }
    }

    /// Short name for messages.
    pub fn name(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::H265 => "H.265",
            VideoCodec::Vp9 => "VP9",
            VideoCodec::Av1 => "AV1",
        }
    }

    /// How much of H.264's bit rate gives about the same picture quality.
    pub fn bitrate_factor(self) -> f64 {
        match self {
            VideoCodec::H264 => 1.0,
            VideoCodec::H265 => 0.65,
            VideoCodec::Vp9 => 0.7,
            VideoCodec::Av1 => 0.55,
        }
    }

    /// The file formats that can hold this codec, the usual one first.
    pub fn containers(self) -> &'static [Container] {
        match self {
            VideoCodec::H264 => &[Container::Mp4, Container::Mov, Container::Mkv],
            VideoCodec::H265 => &[Container::Mp4, Container::Mov, Container::Mkv],
            VideoCodec::Vp9 => &[Container::WebM, Container::Mkv, Container::Mp4],
            VideoCodec::Av1 => &[Container::Mp4, Container::WebM, Container::Mkv],
        }
    }
}

// --------------------------------------------------------- file formats

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Container {
    Mp4,
    Mov,
    Mkv,
    WebM,
}

impl Container {
    pub const ALL: [Container; 4] = [
        Container::Mp4,
        Container::Mov,
        Container::Mkv,
        Container::WebM,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mov => "mov",
            Container::Mkv => "mkv",
            Container::WebM => "webm",
        }
    }

    pub fn from_id(id: &str) -> Option<Container> {
        Container::ALL.into_iter().find(|c| c.id() == id)
    }

    /// File name ending, without the dot.
    pub fn extension(self) -> &'static str {
        self.id()
    }

    pub fn label(self) -> &'static str {
        match self {
            Container::Mp4 => "MP4",
            Container::Mov => "MOV",
            Container::Mkv => "MKV",
            Container::WebM => "WebM",
        }
    }

    /// The GStreamer muxer.
    pub fn muxer(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4mux",
            Container::Mov => "qtmux",
            Container::Mkv => "matroskamux",
            Container::WebM => "webmmux",
        }
    }

    /// Sound formats this file format can carry, the usual one first.
    pub fn audio_formats(self) -> &'static [AudioFormat] {
        match self {
            Container::Mp4 => &[AudioFormat::Aac, AudioFormat::Mp3],
            Container::Mov => &[AudioFormat::Aac, AudioFormat::Wav],
            Container::Mkv => &[
                AudioFormat::Aac,
                AudioFormat::Mp3,
                AudioFormat::Opus,
                AudioFormat::Flac,
                AudioFormat::Wav,
            ],
            Container::WebM => &[AudioFormat::Opus],
        }
    }
}

// ----------------------------------------------------------- audio

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioFormat {
    Aac,
    Mp3,
    Opus,
    Flac,
    Wav,
}

impl AudioFormat {
    pub const ALL: [AudioFormat; 5] = [
        AudioFormat::Aac,
        AudioFormat::Mp3,
        AudioFormat::Opus,
        AudioFormat::Flac,
        AudioFormat::Wav,
    ];

    #[allow(dead_code)]
    pub fn id(self) -> &'static str {
        match self {
            AudioFormat::Aac => "aac",
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Opus => "opus",
            AudioFormat::Flac => "flac",
            AudioFormat::Wav => "wav",
        }
    }

    #[allow(dead_code)]
    pub fn from_id(id: &str) -> Option<AudioFormat> {
        AudioFormat::ALL.into_iter().find(|f| f.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            AudioFormat::Aac => "AAC",
            AudioFormat::Mp3 => "MP3",
            AudioFormat::Opus => "Opus",
            AudioFormat::Flac => "FLAC (lossless)",
            AudioFormat::Wav => "WAV (uncompressed)",
        }
    }

    /// Ending of a sound-only file made in this format.
    pub fn file_extension(self) -> &'static str {
        match self {
            AudioFormat::Aac => "m4a",
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Opus => "opus",
            AudioFormat::Flac => "flac",
            AudioFormat::Wav => "wav",
        }
    }

    /// Does a bit rate mean anything for this format?
    pub fn is_lossy(self) -> bool {
        matches!(
            self,
            AudioFormat::Aac | AudioFormat::Mp3 | AudioFormat::Opus
        )
    }
}

/// Bit rates offered for lossy sound, in kilobits per second.
pub const AUDIO_BITRATES: [u32; 6] = [96, 128, 160, 192, 256, 320];
pub const DEFAULT_AUDIO_KBPS: u32 = 192;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channels {
    Mono,
    Stereo,
}

impl Channels {
    pub const ALL: [Channels; 2] = [Channels::Stereo, Channels::Mono];

    pub fn count(self) -> u32 {
        match self {
            Channels::Mono => 1,
            Channels::Stereo => 2,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Channels::Mono => "mono",
            Channels::Stereo => "stereo",
        }
    }

    pub fn from_id(id: &str) -> Option<Channels> {
        Channels::ALL.into_iter().find(|c| c.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Channels::Mono => "Mono",
            Channels::Stereo => "Stereo",
        }
    }
}

// ------------------------------------------------------------- frame rates

/// Frame rates offered, with the project's own first.
pub fn frame_rates(project: FrameRate) -> Vec<FrameRate> {
    let mut rates = vec![project];
    for r in [
        FrameRate::FPS_24,
        FrameRate::NTSC_23_976,
        FrameRate::FPS_25,
        FrameRate::FPS_30,
        FrameRate::NTSC_29_97,
        FrameRate::FPS_50,
        FrameRate::FPS_60,
        FrameRate::NTSC_59_94,
    ] {
        if !rates.iter().any(|x| (x.as_f64() - r.as_f64()).abs() < 0.001) {
            rates.push(r);
        }
    }
    rates
}

// ------------------------------------------------------- shapes and sizes

/// The picture shapes on offer (width : height).
pub const ASPECTS: [(u32, u32); 7] = [(16, 9), (9, 16), (4, 3), (3, 4), (1, 1), (4, 5), (21, 9)];

/// Sizes by the shorter side: lines, and what the size is known as.
const TIERS: [(u32, &str); 7] = [
    (360, "360p"),
    (480, "480p NTSC"),
    (576, "576p PAL"),
    (720, "720p HD"),
    (1080, "1080p Full HD"),
    (1440, "1440p 2K"),
    (2160, "2160p 4K"),
];

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// Width : height in lowest terms.
pub fn reduced(width: u32, height: u32) -> (u32, u32) {
    let g = gcd(width.max(1), height.max(1)).max(1);
    (width / g, height / g)
}

/// Do two shapes match (to within half a percent)?
pub fn same_shape(a: (u32, u32), b: (u32, u32)) -> bool {
    let (x, y) = (
        f64::from(a.0) / f64::from(a.1.max(1)),
        f64::from(b.0) / f64::from(b.1.max(1)),
    );
    (x - y).abs() / y.max(0.0001) < 0.005
}

/// "16:9", or "1080:1920" shown as "9:16"; odd shapes keep their size ratio.
pub fn aspect_name(width: u32, height: u32) -> String {
    for (w, h) in ASPECTS {
        if same_shape((w, h), (width, height)) {
            return format!("{w}:{h}");
        }
    }
    let (w, h) = reduced(width, height);
    if w > 99 || h > 99 {
        format!("{:.2}:1", f64::from(width) / f64::from(height.max(1)))
    } else {
        format!("{w}:{h}")
    }
}

/// The shapes to list: the project's own first, then the standard ones.
/// Each is `(label, width : height)`.
pub fn aspect_options(project: (u32, u32)) -> Vec<(String, (u32, u32))> {
    let own = reduced(project.0, project.1);
    let mut out = vec![(format!("{} (project)", aspect_name(project.0, project.1)), own)];
    for (w, h) in ASPECTS {
        if !same_shape((w, h), project) {
            out.push((format!("{w}:{h}"), (w, h)));
        }
    }
    out
}

fn even(v: f64) -> u32 {
    ((v / 2.0).round() as u32).max(1) * 2
}

/// A size in a shape with the given shorter side; both sides even.
fn size_for(aspect: (u32, u32), short: u32) -> (u32, u32) {
    let (a, b) = (f64::from(aspect.0), f64::from(aspect.1));
    let long = f64::from(short) * a.max(b) / a.min(b);
    if aspect.0 >= aspect.1 {
        (even(long), even(f64::from(short)))
    } else {
        (even(f64::from(short)), even(long))
    }
}

/// One size on offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
    pub label: String,
}

/// The sizes for a shape, biggest first, from NTSC and PAL standard definition
/// up to 4K. When the shape is the project's, the project's own size is there
/// too (and is where the list starts).
pub fn resolutions(aspect: (u32, u32), project: (u32, u32)) -> Vec<Resolution> {
    let mut out: Vec<Resolution> = TIERS
        .iter()
        .rev()
        .map(|&(short, name)| {
            let (width, height) = size_for(aspect, short);
            Resolution {
                width,
                height,
                label: format!("{width} x {height} - {name}"),
            }
        })
        .collect();
    if same_shape(aspect, project) && !out.iter().any(|r| (r.width, r.height) == project) {
        let at = out
            .iter()
            .position(|r| r.width.min(r.height) < project.0.min(project.1))
            .unwrap_or(out.len());
        out.insert(
            at,
            Resolution {
                width: project.0,
                height: project.1,
                label: format!("{} x {} - project size", project.0, project.1),
            },
        );
    } else if let Some(r) = out
        .iter_mut()
        .find(|r| (r.width, r.height) == project && same_shape(aspect, project))
    {
        r.label.push_str(" (project)");
    }
    out
}

/// Which size to start on: the project's own when it is there, otherwise the
/// one whose shorter side is closest to the project's.
pub fn default_resolution(list: &[Resolution], project: (u32, u32)) -> usize {
    if let Some(i) = list
        .iter()
        .position(|r| (r.width, r.height) == project)
    {
        return i;
    }
    let short = i64::from(project.0.min(project.1));
    list.iter()
        .enumerate()
        .min_by_key(|(_, r)| (i64::from(r.width.min(r.height)) - short).abs())
        .map_or(0, |(i, _)| i)
}

/// How big the project's picture is drawn inside an export frame of another
/// shape. With `fill` it covers the frame (the overhang is cropped), without
/// it it fits inside (the rest is black bars). Same shape: the frame itself.
pub fn canvas_inside(frame: (u32, u32), project: (u32, u32), fill: bool) -> (u32, u32) {
    if same_shape(frame, project) {
        return frame;
    }
    let kx = f64::from(frame.0) / f64::from(project.0.max(1));
    let ky = f64::from(frame.1) / f64::from(project.1.max(1));
    let k = if fill { kx.max(ky) } else { kx.min(ky) };
    (
        even(f64::from(project.0) * k),
        even(f64::from(project.1) * k),
    )
}

// -------------------------------------------------------------- bit rates

/// The video bit rate presets, plus a number of the user's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitratePreset {
    Low,
    Recommended,
    High,
    Maximum,
    Custom,
}

impl BitratePreset {
    pub const ALL: [BitratePreset; 5] = [
        BitratePreset::Low,
        BitratePreset::Recommended,
        BitratePreset::High,
        BitratePreset::Maximum,
        BitratePreset::Custom,
    ];

    pub fn id(self) -> &'static str {
        match self {
            BitratePreset::Low => "low",
            BitratePreset::Recommended => "recommended",
            BitratePreset::High => "high",
            BitratePreset::Maximum => "maximum",
            BitratePreset::Custom => "custom",
        }
    }

    /// Saved choices from older versions used the quality names.
    pub fn from_id(id: &str) -> Option<BitratePreset> {
        let id = id.to_lowercase();
        match id.as_str() {
            "small file" | "small" => Some(BitratePreset::Low),
            _ => BitratePreset::ALL.into_iter().find(|p| p.id() == id),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            BitratePreset::Low => "Low (small file)",
            BitratePreset::Recommended => "Recommended",
            BitratePreset::High => "High",
            BitratePreset::Maximum => "Maximum",
            BitratePreset::Custom => "Custom",
        }
    }

    /// Bits per pixel per frame, for H.264.
    fn bits_per_pixel(self) -> f64 {
        match self {
            BitratePreset::Low => 0.07,
            BitratePreset::Recommended | BitratePreset::Custom => 0.12,
            BitratePreset::High => 0.18,
            BitratePreset::Maximum => 0.28,
        }
    }
}

/// Video bit rate in kilobits per second for a preset: more pixels and frames
/// need more, a more efficient codec needs less.
pub fn video_kbps(
    codec: VideoCodec,
    width: u32,
    height: u32,
    fps: f64,
    preset: BitratePreset,
) -> u32 {
    let bits = f64::from(width) * f64::from(height) * fps * preset.bits_per_pixel();
    let h264 = ((bits / 1000.0).round() as u32).clamp(1_500, 80_000);
    ((f64::from(h264) * codec.bitrate_factor()).round() as u32).clamp(800, 80_000)
}

/// "7.8 Mbps" for a bit rate in kilobits per second.
pub fn mbps(kbps: u32) -> String {
    format!("{:.1} Mbps", f64::from(kbps) / 1000.0)
}

/// Roughly how big the files will be, in bytes.
pub fn estimated_bytes(seconds: f64, video_kbps: Option<u32>, audio_kbps: Option<u32>) -> f64 {
    let kbps = video_kbps.unwrap_or(0) + audio_kbps.unwrap_or(0);
    f64::from(kbps) * 1000.0 / 8.0 * seconds
}

/// Bytes a second of sound takes, for formats that are not set by bit rate.
pub fn lossless_kbps(format: AudioFormat, channels: Channels, rate: u32) -> u32 {
    match format {
        AudioFormat::Wav => rate * 16 * channels.count() / 1000,
        // FLAC typically halves the size of uncompressed sound.
        AudioFormat::Flac => rate * 16 * channels.count() / 2000,
        _ => DEFAULT_AUDIO_KBPS,
    }
}

// -------------------------------------------------------------- presets

/// A ready-made set of choices for a place the video is going (or for sound
/// alone). They are the usual settings people use for those places, not rules
/// the places enforce: they are a starting point, and every choice can still be
/// changed afterwards.
#[derive(Debug, Clone, Copy)]
pub struct SitePreset {
    #[allow(dead_code)]
    pub id: &'static str,
    pub name: &'static str,
    /// What it sets, in a sentence.
    pub note: &'static str,
    /// The picture's shape, or `None` for the project's own.
    pub aspect: Option<(u32, u32)>,
    /// The size by its shorter side in lines (as in the size list), or 0 for the
    /// project's own size.
    pub short_side: u32,
    /// Frames per second, or `None` for the project's own.
    pub fps: Option<u32>,
    pub codec: VideoCodec,
    pub container: Container,
    pub bitrate: BitratePreset,
    pub audio_format: AudioFormat,
    pub audio_kbps: u32,
    pub channels: Channels,
    /// Only a sound file (the video is switched off).
    pub sound_only: bool,
}

const fn video_preset(
    id: &'static str,
    name: &'static str,
    note: &'static str,
    aspect: Option<(u32, u32)>,
    short_side: u32,
    bitrate: BitratePreset,
    audio_kbps: u32,
) -> SitePreset {
    SitePreset {
        id,
        name,
        note,
        aspect,
        short_side,
        fps: None,
        codec: VideoCodec::H264,
        container: Container::Mp4,
        bitrate,
        audio_format: AudioFormat::Aac,
        audio_kbps,
        channels: Channels::Stereo,
        sound_only: false,
    }
}

/// The presets, in the order they are listed.
pub const SITE_PRESETS: [SitePreset; 14] = [
    video_preset(
        "youtube-shorts",
        "YouTube Shorts",
        "Vertical 9:16, 1080 x 1920, H.264 in MP4, AAC sound.",
        Some((9, 16)),
        1080,
        BitratePreset::Recommended,
        192,
    ),
    video_preset(
        "tiktok",
        "TikTok",
        "Vertical 9:16, 1080 x 1920, H.264 in MP4, AAC sound.",
        Some((9, 16)),
        1080,
        BitratePreset::Recommended,
        192,
    ),
    video_preset(
        "instagram-reels",
        "Instagram Reels and Stories",
        "Vertical 9:16, 1080 x 1920, H.264 in MP4, AAC sound.",
        Some((9, 16)),
        1080,
        BitratePreset::Recommended,
        192,
    ),
    video_preset(
        "instagram-feed",
        "Instagram feed (4:5)",
        "Portrait 4:5, 1080 x 1350, H.264 in MP4, AAC sound.",
        Some((4, 5)),
        1080,
        BitratePreset::Recommended,
        192,
    ),
    video_preset(
        "square",
        "Square post (1:1)",
        "Square 1:1, 1080 x 1080, H.264 in MP4, AAC sound. Works on LinkedIn, Facebook and Instagram.",
        Some((1, 1)),
        1080,
        BitratePreset::Recommended,
        192,
    ),
    video_preset(
        "linkedin",
        "LinkedIn video (16:9)",
        "Landscape 16:9, 1920 x 1080, H.264 in MP4, AAC sound.",
        Some((16, 9)),
        1080,
        BitratePreset::Recommended,
        192,
    ),
    video_preset(
        "youtube-1080",
        "YouTube (1080p)",
        "Landscape 16:9, 1920 x 1080, H.264 in MP4, a higher bit rate, AAC sound.",
        Some((16, 9)),
        1080,
        BitratePreset::High,
        256,
    ),
    video_preset(
        "youtube-4k",
        "YouTube (4K)",
        "Landscape 16:9, 3840 x 2160, H.264 in MP4, a higher bit rate, AAC sound. Slow without a hardware encoder.",
        Some((16, 9)),
        2160,
        BitratePreset::High,
        256,
    ),
    video_preset(
        "x",
        "X (Twitter) (16:9, 720p)",
        "Landscape 16:9, 1280 x 720, H.264 in MP4, AAC sound.",
        Some((16, 9)),
        720,
        BitratePreset::Recommended,
        128,
    ),
    video_preset(
        "chat",
        "Small file (chat apps, email)",
        "The project's shape at 720p, H.264 in MP4, a low bit rate, AAC sound.",
        None,
        720,
        BitratePreset::Low,
        128,
    ),
    SitePreset {
        id: "web",
        name: "Web page (WebM)",
        note: "The project's shape at 1080p, VP9 in WebM, Opus sound. Slow without a hardware encoder.",
        aspect: None,
        short_side: 1080,
        fps: None,
        codec: VideoCodec::Vp9,
        container: Container::WebM,
        bitrate: BitratePreset::Recommended,
        audio_format: AudioFormat::Opus,
        audio_kbps: 128,
        channels: Channels::Stereo,
        sound_only: false,
    },
    SitePreset {
        id: "master",
        name: "High-quality master",
        note: "The project's own size and shape, H.265 in MKV at the highest bit rate, lossless FLAC sound. Big files, for keeping.",
        aspect: None,
        short_side: 0,
        fps: None,
        codec: VideoCodec::H265,
        container: Container::Mkv,
        bitrate: BitratePreset::Maximum,
        audio_format: AudioFormat::Flac,
        audio_kbps: 320,
        channels: Channels::Stereo,
        sound_only: false,
    },
    SitePreset {
        id: "sound-mp3",
        name: "Sound only: MP3",
        note: "Just the project's sound, as an MP3 at 192 kbps stereo. Plays everywhere.",
        aspect: None,
        short_side: 0,
        fps: None,
        codec: VideoCodec::H264,
        container: Container::Mp4,
        bitrate: BitratePreset::Recommended,
        audio_format: AudioFormat::Mp3,
        audio_kbps: 192,
        channels: Channels::Stereo,
        sound_only: true,
    },
    SitePreset {
        id: "sound-flac",
        name: "Sound only: lossless (FLAC)",
        note: "Just the project's sound, as a lossless FLAC file.",
        aspect: None,
        short_side: 0,
        fps: None,
        codec: VideoCodec::H264,
        container: Container::Mp4,
        bitrate: BitratePreset::Recommended,
        audio_format: AudioFormat::Flac,
        audio_kbps: 320,
        channels: Channels::Stereo,
        sound_only: true,
    },
];

/// The captions that fall in `start..end` (seconds on the timeline), as they
/// read in a file that begins at `start`: shifted back by `start` and cut off at
/// the edges. Captions wholly outside are left out.
pub fn cues_in_range(
    cues: &[(f64, f64, String)],
    start: f64,
    end: f64,
) -> Vec<(f64, f64, String)> {
    cues.iter()
        .filter(|(a, b, _)| *b > start && *a < end)
        .map(|(a, b, text)| ((a.max(start) - start), (b.min(end) - start), text.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_codec_has_formats_and_every_format_has_sound() {
        for codec in VideoCodec::ALL {
            assert!(!codec.containers().is_empty());
            for container in codec.containers() {
                assert!(!container.audio_formats().is_empty());
            }
        }
        // WebM only takes open formats.
        assert_eq!(Container::WebM.audio_formats(), &[AudioFormat::Opus]);
        assert!(!VideoCodec::H264.containers().contains(&Container::WebM));
        assert!(VideoCodec::Vp9.containers().contains(&Container::WebM));
    }

    #[test]
    fn choices_survive_being_saved_and_read_back() {
        for c in VideoCodec::ALL {
            assert_eq!(VideoCodec::from_id(c.id()), Some(c));
        }
        for c in Container::ALL {
            assert_eq!(Container::from_id(c.id()), Some(c));
        }
        for f in AudioFormat::ALL {
            assert_eq!(AudioFormat::from_id(f.id()), Some(f));
        }
        for c in Channels::ALL {
            assert_eq!(Channels::from_id(c.id()), Some(c));
        }
        for p in BitratePreset::ALL {
            assert_eq!(BitratePreset::from_id(p.id()), Some(p));
        }
        assert_eq!(
            BitratePreset::from_id("Small file"),
            Some(BitratePreset::Low),
            "an older saved quality name still works"
        );
        assert_eq!(VideoCodec::from_id("nonsense"), None);
    }

    #[test]
    fn sizes_depend_on_the_shape_and_run_from_ntsc_and_pal_up() {
        let wide = resolutions((16, 9), (1920, 1080));
        let names: Vec<_> = wide.iter().map(|r| (r.width, r.height)).collect();
        assert_eq!(names[0], (3840, 2160));
        assert!(names.contains(&(1920, 1080)));
        assert!(names.contains(&(854, 480)), "NTSC wide: {names:?}");
        assert!(names.contains(&(1024, 576)), "PAL wide: {names:?}");
        assert!(wide.iter().any(|r| r.label.contains("NTSC")));
        assert!(wide.iter().any(|r| r.label.contains("PAL")));
        let four_three = resolutions((4, 3), (1920, 1080));
        let names: Vec<_> = four_three.iter().map(|r| (r.width, r.height)).collect();
        assert!(names.contains(&(640, 480)), "NTSC 4:3: {names:?}");
        assert!(names.contains(&(768, 576)), "PAL 4:3: {names:?}");
        let tall = resolutions((9, 16), (1080, 1920));
        assert!(tall.iter().any(|r| (r.width, r.height) == (1080, 1920)));
        assert!(tall.iter().all(|r| r.height > r.width));
        for list in [&wide, &four_three, &tall] {
            for r in list.iter() {
                assert!(r.width % 2 == 0 && r.height % 2 == 0, "{r:?}");
            }
            let shorts: Vec<_> = list.iter().map(|r| r.width.min(r.height)).collect();
            assert!(shorts.windows(2).all(|w| w[0] >= w[1]), "biggest first");
        }
    }

    #[test]
    fn the_project_size_is_listed_and_chosen_first() {
        // An odd project size that is not one of the standard ones.
        let list = resolutions((16, 9), (1600, 900));
        let i = default_resolution(&list, (1600, 900));
        assert_eq!((list[i].width, list[i].height), (1600, 900));
        assert!(list[i].label.contains("project size"));
        // A standard one is marked rather than listed twice.
        let list = resolutions((9, 16), (1080, 1920));
        assert_eq!(
            list.iter().filter(|r| (r.width, r.height) == (1080, 1920)).count(),
            1
        );
        let i = default_resolution(&list, (1080, 1920));
        assert!(list[i].label.contains("(project)"));
        // Another shape starts on the size closest in quality.
        let list = resolutions((16, 9), (1080, 1920));
        let i = default_resolution(&list, (1080, 1920));
        assert_eq!(list[i].width.min(list[i].height), 1080);
    }

    #[test]
    fn the_shape_list_starts_with_the_projects_own() {
        let options = aspect_options((1080, 1920));
        assert_eq!(options[0].1, (9, 16));
        assert!(options[0].0.contains("project"));
        assert!(
            options.iter().skip(1).all(|(_, a)| *a != (9, 16)),
            "no duplicate of the project's shape"
        );
        assert!(options.iter().any(|(l, _)| l == "16:9"));
        let odd = aspect_options((1000, 700));
        assert!(odd[0].0.contains("10:7"), "{}", odd[0].0);
        assert_eq!(aspect_name(1920, 1080), "16:9");
        assert_eq!(aspect_name(1080, 1350), "4:5");
    }

    #[test]
    fn a_picture_in_another_shape_fits_or_fills_the_frame() {
        // Vertical project in a wide frame: it fits with bars at the sides.
        let fit = canvas_inside((1920, 1080), (1080, 1920), false);
        assert_eq!(fit, (608, 1080));
        assert!(fit.0 <= 1920 && fit.1 <= 1080);
        // ... or fills it, cropping top and bottom.
        let fill = canvas_inside((1920, 1080), (1080, 1920), true);
        assert_eq!(fill, (1920, 3414));
        assert!(fill.0 >= 1920 && fill.1 >= 1080);
        // Same shape: nothing to do.
        assert_eq!(canvas_inside((720, 1280), (1080, 1920), false), (720, 1280));
        assert_eq!(canvas_inside((1920, 1080), (1920, 1080), true), (1920, 1080));
    }

    #[test]
    fn bit_rate_follows_size_rate_preset_and_codec() {
        let a = video_kbps(VideoCodec::H264, 1080, 1920, 30.0, BitratePreset::Recommended);
        assert!((7_000..8_000).contains(&a), "{a}");
        assert!(video_kbps(VideoCodec::H264, 1080, 1920, 30.0, BitratePreset::Low) < a);
        assert!(video_kbps(VideoCodec::H264, 1080, 1920, 30.0, BitratePreset::Maximum) > a);
        let h265 = video_kbps(VideoCodec::H265, 1080, 1920, 30.0, BitratePreset::Recommended);
        let av1 = video_kbps(VideoCodec::Av1, 1080, 1920, 30.0, BitratePreset::Recommended);
        assert!(av1 < h265 && h265 < a);
        assert_eq!(
            video_kbps(VideoCodec::H264, 7680, 4320, 60.0, BitratePreset::Maximum),
            80_000
        );
        assert_eq!(mbps(7_800), "7.8 Mbps");
    }

    #[test]
    fn frame_rates_start_with_the_projects_without_repeats() {
        let rates = frame_rates(FrameRate::FPS_30);
        assert_eq!(rates[0], FrameRate::FPS_30);
        assert_eq!(
            rates
                .iter()
                .filter(|r| (r.as_f64() - 30.0).abs() < 0.001)
                .count(),
            1
        );
        assert!(rates.iter().any(|r| *r == FrameRate::NTSC_29_97));
        let odd = frame_rates(FrameRate::NTSC_29_97);
        assert_eq!(odd[0], FrameRate::NTSC_29_97);
        assert_eq!(
            odd.iter()
                .filter(|r| (r.as_f64() - 29.97).abs() < 0.001)
                .count(),
            1
        );
    }

    #[test]
    fn estimated_sizes_add_picture_and_sound() {
        let bytes = estimated_bytes(10.0, Some(8000), Some(192));
        assert!((bytes - 10_240_000.0).abs() < 1.0);
        assert!((estimated_bytes(10.0, None, Some(192)) - 240_000.0).abs() < 1.0);
        assert_eq!(lossless_kbps(AudioFormat::Wav, Channels::Stereo, 48_000), 1536);
        assert_eq!(lossless_kbps(AudioFormat::Flac, Channels::Mono, 48_000), 384);
    }

    #[test]
    fn captions_are_cut_to_the_chosen_part_and_start_from_zero() {
        let cues = vec![
            (0.0, 2.0, "before".to_string()),
            (3.0, 6.0, "across the start".to_string()),
            (7.0, 8.0, "inside".to_string()),
            (9.0, 12.0, "across the end".to_string()),
            (13.0, 14.0, "after".to_string()),
        ];
        let part = cues_in_range(&cues, 5.0, 10.0);
        let words: Vec<&str> = part.iter().map(|c| c.2.as_str()).collect();
        assert_eq!(words, ["across the start", "inside", "across the end"]);
        assert_eq!((part[0].0, part[0].1), (0.0, 1.0));
        assert_eq!((part[1].0, part[1].1), (2.0, 3.0));
        assert_eq!((part[2].0, part[2].1), (4.0, 5.0));
    }

    #[test]
    fn every_preset_asks_for_things_that_fit_together() {
        let mut ids = std::collections::HashSet::new();
        for p in SITE_PRESETS {
            assert!(ids.insert(p.id), "{} is listed twice", p.id);
            assert!(!p.name.is_empty() && !p.note.is_empty());
            if !p.sound_only {
                assert!(
                    p.codec.containers().contains(&p.container),
                    "{}: {} cannot go in {}",
                    p.name,
                    p.codec.name(),
                    p.container.label()
                );
                assert!(
                    p.container.audio_formats().contains(&p.audio_format),
                    "{}: {} cannot go in {}",
                    p.name,
                    p.audio_format.label(),
                    p.container.label()
                );
                assert!(
                    p.short_side == 0 || TIERS.iter().any(|t| t.0 == p.short_side),
                    "{}: {} is not a size on offer",
                    p.name,
                    p.short_side
                );
                // A shape the list offers.
                if let Some(a) = p.aspect {
                    assert!(ASPECTS.contains(&a), "{}: {a:?}", p.name);
                }
            }
            if p.audio_format.is_lossy() {
                assert!(AUDIO_BITRATES.contains(&p.audio_kbps), "{}", p.name);
            }
        }
        // A preset's size really is what its note says.
        let shorts = SITE_PRESETS.iter().find(|p| p.id == "youtube-shorts").unwrap();
        let list = resolutions(shorts.aspect.unwrap(), (1080, 1920));
        assert!(list.iter().any(|r| (r.width, r.height) == (1080, 1920)));
        let feed = SITE_PRESETS.iter().find(|p| p.id == "instagram-feed").unwrap();
        let list = resolutions(feed.aspect.unwrap(), (1080, 1920));
        assert!(list.iter().any(|r| (r.width, r.height) == (1080, 1350)), "{list:?}");
    }
}
