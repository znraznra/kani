//! Thumbnails for the media library, made on worker threads.

use crate::app::App;
use crate::editor;
use crate::media::{self, Thumb};
use cutroom_core::state::MediaKind;
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

enum State {
    Loading,
    Ready(gdk::Texture),
    Failed,
}

#[derive(Default)]
pub struct Thumbs {
    map: HashMap<PathBuf, State>,
}

impl Thumbs {
    pub fn get(&self, path: &Path) -> Option<gdk::Texture> {
        match self.map.get(path) {
            Some(State::Ready(texture)) => Some(texture.clone()),
            _ => None,
        }
    }

    fn insert(&mut self, path: PathBuf, state: State) {
        self.map.insert(path, state);
    }

    fn contains(&self, path: &Path) -> bool {
        self.map.contains_key(path)
    }
}

pub fn texture_from(thumb: Thumb) -> gdk::Texture {
    let stride = (thumb.width * 4) as usize;
    gdk::MemoryTexture::new(
        thumb.width,
        thumb.height,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(thumb.rgba),
        stride,
    )
    .upcast()
}

/// Make a thumbnail in the background (once per file) and redraw the library
/// when it is ready.
pub fn request(app: &Rc<App>, path: PathBuf, kind: MediaKind) {
    if kind == MediaKind::Audio || app.thumbs.borrow().contains(&path) {
        return;
    }
    app.thumbs.borrow_mut().insert(path.clone(), State::Loading);
    let app = app.clone();
    glib::spawn_future_local(async move {
        let source = path.clone();
        let thumb = gio::spawn_blocking(move || match kind {
            MediaKind::Video => media::video_thumbnail(&source),
            MediaKind::Image => media::image_thumbnail(&source),
            MediaKind::Audio => None,
        })
        .await
        .ok()
        .flatten();
        let state = match thumb {
            Some(t) => State::Ready(texture_from(t)),
            None => State::Failed,
        };
        app.thumbs.borrow_mut().insert(path, state);
        editor::refresh_library(&app);
    });
}
