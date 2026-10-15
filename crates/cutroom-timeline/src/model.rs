use crate::ids::{ClipId, MediaId, TrackId};
use crate::time::Ticks;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackKind {
    Video,
    Audio,
}

/// How a clip's picture is placed on the canvas. Whole-number units keep the
/// model exact (and `Eq`) and independent of any preview resolution.
///
/// The picture is first fitted inside the canvas (letterboxed, never cropped),
/// then scaled, turned and moved from there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transform {
    /// Horizontal offset of the picture's centre from the canvas centre, in
    /// thousandths of the canvas width (1000 = a whole canvas to the right).
    pub x: i32,
    /// Vertical offset, in thousandths of the canvas height (down is positive).
    pub y: i32,
    /// Size relative to the fitted picture, in thousandths (1000 = fitted).
    pub scale: i32,
    /// Clockwise rotation in tenths of a degree.
    pub rotation: i32,
    /// Opacity in thousandths (1000 = opaque).
    pub opacity: i32,
    pub flip_h: bool,
    pub flip_v: bool,
    /// Slant along the x axis, in tenths of a degree (positive leans the top to the right).
    pub skew_x: i32,
    /// Slant along the y axis, in tenths of a degree (positive tilts the right side down).
    pub skew_y: i32,
    /// Perspective along x, in thousandths: positive makes the right side
    /// smaller (farther away), negative the left.
    pub persp_x: i32,
    /// Perspective along y: positive makes the bottom smaller, negative the top.
    pub persp_y: i32,
}

impl Default for Transform {
    fn default() -> Transform {
        Transform {
            x: 0,
            y: 0,
            scale: 1000,
            rotation: 0,
            opacity: 1000,
            flip_h: false,
            flip_v: false,
            skew_x: 0,
            skew_y: 0,
            persp_x: 0,
            persp_y: 0,
        }
    }
}

impl Transform {
    pub const SKEW_MAX: i32 = 600;
    pub const PERSP_MAX: i32 = 800;

    pub const SCALE_MIN: i32 = 10;
    pub const SCALE_MAX: i32 = 10_000;

    /// The same transform with every number brought into its legal range.
    pub fn clamped(self) -> Transform {
        // Rotation wraps to -180.0..=180.0 degrees.
        let turned = (self.rotation + 1800).rem_euclid(3600) - 1800;
        Transform {
            x: self.x.clamp(-5000, 5000),
            y: self.y.clamp(-5000, 5000),
            scale: self.scale.clamp(Self::SCALE_MIN, Self::SCALE_MAX),
            rotation: turned,
            opacity: self.opacity.clamp(0, 1000),
            skew_x: self.skew_x.clamp(-Self::SKEW_MAX, Self::SKEW_MAX),
            skew_y: self.skew_y.clamp(-Self::SKEW_MAX, Self::SKEW_MAX),
            persp_x: self.persp_x.clamp(-Self::PERSP_MAX, Self::PERSP_MAX),
            persp_y: self.persp_y.clamp(-Self::PERSP_MAX, Self::PERSP_MAX),
            ..self
        }
    }

    pub fn is_default(&self) -> bool {
        *self == Transform::default()
    }
}

/// A clip's sound as recorded.
pub const DEFAULT_VOLUME: i32 = 1000;
/// The loudest a clip can be made (double).
pub const MAX_VOLUME: i32 = 2000;

fn default_volume() -> i32 {
    DEFAULT_VOLUME
}

/// A span of source media placed on a track.
///
/// v0 limitation: no speed changes yet, so source and timeline durations match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub media: MediaId,
    /// Offset into the source media where this clip begins.
    pub source_in: Ticks,
    /// Position on the timeline.
    pub start: Ticks,
    pub duration: Ticks,
    /// The clip's own sound is switched off (for example after "Extract
    /// audio", which puts the sound on an audio track instead).
    #[serde(default)]
    pub audio_muted: bool,
    /// Sound level in thousandths: 1000 is as recorded, 0 silent, 2000 doubled.
    #[serde(default = "default_volume")]
    pub volume: i32,
    /// Where and how the picture sits on the canvas.
    #[serde(default)]
    pub transform: Transform,
    /// Deactivated: the clip stays on the timeline but shows and plays nothing.
    #[serde(default)]
    pub disabled: bool,
    /// Length of the crossfade with the clip before it (zero for none); see
    /// [`crate::transition`].
    #[serde(default)]
    pub crossfade: Ticks,
    /// Settings that change over the clip; see [`crate::keyframes`].
    #[serde(default)]
    pub keyframes: Vec<crate::keyframes::Keyframe>,
    /// Set for a text clip: the words to draw (the clip then has no media file).
    #[serde(default)]
    pub text: Option<crate::text::TextSpec>,
    /// For a caption: when each word is said, measured from the start of the
    /// clip. Empty for ordinary text.
    #[serde(default)]
    pub words: Vec<crate::text::Word>,
    /// Set when the text is a caption: the name of its caption style (empty
    /// when it has been customised). Spoken-word clips are always captions.
    #[serde(default)]
    pub caption: Option<String>,
    /// No longer used (a caption cannot be switched back to plain text); kept
    /// so projects saved by builds 31 and 32 still load.
    #[serde(default)]
    pub caption_off: bool,
}

impl Clip {
    pub fn end(&self) -> Ticks {
        self.start + self.duration
    }

    /// Is this text a caption (for the caption settings and subtitle files)?
    pub fn is_caption(&self) -> bool {
        self.text.is_some() && (self.caption.is_some() || !self.words.is_empty())
    }

    /// The picture settings at timeline time `at`: the clip's own, except
    /// those that have keyframes, which follow them.
    pub fn transform_at(&self, at: Ticks) -> Transform {
        if self.keyframes.is_empty() {
            return self.transform;
        }
        let rel = at - self.start;
        let mut t = self.transform;
        for prop in crate::keyframes::Prop::ALL {
            if let Some(v) = crate::keyframes::value_at(&self.keyframes, prop, rel) {
                prop.set(&mut t, v);
            }
        }
        t.clamped()
    }

    /// The words and look at timeline time `at`: the clip's own, except the
    /// settings that have keyframes (letter size and spacing), which follow them.
    pub fn text_at(&self, at: Ticks) -> Option<crate::text::TextSpec> {
        let mut spec = self.text.clone()?;
        if !self.keyframes.is_empty() {
            let rel = at - self.start;
            for prop in crate::keyframes::Prop::TEXT {
                if let Some(v) = crate::keyframes::value_at(&self.keyframes, prop, rel) {
                    prop.set_text(&mut spec, v);
                }
            }
        }
        let mut spec = spec.clamped();
        spec.active = self.active_range(&spec, at);
        Some(spec)
    }

    /// The part of the text to light up at `at`, if the clip is a caption
    /// with highlighting on.
    fn active_range(&self, spec: &crate::text::TextSpec, at: Ticks) -> Option<(usize, usize)> {
        if self.words.is_empty() || spec.highlight[3] == 0 {
            return None;
        }
        // Only if the words still spell the text (they do unless it was edited
        // in a way that has not been re-timed).
        if crate::text::join_words(&self.words) != spec.content {
            return None;
        }
        let rel = at - self.start;
        let ranges = crate::text::word_ranges(&self.words);
        let spoken = self.words.iter().rposition(|w| w.start <= rel)?;
        let (start, end) = ranges[spoken];
        match spec.highlight_mode {
            crate::text::HighlightMode::Word => {
                // Lit while being said; a short hold after it, until the next word begins.
                let w = &self.words[spoken];
                let next_start = self.words.get(spoken + 1).map(|n| n.start);
                let hold = w.end.max(next_start.map_or(w.end, |n| {
                    n.min(w.end + Ticks(crate::time::TICKS_PER_SECOND / 4))
                }));
                (rel < hold).then_some((start, end))
            }
            crate::text::HighlightMode::Karaoke => Some((ranges[0].0, end)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    pub name: String,
    pub muted: bool,
    pub locked: bool,
    /// Hidden: nothing on the track is shown or heard.
    #[serde(default)]
    pub hidden: bool,
    /// Always sorted by `start` and never overlapping.
    pub clips: Vec<Clip>,
}

impl Track {
    pub fn new(id: TrackId, kind: TrackKind, name: impl Into<String>) -> Self {
        Track {
            id,
            kind,
            name: name.into(),
            muted: false,
            locked: false,
            hidden: false,
            clips: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Timeline {
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub(crate) next_id: u64,
}

impl Timeline {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Creates a track with a fresh id. It is **not** inserted; pass it to
    /// `Edit::InsertTrack` so the change is undoable.
    pub fn new_track(&mut self, kind: TrackKind, name: impl Into<String>) -> Track {
        Track::new(TrackId(self.alloc_id()), kind, name)
    }

    /// Creates a clip with a fresh id. It is **not** inserted; pass it to
    /// `Edit::InsertClip`.
    pub fn new_clip(
        &mut self,
        media: MediaId,
        source_in: Ticks,
        start: Ticks,
        duration: Ticks,
    ) -> Clip {
        Clip {
            id: ClipId(self.alloc_id()),
            media,
            source_in,
            start,
            duration,
            audio_muted: false,
            volume: DEFAULT_VOLUME,
            transform: Transform::default(),
            disabled: false,
            crossfade: Ticks::ZERO,
            keyframes: Vec::new(),
            text: None,
            words: Vec::new(),
            caption: None,
            caption_off: false,
        }
    }

    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn find_clip(&self, id: ClipId) -> Option<(&Track, &Clip)> {
        self.tracks
            .iter()
            .find_map(|t| t.clips.iter().find(|c| c.id == id).map(|c| (t, c)))
    }

    pub fn locate_clip(&self, id: ClipId) -> Option<(usize, usize)> {
        self.tracks
            .iter()
            .enumerate()
            .find_map(|(ti, t)| t.clips.iter().position(|c| c.id == id).map(|ci| (ti, ci)))
    }

    /// End of the last clip on any track.
    pub fn duration(&self) -> Ticks {
        self.tracks
            .iter()
            .filter_map(|t| t.clips.last())
            .map(Clip::end)
            .max()
            .unwrap_or(Ticks::ZERO)
    }
}
