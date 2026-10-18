//! The timeline: a custom-drawn canvas (ruler, tracks, clips, playhead) with
//! mouse editing, zoom and drag-and-drop.

use crate::app::App;
use crate::theme::{self, Palette, Rgb};
use cutroom_core::geom::{pos2, vec2, Pos2, Rect};
use cutroom_core::state::{media_label, new_track_index, MediaKind};
use cutroom_core::timeline_logic::{
    begin_drag, commit_drag, cut_handle_at, drop_target, hit_test, row_at, row_top, update_drag,
    Drag, Hit, NewTrackAt, Tool, Zone, HEADER_W, ROW_GAP, ROW_H, RULER_H, SNAP_PX,
};
use cutroom_core::view::{self, TimelineView};
use cutroom_timeline::{MediaId, Ticks, Timeline, TrackKind, TICKS_PER_SECOND};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{cairo, gdk, glib};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

pub struct TimelineUi {
    pub area: gtk::DrawingArea,
}

/// The time area of the canvas in widget coordinates (right of the track headers).
fn canvas_rect(w: f64, h: f64) -> Rect {
    Rect::from_min_size(
        pos2(HEADER_W, 0.0),
        vec2((w as f32 - HEADER_W).max(1.0), h as f32),
    )
}

pub fn content_height(rows: usize) -> i32 {
    (RULER_H + rows as f32 * (ROW_H + ROW_GAP) + 6.0) as i32
}

pub fn build(app: &Rc<App>) -> TimelineUi {
    let area = gtk::DrawingArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.set_focusable(true);
    area.set_content_height(content_height(3));
    {
        let app = app.clone();
        area.set_draw_func(move |a, cr, w, h| {
            draw(
                &app,
                cr,
                f64::from(w),
                f64::from(h),
                a.scale_factor().max(1),
            );
            pin_ruler(&app, a, cr, f64::from(w));
            crate::range_select::paint(&app, cr, f64::from(w), f64::from(h), voff(a));
        });
    }
    attach_mouse(app, &area);
    attach_scroll_and_motion(app, &area);
    attach_drop(app, &area);
    TimelineUi { area }
}

/// How far the timeline has been scrolled down; the ruler stays pinned to the top.
fn voff(area: &gtk::DrawingArea) -> f64 {
    area.ancestor(gtk::ScrolledWindow::static_type())
        .and_then(|w| w.downcast::<gtk::ScrolledWindow>().ok())
        .map_or(0.0, |sw| sw.vadjustment().value())
}

/// Pointer y in the layout the hit-testing knows: the pinned ruler counts as the ruler.
fn ey(area: &gtk::DrawingArea, y: f64) -> f64 {
    let off = voff(area);
    if off > 0.0 && y >= off && y < off + f64::from(RULER_H) {
        y - off
    } else {
        y
    }
}

pub fn queue_draw(app: &App) {
    app.ui().editor.timeline.area.queue_draw();
    app.ui().editor.queue_navigator();
}

// ----- drawing -------------------------------------------------------------------

fn set(cr: &cairo::Context, color: Rgb, alpha: f64) {
    cr.set_source_rgba(color.0, color.1, color.2, alpha);
}

fn fill_rect(cr: &cairo::Context, color: Rgb, x: f64, y: f64, w: f64, h: f64) {
    set(cr, color, 1.0);
    cr.rectangle(x, y, w, h);
    let _ = cr.fill();
}

fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(
        x + r,
        y + h - r,
        r,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(
        x + r,
        y + r,
        r,
        std::f64::consts::PI,
        3.0 * std::f64::consts::FRAC_PI_2,
    );
    cr.close_path();
}

fn text(cr: &cairo::Context, s: &str, x: f64, baseline: f64, size: f64, color: Rgb, bold: bool) {
    cr.select_font_face(
        crate::fonts::FAMILY,
        cairo::FontSlant::Normal,
        if bold {
            cairo::FontWeight::Bold
        } else {
            cairo::FontWeight::Normal
        },
    );
    cr.set_font_size(size);
    set(cr, color, 1.0);
    cr.move_to(x, baseline);
    let _ = cr.show_text(s);
}

fn line(cr: &cairo::Context, color: Rgb, width: f64, x1: f64, y1: f64, x2: f64, y2: f64) {
    set(cr, color, 1.0);
    cr.set_line_width(width);
    cr.move_to(x1, y1);
    cr.line_to(x2, y2);
    let _ = cr.stroke();
}

fn clip_to(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64) {
    cr.rectangle(x, y, w.max(0.0), h.max(0.0));
    cr.clip();
}

fn clip_x(canvas: Rect, view: &TimelineView, t: Ticks) -> f64 {
    f64::from(canvas.left() + view.x_for(t))
}

/// Header button rectangles for a row: (mute, hide, lock), as (x, width).
fn header_buttons() -> [(f64, f64); 3] {
    let w = 22.0;
    let lock_x = f64::from(HEADER_W) - w - 6.0;
    let hide_x = lock_x - w - 4.0;
    let mute_x = hide_x - w - 4.0;
    [(mute_x, w), (hide_x, w), (lock_x, w)]
}

fn draw(app: &App, cr: &cairo::Context, w: f64, h: f64, scale: i32) {
    let st = app.state.borrow();
    let Some(session) = st.session.as_ref() else {
        return;
    };
    let pal = Palette::of(st.dark);
    let tl = &session.doc.timeline;
    let view = &st.view;
    let rate = session.doc.settings.frame_rate;
    let canvas = canvas_rect(w, h);
    let left = f64::from(canvas.left());

    fill_rect(cr, pal.canvas_bg, 0.0, 0.0, w, h);

    // ---- everything in the time area is clipped to it
    cr.save().ok();
    clip_to(cr, left, 0.0, w - left, h);

    paint_ruler(cr, &pal, view, canvas, w);

    for (row, track) in tl.tracks.iter().enumerate() {
        let top = f64::from(row_top(canvas, row));
        let row_bg = if row % 2 == 0 {
            pal.row_even
        } else {
            pal.row_odd
        };
        fill_rect(cr, row_bg, left, top, w - left, f64::from(ROW_H));

        let base = match track.kind {
            TrackKind::Video => theme::VIDEO_CLIP,
            TrackKind::Audio => theme::AUDIO_CLIP,
        };
        let (fill_base, label_color) = if track.muted {
            (theme::mix(base, row_bg, 0.6), pal.clip_text_muted)
        } else {
            (base, pal.clip_text)
        };

        for clip in &track.clips {
            let moving = matches!(&st.drag, Drag::Move { clip: id, group, .. }
                if *id == clip.id || group.contains(&clip.id));
            let (start, end) = match &st.drag {
                Drag::TrimStart {
                    clip: id,
                    new_start,
                    end,
                    ..
                } if *id == clip.id => (*new_start, *end),
                Drag::TrimEnd {
                    clip: id,
                    start,
                    new_end,
                    ..
                } if *id == clip.id => (*start, *new_end),
                _ => (clip.start, clip.end()),
            };
            let x1 = clip_x(canvas, view, start);
            let x2 = clip_x(canvas, view, end);
            if x2 < left || x1 > w {
                continue;
            }
            let (y1, ch) = (top + 2.0, f64::from(ROW_H) - 4.0);
            let selected = session.is_selected(clip.id);
            let fill_base = if clip.text.is_some() {
                if track.muted {
                    theme::mix(theme::TEXT_CLIP, row_bg, 0.6)
                } else {
                    theme::TEXT_CLIP
                }
            } else {
                fill_base
            };
            let fill = if moving {
                theme::mix(fill_base, row_bg, 0.75)
            } else {
                fill_base
            };
            set(cr, fill, 1.0);
            rounded(cr, x1, y1, x2 - x1, ch, 4.0);
            let _ = cr.fill();
            if selected {
                set(cr, pal.selected_outline, 1.0);
                cr.set_line_width(2.0);
            } else {
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.35);
                cr.set_line_width(1.0);
            }
            rounded(cr, x1 + 0.5, y1 + 0.5, (x2 - x1) - 1.0, ch - 1.0, 4.0);
            let _ = cr.stroke();

            if !moving {
                // Filmstrip and waveform sit behind the label.
                let shown_source_in = clip.source_in + (start - clip.start);
                cr.save().ok();
                clip_to(cr, x1 + 1.0, y1 + 1.0, (x2 - x1) - 2.0, ch - 2.0);
                let analysis = app.analysis.borrow();
                if track.kind == TrackKind::Video {
                    if let Some(strip) = analysis.strip(clip.media) {
                        draw_filmstrip(cr, &strip, view, x1, x2, left, w, y1, ch, shown_source_in);
                    }
                }
                let silent_video = track.kind == TrackKind::Video && clip.audio_muted;
                if let Some(peaks) = analysis.peaks(clip.media).filter(|_| !silent_video) {
                    let video = track.kind == TrackKind::Video;
                    if video {
                        // A dark band so the waveform reads over any picture.
                        cr.set_source_rgba(0.0, 0.0, 0.0, 0.38);
                        cr.rectangle(x1, y1 + ch * 0.58, x2 - x1, ch * 0.42);
                        let _ = cr.fill();
                    }
                    let (y_mid, half) = if video {
                        (y1 + ch * 0.79, ch * 0.19)
                    } else {
                        (y1 + ch * 0.70, ch * 0.27)
                    };
                    cr.set_source_rgba(1.0, 1.0, 1.0, if track.muted { 0.25 } else { 0.75 });
                    draw_waveform(
                        cr,
                        &peaks,
                        view,
                        x1,
                        x2,
                        left,
                        w,
                        shown_source_in,
                        y_mid,
                        half,
                    );
                }
                drop(analysis);
                cr.restore().ok();

                cr.save().ok();
                clip_to(cr, x1 + 3.0, y1, (x2 - x1) - 6.0, ch);
                let name = match &clip.text {
                    Some(spec) => format!("T  {}", spec.label()),
                    None => session
                        .doc
                        .media_item(clip.media)
                        .map(media_label)
                        .unwrap_or_else(|_| "missing media".into()),
                };
                let color = if clip.text.is_none() && session.missing.contains(&clip.media) {
                    (1.0, 0.84, 0.35)
                } else {
                    label_color
                };
                // A shadow keeps the name readable over pictures and waveforms.
                text(
                    cr,
                    &name,
                    x1 + 7.0,
                    y1 + 17.0,
                    12.0,
                    theme::rgb(0, 0, 0),
                    false,
                );
                text(cr, &name, x1 + 6.0, y1 + 16.0, 12.0, color, false);
                let sub = if track.muted {
                    pal.clip_text_muted
                } else {
                    pal.clip_subtext
                };
                text(
                    cr,
                    &view::timecode(end - start, rate),
                    x1 + 6.0,
                    y1 + ch - 6.0,
                    10.0,
                    sub,
                    false,
                );
                // Keyframes: little diamonds along the bottom of the clip.
                let held = match &st.drag {
                    Drag::Key {
                        clip: id, from, to, ..
                    } if *id == clip.id => Some((*from, *to)),
                    _ => None,
                };
                let mut times: Vec<Ticks> = clip
                    .keyframes
                    .iter()
                    .map(|k| match held {
                        Some((from, to)) if k.time == from => to,
                        _ => k.time,
                    })
                    .collect();
                times.sort();
                times.dedup();
                for time in times {
                    let lifted = held.is_some_and(|(_, to)| to == time);
                    let kx = clip_x(canvas, view, clip.start + time);
                    let ky = y1 + 10.0;
                    cr.move_to(kx, ky - 7.0);
                    cr.line_to(kx + 7.0, ky);
                    cr.line_to(kx, ky + 7.0);
                    cr.line_to(kx - 7.0, ky);
                    cr.close_path();
                    if lifted {
                        cr.set_source_rgb(1.0, 1.0, 1.0);
                    } else {
                        set(cr, theme::ACCENT, 1.0);
                    }
                    let _ = cr.fill_preserve();
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.7);
                    cr.set_line_width(1.0);
                    let _ = cr.stroke();
                }
                // Sound level: a bar near the left edge with a knob; middle is
                // "as recorded", the top is double and the bottom is silence.
                if x2 - x1 >= f64::from(cutroom_core::timeline_logic::VOLUME_MIN_WIDTH)
                    && session.clip_has_sound(clip, track.kind)
                {
                    let level = match &st.drag {
                        Drag::Volume { clip: id, to, .. } if *id == clip.id => *to,
                        _ => clip.volume,
                    };
                    let (bar_top, bar_bottom) = (y1 + 7.0, y1 + ch - 7.0);
                    let bx = x1 + f64::from(cutroom_core::timeline_logic::VOLUME_X);
                    let frac = f64::from(level) / f64::from(cutroom_timeline::MAX_VOLUME);
                    let ky = bar_bottom - frac * (bar_bottom - bar_top);
                    let mid = (bar_top + bar_bottom) / 2.0;
                    // The level across the clip, and the bar with its middle mark.
                    cr.set_source_rgba(1.0, 1.0, 1.0, if level == 1000 { 0.18 } else { 0.5 });
                    cr.set_line_width(1.0);
                    cr.set_dash(&[4.0, 3.0], 0.0);
                    cr.move_to(bx, ky);
                    cr.line_to(x2 - 4.0, ky);
                    let _ = cr.stroke();
                    cr.set_dash(&[], 0.0);
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
                    cr.set_line_width(4.0);
                    cr.move_to(bx, bar_top);
                    cr.line_to(bx, bar_bottom);
                    let _ = cr.stroke();
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.85);
                    cr.set_line_width(2.0);
                    cr.move_to(bx, bar_top);
                    cr.line_to(bx, bar_bottom);
                    let _ = cr.stroke();
                    cr.move_to(bx - 4.0, mid);
                    cr.line_to(bx + 4.0, mid);
                    let _ = cr.stroke();
                    cr.arc(bx, ky, 5.0, 0.0, std::f64::consts::TAU);
                    set(cr, theme::ACCENT, 1.0);
                    let _ = cr.fill_preserve();
                    cr.set_source_rgb(1.0, 1.0, 1.0);
                    cr.set_line_width(1.5);
                    let _ = cr.stroke();
                    if matches!(&st.drag, Drag::Volume { clip: id, .. } if *id == clip.id) {
                        text(
                            cr,
                            &format!("{}%", level / 10),
                            bx + 10.0,
                            ky - 6.0,
                            11.0,
                            (1.0, 1.0, 1.0),
                            true,
                        );
                    }
                }
                cr.restore().ok();
            }
            if clip.disabled {
                // Deactivated: veiled with diagonal stripes.
                cr.save().ok();
                rounded(cr, x1, y1, x2 - x1, ch, 4.0);
                cr.clip();
                set(cr, row_bg, 0.72);
                cr.rectangle(x1, y1, x2 - x1, ch);
                let _ = cr.fill();
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.10);
                cr.set_line_width(1.5);
                let mut sx = x1 - ch;
                while sx < x2 {
                    cr.move_to(sx, y1 + ch);
                    cr.line_to(sx + ch, y1);
                    sx += 9.0;
                }
                let _ = cr.stroke();
                cr.restore().ok();
            }
        }

        // Cuts between touching clips: the crossfade band and its button.
        for (i, clip) in track.clips.iter().enumerate() {
            if cutroom_timeline::transition::previous_touching(track, i).is_none() {
                continue;
            }
            let cx = clip_x(canvas, view, clip.start);
            if cx < left - 12.0 || cx > w + 12.0 {
                continue;
            }
            let mid = top + f64::from(ROW_H) / 2.0;
            if let Some((from, to)) = cutroom_timeline::transition::window(track, i) {
                let (x1, x2) = (clip_x(canvas, view, from), clip_x(canvas, view, to));
                cr.save().ok();
                clip_to(cr, left, top, w - left, f64::from(ROW_H));
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.22);
                cr.rectangle(x1, top + 2.0, x2 - x1, f64::from(ROW_H) - 4.0);
                let _ = cr.fill();
                cr.restore().ok();
            }
            let active = cutroom_timeline::transition::effective(track, i) > Ticks::ZERO;
            let side = f64::from(cutroom_core::timeline_logic::CUT_HANDLE) * 2.0;
            rounded(cr, cx - side / 2.0, mid - side / 2.0, side, side, 4.0);
            if active {
                set(cr, theme::ACCENT, 1.0);
            } else {
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
            }
            let _ = cr.fill();
            let tint = if active {
                (0.0, 0.0, 0.0)
            } else {
                (1.0, 1.0, 1.0)
            };
            crate::icons::paint(cr, "arrows-exchange", tint, cx - 7.0, mid - 7.0, 14, scale);
        }

        if track.hidden {
            set(cr, row_bg, 0.7);
            cr.rectangle(left, top, w - left, f64::from(ROW_H));
            let _ = cr.fill();
        }
        if track.locked {
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.28);
            cr.rectangle(left, top, w - left, f64::from(ROW_H));
            let _ = cr.fill();
        }
    }

    // The clip being moved, where it would land.
    if let Drag::Move {
        track,
        orig_track,
        start,
        duration,
        valid,
        new_track,
        group,
        orig_start,
        ..
    } = &st.drag
    {
        if !group.is_empty() {
            let delta = *start - *orig_start;
            for (row, t) in tl.tracks.iter().enumerate() {
                for c in &t.clips {
                    if group.contains(&c.id) || (t.id == *orig_track && c.start == *orig_start) {
                        paint_ghost(cr, canvas, view, row, c.start + delta, c.duration, *valid);
                    }
                }
            }
        } else if let Some(at) = new_track {
            let kind = tl.track(*orig_track).map_or(TrackKind::Video, |t| t.kind);
            paint_new_track_ghost(cr, canvas, view, tl, kind, *at, *start, *duration);
        } else if let Some(row) = tl.tracks.iter().position(|t| t.id == *track) {
            paint_ghost(cr, canvas, view, row, *start, *duration, *valid);
        }
    }
    // Rubber band for multi-select.
    if let Drag::Marquee {
        origin, current, ..
    } = &st.drag
    {
        let (x, y) = (
            f64::from(origin.x.min(current.x)),
            f64::from(origin.y.min(current.y)),
        );
        let (w, h) = (
            f64::from((origin.x - current.x).abs()),
            f64::from((origin.y - current.y).abs()),
        );
        set(cr, pal.selected_outline, 0.18);
        cr.rectangle(x, y, w, h);
        let _ = cr.fill_preserve();
        set(cr, pal.selected_outline, 0.9);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    }
    // Library media being dragged in.
    if let Some(target) = &st.drop_ghost {
        if let Some((clip, ok)) = &target.replace {
            if let Some((row, c)) = tl
                .tracks
                .iter()
                .enumerate()
                .find_map(|(i, t)| t.clips.iter().find(|c| c.id == *clip).map(|c| (i, c)))
            {
                paint_replace_ghost(cr, canvas, view, row, c.start, c.duration, ok);
            }
        } else if let Some(at) = target.new_track {
            paint_new_track_ghost(
                cr,
                canvas,
                view,
                tl,
                target.new_kind,
                at,
                target.start,
                target.duration,
            );
        } else if let Some(row) = tl.tracks.iter().position(|t| t.id == target.track) {
            paint_ghost(
                cr,
                canvas,
                view,
                row,
                target.start,
                target.duration,
                target.valid,
            );
        }
    }

    // Split tool: a hairline where the cut would happen.
    if st.tool == Tool::Split {
        if let Some(pos) = st.hover {
            if let Hit::Clip { .. } = hit_test(tl, view, canvas, pos) {
                if let Some(row) = row_at(canvas, pos.y, tl.tracks.len()) {
                    let top = f64::from(row_top(canvas, row));
                    line(
                        cr,
                        pal.playhead,
                        1.5,
                        f64::from(pos.x),
                        top,
                        f64::from(pos.x),
                        top + f64::from(ROW_H),
                    );
                }
            }
        }
    }

    if let Some(guide) = st.snap_guide {
        let x = clip_x(canvas, view, guide);
        line(cr, pal.snap_guide, 1.0, x, f64::from(RULER_H), x, h);
    }

    // Playhead.
    let x = clip_x(canvas, view, session.playhead);
    if x >= left - 8.0 && x <= w + 8.0 {
        line(cr, pal.playhead, 1.5, x, 0.0, x, h);
        set(cr, pal.playhead, 1.0);
        cr.move_to(x - 6.0, 0.0);
        cr.line_to(x + 6.0, 0.0);
        cr.line_to(x, 11.0);
        cr.close_path();
        let _ = cr.fill();
    }
    cr.restore().ok();

    // ---- track headers (drawn over the left edge)
    fill_rect(cr, pal.header_bg, 0.0, 0.0, left, h);
    fill_rect(cr, pal.ruler_bg, 0.0, 0.0, left, f64::from(RULER_H));
    for (row, track) in tl.tracks.iter().enumerate() {
        let top = f64::from(row_top(canvas, row));
        let row_bg = if row % 2 == 0 {
            pal.row_even
        } else {
            pal.row_odd
        };
        fill_rect(
            cr,
            theme::mix(pal.header_bg, row_bg, 0.35),
            0.0,
            top,
            left,
            f64::from(ROW_H),
        );
        cr.save().ok();
        clip_to(cr, 0.0, top, left, f64::from(ROW_H));
        text(
            cr,
            &track.name,
            10.0,
            top + 28.0,
            13.0,
            pal.header_text,
            true,
        );
        let [(mx, mw), (hx, hw), (lx, lw)] = header_buttons();
        for (x, bw, icon, on) in [
            (
                mx,
                mw,
                if track.muted { "volume-off" } else { "volume" },
                track.muted,
            ),
            (
                hx,
                hw,
                if track.hidden { "eye-off" } else { "eye" },
                track.hidden,
            ),
            (
                lx,
                lw,
                if track.locked { "lock" } else { "lock-open" },
                track.locked,
            ),
        ] {
            let by = top + (f64::from(ROW_H) - 22.0) / 2.0;
            if on {
                set(cr, theme::ACCENT, 0.9);
            } else {
                set(cr, pal.header_text, 0.14);
            }
            rounded(cr, x, by, bw, 22.0, 4.0);
            let _ = cr.fill();
            let tint = if on { (0.0, 0.0, 0.0) } else { pal.header_text };
            crate::icons::paint(cr, icon, tint, x + 3.0, by + 3.0, 16, scale);
        }
        cr.restore().ok();
    }
    // A track header being dragged: its row lit, and a line where it will go.
    if let Some((id, target)) = st.track_drag {
        if let Some(from) = tl.tracks.iter().position(|t| t.id == id) {
            let top = f64::from(row_top(canvas, from));
            set(cr, theme::ACCENT, 0.18);
            cr.rectangle(0.0, top, w, f64::from(ROW_H));
            let _ = cr.fill();
            let y = if target <= from {
                f64::from(row_top(canvas, target)) - f64::from(ROW_GAP) / 2.0
            } else {
                f64::from(row_top(canvas, target)) + f64::from(ROW_H) + f64::from(ROW_GAP) / 2.0
            };
            line(cr, theme::ACCENT, 3.0, 0.0, y, w, y);
        }
    }
    line(cr, pal.tick_minor, 1.0, left, 0.0, left, h);
}

/// Ask for a track's new name.
fn rename_track(app: &Rc<App>, track: cutroom_timeline::TrackId) {
    let current = {
        let st = app.state.borrow();
        st.session
            .as_ref()
            .and_then(|s| s.doc.timeline.track(track).map(|t| t.name.clone()))
    };
    let Some(current) = current else { return };
    let app2 = app.clone();
    crate::text_ui::ask_text(
        app,
        "Rename track",
        "Name of the track",
        "Rename",
        &current,
        Rc::new(move |name| {
            let result = app2
                .state
                .borrow_mut()
                .session
                .as_mut()
                .map_or(Ok(()), |s| s.rename_track(track, name));
            app2.refresh();
            result
        }),
    );
}

/// The right-click menu of a track header: rename, move up, move down.
fn track_menu(
    app: &Rc<App>,
    anchor: &gtk::Widget,
    track: cutroom_timeline::TrackId,
    x: f64,
    y: f64,
) {
    let (range, place, captions) = {
        let st = app.state.borrow();
        let Some(s) = st.session.as_ref() else { return };
        let tl = &s.doc.timeline;
        (
            s.track_move_range(track),
            tl.tracks.iter().position(|t| t.id == track).unwrap_or(0),
            tl.track(track)
                .is_some_and(cutroom_core::state::is_caption_track),
        )
    };
    let popover = gtk::Popover::new();
    popover.set_parent(anchor);
    popover.set_has_arrow(false);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.add_css_class("menu");
    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let add = |label: &str, enabled: bool, act: Box<dyn Fn()>| {
        let b = gtk::Button::with_label(label);
        b.add_css_class("flat");
        b.set_sensitive(enabled);
        if let Some(l) = b.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
            l.set_xalign(0.0);
        }
        let popover = popover.clone();
        b.connect_clicked(move |_| {
            popover.popdown();
            act();
        });
        menu.append(&b);
    };
    {
        let app = app.clone();
        add(
            if captions {
                "Rename... (the Captions track keeps its name)"
            } else {
                "Rename..."
            },
            !captions,
            Box::new(move || rename_track(&app, track)),
        );
    }
    for (label, delta) in [("Move up", -1i64), ("Move down", 1i64)] {
        let enabled = range.is_some_and(|(low, high)| {
            let to = place as i64 + delta;
            to >= low as i64 && to <= high as i64
        });
        let app = app.clone();
        add(
            label,
            enabled,
            Box::new(move || {
                if let Some(s) = app.state.borrow_mut().session.as_mut() {
                    s.move_track(track, (place as i64 + delta).max(0) as usize);
                }
                app.sync_preview(true);
                app.refresh();
            }),
        );
    }
    popover.set_child(Some(&menu));
    popover.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    popover.popup();
}

/// The clip's picture strip: frames tiled along its length.
#[allow(clippy::too_many_arguments)]
fn draw_filmstrip(
    cr: &cairo::Context,
    strip: &crate::strips::Filmstrip,
    view: &TimelineView,
    x1: f64,
    x2: f64,
    left: f64,
    w: f64,
    top: f64,
    height: f64,
    source_in: Ticks,
) {
    let pps = view.pixels_per_second;
    let origin = source_in.as_seconds_f64();
    // Start at the first tile that is on screen.
    let first = ((left - x1).max(0.0) / strip.frame_w).floor();
    let mut x = x1 + first * strip.frame_w;
    while x < x2.min(w) {
        let centre = (x + strip.frame_w / 2.0 - x1).max(0.0);
        if let Some(frame) = strip.frame_for(origin + centre / pps) {
            cr.save().ok();
            cr.rectangle(x, top, strip.frame_w.min(x2 - x), height);
            cr.clip();
            let _ = cr.set_source_surface(frame, x, top);
            let _ = cr.paint();
            cr.restore().ok();
        }
        x += strip.frame_w;
    }
    // Slightly dim the picture so labels and the playhead stay readable.
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.18);
    cr.rectangle(x1, top, x2 - x1, height);
    let _ = cr.fill();
}

/// Mirror waveform, one pixel column per peak span; the colour is already set.
#[allow(clippy::too_many_arguments)]
fn draw_waveform(
    cr: &cairo::Context,
    peaks: &cutroom_core::waveform::Peaks,
    view: &TimelineView,
    x1: f64,
    x2: f64,
    left: f64,
    w: f64,
    source_in: Ticks,
    y_mid: f64,
    half_height: f64,
) {
    let (from, to) = (x1.max(left), x2.min(w));
    if to <= from {
        return;
    }
    let seconds_per_pixel = 1.0 / view.pixels_per_second;
    let start = source_in.as_seconds_f64() + (from - x1) * seconds_per_pixel;
    let columns = peaks.columns(start, seconds_per_pixel, (to - from).ceil() as usize);
    for (i, peak) in columns.iter().enumerate() {
        let h = cutroom_core::waveform::display_height(*peak, half_height).max(0.5);
        cr.rectangle(from + i as f64, y_mid - h, 1.0, h * 2.0);
    }
    let _ = cr.fill();
}

#[allow(clippy::too_many_arguments)]
/// Media held over a clip: the clip is outlined with what letting go will do.
fn paint_replace_ghost(
    cr: &cairo::Context,
    canvas: Rect,
    view: &TimelineView,
    row: usize,
    start: Ticks,
    duration: Ticks,
    ok: &Result<(), String>,
) {
    paint_ghost(cr, canvas, view, row, start, duration, ok.is_ok());
    let top = f64::from(row_top(canvas, row));
    let x = clip_x(canvas, view, start).max(canvas.left() as f64) + 6.0;
    let label = match ok {
        Ok(()) => "Replace".to_string(),
        Err(why) => why.clone(),
    };
    text(
        cr,
        &label,
        x,
        top + f64::from(ROW_H) / 2.0 + 4.0,
        12.0,
        (1.0, 1.0, 1.0),
        true,
    );
}

fn paint_ghost(
    cr: &cairo::Context,
    canvas: Rect,
    view: &TimelineView,
    row: usize,
    start: Ticks,
    duration: Ticks,
    valid: bool,
) {
    paint_ghost_at(
        cr,
        canvas,
        view,
        f64::from(row_top(canvas, row)),
        start,
        duration,
        valid,
    );
}

fn paint_ghost_at(
    cr: &cairo::Context,
    canvas: Rect,
    view: &TimelineView,
    top: f64,
    start: Ticks,
    duration: Ticks,
    valid: bool,
) {
    let (x1, x2) = (
        clip_x(canvas, view, start),
        clip_x(canvas, view, start + duration),
    );
    let color = if valid { theme::ACCENT } else { theme::INVALID };
    set(cr, color, 0.45);
    rounded(cr, x1, top + 2.0, x2 - x1, f64::from(ROW_H) - 4.0, 4.0);
    let _ = cr.fill();
    set(cr, color, 1.0);
    cr.set_line_width(2.0);
    rounded(
        cr,
        x1 + 1.0,
        top + 3.0,
        (x2 - x1) - 2.0,
        f64::from(ROW_H) - 6.0,
        4.0,
    );
    let _ = cr.stroke();
}

/// A new track is about to be made: a line where it will be inserted, with
/// the clip's ghost sitting on it and a label.
#[allow(clippy::too_many_arguments)]
fn paint_new_track_ghost(
    cr: &cairo::Context,
    canvas: Rect,
    view: &TimelineView,
    tl: &Timeline,
    kind: TrackKind,
    at: NewTrackAt,
    start: Ticks,
    duration: Ticks,
) {
    let index = cutroom_core::timeline_logic::new_track_insert_index(tl, kind, at)
        .unwrap_or_else(|| new_track_index(tl, kind, at == NewTrackAt::Top));
    let boundary = f64::from(row_top(canvas, index)) - f64::from(ROW_GAP) / 2.0;
    let (left, right) = (f64::from(canvas.left()), f64::from(canvas.right()));
    line(cr, theme::ACCENT, 3.0, left, boundary, right, boundary);
    paint_ghost_at(
        cr,
        canvas,
        view,
        boundary - f64::from(ROW_H) / 2.0,
        start,
        duration,
        true,
    );
    let label = match kind {
        TrackKind::Video => "New video track",
        TrackKind::Audio => "New audio track",
    };
    let x = clip_x(canvas, view, start).max(left) + 8.0;
    text(cr, label, x, boundary + 4.0, 12.0, theme::ACCENT, true);
}

/// When scrolled down, repaint the ruler and its corner at the top of the view.
fn pin_ruler(app: &App, area: &gtk::DrawingArea, cr: &cairo::Context, w: f64) {
    let off = voff(area);
    if off <= 0.0 {
        return;
    }
    let st = app.state.borrow();
    let Some(session) = st.session.as_ref() else {
        return;
    };
    let pal = Palette::of(st.dark);
    let canvas = canvas_rect(w, f64::from(area.height()));
    let left = f64::from(canvas.left());
    cr.save().ok();
    cr.translate(0.0, off);
    fill_rect(cr, pal.ruler_bg, 0.0, 0.0, w, f64::from(RULER_H));
    paint_ruler(cr, &pal, &st.view, canvas, w);
    let x = clip_x(canvas, &st.view, session.playhead);
    if x >= left - 8.0 && x <= w + 8.0 {
        line(cr, pal.playhead, 1.5, x, 0.0, x, f64::from(RULER_H));
        set(cr, pal.playhead, 1.0);
        cr.move_to(x - 6.0, 0.0);
        cr.line_to(x + 6.0, 0.0);
        cr.line_to(x, 11.0);
        cr.close_path();
        let _ = cr.fill();
    }
    fill_rect(cr, pal.ruler_bg, 0.0, 0.0, left, f64::from(RULER_H));
    line(
        cr,
        pal.tick_major,
        1.0,
        0.0,
        f64::from(RULER_H) - 0.5,
        w,
        f64::from(RULER_H) - 0.5,
    );
    cr.restore().ok();
}

fn paint_ruler(cr: &cairo::Context, pal: &Palette, view: &TimelineView, canvas: Rect, w: f64) {
    let left = f64::from(canvas.left());
    fill_rect(cr, pal.ruler_bg, left, 0.0, w - left, f64::from(RULER_H));
    let step = view::ruler_step_seconds(view.pixels_per_second);
    let visible = (w - left) / view.pixels_per_second;
    let first = (view.scroll_seconds / step).floor() * step;
    let minor = step / 5.0;
    let mut t = first;
    while t <= view.scroll_seconds + visible + step {
        let x = left + (t - view.scroll_seconds) * view.pixels_per_second;
        line(
            cr,
            pal.tick_major,
            1.0,
            x,
            f64::from(RULER_H) - 11.0,
            x,
            f64::from(RULER_H),
        );
        text(
            cr,
            &view::ruler_label(t.max(0.0), step),
            x + 4.0,
            16.0,
            11.0,
            pal.ruler_text,
            false,
        );
        if minor * view.pixels_per_second >= 8.0 {
            for k in 1..5 {
                let mx = x + minor * f64::from(k) * view.pixels_per_second;
                line(
                    cr,
                    pal.tick_minor,
                    1.0,
                    mx,
                    f64::from(RULER_H) - 5.0,
                    mx,
                    f64::from(RULER_H),
                );
            }
        }
        t += step;
    }
}

// ----- mouse -----------------------------------------------------------------------

fn attach_mouse(app: &Rc<App>, area: &gtk::DrawingArea) {
    let moved = Rc::new(Cell::new(false));
    let start = Rc::new(Cell::new((0.0f64, 0.0f64)));

    let drag = gtk::GestureDrag::builder().button(1).build();
    {
        let (app, area, moved, start) = (app.clone(), area.clone(), moved.clone(), start.clone());
        drag.connect_drag_begin(move |gesture, x, y| {
            let additive = gesture
                .current_event_state()
                .intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK);
            area.grab_focus();
            moved.set(false);
            start.set((x, y));
            if crate::range_select::active(&app) {
                // Picking a range to export: only its handles take the mouse.
                app.state.borrow_mut().drag = Drag::None;
                if x >= f64::from(HEADER_W) {
                    crate::range_select::press(&app, x);
                }
                return;
            }
            if x < f64::from(HEADER_W) {
                // Grabbing a track's name (not its buttons) starts moving the track.
                let canvas = canvas_rect(f64::from(area.width()), f64::from(area.height()));
                let mut st = app.state.borrow_mut();
                st.drag = Drag::None;
                let Some(session) = st.session.as_ref() else {
                    return;
                };
                let tl = &session.doc.timeline;
                let [(mx, _), _, _] = header_buttons();
                if x < mx - 2.0 {
                    if let Some(row) = row_at(canvas, ey(&area, y) as f32, tl.tracks.len()) {
                        let id = tl.tracks[row].id;
                        if session.track_move_range(id).is_some() {
                            st.track_drag = Some((id, row));
                        }
                    }
                }
                return;
            }
            let canvas = canvas_rect(f64::from(area.width()), f64::from(area.height()));
            let mut st = app.state.borrow_mut();
            let st = &mut *st;
            let tool = st.tool;
            let Some(session) = st.session.as_mut() else {
                return;
            };
            let pos = pos2(x as f32, ey(&area, y) as f32);
            let on_clip = matches!(
                hit_test(&session.doc.timeline, &st.view, canvas, pos),
                Hit::Clip { .. }
            );
            let on_cut = cut_handle_at(&session.doc.timeline, &st.view, canvas, pos).is_some();
            st.drag = if (tool == Tool::Split && on_clip) || on_cut {
                Drag::None
            } else if additive && on_clip {
                // Ctrl/Shift-click toggles; the click handler does it.
                Drag::None
            } else {
                let mut d = begin_drag(session, &st.view, canvas, pos);
                if let Drag::Marquee {
                    additive: a, base, ..
                } = &mut d
                {
                    *a = additive;
                    if additive {
                        *base = session.selection();
                    }
                }
                d
            };
            if session.playing {
                app.preview.borrow_mut().pause();
            }
            area.queue_draw();
        });
    }
    // The drag's body, run whenever the pointer moves and also while the view
    // scrolls under a pointer parked at an edge.
    let apply: Rc<dyn Fn(f64, f64)> = {
        let (app, area, start) = (app.clone(), area.clone(), start.clone());
        Rc::new(move |dx: f64, dy: f64| {
            let (sx, sy) = start.get();
            if crate::range_select::active(&app) {
                crate::range_select::drag_to(&app, sx + dx);
                return;
            }
            let pos = pos2((sx + dx) as f32, ey(&area, sy + dy) as f32);
            let canvas = canvas_rect(f64::from(area.width()), f64::from(area.height()));
            {
                let mut st = app.state.borrow_mut();
                let st = &mut *st;
                if let (Some((id, target)), Some(session)) =
                    (st.track_drag.as_mut(), st.session.as_ref())
                {
                    let rows = session.doc.timeline.tracks.len();
                    let pitch = ROW_H + ROW_GAP;
                    let rel = (pos.y - canvas.top() - RULER_H).max(0.0);
                    let row = ((rel / pitch).floor() as usize).min(rows.saturating_sub(1));
                    if let Some((low, high)) = session.track_move_range(*id) {
                        *target = row.clamp(low, high);
                    }
                    area.queue_draw();
                    return;
                }
            }
            let scrub = {
                let mut st = app.state.borrow_mut();
                let st = &mut *st;
                if matches!(st.drag, Drag::None) {
                    return;
                }
                let snap = st.snap;
                let threshold = st.view.ticks_for_pixels(SNAP_PX);
                let Some(session) = st.session.as_mut() else {
                    return;
                };
                let frame = Ticks(
                    session
                        .doc
                        .settings
                        .frame_rate
                        .ticks_per_frame()
                        .unwrap_or(TICKS_PER_SECOND / 30),
                );
                let playhead = session.playhead;
                st.snap_guide = update_drag(
                    session,
                    &st.view,
                    canvas,
                    &mut st.drag,
                    pos,
                    snap.then_some(playhead),
                    snap,
                    threshold,
                    frame,
                );
                matches!(st.drag, Drag::Scrub)
            };
            if scrub {
                app.sync_preview(true);
                crate::editor::on_tick(&app);
            }
            area.queue_draw();
        })
    };
    let last_offset = Rc::new(Cell::new((0.0f64, 0.0f64)));
    // How far the track list has scrolled under a pointer that has not moved.
    let v_extra = Rc::new(Cell::new(0.0f64));
    {
        let (moved, apply, last_offset, v_extra) = (
            moved.clone(),
            apply.clone(),
            last_offset.clone(),
            v_extra.clone(),
        );
        drag.connect_drag_update(move |_, dx, dy| {
            v_extra.set(0.0);
            if dx.abs() > 5.0 || dy.abs() > 5.0 {
                moved.set(true);
            }
            last_offset.set((dx, dy));
            apply(dx, dy);
        });
    }
    // Holding a drag (playhead, clip, trim, box) near either end of the
    // timeline scrolls it that way, faster the further out the pointer is.
    let edge_timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    {
        let (app, area, start, apply, last_offset, edge_timer, v_extra, moved) = (
            app.clone(),
            area.clone(),
            start.clone(),
            apply.clone(),
            last_offset.clone(),
            edge_timer.clone(),
            v_extra.clone(),
            moved.clone(),
        );
        drag.connect_drag_begin(move |_, _, _| {
            last_offset.set((0.0, 0.0));
            if let Some(old) = edge_timer.borrow_mut().take() {
                old.remove();
            }
            let (app, area, start, apply, last_offset) = (
                app.clone(),
                area.clone(),
                start.clone(),
                apply.clone(),
                last_offset.clone(),
            );
            v_extra.set(0.0);
            let (v_extra, moved) = (v_extra.clone(), moved.clone());
            let id = glib::timeout_add_local(Duration::from_millis(16), move || {
                let (sx, sy) = start.get();
                if sx < f64::from(HEADER_W) {
                    return glib::ControlFlow::Continue;
                }
                // A plain click on a clip near an edge must not scroll the
                // view or nudge the clip: only a real drag does (the
                // playhead may be held at an edge without moving).
                if !moved.get() && !matches!(app.state.borrow().drag, Drag::Scrub) {
                    return glib::ControlFlow::Continue;
                }
                let (dx, dy) = last_offset.get();
                let x = sx + dx;
                let (left, right) = (f64::from(HEADER_W), f64::from(area.width()));
                const EDGE: f64 = 48.0;
                let push = if x > right - EDGE {
                    x - (right - EDGE)
                } else if x < left + EDGE {
                    -((left + EDGE) - x)
                } else {
                    0.0
                };
                let mut redraw = false;
                if push != 0.0 {
                    let scrolled = {
                        let mut st = app.state.borrow_mut();
                        if matches!(st.drag, Drag::None) {
                            return glib::ControlFlow::Continue;
                        }
                        let pixels = (push * 0.5).clamp(-60.0, 60.0);
                        let pixels = if pixels.abs() < 2.0 {
                            pixels.signum() * 2.0
                        } else {
                            pixels
                        };
                        let before = st.view.scroll_seconds;
                        st.view.scroll_seconds =
                            (before + pixels / st.view.pixels_per_second).max(0.0);
                        st.view.scroll_seconds != before
                    };
                    redraw |= scrolled;
                }
                // Dragging a clip towards the bottom or top of the track list
                // scrolls the list that way.
                let vertical = matches!(
                    app.state.borrow().drag,
                    Drag::Move { .. } | Drag::Marquee { .. }
                );
                if vertical {
                    if let Some(sw) = area
                        .ancestor(gtk::ScrolledWindow::static_type())
                        .and_then(|w| w.downcast::<gtk::ScrolledWindow>().ok())
                    {
                        let vadj = sw.vadjustment();
                        let pointer = sy + dy + v_extra.get() - vadj.value();
                        const V_EDGE: f64 = 36.0;
                        let (low, high) = (V_EDGE + f64::from(RULER_H), vadj.page_size() - V_EDGE);
                        let vpush = if pointer > high {
                            pointer - high
                        } else if pointer < low {
                            -(low - pointer)
                        } else {
                            0.0
                        };
                        if vpush != 0.0 {
                            let step = (vpush * 0.5).clamp(-30.0, 30.0);
                            let step = if step.abs() < 2.0 {
                                step.signum() * 2.0
                            } else {
                                step
                            };
                            let before = vadj.value();
                            let target = (before + step).clamp(
                                vadj.lower(),
                                (vadj.upper() - vadj.page_size()).max(vadj.lower()),
                            );
                            if target != before {
                                vadj.set_value(target);
                                v_extra.set(v_extra.get() + (vadj.value() - before));
                                redraw = true;
                            }
                        }
                    }
                }
                if redraw {
                    apply(dx, dy + v_extra.get());
                }
                glib::ControlFlow::Continue
            });
            *edge_timer.borrow_mut() = Some(id);
        });
    }
    {
        let (app, moved, edge_timer) = (app.clone(), moved.clone(), edge_timer.clone());
        drag.connect_drag_end(move |_, _, _| {
            if let Some(id) = edge_timer.borrow_mut().take() {
                id.remove();
            }
            crate::range_select::release(&app);
            let track_drag = app.state.borrow_mut().track_drag.take();
            if let Some((track, target)) = track_drag {
                if moved.get() {
                    if let Some(s) = app.state.borrow_mut().session.as_mut() {
                        s.move_track(track, target);
                    }
                    app.sync_preview(true);
                }
                app.refresh();
                return;
            }
            let sound_changed;
            let had_drag = {
                let mut st = app.state.borrow_mut();
                let st = &mut *st;
                let had = !matches!(st.drag, Drag::None);
                sound_changed = matches!(st.drag, Drag::Volume { .. });
                if let Some(session) = st.session.as_mut() {
                    if moved.get() {
                        commit_drag(session, &st.drag);
                    } else if let Drag::Key { clip, from, .. } = &st.drag {
                        // A plain click on a diamond: go to it, so the inspector shows its values.
                        if let Some(start) =
                            session.doc.timeline.find_clip(*clip).map(|c| c.1.start)
                        {
                            session.set_playhead(start + *from);
                        }
                    }
                }
                st.drag = Drag::None;
                st.snap_guide = None;
                had
            };
            if had_drag {
                if sound_changed {
                    // A new sound level should be heard at once.
                    app.sync_preview(false);
                }
                app.refresh();
            }
        });
    }
    area.add_controller(drag);

    // Right click on a clip: select it and open its menu.
    let context = gtk::GestureClick::builder().button(3).build();
    {
        let (app, area) = (app.clone(), area.clone());
        context.connect_pressed(move |_, _, x, y| {
            if crate::range_select::active(&app) {
                return;
            }
            let canvas = canvas_rect(f64::from(area.width()), f64::from(area.height()));
            if x < f64::from(HEADER_W) {
                let track = {
                    let st = app.state.borrow();
                    st.session.as_ref().and_then(|s| {
                        let tl = &s.doc.timeline;
                        row_at(canvas, ey(&area, y) as f32, tl.tracks.len())
                            .map(|r| tl.tracks[r].id)
                    })
                };
                if let Some(track) = track {
                    track_menu(&app, area.upcast_ref(), track, x, y);
                }
                return;
            }
            let hit = {
                let mut st = app.state.borrow_mut();
                let st = &mut *st;
                let view = st.view;
                let Some(session) = st.session.as_mut() else {
                    return;
                };
                match hit_test(
                    &session.doc.timeline,
                    &view,
                    canvas,
                    pos2(x as f32, ey(&area, y) as f32),
                ) {
                    Hit::Clip { id, .. } => {
                        if !session.is_selected(id) {
                            session.select_only(id);
                        }
                        Some(id)
                    }
                    _ => None,
                }
            };
            if let Some(clip) = hit {
                app.refresh();
                crate::clip_menu::show(&app, area.upcast_ref(), clip, x, y);
            }
        });
    }
    area.add_controller(context);

    let click = gtk::GestureClick::builder().button(1).build();
    {
        let (app, area, moved) = (app.clone(), area.clone(), moved.clone());
        click.connect_released(move |gesture, n_press, x, y| {
            let additive = gesture
                .current_event_state()
                .intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK);
            if crate::range_select::active(&app) {
                crate::range_select::release(&app);
                if !moved.get() {
                    crate::range_select::click(&app, x, y, f64::from(area.width()), voff(&area));
                }
                return;
            }
            if moved.get() {
                return;
            }
            let canvas = canvas_rect(f64::from(area.width()), f64::from(area.height()));
            let pos = pos2(x as f32, ey(&area, y) as f32);
            // The box on a cut opens its crossfade settings.
            let cut = {
                let st = app.state.borrow();
                st.session.as_ref().and_then(|s| {
                    (x >= f64::from(HEADER_W))
                        .then(|| cut_handle_at(&s.doc.timeline, &st.view, canvas, pos))
                        .flatten()
                })
            };
            if let Some(clip) = cut {
                crate::crossfade_ui::show(&app, area.upcast_ref(), clip, x, y);
                return;
            }
            {
                let mut st = app.state.borrow_mut();
                let st = &mut *st;
                let (tool, view_copy) = (st.tool, st.view);
                let Some(session) = st.session.as_mut() else {
                    return;
                };
                let rate = session.doc.settings.frame_rate;
                if x < f64::from(HEADER_W) {
                    if let Some(row) = row_at(canvas, pos.y, session.doc.timeline.tracks.len()) {
                        let id = session.doc.timeline.tracks[row].id;
                        let [(mx, mw), (hx, hw), (lx, lw)] = header_buttons();
                        if n_press >= 2 && x < mx - 2.0 {
                            let app = app.clone();
                            glib::idle_add_local_once(move || rename_track(&app, id));
                        } else if x >= mx && x <= mx + mw {
                            session.toggle_track_muted(id);
                        } else if x >= hx && x <= hx + hw {
                            session.toggle_track_hidden(id);
                        } else if x >= lx && x <= lx + lw {
                            session.toggle_track_locked(id);
                        }
                    }
                } else {
                    match hit_test(&session.doc.timeline, &view_copy, canvas, pos) {
                        Hit::Ruler => {
                            session.playing = false;
                            let t = view::quantize_to_frame(
                                view_copy.time_for(pos.x - canvas.left()),
                                rate,
                            );
                            session.set_playhead(t);
                        }
                        Hit::Clip { id, .. } => {
                            if additive && tool != Tool::Split {
                                session.toggle_selected(id);
                            } else {
                                session.select_only(id);
                            }
                            // A click anywhere on the timeline also moves the playhead there.
                            if tool != Tool::Split {
                                session.playing = false;
                                session.set_playhead(view::quantize_to_frame(
                                    view_copy.time_for(pos.x - canvas.left()),
                                    rate,
                                ));
                            }
                            if tool == Tool::Split {
                                let at = view::quantize_to_frame(
                                    view_copy.time_for(pos.x - canvas.left()),
                                    rate,
                                );
                                session.split_clip_at(id, at);
                            }
                        }
                        Hit::Empty => {
                            if !additive {
                                session.clear_selection();
                            }
                            session.playing = false;
                            session.set_playhead(view::quantize_to_frame(
                                view_copy.time_for(pos.x - canvas.left()),
                                rate,
                            ));
                        }
                    }
                }
            }
            app.refresh();
        });
    }
    area.add_controller(click);
}

fn attach_scroll_and_motion(app: &Rc<App>, area: &gtk::DrawingArea) {
    // Smooth (not stepped) scrolling, so a touchpad's two-finger swipe and
    // pinch-as-ctrl-scroll arrive as fine, proportional amounts.
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    {
        let app = app.clone();
        scroll.connect_scroll(move |controller, dx, dy| {
            let wheel = controller.unit() == gdk::ScrollUnit::Wheel;
            let state = controller.current_event_state();
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            if !(ctrl || shift || dx != 0.0) {
                return glib::Propagation::Proceed; // let the track list scroll vertically
            }
            {
                let mut st = app.state.borrow_mut();
                if ctrl {
                    let anchor = st.hover.map_or(0.0, |p| p.x - HEADER_W);
                    let factor = if wheel {
                        if dy < 0.0 {
                            1.2
                        } else {
                            1.0 / 1.2
                        }
                    } else {
                        (-dy * 0.02).exp()
                    };
                    st.view.zoom_at(factor, anchor);
                } else {
                    let amount = if dx != 0.0 { dx } else { dy };
                    let pixels = if wheel { amount * 60.0 } else { amount * 8.0 };
                    st.view.pan_pixels(pixels as f32);
                }
            }
            queue_draw(&app);
            glib::Propagation::Stop
        });
    }
    area.add_controller(scroll);

    // Pinching in or out on a touchpad or touch screen zooms the time scale
    // around the middle of the pinch.
    let pinch = gtk::GestureZoom::new();
    pinch.set_propagation_phase(gtk::PropagationPhase::Capture);
    let started = Rc::new(Cell::new((1.0f64, 0.0f32)));
    {
        let (app, started) = (app.clone(), started.clone());
        pinch.connect_begin(move |g, _| {
            let centre = g
                .bounding_box_center()
                .map_or(0.0, |(x, _)| x as f32 - HEADER_W);
            started.set((app.state.borrow().view.pixels_per_second, centre));
        });
    }
    {
        let app = app.clone();
        pinch.connect_scale_changed(move |_, scale| {
            let (base, anchor) = started.get();
            {
                let mut st = app.state.borrow_mut();
                let factor = base * scale / st.view.pixels_per_second;
                st.view.zoom_at(factor, anchor.max(0.0));
            }
            queue_draw(&app);
        });
    }
    area.add_controller(pinch);

    let motion = gtk::EventControllerMotion::new();
    {
        let (app, area) = (app.clone(), area.clone());
        motion.connect_motion(move |_, x, y| {
            let canvas = canvas_rect(f64::from(area.width()), f64::from(area.height()));
            let pos = pos2(x as f32, ey(&area, y) as f32);
            let cursor = {
                let mut st = app.state.borrow_mut();
                st.hover = Some(pos);
                st.timeline_width = f64::from(area.width()) - f64::from(HEADER_W);
                let Some(session) = st.session.as_ref() else {
                    return;
                };
                if x < f64::from(HEADER_W) || !matches!(st.drag, Drag::None) {
                    None
                } else {
                    match hit_test(&session.doc.timeline, &st.view, canvas, pos) {
                        Hit::Clip {
                            zone: Zone::Left | Zone::Right,
                            ..
                        } if st.tool == Tool::Select => Some("col-resize"),
                        Hit::Clip { .. } if st.tool == Tool::Split => Some("crosshair"),
                        _ => None,
                    }
                }
            };
            area.set_cursor_from_name(cursor);
            if app.state.borrow().tool == Tool::Split {
                area.queue_draw();
            }
        });
    }
    {
        let (app, area) = (app.clone(), area.clone());
        motion.connect_leave(move |_| {
            app.state.borrow_mut().hover = None;
            area.queue_draw();
        });
    }
    area.add_controller(motion);
}

// ----- dropping media and files -------------------------------------------------------

fn attach_drop(app: &Rc<App>, area: &gtk::DrawingArea) {
    let target = gtk::DropTarget::new(glib::Type::INVALID, gdk::DragAction::COPY);
    target.set_types(&[glib::Type::STRING, gdk::FileList::static_type()]);
    {
        let (app, area) = (app.clone(), area.clone());
        target.connect_motion(move |_, x, y| {
            let canvas = canvas_rect(f64::from(area.width()), f64::from(area.height()));
            {
                let mut st = app.state.borrow_mut();
                let st = &mut *st;
                st.drop_ghost = None;
                if let (Some(media), Some(session)) = (st.dragging_media, st.session.as_ref()) {
                    let threshold = st.view.ticks_for_pixels(SNAP_PX);
                    st.drop_ghost = drop_target(
                        session,
                        &st.view,
                        canvas,
                        pos2(x as f32, ey(&area, y) as f32),
                        media,
                        st.snap,
                        threshold,
                    );
                }
            }
            area.queue_draw();
            gdk::DragAction::COPY
        });
    }
    {
        let (app, area) = (app.clone(), area.clone());
        target.connect_leave(move |_| {
            app.state.borrow_mut().drop_ghost = None;
            area.queue_draw();
        });
    }
    {
        let (app, area) = (app.clone(), area.clone());
        target.connect_drop(move |_, value, x, y| {
            let canvas = canvas_rect(f64::from(area.width()), f64::from(area.height()));
            let pos = pos2(x as f32, ey(&area, y) as f32);
            app.state.borrow_mut().drop_ghost = None;

            // Media dragged from the library.
            if let Ok(text) = value.get::<String>() {
                let Some(id) = text
                    .strip_prefix("cutroom-media:")
                    .and_then(|s| s.parse::<u64>().ok())
                else {
                    return false;
                };
                drop_library_media(&app, MediaId(id), canvas, pos);
                return true;
            }

            // Files dragged in from the file manager.
            if let Ok(list) = value.get::<gdk::FileList>() {
                let files: Vec<PathBuf> = list.files().iter().filter_map(|f| f.path()).collect();
                if files.is_empty() {
                    return false;
                }
                let place_at = {
                    let st = app.state.borrow();
                    st.session.as_ref().and_then(|s| {
                        let row = row_at(canvas, pos.y, s.doc.timeline.tracks.len())?;
                        let rate = s.doc.settings.frame_rate;
                        let start =
                            view::quantize_to_frame(st.view.time_for(pos.x - canvas.left()), rate);
                        Some((s.doc.timeline.tracks[row].id, start))
                    })
                };
                app.handle_timeline_file_drop(files, place_at);
                return true;
            }
            false
        });
    }
    area.add_controller(target);
}

fn drop_library_media(app: &Rc<App>, media: MediaId, canvas: Rect, pos: Pos2) {
    let mut replace_with_picker = None;
    let mut replaced = false;
    {
        let mut st = app.state.borrow_mut();
        let st = &mut *st;
        let threshold = st.view.ticks_for_pixels(SNAP_PX);
        let snap = st.snap;
        let Some(session) = st.session.as_mut() else {
            return;
        };
        let target = drop_target(session, &st.view, canvas, pos, media, snap, threshold);
        if let Some((clip, ok)) = target.as_ref().and_then(|t| t.replace.clone()) {
            st.dragging_media = None;
            match ok {
                Err(why) => session.status = Some(why),
                Ok(()) if session.media_kind_of(media) == MediaKind::Image => {
                    session.replace_clip_media(clip, media, Ticks::ZERO);
                }
                Ok(()) => replace_with_picker = Some(clip),
            }
            replaced = true;
        } else {
            match target {
                Some(target) if target.valid => match target.new_track {
                    Some(at) => {
                        match cutroom_core::timeline_logic::new_track_insert_index(
                            &session.doc.timeline,
                            target.new_kind,
                            at,
                        ) {
                            Some(index) => {
                                session.add_media_on_new_track_at(
                                    media,
                                    target.new_kind,
                                    index,
                                    target.start,
                                );
                            }
                            None => session.status = Some("A new track can't go there".into()),
                        }
                    }
                    None => {
                        session.add_media_at(media, target.track, target.start);
                    }
                },
                Some(target) if target.wrong_kind => {
                    session.status = Some(match session.media_kind_of(media).track_kind() {
                        TrackKind::Audio => "Audio goes on an audio track".into(),
                        TrackKind::Video if session.media_kind_of(media) == MediaKind::Video => {
                            "This video has no sound, so it can't go on an audio track".into()
                        }
                        TrackKind::Video => "Pictures go on a video track".into(),
                    });
                }
                Some(_) => {
                    session.status = Some("Clips can't overlap, or the track is locked".into())
                }
                None => session.status = Some("Drop media onto a track".into()),
            }
        }
        st.dragging_media = None;
    }
    if let Some(clip) = replace_with_picker {
        let _ = crate::replace_ui::open(app, clip, media);
    }
    if replaced {
        app.sync_preview(true);
    }
    app.refresh();
}
