use crate::doc::{MediaPath, ProjectDoc, SCHEMA_VERSION};
use crate::error::ProjectError;
use crate::migrate::migrate;
use cutroom_timeline::MediaId;
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub const PROJECT_FILE: &str = "project.json";
const MEDIA_DIR: &str = "media";
const PROXY_DIR: &str = "proxies";
const CACHE_DIR: &str = "cache";
const THUMB_DIR: &str = "thumbnails";

/// The picture the home screen shows for the project in `root`: the preview
/// as it last was, saved when the project is saved or closed.
pub fn cover_path(root: &std::path::Path) -> std::path::PathBuf {
    root.join(THUMB_DIR).join("cover.png")
}
const AUTOSAVE_DIR: &str = "autosave";

#[derive(Serialize)]
struct EnvelopeRef<'a> {
    schema_version: u32,
    app: &'static str,
    project: &'a ProjectDoc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportMode {
    /// Keep the file where it is and point at it.
    Reference,
    /// Copy the file into the project's `media/` folder.
    Copy,
}

/// How much of each end of a file the fingerprint reads.
const FINGERPRINT_SPAN: u64 = 4 * 1024 * 1024;

/// A quick identity for a media file: BLAKE3 over the file size plus its first
/// and last 4 MB. Reading a multi-gigabyte video in full would freeze the app
/// on import, and this is plenty to recognise a moved or renamed file (it
/// would miss an edit confined to the middle of a very large file).
pub fn hash_file(path: &Path) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let mut hasher = blake3::Hasher::new();
    hasher.update(&len.to_le_bytes());

    let head_len = len.min(FINGERPRINT_SPAN);
    let mut buffer = Vec::new();
    Read::by_ref(&mut file)
        .take(head_len)
        .read_to_end(&mut buffer)?;
    hasher.update(&buffer);

    if len > FINGERPRINT_SPAN {
        let tail_start = (len - FINGERPRINT_SPAN).max(FINGERPRINT_SPAN);
        file.seek(SeekFrom::Start(tail_start))?;
        buffer.clear();
        file.read_to_end(&mut buffer)?;
        hasher.update(&buffer);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// A project folder on disk. Holds no document state itself.
#[derive(Debug, Clone)]
pub struct ProjectFolder {
    root: PathBuf,
}

impl ProjectFolder {
    fn at(root: impl AsRef<Path>) -> Result<Self, ProjectError> {
        Ok(ProjectFolder {
            root: std::path::absolute(root.as_ref())?,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Create a new project folder (and any missing parents) and save `doc` into it.
    pub fn create(root: impl AsRef<Path>, doc: &ProjectDoc) -> Result<Self, ProjectError> {
        let folder = Self::at(root)?;
        if folder.root.join(PROJECT_FILE).exists() {
            return Err(ProjectError::AlreadyExists(folder.root));
        }
        for dir in [MEDIA_DIR, PROXY_DIR, CACHE_DIR, THUMB_DIR, AUTOSAVE_DIR] {
            fs::create_dir_all(folder.root.join(dir))?;
        }
        folder.save(doc)?;
        Ok(folder)
    }

    /// Open an existing project, migrating older files to the current schema.
    pub fn open(root: impl AsRef<Path>) -> Result<(Self, ProjectDoc), ProjectError> {
        let folder = Self::at(root)?;
        let file = folder.root.join(PROJECT_FILE);
        if !file.is_file() {
            return Err(ProjectError::NotAProject(folder.root));
        }
        let value: Value = serde_json::from_slice(&fs::read(&file)?)?;
        let mut value = migrate(value)?;
        let project = value
            .get_mut("project")
            .map(Value::take)
            .ok_or_else(|| ProjectError::Corrupt("missing project".into()))?;
        let doc: ProjectDoc = serde_json::from_value(project)?;
        Ok((folder, doc))
    }

    /// Atomic save: write a temp file, flush it, then rename over the real one,
    /// so a crash mid-save never leaves a half-written project.
    pub fn save(&self, doc: &ProjectDoc) -> Result<(), ProjectError> {
        let bytes = encode(doc)?;
        // Unique per call so a background autosave and a manual save never
        // write the same temp file.
        let tmp = self.root.join(format!(
            "{PROJECT_FILE}.{}.{}.tmp",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, self.root.join(PROJECT_FILE))?;
        Ok(())
    }

    /// Write a timestamped backup into `autosave/`, keeping only the newest `keep`.
    pub fn autosave(&self, doc: &ProjectDoc, keep: usize) -> Result<PathBuf, ProjectError> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.autosave_at(doc, stamp, keep)
    }

    pub(crate) fn autosave_at(
        &self,
        doc: &ProjectDoc,
        stamp: u64,
        keep: usize,
    ) -> Result<PathBuf, ProjectError> {
        let dir = self.root.join(AUTOSAVE_DIR);
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("autosave-{stamp:020}.json"));
        fs::write(&path, encode(doc)?)?;

        let mut backups: Vec<PathBuf> = fs::read_dir(&dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("autosave-") && n.ends_with(".json"))
            })
            .collect();
        backups.sort(); // zero-padded stamps sort chronologically
        let excess = backups.len().saturating_sub(keep.max(1));
        for old in backups.into_iter().take(excess) {
            let _ = fs::remove_file(old);
        }
        Ok(path)
    }

    // ----- locations -------------------------------------------------------

    pub fn media_dir(&self) -> PathBuf {
        self.root.join(MEDIA_DIR)
    }

    pub fn thumbnail_dir(&self) -> PathBuf {
        self.root.join(THUMB_DIR)
    }

    /// The picture the home screen shows for this project.
    pub fn cover_path(&self) -> PathBuf {
        cover_path(&self.root)
    }

    pub fn proxy_dir(&self, doc: &ProjectDoc) -> PathBuf {
        self.storage_dir(doc.storage.proxy_dir.as_deref(), PROXY_DIR)
    }

    pub fn cache_dir(&self, doc: &ProjectDoc) -> PathBuf {
        self.storage_dir(doc.storage.cache_dir.as_deref(), CACHE_DIR)
    }

    fn storage_dir(&self, setting: Option<&str>, default: &str) -> PathBuf {
        match setting {
            Some(s) if Path::new(s).is_absolute() => PathBuf::from(s),
            Some(s) => self.root.join(s),
            None => self.root.join(default),
        }
    }

    /// Resolve a media path to a real filesystem path.
    pub fn resolve(&self, path: &MediaPath) -> Result<PathBuf, ProjectError> {
        match path {
            MediaPath::Absolute(p) => Ok(PathBuf::from(p)),
            MediaPath::Relative(rel) => {
                let mut out = self.root.clone();
                for part in rel.split('/') {
                    // Reject anything that could climb out of the project folder.
                    if part.is_empty() || part == "." || part == ".." || part.contains(['\\', ':'])
                    {
                        return Err(ProjectError::BadMediaPath(rel.clone()));
                    }
                    out.push(part);
                }
                Ok(out)
            }
        }
    }

    /// Relative path if `file` is inside the project folder, absolute otherwise.
    fn media_path_for(&self, file: &Path) -> Result<MediaPath, ProjectError> {
        let abs = std::path::absolute(file)?;
        let to_string = |p: &Path| {
            p.to_str()
                .map(str::to_owned)
                .ok_or_else(|| ProjectError::BadMediaPath(p.display().to_string()))
        };
        match abs.strip_prefix(&self.root) {
            Ok(rel) => {
                let mut parts = Vec::new();
                for component in rel.components() {
                    match component {
                        Component::Normal(os) => parts.push(to_string(Path::new(os))?),
                        _ => return Ok(MediaPath::Absolute(to_string(&abs)?)),
                    }
                }
                Ok(MediaPath::Relative(parts.join("/")))
            }
            Err(_) => Ok(MediaPath::Absolute(to_string(&abs)?)),
        }
    }

    // ----- media management ------------------------------------------------

    pub fn import_media(
        &self,
        doc: &mut ProjectDoc,
        src: &Path,
        mode: ImportMode,
    ) -> Result<MediaId, ProjectError> {
        let meta = fs::metadata(src)?;
        if !meta.is_file() {
            return Err(ProjectError::NotAFile(src.to_path_buf()));
        }
        let hash = hash_file(src)?;
        let path = match mode {
            ImportMode::Reference => self.media_path_for(src)?,
            ImportMode::Copy => {
                let dest = unique_destination(&self.media_dir(), src)?;
                fs::copy(src, &dest)?;
                self.media_path_for(&dest)?
            }
        };
        Ok(doc.add_media(path, Some(hash), Some(meta.len())))
    }

    /// Copy every media file that lives outside the project into `media/` and
    /// point the document at the copies. Returns how many files were copied.
    pub fn collect_media(&self, doc: &mut ProjectDoc) -> Result<usize, ProjectError> {
        let mut copied = 0;
        for index in 0..doc.media.len() {
            let current = self.resolve(&doc.media[index].path)?;
            if matches!(doc.media[index].path, MediaPath::Relative(_)) || !current.is_file() {
                continue;
            }
            let dest = unique_destination(&self.media_dir(), &current)?;
            fs::copy(&current, &dest)?;
            doc.media[index].path = self.media_path_for(&dest)?;
            copied += 1;
        }
        Ok(copied)
    }

    /// Point one item at a file the user picked (after it was moved or lost).
    pub fn relink_one(
        &self,
        doc: &mut ProjectDoc,
        id: MediaId,
        new_file: &Path,
    ) -> Result<(), ProjectError> {
        let meta = fs::metadata(new_file)?;
        if !meta.is_file() {
            return Err(ProjectError::NotAFile(new_file.to_path_buf()));
        }
        let hash = hash_file(new_file)?;
        let path = self.media_path_for(new_file)?;
        doc.relink_media(id, path, Some(hash), Some(meta.len()))
    }

    /// Is this file stored inside the project folder?
    pub fn contains(&self, path: &Path) -> bool {
        let root = fs::canonicalize(self.root()).unwrap_or_else(|_| self.root().to_path_buf());
        let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        path.starts_with(root)
    }

    pub fn find_missing(&self, doc: &ProjectDoc) -> Vec<MediaId> {
        doc.media
            .iter()
            .filter(|m| !self.resolve(&m.path).map(|p| p.is_file()).unwrap_or(false))
            .map(|m| m.id)
            .collect()
    }

    /// Search `search_dir` (recursively, not following symlinks) for files whose
    /// content hash matches missing media, and repoint the document at them.
    pub fn relink_by_hash(
        &self,
        doc: &mut ProjectDoc,
        search_dir: &Path,
    ) -> Result<Vec<MediaId>, ProjectError> {
        let mut wanted: Vec<MediaId> = self
            .find_missing(doc)
            .into_iter()
            .filter(|id| doc.media.iter().any(|m| m.id == *id && m.hash.is_some()))
            .collect();
        let mut relinked = Vec::new();
        let mut pending_dirs = vec![search_dir.to_path_buf()];

        while let Some(dir) = pending_dirs.pop() {
            if wanted.is_empty() {
                break;
            }
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    pending_dirs.push(entry.path());
                    continue;
                }
                if !kind.is_file() {
                    continue;
                }
                let Ok(meta) = entry.metadata() else { continue };
                let size_matches = |id: &MediaId| {
                    doc.media
                        .iter()
                        .find(|m| m.id == *id)
                        .is_some_and(|m| m.size.is_none_or(|s| s == meta.len()))
                };
                if !wanted.iter().any(size_matches) {
                    continue;
                }
                let Ok(digest) = hash_file(&entry.path()) else {
                    continue;
                };
                let hit = wanted.iter().position(|id| {
                    doc.media
                        .iter()
                        .find(|m| m.id == *id)
                        .is_some_and(|m| m.hash.as_deref() == Some(digest.as_str()))
                });
                if let Some(pos) = hit {
                    let id = wanted.remove(pos);
                    let path = self.media_path_for(&entry.path())?;
                    if let Some(item) = doc.media.iter_mut().find(|m| m.id == id) {
                        item.path = path;
                    }
                    relinked.push(id);
                }
            }
        }
        Ok(relinked)
    }

    /// Move the whole project folder. Relative media references stay valid.
    /// Custom absolute proxy/cache locations are left where they are.
    pub fn move_to(self, new_root: impl AsRef<Path>) -> Result<ProjectFolder, ProjectError> {
        let dest = std::path::absolute(new_root.as_ref())?;
        let occupied = dest.exists() && (!dest.is_dir() || fs::read_dir(&dest)?.next().is_some());
        if dest.starts_with(&self.root) || occupied {
            return Err(ProjectError::InvalidDestination(dest));
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        if fs::rename(&self.root, &dest).is_err() {
            // Different drive/volume: copy, then remove the original.
            copy_dir_recursive(&self.root, &dest)?;
            fs::remove_dir_all(&self.root)?;
        }
        Ok(ProjectFolder { root: dest })
    }
}

fn encode(doc: &ProjectDoc) -> Result<Vec<u8>, ProjectError> {
    let envelope = EnvelopeRef {
        schema_version: SCHEMA_VERSION,
        app: "kani",
        project: doc,
    };
    Ok(serde_json::to_vec_pretty(&envelope)?)
}

/// `dir/name.ext`, or `dir/name-1.ext`, `name-2.ext`... if taken.
fn unique_destination(dir: &Path, src: &Path) -> Result<PathBuf, ProjectError> {
    fs::create_dir_all(dir)?;
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| ProjectError::BadMediaPath(src.display().to_string()))?;
    let ext = src.extension().and_then(|e| e.to_str());
    let name = |n: u32| match (n, ext) {
        (0, Some(e)) => format!("{stem}.{e}"),
        (0, None) => stem.to_owned(),
        (n, Some(e)) => format!("{stem}-{n}.{e}"),
        (n, None) => format!("{stem}-{n}"),
    };
    (0..)
        .map(|n| dir.join(name(n)))
        .find(|p| !p.exists())
        .ok_or_else(|| ProjectError::BadMediaPath(src.display().to_string()))
}

fn copy_dir_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::ProjectSettings;
    use cutroom_timeline::{apply, Edit, FrameRate, Ticks, TrackKind};
    use tempfile::tempdir;

    fn new_doc() -> ProjectDoc {
        ProjectDoc::new("demo", ProjectSettings::vertical(FrameRate::FPS_30))
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn create_save_open_roundtrip_with_edits() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("proj");
        let mut doc = new_doc();
        let media = doc.add_media(MediaPath::Relative("media/a.mp4".into()), None, None);
        let track = doc.timeline.new_track(TrackKind::Video, "V1");
        let track_id = track.id;
        apply(&mut doc.timeline, Edit::InsertTrack { index: 0, track }).unwrap();
        let clip = doc
            .timeline
            .new_clip(media, Ticks(0), Ticks(0), Ticks(1000));
        apply(
            &mut doc.timeline,
            Edit::InsertClip {
                track: track_id,
                clip,
            },
        )
        .unwrap();

        ProjectFolder::create(&root, &doc).unwrap();
        let (_, loaded) = ProjectFolder::open(&root).unwrap();
        assert_eq!(loaded, doc);
        assert!(
            !root.join("project.json.tmp").exists(),
            "temp file must not linger"
        );
    }

    #[test]
    fn id_counters_survive_a_reload() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("proj");
        let mut doc = new_doc();
        let first = doc.add_media(MediaPath::Absolute("/x".into()), None, None);
        let clip = doc.timeline.new_clip(first, Ticks(0), Ticks(0), Ticks(1));
        ProjectFolder::create(&root, &doc).unwrap();

        let (_, mut loaded) = ProjectFolder::open(&root).unwrap();
        let second = loaded.add_media(MediaPath::Absolute("/y".into()), None, None);
        let next_clip = loaded
            .timeline
            .new_clip(second, Ticks(0), Ticks(0), Ticks(1));
        assert_ne!(first, second);
        assert_ne!(clip.id, next_clip.id);
    }

    #[test]
    fn create_refuses_to_overwrite() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("proj");
        ProjectFolder::create(&root, &new_doc()).unwrap();
        assert!(matches!(
            ProjectFolder::create(&root, &new_doc()),
            Err(ProjectError::AlreadyExists(_))
        ));
    }

    #[test]
    fn open_rejects_plain_folders_and_newer_projects() {
        let tmp = tempdir().unwrap();
        assert!(matches!(
            ProjectFolder::open(tmp.path()),
            Err(ProjectError::NotAProject(_))
        ));

        let root = tmp.path().join("future");
        let folder = ProjectFolder::create(&root, &new_doc()).unwrap();
        let mut value: Value =
            serde_json::from_slice(&fs::read(root.join(PROJECT_FILE)).unwrap()).unwrap();
        value["schema_version"] = Value::from(SCHEMA_VERSION + 1);
        fs::write(
            folder.root().join(PROJECT_FILE),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            ProjectFolder::open(&root),
            Err(ProjectError::TooNew { .. })
        ));
    }

    #[test]
    fn copy_import_uses_relative_paths_and_avoids_collisions() {
        let tmp = tempdir().unwrap();
        let folder = ProjectFolder::create(tmp.path().join("proj"), &new_doc()).unwrap();
        let mut doc = new_doc();
        let src = tmp.path().join("footage/clip.mp4");
        write_file(&src, b"one");

        let a = folder
            .import_media(&mut doc, &src, ImportMode::Copy)
            .unwrap();
        let b = folder
            .import_media(&mut doc, &src, ImportMode::Copy)
            .unwrap();
        assert_eq!(
            doc.media_item(a).unwrap().path,
            MediaPath::Relative("media/clip.mp4".into())
        );
        assert_eq!(
            doc.media_item(b).unwrap().path,
            MediaPath::Relative("media/clip-1.mp4".into())
        );
        assert!(folder.find_missing(&doc).is_empty());
    }

    #[test]
    fn reference_import_outside_project_is_absolute() {
        let tmp = tempdir().unwrap();
        let folder = ProjectFolder::create(tmp.path().join("proj"), &new_doc()).unwrap();
        let mut doc = new_doc();
        let src = tmp.path().join("footage/clip.mp4");
        write_file(&src, b"data");
        let id = folder
            .import_media(&mut doc, &src, ImportMode::Reference)
            .unwrap();
        assert!(matches!(
            doc.media_item(id).unwrap().path,
            MediaPath::Absolute(_)
        ));
    }

    #[test]
    fn moving_a_project_keeps_relative_media_working() {
        let tmp = tempdir().unwrap();
        let folder = ProjectFolder::create(tmp.path().join("old/proj"), &new_doc()).unwrap();
        let mut doc = new_doc();
        let src = tmp.path().join("clip.mp4");
        write_file(&src, b"data");
        folder
            .import_media(&mut doc, &src, ImportMode::Copy)
            .unwrap();
        folder.save(&doc).unwrap();

        let moved = folder
            .move_to(tmp.path().join("elsewhere/new-home"))
            .unwrap();
        assert!(!tmp.path().join("old/proj").exists());
        let (reopened, loaded) = ProjectFolder::open(moved.root()).unwrap();
        assert!(reopened.find_missing(&loaded).is_empty());
    }

    #[test]
    fn cannot_move_into_itself_or_onto_a_full_folder() {
        let tmp = tempdir().unwrap();
        let folder = ProjectFolder::create(tmp.path().join("proj"), &new_doc()).unwrap();
        assert!(matches!(
            folder.clone().move_to(folder.root().join("inside")),
            Err(ProjectError::InvalidDestination(_))
        ));
        let busy = tmp.path().join("busy");
        write_file(&busy.join("file.txt"), b"x");
        assert!(matches!(
            folder.move_to(&busy),
            Err(ProjectError::InvalidDestination(_))
        ));
    }

    #[test]
    fn collect_media_gathers_external_files() {
        let tmp = tempdir().unwrap();
        let folder = ProjectFolder::create(tmp.path().join("proj"), &new_doc()).unwrap();
        let mut doc = new_doc();
        let src = tmp.path().join("elsewhere/clip.mp4");
        write_file(&src, b"data");
        let id = folder
            .import_media(&mut doc, &src, ImportMode::Reference)
            .unwrap();

        assert_eq!(folder.collect_media(&mut doc).unwrap(), 1);
        assert_eq!(
            doc.media_item(id).unwrap().path,
            MediaPath::Relative("media/clip.mp4".into())
        );
        assert!(folder.media_dir().join("clip.mp4").is_file());
        assert_eq!(
            folder.collect_media(&mut doc).unwrap(),
            0,
            "second run is a no-op"
        );
    }

    #[test]
    fn relink_finds_moved_files_by_content() {
        let tmp = tempdir().unwrap();
        let folder = ProjectFolder::create(tmp.path().join("proj"), &new_doc()).unwrap();
        let mut doc = new_doc();
        let original = tmp.path().join("drive-a/clip.mp4");
        write_file(&original, b"the real footage");
        let id = folder
            .import_media(&mut doc, &original, ImportMode::Reference)
            .unwrap();
        // A decoy with the same size but different content.
        write_file(&tmp.path().join("drive-b/decoy.mp4"), b"the fake footage");

        // The user moves/renames the file.
        let new_home = tmp.path().join("drive-b/deep/renamed.mov");
        fs::create_dir_all(new_home.parent().unwrap()).unwrap();
        fs::rename(&original, &new_home).unwrap();
        assert_eq!(folder.find_missing(&doc), vec![id]);

        let relinked = folder
            .relink_by_hash(&mut doc, &tmp.path().join("drive-b"))
            .unwrap();
        assert_eq!(relinked, vec![id]);
        assert!(folder.find_missing(&doc).is_empty());
        assert_eq!(
            folder.resolve(&doc.media_item(id).unwrap().path).unwrap(),
            new_home
        );
    }

    #[test]
    fn relative_paths_cannot_escape_the_project() {
        let tmp = tempdir().unwrap();
        let folder = ProjectFolder::create(tmp.path().join("proj"), &new_doc()).unwrap();
        for bad in [
            "../secret.mp4",
            "media/../../x",
            "/etc/passwd",
            "a//b",
            "C:evil",
        ] {
            assert!(
                matches!(
                    folder.resolve(&MediaPath::Relative(bad.into())),
                    Err(ProjectError::BadMediaPath(_))
                ),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn storage_locations_follow_settings() {
        let tmp = tempdir().unwrap();
        let folder = ProjectFolder::create(tmp.path().join("proj"), &new_doc()).unwrap();
        let mut doc = new_doc();
        assert_eq!(folder.proxy_dir(&doc), folder.root().join("proxies"));

        let scratch = tmp.path().join("big-drive/proxies");
        doc.storage.proxy_dir = Some(scratch.to_str().unwrap().to_owned());
        doc.storage.cache_dir = Some("my-cache".into());
        assert_eq!(folder.proxy_dir(&doc), scratch);
        assert_eq!(folder.cache_dir(&doc), folder.root().join("my-cache"));
    }

    #[test]
    fn autosave_keeps_only_the_newest() {
        let tmp = tempdir().unwrap();
        let folder = ProjectFolder::create(tmp.path().join("proj"), &new_doc()).unwrap();
        let doc = new_doc();
        for stamp in [10, 20, 30, 40] {
            folder.autosave_at(&doc, stamp, 2).unwrap();
        }
        let mut names: Vec<_> = fs::read_dir(folder.root().join("autosave"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names.len(), 2);
        assert!(names[0].ends_with("30.json") && names[1].ends_with("40.json"));
    }

    #[test]
    fn the_fingerprint_is_stable_and_sensitive_to_content_and_size() {
        let tmp = tempdir().unwrap();
        let a = tmp.path().join("a.bin");
        let b = tmp.path().join("b.bin");
        write_file(&a, b"hello world");
        write_file(&b, b"hello world");
        assert_eq!(hash_file(&a).unwrap(), hash_file(&b).unwrap());
        write_file(&b, b"hello worle");
        assert_ne!(hash_file(&a).unwrap(), hash_file(&b).unwrap());
        write_file(&b, b"hello world!");
        assert_ne!(
            hash_file(&a).unwrap(),
            hash_file(&b).unwrap(),
            "size is part of it"
        );
        write_file(&b, b"");
        assert!(hash_file(&b).is_ok(), "empty files are fine");
    }

    #[test]
    fn large_files_are_fingerprinted_from_both_ends_only() {
        let tmp = tempdir().unwrap();
        let big = FINGERPRINT_SPAN as usize * 3;
        let mut data = vec![7u8; big];
        let path = tmp.path().join("big.bin");
        write_file(&path, &data);
        let original = hash_file(&path).unwrap();

        data[10] = 8; // inside the first 4 MB
        write_file(&path, &data);
        let head_changed = hash_file(&path).unwrap();
        assert_ne!(original, head_changed);

        data[10] = 7;
        data[big - 10] = 8; // inside the last 4 MB
        write_file(&path, &data);
        assert_ne!(original, hash_file(&path).unwrap());

        data[big - 10] = 7;
        data[big / 2] = 8; // the middle is deliberately not read
        write_file(&path, &data);
        assert_eq!(original, hash_file(&path).unwrap());
    }

    #[test]
    fn relinking_one_item_points_it_at_the_chosen_file_and_forgets_the_probe() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = ProjectFolder::create(
            &tmp.path().join("p"),
            &ProjectDoc::new(
                "p",
                ProjectSettings::vertical(cutroom_timeline::FrameRate::FPS_30),
            ),
        )
        .unwrap();
        let mut doc = ProjectDoc::new(
            "p",
            ProjectSettings::vertical(cutroom_timeline::FrameRate::FPS_30),
        );
        let old = tmp.path().join("old.mp4");
        std::fs::write(&old, b"old data").unwrap();
        let id = folder
            .import_media(&mut doc, &old, ImportMode::Reference)
            .unwrap();
        doc.set_media_duration(id, Ticks::from_seconds_f64(4.0))
            .unwrap();
        std::fs::remove_file(&old).unwrap();
        assert_eq!(folder.find_missing(&doc), vec![id]);

        let new = tmp.path().join("moved").join("new name.mp4");
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        std::fs::write(&new, b"new data!").unwrap();
        folder.relink_one(&mut doc, id, &new).unwrap();
        assert!(folder.find_missing(&doc).is_empty());
        let item = doc.media_item(id).unwrap();
        assert_eq!(item.size, Some(9));
        assert!(
            item.duration.is_none() && item.probe.is_none(),
            "probed again after a relink"
        );
        assert!(
            folder.relink_one(&mut doc, id, tmp.path()).is_err(),
            "folders are refused"
        );
    }

    #[test]
    fn contains_tells_project_files_from_outside_files() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = ProjectFolder::create(
            &tmp.path().join("p"),
            &ProjectDoc::new(
                "p",
                ProjectSettings::vertical(cutroom_timeline::FrameRate::FPS_30),
            ),
        )
        .unwrap();
        std::fs::create_dir_all(folder.media_dir()).unwrap();
        let inside = folder.media_dir().join("a.mp4");
        std::fs::write(&inside, b"x").unwrap();
        let outside = tmp.path().join("b.mp4");
        std::fs::write(&outside, b"x").unwrap();
        assert!(folder.contains(&inside));
        assert!(!folder.contains(&outside));
    }
}
