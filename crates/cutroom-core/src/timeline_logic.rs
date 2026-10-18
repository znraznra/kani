//! Timeline editing interaction logic (hit testing, dragging, trimming,
//! dropping), independent of any GUI toolkit. Geometry is in the timeline
//! canvas's own pixel space.

use crate::geom::{Pos2, Rect};
use crate::state::Session;
use crate::view::{self, TimelineView};
use cutroom_timeline::{
    snap_points_with_extra, snap_span_with_extra, snap_with_extra, ClipId, MediaId, Ticks,
    Timeline, Track, TrackId, TrackKind, DEFAULT_VOLUME, MAX_VOLUME,
};

pub const HEADER_W: f32 = 158.0;
pub const RULER_H: f32 = 26.0;
pub const ROW_H: f32 = 46.0;
pub const ROW_GAP: f32 = 3.0;
pub const EDGE_PX: f32 = 7.0;
pub const SNAP_PX: f32 = 8.0;

/// What a plain click on a clip does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    /// Click selects; drag moves or trims.
    #[default]
    Select,
    /// Click a clip to cut it at that point.
    Split,
}

#[derive(Debug, Clone, Default)]
pub enum Drag {
    #[default]
    None,
    Scrub,
    Move {
        clip: ClipId,
        grab_offset: Ticks,
        orig_track: TrackId,
        orig_start: Ticks,
        duration: Ticks,
        track: TrackId,
        start: Ticks,
        valid: bool,
        /// Set while the pointer is over the ruler or below every track:
        /// the clip will get a track of its own.
        new_track: Option<NewTrackAt>,
        /// The other selected clips, moved by the same amount in time (they
        /// keep their tracks). Empty for a single-clip move.
        group: Vec<ClipId>,
    },
    /// A rectangle dragged over empty space: selects the clips it touches.
    Marquee {
        origin: Pos2,
        current: Pos2,
        /// Ctrl/Shift held: keep what was already selected.
        additive: bool,
        base: Vec<ClipId>,
    },
    /// The sound-level knob of a clip being dragged. Levels are thousandths.
    Volume {
        clip: ClipId,
        from: i32,
        to: i32,
    },
    /// A keyframe diamond being dragged along its clip. Times are measured
    /// from the start of the clip.
    Key {
        clip: ClipId,
        from: Ticks,
        to: Ticks,
    },
    TrimStart {
        clip: ClipId,
        orig_start: Ticks,
        end: Ticks,
        new_start: Ticks,
    },
    TrimEnd {
        clip: ClipId,
        start: Ticks,
        orig_end: Ticks,
        new_end: Ticks,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Body,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Ruler,
    Clip { id: ClipId, zone: Zone },
    Empty,
}

/// Which end of its group a brand-new track would be created at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewTrackAt {
    /// Dropped over the ruler, above every track.
    Top,
    /// Dropped in the empty space below every track.
    Bottom,
    /// Dropped on the line between two rows: a new track at this index
    /// (between row `i - 1` and row `i`).
    Between(usize),
}

/// How close (in pixels) to the line between two rows the pointer must be to
/// make a new track there.
pub const BETWEEN_PX: f32 = 6.0;

/// Where a new track of `kind` would be inserted for `at`, or `None` when that
/// place is not allowed (a video track among the audio tracks, an audio track
/// among the video tracks, anything above the Captions track).
pub fn new_track_insert_index(tl: &Timeline, kind: TrackKind, at: NewTrackAt) -> Option<usize> {
    match at {
        NewTrackAt::Top => Some(crate::state::new_track_index(tl, kind, true)),
        NewTrackAt::Bottom => Some(crate::state::new_track_index(tl, kind, false)),
        NewTrackAt::Between(i) => crate::state::track_slot_allowed(tl, kind, i).then_some(i),
    }
}

/// [`spot_at`], but a line between rows where a `kind` track may not go
/// counts as the row under the pointer (or nothing).
pub fn spot_for_kind(tl: &Timeline, canvas: Rect, y: f32, kind: TrackKind) -> Spot {
    match spot_at(canvas, y, tl.tracks.len()) {
        Spot::NewTrack(NewTrackAt::Between(i))
            if new_track_insert_index(tl, kind, NewTrackAt::Between(i)).is_none() =>
        {
            row_at(canvas, y, tl.tracks.len()).map_or(Spot::Nowhere, Spot::Row)
        }
        spot => spot,
    }
}

/// What lies under the pointer in the track area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spot {
    Row(usize),
    /// Over the ruler: a new track at the top.
    NewTrack(NewTrackAt),
    /// The gap between rows, or beside the timeline.
    Nowhere,
}

/// Where media dragged from the library would land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropTarget {
    pub track: TrackId,
    pub start: Ticks,
    pub duration: Ticks,
    pub valid: bool,
    /// The row is the wrong kind of track for this media (audio vs video).
    pub wrong_kind: bool,
    /// Set when the drop makes a new track (`track` is then meaningless).
    pub new_track: Option<NewTrackAt>,
    /// The kind of track the media ends up on (or would get as a new track).
    pub new_kind: TrackKind,
    /// Held over a clip: dropping replaces that clip, if the file is long enough
    /// (`Err` says why it cannot).
    pub replace: Option<(ClipId, Result<(), String>)>,
}

// ----- geometry helpers (pure) --------------------------------------------

/// Classify the pointer's vertical position: a track row, the ruler (new track
/// at the top), the empty space under the last row (new track at the bottom).
pub fn spot_at(canvas: Rect, y: f32, rows: usize) -> Spot {
    if y < canvas.top() + RULER_H {
        return Spot::NewTrack(NewTrackAt::Top);
    }
    for i in 1..rows {
        let line = row_top(canvas, i) - ROW_GAP / 2.0;
        if (y - line).abs() <= BETWEEN_PX {
            return Spot::NewTrack(NewTrackAt::Between(i));
        }
    }
    if let Some(row) = row_at(canvas, y, rows) {
        return Spot::Row(row);
    }
    if y >= row_top(canvas, rows) - ROW_GAP {
        return Spot::NewTrack(NewTrackAt::Bottom);
    }
    Spot::Nowhere
}

pub fn row_top(canvas: Rect, row: usize) -> f32 {
    canvas.top() + RULER_H + row as f32 * (ROW_H + ROW_GAP)
}

pub fn row_at(canvas: Rect, y: f32, rows: usize) -> Option<usize> {
    let rel = y - canvas.top() - RULER_H;
    if rel < 0.0 {
        return None;
    }
    let pitch = ROW_H + ROW_GAP;
    let row = (rel / pitch).floor() as usize;
    (rel.rem_euclid(pitch) <= ROW_H && row < rows).then_some(row)
}

pub fn hit_test(tl: &Timeline, view: &TimelineView, canvas: Rect, pos: Pos2) -> Hit {
    if pos.y < canvas.top() + RULER_H {
        return Hit::Ruler;
    }
    let Some(row) = row_at(canvas, pos.y, tl.tracks.len()) else {
        return Hit::Empty;
    };
    for clip in &tl.tracks[row].clips {
        let left = canvas.left() + view.x_for(clip.start);
        let right = canvas.left() + view.x_for(clip.end());
        if pos.x < left || pos.x > right {
            continue;
        }
        let edge = EDGE_PX.min((right - left) / 3.0);
        let zone = if pos.x - left <= edge {
            Zone::Left
        } else if right - pos.x <= edge {
            Zone::Right
        } else {
            Zone::Body
        };
        return Hit::Clip { id: clip.id, zone };
    }
    Hit::Empty
}

/// How far (in pixels) the pointer may be from a keyframe diamond to take it.
pub const KEY_REACH: f32 = 9.0;
/// Where the diamonds sit, down from the top of the row.
pub const KEY_Y: f32 = 12.0;

/// Where the sound-level bar sits, in pixels from the left edge of a clip.
pub const VOLUME_X: f32 = 13.0;
/// Clips narrower than this have no level bar.
pub const VOLUME_MIN_WIDTH: f32 = 56.0;

/// The vertical extent of a clip's level bar: (top, bottom) in canvas pixels.
pub fn volume_span(canvas: Rect, row: usize) -> (f32, f32) {
    let top = row_top(canvas, row) + 2.0;
    (top + 7.0, top + (ROW_H - 4.0) - 7.0)
}

/// The knob's y for a level (nothing at the bottom, double at the top, as
/// recorded in the middle).
pub fn volume_y(canvas: Rect, row: usize, level: i32) -> f32 {
    let (top, bottom) = volume_span(canvas, row);
    bottom - (level.clamp(0, MAX_VOLUME) as f32 / MAX_VOLUME as f32) * (bottom - top)
}

/// The level for a pointer height, snapping to "as recorded" when close.
pub fn volume_at(canvas: Rect, row: usize, y: f32) -> i32 {
    let (top, bottom) = volume_span(canvas, row);
    let frac = ((bottom - y) / (bottom - top)).clamp(0.0, 1.0);
    let level = (frac * MAX_VOLUME as f32).round() as i32;
    if (level - DEFAULT_VOLUME).abs() <= 80 {
        DEFAULT_VOLUME
    } else {
        level / 10 * 10
    }
}

/// The level bar under `pos`: its clip, row and current level.
pub fn volume_bar_at(
    session: &Session,
    view: &TimelineView,
    canvas: Rect,
    pos: Pos2,
) -> Option<(ClipId, usize, i32)> {
    let tl = &session.doc.timeline;
    let row = row_at(canvas, pos.y, tl.tracks.len())?;
    let track = &tl.tracks[row];
    let (top, bottom) = volume_span(canvas, row);
    if pos.y < top - 6.0 || pos.y > bottom + 6.0 {
        return None;
    }
    track.clips.iter().find_map(|c| {
        let left = canvas.left() + view.x_for(c.start);
        let right = canvas.left() + view.x_for(c.end());
        if right - left < VOLUME_MIN_WIDTH || !session.clip_has_sound(c, track.kind) {
            return None;
        }
        ((pos.x - (left + VOLUME_X)).abs() <= 7.0).then_some((c.id, row, c.volume))
    })
}

/// The keyframe diamond under `pos`: its clip and its time from the clip's start.
pub fn key_at(
    tl: &Timeline,
    view: &TimelineView,
    canvas: Rect,
    pos: Pos2,
) -> Option<(ClipId, Ticks)> {
    let row = row_at(canvas, pos.y, tl.tracks.len())?;
    // Diamonds sit along the top of the clip.
    let ky = row_top(canvas, row) + KEY_Y;
    if (pos.y - ky).abs() > KEY_REACH + 2.0 {
        return None;
    }
    let mut best: Option<(f32, ClipId, Ticks)> = None;
    for clip in &tl.tracks[row].clips {
        for k in &clip.keyframes {
            let kx = canvas.left() + view.x_for(clip.start + k.time);
            let d = (pos.x - kx).abs();
            if d <= KEY_REACH && best.map_or(true, |b| d < b.0) {
                best = Some((d, clip.id, k.time));
            }
        }
    }
    best.map(|b| (b.1, b.2))
}

/// Half the side of the little box drawn on a cut between touching clips.
pub const CUT_HANDLE: f32 = 9.0;

/// The clip after a cut whose handle (the crossfade button) is at `pos`.
pub fn cut_handle_at(
    tl: &Timeline,
    view: &TimelineView,
    canvas: Rect,
    pos: Pos2,
) -> Option<ClipId> {
    for (row, track) in tl.tracks.iter().enumerate() {
        let mid = row_top(canvas, row) + ROW_H / 2.0;
        if (pos.y - mid).abs() > CUT_HANDLE {
            continue;
        }
        for i in 0..track.clips.len() {
            if cutroom_timeline::transition::previous_touching(track, i).is_none() {
                continue;
            }
            let x = canvas.left() + view.x_for(track.clips[i].start);
            if (pos.x - x).abs() <= CUT_HANDLE {
                return Some(track.clips[i].id);
            }
        }
    }
    None
}

/// Every clip whose picture on the timeline overlaps `rect`.
pub fn clips_in_rect(tl: &Timeline, view: &TimelineView, canvas: Rect, rect: Rect) -> Vec<ClipId> {
    let mut hits = Vec::new();
    for (row, track) in tl.tracks.iter().enumerate() {
        let (top, bottom) = (row_top(canvas, row), row_top(canvas, row) + ROW_H);
        if bottom < rect.min.y || top > rect.max.y {
            continue;
        }
        for clip in &track.clips {
            let left = canvas.left() + view.x_for(clip.start);
            let right = canvas.left() + view.x_for(clip.end());
            if right >= rect.min.x && left <= rect.max.x {
                hits.push(clip.id);
            }
        }
    }
    hits
}

/// End of the clip before `clip` and start of the clip after it on its track.
pub fn neighbor_bounds(track: &Track, clip: ClipId) -> (Ticks, Option<Ticks>) {
    let Some(me) = track.clips.iter().find(|c| c.id == clip) else {
        return (Ticks::ZERO, None);
    };
    let prev_end = track
        .clips
        .iter()
        .filter(|c| c.id != clip && c.start < me.start)
        .map(|c| c.end())
        .max()
        .unwrap_or(Ticks::ZERO);
    let next_start = track
        .clips
        .iter()
        .filter(|c| c.id != clip && c.start > me.start)
        .map(|c| c.start)
        .min();
    (prev_end, next_start)
}

pub fn fits(track: &Track, clip: ClipId, start: Ticks, duration: Ticks) -> bool {
    !track.locked
        && !track
            .clips
            .iter()
            .any(|c| c.id != clip && c.start < start + duration && start < c.end())
}

/// Would moving the clip and its group by `delta` leave every one of them in
/// free space (ignoring each other) on an unlocked track?
fn group_fits(tl: &Timeline, clip: &ClipId, group: &[ClipId], delta: Ticks) -> bool {
    let members: Vec<ClipId> = std::iter::once(*clip)
        .chain(group.iter().copied())
        .collect();
    members.iter().all(|id| {
        let Some((track, c)) = tl.find_clip(*id) else {
            return false;
        };
        let (start, end) = (c.start + delta, c.end() + delta);
        start >= Ticks::ZERO
            && !track.locked
            && !track
                .clips
                .iter()
                .any(|o| !members.contains(&o.id) && o.start < end && start < o.end())
    })
}

fn clamp_ticks(value: Ticks, low: Ticks, high: Ticks) -> Ticks {
    if low > high {
        low
    } else {
        value.max(low).min(high)
    }
}

/// Where library media dragged to `pos` would land, or `None` if the pointer
/// is not over a track that can take it.
pub fn drop_target(
    session: &Session,
    view: &TimelineView,
    canvas: Rect,
    pos: Pos2,
    media: MediaId,
    snap_enabled: bool,
    threshold: Ticks,
) -> Option<DropTarget> {
    let tl = &session.doc.timeline;
    let rate = session.doc.settings.frame_rate;
    let duration = session.media_duration(media);
    let mut start = view::quantize_to_frame(view.time_for(pos.x - canvas.left()), rate);
    if snap_enabled {
        let extra = [session.playhead];
        if let Some(snapped) = snap_span_with_extra(tl, start, duration, threshold, None, &extra) {
            start = snapped;
        }
    }
    let media_kind = session.media_kind_of(media).track_kind();
    match spot_for_kind(tl, canvas, pos.y, media_kind) {
        Spot::Nowhere => None,
        // A new track is always possible.
        Spot::NewTrack(at) => Some(DropTarget {
            track: TrackId(u64::MAX),
            start,
            duration,
            valid: true,
            wrong_kind: false,
            new_track: Some(at),
            new_kind: session.media_kind_of(media).track_kind(),
            replace: None,
        }),
        Spot::Row(row) => {
            let track = &tl.tracks[row];
            // Video files may also go on audio tracks (sound only).
            let wrong_kind = !session.can_place_on(media, track.kind);
            let replace = match hit_test(tl, view, canvas, pos) {
                Hit::Clip { id, .. } if !wrong_kind && !track.locked => tl
                    .find_clip(id)
                    .filter(|(_, c)| c.text.is_none())
                    .map(|_| (id, session.can_replace(id, media))),
                _ => None,
            };
            Some(DropTarget {
                track: track.id,
                start,
                duration,
                valid: !wrong_kind && fits(track, ClipId(u64::MAX), start, duration),
                wrong_kind,
                new_track: None,
                new_kind: track.kind,
                replace,
            })
        }
    }
}

pub fn begin_drag(session: &mut Session, view: &TimelineView, canvas: Rect, pos: Pos2) -> Drag {
    if let Some((id, _, level)) = volume_bar_at(session, view, canvas, pos) {
        if session.is_selected(id) {
            session.selected = Some(id);
        } else {
            session.select_only(id);
        }
        return Drag::Volume {
            clip: id,
            from: level,
            to: level,
        };
    }
    let tl = &session.doc.timeline;
    if let Some((id, time)) = key_at(tl, view, canvas, pos) {
        if session.is_selected(id) {
            session.selected = Some(id);
        } else {
            session.select_only(id);
        }
        return Drag::Key {
            clip: id,
            from: time,
            to: time,
        };
    }
    let tl = &session.doc.timeline;
    match hit_test(tl, view, canvas, pos) {
        Hit::Ruler => {
            session.playing = false;
            Drag::Scrub
        }
        Hit::Empty => Drag::Marquee {
            origin: pos,
            current: pos,
            additive: false,
            base: Vec::new(),
        },
        Hit::Clip { id, zone } => {
            let Some((track, clip)) = tl.find_clip(id) else {
                return Drag::None;
            };
            let (track_id, clip) = (track.id, clip.clone());
            // Grabbing a clip that is part of the selection keeps the selection.
            if session.is_selected(id) {
                session.selected = Some(id);
            } else {
                session.select_only(id);
            }
            match zone {
                Zone::Left => Drag::TrimStart {
                    clip: id,
                    orig_start: clip.start,
                    end: clip.end(),
                    new_start: clip.start,
                },
                Zone::Right => Drag::TrimEnd {
                    clip: id,
                    start: clip.start,
                    orig_end: clip.end(),
                    new_end: clip.end(),
                },
                Zone::Body => Drag::Move {
                    clip: id,
                    grab_offset: view.time_for(pos.x - canvas.left()) - clip.start,
                    orig_track: track_id,
                    orig_start: clip.start,
                    duration: clip.duration,
                    track: track_id,
                    start: clip.start,
                    valid: true,
                    new_track: None,
                    group: {
                        let mut g = session.selection();
                        g.retain(|c| *c != id);
                        g
                    },
                },
            }
        }
    }
}

/// Update the in-flight drag. Returns where a snap guide line should be drawn.
#[allow(clippy::too_many_arguments)]
pub fn update_drag(
    session: &mut Session,
    view: &TimelineView,
    canvas: Rect,
    drag: &mut Drag,
    pos: Pos2,
    playhead_snap: Option<Ticks>,
    snap_enabled: bool,
    threshold: Ticks,
    frame_ticks: Ticks,
) -> Option<Ticks> {
    let rate = session.doc.settings.frame_rate;
    let pointer_time = view.time_for(pos.x - canvas.left());
    let extra: Vec<Ticks> = playhead_snap.into_iter().collect();
    let tl = &session.doc.timeline;

    match drag {
        Drag::None => None,
        Drag::Volume { clip, to, .. } => {
            if let Some(row) = tl
                .tracks
                .iter()
                .position(|t| t.clips.iter().any(|c| c.id == *clip))
            {
                *to = volume_at(canvas, row, pos.y);
            }
            None
        }
        Drag::Key { clip, to, .. } => {
            let (start, duration) = tl
                .find_clip(*clip)
                .map(|(_, c)| (c.start, c.duration))
                .unwrap_or((Ticks::ZERO, Ticks::ZERO));
            let mut t = (pointer_time - start).clamp(Ticks::ZERO, duration);
            let mut guide = None;
            if snap_enabled {
                // Snap to the playhead, then to whole frames.
                if let Some(p) = playhead_snap {
                    if (start + t - p).0.abs() <= threshold.0 {
                        t = (p - start).clamp(Ticks::ZERO, duration);
                        guide = Some(p);
                    }
                }
                if guide.is_none() && frame_ticks.0 > 0 {
                    t = Ticks((t.0 + frame_ticks.0 / 2) / frame_ticks.0 * frame_ticks.0)
                        .clamp(Ticks::ZERO, duration);
                }
            }
            *to = t;
            guide
        }
        Drag::Marquee {
            origin,
            current,
            additive,
            base,
        } => {
            *current = pos;
            let rect = Rect {
                min: Pos2 {
                    x: origin.x.min(current.x),
                    y: origin.y.min(current.y),
                },
                max: Pos2 {
                    x: origin.x.max(current.x),
                    y: origin.y.max(current.y),
                },
            };
            let mut ids = clips_in_rect(tl, view, canvas, rect);
            if *additive {
                for id in base.iter() {
                    if !ids.contains(id) {
                        ids.push(*id);
                    }
                }
            }
            session.select_many(&ids);
            None
        }
        Drag::Scrub => {
            let t = view::quantize_to_frame(pointer_time, rate);
            session.set_playhead(t);
            None
        }
        Drag::Move {
            clip,
            grab_offset,
            orig_track,
            duration,
            track,
            start,
            valid,
            new_track,
            orig_start,
            group,
            ..
        } => {
            let mut proposed =
                view::quantize_to_frame((pointer_time - *grab_offset).max(Ticks::ZERO), rate);
            let mut guide = None;
            if snap_enabled {
                if let Some(snapped) =
                    snap_span_with_extra(tl, proposed, *duration, threshold, Some(*clip), &extra)
                {
                    let points = snap_points_with_extra(tl, Some(*clip), &extra);
                    guide = if points.contains(&snapped) {
                        Some(snapped)
                    } else if points.contains(&(snapped + *duration)) {
                        Some(snapped + *duration)
                    } else {
                        None
                    };
                    proposed = snapped;
                }
            }
            if !group.is_empty() {
                // A group only slides in time. It may not start before zero
                // or land on a clip that isn't part of it.
                let earliest = group
                    .iter()
                    .filter_map(|id| tl.find_clip(*id).map(|(_, c)| c.start))
                    .chain(tl.find_clip(*clip).map(|(_, c)| c.start))
                    .min()
                    .unwrap_or(*orig_start);
                let lowest = *orig_start - earliest;
                *new_track = None;
                *track = *orig_track;
                *start = proposed.max(lowest);
                let delta = *start - *orig_start;
                *valid = group_fits(tl, clip, group, delta);
                return guide;
            }
            // Pick the track under the pointer. Same kind only, except that a
            // video clip with sound may go to an audio track as sound only.
            let source_kind = tl.track(*orig_track).map(|t| t.kind);
            let media = tl.find_clip(*clip).map(|(_, c)| c.media);
            *new_track = None;
            let kind = source_kind.unwrap_or(TrackKind::Video);
            match spot_for_kind(tl, canvas, pos.y, kind) {
                Spot::NewTrack(at) => *new_track = Some(at),
                Spot::Row(row) => {
                    let candidate = &tl.tracks[row];
                    let same_kind = Some(candidate.kind) == source_kind;
                    let as_audio = source_kind == Some(TrackKind::Video)
                        && candidate.kind == TrackKind::Audio
                        && media.is_some_and(|m| session.can_place_on(m, TrackKind::Audio));
                    if same_kind || as_audio {
                        *track = candidate.id;
                    }
                }
                Spot::Nowhere => {}
            }
            *start = proposed;
            *valid = new_track.is_some()
                || tl
                    .track(*track)
                    .is_some_and(|t| fits(t, *clip, *start, *duration));
            guide
        }
        Drag::TrimStart {
            clip,
            orig_start,
            end,
            new_start,
        } => {
            let (track, c) = tl.find_clip(*clip)?;
            let (prev_end, _) = neighbor_bounds(track, *clip);
            let by_source = *orig_start - c.source_in;
            let lower = prev_end.max(by_source).max(Ticks::ZERO);
            let upper = *end - frame_ticks;
            let mut t = view::quantize_to_frame(pointer_time, rate);
            let mut guide = None;
            if snap_enabled {
                if let Some(s) = snap_with_extra(tl, t, threshold, Some(*clip), &extra) {
                    t = s;
                    guide = Some(s);
                }
            }
            *new_start = clamp_ticks(t, lower, upper);
            guide.filter(|g| g == new_start)
        }
        Drag::TrimEnd {
            clip,
            start,
            orig_end,
            new_end,
        } => {
            let (track, c) = tl.find_clip(*clip)?;
            let (_, next_start) = neighbor_bounds(track, *clip);
            let lower = *start + frame_ticks;
            // Until the real duration is known (Phase 3) there is no end of
            // the source to stop at, so the clip may be extended freely.
            let mut upper = Ticks(i64::MAX / 4);
            if let Some(duration) = session.known_media_duration(c.media) {
                let source_end = c.start + (duration - c.source_in);
                upper = source_end.max(*orig_end); // never force a clip shorter than it is
            }
            if let Some(next) = next_start {
                upper = upper.min(next);
            }
            let mut t = view::quantize_to_frame(pointer_time, rate);
            let mut guide = None;
            if snap_enabled {
                if let Some(s) = snap_with_extra(tl, t, threshold, Some(*clip), &extra) {
                    t = s;
                    guide = Some(s);
                }
            }
            *new_end = clamp_ticks(t, lower, upper);
            guide.filter(|g| g == new_end)
        }
    }
}

pub fn commit_drag(session: &mut Session, drag: &Drag) {
    match drag {
        Drag::Marquee { .. } => {}
        Drag::Key { clip, from, to } => {
            session.move_keyframes(*clip, *from, *to);
        }
        Drag::Volume { clip, from, to } => {
            if from != to {
                session.set_clip_volume(*clip, *to);
            }
        }
        Drag::Move {
            clip,
            orig_track,
            orig_start,
            track,
            start,
            valid,
            new_track,
            group,
            ..
        } => {
            if new_track.is_none() && (track, start) == (orig_track, orig_start) {
                return;
            }
            if !group.is_empty() {
                if *valid {
                    session.move_clips_by(clip, group, *start - *orig_start);
                } else {
                    session.status = Some("Clips can't overlap, or a track is locked".into());
                }
                return;
            }
            if let Some(at) = new_track {
                // The new track has the kind of the clip's own track.
                let kind = session
                    .doc
                    .timeline
                    .track(*orig_track)
                    .map(|t| t.kind)
                    .unwrap_or(TrackKind::Video);
                match new_track_insert_index(&session.doc.timeline, kind, *at) {
                    Some(index) => {
                        session.move_clip_to_new_track_at(*clip, kind, index, *start);
                    }
                    None => session.status = Some("A new track can't go there.".into()),
                }
            } else if *valid {
                session.move_clip(*clip, *track, *start);
            } else {
                session.status = Some("Clips can't overlap, or the track is locked".into());
            }
        }
        Drag::TrimStart {
            clip,
            orig_start,
            end,
            new_start,
        } => {
            if new_start != orig_start {
                session.trim_clip(*clip, *new_start, *end);
            }
        }
        Drag::TrimEnd {
            clip,
            start,
            orig_end,
            new_end,
        } => {
            if new_end != orig_end {
                session.trim_clip(*clip, *start, *new_end);
            }
        }
        Drag::None | Drag::Scrub => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{pos2, vec2};
    use cutroom_timeline::{apply, Edit, MediaId, TrackKind};

    /// One video track with a clip from 2 s to 4 s.
    fn fixture() -> (Timeline, ClipId, TrackId) {
        let mut tl = Timeline::new();
        let track = tl.new_track(TrackKind::Video, "V1");
        let track_id = track.id;
        apply(&mut tl, Edit::InsertTrack { index: 0, track }).unwrap();
        let clip = tl.new_clip(
            MediaId(1),
            Ticks::ZERO,
            Ticks::from_seconds_f64(2.0),
            Ticks::from_seconds_f64(2.0),
        );
        let id = clip.id;
        apply(
            &mut tl,
            Edit::InsertClip {
                track: track_id,
                clip,
            },
        )
        .unwrap();
        (tl, id, track_id)
    }

    fn canvas() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 300.0))
    }

    fn view() -> TimelineView {
        TimelineView {
            pixels_per_second: 100.0,
            scroll_seconds: 0.0,
        }
    }

    #[test]
    fn ruler_clicks_are_ruler_hits() {
        let (tl, _, _) = fixture();
        assert_eq!(
            hit_test(&tl, &view(), canvas(), pos2(300.0, 5.0)),
            Hit::Ruler
        );
    }

    #[test]
    fn clip_zones_body_and_edges() {
        let (tl, id, _) = fixture();
        let y = RULER_H + ROW_H / 2.0;
        // The clip spans x = 200..400.
        assert_eq!(
            hit_test(&tl, &view(), canvas(), pos2(300.0, y)),
            Hit::Clip {
                id,
                zone: Zone::Body
            }
        );
        assert_eq!(
            hit_test(&tl, &view(), canvas(), pos2(203.0, y)),
            Hit::Clip {
                id,
                zone: Zone::Left
            }
        );
        assert_eq!(
            hit_test(&tl, &view(), canvas(), pos2(397.0, y)),
            Hit::Clip {
                id,
                zone: Zone::Right
            }
        );
    }

    #[test]
    fn empty_space_and_row_gaps_are_empty() {
        let (tl, _, _) = fixture();
        let y = RULER_H + ROW_H / 2.0;
        assert_eq!(hit_test(&tl, &view(), canvas(), pos2(600.0, y)), Hit::Empty);
        let gap_y = RULER_H + ROW_H + ROW_GAP / 2.0 + 0.1;
        assert_eq!(
            hit_test(&tl, &view(), canvas(), pos2(300.0, gap_y)),
            Hit::Empty
        );
        assert_eq!(
            hit_test(&tl, &view(), canvas(), pos2(300.0, 250.0)),
            Hit::Empty
        );
    }

    #[test]
    fn scrolling_shifts_hit_areas() {
        let (tl, id, _) = fixture();
        let scrolled = TimelineView {
            pixels_per_second: 100.0,
            scroll_seconds: 1.0,
        };
        let y = RULER_H + ROW_H / 2.0;
        // The clip now spans x = 100..300.
        assert_eq!(
            hit_test(&tl, &scrolled, canvas(), pos2(200.0, y)),
            Hit::Clip {
                id,
                zone: Zone::Body
            }
        );
    }

    #[test]
    fn narrow_clips_still_have_a_body() {
        let (mut tl, _, track_id) = fixture();
        let tiny = tl.new_clip(
            MediaId(1),
            Ticks::ZERO,
            Ticks::from_seconds_f64(6.0),
            Ticks::from_seconds_f64(0.06), // 6 px wide
        );
        let tiny_id = tiny.id;
        apply(
            &mut tl,
            Edit::InsertClip {
                track: track_id,
                clip: tiny,
            },
        )
        .unwrap();
        let y = RULER_H + ROW_H / 2.0;
        assert_eq!(
            hit_test(&tl, &view(), canvas(), pos2(603.0, y)),
            Hit::Clip {
                id: tiny_id,
                zone: Zone::Body
            }
        );
    }

    #[test]
    fn neighbor_bounds_and_fit_checks() {
        let (mut tl, id, track_id) = fixture();
        let other = tl.new_clip(
            MediaId(1),
            Ticks::ZERO,
            Ticks::from_seconds_f64(5.0),
            Ticks::from_seconds_f64(1.0),
        );
        apply(
            &mut tl,
            Edit::InsertClip {
                track: track_id,
                clip: other,
            },
        )
        .unwrap();
        let track = tl.track(track_id).unwrap();
        let (prev, next) = neighbor_bounds(track, id);
        assert_eq!(prev, Ticks::ZERO);
        assert_eq!(next, Some(Ticks::from_seconds_f64(5.0)));

        let two = Ticks::from_seconds_f64(2.0);
        assert!(fits(track, id, Ticks::from_seconds_f64(2.5), two));
        assert!(!fits(track, id, Ticks::from_seconds_f64(3.5), two));
        assert!(fits(track, id, Ticks::ZERO, two));
    }

    #[test]
    fn clamp_never_panics_on_inverted_bounds() {
        assert_eq!(clamp_ticks(Ticks(5), Ticks(10), Ticks(3)), Ticks(10));
        assert_eq!(clamp_ticks(Ticks(5), Ticks(1), Ticks(9)), Ticks(5));
        assert_eq!(clamp_ticks(Ticks(50), Ticks(1), Ticks(9)), Ticks(9));
    }

    // ----- interaction logic, driven without a window ----------------------

    use cutroom_project::{ImportMode, ProjectSettings};
    use cutroom_timeline::FrameRate;
    use tempfile::{tempdir, TempDir};

    fn secs(s: f64) -> Ticks {
        Ticks::from_seconds_f64(s)
    }

    /// Tracks (top to bottom): Video 2, Video 1, Audio 1.
    /// "Video 1" holds clip A at 0..5 s and clip B at 6..11 s.
    fn editing_session() -> (TempDir, Session, ClipId, ClipId) {
        let tmp = tempdir().unwrap();
        let mut session = Session::create(
            &tmp.path().join("proj"),
            "demo",
            ProjectSettings::vertical(FrameRate::FPS_30),
        )
        .unwrap();
        // The tests below use an empty "Video 2" overlay on top as row 0.
        let overlay = session.doc.timeline.new_track(TrackKind::Video, "Video 2");
        cutroom_timeline::apply(
            &mut session.doc.timeline,
            cutroom_timeline::Edit::InsertTrack {
                index: 0,
                track: overlay,
            },
        )
        .unwrap();
        let file = tmp.path().join("clip.mp4");
        std::fs::write(&file, b"x").unwrap();
        session.import_mode = ImportMode::Reference;
        let media = session.import_files(&[file])[0];
        let a = session.add_media_to_timeline(media).unwrap();
        session.set_playhead(secs(6.0));
        let b = session.add_media_to_timeline(media).unwrap();
        session.set_playhead(Ticks::ZERO);
        (tmp, session, a, b)
    }

    fn y_of_row(row: usize) -> f32 {
        RULER_H + row as f32 * (ROW_H + ROW_GAP) + ROW_H / 2.0
    }

    fn frame_ticks() -> Ticks {
        Ticks(FrameRate::FPS_30.ticks_per_frame().unwrap())
    }

    fn drag_update(
        session: &mut Session,
        drag: &mut Drag,
        x: f32,
        y: f32,
        snap: bool,
    ) -> Option<Ticks> {
        let playhead = session.playhead;
        update_drag(
            session,
            &view(),
            canvas(),
            drag,
            pos2(x, y),
            snap.then_some(playhead),
            snap,
            view().ticks_for_pixels(SNAP_PX),
            frame_ticks(),
        )
    }

    fn clip_start(session: &Session, id: ClipId) -> f64 {
        session
            .doc
            .timeline
            .find_clip(id)
            .unwrap()
            .1
            .start
            .as_seconds_f64()
    }

    #[test]
    fn dragging_a_clip_moves_it_on_release() {
        let (_tmp, mut session, a, _) = editing_session();
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(250.0, y_of_row(1)));
        assert!(matches!(drag, Drag::Move { .. }));
        assert_eq!(session.selected, Some(a));
        drag_update(&mut session, &mut drag, 250.0 + 40.0, y_of_row(1), false);
        commit_drag(&mut session, &drag);
        assert!(
            (clip_start(&session, a) - 0.4).abs() < 0.04,
            "{}",
            clip_start(&session, a)
        );
        assert!(session.history.can_undo());
    }

    #[test]
    fn overlapping_drop_is_refused_and_nothing_changes() {
        let (_tmp, mut session, a, _) = editing_session();
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(250.0, y_of_row(1)));
        drag_update(&mut session, &mut drag, 250.0 + 120.0, y_of_row(1), false);
        match &drag {
            Drag::Move { valid, .. } => assert!(!valid),
            other => panic!("expected a move, got {other:?}"),
        }
        commit_drag(&mut session, &drag);
        assert_eq!(clip_start(&session, a), 0.0);
        assert!(session.status.as_deref().unwrap().contains("overlap"));
    }

    #[test]
    fn dragging_snaps_the_clip_end_to_the_next_clip() {
        let (_tmp, mut session, _, _) = editing_session();
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(250.0, y_of_row(1)));
        // Proposed start ~1.03 s, so the end is ~6.03 s, 3 ms off clip B's start at 6.0 s.
        let guide = drag_update(&mut session, &mut drag, 250.0 + 103.0, y_of_row(1), true);
        assert_eq!(guide, Some(secs(6.0)));
        match drag {
            Drag::Move { start, valid, .. } => {
                assert_eq!(start, secs(1.0));
                assert!(valid);
            }
            other => panic!("expected a move, got {other:?}"),
        }
    }

    #[test]
    fn a_keyframe_diamond_can_be_dragged_along_its_clip() {
        let (_tmp, mut session, a, _) = editing_session();
        // Clip `a` starts at 0 on row 1; key X at 1 s and 2 s.
        session.set_playhead(secs(1.0));
        session.toggle_keyframes(a, &[cutroom_timeline::Prop::X]);
        session.set_playhead(secs(2.0));
        session.toggle_keyframes(a, &[cutroom_timeline::Prop::X]);
        let diamond_y = RULER_H + (ROW_H + ROW_GAP) + KEY_Y;
        // Grabbing the diamond at 1 s (x = 100 px) takes the key, not the clip body.
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(100.0, diamond_y));
        assert!(
            matches!(drag, Drag::Key { from, .. } if from == secs(1.0)),
            "{drag:?}"
        );
        // Drag to 1.5 s with snapping off.
        drag_update(&mut session, &mut drag, 150.0, diamond_y, false);
        assert!(
            matches!(drag, Drag::Key { to, .. } if to == secs(1.5)),
            "{drag:?}"
        );
        commit_drag(&mut session, &drag);
        let times: Vec<_> = session
            .doc
            .timeline
            .find_clip(a)
            .unwrap()
            .1
            .keyframes
            .iter()
            .map(|k| k.time)
            .collect();
        assert_eq!(times, vec![secs(1.5), secs(2.0)]);
        // Lower down the clip, the same x is the body: a normal move.
        let drag = begin_drag(
            &mut session,
            &view(),
            canvas(),
            pos2(150.0, y_of_row(1) + 12.0),
        );
        assert!(matches!(drag, Drag::Move { .. }));
        // Dragging one key onto another replaces it, and past the ends stays inside the clip.
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(150.0, diamond_y));
        drag_update(&mut session, &mut drag, 200.0, diamond_y, false);
        commit_drag(&mut session, &drag);
        assert_eq!(
            session.doc.timeline.find_clip(a).unwrap().1.keyframes.len(),
            1
        );
        session.undo();
        assert_eq!(
            session.doc.timeline.find_clip(a).unwrap().1.keyframes.len(),
            2
        );
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(150.0, diamond_y));
        drag_update(&mut session, &mut drag, -500.0, diamond_y, false);
        assert!(matches!(drag, Drag::Key { to, .. } if to == Ticks::ZERO));
    }

    #[test]
    fn the_level_bar_sets_a_clips_sound_level() {
        let (_tmp, mut session, a, _) = editing_session();
        let media = session.doc.timeline.find_clip(a).unwrap().1.media;
        session.apply_probe(
            media,
            None,
            cutroom_project::MediaProbe {
                has_video: true,
                has_audio: true,
                ..Default::default()
            },
        );
        assert_eq!(session.doc.timeline.find_clip(a).unwrap().1.volume, 1000);
        let row = 1;
        let bar_x = VOLUME_X; // clip `a` starts at x = 0
        let mid = volume_y(canvas(), row, 1000);
        let (top, bottom) = volume_span(canvas(), row);
        assert!(mid > top && mid < bottom, "the knob starts in the middle");
        assert!(((top + bottom) / 2.0 - mid).abs() < 0.5);
        // Dragging the knob up doubles it at most, and down silences it.
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(bar_x, mid));
        assert!(matches!(drag, Drag::Volume { from: 1000, .. }), "{drag:?}");
        drag_update(&mut session, &mut drag, bar_x, top - 50.0, false);
        assert!(matches!(drag, Drag::Volume { to: 2000, .. }), "{drag:?}");
        commit_drag(&mut session, &drag);
        assert_eq!(session.doc.timeline.find_clip(a).unwrap().1.volume, 2000);
        session.undo();
        assert_eq!(session.doc.timeline.find_clip(a).unwrap().1.volume, 1000);
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(bar_x, mid));
        drag_update(&mut session, &mut drag, bar_x, bottom + 50.0, false);
        assert!(matches!(drag, Drag::Volume { to: 0, .. }));
        // Close to the middle snaps back to "as recorded".
        drag_update(&mut session, &mut drag, bar_x, mid + 0.8, false);
        assert!(matches!(drag, Drag::Volume { to: 1000, .. }));
        // Away from the bar, the same height is just the clip.
        let drag = begin_drag(&mut session, &view(), canvas(), pos2(bar_x + 60.0, mid));
        assert!(matches!(drag, Drag::Move { .. }));
    }

    #[test]
    fn snapping_can_be_turned_off() {
        let (_tmp, mut session, _, _) = editing_session();
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(250.0, y_of_row(1)));
        let guide = drag_update(&mut session, &mut drag, 250.0 + 103.0, y_of_row(1), false);
        assert_eq!(guide, None);
        match drag {
            Drag::Move { start, .. } => assert_ne!(start, secs(1.0)),
            other => panic!("expected a move, got {other:?}"),
        }
    }

    #[test]
    fn clips_can_move_to_another_video_track_or_to_audio_as_sound_only() {
        let (_tmp, mut session, a, _) = editing_session();
        let video2 = session.doc.timeline.tracks[0].id;
        let audio1 = session.doc.timeline.tracks[2].id;

        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(250.0, y_of_row(1)));
        drag_update(&mut session, &mut drag, 250.0, y_of_row(2), false); // audio row
        match &drag {
            Drag::Move { track, .. } => {
                assert_eq!(
                    *track, audio1,
                    "a video clip with sound may go to an audio track"
                )
            }
            other => panic!("expected a move, got {other:?}"),
        }
        drag_update(&mut session, &mut drag, 250.0, y_of_row(0), false); // Video 2
        commit_drag(&mut session, &drag);
        // Same time, different track: the drag target differs, so it commits.
        let (track, _) = session.doc.timeline.find_clip(a).unwrap();
        assert_eq!(track.id, video2);
    }

    #[test]
    fn trimming_the_end_is_limited_by_the_source_length_once_it_is_known() {
        let (_tmp, mut session, _, b) = editing_session();
        let media = session.doc.timeline.find_clip(b).unwrap().1.media;
        session.doc.set_media_duration(media, secs(5.0)).unwrap();
        // Clip B's right edge is at x = 1100.
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(1097.0, y_of_row(1)));
        assert!(matches!(drag, Drag::TrimEnd { .. }));
        drag_update(&mut session, &mut drag, 1500.0, y_of_row(1), false);
        match &drag {
            Drag::TrimEnd { new_end, .. } => {
                assert_eq!(*new_end, secs(11.0), "cannot exceed the 5 s source")
            }
            other => panic!("{other:?}"),
        }
        drag_update(&mut session, &mut drag, 800.0, y_of_row(1), false);
        commit_drag(&mut session, &drag);
        let clip = session.doc.timeline.find_clip(b).unwrap().1;
        assert!((clip.duration.as_seconds_f64() - 2.0).abs() < 0.04);
    }

    #[test]
    fn trimming_the_start_moves_the_source_in_point() {
        let (_tmp, mut session, _, b) = editing_session();
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(603.0, y_of_row(1)));
        assert!(matches!(drag, Drag::TrimStart { .. }));
        drag_update(&mut session, &mut drag, 700.0, y_of_row(1), false);
        commit_drag(&mut session, &drag);
        let clip = session.doc.timeline.find_clip(b).unwrap().1;
        assert!((clip.start.as_seconds_f64() - 7.0).abs() < 0.04);
        assert!((clip.source_in.as_seconds_f64() - 1.0).abs() < 0.04);
        assert_eq!(clip.end(), secs(11.0), "the out point stays put");
    }

    #[test]
    fn the_start_cannot_be_dragged_before_the_start_of_the_source() {
        let (_tmp, mut session, _, _) = editing_session();
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(603.0, y_of_row(1)));
        drag_update(&mut session, &mut drag, 520.0, y_of_row(1), false);
        match drag {
            Drag::TrimStart {
                new_start,
                orig_start,
                ..
            } => assert_eq!(new_start, orig_start),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn scrubbing_the_ruler_moves_the_playhead_to_whole_frames() {
        let (_tmp, mut session, _, _) = editing_session();
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(300.0, 5.0));
        assert!(matches!(drag, Drag::Scrub));
        drag_update(&mut session, &mut drag, 333.0, 5.0, false);
        let per_frame = FrameRate::FPS_30.ticks_per_frame().unwrap();
        assert_eq!(session.playhead.0 % per_frame, 0);
        assert!((session.playhead.as_seconds_f64() - 3.33).abs() < 0.04);
    }

    #[test]
    fn pressing_on_empty_space_starts_a_marquee() {
        let (_tmp, mut session, _, _) = editing_session();
        let drag = begin_drag(&mut session, &view(), canvas(), pos2(900.0, y_of_row(0)));
        assert!(matches!(drag, Drag::Marquee { .. }));
    }

    #[test]
    fn the_playhead_is_a_snap_target() {
        let (_tmp, mut session, _, _) = editing_session();
        session.set_playhead(secs(3.0));
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(250.0, y_of_row(1)));
        // Move so the clip START lands ~3 ms from the playhead at 3.0 s (grab offset 2.5 s).
        let guide = drag_update(&mut session, &mut drag, 250.0 + 303.0, y_of_row(1), true);
        assert_eq!(guide, Some(secs(3.0)));
    }

    #[test]
    fn clips_can_be_extended_while_the_duration_is_unknown() {
        let (_tmp, mut session, _, b) = editing_session();
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(1097.0, y_of_row(1)));
        drag_update(&mut session, &mut drag, 1500.0, y_of_row(1), false);
        match &drag {
            Drag::TrimEnd { new_end, .. } => {
                assert!(
                    *new_end > secs(11.0),
                    "placeholder durations must not cap trimming"
                )
            }
            other => panic!("{other:?}"),
        }
        commit_drag(&mut session, &drag);
        let clip = session.doc.timeline.find_clip(b).unwrap().1;
        assert!(clip.duration > secs(5.0));
    }

    #[test]
    fn extending_is_still_stopped_by_the_next_clip() {
        let (_tmp, mut session, a, _) = editing_session();
        // Clip A ends at x = 500; clip B starts at x = 600.
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(497.0, y_of_row(1)));
        assert!(matches!(drag, Drag::TrimEnd { clip, .. } if clip == a));
        drag_update(&mut session, &mut drag, 900.0, y_of_row(1), false);
        match &drag {
            Drag::TrimEnd { new_end, .. } => assert_eq!(*new_end, secs(6.0)),
            other => panic!("{other:?}"),
        }
    }

    // ----- dropping library media onto the timeline ------------------------

    fn media_id(session: &Session) -> MediaId {
        session.doc.media[0].id
    }

    #[test]
    fn drop_target_lands_on_the_row_and_time_under_the_pointer() {
        let (_tmp, session, _, _) = editing_session();
        let target = drop_target(
            &session,
            &view(),
            canvas(),
            pos2(1300.0 - 500.0, y_of_row(0)), // 8.0 s on "Video 2"
            media_id(&session),
            false,
            view().ticks_for_pixels(SNAP_PX),
        )
        .unwrap();
        assert_eq!(target.track, session.doc.timeline.tracks[0].id);
        assert!((target.start.as_seconds_f64() - 8.0).abs() < 0.04);
        assert!(target.valid);
        assert_eq!(target.duration, secs(5.0));
    }

    #[test]
    fn drop_target_is_invalid_when_it_would_overlap() {
        let (_tmp, session, _, _) = editing_session();
        let target = drop_target(
            &session,
            &view(),
            canvas(),
            pos2(300.0, y_of_row(1)), // inside clip A on "Video 1"
            media_id(&session),
            false,
            view().ticks_for_pixels(SNAP_PX),
        )
        .unwrap();
        assert!(!target.valid);
    }

    #[test]
    fn the_ruler_and_the_space_below_make_new_tracks_but_row_gaps_do_nothing() {
        let (_tmp, session, _, _) = editing_session();
        let media = media_id(&session);
        let threshold = view().ticks_for_pixels(SNAP_PX);
        let at = |y: f32| {
            drop_target(
                &session,
                &view(),
                canvas(),
                pos2(300.0, y),
                media,
                false,
                threshold,
            )
        };

        let top = at(5.0).unwrap();
        assert_eq!(top.new_track, Some(NewTrackAt::Top));
        assert!(top.valid && !top.wrong_kind);
        let bottom = at(280.0).unwrap();
        assert_eq!(bottom.new_track, Some(NewTrackAt::Bottom));
        assert!(bottom.valid);
        let gap = row_top(canvas(), 1) - ROW_GAP / 2.0;
        assert_eq!(
            at(gap).unwrap().new_track,
            Some(NewTrackAt::Between(1)),
            "the line between two video rows makes a video track there"
        );
    }

    #[test]
    fn a_new_track_between_rows_stays_in_its_own_group() {
        let (_tmp, session, _, _) = editing_session();
        // Rows: Video 2, Video 1, Audio 1.
        let tl = &session.doc.timeline;
        let v = TrackKind::Video;
        let a = TrackKind::Audio;
        assert_eq!(
            new_track_insert_index(tl, v, NewTrackAt::Between(1)),
            Some(1)
        );
        assert_eq!(
            new_track_insert_index(tl, v, NewTrackAt::Between(2)),
            Some(2)
        );
        assert_eq!(
            new_track_insert_index(tl, a, NewTrackAt::Between(2)),
            Some(2)
        );
        assert_eq!(
            new_track_insert_index(tl, a, NewTrackAt::Between(1)),
            None,
            "no audio among video"
        );
        assert_eq!(
            new_track_insert_index(tl, v, NewTrackAt::Between(3)),
            None,
            "no video among audio"
        );
        // Not allowed for this kind: the line counts as the row under it.
        let line = row_top(canvas(), 1) - ROW_GAP / 2.0;
        assert_eq!(spot_for_kind(tl, canvas(), line, a), Spot::Nowhere);
        assert_eq!(
            spot_for_kind(tl, canvas(), line + 3.0, a),
            Spot::Row(1),
            "inside the row, an audio clip just sees the row"
        );
    }

    #[test]
    fn dragging_a_clip_onto_the_line_between_rows_makes_a_track_there() {
        let (_tmp, mut session, a, _) = editing_session();
        let view = view();
        let line = row_top(canvas(), 1) - ROW_GAP / 2.0;
        let (_, clip) = session.doc.timeline.find_clip(a).unwrap();
        let x = canvas().left() + view.x_for(clip.start) + 10.0;
        let mut drag = begin_drag(&mut session, &view, canvas(), pos2(x, y_of_row(1)));
        drag_update(&mut session, &mut drag, x, line, false);
        match &drag {
            Drag::Move { new_track, .. } => assert_eq!(*new_track, Some(NewTrackAt::Between(1))),
            other => panic!("{other:?}"),
        }
        commit_drag(&mut session, &drag);
        let names: Vec<&str> = session
            .doc
            .timeline
            .tracks
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names.len(), 4, "{names:?}");
        assert_eq!(session.doc.timeline.tracks[1].clips[0].id, a, "{names:?}");
    }

    #[test]
    fn video_over_an_audio_row_is_sound_only_when_it_has_sound() {
        let (_tmp, mut session, _, _) = editing_session();
        let media = media_id(&session);
        let threshold = view().ticks_for_pixels(SNAP_PX);
        let at = |session: &Session| {
            drop_target(
                session,
                &view(),
                canvas(),
                pos2(300.0, y_of_row(2)),
                media,
                false,
                threshold,
            )
            .unwrap()
        };
        assert!(
            !at(&session).wrong_kind,
            "unprobed video is assumed to have sound"
        );
        session.apply_probe(
            media,
            Some(secs(5.0)),
            cutroom_project::MediaProbe {
                has_video: true,
                has_audio: false,
                ..Default::default()
            },
        );
        let silent = at(&session);
        assert!(silent.wrong_kind && !silent.valid, "nothing to hear");
    }

    #[test]
    fn a_silent_clip_cannot_move_to_an_audio_row() {
        let (_tmp, mut session, a, _) = editing_session();
        let media = session.doc.timeline.find_clip(a).unwrap().1.media;
        session.apply_probe(
            media,
            Some(secs(5.0)),
            cutroom_project::MediaProbe {
                has_video: true,
                has_audio: false,
                ..Default::default()
            },
        );
        let video1 = session.doc.timeline.tracks[1].id;
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(250.0, y_of_row(1)));
        drag_update(&mut session, &mut drag, 250.0, y_of_row(2), false);
        match &drag {
            Drag::Move { track, .. } => assert_eq!(*track, video1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn dragging_a_clip_below_the_tracks_gives_it_a_new_track() {
        let (_tmp, mut session, a, _) = editing_session();
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(250.0, y_of_row(1)));
        drag_update(&mut session, &mut drag, 350.0, 280.0, false);
        match &drag {
            Drag::Move {
                new_track, valid, ..
            } => {
                assert_eq!(*new_track, Some(NewTrackAt::Bottom));
                assert!(*valid);
            }
            other => panic!("{other:?}"),
        }
        commit_drag(&mut session, &drag);
        let (track, clip) = session.doc.timeline.find_clip(a).unwrap();
        assert_eq!(track.name, "Video 3", "a video clip gets a video track");
        assert_eq!(
            session.doc.timeline.tracks[2].id, track.id,
            "just above the audio tracks"
        );
        assert!(clip.start > Ticks::ZERO, "it moved sideways too");
        session.undo();
        assert_eq!(session.doc.timeline.tracks.len(), 3);
        assert_eq!(
            session.doc.timeline.find_clip(a).unwrap().1.start,
            Ticks::ZERO
        );
    }

    #[test]
    fn dragging_a_clip_over_the_ruler_gives_it_a_new_top_track() {
        let (_tmp, mut session, a, _) = editing_session();
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(250.0, y_of_row(1)));
        drag_update(&mut session, &mut drag, 250.0, 5.0, false);
        assert!(matches!(
            &drag,
            Drag::Move {
                new_track: Some(NewTrackAt::Top),
                ..
            }
        ));
        commit_drag(&mut session, &drag);
        assert_eq!(
            session.doc.timeline.tracks[0].id,
            session.doc.timeline.find_clip(a).unwrap().0.id
        );
        assert_eq!(session.doc.timeline.tracks[0].name, "Video 3");
    }

    #[test]
    fn moving_back_over_a_row_cancels_the_new_track() {
        let (_tmp, mut session, _, _) = editing_session();
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(250.0, y_of_row(1)));
        drag_update(&mut session, &mut drag, 250.0, 280.0, false);
        drag_update(&mut session, &mut drag, 250.0, y_of_row(1), false);
        assert!(matches!(
            &drag,
            Drag::Move {
                new_track: None,
                ..
            }
        ));
    }

    #[test]
    fn audio_media_lands_on_audio_rows_and_not_on_video_rows() {
        let (tmp, mut session, _, _) = editing_session();
        let song_path = tmp.path().join("song.mp3");
        std::fs::write(&song_path, b"x").unwrap();
        let song = session.import_files(&[song_path])[0];
        let threshold = view().ticks_for_pixels(SNAP_PX);

        let on_audio = drop_target(
            &session,
            &view(),
            canvas(),
            pos2(300.0, y_of_row(2)),
            song,
            false,
            threshold,
        )
        .unwrap();
        assert!(on_audio.valid && !on_audio.wrong_kind);
        assert_eq!(on_audio.track, session.doc.timeline.tracks[2].id);

        let on_video = drop_target(
            &session,
            &view(),
            canvas(),
            pos2(300.0, y_of_row(0)),
            song,
            false,
            threshold,
        )
        .unwrap();
        assert!(on_video.wrong_kind && !on_video.valid);
    }

    #[test]
    fn drop_target_snaps_to_neighbouring_clip_edges() {
        let (_tmp, session, _, _) = editing_session();
        // Pointer at ~11.03 s on Video 1 snaps to the end of clip B at 11.0 s.
        let target = drop_target(
            &session,
            &view(),
            canvas(),
            pos2(1103.0, y_of_row(1)),
            media_id(&session),
            true,
            view().ticks_for_pixels(SNAP_PX),
        )
        .unwrap();
        assert_eq!(target.start, secs(11.0));
        assert!(target.valid);
    }

    #[test]
    fn drop_target_on_a_locked_track_is_invalid() {
        let (_tmp, mut session, _, _) = editing_session();
        let track = session.doc.timeline.tracks[0].id;
        session.toggle_track_locked(track);
        let target = drop_target(
            &session,
            &view(),
            canvas(),
            pos2(300.0, y_of_row(0)),
            media_id(&session),
            false,
            view().ticks_for_pixels(SNAP_PX),
        )
        .unwrap();
        assert!(!target.valid);
    }

    #[test]
    fn marquee_selects_clips_it_touches() {
        let (_tmp, mut session, a, b) = editing_session();
        let y = y_of_row(1);
        // Start in empty space left of nothing: row 1 is Video 1, clips at x 0..500 and 600..1100.
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(560.0, y));
        assert!(matches!(drag, Drag::Marquee { .. }));
        drag_update(&mut session, &mut drag, 700.0, y, false);
        assert_eq!(session.selection(), vec![b]);
        drag_update(&mut session, &mut drag, 100.0, y + 5.0, false);
        assert_eq!(session.selection(), vec![a]);
        // Spanning both rows' worth of time: from the far left to the far right.
        drag_update(&mut session, &mut drag, 1050.0, y, false);
        assert_eq!(session.selection(), vec![b]);
        drag_update(&mut session, &mut drag, 580.0, y, false);
        assert!(session.selection().is_empty());
    }

    #[test]
    fn marquee_additive_keeps_base_and_toggle_works() {
        let (_tmp, mut session, a, b) = editing_session();
        let y = y_of_row(1);
        session.select_only(a);
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(560.0, y));
        if let Drag::Marquee { additive, base, .. } = &mut drag {
            *additive = true;
            *base = session.selection();
        }
        drag_update(&mut session, &mut drag, 700.0, y, false);
        assert_eq!(session.selection(), vec![a, b]);
        session.toggle_selected(a);
        assert_eq!(session.selection(), vec![b]);
        session.clear_selection();
        assert!(session.selection().is_empty());
    }

    #[test]
    fn grabbing_a_selected_clip_keeps_the_group() {
        let (_tmp, mut session, a, b) = editing_session();
        session.select_many(&[a, b]);
        let _ = begin_drag(&mut session, &view(), canvas(), pos2(250.0, y_of_row(1)));
        assert_eq!(session.selection(), vec![a, b]);
        let _ = begin_drag(&mut session, &view(), canvas(), pos2(250.0, y_of_row(1)));
        session.select_only(b);
        let _ = begin_drag(&mut session, &view(), canvas(), pos2(250.0, y_of_row(1)));
        assert_eq!(session.selection(), vec![a]);
    }

    #[test]
    fn dragging_a_selected_clip_moves_the_whole_selection() {
        let (_tmp, mut session, a, b) = editing_session();
        session.select_many(&[a, b]);
        let y = y_of_row(1);
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(250.0, y));
        assert!(matches!(&drag, Drag::Move { group, .. } if group == &vec![b]));
        // A at 0..5 s grabbed at 2.5 s; pull 1 s right (also try to change track).
        drag_update(&mut session, &mut drag, 350.0, y_of_row(0), false);
        if let Drag::Move {
            start,
            track,
            orig_track,
            valid,
            ..
        } = &drag
        {
            assert_eq!(*start, secs(1.0));
            assert_eq!(track, orig_track);
            assert!(*valid);
        }
        commit_drag(&mut session, &drag);
        let tl = &session.doc.timeline;
        assert_eq!(tl.find_clip(a).unwrap().1.start, secs(1.0));
        assert_eq!(tl.find_clip(b).unwrap().1.start, secs(7.0));
    }

    #[test]
    fn a_group_cannot_start_before_zero() {
        let (_tmp, mut session, a, b) = editing_session();
        session.select_many(&[a, b]);
        let y = y_of_row(1);
        let mut drag = begin_drag(&mut session, &view(), canvas(), pos2(250.0, y));
        drag_update(&mut session, &mut drag, 20.0, y, false);
        if let Drag::Move { start, .. } = &drag {
            assert_eq!(*start, Ticks::ZERO);
        }
    }

    #[test]
    fn cut_handles_sit_on_touching_clips_only() {
        let (_tmp, mut session, a, b) = editing_session();
        let y = y_of_row(1);
        // A is 0..5 s and B is 6..11 s: a 1 s gap, no handle.
        assert_eq!(
            cut_handle_at(&session.doc.timeline, &view(), canvas(), pos2(600.0, y)),
            None
        );
        // Slide B against A.
        assert!(session.move_clip(
            b,
            session.doc.timeline.find_clip(b).unwrap().0.id,
            secs(5.0)
        ));
        assert_eq!(
            cut_handle_at(&session.doc.timeline, &view(), canvas(), pos2(504.0, y)),
            Some(b)
        );
        assert_eq!(
            cut_handle_at(
                &session.doc.timeline,
                &view(),
                canvas(),
                pos2(504.0, y + 30.0)
            ),
            None
        );
        assert!(session.can_crossfade(b) && !session.can_crossfade(a));
        assert!(session.set_clip_crossfade(b, secs(2.0)));
        assert_eq!(session.crossfade_of(b), secs(2.0));
        session.undo();
        assert_eq!(session.crossfade_of(b), Ticks::ZERO);
        // Far too long: kept within what the clips allow (never beyond the maximum).
        assert!(session.set_clip_crossfade(b, secs(500.0)));
        assert!(session.crossfade_of(b) <= cutroom_timeline::transition::MAX_CROSSFADE);
    }
}
