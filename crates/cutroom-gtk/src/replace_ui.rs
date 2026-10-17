//! Replace a clip with part of another file. Dropping library media onto a
//! clip opens this window when the file is long enough: slide the highlighted
//! window (as long as the clip) along the file to choose the part to use,
//! watch it in the preview, then Replace. The clip keeps its place, length,
//! look and keyframes.

use crate::app::App;
use crate::dialogs::{buttons, caption, content_box, dialog_window};
use cutroom_timeline::{ClipId, MediaId, Ticks};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::glib;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const PREVIEW_W: i32 = 480;
const PREVIEW_H: i32 = 270;
const BAR_H: i32 = 56;

/// Where the chosen part may start: from 0 to `length - clip`.
pub fn clamp_start(start: f64, clip: f64, length: f64) -> f64 {
    start.clamp(0.0, (length - clip).max(0.0))
}

/// The start a drag on the bar gives: the window keeps where it was grabbed.
pub fn start_from_bar(x: f64, grab: f64, bar_width: f64, clip: f64, length: f64) -> f64 {
    let seconds_per_px = length / bar_width.max(1.0);
    clamp_start((x - grab) * seconds_per_px, clip, length)
}

/// A paused pipeline that hands back the picture at any time of a file, and
/// can play a stretch of it.
struct Grabber {
    playbin: gst::Element,
    sink: gst_app::AppSink,
}

impl Grabber {
    fn new(uri: &str) -> Option<Grabber> {
        let playbin = gst::ElementFactory::make("playbin")
            .property("uri", uri)
            .build()
            .ok()?;
        let bin = gst::parse::bin_from_description(
            &format!(
                "videoconvert ! videoscale ! video/x-raw,format=RGBA,width=[1,{PREVIEW_W}],height=[1,{PREVIEW_H}],pixel-aspect-ratio=1/1 \
                 ! appsink name=sink sync=true max-buffers=2 drop=true"
            ),
            true,
        )
        .ok()?;
        let sink = bin.by_name("sink")?.downcast::<gst_app::AppSink>().ok()?;
        playbin.set_property("video-sink", &bin);
        playbin.set_state(gst::State::Paused).ok()?;
        let _ = playbin.state(gst::ClockTime::from_seconds(5));
        Some(Grabber { playbin, sink })
    }

    /// The picture at `seconds` (paused).
    fn frame_at(&self, seconds: f64) -> Option<gtk::gdk::Texture> {
        let _ = self.playbin.set_state(gst::State::Paused);
        let at = gst::ClockTime::from_nseconds((seconds.max(0.0) * 1e9) as u64);
        self.playbin
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE, at)
            .ok()?;
        let sample = self.sink.try_pull_preroll(gst::ClockTime::from_mseconds(1500))?;
        crate::media::sample_to_thumb(&sample).map(crate::thumbs::texture_from)
    }

    fn play_from(&self, seconds: f64) {
        let at = gst::ClockTime::from_nseconds((seconds.max(0.0) * 1e9) as u64);
        let _ = self
            .playbin
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE, at);
        let _ = self.playbin.set_state(gst::State::Playing);
    }

    fn pause(&self) {
        let _ = self.playbin.set_state(gst::State::Paused);
    }

    fn position(&self) -> Option<f64> {
        self.playbin
            .query_position::<gst::ClockTime>()
            .map(|p| p.nseconds() as f64 / 1e9)
    }

    /// The newest picture while playing, if one came.
    fn latest(&self) -> Option<gtk::gdk::Texture> {
        let sample = self.sink.try_pull_sample(gst::ClockTime::ZERO)?;
        crate::media::sample_to_thumb(&sample).map(crate::thumbs::texture_from)
    }
}

impl Drop for Grabber {
    fn drop(&mut self) {
        let _ = self.playbin.set_state(gst::State::Null);
    }
}

fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0);
    format!("{:02}:{:05.2}", (s / 60.0).floor() as u64, s % 60.0)
}

/// Open the window for replacing `clip` with part of `media`.
/// Returns the window and its Replace button (tests press it).
pub fn open(app: &Rc<App>, clip: ClipId, media: MediaId) -> Option<(gtk::Window, gtk::Button)> {
    let facts = {
        let st = app.state.borrow();
        st.session.as_ref().and_then(|s| {
            let (_, c) = s.doc.timeline.find_clip(clip)?;
            let length = s.known_media_duration(media)?.as_seconds_f64();
            let path = s.media_path(media)?;
            let name = s
                .doc
                .media_item(media)
                .map(cutroom_core::state::media_label)
                .unwrap_or_default();
            Some((c.duration.as_seconds_f64(), length, path, name))
        })
    };
    let (clip_len, length, path, name) = facts?;
    crate::strips::request(app, media);

    let window = dialog_window(app, "Replace clip", 640);
    let body = content_box();
    let intro = caption(&format!(
        "Slide the highlighted part along \"{name}\" to choose the {clip_len:.2} s that replace the clip. Its place, length, look and keyframes stay."
    ));
    intro.set_wrap(true);
    intro.set_max_width_chars(70);
    body.append(&intro);

    let picture = gtk::Picture::new();
    picture.set_size_request(PREVIEW_W, PREVIEW_H);
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.add_css_class("preview-canvas");
    body.append(&picture);

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let play = gtk::Button::with_label("Play this part");
    let span = gtk::Label::new(None);
    span.set_hexpand(true);
    span.set_xalign(1.0);
    span.add_css_class("timecode");
    controls.append(&play);
    controls.append(&span);
    body.append(&controls);

    let bar = gtk::DrawingArea::new();
    bar.set_content_height(BAR_H);
    bar.set_hexpand(true);
    bar.set_tooltip_text(Some("Drag the highlighted part, or click where it should be"));
    body.append(&bar);

    let cancel = gtk::Button::with_label("Cancel");
    let replace = gtk::Button::with_label("Replace");
    replace.add_css_class("suggested-action");
    body.append(&buttons(&replace, &cancel));
    window.set_child(Some(&body));

    let start = Rc::new(Cell::new(0.0f64));
    let grabber = Rc::new(Grabber::new(&crate::media::path_to_uri(&path)));
    let playing: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));

    let stop_playing = {
        let (playing, grabber, play) = (playing.clone(), grabber.clone(), play.clone());
        Rc::new(move || {
            if let Some(id) = playing.borrow_mut().take() {
                id.remove();
            }
            if let Some(g) = grabber.as_ref() {
                g.pause();
            }
            play.set_label("Play this part");
        })
    };
    let show_frame = {
        let (grabber, picture, start, span) =
            (grabber.clone(), picture.clone(), start.clone(), span.clone());
        Rc::new(move || {
            let s = start.get();
            span.set_text(&format!(
                "{} - {} of {}",
                clock(s),
                clock(s + clip_len),
                clock(length)
            ));
            if let Some(texture) = grabber.as_ref().as_ref().and_then(|g| g.frame_at(s)) {
                picture.set_paintable(Some(&texture));
            }
        })
    };
    // Fetching a picture waits for the decoder, so a drag asks at most every 80 ms.
    let pending: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let request_frame = {
        let (pending, show_frame) = (pending.clone(), show_frame.clone());
        Rc::new(move || {
            if pending.replace(true) {
                return;
            }
            let (pending, show_frame) = (pending.clone(), show_frame.clone());
            glib::timeout_add_local_once(Duration::from_millis(80), move || {
                pending.set(false);
                show_frame();
            });
        })
    };

    {
        let (app, start) = (app.clone(), start.clone());
        bar.set_draw_func(move |_, cr, w, h| {
            let (w, h) = (f64::from(w), f64::from(h));
            cr.set_source_rgb(0.12, 0.12, 0.14);
            cr.rectangle(0.0, 0.0, w, h);
            let _ = cr.fill();
            // The file as a filmstrip, when it has been made.
            if let Some(strip) = app.analysis.borrow().strip(media) {
                if let Some(first) = strip.frames.first() {
                    let scale = h / f64::from(first.height().max(1));
                    let tile = (f64::from(first.width()) * scale).max(8.0);
                    let mut x = 0.0;
                    while x < w {
                        let t = x / w * length;
                        if let Some(surface) = strip.frame_for(t) {
                            cr.save().ok();
                            cr.rectangle(x, 0.0, tile, h);
                            cr.clip();
                            cr.translate(x, 0.0);
                            cr.scale(scale, scale);
                            let _ = cr.set_source_surface(surface, 0.0, 0.0);
                            let _ = cr.paint();
                            cr.restore().ok();
                        }
                        x += tile;
                    }
                }
            }
            let k = w / length.max(0.001);
            let (x0, x1) = (start.get() * k, (start.get() + clip_len) * k);
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.6);
            cr.rectangle(0.0, 0.0, x0, h);
            cr.rectangle(x1, 0.0, w - x1, h);
            let _ = cr.fill();
            let a = crate::theme::ACCENT;
            cr.set_source_rgb(a.0, a.1, a.2);
            cr.set_line_width(3.0);
            cr.rectangle(x0 + 1.5, 1.5, (x1 - x0 - 3.0).max(1.0), h - 3.0);
            let _ = cr.stroke();
        });
    }

    let drag = gtk::GestureDrag::new();
    let grab = Rc::new(Cell::new((0.0f64, 0.0f64)));
    {
        let (bar, start, grab, request_frame, stop_playing) = (
            bar.clone(),
            start.clone(),
            grab.clone(),
            request_frame.clone(),
            stop_playing.clone(),
        );
        drag.connect_drag_begin(move |_, x, _| {
            stop_playing();
            let w = f64::from(bar.width()).max(1.0);
            let k = w / length.max(0.001);
            let (x0, x1) = (start.get() * k, (start.get() + clip_len) * k);
            let offset = if x >= x0 && x <= x1 {
                x - x0
            } else {
                // A click outside: the part centres there.
                (x1 - x0) / 2.0
            };
            grab.set((x, offset));
            start.set(start_from_bar(x, offset, w, clip_len, length));
            bar.queue_draw();
            request_frame();
        });
    }
    {
        let (bar, start, grab, request_frame) =
            (bar.clone(), start.clone(), grab.clone(), request_frame.clone());
        drag.connect_drag_update(move |_, dx, _| {
            let (x, offset) = grab.get();
            let w = f64::from(bar.width()).max(1.0);
            start.set(start_from_bar(x + dx, offset, w, clip_len, length));
            bar.queue_draw();
            request_frame();
        });
    }
    {
        let show_frame = show_frame.clone();
        drag.connect_drag_end(move |_, _, _| show_frame());
    }
    bar.add_controller(drag);

    {
        let (grabber, playing, picture, start, stop_playing) = (
            grabber.clone(),
            playing.clone(),
            picture.clone(),
            start.clone(),
            stop_playing.clone(),
        );
        play.connect_clicked(move |button| {
            if playing.borrow().is_some() {
                stop_playing();
                return;
            }
            let Some(g) = grabber.as_ref() else { return };
            let (from, to) = (start.get(), start.get() + clip_len);
            g.play_from(from);
            button.set_label("Stop");
            let (grabber, picture, stop_playing) =
                (grabber.clone(), picture.clone(), stop_playing.clone());
            let id = glib::timeout_add_local(Duration::from_millis(30), move || {
                let Some(g) = grabber.as_ref() else {
                    return glib::ControlFlow::Break;
                };
                if let Some(texture) = g.latest() {
                    picture.set_paintable(Some(&texture));
                }
                if g.position().is_some_and(|p| p >= to) {
                    // Let the stop below remove this timer.
                    let stop_playing = stop_playing.clone();
                    glib::idle_add_local_once(move || stop_playing());
                }
                glib::ControlFlow::Continue
            });
            *playing.borrow_mut() = Some(id);
        });
    }
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    {
        let (app, window, start) = (app.clone(), window.clone(), start.clone());
        replace.connect_clicked(move |_| {
            let at = Ticks::from_seconds_f64(start.get());
            if let Some(s) = app.state.borrow_mut().session.as_mut() {
                s.replace_clip_media(clip, media, at);
            }
            window.close();
            app.sync_preview(true);
            app.refresh();
        });
    }
    {
        let (stop_playing, grabber) = (stop_playing.clone(), grabber.clone());
        window.connect_close_request(move |_| {
            stop_playing();
            // Let go of the decoder now, not when the last closure goes.
            if let Some(g) = grabber.as_ref() {
                let _ = g.playbin.set_state(gst::State::Null);
            }
            glib::Propagation::Proceed
        });
    }
    if grabber.is_none() {
        play.set_sensitive(false);
        span.set_text("The file could not be opened for a preview");
    }
    window.present();
    show_frame();
    Some((window, replace))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chosen_part_stays_inside_the_file() {
        assert_eq!(clamp_start(-3.0, 5.0, 20.0), 0.0);
        assert_eq!(clamp_start(18.0, 5.0, 20.0), 15.0);
        assert_eq!(clamp_start(4.0, 5.0, 20.0), 4.0);
        // A bar 200 px wide for a 20 s file: 10 px a second.
        assert_eq!(start_from_bar(100.0, 20.0, 200.0, 5.0, 20.0), 8.0);
        assert_eq!(start_from_bar(500.0, 0.0, 200.0, 5.0, 20.0), 15.0);
    }
}
