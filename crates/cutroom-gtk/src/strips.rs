//! Waveforms and filmstrips for timeline clips: found in the background after
//! a file is probed, then drawn behind the clip labels.

use crate::analysis;
use crate::app::App;
use crate::media::Thumb;
use crate::timeline;
use cutroom_core::timeline_logic::ROW_H;
use cutroom_core::waveform::Peaks;
use cutroom_timeline::MediaId;
use gtk4::{cairo, gio, glib};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Height of a filmstrip frame: the inside of a clip.
pub fn strip_height() -> i32 {
    ROW_H as i32 - 4
}

pub struct Filmstrip {
    pub frame_w: f64,
    /// When each frame was taken (seconds into the source), ascending.
    pub times: Vec<f64>,
    pub frames: Vec<cairo::ImageSurface>,
}

impl Filmstrip {
    /// The frame to show for source time `t`: the latest one taken at or before it.
    pub fn frame_for(&self, t: f64) -> Option<&cairo::ImageSurface> {
        self.frames.get(frame_index(&self.times, t))
    }
}

/// Index of the last time that is `<= t` (0 when `t` is before all of them).
pub fn frame_index(times: &[f64], t: f64) -> usize {
    times.partition_point(|&x| x <= t).saturating_sub(1)
}

#[derive(Default)]
pub struct Analysis {
    peaks: HashMap<MediaId, Rc<Peaks>>,
    strips: HashMap<MediaId, Rc<Filmstrip>>,
    asked_peaks: HashSet<MediaId>,
    asked_strips: HashSet<MediaId>,
}

impl Analysis {
    pub fn peaks(&self, id: MediaId) -> Option<Rc<Peaks>> {
        self.peaks.get(&id).cloned()
    }

    pub fn strip(&self, id: MediaId) -> Option<Rc<Filmstrip>> {
        self.strips.get(&id).cloned()
    }

    /// Drop everything known about one media (its file changed or went away).
    pub fn forget(&mut self, id: MediaId) {
        self.peaks.remove(&id);
        self.strips.remove(&id);
        self.asked_peaks.remove(&id);
        self.asked_strips.remove(&id);
    }
}

/// RGBA pixels to a Cairo surface (premultiplied BGRA in memory).
pub fn thumb_to_surface(thumb: Thumb) -> Option<cairo::ImageSurface> {
    let (w, h) = (thumb.width, thumb.height);
    let stride = cairo::Format::ARgb32.stride_for_width(w as u32).ok()?;
    let mut data = vec![0u8; stride as usize * h as usize];
    for y in 0..h as usize {
        for x in 0..w as usize {
            let src = (y * w as usize + x) * 4;
            let dst = y * stride as usize + x * 4;
            let a = u32::from(thumb.rgba[src + 3]);
            let premultiply = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
            data[dst] = premultiply(thumb.rgba[src + 2]);
            data[dst + 1] = premultiply(thumb.rgba[src + 1]);
            data[dst + 2] = premultiply(thumb.rgba[src]);
            data[dst + 3] = a as u8;
        }
    }
    cairo::ImageSurface::create_for_data(data, cairo::Format::ARgb32, w, h, stride).ok()
}

/// Peaks from the cache file, or decoded now (and cached for next time).
fn load_or_compute_peaks(path: &Path, cache: Option<&Path>) -> Option<Peaks> {
    if let Some(cache) = cache {
        if let Some(peaks) = std::fs::read(cache).ok().and_then(|b| Peaks::decode(&b)) {
            return Some(peaks);
        }
    }
    let peaks = analysis::compute_peaks(path).ok()?;
    if let Some(cache) = cache {
        if let Some(dir) = cache.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let temp = cache.with_extension("tmp");
        if std::fs::write(&temp, peaks.encode()).is_ok() {
            let _ = std::fs::rename(&temp, cache);
        }
    }
    Some(peaks)
}

fn same_project(app: &App, root: &Path) -> bool {
    app.state
        .borrow()
        .session
        .as_ref()
        .is_some_and(|s| s.folder.root() == root)
}

struct Job {
    root: PathBuf,
    path: PathBuf,
    cache: Option<PathBuf>,
    has_audio: bool,
    is_video: bool,
    duration: cutroom_timeline::Ticks,
}

fn job_for(app: &App, id: MediaId) -> Option<Job> {
    let st = app.state.borrow();
    let session = st.session.as_ref()?;
    let item = session.doc.media_item(id).ok()?;
    let probe = item.probe.as_ref()?;
    let path = session.media_path(id).filter(|p| p.is_file())?;
    let kind = session.media_kind_of(id);
    Some(Job {
        root: session.folder.root().to_path_buf(),
        path,
        cache: session.waveform_cache_path(id),
        has_audio: probe.has_audio && kind != cutroom_core::state::MediaKind::Image,
        is_video: kind == cutroom_core::state::MediaKind::Video && probe.has_video,
        duration: session.media_duration(id),
    })
}

/// Start whatever is still missing for this media (once each).
pub fn request(app: &Rc<App>, id: MediaId) {
    let Some(job) = job_for(app, id) else { return };
    let (want_peaks, want_strip) = {
        let mut analysis = app.analysis.borrow_mut();
        (
            job.has_audio && analysis.asked_peaks.insert(id),
            job.is_video && job.duration.0 > 0 && analysis.asked_strips.insert(id),
        )
    };
    if want_peaks {
        let (app, path, cache, root) = (
            app.clone(),
            job.path.clone(),
            job.cache.clone(),
            job.root.clone(),
        );
        glib::spawn_future_local(async move {
            let peaks = gio::spawn_blocking(move || load_or_compute_peaks(&path, cache.as_deref()))
                .await
                .ok()
                .flatten();
            if let Some(peaks) = peaks {
                if same_project(&app, &root) {
                    app.analysis.borrow_mut().peaks.insert(id, Rc::new(peaks));
                    timeline::queue_draw(&app);
                }
            }
        });
    }
    if want_strip {
        let (app, path, root, duration) = (
            app.clone(),
            job.path.clone(),
            job.root.clone(),
            job.duration,
        );
        glib::spawn_future_local(async move {
            let frames = gio::spawn_blocking(move || {
                let times = analysis::filmstrip_times(duration, 80);
                analysis::filmstrip_frames(&path, &times, strip_height())
            })
            .await
            .unwrap_or_default();
            if frames.is_empty() || !same_project(&app, &root) {
                return;
            }
            let mut strip = Filmstrip {
                frame_w: f64::from(frames[0].1.width),
                times: Vec::new(),
                frames: Vec::new(),
            };
            for (time, thumb) in frames {
                if let Some(surface) = thumb_to_surface(thumb) {
                    strip.times.push(time.as_seconds_f64());
                    strip.frames.push(surface);
                }
            }
            if !strip.frames.is_empty() {
                app.analysis.borrow_mut().strips.insert(id, Rc::new(strip));
                timeline::queue_draw(&app);
            }
        });
    }
}

/// Ask for everything for every probed media in the open project.
pub fn request_all(app: &Rc<App>) {
    let ids: Vec<MediaId> = app
        .state
        .borrow()
        .session
        .as_ref()
        .map(|s| s.doc.media.iter().map(|m| m.id).collect())
        .unwrap_or_default();
    for id in ids {
        request(app, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_shown_is_the_latest_one_at_or_before_the_time() {
        let times = [0.0, 1.0, 2.0, 3.0];
        assert_eq!(frame_index(&times, -1.0), 0);
        assert_eq!(frame_index(&times, 0.0), 0);
        assert_eq!(frame_index(&times, 0.99), 0);
        assert_eq!(frame_index(&times, 1.0), 1);
        assert_eq!(frame_index(&times, 2.5), 2);
        assert_eq!(frame_index(&times, 99.0), 3);
        assert_eq!(frame_index(&[], 1.0), 0);
    }

    #[test]
    fn pixels_become_premultiplied_bgra() {
        let thumb = Thumb {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 128],
        };
        let mut surface = thumb_to_surface(thumb).unwrap();
        surface.flush();
        let data = surface.data().unwrap();
        assert_eq!(
            &data[0..4],
            &[0, 0, 255, 255],
            "opaque red is B=0 G=0 R=255 A=255"
        );
        assert_eq!(
            &data[4..8],
            &[0, 128, 0, 128],
            "half-transparent green is premultiplied"
        );
    }

    #[test]
    fn a_strip_picks_frames_by_source_time() {
        let surface = |_: i32| cairo::ImageSurface::create(cairo::Format::ARgb32, 4, 4).unwrap();
        let strip = Filmstrip {
            frame_w: 4.0,
            times: vec![0.0, 2.0],
            frames: vec![surface(0), surface(1)],
        };
        assert!(strip.frame_for(1.9).is_some());
        let empty = Filmstrip {
            frame_w: 4.0,
            times: vec![],
            frames: vec![],
        };
        assert!(empty.frame_for(1.0).is_none());
    }

    #[test]
    fn peaks_are_cached_and_read_back_without_decoding() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache").join("x.peaks");
        let peaks = Peaks {
            per_second: 100,
            data: vec![1, 2, 3],
        };
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        std::fs::write(&cache, peaks.encode()).unwrap();
        // The media path does not exist: only the cache can answer.
        let loaded = load_or_compute_peaks(&tmp.path().join("missing.wav"), Some(&cache));
        assert_eq!(loaded, Some(peaks));
        std::fs::write(&cache, b"damaged").unwrap();
        assert_eq!(
            load_or_compute_peaks(&tmp.path().join("missing.wav"), Some(&cache)),
            None,
            "a damaged cache is not trusted"
        );
    }
}
