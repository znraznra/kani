use crate::error::ProjectError;
use cutroom_timeline::{FrameRate, MediaId, Ticks, Timeline};
use serde::{Deserialize, Serialize};

/// Bump when the on-disk format changes, and add a step in `migrate.rs`.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectSettings {
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    pub sample_rate: u32,
}

impl ProjectSettings {
    /// 1080x1920 (Reels / Shorts / TikTok).
    pub fn vertical(frame_rate: FrameRate) -> Self {
        Self {
            width: 1080,
            height: 1920,
            frame_rate,
            sample_rate: 48_000,
        }
    }

    /// 1920x1080.
    pub fn horizontal(frame_rate: FrameRate) -> Self {
        Self {
            width: 1920,
            height: 1080,
            frame_rate,
            sample_rate: 48_000,
        }
    }

    /// 1080x1080.
    pub fn square(frame_rate: FrameRate) -> Self {
        Self {
            width: 1080,
            height: 1080,
            frame_rate,
            sample_rate: 48_000,
        }
    }
}

/// Where heavy generated files live. `None` means the default folder inside
/// the project. Values may be absolute (e.g. a big scratch drive) or relative
/// to the project folder.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StorageSettings {
    #[serde(default)]
    pub proxy_dir: Option<String>,
    #[serde(default)]
    pub cache_dir: Option<String>,
}

/// How a media file is located.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "path")]
pub enum MediaPath {
    /// Relative to the project folder, always `/`-separated so projects move
    /// freely between Windows, macOS and Linux.
    Relative(String),
    /// Anywhere else on disk.
    Absolute(String),
}

/// What reading a media file's streams revealed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MediaProbe {
    pub has_video: bool,
    pub has_audio: bool,
    pub width: u32,
    pub height: u32,
    /// Frame rate of the video stream, if it has one.
    #[serde(default)]
    pub frame_rate: Option<FrameRate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaItem {
    pub id: MediaId,
    pub path: MediaPath,
    /// BLAKE3 hex digest, used to relink files that moved.
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    /// Filled in once the media has been probed (Phase 3).
    #[serde(default)]
    pub duration: Option<Ticks>,
    /// Filled in once the media has been probed.
    #[serde(default)]
    pub probe: Option<MediaProbe>,
    /// A name the user gave it in the library (the file keeps its name).
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectDoc {
    pub name: String,
    pub settings: ProjectSettings,
    #[serde(default)]
    pub storage: StorageSettings,
    #[serde(default)]
    pub media: Vec<MediaItem>,
    pub timeline: Timeline,
    #[serde(default)]
    next_media_id: u64,
}

impl ProjectDoc {
    pub fn new(name: impl Into<String>, settings: ProjectSettings) -> Self {
        ProjectDoc {
            name: name.into(),
            settings,
            storage: StorageSettings::default(),
            media: Vec::new(),
            timeline: Timeline::new(),
            next_media_id: 0,
        }
    }

    pub fn add_media(
        &mut self,
        path: MediaPath,
        hash: Option<String>,
        size: Option<u64>,
    ) -> MediaId {
        self.next_media_id += 1;
        let id = MediaId(self.next_media_id);
        self.media.push(MediaItem {
            id,
            path,
            hash,
            size,
            duration: None,
            probe: None,
            name: None,
        });
        id
    }

    pub fn set_media_duration(&mut self, id: MediaId, duration: Ticks) -> Result<(), ProjectError> {
        let item = self
            .media
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or(ProjectError::MediaNotFound(id))?;
        item.duration = Some(duration);
        Ok(())
    }

    /// Record a probe. A zero or missing duration (images) leaves the
    /// duration unknown so the placeholder length keeps applying.
    pub fn set_media_probe(
        &mut self,
        id: MediaId,
        duration: Option<Ticks>,
        probe: MediaProbe,
    ) -> Result<(), ProjectError> {
        let item = self
            .media
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or(ProjectError::MediaNotFound(id))?;
        item.duration = duration.filter(|d| *d > Ticks::ZERO);
        item.probe = Some(probe);
        Ok(())
    }

    /// Give a library item a display name (`None` or blank goes back to the file name).
    pub fn rename_media(&mut self, id: MediaId, name: Option<String>) -> Result<(), ProjectError> {
        let item = self
            .media
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or(ProjectError::MediaNotFound(id))?;
        item.name = name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty());
        Ok(())
    }

    /// Take an item out of the library (the timeline is not touched here).
    pub fn remove_media(&mut self, id: MediaId) -> Result<MediaItem, ProjectError> {
        let index = self
            .media
            .iter()
            .position(|m| m.id == id)
            .ok_or(ProjectError::MediaNotFound(id))?;
        Ok(self.media.remove(index))
    }

    /// Point an item at another file. What was probed is forgotten, so the
    /// new file gets probed again.
    pub fn relink_media(
        &mut self,
        id: MediaId,
        path: MediaPath,
        hash: Option<String>,
        size: Option<u64>,
    ) -> Result<(), ProjectError> {
        let item = self
            .media
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or(ProjectError::MediaNotFound(id))?;
        item.path = path;
        item.hash = hash;
        item.size = size;
        item.probe = None;
        item.duration = None;
        Ok(())
    }

    pub fn media_item(&self, id: MediaId) -> Result<&MediaItem, ProjectError> {
        self.media
            .iter()
            .find(|m| m.id == id)
            .ok_or(ProjectError::MediaNotFound(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutroom_timeline::Ticks;

    #[test]
    fn media_can_be_renamed_removed_and_relinked_in_the_document() {
        let mut doc = ProjectDoc::new(
            "p",
            ProjectSettings::vertical(cutroom_timeline::FrameRate::FPS_30),
        );
        let a = doc.add_media(
            MediaPath::Absolute("/x/a.mp4".into()),
            Some("h".into()),
            Some(3),
        );
        let b = doc.add_media(MediaPath::Absolute("/x/b.mp4".into()), None, None);
        doc.rename_media(a, Some("  Intro  ".into())).unwrap();
        assert_eq!(doc.media_item(a).unwrap().name.as_deref(), Some("Intro"));
        doc.rename_media(a, Some("   ".into())).unwrap();
        assert_eq!(
            doc.media_item(a).unwrap().name,
            None,
            "blank goes back to the file name"
        );

        doc.set_media_duration(b, Ticks::from_seconds_f64(2.0))
            .unwrap();
        doc.relink_media(
            b,
            MediaPath::Absolute("/y/b2.mp4".into()),
            Some("h2".into()),
            Some(9),
        )
        .unwrap();
        let item = doc.media_item(b).unwrap();
        assert_eq!((item.size, item.duration), (Some(9), None));

        let removed = doc.remove_media(a).unwrap();
        assert_eq!(removed.id, a);
        assert!(doc.media_item(a).is_err());
        assert!(doc.remove_media(a).is_err());
        let c = doc.add_media(MediaPath::Absolute("/x/c.mp4".into()), None, None);
        assert_ne!(c, a, "ids are never reused");
    }
}
