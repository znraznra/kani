use crate::error::EditError;
use crate::ids::{ClipId, TrackId};
use crate::model::{Clip, Timeline, Track, Transform};
use crate::time::Ticks;
use serde::{Deserialize, Serialize};

/// A single, reversible change to a [`Timeline`].
///
/// [`apply`] performs the edit and returns its exact inverse, which is what
/// powers undo/redo. Higher-level operations (split, ripple delete, trim) are
/// built as `Edit::Batch` values from primitives, so there is only one place
/// where the timeline is mutated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edit {
    InsertTrack {
        index: usize,
        track: Track,
    },
    RemoveTrack {
        track: TrackId,
    },
    SetTrackFlags {
        track: TrackId,
        muted: bool,
        locked: bool,
    },
    /// Replace a clip's keyframes.
    SetClipKeyframes {
        clip: ClipId,
        keyframes: Vec<crate::keyframes::Keyframe>,
    },
    /// Set the length of the crossfade a clip has with the one before it.
    SetClipCrossfade {
        clip: ClipId,
        crossfade: Ticks,
    },
    /// Change the words and look of a text clip.
    SetClipText {
        clip: ClipId,
        text: Option<crate::text::TextSpec>,
    },
    /// Change when a caption's words are said.
    SetClipWords {
        clip: ClipId,
        words: Vec<crate::text::Word>,
    },
    /// Move a track to another place in the list (index after the move).
    MoveTrack {
        track: TrackId,
        index: usize,
    },
    /// Give a track a new name.
    RenameTrack {
        track: TrackId,
        name: String,
    },
    /// Hide or show a whole track.
    SetTrackHidden {
        track: TrackId,
        hidden: bool,
    },
    InsertClip {
        track: TrackId,
        clip: Clip,
    },
    RemoveClip {
        clip: ClipId,
    },
    /// Move and/or retime an existing clip, optionally onto another track.
    SetClip {
        clip: ClipId,
        track: TrackId,
        start: Ticks,
        duration: Ticks,
        source_in: Ticks,
    },
    /// Move, scale, turn or fade a clip's picture.
    SetClipTransform {
        clip: ClipId,
        transform: Transform,
    },
    /// Switch a clip's own sound on or off.
    /// Change a clip's sound level (thousandths).
    SetClipVolume {
        clip: ClipId,
        volume: i32,
    },
    SetClipAudioMuted {
        clip: ClipId,
        muted: bool,
    },
    /// Mark a text clip as a caption (with its style's name), or not.
    SetClipCaption {
        clip: ClipId,
        caption: Option<String>,
        off: bool,
    },
    /// Deactivate (or reactivate) a clip.
    SetClipDisabled {
        clip: ClipId,
        disabled: bool,
    },
    /// Applied in order; if any step fails, earlier steps are rolled back.
    Batch(Vec<Edit>),
}

fn validate_range(start: Ticks, duration: Ticks, source_in: Ticks) -> Result<(), EditError> {
    if duration <= Ticks::ZERO {
        return Err(EditError::InvalidRange("duration must be positive"));
    }
    if start < Ticks::ZERO {
        return Err(EditError::InvalidRange("start must not be negative"));
    }
    if source_in < Ticks::ZERO {
        return Err(EditError::InvalidRange("source_in must not be negative"));
    }
    if start.0.checked_add(duration.0).is_none() || source_in.0.checked_add(duration.0).is_none() {
        return Err(EditError::InvalidRange("range overflows"));
    }
    Ok(())
}

fn insert_sorted(track: &mut Track, clip: Clip) -> Result<(), EditError> {
    let pos = track.clips.partition_point(|c| c.start < clip.start);
    if pos > 0 {
        let prev = &track.clips[pos - 1];
        if prev.end() > clip.start {
            return Err(EditError::Overlap {
                track: track.id,
                with: prev.id,
            });
        }
    }
    if let Some(next) = track.clips.get(pos) {
        if next.start < clip.end() {
            return Err(EditError::Overlap {
                track: track.id,
                with: next.id,
            });
        }
    }
    track.clips.insert(pos, clip);
    Ok(())
}

fn id_in_use(tl: &Timeline, id: u64) -> bool {
    tl.tracks
        .iter()
        .any(|t| t.id.0 == id || t.clips.iter().any(|c| c.id.0 == id))
}

/// Applies `edit` and returns its inverse. On error the timeline is unchanged.
pub fn apply(tl: &mut Timeline, edit: Edit) -> Result<Edit, EditError> {
    match edit {
        Edit::InsertTrack { index, track } => {
            if index > tl.tracks.len() {
                return Err(EditError::IndexOutOfRange);
            }
            if id_in_use(tl, track.id.0) {
                return Err(EditError::DuplicateId(track.id.0));
            }
            let mut previous_end = Ticks::ZERO;
            for clip in &track.clips {
                validate_range(clip.start, clip.duration, clip.source_in)?;
                if id_in_use(tl, clip.id.0) {
                    return Err(EditError::DuplicateId(clip.id.0));
                }
                if clip.start < previous_end {
                    return Err(EditError::InvalidRange(
                        "track clips must be sorted and disjoint",
                    ));
                }
                previous_end = clip.end();
            }
            let highest = track
                .clips
                .iter()
                .map(|c| c.id.0)
                .chain([track.id.0])
                .max()
                .unwrap_or(0);
            tl.next_id = tl.next_id.max(highest);
            let id = track.id;
            tl.tracks.insert(index, track);
            Ok(Edit::RemoveTrack { track: id })
        }
        Edit::RemoveTrack { track } => {
            let index = tl
                .tracks
                .iter()
                .position(|t| t.id == track)
                .ok_or(EditError::TrackNotFound(track))?;
            let removed = tl.tracks.remove(index);
            Ok(Edit::InsertTrack {
                index,
                track: removed,
            })
        }
        Edit::SetTrackFlags {
            track,
            muted,
            locked,
        } => {
            let t = tl
                .tracks
                .iter_mut()
                .find(|t| t.id == track)
                .ok_or(EditError::TrackNotFound(track))?;
            let inverse = Edit::SetTrackFlags {
                track,
                muted: t.muted,
                locked: t.locked,
            };
            t.muted = muted;
            t.locked = locked;
            Ok(inverse)
        }
        Edit::SetClipKeyframes { clip, keyframes } => {
            let (ti, ci) = tl.locate_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
            let c = &mut tl.tracks[ti].clips[ci];
            let inverse = Edit::SetClipKeyframes {
                clip,
                keyframes: std::mem::replace(&mut c.keyframes, keyframes),
            };
            Ok(inverse)
        }
        Edit::SetClipCrossfade { clip, crossfade } => {
            if crossfade < Ticks::ZERO {
                return Err(EditError::InvalidRange("crossfade must not be negative"));
            }
            let (ti, ci) = tl.locate_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
            let c = &mut tl.tracks[ti].clips[ci];
            let inverse = Edit::SetClipCrossfade {
                clip,
                crossfade: c.crossfade,
            };
            c.crossfade = crossfade;
            Ok(inverse)
        }
        Edit::SetClipText { clip, text } => {
            let (ti, ci) = tl.locate_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
            let c = &mut tl.tracks[ti].clips[ci];
            let inverse = Edit::SetClipText {
                clip,
                text: c.text.clone(),
            };
            c.text = text.map(crate::text::TextSpec::clamped);
            Ok(inverse)
        }
        Edit::SetClipWords { clip, words } => {
            let (ti, ci) = tl.locate_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
            let c = &mut tl.tracks[ti].clips[ci];
            let inverse = Edit::SetClipWords {
                clip,
                words: std::mem::replace(&mut c.words, words),
            };
            Ok(inverse)
        }
        Edit::MoveTrack { track, index } => {
            let from = tl
                .tracks
                .iter()
                .position(|t| t.id == track)
                .ok_or(EditError::TrackNotFound(track))?;
            if index >= tl.tracks.len() {
                return Err(EditError::IndexOutOfRange);
            }
            let moved = tl.tracks.remove(from);
            tl.tracks.insert(index, moved);
            Ok(Edit::MoveTrack { track, index: from })
        }
        Edit::RenameTrack { track, name } => {
            let t = tl
                .tracks
                .iter_mut()
                .find(|t| t.id == track)
                .ok_or(EditError::TrackNotFound(track))?;
            let inverse = Edit::RenameTrack {
                track,
                name: std::mem::replace(&mut t.name, name),
            };
            Ok(inverse)
        }
        Edit::SetTrackHidden { track, hidden } => {
            let t = tl
                .tracks
                .iter_mut()
                .find(|t| t.id == track)
                .ok_or(EditError::TrackNotFound(track))?;
            let inverse = Edit::SetTrackHidden {
                track,
                hidden: t.hidden,
            };
            t.hidden = hidden;
            Ok(inverse)
        }
        Edit::SetClipTransform { clip, transform } => {
            let (ti, ci) = tl.locate_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
            let c = &mut tl.tracks[ti].clips[ci];
            let inverse = Edit::SetClipTransform {
                clip,
                transform: c.transform,
            };
            c.transform = transform.clamped();
            Ok(inverse)
        }
        Edit::SetClipVolume { clip, volume } => {
            let (ti, ci) = tl.locate_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
            let c = &mut tl.tracks[ti].clips[ci];
            let inverse = Edit::SetClipVolume {
                clip,
                volume: c.volume,
            };
            c.volume = volume.clamp(0, crate::model::MAX_VOLUME);
            Ok(inverse)
        }
        Edit::SetClipCaption { clip, caption, off } => {
            let (ti, ci) = tl.locate_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
            let c = &mut tl.tracks[ti].clips[ci];
            let inverse = Edit::SetClipCaption {
                clip,
                caption: c.caption.take(),
                off: c.caption_off,
            };
            c.caption = caption;
            c.caption_off = off;
            Ok(inverse)
        }
        Edit::SetClipAudioMuted { clip, muted } => {
            let (ti, ci) = tl.locate_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
            let c = &mut tl.tracks[ti].clips[ci];
            let inverse = Edit::SetClipAudioMuted {
                clip,
                muted: c.audio_muted,
            };
            c.audio_muted = muted;
            Ok(inverse)
        }
        Edit::SetClipDisabled { clip, disabled } => {
            let (ti, ci) = tl.locate_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
            let c = &mut tl.tracks[ti].clips[ci];
            let inverse = Edit::SetClipDisabled {
                clip,
                disabled: c.disabled,
            };
            c.disabled = disabled;
            Ok(inverse)
        }
        Edit::InsertClip { track, clip } => {
            validate_range(clip.start, clip.duration, clip.source_in)?;
            if id_in_use(tl, clip.id.0) {
                return Err(EditError::DuplicateId(clip.id.0));
            }
            let t = tl
                .tracks
                .iter_mut()
                .find(|t| t.id == track)
                .ok_or(EditError::TrackNotFound(track))?;
            if t.locked {
                return Err(EditError::TrackLocked(track));
            }
            let id = clip.id;
            insert_sorted(t, clip)?;
            tl.next_id = tl.next_id.max(id.0);
            Ok(Edit::RemoveClip { clip: id })
        }
        Edit::RemoveClip { clip } => {
            let (ti, ci) = tl.locate_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
            let track = &mut tl.tracks[ti];
            if track.locked {
                return Err(EditError::TrackLocked(track.id));
            }
            let removed = track.clips.remove(ci);
            Ok(Edit::InsertClip {
                track: track.id,
                clip: removed,
            })
        }
        Edit::SetClip {
            clip,
            track,
            start,
            duration,
            source_in,
        } => {
            validate_range(start, duration, source_in)?;
            let (ti, ci) = tl.locate_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
            if tl.tracks[ti].locked {
                return Err(EditError::TrackLocked(tl.tracks[ti].id));
            }
            let di = tl
                .tracks
                .iter()
                .position(|t| t.id == track)
                .ok_or(EditError::TrackNotFound(track))?;
            if tl.tracks[di].locked {
                return Err(EditError::TrackLocked(track));
            }
            let old = tl.tracks[ti].clips.remove(ci);
            let inverse = Edit::SetClip {
                clip,
                track: tl.tracks[ti].id,
                start: old.start,
                duration: old.duration,
                source_in: old.source_in,
            };
            let updated = Clip {
                start,
                duration,
                source_in,
                ..old.clone()
            };
            match insert_sorted(&mut tl.tracks[di], updated) {
                Ok(()) => Ok(inverse),
                Err(e) => {
                    // Nothing was inserted, so putting the clip back at its
                    // old index restores the exact previous state.
                    tl.tracks[ti].clips.insert(ci, old);
                    Err(e)
                }
            }
        }
        Edit::Batch(edits) => {
            let mut inverses: Vec<Edit> = Vec::with_capacity(edits.len());
            for edit in edits {
                match apply(tl, edit) {
                    Ok(inverse) => inverses.push(inverse),
                    Err(e) => {
                        for inverse in inverses.into_iter().rev() {
                            // Inverses of just-applied edits cannot fail.
                            let _ = apply(tl, inverse);
                        }
                        return Err(e);
                    }
                }
            }
            inverses.reverse();
            Ok(Edit::Batch(inverses))
        }
    }
}

/// Builders for common editing operations. They never touch the clips
/// themselves; they only return an [`Edit`] to run through [`History`](crate::History).
impl Timeline {
    /// Cut `clip` in two at timeline time `at`.
    pub fn split_edit(&mut self, clip: ClipId, at: Ticks) -> Result<Edit, EditError> {
        let (
            track_id,
            media,
            source_in,
            start,
            end,
            audio_muted,
            volume,
            transform,
            disabled,
            keyframes,
            text,
            words,
        ) = {
            let (track, c) = self.find_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
            (
                track.id,
                c.media,
                c.source_in,
                c.start,
                c.end(),
                c.audio_muted,
                c.volume,
                c.transform,
                c.disabled,
                c.keyframes.clone(),
                c.text.clone(),
                c.words.clone(),
            )
        };
        if at <= start || at >= end {
            return Err(EditError::SplitOutOfRange);
        }
        let (left_keys, right_keys) = crate::keyframes::split(&keyframes, at - start);
        // A caption's words go to the half they are said in; each half's text
        // is the words it keeps.
        let cut = at - start;
        let (left_words, right_words, left_text, right_text) = if words.is_empty() {
            (Vec::new(), Vec::new(), text.clone(), text.clone())
        } else {
            let (mut l, mut r) = (Vec::new(), Vec::new());
            for w in &words {
                let mid = Ticks((w.start.0 + w.end.0) / 2);
                if mid < cut {
                    l.push(crate::text::Word {
                        end: w.end.min(cut),
                        ..w.clone()
                    });
                } else {
                    r.push(crate::text::Word {
                        text: w.text.clone(),
                        start: (w.start - cut).max(Ticks::ZERO),
                        end: w.end - cut,
                    });
                }
            }
            let with = |ws: &[crate::text::Word]| {
                text.clone().map(|mut t| {
                    t.content = crate::text::join_words(ws);
                    t
                })
            };
            let (lt, rt) = (with(&l), with(&r));
            (l, r, lt, rt)
        };
        let (caption, caption_off) = self
            .find_clip(clip)
            .map_or((None, false), |c| (c.1.caption.clone(), c.1.caption_off));
        let right = Clip {
            id: ClipId(self.alloc_id()),
            media,
            source_in: source_in + (at - start),
            start: at,
            duration: end - at,
            audio_muted,
            volume,
            transform,
            disabled,
            crossfade: Ticks::ZERO,
            keyframes: right_keys,
            text: right_text,
            words: right_words,
            caption,
            caption_off,
        };
        let mut steps = vec![
            Edit::SetClip {
                clip,
                track: track_id,
                start,
                duration: at - start,
                source_in,
            },
            Edit::InsertClip {
                track: track_id,
                clip: right,
            },
        ];
        if !keyframes.is_empty() {
            steps.push(Edit::SetClipKeyframes {
                clip,
                keyframes: left_keys,
            });
        }
        if !words.is_empty() {
            steps.push(Edit::SetClipWords {
                clip,
                words: left_words,
            });
            steps.push(Edit::SetClipText {
                clip,
                text: left_text,
            });
        }
        Ok(Edit::Batch(steps))
    }

    /// Remove `clip` and close the gap by shifting later clips on that track left.
    pub fn ripple_delete_edit(&self, clip: ClipId) -> Result<Edit, EditError> {
        let (track, c) = self.find_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
        let (removed_end, gap) = (c.end(), c.duration);
        let mut edits = vec![Edit::RemoveClip { clip }];
        edits.extend(
            track
                .clips
                .iter()
                .filter(|other| other.id != clip && other.start >= removed_end)
                .map(|other| Edit::SetClip {
                    clip: other.id,
                    track: track.id,
                    start: other.start - gap,
                    duration: other.duration,
                    source_in: other.source_in,
                }),
        );
        Ok(Edit::Batch(edits))
    }

    /// Change a clip's in/out points on the timeline, keeping the underlying
    /// media frame under each edge fixed (moving the start trims the source in-point).
    pub fn trim_edit(
        &self,
        clip: ClipId,
        new_start: Ticks,
        new_end: Ticks,
    ) -> Result<Edit, EditError> {
        let (track, c) = self.find_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
        if new_end <= new_start {
            return Err(EditError::InvalidRange("end must be after start"));
        }
        let set = Edit::SetClip {
            clip,
            track: track.id,
            start: new_start,
            duration: new_end - new_start,
            source_in: c.source_in + (new_start - c.start),
        };
        if (c.keyframes.is_empty() && c.words.is_empty()) || new_start == c.start {
            return Ok(set);
        }
        // Trimming the head must not slide the animation (or the spoken
        // words) along the picture.
        let by = c.start - new_start;
        let mut steps = vec![set];
        if !c.keyframes.is_empty() {
            steps.push(Edit::SetClipKeyframes {
                clip,
                keyframes: crate::keyframes::shifted(&c.keyframes, by),
            });
        }
        if !c.words.is_empty() {
            steps.push(Edit::SetClipWords {
                clip,
                words: c
                    .words
                    .iter()
                    .map(|w| crate::text::Word {
                        text: w.text.clone(),
                        start: w.start + by,
                        end: w.end + by,
                    })
                    .collect(),
            });
        }
        Ok(Edit::Batch(steps))
    }

    /// Move a clip to `start` on `track` (possibly a different track).
    pub fn move_edit(&self, clip: ClipId, track: TrackId, start: Ticks) -> Result<Edit, EditError> {
        let (_, c) = self.find_clip(clip).ok_or(EditError::ClipNotFound(clip))?;
        Ok(Edit::SetClip {
            clip,
            track,
            start,
            duration: c.duration,
            source_in: c.source_in,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::MediaId;
    use crate::model::TrackKind;

    fn t(n: i64) -> Ticks {
        Ticks(n)
    }

    #[test]
    fn tracks_move_and_rename_and_undo() {
        let mut tl = Timeline::new();
        let mut ids = Vec::new();
        for name in ["A", "B", "C"] {
            let track = tl.new_track(TrackKind::Video, name);
            ids.push(track.id);
            let n = tl.tracks.len();
            apply(&mut tl, Edit::InsertTrack { index: n, track }).unwrap();
        }
        let names = |tl: &Timeline| tl.tracks.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
        let back = apply(
            &mut tl,
            Edit::MoveTrack {
                track: ids[0],
                index: 2,
            },
        )
        .unwrap();
        assert_eq!(names(&tl), ["B", "C", "A"]);
        apply(&mut tl, back).unwrap();
        assert_eq!(names(&tl), ["A", "B", "C"]);
        assert!(apply(
            &mut tl,
            Edit::MoveTrack {
                track: ids[0],
                index: 3
            }
        )
        .is_err());
        let back = apply(
            &mut tl,
            Edit::RenameTrack {
                track: ids[1],
                name: "Music".into(),
            },
        )
        .unwrap();
        assert_eq!(names(&tl), ["A", "Music", "C"]);
        apply(&mut tl, back).unwrap();
        assert_eq!(names(&tl), ["A", "B", "C"]);
    }

    /// One video track holding clips at [0,10), [10,20), [30,40).
    fn fixture() -> (Timeline, TrackId, [ClipId; 3]) {
        let mut tl = Timeline::new();
        let track = tl.new_track(TrackKind::Video, "V1");
        let track_id = track.id;
        apply(&mut tl, Edit::InsertTrack { index: 0, track }).unwrap();
        let mut ids = Vec::new();
        for (start, dur) in [(0, 10), (10, 10), (30, 10)] {
            let clip = tl.new_clip(MediaId(1), t(100), t(start), t(dur));
            ids.push(clip.id);
            apply(
                &mut tl,
                Edit::InsertClip {
                    track: track_id,
                    clip,
                },
            )
            .unwrap();
        }
        (tl, track_id, [ids[0], ids[1], ids[2]])
    }

    fn caption_fixture() -> (Timeline, ClipId) {
        use crate::text::{TextSpec, Word};
        let mut tl = Timeline::new();
        let track = tl.new_track(TrackKind::Video, "Captions");
        let track_id = track.id;
        apply(&mut tl, Edit::InsertTrack { index: 0, track }).unwrap();
        let mut clip = tl.new_clip(MediaId(0), t(0), t(0), t(40));
        let words = vec![
            Word {
                text: "one".into(),
                start: t(0),
                end: t(10),
            },
            Word {
                text: " two".into(),
                start: t(10),
                end: t(20),
            },
            Word {
                text: " three".into(),
                start: t(20),
                end: t(30),
            },
            Word {
                text: " four".into(),
                start: t(30),
                end: t(40),
            },
        ];
        clip.text = Some(TextSpec {
            content: crate::text::join_words(&words),
            highlight: [255, 200, 0, 255],
            ..TextSpec::default()
        });
        clip.words = words;
        let id = clip.id;
        apply(
            &mut tl,
            Edit::InsertClip {
                track: track_id,
                clip,
            },
        )
        .unwrap();
        (tl, id)
    }

    #[test]
    fn splitting_a_caption_shares_out_its_words() {
        let (mut tl, id) = caption_fixture();
        let e = tl.split_edit(id, t(20)).unwrap();
        apply(&mut tl, e).unwrap();
        let (_, left) = tl.find_clip(id).unwrap();
        assert_eq!(left.text.as_ref().unwrap().content, "one two");
        assert_eq!(left.words.len(), 2);
        let right = tl.tracks[0].clips.iter().find(|c| c.id != id).unwrap();
        assert_eq!(right.text.as_ref().unwrap().content, "three four");
        assert_eq!(right.words[0].start, t(0));
        assert_eq!(right.words[1].end, t(20));
    }

    #[test]
    fn the_word_being_said_is_the_one_lit() {
        let (tl, id) = caption_fixture();
        let (_, c) = tl.find_clip(id).unwrap();
        let spec = c.text_at(t(12)).unwrap();
        let (a, b) = spec.active.unwrap();
        assert_eq!(&spec.content[a..b], "two");
        let mut k = c.clone();
        k.text.as_mut().unwrap().highlight_mode = crate::text::HighlightMode::Karaoke;
        let spec = k.text_at(t(22)).unwrap();
        let (a, b) = spec.active.unwrap();
        assert_eq!(&spec.content[a..b], "one two three");
    }

    #[test]
    fn trimming_a_captions_head_keeps_words_with_the_speech() {
        let (mut tl, id) = caption_fixture();
        let (_, c) = tl.find_clip(id).unwrap();
        let before = c.words[1].start;
        let e = tl.trim_edit(id, t(5), c.end()).unwrap();
        apply(&mut tl, e).unwrap();
        let (_, c) = tl.find_clip(id).unwrap();
        assert_eq!(c.words[1].start, before - t(5));
    }

    #[test]
    fn insert_rejects_overlap() {
        let (mut tl, track, _) = fixture();
        let clip = tl.new_clip(MediaId(1), t(0), t(5), t(10));
        let err = apply(&mut tl, Edit::InsertClip { track, clip }).unwrap_err();
        assert!(matches!(err, EditError::Overlap { .. }));
    }

    #[test]
    fn insert_rejects_zero_duration_and_negative_start() {
        let (mut tl, track, _) = fixture();
        let zero = tl.new_clip(MediaId(1), t(0), t(50), t(0));
        assert!(matches!(
            apply(&mut tl, Edit::InsertClip { track, clip: zero }),
            Err(EditError::InvalidRange(_))
        ));
        let negative = tl.new_clip(MediaId(1), t(0), t(-5), t(3));
        assert!(matches!(
            apply(
                &mut tl,
                Edit::InsertClip {
                    track,
                    clip: negative
                }
            ),
            Err(EditError::InvalidRange(_))
        ));
    }

    #[test]
    fn clips_stay_sorted() {
        let (mut tl, track, _) = fixture();
        let clip = tl.new_clip(MediaId(1), t(0), t(20), t(10));
        apply(&mut tl, Edit::InsertClip { track, clip }).unwrap();
        let starts: Vec<_> = tl
            .track(track)
            .unwrap()
            .clips
            .iter()
            .map(|c| c.start.0)
            .collect();
        assert_eq!(starts, vec![0, 10, 20, 30]);
    }

    #[test]
    fn split_adjusts_source_in_and_undoes() {
        let (mut tl, track, [first, ..]) = fixture();
        let before = tl.tracks.clone();
        let split = tl.split_edit(first, t(4)).unwrap();
        let inverse = apply(&mut tl, split).unwrap();

        let clips = &tl.track(track).unwrap().clips;
        assert_eq!(clips.len(), 4);
        assert_eq!((clips[0].start.0, clips[0].duration.0), (0, 4));
        assert_eq!((clips[1].start.0, clips[1].duration.0), (4, 6));
        assert_eq!(
            clips[1].source_in.0, 104,
            "right half starts 4 ticks into the source"
        );

        apply(&mut tl, inverse).unwrap();
        assert_eq!(tl.tracks, before);
    }

    #[test]
    fn split_outside_clip_is_an_error() {
        let (mut tl, _, [first, ..]) = fixture();
        assert_eq!(tl.split_edit(first, t(0)), Err(EditError::SplitOutOfRange));
        assert_eq!(tl.split_edit(first, t(10)), Err(EditError::SplitOutOfRange));
    }

    #[test]
    fn ripple_delete_closes_gap_and_undoes() {
        let (mut tl, track, [_, second, _]) = fixture();
        let before = tl.tracks.clone();
        let edit = tl.ripple_delete_edit(second).unwrap();
        let inverse = apply(&mut tl, edit).unwrap();

        let starts: Vec<_> = tl
            .track(track)
            .unwrap()
            .clips
            .iter()
            .map(|c| c.start.0)
            .collect();
        assert_eq!(starts, vec![0, 20], "third clip shifts left by 10");

        apply(&mut tl, inverse).unwrap();
        assert_eq!(tl.tracks, before);
    }

    #[test]
    fn trim_start_moves_source_in() {
        let (mut tl, track, [first, ..]) = fixture();
        let trim = tl.trim_edit(first, t(3), t(10)).unwrap();
        apply(&mut tl, trim).unwrap();
        let c = &tl.track(track).unwrap().clips[0];
        assert_eq!((c.start.0, c.duration.0, c.source_in.0), (3, 7, 103));
    }

    #[test]
    fn failed_set_clip_leaves_timeline_untouched() {
        let (mut tl, track, [first, ..]) = fixture();
        let before = tl.tracks.clone();
        let edit = tl.move_edit(first, track, t(12)).unwrap(); // lands on second clip
        assert!(matches!(
            apply(&mut tl, edit),
            Err(EditError::Overlap { .. })
        ));
        assert_eq!(tl.tracks, before);
    }

    #[test]
    fn failed_batch_rolls_back() {
        let (mut tl, track, [first, second, _]) = fixture();
        let before = tl.tracks.clone();
        let batch = Edit::Batch(vec![
            Edit::RemoveClip { clip: first },
            Edit::SetClip {
                clip: second,
                track,
                start: t(0),
                duration: t(10),
                source_in: t(100),
            },
            Edit::RemoveClip { clip: ClipId(9999) }, // fails
        ]);
        assert_eq!(
            apply(&mut tl, batch),
            Err(EditError::ClipNotFound(ClipId(9999)))
        );
        assert_eq!(tl.tracks, before);
    }

    #[test]
    fn locked_tracks_reject_clip_edits_but_can_be_unlocked() {
        let (mut tl, track, [first, ..]) = fixture();
        apply(
            &mut tl,
            Edit::SetTrackFlags {
                track,
                muted: false,
                locked: true,
            },
        )
        .unwrap();
        assert_eq!(
            apply(&mut tl, Edit::RemoveClip { clip: first }),
            Err(EditError::TrackLocked(track))
        );
        apply(
            &mut tl,
            Edit::SetTrackFlags {
                track,
                muted: false,
                locked: false,
            },
        )
        .unwrap();
        assert!(apply(&mut tl, Edit::RemoveClip { clip: first }).is_ok());
    }

    #[test]
    fn move_between_tracks() {
        let (mut tl, _, [first, ..]) = fixture();
        let v2 = tl.new_track(TrackKind::Video, "V2");
        let v2_id = v2.id;
        apply(
            &mut tl,
            Edit::InsertTrack {
                index: 1,
                track: v2,
            },
        )
        .unwrap();
        let edit = tl.move_edit(first, v2_id, t(5)).unwrap();
        apply(&mut tl, edit).unwrap();
        let (track, clip) = tl.find_clip(first).unwrap();
        assert_eq!((track.id, clip.start.0), (v2_id, 5));
    }

    #[test]
    fn edits_roundtrip_through_json() {
        let (mut tl, _, [first, ..]) = fixture();
        let edit = tl.split_edit(first, t(4)).unwrap();
        let json = serde_json::to_string(&edit).unwrap();
        let back: Edit = serde_json::from_str(&json).unwrap();
        assert_eq!(edit, back);
    }

    #[test]
    fn a_clips_sound_can_be_switched_off_and_the_inverse_switches_it_back() {
        let (mut tl, _, ids) = fixture();
        let clip = ids[0];
        let inverse = apply(&mut tl, Edit::SetClipAudioMuted { clip, muted: true }).unwrap();
        assert!(tl.find_clip(clip).unwrap().1.audio_muted);
        apply(&mut tl, inverse).unwrap();
        assert!(!tl.find_clip(clip).unwrap().1.audio_muted);
        assert!(apply(
            &mut tl,
            Edit::SetClipAudioMuted {
                clip: ClipId(999),
                muted: true
            }
        )
        .is_err());
    }

    #[test]
    fn splitting_keeps_the_sound_setting_on_both_halves() {
        let (mut tl, _, ids) = fixture();
        let clip = ids[0];
        apply(&mut tl, Edit::SetClipAudioMuted { clip, muted: true }).unwrap();
        let edit = tl.split_edit(clip, t(5)).unwrap();
        apply(&mut tl, edit).unwrap();
        let muted: Vec<bool> = tl.tracks[0]
            .clips
            .iter()
            .take(2)
            .map(|c| c.audio_muted)
            .collect();
        assert_eq!(muted, [true, true]);
    }

    #[test]
    fn old_projects_without_the_field_load_with_sound_on() {
        let json = r#"{"id":1,"media":2,"source_in":0,"start":0,"duration":254016000000}"#;
        let clip: Clip = serde_json::from_str(json).unwrap();
        assert!(!clip.audio_muted);
    }

    #[test]
    fn a_transform_edit_is_undoable_and_clamped() {
        let (mut tl, _, ids) = fixture();
        let clip = ids[0];
        let wild = Transform {
            x: 99_999,
            scale: 5,
            rotation: 3700,
            opacity: 2000,
            ..Transform::default()
        };
        let inverse = apply(
            &mut tl,
            Edit::SetClipTransform {
                clip,
                transform: wild,
            },
        )
        .unwrap();
        let got = tl.find_clip(clip).unwrap().1.transform;
        assert_eq!(got.x, 5000);
        assert_eq!(got.scale, Transform::SCALE_MIN);
        assert_eq!(got.rotation, 100, "370.0 degrees wraps to 10.0");
        assert_eq!(got.opacity, 1000);
        apply(&mut tl, inverse).unwrap();
        assert!(tl.find_clip(clip).unwrap().1.transform.is_default());
        assert!(apply(
            &mut tl,
            Edit::SetClipTransform {
                clip: ClipId(999),
                transform: Transform::default()
            }
        )
        .is_err());
    }

    #[test]
    fn rotation_wraps_to_half_a_turn_each_way() {
        let r = |rotation| {
            Transform {
                rotation,
                ..Transform::default()
            }
            .clamped()
            .rotation
        };
        assert_eq!(r(0), 0);
        assert_eq!(
            r(1800),
            -1800,
            "180 degrees is the same either way; the lower one is used"
        );
        assert_eq!(r(-1800), -1800);
        assert_eq!(r(900), 900);
        assert_eq!(r(3600), 0);
        assert_eq!(r(-3700), -100);
    }

    #[test]
    fn splitting_keeps_the_transform_on_both_halves() {
        let (mut tl, _, ids) = fixture();
        let clip = ids[0];
        let moved = Transform {
            x: 250,
            scale: 500,
            flip_h: true,
            ..Transform::default()
        };
        apply(
            &mut tl,
            Edit::SetClipTransform {
                clip,
                transform: moved,
            },
        )
        .unwrap();
        let edit = tl.split_edit(clip, t(5)).unwrap();
        apply(&mut tl, edit).unwrap();
        let both: Vec<Transform> = tl.tracks[0]
            .clips
            .iter()
            .take(2)
            .map(|c| c.transform)
            .collect();
        assert_eq!(both, [moved, moved]);
    }

    #[test]
    fn old_projects_without_a_transform_load_with_the_default() {
        let json = r#"{"id":1,"media":2,"source_in":0,"start":0,"duration":10}"#;
        let clip: Clip = serde_json::from_str(json).unwrap();
        assert!(clip.transform.is_default());
        let partial: Transform = serde_json::from_str(r#"{"x":100}"#).unwrap();
        assert_eq!(
            (partial.x, partial.scale, partial.opacity),
            (100, 1000, 1000)
        );
    }

    #[test]
    fn deactivating_a_clip_undoes_and_survives_a_split() {
        let (mut tl, clip, _) = {
            let mut tl = Timeline::new();
            let track = tl.new_track(TrackKind::Video, "V1");
            let tid = track.id;
            apply(&mut tl, Edit::InsertTrack { index: 0, track }).unwrap();
            let c = tl.new_clip(
                MediaId(1),
                Ticks::ZERO,
                Ticks::ZERO,
                Ticks::from_seconds_f64(4.0),
            );
            let id = c.id;
            apply(
                &mut tl,
                Edit::InsertClip {
                    track: tid,
                    clip: c,
                },
            )
            .unwrap();
            (tl, id, tid)
        };
        let inverse = apply(
            &mut tl,
            Edit::SetClipDisabled {
                clip,
                disabled: true,
            },
        )
        .unwrap();
        assert!(tl.find_clip(clip).unwrap().1.disabled);
        let split = tl.split_edit(clip, Ticks::from_seconds_f64(1.0)).unwrap();
        apply(&mut tl, split).unwrap();
        assert!(tl.tracks[0].clips.iter().all(|c| c.disabled));
        let _ = inverse;
    }
}
