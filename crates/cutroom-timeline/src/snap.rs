use crate::ids::ClipId;
use crate::model::Timeline;
use crate::time::Ticks;

/// Times a dragged item may snap to: the timeline start and every clip edge.
/// `exclude` omits the clip currently being dragged.
pub fn snap_points(tl: &Timeline, exclude: Option<ClipId>) -> Vec<Ticks> {
    snap_points_with_extra(tl, exclude, &[])
}

/// Like [`snap_points`], plus caller-supplied points (e.g. the playhead).
pub fn snap_points_with_extra(
    tl: &Timeline,
    exclude: Option<ClipId>,
    extra: &[Ticks],
) -> Vec<Ticks> {
    let mut points = vec![Ticks::ZERO];
    points.extend_from_slice(extra);
    for clip in tl.tracks.iter().flat_map(|t| &t.clips) {
        if Some(clip.id) != exclude {
            points.push(clip.start);
            points.push(clip.end());
        }
    }
    points.sort();
    points.dedup();
    points
}

/// Nearest snap point within `threshold` of `candidate`, if any.
pub fn snap(
    tl: &Timeline,
    candidate: Ticks,
    threshold: Ticks,
    exclude: Option<ClipId>,
) -> Option<Ticks> {
    snap_with_extra(tl, candidate, threshold, exclude, &[])
}

/// [`snap`] with extra snap points.
pub fn snap_with_extra(
    tl: &Timeline,
    candidate: Ticks,
    threshold: Ticks,
    exclude: Option<ClipId>,
    extra: &[Ticks],
) -> Option<Ticks> {
    snap_points_with_extra(tl, exclude, extra)
        .into_iter()
        .map(|p| (p, (p - candidate).abs()))
        .filter(|&(_, distance)| distance <= threshold)
        .min_by_key(|&(_, distance)| distance)
        .map(|(p, _)| p)
}

/// Snap a dragged span: whichever edge (start or end) is closest to a snap
/// point wins. Returns the adjusted start, or `None` if neither edge is in range.
pub fn snap_span(
    tl: &Timeline,
    start: Ticks,
    duration: Ticks,
    threshold: Ticks,
    exclude: Option<ClipId>,
) -> Option<Ticks> {
    snap_span_with_extra(tl, start, duration, threshold, exclude, &[])
}

/// [`snap_span`] with extra snap points.
pub fn snap_span_with_extra(
    tl: &Timeline,
    start: Ticks,
    duration: Ticks,
    threshold: Ticks,
    exclude: Option<ClipId>,
    extra: &[Ticks],
) -> Option<Ticks> {
    let start_snap =
        snap_with_extra(tl, start, threshold, exclude, extra).map(|p| (p, (p - start).abs()));
    let end_snap = snap_with_extra(tl, start + duration, threshold, exclude, extra)
        .map(|p| (p - duration, (p - (start + duration)).abs()));
    match (start_snap, end_snap) {
        (Some(a), Some(b)) => Some(if a.1 <= b.1 { a.0 } else { b.0 }),
        (Some(a), None) => Some(a.0),
        (None, Some(b)) => Some(b.0),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{apply, Edit};
    use crate::ids::MediaId;
    use crate::model::TrackKind;

    fn timeline() -> (Timeline, ClipId) {
        let mut tl = Timeline::new();
        let track = tl.new_track(TrackKind::Video, "V1");
        let track_id = track.id;
        apply(&mut tl, Edit::InsertTrack { index: 0, track }).unwrap();
        let a = tl.new_clip(MediaId(1), Ticks(0), Ticks(100), Ticks(50));
        let a_id = a.id;
        apply(
            &mut tl,
            Edit::InsertClip {
                track: track_id,
                clip: a,
            },
        )
        .unwrap();
        (tl, a_id)
    }

    #[test]
    fn snaps_to_clip_edges_within_threshold() {
        let (tl, _) = timeline();
        assert_eq!(snap(&tl, Ticks(148), Ticks(5), None), Some(Ticks(150)));
        assert_eq!(snap(&tl, Ticks(103), Ticks(5), None), Some(Ticks(100)));
        assert_eq!(snap(&tl, Ticks(125), Ticks(5), None), None);
    }

    #[test]
    fn excluded_clip_does_not_snap_to_itself() {
        let (tl, id) = timeline();
        assert_eq!(snap(&tl, Ticks(101), Ticks(5), Some(id)), None);
    }

    #[test]
    fn extra_points_such_as_the_playhead_also_snap() {
        let (tl, _) = timeline();
        assert_eq!(snap(&tl, Ticks(300), Ticks(5), None), None);
        assert_eq!(
            snap_with_extra(&tl, Ticks(303), Ticks(5), None, &[Ticks(300)]),
            Some(Ticks(300))
        );
        assert_eq!(
            snap_span_with_extra(&tl, Ticks(296), Ticks(10), Ticks(5), None, &[Ticks(300)]),
            Some(Ticks(300))
        );
    }

    #[test]
    fn span_snaps_by_its_end_edge() {
        let (tl, _) = timeline();
        // A 30-tick clip dropped at 118 ends at 148; snapping its end to the
        // existing clip's end at 150 moves the start to 120.
        assert_eq!(
            snap_span(&tl, Ticks(118), Ticks(30), Ticks(5), None),
            Some(Ticks(120))
        );
    }
}
