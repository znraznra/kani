//! Crossfades between two clips that touch on a track.
//!
//! A crossfade belongs to the clip that comes second (`Clip::crossfade` is its
//! length) and works across the cut between it and the clip before it. It
//! spans half its length on each side of the cut.
//!
//! * If both clips have spare footage there (the second starts at least half
//!   a crossfade into its file, and the first file goes on past its end), the
//!   two pictures overlap and the second fades in over the first: a true
//!   crossfade.
//! * If not, each clip fades through its own ends: the first fades out as the
//!   cut nears, the second fades in after it.

use crate::model::{Clip, Track};
use crate::time::Ticks;

/// The longest crossfade, so a slip of the slider can't swallow a whole clip.
pub const MAX_CROSSFADE: Ticks = Ticks(crate::time::TICKS_PER_SECOND * 10);

/// How the two clips look and sound at one moment of a crossfade.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mix {
    /// Show the first clip (with its opacity and volume).
    pub first: Option<Level>,
    /// Show the second clip.
    pub second: Option<Level>,
    /// Both pictures overlap, so the first clip runs past its end and the
    /// second starts before its beginning.
    pub overlapping: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    pub opacity: f64,
    pub volume: f64,
}

/// The clip before `index` if it touches it (the cut a crossfade works on).
pub fn previous_touching(track: &Track, index: usize) -> Option<&Clip> {
    let this = track.clips.get(index)?;
    let before = track.clips.get(index.checked_sub(1)?)?;
    (before.end() == this.start).then_some(before)
}

/// The largest crossfade the clip at `index` can have: the two clips can each
/// give up half of it from their own length at worst.
pub fn max_crossfade(track: &Track, index: usize) -> Ticks {
    let Some(before) = previous_touching(track, index) else {
        return Ticks::ZERO;
    };
    let this = &track.clips[index];
    let cap = Ticks(before.duration.0.min(this.duration.0).saturating_mul(2));
    Ticks(cap.0.min(MAX_CROSSFADE.0))
}

/// The crossfade that really applies to the clip at `index` (zero if it has
/// none, or its cut is no longer a touching one).
pub fn effective(track: &Track, index: usize) -> Ticks {
    let Some(clip) = track.clips.get(index) else {
        return Ticks::ZERO;
    };
    if clip.crossfade <= Ticks::ZERO || clip.disabled {
        return Ticks::ZERO;
    }
    match previous_touching(track, index) {
        Some(before) if !before.disabled => {
            Ticks(clip.crossfade.0.min(max_crossfade(track, index).0))
        }
        _ => Ticks::ZERO,
    }
}

/// The span of time in which a clip's crossfade with the one before is
/// working, if it has one.
pub fn window(track: &Track, index: usize) -> Option<(Ticks, Ticks)> {
    let d = effective(track, index);
    if d <= Ticks::ZERO {
        return None;
    }
    let cut = track.clips[index].start;
    let half = Ticks(d.0 / 2);
    Some((cut - half, cut + half))
}

/// What to show and play at time `t` for the crossfade at `index`. `None`
/// outside its window.
pub fn mix_at(track: &Track, index: usize, t: Ticks) -> Option<Mix> {
    let (from, to) = window(track, index)?;
    if t < from || t >= to {
        return None;
    }
    let before = previous_touching(track, index)?;
    let this = &track.clips[index];
    let cut = this.start;
    let d = (to - from).0 as f64;
    let half = Ticks((to - from).0 / 2);
    // Is there spare footage on both sides of the cut?
    let overlapping = this.source_in >= half && before.source_in >= Ticks::ZERO;
    if overlapping {
        let p = ((t - from).0 as f64 / d).clamp(0.0, 1.0);
        let quarter = p * std::f64::consts::FRAC_PI_2;
        return Some(Mix {
            first: Some(Level {
                opacity: 1.0,
                volume: quarter.cos(),
            }),
            second: Some(Level {
                opacity: p,
                volume: quarter.sin(),
            }),
            overlapping: true,
        });
    }
    let h = half.0 as f64;
    if t < cut {
        let f = ((cut - t).0 as f64 / h).clamp(0.0, 1.0);
        Some(Mix {
            first: Some(Level {
                opacity: f,
                volume: f,
            }),
            second: None,
            overlapping: false,
        })
    } else {
        let f = ((t - cut).0 as f64 / h).clamp(0.0, 1.0);
        Some(Mix {
            first: None,
            second: Some(Level {
                opacity: f,
                volume: f,
            }),
            overlapping: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ClipId, MediaId, TrackId};
    use crate::model::TrackKind;

    fn s(secs: f64) -> Ticks {
        Ticks::from_seconds_f64(secs)
    }

    fn clip(id: u64, source_in: f64, start: f64, duration: f64) -> Clip {
        Clip {
            id: ClipId(id),
            media: MediaId(1),
            source_in: s(source_in),
            start: s(start),
            duration: s(duration),
            audio_muted: false,
            volume: 1000,
            transform: Default::default(),
            disabled: false,
            crossfade: Ticks::ZERO,
            keyframes: Vec::new(),
            text: None,
            words: Vec::new(),
            caption: None,
            caption_off: false,
        }
    }

    fn track(a: Clip, b: Clip) -> Track {
        let mut t = Track::new(TrackId(1), TrackKind::Video, "V");
        t.clips = vec![a, b];
        t
    }

    #[test]
    fn only_touching_clips_can_crossfade() {
        let mut t = track(clip(2, 0.0, 0.0, 4.0), clip(3, 4.0, 4.0, 4.0));
        t.clips[1].crossfade = s(1.0);
        assert_eq!(effective(&t, 1), s(1.0));
        t.clips[1].start = s(5.0);
        assert_eq!(effective(&t, 1), Ticks::ZERO);
        assert_eq!(max_crossfade(&t, 1), Ticks::ZERO);
    }

    #[test]
    fn it_is_capped_by_the_shorter_clip_and_the_global_maximum() {
        let mut t = track(clip(2, 0.0, 0.0, 1.0), clip(3, 0.0, 1.0, 30.0));
        t.clips[1].crossfade = s(8.0);
        assert_eq!(effective(&t, 1), s(2.0));
        let mut long = track(clip(2, 0.0, 0.0, 60.0), clip(3, 0.0, 60.0, 60.0));
        long.clips[1].crossfade = s(30.0);
        assert_eq!(effective(&long, 1), MAX_CROSSFADE);
    }

    #[test]
    fn with_spare_footage_both_pictures_overlap_and_the_second_fades_in() {
        let mut t = track(clip(2, 0.0, 0.0, 4.0), clip(3, 4.0, 4.0, 4.0));
        t.clips[1].crossfade = s(2.0); // window 3.0 .. 5.0
        assert_eq!(mix_at(&t, 1, s(2.9)), None);
        assert_eq!(mix_at(&t, 1, s(5.0)), None);
        let start = mix_at(&t, 1, s(3.0)).unwrap();
        assert!(start.overlapping);
        assert_eq!(start.second.unwrap().opacity, 0.0);
        assert_eq!(start.first.unwrap().opacity, 1.0);
        let mid = mix_at(&t, 1, s(4.0)).unwrap();
        assert!((mid.second.unwrap().opacity - 0.5).abs() < 1e-9);
        // Equal power: the two volumes keep the loudness steady.
        let (a, b) = (mid.first.unwrap().volume, mid.second.unwrap().volume);
        assert!((a * a + b * b - 1.0).abs() < 1e-9);
    }

    #[test]
    fn without_spare_footage_each_clip_fades_through_its_own_end() {
        let mut t = track(clip(2, 0.0, 0.0, 4.0), clip(3, 0.0, 4.0, 4.0));
        t.clips[1].crossfade = s(2.0);
        let before_cut = mix_at(&t, 1, s(3.5)).unwrap();
        assert!(!before_cut.overlapping && before_cut.second.is_none());
        assert!((before_cut.first.unwrap().opacity - 0.5).abs() < 1e-9);
        let after_cut = mix_at(&t, 1, s(4.5)).unwrap();
        assert!(after_cut.first.is_none());
        assert!((after_cut.second.unwrap().opacity - 0.5).abs() < 1e-9);
        assert_eq!(mix_at(&t, 1, s(4.0)).unwrap().second.unwrap().opacity, 0.0);
    }

    #[test]
    fn deactivated_neighbours_switch_it_off() {
        let mut t = track(clip(2, 0.0, 0.0, 4.0), clip(3, 4.0, 4.0, 4.0));
        t.clips[1].crossfade = s(1.0);
        t.clips[0].disabled = true;
        assert_eq!(effective(&t, 1), Ticks::ZERO);
    }
}
