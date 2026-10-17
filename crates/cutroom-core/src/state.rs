//! Editing session: the open project plus everything the UI needs to drive it.
//!
//! No egui types here. All behaviour is testable without a window, and every
//! timeline change goes through [`cutroom_timeline::History`].

use cutroom_project::{
    ImportMode, MediaItem, MediaPath, MediaProbe, ProjectDoc, ProjectError, ProjectFolder,
    ProjectSettings,
};
use cutroom_timeline::{
    apply, Clip, ClipId, Edit, EditError, History, MediaId, Ticks, Track, TrackId, TrackKind,
};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, TryRecvError};
use std::time::{Duration, Instant};
use thiserror::Error;

/// Until Phase 3 can probe real durations, imported media is treated as this long.
pub const PLACEHOLDER_MEDIA_SECONDS: f64 = 5.0;

/// File types the importer accepts (dialog filter and drag-and-drop).
pub const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "mkv", "webm", "avi", "m4v", "mp3", "wav", "aac", "flac", "ogg", "m4a", "png",
    "jpg", "jpeg", "webp", "gif",
];

pub fn is_supported_media(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| MEDIA_EXTENSIONS.iter().any(|m| m.eq_ignore_ascii_case(e)))
}

/// Most files imported from one folder at once.
pub const MAX_FOLDER_IMPORT: usize = 500;

/// Every supported media file in `dir` and its subfolders, in a stable order.
pub fn collect_media_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        if depth > 8 || out.len() >= MAX_FOLDER_IMPORT {
            return;
        }
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<PathBuf> = read.filter_map(Result::ok).map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            if out.len() >= MAX_FOLDER_IMPORT {
                return;
            }
            if path.is_dir() {
                walk(&path, depth + 1, out);
            } else if is_supported_media(&path) {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, 0, &mut files);
    files
}

/// What a media file is, judged from its extension until Phase 3 can probe it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Video,
    Audio,
    Image,
}

const AUDIO_EXTENSIONS: &[&str] = &["mp3", "wav", "aac", "flac", "ogg", "m4a"];
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif"];

impl MediaKind {
    pub fn from_path(path: &str) -> MediaKind {
        let ext = path.rsplit('.').next().unwrap_or("");
        let has = |list: &[&str]| list.iter().any(|e| e.eq_ignore_ascii_case(ext));
        if has(AUDIO_EXTENSIONS) {
            MediaKind::Audio
        } else if has(IMAGE_EXTENSIONS) {
            MediaKind::Image
        } else {
            MediaKind::Video
        }
    }

    /// Audio goes on audio tracks; video and images go on video tracks.
    pub fn track_kind(self) -> TrackKind {
        match self {
            MediaKind::Audio => TrackKind::Audio,
            MediaKind::Video | MediaKind::Image => TrackKind::Video,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MediaKind::Video => "Video",
            MediaKind::Audio => "Audio",
            MediaKind::Image => "Image",
        }
    }
}

/// `3 minutes ago` style text for the autosave note.
pub fn format_ago(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0..=59 => "just now".to_owned(),
        60..=119 => "1 minute ago".to_owned(),
        120..=3599 => format!("{} minutes ago", secs / 60),
        3600..=7199 => "1 hour ago".to_owned(),
        _ => format!("{} hours ago", secs / 3600),
    }
}

/// What kind of clip something is, for sharing settings only between clips
/// of the same kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipKind {
    /// A caption made by Generate captions.
    Caption,
    /// Any other text.
    Text,
    Video,
    Image,
    Audio,
}

/// Attributes copied from one clip, to paste on clips of the same kind.
#[derive(Debug, Clone)]
pub struct AttributeCopy {
    pub kind: ClipKind,
    /// The look of a text clip (its words are not copied).
    pub text: Option<cutroom_timeline::TextSpec>,
    pub caption: Option<String>,
    pub transform: cutroom_timeline::Transform,
    pub volume: i32,
}

/// A clip taken off the timeline by Copy or Cut, ready to be pasted.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipCopy {
    pub media: MediaId,
    pub source_in: Ticks,
    pub duration: Ticks,
    pub kind: TrackKind,
    /// The track it came from; pasting prefers it when there is room.
    pub source_track: Option<TrackId>,
    /// Where it started, relative to the earliest clip copied with it.
    pub offset: Ticks,
    pub transform: cutroom_timeline::Transform,
    pub audio_muted: bool,
    pub disabled: bool,
    /// Set for a text clip.
    pub text: Option<cutroom_timeline::TextSpec>,
    /// A caption's spoken words.
    pub words: Vec<cutroom_timeline::Word>,
    pub caption: Option<String>,
    pub caption_off: bool,
    pub volume: i32,
}

/// A stretch of sound to transcribe.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptionSource {
    pub path: PathBuf,
    /// Where in the file, and how long, in seconds.
    pub source_in: f64,
    pub duration: f64,
    /// Where on the timeline it starts, in seconds.
    pub start: f64,
}

/// The tracks every project starts with. They are never removed automatically.
pub fn is_default_track(track: &cutroom_timeline::Track) -> bool {
    matches!(
        (track.kind, track.name.as_str()),
        (TrackKind::Video, "Video 1") | (TrackKind::Audio, "Audio 1")
    )
}

/// Where a new track of `kind` goes. Video layers stack above audio tracks;
/// `at_top` puts it at the front of its group (the top layer for video).
pub fn new_track_index(tl: &cutroom_timeline::Timeline, kind: TrackKind, at_top: bool) -> usize {
    match (kind, at_top) {
        (TrackKind::Video, true) => 0,
        (TrackKind::Video, false) => tl
            .tracks
            .iter()
            .rposition(|t| t.kind == TrackKind::Video)
            .map_or(0, |i| i + 1),
        (TrackKind::Audio, true) => tl
            .tracks
            .iter()
            .position(|t| t.kind == TrackKind::Audio)
            .unwrap_or(tl.tracks.len()),
        (TrackKind::Audio, false) => tl.tracks.len(),
    }
}

struct AutosaveJob {
    revision: u64,
    done: mpsc::Receiver<Result<(), String>>,
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    Edit(#[from] EditError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageKind {
    Proxy,
    Cache,
}

pub struct Session {
    pub folder: ProjectFolder,
    pub doc: ProjectDoc,
    pub history: History,
    pub selected: Option<ClipId>,
    /// The whole selection when several clips are selected. Only counts while it
    /// contains `selected`, so anything that picks a single clip by setting
    /// `selected` directly leaves it stale and harmless.
    pub multi: BTreeSet<ClipId>,
    /// Attributes copied with "Copy attributes".
    pub attributes: Option<AttributeCopy>,
    pub playhead: Ticks,
    pub playing: bool,
    pub import_mode: ImportMode,
    /// Last message for the status bar (errors from rejected edits land here).
    pub status: Option<String>,
    pub missing: HashSet<MediaId>,
    /// A text clip being edited live: its words before the first live change,
    /// so the one undo step the edit becomes goes back to them.
    text_baseline: Option<(ClipId, cutroom_timeline::TextSpec)>,
    /// Bumped on every change; compared with `saved_revision` to know if there
    /// is anything unsaved.
    revision: u64,
    saved_revision: u64,
    autosave_job: Option<AutosaveJob>,
    last_autosave: Instant,
    last_autosaved: Option<Instant>,
}

impl Session {
    pub fn create(
        root: &Path,
        name: &str,
        settings: ProjectSettings,
    ) -> Result<Session, SessionError> {
        let mut doc = ProjectDoc::new(name, settings);
        // Starter tracks are part of the new document, not an undoable edit.
        for (kind, label) in [(TrackKind::Video, "Video 1"), (TrackKind::Audio, "Audio 1")] {
            let track = doc.timeline.new_track(kind, label);
            let index = doc.timeline.tracks.len();
            apply(&mut doc.timeline, Edit::InsertTrack { index, track })?;
        }
        let folder = ProjectFolder::create(root, &doc)?;
        Ok(Self::from_parts(folder, doc))
    }

    pub fn open(root: &Path) -> Result<Session, SessionError> {
        let (folder, doc) = ProjectFolder::open(root)?;
        let mut session = Self::from_parts(folder, doc);
        session.refresh_missing();
        Ok(session)
    }

    fn from_parts(folder: ProjectFolder, doc: ProjectDoc) -> Session {
        let import_mode = if doc.storage.import_copy {
            ImportMode::Copy
        } else {
            ImportMode::Reference
        };
        Session {
            folder,
            doc,
            history: History::default(),
            selected: None,
            multi: BTreeSet::new(),
            attributes: None,
            playhead: Ticks::ZERO,
            playing: false,
            import_mode,
            status: None,
            missing: HashSet::new(),
            text_baseline: None,
            revision: 0,
            saved_revision: 0,
            autosave_job: None,
            last_autosave: Instant::now(),
            last_autosaved: None,
        }
    }

    // ----- saving ----------------------------------------------------------

    pub fn title(&self) -> String {
        format!(
            "{}{} - Kani",
            self.doc.name,
            if self.is_dirty() { " *" } else { "" }
        )
    }

    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    fn mark_dirty(&mut self) {
        self.revision += 1;
    }

    /// Save now. Waits for a background autosave first so an older snapshot
    /// can never land after this one.
    pub fn save(&mut self) {
        self.wait_for_autosave();
        match self.folder.save(&self.doc) {
            Ok(()) => {
                self.saved_revision = self.revision;
                self.status = Some("Saved".into());
            }
            Err(e) => self.status = Some(format!("Save failed: {e}")),
        }
    }

    /// Final save when leaving the project or quitting.
    pub fn close(&mut self) -> Result<(), SessionError> {
        self.wait_for_autosave();
        if self.is_dirty() {
            self.folder.save(&self.doc)?;
            self.saved_revision = self.revision;
        }
        Ok(())
    }

    /// Start a background autosave if there are unsaved changes and `every`
    /// has passed. `Duration::ZERO` means autosave is off. Never blocks.
    pub fn autosave_if_due(&mut self, every: Duration) {
        self.poll_autosave();
        if every.is_zero()
            || self.autosave_job.is_some()
            || !self.is_dirty()
            || self.last_autosave.elapsed() < every
        {
            return;
        }
        self.start_autosave();
    }

    /// Snapshot the project and write it on a worker thread: the real
    /// `project.json` (atomically) plus a rotating backup in `autosave/`.
    pub fn start_autosave(&mut self) {
        if self.autosave_job.is_some() {
            return;
        }
        let revision = self.revision;
        let folder = self.folder.clone();
        let doc = self.doc.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = folder
                .save(&doc)
                .and_then(|()| folder.autosave(&doc, 10).map(|_| ()))
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        self.last_autosave = Instant::now();
        self.autosave_job = Some(AutosaveJob { revision, done: rx });
    }

    fn finish_autosave(&mut self, revision: u64, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.saved_revision = self.saved_revision.max(revision);
                self.last_autosaved = Some(Instant::now());
            }
            Err(e) => self.status = Some(format!("Autosave failed: {e}")),
        }
    }

    /// Collect the result of a finished background autosave, if any.
    pub fn poll_autosave(&mut self) {
        let Some(job) = self.autosave_job.take() else {
            return;
        };
        match job.done.try_recv() {
            Ok(result) => self.finish_autosave(job.revision, result),
            Err(TryRecvError::Empty) => self.autosave_job = Some(job),
            Err(TryRecvError::Disconnected) => {
                self.status = Some("Autosave failed: the worker stopped unexpectedly".into());
            }
        }
    }

    fn wait_for_autosave(&mut self) {
        if let Some(job) = self.autosave_job.take() {
            match job.done.recv() {
                Ok(result) => self.finish_autosave(job.revision, result),
                Err(_) => {
                    self.status = Some("Autosave failed: the worker stopped unexpectedly".into())
                }
            }
        }
    }

    pub fn autosave_running(&self) -> bool {
        self.autosave_job.is_some()
    }

    /// "Autosaved 3 minutes ago", once an autosave has happened this session.
    pub fn autosave_note(&self) -> Option<String> {
        self.last_autosaved
            .map(|t| format!("Autosaved {}", format_ago(t.elapsed())))
    }

    pub fn move_project(&mut self, destination: &Path) -> Result<(), SessionError> {
        self.folder = self.folder.clone().move_to(destination)?;
        self.status = Some(format!("Project moved to {}", self.folder.root().display()));
        Ok(())
    }

    /// Rename the project (the name shown in Kani; the folder keeps its name).
    pub fn set_name(&mut self, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() || name == self.doc.name {
            return false;
        }
        self.doc.name = name.to_owned();
        self.mark_dirty();
        true
    }

    /// Change resolution, frame rate and colour space. Not an undoable edit:
    /// it is a project setting, like the name. Returns true if anything changed.
    pub fn set_format(
        &mut self,
        width: u32,
        height: u32,
        frame_rate: cutroom_timeline::FrameRate,
        color_space: cutroom_project::ColorSpace,
    ) -> bool {
        let st = &mut self.doc.settings;
        let (width, height) = (width.clamp(64, 8192) & !1, height.clamp(64, 8192) & !1);
        if st.width == width
            && st.height == height
            && st.frame_rate == frame_rate
            && st.color_space == color_space
        {
            return false;
        }
        st.width = width;
        st.height = height;
        st.frame_rate = frame_rate;
        st.color_space = color_space;
        self.mark_dirty();
        true
    }

    /// Choose whether newly imported media is copied into the project. With
    /// `relink_existing`, media already in the project is copied in too and
    /// the project is repointed at the copies.
    pub fn set_import_copy(
        &mut self,
        copy: bool,
        relink_existing: bool,
    ) -> Result<usize, SessionError> {
        self.import_mode = if copy {
            ImportMode::Copy
        } else {
            ImportMode::Reference
        };
        if self.doc.storage.import_copy != copy {
            self.doc.storage.import_copy = copy;
            self.mark_dirty();
        }
        if copy && relink_existing {
            self.collect_media()
        } else {
            Ok(0)
        }
    }

    pub fn collect_media(&mut self) -> Result<usize, SessionError> {
        let copied = self.folder.collect_media(&mut self.doc)?;
        if copied > 0 {
            self.mark_dirty();
        }
        self.refresh_missing();
        Ok(copied)
    }

    pub fn set_storage_dir(&mut self, which: StorageKind, dir: Option<&Path>) {
        let value = dir.map(|d| d.to_string_lossy().into_owned());
        match which {
            StorageKind::Proxy => self.doc.storage.proxy_dir = value,
            StorageKind::Cache => self.doc.storage.cache_dir = value,
        }
        self.mark_dirty();
    }

    // ----- editing ---------------------------------------------------------

    /// Run an edit through the undo history. Rejected edits leave the timeline
    /// unchanged and put the reason in the status bar.
    pub fn exec(&mut self, edit: Edit) -> bool {
        let (edit, pruned) = self.with_pruned_tracks(edit);
        match self.history.execute(&mut self.doc.timeline, edit) {
            Ok(()) => {
                self.mark_dirty();
                match pruned.as_slice() {
                    [] => {}
                    [one] => self.status = Some(format!("Removed the empty track \"{one}\"")),
                    many => {
                        self.status = Some(format!("Removed {} empty tracks", many.len()));
                    }
                }
                true
            }
            Err(e) => {
                self.status = Some(e.to_string());
                false
            }
        }
    }

    /// A track that this edit leaves empty is removed in the same undo step
    /// (the default "Video 1" and "Audio 1" always stay, and so does any track
    /// that was already empty before the edit).
    fn with_pruned_tracks(&self, edit: Edit) -> (Edit, Vec<String>) {
        let candidates: Vec<TrackId> = self
            .doc
            .timeline
            .tracks
            .iter()
            .filter(|t| !t.clips.is_empty() && !is_default_track(t))
            .map(|t| t.id)
            .collect();
        if candidates.is_empty() {
            return (edit, Vec::new());
        }
        let mut after = self.doc.timeline.clone();
        if cutroom_timeline::apply(&mut after, edit.clone()).is_err() {
            return (edit, Vec::new());
        }
        let emptied: Vec<TrackId> = candidates
            .into_iter()
            .filter(|id| after.track(*id).is_some_and(|t| t.clips.is_empty()))
            .collect();
        if emptied.is_empty() {
            return (edit, Vec::new());
        }
        let names = emptied
            .iter()
            .filter_map(|id| self.doc.timeline.track(*id).map(|t| t.name.clone()))
            .collect();
        let mut steps = vec![edit];
        steps.extend(emptied.into_iter().map(|track| Edit::RemoveTrack { track }));
        (Edit::Batch(steps), names)
    }

    pub fn undo(&mut self) {
        match self.history.undo(&mut self.doc.timeline) {
            Ok(true) => {
                self.mark_dirty();
                self.fix_selection();
            }
            Ok(false) => {}
            Err(e) => self.status = Some(format!("Cannot undo: {e}")),
        }
    }

    pub fn redo(&mut self) {
        match self.history.redo(&mut self.doc.timeline) {
            Ok(true) => {
                self.mark_dirty();
                self.fix_selection();
            }
            Ok(false) => {}
            Err(e) => self.status = Some(format!("Cannot redo: {e}")),
        }
    }

    fn fix_selection(&mut self) {
        if let Some(id) = self.selected {
            if self.doc.timeline.find_clip(id).is_none() {
                self.selected = None;
            }
        }
        let timeline = &self.doc.timeline;
        self.multi.retain(|id| timeline.find_clip(*id).is_some());
    }

    /// The kind of a clip: generated caption, other text, or by its media.
    pub fn clip_kind(&self, clip: ClipId) -> Option<ClipKind> {
        let (_, c) = self.doc.timeline.find_clip(clip)?;
        if c.text.is_some() {
            return Some(if c.is_caption() {
                ClipKind::Caption
            } else {
                ClipKind::Text
            });
        }
        Some(match self.media_kind_of(c.media) {
            MediaKind::Image => ClipKind::Image,
            MediaKind::Audio => ClipKind::Audio,
            _ => ClipKind::Video,
        })
    }

    /// The other clips a change to `clip` is shared with: the selected clips
    /// of the same kind, or, when `all_captions` is on and `clip` is a
    /// caption, every caption on the timeline.
    pub fn group_targets(&self, clip: ClipId, all_captions: bool) -> Vec<ClipId> {
        let Some(kind) = self.clip_kind(clip) else {
            return Vec::new();
        };
        let ids: Vec<ClipId> = if all_captions && kind == ClipKind::Caption {
            self.doc
                .timeline
                .tracks
                .iter()
                .flat_map(|t| t.clips.iter())
                .map(|c| c.id)
                .collect()
        } else if self.is_selected(clip) {
            self.selection()
        } else {
            Vec::new()
        };
        ids.into_iter()
            .filter(|id| *id != clip && self.clip_kind(*id) == Some(kind))
            .collect()
    }

    /// Remember a clip's attributes (look, position and size, sound level)
    /// to paste on clips of the same kind.
    pub fn copy_attributes(&mut self, clip: ClipId) -> bool {
        let Some(kind) = self.clip_kind(clip) else {
            return false;
        };
        let Some((_, c)) = self.doc.timeline.find_clip(clip) else {
            return false;
        };
        self.attributes = Some(AttributeCopy {
            kind,
            text: c.text_at(self.playhead),
            caption: c.caption.clone(),
            transform: c.transform_at(self.playhead),
            volume: c.volume,
        });
        true
    }

    /// The clips "Paste attributes" would change when used on `clip`: the
    /// selected clips of the copied kind (or just `clip`).
    fn paste_targets(&self, clip: ClipId) -> Vec<ClipId> {
        let Some(copy) = &self.attributes else {
            return Vec::new();
        };
        let mut ids = if self.is_selected(clip) {
            self.selection()
        } else {
            vec![clip]
        };
        ids.retain(|id| self.clip_kind(*id) == Some(copy.kind));
        ids
    }

    pub fn can_paste_attributes(&self, clip: ClipId) -> bool {
        !self.paste_targets(clip).is_empty()
    }

    /// Give the selected clips (or `clip`) the copied attributes, if they are
    /// of the same kind. One undo step. Returns how many clips changed.
    pub fn paste_attributes(&mut self, clip: ClipId) -> usize {
        let Some(copy) = self.attributes.clone() else {
            return 0;
        };
        let targets = self.paste_targets(clip);
        self.apply_attributes(&copy, &targets)
    }

    /// Give every other generated caption the look, position and size of
    /// `clip` (used when "Apply to all captions" is switched on). One undo
    /// step. Returns how many captions changed.
    pub fn match_all_captions(&mut self, clip: ClipId) -> usize {
        if self.clip_kind(clip) != Some(ClipKind::Caption) {
            return 0;
        }
        let Some((_, c)) = self.doc.timeline.find_clip(clip) else {
            return 0;
        };
        let copy = AttributeCopy {
            kind: ClipKind::Caption,
            text: c.text_at(self.playhead),
            caption: c.caption.clone(),
            transform: c.transform_at(self.playhead),
            volume: c.volume,
        };
        let targets = self.group_targets(clip, true);
        self.apply_attributes(&copy, &targets)
    }

    fn apply_attributes(&mut self, copy: &AttributeCopy, targets: &[ClipId]) -> usize {
        let mut steps = Vec::new();
        let mut changed = 0;
        for &id in targets {
            let Some((_, c)) = self.doc.timeline.find_clip(id) else {
                continue;
            };
            let before = steps.len();
            if let (Some(look), Some(own)) = (&copy.text, &c.text) {
                let mut spec = look.clone();
                spec.content = own.content.clone();
                if c.words.is_empty() {
                    spec.highlight = own.highlight;
                }
                if spec.clone().clamped() != *own {
                    steps.push(Edit::SetClipText {
                        clip: id,
                        text: Some(spec.clamped()),
                    });
                }
                if copy.caption.is_some() && c.caption != copy.caption {
                    steps.push(Edit::SetClipCaption {
                        clip: id,
                        caption: copy.caption.clone(),
                        off: false,
                    });
                }
            }
            if self.can_transform(id) && copy.transform != c.transform {
                steps.push(Edit::SetClipTransform {
                    clip: id,
                    transform: copy.transform,
                });
            }
            if copy.kind != ClipKind::Caption
                && copy.kind != ClipKind::Text
                && copy.volume != c.volume
            {
                steps.push(Edit::SetClipVolume {
                    clip: id,
                    volume: copy.volume,
                });
            }
            if steps.len() > before {
                changed += 1;
            }
        }
        self.restore_text_baseline();
        let ok = match steps.len() {
            0 => false,
            1 => self.exec(steps.remove(0)),
            _ => self.exec(Edit::Batch(steps)),
        };
        if ok {
            changed
        } else {
            0
        }
    }

    /// Every selected clip that exists, leftmost first.
    pub fn selection(&self) -> Vec<ClipId> {
        let mut ids: Vec<ClipId> = match self.selected {
            Some(s) if self.multi.contains(&s) => self.multi.iter().copied().collect(),
            Some(s) => vec![s],
            None => Vec::new(),
        };
        ids.retain(|id| self.doc.timeline.find_clip(*id).is_some());
        ids.sort_by_key(|id| {
            self.doc
                .timeline
                .find_clip(*id)
                .map(|(_, c)| (c.start, c.id))
        });
        ids
    }

    pub fn is_selected(&self, id: ClipId) -> bool {
        match self.selected {
            Some(s) if self.multi.contains(&s) => self.multi.contains(&id),
            Some(s) => s == id,
            None => false,
        }
    }

    /// Select just this clip.
    pub fn select_only(&mut self, id: ClipId) {
        self.selected = Some(id);
        self.multi.clear();
    }

    /// Deselect everything.
    pub fn clear_selection(&mut self) {
        self.selected = None;
        self.multi.clear();
    }

    /// Select exactly these clips (the leftmost becomes the one shown in the inspector).
    pub fn select_many(&mut self, ids: &[ClipId]) {
        self.multi = ids.iter().copied().collect();
        let first = self
            .multi
            .iter()
            .copied()
            .filter_map(|id| {
                self.doc
                    .timeline
                    .find_clip(id)
                    .map(|(_, c)| (c.start, c.id))
            })
            .min();
        self.selected = first.map(|(_, id)| id);
        if self.selected.is_none() {
            self.multi.clear();
        }
    }

    /// Ctrl/Shift-click: add the clip to the selection, or take it out.
    pub fn toggle_selected(&mut self, id: ClipId) {
        let mut ids = self.selection();
        if let Some(i) = ids.iter().position(|c| *c == id) {
            ids.remove(i);
        } else {
            ids.push(id);
        }
        self.select_many(&ids);
    }

    pub fn selected_clip(&self) -> Option<(&Track, &Clip)> {
        self.selected.and_then(|id| self.doc.timeline.find_clip(id))
    }

    pub fn import_files(&mut self, paths: &[PathBuf]) -> Vec<MediaId> {
        let mut added = Vec::new();
        for path in paths {
            match self
                .folder
                .import_media(&mut self.doc, path, self.import_mode)
            {
                Ok(id) => {
                    self.mark_dirty();
                    added.push(id);
                }
                Err(e) => self.status = Some(format!("Import failed for {}: {e}", path.display())),
            }
        }
        self.refresh_missing();
        added
    }

    pub fn refresh_missing(&mut self) {
        self.missing = self.folder.find_missing(&self.doc).into_iter().collect();
    }

    pub fn media_duration(&self, id: MediaId) -> Ticks {
        self.doc
            .media_item(id)
            .ok()
            .and_then(|m| m.duration)
            .unwrap_or_else(|| Ticks::from_seconds_f64(PLACEHOLDER_MEDIA_SECONDS))
    }

    /// The real duration, once Phase 3 has probed the file. `None` until then.
    pub fn known_media_duration(&self, id: MediaId) -> Option<Ticks> {
        self.doc.media_item(id).ok().and_then(|m| m.duration)
    }

    /// Video, audio or image: the extension decides images, otherwise the
    /// probed streams decide, falling back to the extension until probed.
    pub fn media_kind_of(&self, id: MediaId) -> MediaKind {
        let Ok(item) = self.doc.media_item(id) else {
            return MediaKind::Video;
        };
        let by_extension = match &item.path {
            MediaPath::Relative(p) | MediaPath::Absolute(p) => MediaKind::from_path(p),
        };
        if by_extension == MediaKind::Image {
            return MediaKind::Image;
        }
        match &item.probe {
            Some(p) if p.has_video => MediaKind::Video,
            Some(p) if p.has_audio => MediaKind::Audio,
            _ => by_extension,
        }
    }

    /// Record what reading the file revealed (its real length and streams).
    pub fn apply_probe(&mut self, id: MediaId, duration: Option<Ticks>, probe: MediaProbe) {
        if self.doc.set_media_probe(id, duration, probe).is_ok() {
            self.mark_dirty();
        }
    }

    pub fn media_path(&self, id: MediaId) -> Option<PathBuf> {
        let item = self.doc.media_item(id).ok()?;
        self.folder.resolve(&item.path).ok()
    }

    /// Place media on the first unlocked track of the right kind (audio on an
    /// audio track, video and images on a video track) at the playhead, or
    /// after the last clip if the playhead position is taken.
    pub fn add_media_to_timeline(&mut self, media: MediaId) -> Option<ClipId> {
        let duration = self.media_duration(media);
        let wanted = self.media_kind_of(media).track_kind();
        let Some(track) = self
            .doc
            .timeline
            .tracks
            .iter()
            .rev() // lowest track first, like the default "Video 1"
            .find(|t| t.kind == wanted && !t.locked)
        else {
            self.status = Some(match wanted {
                TrackKind::Video => "No unlocked video track to add to".into(),
                TrackKind::Audio => "No unlocked audio track to add to".into(),
            });
            return None;
        };
        let track_id = track.id;
        let mut start = self.playhead;
        let taken = track
            .clips
            .iter()
            .any(|c| c.start < start + duration && start < c.end());
        if taken {
            start = track.clips.last().map(Clip::end).unwrap_or(Ticks::ZERO);
        }
        self.add_media_at(media, track_id, start)
    }

    // ----- library items: usage, rename, relink, remove ---------------------

    /// How many clips on the timeline use this media.
    pub fn media_usage(&self, id: MediaId) -> usize {
        self.doc
            .timeline
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| c.media == id)
            .count()
    }

    /// Where this media's waveform is cached (named after its fingerprint, so
    /// it survives moving or renaming the file).
    pub fn waveform_cache_path(&self, id: MediaId) -> Option<PathBuf> {
        let item = self.doc.media_item(id).ok()?;
        let key = match &item.hash {
            Some(hash) => hash.chars().take(24).collect::<String>(),
            None => format!("media-{}", id.0),
        };
        Some(
            self.folder
                .cache_dir(&self.doc)
                .join("waveforms")
                .join(format!("{key}.peaks")),
        )
    }

    /// Where this media's proxy lives (named after its fingerprint).
    pub fn proxy_path(&self, id: MediaId) -> Option<PathBuf> {
        let item = self.doc.media_item(id).ok()?;
        let key = match &item.hash {
            Some(hash) => hash.chars().take(24).collect::<String>(),
            None => format!("media-{}", id.0),
        };
        Some(
            self.folder
                .proxy_dir(&self.doc)
                .join(format!("{key}-proxy.webm")),
        )
    }

    /// The proxy file, if one has been made.
    pub fn existing_proxy(&self, id: MediaId) -> Option<PathBuf> {
        self.proxy_path(id).filter(|p| p.is_file())
    }

    /// Videos (not audio or pictures) can have a proxy.
    pub fn can_have_proxy(&self, id: MediaId) -> bool {
        self.media_kind_of(id) == MediaKind::Video
            && self
                .doc
                .media_item(id)
                .ok()
                .and_then(|m| m.probe.as_ref())
                .is_some_and(|p| p.has_video && p.width > 0 && p.height > 0)
    }

    /// What the preview should play for this media: its proxy when proxies are
    /// wanted and one exists, otherwise the file itself.
    pub fn preview_path(&self, id: MediaId, use_proxies: bool) -> Option<PathBuf> {
        if use_proxies {
            if let Some(proxy) = self.existing_proxy(id) {
                return Some(proxy);
            }
        }
        self.media_path(id)
    }

    /// The file is gone from where the project expects it.
    pub fn media_is_missing(&self, id: MediaId) -> bool {
        self.media_path(id).is_none_or(|p| !p.is_file())
    }

    /// The file is stored inside the project folder (so the project owns it).
    pub fn media_in_project(&self, id: MediaId) -> bool {
        self.media_path(id)
            .is_some_and(|p| self.folder.contains(&p))
    }

    /// Give the library item a display name; blank restores the file name.
    pub fn rename_media(&mut self, id: MediaId, name: &str) {
        if self.doc.rename_media(id, Some(name.to_owned())).is_ok() {
            self.mark_dirty();
        }
    }

    /// Point the item at another file. The clips keep their timing.
    pub fn relink_media(&mut self, id: MediaId, file: &Path) -> Result<(), String> {
        self.folder
            .relink_one(&mut self.doc, id, file)
            .map_err(|e| e.to_string())?;
        self.mark_dirty();
        Ok(())
    }

    /// Take the media out of the project, together with every clip that uses
    /// it. The file stays on disk unless `delete_file` is set, and only files
    /// the project itself stores may be deleted. Removing clips can't be
    /// undone (the library item they would come back to is gone).
    /// Returns how many clips were removed.
    pub fn remove_media(&mut self, id: MediaId, delete_file: bool) -> Result<usize, String> {
        self.doc.media_item(id).map_err(|e| e.to_string())?;
        let file = self.media_path(id);
        if delete_file && !self.media_in_project(id) {
            return Err("Only files stored inside the project can be deleted from disk".into());
        }
        let clips: Vec<ClipId> = self
            .doc
            .timeline
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| c.media == id)
            .map(|c| c.id)
            .collect();
        if !clips.is_empty() {
            let steps = clips
                .iter()
                .map(|c| Edit::RemoveClip { clip: *c })
                .collect();
            if !self.exec(Edit::Batch(steps)) {
                return Err("The clips using this media could not be removed".into());
            }
            self.history.clear();
        }
        self.doc.remove_media(id).map_err(|e| e.to_string())?;
        self.mark_dirty();
        self.fix_selection();
        if delete_file {
            if let Some(file) = file {
                std::fs::remove_file(&file).map_err(|e| {
                    format!("Removed from the project, but the file wasn't deleted: {e}")
                })?;
            }
        }
        Ok(clips.len())
    }

    /// Select the first clip using this media and put the playhead on it.
    pub fn find_media_in_timeline(&mut self, id: MediaId) -> bool {
        let found = self
            .doc
            .timeline
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| c.media == id)
            .min_by_key(|c| c.start)
            .map(|c| (c.id, c.start));
        match found {
            Some((clip, start)) => {
                self.selected = Some(clip);
                self.set_playhead(start);
                true
            }
            None => false,
        }
    }

    /// Whether the file has sound. Unknown (not probed yet) counts as yes.
    pub fn media_has_audio(&self, id: MediaId) -> bool {
        match self.media_kind_of(id) {
            MediaKind::Audio => true,
            MediaKind::Image => false,
            MediaKind::Video => self
                .doc
                .media_item(id)
                .ok()
                .and_then(|m| m.probe.as_ref())
                .is_none_or(|p| p.has_audio),
        }
    }

    /// Audio files go on audio tracks; video files on video tracks, or on an
    /// audio track as sound only (when they have sound); images on video tracks.
    pub fn can_place_on(&self, media: MediaId, track: TrackKind) -> bool {
        match (self.media_kind_of(media), track) {
            (MediaKind::Audio, TrackKind::Audio) => true,
            (MediaKind::Video, TrackKind::Video) => true,
            (MediaKind::Video, TrackKind::Audio) => self.media_has_audio(media),
            (MediaKind::Image, TrackKind::Video) => true,
            _ => false,
        }
    }

    /// Put media on a brand-new track (video above, audio below) at `start`,
    /// as a single undo step.
    pub fn add_media_on_new_track(
        &mut self,
        media: MediaId,
        kind: TrackKind,
        at_top: bool,
        start: Ticks,
    ) -> Option<ClipId> {
        let duration = self.media_duration(media);
        let index = new_track_index(&self.doc.timeline, kind, at_top);
        let track = self.new_named_track(kind);
        let track_id = track.id;
        let clip = self
            .doc
            .timeline
            .new_clip(media, Ticks::ZERO, start, duration);
        let id = clip.id;
        let edit = Edit::Batch(vec![
            Edit::InsertTrack { index, track },
            Edit::InsertClip {
                track: track_id,
                clip,
            },
        ]);
        self.exec(edit).then(|| {
            self.selected = Some(id);
            id
        })
    }

    /// Move a clip onto a brand-new track (of `kind`) at `start`, as one undo step.
    pub fn move_clip_to_new_track(
        &mut self,
        clip: ClipId,
        kind: TrackKind,
        at_top: bool,
        start: Ticks,
    ) -> bool {
        let Some((_, c)) = self.doc.timeline.find_clip(clip) else {
            return false;
        };
        let (duration, source_in) = (c.duration, c.source_in);
        let index = new_track_index(&self.doc.timeline, kind, at_top);
        let track = self.new_named_track(kind);
        let track_id = track.id;
        self.exec(Edit::Batch(vec![
            Edit::InsertTrack { index, track },
            Edit::SetClip {
                clip,
                track: track_id,
                start,
                duration,
                source_in,
            },
        ]))
    }

    fn new_named_track(&mut self, kind: TrackKind) -> cutroom_timeline::Track {
        let count = self
            .doc
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == kind)
            .count();
        let label = match kind {
            TrackKind::Video => format!("Video {}", count + 1),
            TrackKind::Audio => format!("Audio {}", count + 1),
        };
        self.doc.timeline.new_track(kind, label)
    }

    /// Copy the selected clip (nothing is changed).
    pub fn copy_selected(&self) -> Option<ClipCopy> {
        let (track, clip) = self.selected_clip()?;
        Some(ClipCopy {
            media: clip.media,
            source_in: clip.source_in,
            duration: clip.duration,
            kind: track.kind,
            source_track: Some(track.id),
            offset: Ticks::ZERO,
            transform: clip.transform,
            audio_muted: clip.audio_muted,
            disabled: clip.disabled,
            text: clip.text.clone(),
            words: clip.words.clone(),
            caption: clip.caption.clone(),
            caption_off: clip.caption_off,
            volume: clip.volume,
        })
    }

    /// Copy every selected clip, remembering how far apart they were.
    pub fn copy_selection(&self) -> Vec<ClipCopy> {
        let ids = self.selection();
        let first = ids
            .iter()
            .filter_map(|id| self.doc.timeline.find_clip(*id).map(|(_, c)| c.start))
            .min();
        let Some(first) = first else {
            return Vec::new();
        };
        ids.iter()
            .filter_map(|id| {
                let (track, clip) = self.doc.timeline.find_clip(*id)?;
                Some(ClipCopy {
                    media: clip.media,
                    source_in: clip.source_in,
                    duration: clip.duration,
                    kind: track.kind,
                    source_track: Some(track.id),
                    offset: clip.start - first,
                    transform: clip.transform,
                    audio_muted: clip.audio_muted,
                    disabled: clip.disabled,
                    text: clip.text.clone(),
                    words: clip.words.clone(),
                    caption: clip.caption.clone(),
                    caption_off: clip.caption_off,
                    volume: clip.volume,
                })
            })
            .collect()
    }

    /// Paste a group at the playhead as one undo step, keeping the spacing
    /// between the clips. Each goes back on its own track when there is room,
    /// otherwise on a new track (shared by clips that came from the same one).
    pub fn paste_group(&mut self, copies: &[ClipCopy]) -> Vec<ClipId> {
        if copies.is_empty() {
            return Vec::new();
        }
        let mut scratch = self.doc.timeline.clone();
        let mut steps: Vec<Edit> = Vec::new();
        let mut ids = Vec::new();
        let mut assigned: std::collections::HashMap<TrackId, TrackId> = Default::default();
        let mut order: Vec<&ClipCopy> = copies.iter().collect();
        order.sort_by_key(|c| c.offset);
        for copy in order {
            let start = self.playhead + copy.offset;
            let usable = |t: &&cutroom_timeline::Track| t.kind == copy.kind && !t.locked;
            let prefer = copy
                .source_track
                .map(|s| assigned.get(&s).copied().unwrap_or(s));
            let free = prefer
                .and_then(|id| scratch.track(id))
                .filter(usable)
                .or_else(|| scratch.tracks.iter().rev().find(usable))
                .filter(|t| {
                    !t.clips
                        .iter()
                        .any(|c| c.start < start + copy.duration && start < c.end())
                })
                .map(|t| t.id);
            let mut clip =
                self.doc
                    .timeline
                    .new_clip(copy.media, copy.source_in, start, copy.duration);
            clip.transform = copy.transform;
            clip.audio_muted = copy.audio_muted;
            clip.disabled = copy.disabled;
            clip.text = copy.text.clone();
            clip.words = copy.words.clone();
            clip.caption = copy.caption.clone();
            clip.caption_off = copy.caption_off;
            clip.volume = copy.volume;
            let id = clip.id;
            let mut local = Vec::new();
            let target = match free {
                Some(track) => {
                    local.push(Edit::InsertClip { track, clip });
                    track
                }
                None => {
                    let index = new_track_index(&scratch, copy.kind, copy.kind == TrackKind::Video);
                    let count = scratch
                        .tracks
                        .iter()
                        .filter(|t| t.kind == copy.kind)
                        .count();
                    let label = match copy.kind {
                        TrackKind::Video => format!("Video {}", count + 1),
                        TrackKind::Audio => format!("Audio {}", count + 1),
                    };
                    let track = self.doc.timeline.new_track(copy.kind, label);
                    let track_id = track.id;
                    local.push(Edit::InsertTrack { index, track });
                    local.push(Edit::InsertClip {
                        track: track_id,
                        clip,
                    });
                    track_id
                }
            };
            for e in local {
                if cutroom_timeline::apply(&mut scratch, e.clone()).is_err() {
                    return Vec::new();
                }
                steps.push(e);
            }
            if let Some(s) = copy.source_track {
                assigned.insert(s, target);
            }
            ids.push(id);
        }
        let edit = if steps.len() == 1 {
            steps.remove(0)
        } else {
            Edit::Batch(steps)
        };
        if self.exec(edit) {
            self.select_many(&ids);
            ids
        } else {
            Vec::new()
        }
    }

    /// Slide these clips by the same amount (each stays on its own track), as
    /// one undo step.
    pub fn move_clips_by(&mut self, clip: &ClipId, others: &[ClipId], delta: Ticks) -> bool {
        let mut members: Vec<ClipId> = std::iter::once(*clip)
            .chain(others.iter().copied())
            .collect();
        let start_of = |tl: &cutroom_timeline::Timeline, id: &ClipId| {
            tl.find_clip(*id)
                .map(|(_, c)| c.start)
                .unwrap_or(Ticks::ZERO)
        };
        members.sort_by_key(|id| start_of(&self.doc.timeline, id));
        if delta > Ticks::ZERO {
            members.reverse(); // the right-most moves first so nobody lands on a neighbour
        }
        let mut scratch = self.doc.timeline.clone();
        let mut steps = Vec::new();
        for id in members {
            let Some((track, c)) = scratch.find_clip(id) else {
                continue;
            };
            let (track_id, new_start) = (track.id, c.start + delta);
            match scratch.move_edit(id, track_id, new_start) {
                Ok(edit) => {
                    if cutroom_timeline::apply(&mut scratch, edit.clone()).is_err() {
                        self.status = Some("Clips can't overlap, or a track is locked".into());
                        return false;
                    }
                    steps.push(edit);
                }
                Err(e) => {
                    self.status = Some(e.to_string());
                    return false;
                }
            }
        }
        if steps.is_empty() {
            return false;
        }
        self.exec(Edit::Batch(steps))
    }

    /// Paste at the playhead. If the playhead is resting on something where
    /// the pasted clip would go, a new track is created for it; otherwise it
    /// goes into the existing track.
    pub fn paste_clip(&mut self, copy: &ClipCopy) -> Option<ClipId> {
        self.paste_group(std::slice::from_ref(copy))
            .first()
            .copied()
    }

    /// Paste a whole media file at the playhead (same rule as [`Session::paste_clip`]).
    pub fn paste_media(&mut self, media: MediaId) -> Option<ClipId> {
        let kind = self.media_kind_of(media).track_kind();
        let duration = self.media_duration(media);
        self.paste_spec(media, Ticks::ZERO, duration, kind, None, None)
    }

    fn paste_spec(
        &mut self,
        media: MediaId,
        source_in: Ticks,
        duration: Ticks,
        kind: TrackKind,
        prefer: Option<TrackId>,
        text: Option<cutroom_timeline::TextSpec>,
    ) -> Option<ClipId> {
        let start = self.playhead;
        self.place_spec_with(
            media,
            source_in,
            duration,
            kind,
            prefer,
            start,
            Vec::new(),
            text,
        )
    }

    /// Put a clip at `start` on `prefer` (or the default track of `kind`) if it
    /// fits there, otherwise on a new track. `before` edits run first, in the
    /// same undo step.
    #[allow(clippy::too_many_arguments)]
    fn place_spec(
        &mut self,
        media: MediaId,
        source_in: Ticks,
        duration: Ticks,
        kind: TrackKind,
        prefer: Option<TrackId>,
        start: Ticks,
        before: Vec<Edit>,
    ) -> Option<ClipId> {
        self.place_spec_with(
            media, source_in, duration, kind, prefer, start, before, None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn place_spec_with(
        &mut self,
        media: MediaId,
        source_in: Ticks,
        duration: Ticks,
        kind: TrackKind,
        prefer: Option<TrackId>,
        start: Ticks,
        before: Vec<Edit>,
        text: Option<cutroom_timeline::TextSpec>,
    ) -> Option<ClipId> {
        let free_track = {
            let tl = &self.doc.timeline;
            let usable = |t: &&cutroom_timeline::Track| t.kind == kind && !t.locked;
            prefer
                .and_then(|id| tl.track(id))
                .filter(usable)
                // Words belong on top: never fall back to the lowest track.
                .or_else(|| {
                    tl.tracks
                        .iter()
                        .rev()
                        .find(usable)
                        .filter(|_| text.is_none())
                })
                .filter(|t| {
                    !t.clips
                        .iter()
                        .any(|c| c.start < start + duration && start < c.end())
                })
                .map(|t| t.id)
        };
        let mut clip = self
            .doc
            .timeline
            .new_clip(media, source_in, start, duration);
        clip.text = text;
        let id = clip.id;
        let mut steps = before;
        match free_track {
            Some(track) => steps.push(Edit::InsertClip { track, clip }),
            None => {
                // Video layers go on top; audio tracks at the bottom.
                let index = new_track_index(&self.doc.timeline, kind, kind == TrackKind::Video);
                let track = self.new_named_track(kind);
                let track_id = track.id;
                steps.push(Edit::InsertTrack { index, track });
                steps.push(Edit::InsertClip {
                    track: track_id,
                    clip,
                });
            }
        }
        let edit = if steps.len() == 1 {
            steps.remove(0)
        } else {
            Edit::Batch(steps)
        };
        self.exec(edit).then(|| {
            self.selected = Some(id);
            id
        })
    }

    // ----- picture placement ------------------------------------------------------

    /// The clip's current transform.
    pub fn clip_transform(&self, clip: ClipId) -> Option<cutroom_timeline::Transform> {
        self.doc.timeline.find_clip(clip).map(|(_, c)| c.transform)
    }

    /// Can this clip's picture be moved, turned or faded? (Clips on video tracks.)
    pub fn can_transform(&self, clip: ClipId) -> bool {
        self.doc.timeline.find_clip(clip).is_some_and(|(t, c)| {
            t.kind == TrackKind::Video && self.media_kind_of(c.media) != MediaKind::Audio
        })
    }

    /// Set a clip's transform as one undo step. Nothing happens if it already is that.
    pub fn set_clip_transform(
        &mut self,
        clip: ClipId,
        transform: cutroom_timeline::Transform,
    ) -> bool {
        let wanted = transform.clamped();
        match self.clip_transform(clip) {
            Some(current) if current != wanted && self.can_transform(clip) => {
                self.exec(Edit::SetClipTransform { clip, transform })
            }
            _ => false,
        }
    }

    /// Deactivate or reactivate these clips as one undo step.
    pub fn set_clips_disabled(&mut self, ids: &[ClipId], disabled: bool) -> bool {
        let edits: Vec<Edit> = ids
            .iter()
            .filter(|id| {
                self.doc
                    .timeline
                    .find_clip(**id)
                    .is_some_and(|(_, c)| c.disabled != disabled)
            })
            .map(|&clip| Edit::SetClipDisabled { clip, disabled })
            .collect();
        match edits.len() {
            0 => false,
            1 => self.exec(edits.into_iter().next().unwrap()),
            _ => self.exec(Edit::Batch(edits)),
        }
    }

    /// V: deactivate the selected clips, or reactivate them if they are all
    /// already off.
    pub fn toggle_selected_disabled(&mut self) -> bool {
        let ids = self.selection();
        let all_off = !ids.is_empty()
            && ids.iter().all(|id| {
                self.doc
                    .timeline
                    .find_clip(*id)
                    .is_some_and(|(_, c)| c.disabled)
            });
        self.set_clips_disabled(&ids, !all_off)
    }

    // ----- keyframes ----------------------------------------------------------------

    /// Half a frame: how close a keyframe has to be to the playhead to count as "at" it.
    fn key_tolerance(&self) -> Ticks {
        Ticks(
            self.doc
                .settings
                .frame_rate
                .ticks_per_frame()
                .map_or(0, |t| t / 2),
        )
    }

    /// The clip's picture settings at the playhead (animated ones follow their keyframes).
    pub fn transform_at_playhead(&self, clip: ClipId) -> Option<cutroom_timeline::Transform> {
        self.doc
            .timeline
            .find_clip(clip)
            .map(|(_, c)| c.transform_at(self.playhead))
    }

    /// The playhead measured from the start of the clip, kept inside it.
    fn clip_time(&self, clip: ClipId) -> Option<Ticks> {
        let (_, c) = self.doc.timeline.find_clip(clip)?;
        Some((self.playhead - c.start).max(Ticks::ZERO).min(c.duration))
    }

    /// Does this setting have any keyframes?
    pub fn is_animated(&self, clip: ClipId, prop: cutroom_timeline::Prop) -> bool {
        self.doc
            .timeline
            .find_clip(clip)
            .is_some_and(|(_, c)| c.keyframes.iter().any(|k| k.prop == prop))
    }

    /// Is there a keyframe for this setting at the playhead?
    pub fn has_key_at_playhead(&self, clip: ClipId, prop: cutroom_timeline::Prop) -> bool {
        let Some((_, c)) = self.doc.timeline.find_clip(clip) else {
            return false;
        };
        let at = self.playhead - c.start;
        cutroom_timeline::keyframes::key_near(&c.keyframes, prop, at, self.key_tolerance())
            .is_some()
    }

    /// The diamond button: add a keyframe for these settings at the playhead,
    /// or take it away if they all already have one there (one undo step).
    pub fn toggle_keyframes(&mut self, clip: ClipId, props: &[cutroom_timeline::Prop]) -> bool {
        let Some((_, c)) = self.doc.timeline.find_clip(clip) else {
            return false;
        };
        if self.playhead < c.start || self.playhead >= c.end() {
            self.status = Some("Move the playhead onto the clip to add a keyframe".into());
            return false;
        }
        let tol = self.key_tolerance();
        let at = self.playhead - c.start;
        let all_there = props
            .iter()
            .all(|p| cutroom_timeline::keyframes::key_near(&c.keyframes, *p, at, tol).is_some());
        let shown = c.transform_at(self.playhead);
        let shown_text = c.text_at(self.playhead);
        let mut keys = c.keyframes.clone();
        for &prop in props {
            if all_there {
                if let Some(i) = cutroom_timeline::keyframes::key_near(&keys, prop, at, tol) {
                    keys.remove(i);
                }
            } else {
                let value = if prop.is_text() {
                    // Text settings only exist on text clips.
                    match shown_text.as_ref().and_then(|t| prop.get_text(t)) {
                        Some(v) => v,
                        None => continue,
                    }
                } else {
                    prop.get(&shown)
                };
                cutroom_timeline::keyframes::upsert(&mut keys, prop, at, value, tol);
            }
        }
        self.exec(Edit::SetClipKeyframes {
            clip,
            keyframes: keys,
        })
    }

    /// Change the clip's picture settings as the controls show them (at the
    /// playhead): a setting with keyframes gets a keyframe here, any other
    /// changes the clip's own value. One undo step.
    pub fn edit_transform(&mut self, clip: ClipId, new: cutroom_timeline::Transform) -> bool {
        self.edit_transform_for(clip, new, &[])
    }

    /// Like [`Session::edit_transform`], and every setting that this change
    /// altered is set the same way on the clips in `targets` (one undo step).
    pub fn edit_transform_for(
        &mut self,
        clip: ClipId,
        new: cutroom_timeline::Transform,
        targets: &[ClipId],
    ) -> bool {
        let Some(old) = self.transform_at_playhead(clip) else {
            return false;
        };
        let new = new.clamped();
        let mut steps = self.transform_steps(clip, new);
        for t in targets.iter().filter(|t| **t != clip) {
            let Some(cur) = self.transform_at_playhead(*t) else {
                continue;
            };
            let merged = cutroom_timeline::Transform {
                x: if new.x != old.x { new.x } else { cur.x },
                y: if new.y != old.y { new.y } else { cur.y },
                scale: if new.scale != old.scale {
                    new.scale
                } else {
                    cur.scale
                },
                rotation: if new.rotation != old.rotation {
                    new.rotation
                } else {
                    cur.rotation
                },
                opacity: if new.opacity != old.opacity {
                    new.opacity
                } else {
                    cur.opacity
                },
                flip_h: if new.flip_h != old.flip_h {
                    new.flip_h
                } else {
                    cur.flip_h
                },
                flip_v: if new.flip_v != old.flip_v {
                    new.flip_v
                } else {
                    cur.flip_v
                },
                skew_x: if new.skew_x != old.skew_x {
                    new.skew_x
                } else {
                    cur.skew_x
                },
                skew_y: if new.skew_y != old.skew_y {
                    new.skew_y
                } else {
                    cur.skew_y
                },
                persp_x: if new.persp_x != old.persp_x {
                    new.persp_x
                } else {
                    cur.persp_x
                },
                persp_y: if new.persp_y != old.persp_y {
                    new.persp_y
                } else {
                    cur.persp_y
                },
                // The bend is each clip's own: a group edit never carries it over.
                warp: cur.warp,
            };
            steps.extend(self.transform_steps(*t, merged));
        }
        match steps.len() {
            0 => false,
            1 => self.exec(steps.remove(0)),
            _ => self.exec(Edit::Batch(steps)),
        }
    }

    /// The edits that make `clip` show `new` at the playhead.
    fn transform_steps(&self, clip: ClipId, new: cutroom_timeline::Transform) -> Vec<Edit> {
        let Some((_, c)) = self.doc.timeline.find_clip(clip) else {
            return Vec::new();
        };
        if !self.can_transform(clip) {
            return Vec::new();
        }
        let new = new.clamped();
        let shown = c.transform_at(self.playhead);
        let tol = self.key_tolerance();
        let at = self.clip_time(clip).unwrap_or(Ticks::ZERO);
        let mut base = c.transform;
        let mut keys = c.keyframes.clone();
        base.flip_h = new.flip_h;
        base.flip_v = new.flip_v;
        for prop in cutroom_timeline::Prop::TRANSFORM
            .into_iter()
            .chain(cutroom_timeline::Prop::WARP)
        {
            if prop.get(&new) == prop.get(&shown) {
                continue;
            }
            if c.keyframes.iter().any(|k| k.prop == prop) {
                cutroom_timeline::keyframes::upsert(&mut keys, prop, at, prop.get(&new), tol);
            } else {
                prop.set(&mut base, prop.get(&new));
            }
        }
        let mut steps = Vec::new();
        if base != c.transform {
            steps.push(Edit::SetClipTransform {
                clip,
                transform: base,
            });
        }
        if keys != c.keyframes {
            steps.push(Edit::SetClipKeyframes {
                clip,
                keyframes: keys,
            });
        }
        steps
    }

    /// Change a text clip's words and look as the controls show them (at the
    /// playhead): letter size and spacing with keyframes get a keyframe here,
    /// everything else changes the clip's own text. One undo step, back to how
    /// the clip was before any live changes ([`Session::preview_clip_text`]).
    pub fn edit_text(&mut self, clip: ClipId, new: cutroom_timeline::TextSpec) -> bool {
        self.edit_text_for(clip, new, &[])
    }

    /// Like [`Session::edit_text`], and every look setting this change altered
    /// (not the words) is set the same way on the text clips in `targets`.
    pub fn edit_text_for(
        &mut self,
        clip: ClipId,
        new: cutroom_timeline::TextSpec,
        targets: &[ClipId],
    ) -> bool {
        // Put back what the clip said before the live changes.
        self.restore_text_baseline();
        let Some(old) = self
            .doc
            .timeline
            .find_clip(clip)
            .and_then(|(_, c)| c.text_at(self.playhead))
        else {
            return false;
        };
        let new = new.clamped();
        let mut steps = self.text_steps(clip, new.clone());
        for t in targets.iter().filter(|t| **t != clip) {
            let Some(cur) = self
                .doc
                .timeline
                .find_clip(*t)
                .and_then(|(_, c)| c.text_at(self.playhead))
            else {
                continue;
            };
            steps.extend(self.text_steps(*t, cur.with_changes(&old, &new)));
        }
        match steps.len() {
            0 => false,
            1 => self.exec(steps.remove(0)),
            _ => self.exec(Edit::Batch(steps)),
        }
    }

    /// The edits that make a text clip say and look like `new`.
    fn text_steps(&self, clip: ClipId, new: cutroom_timeline::TextSpec) -> Vec<Edit> {
        let Some((_, c)) = self.doc.timeline.find_clip(clip) else {
            return Vec::new();
        };
        let Some(base_now) = c.text.clone() else {
            return Vec::new();
        };
        let new = new.clamped();
        let shown = c.text_at(self.playhead).unwrap_or_else(|| base_now.clone());
        let tol = self.key_tolerance();
        let at = self.clip_time(clip).unwrap_or(Ticks::ZERO);
        let mut base = new.clone();
        // Keep the clip's own value for animated settings unless they have no keys.
        let mut keys = c.keyframes.clone();
        for prop in cutroom_timeline::Prop::TEXT {
            let (n, s) = (prop.get_text(&new), prop.get_text(&shown));
            let animated = c.keyframes.iter().any(|k| k.prop == prop);
            if animated {
                if let Some(v) = n.filter(|v| Some(*v) != s) {
                    cutroom_timeline::keyframes::upsert(&mut keys, prop, at, v, tol);
                }
                if let Some(v) = prop.get_text(&base_now) {
                    prop.set_text(&mut base, v);
                }
            }
        }
        let mut steps = Vec::new();
        // Caption words follow the text when it is edited.
        if !c.words.is_empty() && base.content != base_now.content {
            steps.push(Edit::SetClipWords {
                clip,
                words: cutroom_timeline::text::retime(&c.words, &base.content, c.duration),
            });
        }
        if Some(&base) != c.text.as_ref() {
            steps.push(Edit::SetClipText {
                clip,
                text: Some(base),
            });
        }
        if keys != c.keyframes {
            steps.push(Edit::SetClipKeyframes {
                clip,
                keyframes: keys,
            });
        }
        steps
    }

    /// Can this clip have a crossfade with the clip before it (do they touch)?
    pub fn can_crossfade(&self, clip: ClipId) -> bool {
        self.doc.timeline.locate_clip(clip).is_some_and(|(ti, ci)| {
            cutroom_timeline::transition::max_crossfade(&self.doc.timeline.tracks[ti], ci)
                > Ticks::ZERO
        })
    }

    /// The crossfade length a clip has now (zero for none).
    pub fn crossfade_of(&self, clip: ClipId) -> Ticks {
        self.doc
            .timeline
            .locate_clip(clip)
            .map_or(Ticks::ZERO, |(ti, ci)| {
                cutroom_timeline::transition::effective(&self.doc.timeline.tracks[ti], ci)
            })
    }

    /// The longest crossfade this clip can have.
    pub fn max_crossfade_of(&self, clip: ClipId) -> Ticks {
        self.doc
            .timeline
            .locate_clip(clip)
            .map_or(Ticks::ZERO, |(ti, ci)| {
                cutroom_timeline::transition::max_crossfade(&self.doc.timeline.tracks[ti], ci)
            })
    }

    /// Give `clip` a crossfade with the clip before it (zero removes it), as
    /// one undo step. The length is kept within what the two clips allow.
    pub fn set_clip_crossfade(&mut self, clip: ClipId, length: Ticks) -> bool {
        let max = self.max_crossfade_of(clip);
        if length > Ticks::ZERO && max == Ticks::ZERO {
            self.status = Some("A crossfade needs two clips that touch".into());
            return false;
        }
        let wanted = Ticks(length.0.clamp(0, max.0));
        let current = self
            .doc
            .timeline
            .find_clip(clip)
            .map_or(Ticks::ZERO, |(_, c)| c.crossfade);
        if wanted == current {
            return false;
        }
        self.exec(Edit::SetClipCrossfade {
            clip,
            crossfade: wanted,
        })
    }

    /// "Mirror": flip the picture left to right.
    pub fn mirror_clip(&mut self, clip: ClipId) -> bool {
        let Some(mut t) = self.transform_at_playhead(clip) else {
            return false;
        };
        t.flip_h = !t.flip_h;
        self.edit_transform(clip, t)
    }

    /// "Rotate": a quarter turn clockwise.
    pub fn rotate_clip(&mut self, clip: ClipId) -> bool {
        let Some(mut t) = self.transform_at_playhead(clip) else {
            return false;
        };
        t.rotation += 900;
        self.edit_transform(clip, t)
    }

    /// Can the sound of this clip be pulled out onto an audio track?
    pub fn can_extract_audio(&self, clip: ClipId) -> bool {
        let Some((track, c)) = self.doc.timeline.find_clip(clip) else {
            return false;
        };
        track.kind == TrackKind::Video
            && !c.audio_muted
            && self.media_kind_of(c.media) == MediaKind::Video
            && self.media_has_audio(c.media)
    }

    /// "Extract audio": the clip's sound becomes an audio clip (same moment,
    /// same part of the file) and the video clip goes silent, as one undo step.
    pub fn extract_audio(&mut self, clip: ClipId) -> Result<ClipId, String> {
        if !self.can_extract_audio(clip) {
            return Err("This clip has no sound to extract".into());
        }
        let (media, source_in, start, duration) = {
            let (_, c) = self.doc.timeline.find_clip(clip).ok_or("clip not found")?;
            (c.media, c.source_in, c.start, c.duration)
        };
        let id = self
            .place_spec(
                media,
                source_in,
                duration,
                TrackKind::Audio,
                None,
                start,
                vec![Edit::SetClipAudioMuted { clip, muted: true }],
            )
            .ok_or("The audio could not be placed")?;
        self.status = Some("Extracted the audio onto an audio track".into());
        Ok(id)
    }

    /// Place media at an exact track and time (used by drag and drop).
    // ----- text clips ---------------------------------------------------------

    /// How long a new text clip lasts.
    pub const TEXT_DEFAULT_SECONDS: f64 = 3.0;

    /// Put a text clip at the playhead, on the topmost free video track or a
    /// new track above the others, as one undo step. The new clip is selected.
    pub fn add_text_clip(&mut self, spec: cutroom_timeline::TextSpec) -> Option<ClipId> {
        let top = self
            .doc
            .timeline
            .tracks
            .iter()
            .find(|t| t.kind == TrackKind::Video && !t.locked)
            .map(|t| t.id);
        let start = self.playhead;
        let id = self.place_spec_with(
            MediaId(0),
            Ticks::ZERO,
            Ticks::from_seconds_f64(Self::TEXT_DEFAULT_SECONDS),
            TrackKind::Video,
            top,
            start,
            Vec::new(),
            Some(spec.clamped()),
        )?;
        self.status = Some("Added a text clip".into());
        Some(id)
    }

    /// What to transcribe: the selected clips, or everything with sound when
    /// nothing is selected. Muted tracks and clips and silent files are left out.
    pub fn caption_sources(&self) -> Vec<CaptionSource> {
        let chosen = self.selection();
        let mut out: Vec<CaptionSource> = Vec::new();
        for track in self.doc.timeline.tracks.iter().filter(|t| !t.muted) {
            for c in &track.clips {
                if c.text.is_some() || c.disabled || c.audio_muted {
                    continue;
                }
                if !chosen.is_empty() && !chosen.contains(&c.id) {
                    continue;
                }
                let has_audio = self
                    .doc
                    .media_item(c.media)
                    .ok()
                    .and_then(|m| m.probe.as_ref())
                    .map_or(track.kind == TrackKind::Audio, |p| p.has_audio);
                if !has_audio || self.missing.contains(&c.media) {
                    continue;
                }
                let Some(path) = self.media_path(c.media) else {
                    continue;
                };
                let source = CaptionSource {
                    path,
                    source_in: c.source_in.as_seconds_f64(),
                    duration: c.duration.as_seconds_f64(),
                    start: c.start.as_seconds_f64(),
                };
                // A picture clip and its separate sound would say the same words twice.
                if !out.iter().any(|o| o == &source) {
                    out.push(source);
                }
            }
        }
        out.sort_by(|a, b| a.start.total_cmp(&b.start));
        out
    }

    /// Does this clip make sound? Pictures, text and silent files do not.
    pub fn clip_has_sound(&self, clip: &cutroom_timeline::Clip, track: TrackKind) -> bool {
        if clip.text.is_some() || self.missing.contains(&clip.media) {
            return false;
        }
        if self.media_kind_of(clip.media) == MediaKind::Image {
            return false;
        }
        match self
            .doc
            .media_item(clip.media)
            .ok()
            .and_then(|m| m.probe.as_ref())
        {
            Some(p) => p.has_audio,
            None => track == TrackKind::Audio,
        }
    }

    /// Turn a text clip into a caption in the style `name` (or back into
    /// plain text with `None`). Turning it on gives it that style's look;
    /// the words, size of the box and position stay. One undo step.
    pub fn set_caption(&mut self, clip: ClipId, caption: Option<String>) -> bool {
        let Some((_, c)) = self.doc.timeline.find_clip(clip) else {
            return false;
        };
        let Some(text) = c.text.clone() else {
            return false;
        };
        let mut steps = Vec::new();
        let off = caption.is_none();
        if c.caption != caption || c.caption_off != off {
            steps.push(Edit::SetClipCaption {
                clip,
                caption: caption.clone(),
                off,
            });
        }
        if let Some(name) = caption.as_deref() {
            if let Some(style) = cutroom_timeline::caption_styles()
                .into_iter()
                .find(|s| s.name == name)
            {
                let mut spec = style.spec;
                spec.content = text.content.clone();
                // Plain text with no spoken words cannot light them up.
                if c.words.is_empty() {
                    spec.highlight = [0, 0, 0, 0];
                }
                if spec != text {
                    steps.push(Edit::SetClipText {
                        clip,
                        text: Some(spec.clamped()),
                    });
                }
            }
        }
        self.restore_text_baseline();
        match steps.len() {
            0 => false,
            1 => self.exec(steps.remove(0)),
            _ => self.exec(Edit::Batch(steps)),
        }
    }

    /// Give a caption the look of the style `name`, whether or not it
    /// already has that name. Words, position and timing stay.
    pub fn force_caption_style(&mut self, clip: ClipId, name: &str) -> bool {
        self.force_caption_style_for(clip, name, &[])
    }

    /// Like [`Session::force_caption_style`], for `clip` and the clips in
    /// `targets` (one undo step).
    pub fn force_caption_style_for(
        &mut self,
        clip: ClipId,
        name: &str,
        targets: &[ClipId],
    ) -> bool {
        let Some(style) = cutroom_timeline::caption_styles()
            .into_iter()
            .find(|s| s.name == name)
        else {
            return false;
        };
        let mut steps = Vec::new();
        let ids = std::iter::once(clip).chain(targets.iter().copied().filter(|t| *t != clip));
        for id in ids {
            let Some((_, c)) = self.doc.timeline.find_clip(id) else {
                continue;
            };
            let Some(text) = c.text.clone() else {
                continue;
            };
            let mut spec = style.spec.clone();
            spec.content = text.content.clone();
            if c.words.is_empty() {
                spec.highlight = [0, 0, 0, 0];
            }
            let spec = spec.clamped();
            if text != spec {
                steps.push(Edit::SetClipText {
                    clip: id,
                    text: Some(spec),
                });
            }
            if c.caption.as_deref() != Some(name) || c.caption_off {
                steps.push(Edit::SetClipCaption {
                    clip: id,
                    caption: Some(name.to_string()),
                    off: false,
                });
            }
        }
        if steps.is_empty() {
            return false;
        }
        self.restore_text_baseline();
        self.exec(Edit::Batch(steps))
    }

    /// Set a clip's sound level (thousandths; 1000 = as recorded). One undo step.
    pub fn set_clip_volume(&mut self, clip: ClipId, volume: i32) -> bool {
        let volume = volume.clamp(0, cutroom_timeline::MAX_VOLUME);
        match self.doc.timeline.find_clip(clip) {
            Some((_, c)) if c.volume != volume => self.exec(Edit::SetClipVolume { clip, volume }),
            _ => false,
        }
    }

    /// Move every keyframe at clip time `from` to clip time `to` (one undo
    /// step). Keys already at `to` for the same setting are replaced.
    pub fn move_keyframes(&mut self, clip: ClipId, from: Ticks, to: Ticks) -> bool {
        if from == to {
            return false;
        }
        let Some((_, c)) = self.doc.timeline.find_clip(clip) else {
            return false;
        };
        let to = to.clamp(Ticks::ZERO, c.duration);
        let moving: Vec<_> = c
            .keyframes
            .iter()
            .copied()
            .filter(|k| k.time == from)
            .collect();
        if moving.is_empty() {
            return false;
        }
        let mut keys: Vec<_> = c
            .keyframes
            .iter()
            .copied()
            .filter(|k| {
                k.time != from && !(k.time == to && moving.iter().any(|m| m.prop == k.prop))
            })
            .collect();
        keys.extend(
            moving
                .into_iter()
                .map(|k| cutroom_timeline::Keyframe { time: to, ..k }),
        );
        keys.sort_by_key(|k| (k.prop.order(), k.time));
        self.exec(Edit::SetClipKeyframes {
            clip,
            keyframes: keys,
        })
    }

    /// Put spoken captions on the timeline, one text clip per line, on a
    /// "Captions" track (made if needed) as a single undo step. Captions
    /// already on that track where the new ones go are replaced. `lines` are
    /// in seconds on the timeline. Returns how many were added.
    pub fn add_captions(
        &mut self,
        lines: &[cutroom_captions::lines::Line],
        style: &cutroom_timeline::TextSpec,
        style_name: &str,
    ) -> usize {
        use cutroom_timeline::{text::Word, Transform};
        let lines: Vec<_> = lines.iter().filter(|l| !l.text().is_empty()).collect();
        let (Some(first), Some(last)) = (lines.first(), lines.last()) else {
            return 0;
        };
        let (from, to) = (
            Ticks::from_seconds_f64(first.start.max(0.0)),
            Ticks::from_seconds_f64(last.end),
        );
        let existing = self
            .doc
            .timeline
            .tracks
            .iter()
            .find(|t| t.kind == TrackKind::Video && t.name == "Captions" && !t.locked)
            .map(|t| t.id);
        let mut steps = Vec::new();
        let track = match existing {
            Some(id) => {
                for c in &self
                    .doc
                    .timeline
                    .track(id)
                    .map(|t| t.clips.clone())
                    .unwrap_or_default()
                {
                    if c.start < to && from < c.end() {
                        steps.push(Edit::RemoveClip { clip: c.id });
                    }
                }
                id
            }
            None => {
                let track = self.doc.timeline.new_track(TrackKind::Video, "Captions");
                let id = track.id;
                let index = new_track_index(&self.doc.timeline, TrackKind::Video, true);
                steps.push(Edit::InsertTrack { index, track });
                id
            }
        };
        let mut added = 0;
        let mut first_id = None;
        let mut prev_end = Ticks::ZERO;
        for line in lines {
            let start = Ticks::from_seconds_f64(line.start.max(0.0)).max(prev_end);
            let end = Ticks::from_seconds_f64(line.end).max(start + Ticks::from_seconds_f64(0.2));
            let mut clip = self
                .doc
                .timeline
                .new_clip(MediaId(0), Ticks::ZERO, start, end - start);
            let words: Vec<Word> = line
                .words
                .iter()
                .enumerate()
                .map(|(i, w)| Word {
                    // The first word of a line carries no leading space.
                    text: if i == 0 {
                        w.text.trim_start().to_string()
                    } else {
                        w.text.clone()
                    },
                    start: (Ticks::from_seconds_f64(w.start) - start).max(Ticks::ZERO),
                    end: (Ticks::from_seconds_f64(w.end) - start).clamp(Ticks::ZERO, end - start),
                })
                .collect();
            let mut spec = style.clone();
            spec.content = cutroom_timeline::text::join_words(&words);
            clip.text = Some(spec.clamped());
            clip.words = words;
            clip.caption = Some(style_name.to_string());
            clip.caption_off = false;
            clip.transform = Transform {
                y: cutroom_timeline::text::CAPTION_Y,
                ..Transform::default()
            };
            first_id.get_or_insert(clip.id);
            prev_end = end;
            steps.push(Edit::InsertClip { track, clip });
            added += 1;
        }
        let edit = if steps.len() == 1 {
            steps.remove(0)
        } else {
            Edit::Batch(steps)
        };
        if !self.exec(edit) {
            return 0;
        }
        self.selected = first_id;
        self.status = Some(format!("Added {added} captions"));
        added
    }

    /// Every caption on the timeline as (start, end, text) in seconds, in
    /// order: what a subtitle file needs.
    pub fn caption_cues(&self) -> Vec<(f64, f64, String)> {
        let mut cues: Vec<_> = self
            .doc
            .timeline
            .tracks
            .iter()
            .filter(|t| !t.hidden)
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.is_caption() && !c.disabled)
            .filter_map(|c| {
                let text = c.text.as_ref()?.content.trim().to_string();
                (!text.is_empty())
                    .then(|| (c.start.as_seconds_f64(), c.end().as_seconds_f64(), text))
            })
            .collect();
        cues.sort_by(|a, b| a.0.total_cmp(&b.0));
        cues
    }

    /// The look of the first caption (its text style and where it sits), for
    /// caption files that carry a style.
    pub fn caption_look(&self) -> Option<(cutroom_timeline::TextSpec, cutroom_timeline::Transform)> {
        let mut clips: Vec<_> = self
            .doc
            .timeline
            .tracks
            .iter()
            .filter(|t| !t.hidden)
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.is_caption() && !c.disabled)
            .collect();
        clips.sort_by_key(|c| c.start);
        let clip = clips.first()?;
        Some((clip.text.clone()?, clip.transform))
    }

    /// The words and look of a text clip (None for other clips).
    pub fn clip_text(&self, clip: ClipId) -> Option<cutroom_timeline::TextSpec> {
        self.doc.timeline.find_clip(clip)?.1.text.clone()
    }

    /// Put back what a text clip said before live (unrecorded) changes.
    fn restore_text_baseline(&mut self) {
        if let Some((id, before)) = self.text_baseline.take() {
            if let Some((ti, ci)) = self.doc.timeline.locate_clip(id) {
                self.doc.timeline.tracks[ti].clips[ci].text = Some(before);
            }
        }
    }

    /// The Reset button of "Position & Size": every picture setting goes back
    /// to its default and its keyframes are removed. One undo step.
    pub fn reset_transform(&mut self, clip: ClipId) -> bool {
        let Some((_, c)) = self.doc.timeline.find_clip(clip) else {
            return false;
        };
        let keys: Vec<_> = c
            .keyframes
            .iter()
            .copied()
            .filter(|k| !k.prop.is_picture())
            .collect();
        let mut steps = Vec::new();
        if !c.transform.is_default() {
            steps.push(Edit::SetClipTransform {
                clip,
                transform: cutroom_timeline::Transform::default(),
            });
        }
        if keys.len() != c.keyframes.len() {
            steps.push(Edit::SetClipKeyframes {
                clip,
                keyframes: keys,
            });
        }
        match steps.len() {
            0 => false,
            1 => self.exec(steps.remove(0)),
            _ => self.exec(Edit::Batch(steps)),
        }
    }

    /// Put some of a clip's picture settings back to their defaults and drop
    /// their keyframes (the Reset buttons of the Morph section). One undo step.
    pub fn reset_props(&mut self, clip: ClipId, props: &[cutroom_timeline::Prop]) -> bool {
        let Some((_, c)) = self.doc.timeline.find_clip(clip) else {
            return false;
        };
        let plain = cutroom_timeline::Transform::default();
        let mut base = c.transform;
        for p in props {
            p.set(&mut base, p.get(&plain));
        }
        let keys: Vec<_> = c
            .keyframes
            .iter()
            .copied()
            .filter(|k| !props.contains(&k.prop))
            .collect();
        let mut steps = Vec::new();
        if base != c.transform {
            steps.push(Edit::SetClipTransform {
                clip,
                transform: base,
            });
        }
        if keys.len() != c.keyframes.len() {
            steps.push(Edit::SetClipKeyframes {
                clip,
                keyframes: keys,
            });
        }
        match steps.len() {
            0 => false,
            1 => self.exec(steps.remove(0)),
            _ => self.exec(Edit::Batch(steps)),
        }
    }

    /// The Reset buttons of a text clip's "Text" and "Effects" sections: the
    /// look goes back to the default (the words stay) and the section's
    /// keyframes are removed. One undo step.
    pub fn reset_text_section(&mut self, clip: ClipId, effects: bool) -> bool {
        self.restore_text_baseline();
        let Some((_, c)) = self.doc.timeline.find_clip(clip) else {
            return false;
        };
        let Some(mut spec) = c.text.clone() else {
            return false;
        };
        let d = cutroom_timeline::TextSpec::default();
        if effects {
            spec.outline = d.outline;
            spec.outline_color = d.outline_color;
            spec.shadow = d.shadow;
            spec.shadow_blur = d.shadow_blur;
            spec.shadow_color = d.shadow_color;
            spec.background = d.background;
            spec.padding = d.padding;
            spec.corner = d.corner;
            spec.outline_on = false;
            spec.shadow_on = false;
            spec.box_on = false;
        } else {
            spec.font = d.font;
            spec.size = d.size;
            spec.bold = d.bold;
            spec.italic = d.italic;
            spec.align = d.align;
            spec.color = d.color;
            spec.spacing = d.spacing;
        }
        let keys: Vec<_> = if effects {
            c.keyframes.clone()
        } else {
            c.keyframes
                .iter()
                .copied()
                .filter(|k| !k.prop.is_text())
                .collect()
        };
        let mut steps = Vec::new();
        if Some(&spec) != c.text.as_ref() {
            steps.push(Edit::SetClipText {
                clip,
                text: Some(spec),
            });
        }
        if keys.len() != c.keyframes.len() {
            steps.push(Edit::SetClipKeyframes {
                clip,
                keyframes: keys,
            });
        }
        match steps.len() {
            0 => false,
            1 => self.exec(steps.remove(0)),
            _ => self.exec(Edit::Batch(steps)),
        }
    }

    /// Show a change to a text clip's words or look right away without
    /// recording it. [`Session::set_clip_text`] records it afterwards.
    pub fn preview_clip_text(&mut self, clip: ClipId, spec: cutroom_timeline::TextSpec) {
        let Some((ti, ci)) = self.doc.timeline.locate_clip(clip) else {
            return;
        };
        let target = &mut self.doc.timeline.tracks[ti].clips[ci];
        if target.text.is_none() {
            return;
        }
        if self.text_baseline.as_ref().map(|(id, _)| *id) != Some(clip) {
            self.text_baseline = target.text.clone().map(|t| (clip, t));
        }
        target.text = Some(spec.clamped());
    }

    /// Change a text clip's words or look as one undo step. Nothing happens
    /// if the clip is not a text clip or the text is unchanged.
    pub fn set_clip_text(&mut self, clip: ClipId, spec: cutroom_timeline::TextSpec) -> bool {
        let spec = spec.clamped();
        // Put back what the clip said before the live changes, so the undo
        // step goes back to that.
        self.restore_text_baseline();
        match self.clip_text(clip) {
            Some(current) if current != spec => self.exec(Edit::SetClipText {
                clip,
                text: Some(spec),
            }),
            _ => false,
        }
    }

    pub fn add_media_at(&mut self, media: MediaId, track: TrackId, start: Ticks) -> Option<ClipId> {
        let kind = self.doc.timeline.track(track)?.kind;
        if !self.can_place_on(media, kind) {
            self.status = Some("That file can't go on this kind of track".into());
            return None;
        }
        let duration = self.media_duration(media);
        let clip = self
            .doc
            .timeline
            .new_clip(media, Ticks::ZERO, start, duration);
        let id = clip.id;
        if self.exec(Edit::InsertClip { track, clip }) {
            self.selected = Some(id);
            Some(id)
        } else {
            None
        }
    }

    pub fn split_selected(&mut self) {
        let Some(id) = self.selected else {
            self.status = Some("Select a clip to split".into());
            return;
        };
        match self.doc.timeline.split_edit(id, self.playhead) {
            Ok(edit) => {
                self.exec(edit);
            }
            Err(e) => self.status = Some(e.to_string()),
        }
    }

    /// Cut a specific clip at a specific time (used by the split tool).
    pub fn split_clip_at(&mut self, clip: ClipId, at: Ticks) {
        match self.doc.timeline.split_edit(clip, at) {
            Ok(edit) => {
                self.exec(edit);
            }
            Err(e) => self.status = Some(e.to_string()),
        }
    }

    /// Cut every clip under the playhead on every unlocked track, as one undo step.
    pub fn split_all_at_playhead(&mut self) {
        let at = self.playhead;
        let targets: Vec<ClipId> = self
            .doc
            .timeline
            .tracks
            .iter()
            .filter(|t| !t.locked)
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.start < at && at < c.end())
            .map(|c| c.id)
            .collect();
        if targets.is_empty() {
            self.status = Some("Nothing under the playhead to split".into());
            return;
        }
        let mut edits = Vec::with_capacity(targets.len());
        for id in targets {
            match self.doc.timeline.split_edit(id, at) {
                Ok(edit) => edits.push(edit),
                Err(e) => {
                    self.status = Some(e.to_string());
                    return;
                }
            }
        }
        self.exec(Edit::Batch(edits));
    }

    /// Remove the part of the selected clip before the playhead.
    pub fn delete_left_of_playhead(&mut self) {
        let Some((_, clip)) = self.selected_clip() else {
            self.status = Some("Select a clip first".into());
            return;
        };
        let (id, start, end) = (clip.id, clip.start, clip.end());
        if self.playhead <= start || self.playhead >= end {
            self.status = Some("Move the playhead inside the selected clip".into());
            return;
        }
        let playhead = self.playhead;
        self.trim_clip(id, playhead, end);
    }

    /// Remove the part of the selected clip after the playhead.
    pub fn delete_right_of_playhead(&mut self) {
        let Some((_, clip)) = self.selected_clip() else {
            self.status = Some("Select a clip first".into());
            return;
        };
        let (id, start, end) = (clip.id, clip.start, clip.end());
        if self.playhead <= start || self.playhead >= end {
            self.status = Some("Move the playhead inside the selected clip".into());
            return;
        }
        let playhead = self.playhead;
        self.trim_clip(id, start, playhead);
    }

    pub fn delete_selected(&mut self, ripple: bool) {
        let ids = self.selection();
        let edit = match ids.as_slice() {
            [] => return,
            [id] => {
                if ripple {
                    match self.doc.timeline.ripple_delete_edit(*id) {
                        Ok(edit) => edit,
                        Err(e) => {
                            self.status = Some(e.to_string());
                            return;
                        }
                    }
                } else {
                    Edit::RemoveClip { clip: *id }
                }
            }
            many => {
                // Right to left, each edit worked out on a scratch copy so that
                // closing gaps never disturbs a clip still to be removed.
                let mut scratch = self.doc.timeline.clone();
                let mut edits = Vec::new();
                for id in many.iter().rev() {
                    let edit = if ripple {
                        match scratch.ripple_delete_edit(*id) {
                            Ok(edit) => edit,
                            Err(e) => {
                                self.status = Some(e.to_string());
                                return;
                            }
                        }
                    } else {
                        Edit::RemoveClip { clip: *id }
                    };
                    if let Err(e) = cutroom_timeline::apply(&mut scratch, edit.clone()) {
                        self.status = Some(e.to_string());
                        return;
                    }
                    edits.push(edit);
                }
                Edit::Batch(edits)
            }
        };
        if self.exec(edit) {
            self.selected = None;
            self.multi.clear();
        }
    }

    pub fn move_clip(&mut self, clip: ClipId, track: TrackId, start: Ticks) -> bool {
        match self.doc.timeline.move_edit(clip, track, start) {
            Ok(edit) => self.exec(edit),
            Err(e) => {
                self.status = Some(e.to_string());
                false
            }
        }
    }

    pub fn trim_clip(&mut self, clip: ClipId, new_start: Ticks, new_end: Ticks) -> bool {
        match self.doc.timeline.trim_edit(clip, new_start, new_end) {
            Ok(edit) => self.exec(edit),
            Err(e) => {
                self.status = Some(e.to_string());
                false
            }
        }
    }

    pub fn toggle_track_muted(&mut self, track: TrackId) {
        self.set_track_flags(track, |muted, _| (!muted, None));
    }

    /// The eye button: hide a track (nothing on it shows or plays) or show it again.
    pub fn toggle_track_hidden(&mut self, track: TrackId) {
        if let Some(t) = self.doc.timeline.track(track) {
            let hidden = !t.hidden;
            self.exec(Edit::SetTrackHidden { track, hidden });
        }
    }

    pub fn toggle_track_locked(&mut self, track: TrackId) {
        self.set_track_flags(track, |_, locked| (false, Some(!locked)));
    }

    fn set_track_flags(
        &mut self,
        track: TrackId,
        change: impl Fn(bool, bool) -> (bool, Option<bool>),
    ) {
        let Some(current) = self.doc.timeline.track(track) else {
            return;
        };
        let (muted, locked) = (current.muted, current.locked);
        let (new_muted, new_locked) = change(muted, locked);
        // `change` returns Some(locked) only when the lock is what's being toggled.
        let (muted, locked) = match new_locked {
            Some(l) => (muted, l),
            None => (new_muted, locked),
        };
        self.exec(Edit::SetTrackFlags {
            track,
            muted,
            locked,
        });
    }

    pub fn add_track(&mut self, kind: TrackKind) {
        let count = self
            .doc
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == kind)
            .count();
        let (label, index) = match kind {
            // New video layers go on top, new audio tracks at the bottom.
            TrackKind::Video => (format!("Video {}", count + 1), 0),
            TrackKind::Audio => (
                format!("Audio {}", count + 1),
                self.doc.timeline.tracks.len(),
            ),
        };
        let track = self.doc.timeline.new_track(kind, label);
        self.exec(Edit::InsertTrack { index, track });
    }

    // ----- playback --------------------------------------------------------

    pub fn timeline_end(&self) -> Ticks {
        self.doc.timeline.duration()
    }

    pub fn toggle_play(&mut self) {
        if self.playing {
            self.playing = false;
            return;
        }
        let end = self.timeline_end();
        if end == Ticks::ZERO {
            return;
        }
        if self.playhead >= end {
            self.playhead = Ticks::ZERO;
        }
        self.playing = true;
    }

    /// Advance the playhead by `dt` seconds of wall-clock time.
    pub fn tick(&mut self, dt: f64) {
        if !self.playing {
            return;
        }
        self.playhead = self.playhead + Ticks::from_seconds_f64(dt);
        let end = self.timeline_end();
        if self.playhead >= end {
            self.playhead = end;
            self.playing = false;
        }
    }

    pub fn set_playhead(&mut self, t: Ticks) {
        self.playhead = t.max(Ticks::ZERO);
    }

    /// Pretend the last autosave was `by` ago (tests only).
    #[cfg(test)]
    pub(crate) fn backdate_autosave(&mut self, by: Duration) {
        self.last_autosave = Instant::now().checked_sub(by).unwrap_or_else(Instant::now);
    }
}

/// File name shown on clips and in the media list.
pub fn media_label(item: &MediaItem) -> String {
    if let Some(name) = item.name.as_deref().filter(|n| !n.is_empty()) {
        return name.to_owned();
    }
    let path = match &item.path {
        MediaPath::Relative(p) | MediaPath::Absolute(p) => p,
    };
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutroom_timeline::FrameRate;
    use tempfile::{tempdir, TempDir};

    /// A new project plus an empty "Video 2" overlay track on top, which most
    /// tests want (a brand-new project has only Video 1 and Audio 1).
    fn new_session() -> (TempDir, Session) {
        let (tmp, mut session) = bare_session();
        let track = session.doc.timeline.new_track(TrackKind::Video, "Video 2");
        apply(
            &mut session.doc.timeline,
            Edit::InsertTrack { index: 0, track },
        )
        .unwrap();
        (tmp, session)
    }

    fn bare_session() -> (TempDir, Session) {
        let tmp = tempdir().unwrap();
        let session = Session::create(
            &tmp.path().join("proj"),
            "demo",
            ProjectSettings::vertical(FrameRate::FPS_30),
        )
        .unwrap();
        (tmp, session)
    }

    fn import_one(tmp: &TempDir, session: &mut Session, name: &str) -> MediaId {
        let path = tmp.path().join(name);
        std::fs::write(&path, name.as_bytes()).unwrap();
        session.import_files(&[path])[0]
    }

    fn clip_starts(session: &Session) -> Vec<f64> {
        session
            .doc
            .timeline
            .tracks
            .iter()
            .find(|t| t.name == "Video 1")
            .unwrap()
            .clips
            .iter()
            .map(|c| c.start.as_seconds_f64())
            .collect()
    }

    #[test]
    fn new_project_has_only_video_1_and_audio_1_and_is_clean() {
        let (_tmp, session) = bare_session();
        let names: Vec<_> = session
            .doc
            .timeline
            .tracks
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, ["Video 1", "Audio 1"]);
        assert!(!session.is_dirty());
        assert!(
            !session.history.can_undo(),
            "starter tracks are not undoable"
        );
    }

    #[test]
    fn import_then_add_places_clip_at_playhead() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        session.set_playhead(Ticks::from_seconds_f64(2.0));
        let id = session.add_media_to_timeline(media).unwrap();
        assert_eq!(session.selected, Some(id));
        assert_eq!(clip_starts(&session), vec![2.0]);
        assert!(session.is_dirty());
    }

    #[test]
    fn adding_at_an_occupied_playhead_appends_after_the_last_clip() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        session.add_media_to_timeline(media).unwrap(); // 0..5
        session.set_playhead(Ticks::from_seconds_f64(1.0));
        session.add_media_to_timeline(media).unwrap(); // would overlap -> after
        assert_eq!(clip_starts(&session), vec![0.0, 5.0]);
    }

    #[test]
    fn split_then_undo_restores_one_clip() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        session.add_media_to_timeline(media).unwrap();
        session.set_playhead(Ticks::from_seconds_f64(2.0));
        session.split_selected();
        assert_eq!(clip_starts(&session), vec![0.0, 2.0]);
        session.undo();
        assert_eq!(clip_starts(&session), vec![0.0]);
        session.redo();
        assert_eq!(clip_starts(&session), vec![0.0, 2.0]);
    }

    #[test]
    fn split_outside_the_clip_reports_a_status_message() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        session.add_media_to_timeline(media).unwrap();
        session.set_playhead(Ticks::from_seconds_f64(9.0));
        session.split_selected();
        assert!(session.status.as_deref().unwrap().contains("split"));
        assert_eq!(clip_starts(&session), vec![0.0]);
    }

    #[test]
    fn ripple_delete_closes_the_gap() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let first = session.add_media_to_timeline(media).unwrap();
        session.add_media_to_timeline(media).unwrap();
        assert_eq!(clip_starts(&session), vec![0.0, 5.0]);
        session.selected = Some(first);
        session.delete_selected(true);
        assert_eq!(clip_starts(&session), vec![0.0]);
        assert_eq!(session.selected, None);
    }

    #[test]
    fn plain_delete_leaves_a_gap() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let first = session.add_media_to_timeline(media).unwrap();
        session.add_media_to_timeline(media).unwrap();
        session.selected = Some(first);
        session.delete_selected(false);
        assert_eq!(clip_starts(&session), vec![5.0]);
    }

    #[test]
    fn undo_that_removes_the_selected_clip_clears_selection() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        session.add_media_to_timeline(media).unwrap();
        assert!(session.selected.is_some());
        session.undo();
        assert_eq!(session.selected, None);
    }

    #[test]
    fn locked_track_blocks_edits_until_unlocked() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let clip = session.add_media_to_timeline(media).unwrap();
        let track = session.doc.timeline.find_clip(clip).unwrap().0.id;
        session.toggle_track_locked(track);
        session.delete_selected(false);
        assert_eq!(
            clip_starts(&session),
            vec![0.0],
            "delete rejected while locked"
        );
        session.toggle_track_locked(track);
        session.delete_selected(false);
        assert!(clip_starts(&session).is_empty());
    }

    #[test]
    fn muting_is_independent_of_locking() {
        let (_tmp, mut session) = new_session();
        let track = session.doc.timeline.tracks[0].id;
        session.toggle_track_muted(track);
        let t = session.doc.timeline.track(track).unwrap();
        assert!(t.muted && !t.locked);
        session.toggle_track_locked(track);
        let t = session.doc.timeline.track(track).unwrap();
        assert!(t.muted && t.locked);
    }

    #[test]
    fn new_video_tracks_go_on_top_and_audio_at_the_bottom() {
        let (_tmp, mut session) = new_session();
        session.add_track(TrackKind::Video);
        session.add_track(TrackKind::Audio);
        let names: Vec<_> = session
            .doc
            .timeline
            .tracks
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["Video 3", "Video 2", "Video 1", "Audio 1", "Audio 2"]
        );
    }

    #[test]
    fn save_and_reopen_keeps_edits_and_media() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        session.add_media_to_timeline(media).unwrap();
        session.save();
        assert!(!session.is_dirty());

        let reopened = Session::open(session.folder.root()).unwrap();
        assert_eq!(reopened.doc, session.doc);
        assert!(reopened.missing.is_empty());
    }

    #[test]
    fn missing_media_is_reported_after_reopen() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        session.save();
        std::fs::remove_file(tmp.path().join("a.mp4")).unwrap();
        let reopened = Session::open(session.folder.root()).unwrap();
        assert!(reopened.missing.contains(&media));
    }

    #[test]
    fn playback_runs_to_the_end_and_stops() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        session.add_media_to_timeline(media).unwrap(); // 5 s
        session.set_playhead(Ticks::ZERO);
        session.toggle_play();
        assert!(session.playing);
        session.tick(2.0);
        assert!(session.playing);
        session.tick(10.0);
        assert!(!session.playing);
        assert_eq!(session.playhead, session.timeline_end());
        session.toggle_play(); // at the end: restarts from zero
        assert!(session.playing && session.playhead == Ticks::ZERO);
    }

    #[test]
    fn empty_timeline_cannot_play() {
        let (_tmp, mut session) = new_session();
        session.toggle_play();
        assert!(!session.playing);
    }

    #[test]
    fn moving_the_project_keeps_it_openable() {
        let (tmp, mut session) = new_session();
        session.save();
        let dest = tmp.path().join("elsewhere/moved");
        session.move_project(&dest).unwrap();
        assert!(Session::open(&dest).is_ok());
        assert!(!tmp.path().join("proj").exists());
    }

    #[test]
    fn storage_dirs_mark_the_project_dirty() {
        let (tmp, mut session) = new_session();
        session.set_storage_dir(StorageKind::Proxy, Some(&tmp.path().join("scratch")));
        assert!(session.is_dirty());
        assert_eq!(
            session.folder.proxy_dir(&session.doc),
            tmp.path().join("scratch")
        );
        session.set_storage_dir(StorageKind::Proxy, None);
        assert_eq!(
            session.folder.proxy_dir(&session.doc),
            session.folder.root().join("proxies")
        );
    }

    #[test]
    fn media_extension_check_is_case_insensitive_and_needs_a_real_file() {
        let tmp = tempdir().unwrap();
        for (name, expected) in [
            ("a.MP4", true),
            ("b.mov", true),
            ("c.txt", false),
            ("noext", false),
        ] {
            let path = tmp.path().join(name);
            std::fs::write(&path, b"x").unwrap();
            assert_eq!(is_supported_media(&path), expected, "{name}");
        }
        assert!(!is_supported_media(&tmp.path().join("missing.mp4")));
        assert!(!is_supported_media(tmp.path()), "folders are not media");
    }

    #[test]
    fn add_media_at_places_exactly_and_rejects_overlap() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let track = session.doc.timeline.tracks[1].id; // Video 1
        let at = Ticks::from_seconds_f64(3.0);
        let id = session.add_media_at(media, track, at).unwrap();
        assert_eq!(session.doc.timeline.find_clip(id).unwrap().1.start, at);
        // 3..8 s is taken, so 4 s overlaps.
        assert!(session
            .add_media_at(media, track, Ticks::from_seconds_f64(4.0))
            .is_none());
        assert!(session.status.as_deref().unwrap().contains("overlap"));
    }

    #[test]
    fn split_all_cuts_every_unlocked_track_in_one_undo_step() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let v1 = session.doc.timeline.tracks[1].id;
        let v2 = session.doc.timeline.tracks[0].id;
        session.add_media_at(media, v1, Ticks::ZERO).unwrap();
        session.add_media_at(media, v2, Ticks::ZERO).unwrap();
        session.set_playhead(Ticks::from_seconds_f64(2.0));
        session.split_all_at_playhead();
        let counts: Vec<_> = session
            .doc
            .timeline
            .tracks
            .iter()
            .map(|t| t.clips.len())
            .collect();
        assert_eq!(counts, [2, 2, 0]);
        session.undo();
        let counts: Vec<_> = session
            .doc
            .timeline
            .tracks
            .iter()
            .map(|t| t.clips.len())
            .collect();
        assert_eq!(counts, [1, 1, 0], "one undo reverts both cuts");
    }

    #[test]
    fn split_all_skips_locked_tracks_and_empty_spots() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let v1 = session.doc.timeline.tracks[1].id;
        session.add_media_at(media, v1, Ticks::ZERO).unwrap();
        session.set_playhead(Ticks::from_seconds_f64(2.0));
        session.toggle_track_locked(v1);
        session.split_all_at_playhead();
        assert_eq!(session.doc.timeline.track(v1).unwrap().clips.len(), 1);
        assert!(session.status.as_deref().unwrap().contains("Nothing"));
    }

    #[test]
    fn delete_left_and_right_trim_the_selected_clip() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let id = session.add_media_to_timeline(media).unwrap(); // 0..5
        session.set_playhead(Ticks::from_seconds_f64(2.0));
        session.delete_left_of_playhead();
        {
            let (_, clip) = session.doc.timeline.find_clip(id).unwrap();
            assert_eq!(clip.start, Ticks::from_seconds_f64(2.0));
            assert_eq!(clip.source_in, Ticks::from_seconds_f64(2.0));
            assert_eq!(clip.end(), Ticks::from_seconds_f64(5.0));
        }
        session.set_playhead(Ticks::from_seconds_f64(4.0));
        session.delete_right_of_playhead();
        let (_, clip) = session.doc.timeline.find_clip(id).unwrap();
        assert_eq!(clip.end(), Ticks::from_seconds_f64(4.0));
        session.undo();
        session.undo();
        let (_, clip) = session.doc.timeline.find_clip(id).unwrap();
        assert_eq!(
            (clip.start, clip.end()),
            (Ticks::ZERO, Ticks::from_seconds_f64(5.0))
        );
    }

    #[test]
    fn delete_left_needs_the_playhead_inside_the_clip() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        session.add_media_to_timeline(media).unwrap();
        session.set_playhead(Ticks::from_seconds_f64(9.0));
        session.delete_left_of_playhead();
        assert!(session.status.as_deref().unwrap().contains("playhead"));
        session.selected = None;
        session.delete_right_of_playhead();
        assert!(session.status.as_deref().unwrap().contains("Select"));
    }

    #[test]
    fn split_clip_at_cuts_where_asked() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let id = session.add_media_to_timeline(media).unwrap();
        session.split_clip_at(id, Ticks::from_seconds_f64(1.5));
        assert_eq!(clip_starts(&session), vec![0.0, 1.5]);
    }

    #[test]
    fn durations_are_unknown_until_probed() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        assert_eq!(session.known_media_duration(media), None);
        session
            .doc
            .set_media_duration(media, Ticks::from_seconds_f64(12.0))
            .unwrap();
        assert_eq!(
            session.known_media_duration(media),
            Some(Ticks::from_seconds_f64(12.0))
        );
        assert_eq!(session.media_duration(media), Ticks::from_seconds_f64(12.0));
    }

    #[test]
    fn media_labels_use_the_file_name() {
        let item = |p: MediaPath| MediaItem {
            id: MediaId(1),
            path: p,
            hash: None,
            size: None,
            duration: None,
            probe: None,
            name: None,
        };
        assert_eq!(
            media_label(&item(MediaPath::Relative("media/a.mp4".into()))),
            "a.mp4"
        );
        assert_eq!(
            media_label(&item(MediaPath::Absolute("C:\\x\\b.mov".into()))),
            "b.mov"
        );
    }

    // ----- background autosave, closing, media kinds -----------------------

    fn wait_until_idle(session: &mut Session) {
        for _ in 0..500 {
            session.poll_autosave();
            if !session.autosave_running() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("background autosave did not finish");
    }

    fn dirty_session() -> (TempDir, Session) {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        session.add_media_to_timeline(media).unwrap();
        assert!(session.is_dirty());
        (tmp, session)
    }

    #[test]
    fn background_autosave_writes_the_project_and_a_backup_then_clears_dirty() {
        let (_tmp, mut session) = dirty_session();
        session.start_autosave();
        wait_until_idle(&mut session);
        assert!(!session.is_dirty());
        assert!(session.autosave_note().unwrap().starts_with("Autosaved"));

        let reopened = Session::open(session.folder.root()).unwrap();
        assert_eq!(
            reopened.doc.timeline, session.doc.timeline,
            "project.json itself was updated"
        );
        let backups = std::fs::read_dir(session.folder.root().join("autosave"))
            .unwrap()
            .count();
        assert_eq!(backups, 1);
    }

    #[test]
    fn autosave_waits_for_its_interval_and_skips_clean_projects() {
        let (_tmp, mut session) = dirty_session();
        let ten_minutes = Duration::from_secs(600);
        session.autosave_if_due(ten_minutes);
        assert!(!session.autosave_running(), "not due yet");

        session.backdate_autosave(Duration::from_secs(601));
        session.autosave_if_due(Duration::ZERO);
        assert!(!session.autosave_running(), "zero means autosave is off");

        session.autosave_if_due(ten_minutes);
        assert!(
            session.autosave_running() || !session.is_dirty(),
            "due, so it starts"
        );
        wait_until_idle(&mut session);
        assert!(!session.is_dirty());

        session.backdate_autosave(Duration::from_secs(601));
        session.autosave_if_due(ten_minutes);
        assert!(
            !session.autosave_running(),
            "nothing changed, nothing to save"
        );
    }

    #[test]
    fn edits_made_while_an_autosave_runs_stay_unsaved() {
        let (tmp, mut session) = dirty_session();
        session.start_autosave();
        // This edit happens after the snapshot was taken.
        let media = import_one(&tmp, &mut session, "b.mp4");
        session.add_media_to_timeline(media).unwrap();
        wait_until_idle(&mut session);
        assert!(session.is_dirty(), "the newer edit has not been saved yet");
        session.save();
        assert!(!session.is_dirty());
    }

    #[test]
    fn a_manual_save_waits_for_the_autosave_and_leaves_a_clean_project() {
        let (_tmp, mut session) = dirty_session();
        session.start_autosave();
        session.save();
        assert!(!session.autosave_running());
        assert!(!session.is_dirty());
    }

    #[test]
    fn closing_saves_unsaved_work() {
        let (_tmp, mut session) = dirty_session();
        session.close().unwrap();
        assert!(!session.is_dirty());
        let reopened = Session::open(session.folder.root()).unwrap();
        assert_eq!(reopened.doc.timeline.tracks[1].clips.len(), 1);
    }

    #[test]
    fn closing_while_an_autosave_runs_is_safe() {
        let (_tmp, mut session) = dirty_session();
        session.start_autosave();
        session.close().unwrap();
        assert!(!session.is_dirty());
        assert!(!session.autosave_running());
    }

    #[test]
    fn ago_text_reads_naturally() {
        assert_eq!(format_ago(Duration::from_secs(5)), "just now");
        assert_eq!(format_ago(Duration::from_secs(90)), "1 minute ago");
        assert_eq!(format_ago(Duration::from_secs(600)), "10 minutes ago");
        assert_eq!(format_ago(Duration::from_secs(3700)), "1 hour ago");
        assert_eq!(format_ago(Duration::from_secs(3 * 3600)), "3 hours ago");
    }

    #[test]
    fn media_kinds_come_from_the_extension() {
        for (name, kind) in [
            ("a.mp4", MediaKind::Video),
            ("b.MOV", MediaKind::Video),
            ("song.mp3", MediaKind::Audio),
            ("voice.WAV", MediaKind::Audio),
            ("photo.jpg", MediaKind::Image),
            ("logo.PNG", MediaKind::Image),
            ("weird", MediaKind::Video),
        ] {
            assert_eq!(MediaKind::from_path(name), kind, "{name}");
        }
        assert_eq!(MediaKind::Audio.track_kind(), TrackKind::Audio);
        assert_eq!(MediaKind::Image.track_kind(), TrackKind::Video);
    }

    #[test]
    fn audio_goes_to_the_audio_track_and_video_and_images_to_video() {
        let (tmp, mut session) = new_session();
        let song = import_one(&tmp, &mut session, "song.mp3");
        let clip = import_one(&tmp, &mut session, "clip.mp4");
        let photo = import_one(&tmp, &mut session, "photo.png");

        let a = session.add_media_to_timeline(song).unwrap();
        let v = session.add_media_to_timeline(clip).unwrap();
        session.set_playhead(Ticks::from_seconds_f64(20.0));
        let p = session.add_media_to_timeline(photo).unwrap();

        let kind_of = |id| session.doc.timeline.find_clip(id).unwrap().0.kind;
        assert_eq!(kind_of(a), TrackKind::Audio);
        assert_eq!(kind_of(v), TrackKind::Video);
        assert_eq!(kind_of(p), TrackKind::Video);
    }

    #[test]
    fn audio_cannot_be_added_without_an_unlocked_audio_track() {
        let (tmp, mut session) = new_session();
        let song = import_one(&tmp, &mut session, "song.mp3");
        let audio = session.doc.timeline.tracks[2].id;
        session.toggle_track_locked(audio);
        assert!(session.add_media_to_timeline(song).is_none());
        assert!(session.status.as_deref().unwrap().contains("audio track"));
    }

    #[test]
    fn folder_scans_find_nested_media_in_order_and_skip_the_rest() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("shoot");
        std::fs::create_dir_all(dir.join("day2/b-roll")).unwrap();
        for (name, _) in [
            ("a.mp4", 1),
            ("notes.txt", 2),
            ("day2/c.mov", 3),
            ("day2/b-roll/d.png", 4),
            ("day2/e.doc", 5),
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let found: Vec<String> = collect_media_files(&dir)
            .iter()
            .map(|p| {
                p.strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(found, ["a.mp4", "day2/b-roll/d.png", "day2/c.mov"]);
        assert!(collect_media_files(&tmp.path().join("missing")).is_empty());
    }

    #[test]
    fn folder_scans_are_capped() {
        let tmp = tempdir().unwrap();
        for i in 0..(MAX_FOLDER_IMPORT + 25) {
            std::fs::write(tmp.path().join(format!("{i:04}.mp4")), b"x").unwrap();
        }
        assert_eq!(collect_media_files(tmp.path()).len(), MAX_FOLDER_IMPORT);
    }

    #[test]
    fn probing_gives_the_real_duration_and_changes_the_kind() {
        let (tmp, mut session) = new_session();
        // A .mp4 with no video stream is audio.
        let id = import_one(&tmp, &mut session, "podcast.mp4");
        assert_eq!(
            session.media_kind_of(id),
            MediaKind::Video,
            "before probing, the extension decides"
        );
        session.apply_probe(
            id,
            Some(Ticks::from_seconds_f64(61.5)),
            MediaProbe {
                has_video: false,
                has_audio: true,
                ..Default::default()
            },
        );
        assert_eq!(session.media_kind_of(id), MediaKind::Audio);
        assert_eq!(session.media_duration(id), Ticks::from_seconds_f64(61.5));
        assert_eq!(
            session.known_media_duration(id),
            Some(Ticks::from_seconds_f64(61.5))
        );
    }

    #[test]
    fn images_stay_images_whatever_the_probe_says() {
        let (tmp, mut session) = new_session();
        let id = import_one(&tmp, &mut session, "logo.png");
        session.apply_probe(
            id,
            None,
            MediaProbe {
                has_video: true,
                width: 100,
                height: 100,
                ..Default::default()
            },
        );
        assert_eq!(session.media_kind_of(id), MediaKind::Image);
    }

    #[test]
    fn a_file_with_both_streams_is_video_and_the_probe_survives_saving() {
        let (tmp, mut session) = new_session();
        let id = import_one(&tmp, &mut session, "clip.mov");
        let probe = MediaProbe {
            has_video: true,
            has_audio: true,
            width: 1080,
            height: 1920,
            frame_rate: Some(cutroom_timeline::FrameRate::FPS_30),
        };
        session.apply_probe(id, Some(Ticks::from_seconds_f64(12.0)), probe.clone());
        assert_eq!(session.media_kind_of(id), MediaKind::Video);
        session.save();
        let reopened = Session::open(session.folder.root()).unwrap();
        assert_eq!(reopened.doc.media_item(id).unwrap().probe, Some(probe));
    }

    #[test]
    fn the_media_path_resolves_for_linked_and_missing_media() {
        let (tmp, mut session) = new_session();
        let id = import_one(&tmp, &mut session, "a.mp4");
        assert_eq!(session.media_path(id), Some(tmp.path().join("a.mp4")));
        assert_eq!(session.media_path(MediaId(999)), None);
    }

    // ----- kinds, new tracks, copy and paste --------------------------------

    fn probed(session: &mut Session, id: MediaId, video: bool, audio: bool, secs: f64) {
        session.apply_probe(
            id,
            Some(Ticks::from_seconds_f64(secs)),
            MediaProbe {
                has_video: video,
                has_audio: audio,
                ..Default::default()
            },
        );
    }

    #[test]
    fn video_files_may_go_on_audio_tracks_when_they_have_sound() {
        let (tmp, mut session) = new_session();
        let with_sound = import_one(&tmp, &mut session, "talk.mp4");
        let silent = import_one(&tmp, &mut session, "b-roll.mp4");
        let song = import_one(&tmp, &mut session, "song.mp3");
        let photo = import_one(&tmp, &mut session, "logo.png");
        probed(&mut session, with_sound, true, true, 5.0);
        probed(&mut session, silent, true, false, 5.0);
        probed(&mut session, song, false, true, 5.0);

        assert!(session.can_place_on(with_sound, TrackKind::Video));
        assert!(
            session.can_place_on(with_sound, TrackKind::Audio),
            "sound only"
        );
        assert!(
            !session.can_place_on(silent, TrackKind::Audio),
            "nothing to hear"
        );
        assert!(session.can_place_on(silent, TrackKind::Video));
        assert!(session.can_place_on(song, TrackKind::Audio));
        assert!(!session.can_place_on(song, TrackKind::Video));
        assert!(session.can_place_on(photo, TrackKind::Video));
        assert!(!session.can_place_on(photo, TrackKind::Audio));
        assert!(session.media_has_audio(with_sound) && !session.media_has_audio(silent));
    }

    #[test]
    fn unprobed_video_is_assumed_to_have_sound() {
        let (tmp, mut session) = new_session();
        let id = import_one(&tmp, &mut session, "fresh.mp4");
        assert!(session.media_has_audio(id));
    }

    #[test]
    fn new_track_positions_keep_video_above_audio() {
        let (_tmp, session) = new_session(); // Video 2, Video 1, Audio 1
        let tl = &session.doc.timeline;
        assert_eq!(new_track_index(tl, TrackKind::Video, true), 0);
        assert_eq!(
            new_track_index(tl, TrackKind::Video, false),
            2,
            "just above the audio tracks"
        );
        assert_eq!(
            new_track_index(tl, TrackKind::Audio, true),
            2,
            "first of the audio tracks"
        );
        assert_eq!(new_track_index(tl, TrackKind::Audio, false), 3);
    }

    #[test]
    fn media_on_a_new_track_is_one_undo_step() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let id = session
            .add_media_on_new_track(media, TrackKind::Video, true, Ticks::from_seconds_f64(1.0))
            .unwrap();
        let names: Vec<_> = session
            .doc
            .timeline
            .tracks
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, ["Video 3", "Video 2", "Video 1", "Audio 1"]);
        assert_eq!(
            session.doc.timeline.find_clip(id).unwrap().0.name,
            "Video 3"
        );
        session.undo();
        assert_eq!(
            session.doc.timeline.tracks.len(),
            3,
            "the track and the clip go together"
        );
        assert!(session.doc.timeline.find_clip(id).is_none());
    }

    #[test]
    fn moving_a_clip_to_a_new_track_is_one_undo_step() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let id = session.add_media_to_timeline(media).unwrap();
        assert!(session.move_clip_to_new_track(
            id,
            TrackKind::Video,
            false,
            Ticks::from_seconds_f64(2.0)
        ));
        let (track, clip) = session.doc.timeline.find_clip(id).unwrap();
        assert_eq!(
            (track.name.as_str(), clip.start),
            ("Video 3", Ticks::from_seconds_f64(2.0))
        );
        assert_eq!(
            session.doc.timeline.tracks[2].name, "Video 3",
            "above the audio track, below the other videos"
        );
        session.undo();
        assert_eq!(
            session.doc.timeline.find_clip(id).unwrap().0.name,
            "Video 1"
        );
        assert_eq!(session.doc.timeline.tracks.len(), 3);
    }

    #[test]
    fn pasting_where_there_is_room_uses_the_existing_track() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let first = session.add_media_to_timeline(media).unwrap(); // 0..5 s on Video 1
        session.selected = Some(first);
        let copy = session.copy_selected().unwrap();
        session.set_playhead(Ticks::from_seconds_f64(6.0)); // empty here
        let pasted = session.paste_clip(&copy).unwrap();
        assert_eq!(session.doc.timeline.tracks.len(), 3, "no new track");
        let (track, clip) = session.doc.timeline.find_clip(pasted).unwrap();
        assert_eq!(
            (track.name.as_str(), clip.start),
            ("Video 1", Ticks::from_seconds_f64(6.0))
        );
    }

    #[test]
    fn pasting_onto_a_clip_makes_a_new_track_on_top() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let first = session.add_media_to_timeline(media).unwrap();
        session.selected = Some(first);
        let copy = session.copy_selected().unwrap();
        session.set_playhead(Ticks::from_seconds_f64(2.0)); // resting on the clip
        let pasted = session.paste_clip(&copy).unwrap();
        assert_eq!(
            session.doc.timeline.tracks[0].name, "Video 3",
            "a new top layer"
        );
        let (track, clip) = session.doc.timeline.find_clip(pasted).unwrap();
        assert_eq!(
            (track.name.as_str(), clip.start),
            ("Video 3", Ticks::from_seconds_f64(2.0))
        );
        assert_eq!(
            session.doc.timeline.find_clip(first).unwrap().1.start,
            Ticks::ZERO,
            "the original is untouched"
        );
        session.undo();
        assert_eq!(
            session.doc.timeline.tracks.len(),
            3,
            "one undo removes the track and the clip"
        );
    }

    #[test]
    fn a_paste_that_would_run_into_a_later_clip_also_gets_a_new_track() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let first = session.add_media_to_timeline(media).unwrap(); // 0..5
        session.set_playhead(Ticks::from_seconds_f64(7.0));
        session.add_media_to_timeline(media).unwrap(); // 7..12
        session.selected = Some(first);
        let copy = session.copy_selected().unwrap(); // 5 s long
        session.set_playhead(Ticks::from_seconds_f64(5.5)); // free at 5.5, but 5.5..10.5 hits the 7..12 clip
        let pasted = session.paste_clip(&copy).unwrap();
        assert_eq!(
            session.doc.timeline.find_clip(pasted).unwrap().0.name,
            "Video 3"
        );
    }

    #[test]
    fn audio_pastes_get_a_new_track_at_the_bottom() {
        let (tmp, mut session) = new_session();
        let song = import_one(&tmp, &mut session, "song.mp3");
        session.add_media_to_timeline(song).unwrap();
        session.set_playhead(Ticks::from_seconds_f64(1.0));
        let pasted = session.paste_media(song).unwrap();
        let tracks = &session.doc.timeline.tracks;
        assert_eq!(tracks.last().unwrap().name, "Audio 2");
        assert_eq!(
            session.doc.timeline.find_clip(pasted).unwrap().0.name,
            "Audio 2"
        );
    }

    #[test]
    fn pasting_media_into_an_empty_timeline_uses_the_default_track() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        session.set_playhead(Ticks::from_seconds_f64(3.0));
        let id = session.paste_media(media).unwrap();
        assert_eq!(session.doc.timeline.tracks.len(), 3);
        assert_eq!(
            session.doc.timeline.find_clip(id).unwrap().0.name,
            "Video 1"
        );
    }

    #[test]
    fn pasting_skips_locked_tracks() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let v1 = session.doc.timeline.tracks[1].id;
        let v2 = session.doc.timeline.tracks[0].id;
        session.toggle_track_locked(v1);
        session.toggle_track_locked(v2);
        let id = session.paste_media(media).unwrap();
        assert_eq!(
            session.doc.timeline.tracks.len(),
            4,
            "every video track was locked, so a new one is made"
        );
        assert_eq!(
            session.doc.timeline.find_clip(id).unwrap().0.name,
            "Video 3"
        );
    }

    #[test]
    fn copying_needs_a_selection_and_keeps_the_original() {
        let (tmp, mut session) = new_session();
        assert!(session.copy_selected().is_none());
        let media = import_one(&tmp, &mut session, "a.mp4");
        let id = session.add_media_to_timeline(media).unwrap();
        let copy = session.copy_selected().unwrap();
        assert_eq!((copy.media, copy.kind), (media, TrackKind::Video));
        assert!(session.doc.timeline.find_clip(id).is_some());
    }

    // ----- empty tracks are removed ------------------------------------------

    fn names(session: &Session) -> Vec<String> {
        session
            .doc
            .timeline
            .tracks
            .iter()
            .map(|t| t.name.clone())
            .collect()
    }

    #[test]
    fn a_track_left_empty_by_an_edit_is_removed_with_it_and_undo_brings_both_back() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let id = session
            .add_media_on_new_track(media, TrackKind::Video, true, Ticks::ZERO)
            .unwrap();
        assert_eq!(
            names(&session),
            ["Video 3", "Video 2", "Video 1", "Audio 1"]
        );
        session.selected = Some(id);
        session.delete_selected(false);
        assert_eq!(
            names(&session),
            ["Video 2", "Video 1", "Audio 1"],
            "Video 3 went with its last clip"
        );
        assert!(session.status.as_deref().unwrap().contains("Video 3"));
        session.undo();
        assert_eq!(
            names(&session),
            ["Video 3", "Video 2", "Video 1", "Audio 1"]
        );
        assert!(
            session.doc.timeline.find_clip(id).is_some(),
            "one undo restores track and clip"
        );
    }

    #[test]
    fn the_default_tracks_stay_even_when_empty() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let song = import_one(&tmp, &mut session, "s.mp3");
        let v = session.add_media_to_timeline(media).unwrap();
        let a = session.add_media_to_timeline(song).unwrap();
        for id in [v, a] {
            session.selected = Some(id);
            session.delete_selected(false);
        }
        assert_eq!(names(&session), ["Video 2", "Video 1", "Audio 1"]);
    }

    #[test]
    fn moving_the_only_clip_off_a_track_removes_that_track() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let clip = session
            .add_media_on_new_track(media, TrackKind::Video, true, Ticks::ZERO)
            .unwrap(); // Video 3 holds it
        let video1 = session.doc.timeline.tracks[2].id;
        assert!(session.move_clip(clip, video1, Ticks::ZERO));
        assert_eq!(names(&session), ["Video 2", "Video 1", "Audio 1"]);
        assert_eq!(
            session.doc.timeline.find_clip(clip).unwrap().0.name,
            "Video 1"
        );
    }

    #[test]
    fn tracks_that_were_already_empty_are_left_alone() {
        let (tmp, mut session) = new_session();
        session.add_track(TrackKind::Audio); // an empty Audio 2 the user made
        let media = import_one(&tmp, &mut session, "a.mp4");
        let clip = session.add_media_to_timeline(media).unwrap();
        session.selected = Some(clip);
        session.delete_selected(false);
        assert!(
            names(&session).contains(&"Audio 2".to_string()),
            "{:?}",
            names(&session)
        );
    }

    // ----- library items ---------------------------------------------------

    #[test]
    fn media_usage_and_find_in_timeline() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        assert_eq!(session.media_usage(media), 0);
        assert!(!session.find_media_in_timeline(media));
        session.add_media_to_timeline(media).unwrap();
        session.set_playhead(Ticks::from_seconds_f64(8.0));
        let second = session.add_media_to_timeline(media).unwrap();
        assert_eq!(session.media_usage(media), 2);
        session.selected = None;
        assert!(session.find_media_in_timeline(media));
        assert_ne!(
            session.selected,
            Some(second),
            "the first one on the timeline"
        );
        assert_eq!(session.playhead, Ticks::ZERO);
    }

    #[test]
    fn project_settings_change_format_name_and_import() {
        use cutroom_project::ColorSpace;
        let (tmp, mut session) = new_session();
        let rate = FrameRate::FPS_24;
        assert!(session.set_format(1921, 1081, rate, ColorSpace::DisplayP3));
        assert_eq!(
            (session.doc.settings.width, session.doc.settings.height),
            (1920, 1080),
            "sizes stay even"
        );
        assert_eq!(session.doc.settings.frame_rate, rate);
        assert_eq!(session.doc.settings.color_space, ColorSpace::DisplayP3);
        assert!(!session.set_format(1920, 1080, rate, ColorSpace::DisplayP3));
        assert!(session.set_name("  Renamed ") && session.doc.name == "Renamed");
        assert!(!session.set_name("  "));
        // Copy mode is stored in the project and survives a reopen.
        let media = import_one(&tmp, &mut session, "a.mp4");
        assert!(session.doc.media_item(media).is_ok());
        assert_eq!(session.set_import_copy(true, true).unwrap_or(0) <= 1, true);
        assert_eq!(session.import_mode, ImportMode::Copy);
        session.save();
        let root = session.folder.root().to_path_buf();
        let reopened = Session::open(&root).unwrap();
        assert_eq!(reopened.import_mode, ImportMode::Copy);
        assert_eq!(reopened.doc.settings.color_space, ColorSpace::DisplayP3);
    }

    #[test]
    fn renaming_changes_the_label_only() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        session.rename_media(media, "Opening shot");
        assert_eq!(
            media_label(session.doc.media_item(media).unwrap()),
            "Opening shot"
        );
        session.rename_media(media, "");
        assert_eq!(media_label(session.doc.media_item(media).unwrap()), "a.mp4");
    }

    #[test]
    fn relinking_repoints_the_media_and_keeps_the_clips() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let clip = session.add_media_to_timeline(media).unwrap();
        let old = session.media_path(media).unwrap();
        std::fs::remove_file(&old).unwrap();
        assert!(session.media_is_missing(media));
        let replacement = tmp.path().join("elsewhere.mp4");
        std::fs::write(&replacement, b"new bytes").unwrap();
        session.relink_media(media, &replacement).unwrap();
        assert!(!session.media_is_missing(media));
        assert!(session.doc.timeline.find_clip(clip).is_some());
        assert!(session.relink_media(media, tmp.path()).is_err());
    }

    #[test]
    fn removing_media_takes_its_clips_with_it_and_keeps_the_file() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let other = import_one(&tmp, &mut session, "b.mp4");
        session.add_media_to_timeline(media).unwrap();
        session.set_playhead(Ticks::from_seconds_f64(8.0));
        session.add_media_to_timeline(media).unwrap();
        session.add_media_to_timeline(other).unwrap();
        let file = session.media_path(media).unwrap();
        assert_eq!(session.remove_media(media, false), Ok(2));
        assert!(session.doc.media_item(media).is_err());
        assert_eq!(session.media_usage(other), 1, "other media is untouched");
        assert!(file.is_file(), "the file stays on disk");
        assert!(!session.history.can_undo(), "this can't be undone");
        assert!(session.remove_media(media, false).is_err());
    }

    #[test]
    fn only_files_stored_inside_the_project_may_be_deleted_from_disk() {
        let (tmp, mut session) = new_session();
        let outside = import_one(&tmp, &mut session, "mine.mp4"); // linked, not copied
        let outside_file = session.media_path(outside).unwrap();
        assert!(!session.media_in_project(outside));
        assert!(session.remove_media(outside, true).is_err());
        assert!(
            outside_file.is_file() && session.doc.media_item(outside).is_ok(),
            "nothing happened"
        );

        session.import_mode = ImportMode::Copy;
        let copy_src = tmp.path().join("copied.mp4");
        std::fs::write(&copy_src, b"data").unwrap();
        let inside = session.import_files(&[copy_src.clone()])[0];
        let inside_file = session.media_path(inside).unwrap();
        assert!(session.media_in_project(inside));
        assert_eq!(session.remove_media(inside, true), Ok(0));
        assert!(!inside_file.exists(), "the project's copy is deleted");
        assert!(copy_src.exists(), "the original is untouched");
    }

    // ----- proxies -------------------------------------------------------------

    #[test]
    fn proxies_are_named_after_the_fingerprint_and_used_only_when_they_exist() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "big.mp4");
        let original = session.media_path(media).unwrap();
        let proxy = session.proxy_path(media).unwrap();
        assert!(proxy.starts_with(session.folder.proxy_dir(&session.doc)));
        assert!(proxy.to_string_lossy().ends_with("-proxy.webm"));
        assert_eq!(
            session.preview_path(media, true),
            Some(original.clone()),
            "no proxy yet"
        );

        std::fs::create_dir_all(proxy.parent().unwrap()).unwrap();
        std::fs::write(&proxy, b"proxy").unwrap();
        assert_eq!(session.existing_proxy(media), Some(proxy.clone()));
        assert_eq!(session.preview_path(media, true), Some(proxy));
        assert_eq!(
            session.preview_path(media, false),
            Some(original),
            "proxies switched off"
        );
    }

    #[test]
    fn only_probed_videos_can_have_proxies() {
        let (tmp, mut session) = new_session();
        let video = import_one(&tmp, &mut session, "v.mp4");
        let song = import_one(&tmp, &mut session, "s.mp3");
        let photo = import_one(&tmp, &mut session, "p.png");
        assert!(!session.can_have_proxy(video), "not probed yet");
        probed(&mut session, video, true, true, 5.0);
        session.apply_probe(
            video,
            Some(Ticks::from_seconds_f64(5.0)),
            MediaProbe {
                has_video: true,
                has_audio: true,
                width: 1920,
                height: 1080,
                ..Default::default()
            },
        );
        assert!(session.can_have_proxy(video));
        assert!(!session.can_have_proxy(song));
        assert!(!session.can_have_proxy(photo));
    }

    #[test]
    fn waveform_cache_files_live_in_the_cache_folder() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let path = session.waveform_cache_path(media).unwrap();
        assert!(path.starts_with(session.folder.cache_dir(&session.doc)));
        assert!(path.to_string_lossy().ends_with(".peaks"));
    }

    // ----- extract audio -------------------------------------------------------

    #[test]
    fn extracting_audio_moves_the_sound_to_an_audio_track_in_one_undo_step() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "talk.mp4");
        probed(&mut session, media, true, true, 5.0);
        session.set_playhead(Ticks::from_seconds_f64(2.0));
        let video = session.add_media_to_timeline(media).unwrap();
        assert!(session.can_extract_audio(video));
        let audio = session.extract_audio(video).unwrap();

        let (audio_track, audio_clip) = session.doc.timeline.find_clip(audio).unwrap();
        assert_eq!(audio_track.kind, TrackKind::Audio);
        assert_eq!(audio_clip.start, Ticks::from_seconds_f64(2.0));
        assert_eq!(audio_clip.media, media);
        assert!(
            session.doc.timeline.find_clip(video).unwrap().1.audio_muted,
            "the picture is silent now"
        );
        assert!(!session.can_extract_audio(video), "nothing left to extract");

        session.undo();
        assert!(session.doc.timeline.find_clip(audio).is_none());
        assert!(!session.doc.timeline.find_clip(video).unwrap().1.audio_muted);
    }

    #[test]
    fn extracted_audio_gets_a_new_audio_track_when_the_default_one_is_busy() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "talk.mp4");
        let song = import_one(&tmp, &mut session, "s.mp3");
        probed(&mut session, media, true, true, 5.0);
        session.add_media_to_timeline(song).unwrap(); // Audio 1 is occupied at 0..5 s
        let video = session.add_media_to_timeline(media).unwrap();
        let audio = session.extract_audio(video).unwrap();
        assert_eq!(
            session.doc.timeline.find_clip(audio).unwrap().0.name,
            "Audio 2"
        );
        session.undo();
        assert_eq!(
            session
                .doc
                .timeline
                .tracks
                .iter()
                .filter(|t| t.kind == TrackKind::Audio)
                .count(),
            1,
            "the new track goes away with the undo"
        );
    }

    #[test]
    fn only_video_clips_with_sound_can_have_it_extracted() {
        let (tmp, mut session) = new_session();
        let silent = import_one(&tmp, &mut session, "b-roll.mp4");
        let song = import_one(&tmp, &mut session, "s.mp3");
        let photo = import_one(&tmp, &mut session, "p.png");
        probed(&mut session, silent, true, false, 5.0);
        let a = session.add_media_to_timeline(silent).unwrap();
        session.set_playhead(Ticks::from_seconds_f64(10.0));
        let b = session.add_media_to_timeline(song).unwrap();
        let c = session.add_media_to_timeline(photo).unwrap();
        for clip in [a, b, c] {
            assert!(!session.can_extract_audio(clip));
            assert!(session.extract_audio(clip).is_err());
        }
        assert!(!session.can_extract_audio(ClipId(9999)));
    }

    // ----- picture placement -----------------------------------------------------

    #[test]
    fn transforms_are_set_as_undoable_steps_and_only_on_video_clips() {
        use cutroom_timeline::Transform;
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let song = import_one(&tmp, &mut session, "s.mp3");
        let video = session.add_media_to_timeline(media).unwrap();
        let audio = session.add_media_to_timeline(song).unwrap();
        let moved = Transform {
            x: 300,
            scale: 600,
            ..Transform::default()
        };
        assert!(session.set_clip_transform(video, moved));
        assert_eq!(session.clip_transform(video), Some(moved));
        assert!(
            !session.set_clip_transform(video, moved),
            "no change, no undo entry"
        );
        assert!(!session.can_transform(audio));
        assert!(
            !session.set_clip_transform(audio, moved),
            "sound has no picture"
        );
        session.undo();
        assert!(session.clip_transform(video).unwrap().is_default());
    }

    #[test]
    fn mirror_toggles_and_rotate_turns_a_quarter_at_a_time() {
        let (tmp, mut session) = new_session();
        let media = import_one(&tmp, &mut session, "a.mp4");
        let clip = session.add_media_to_timeline(media).unwrap();
        assert!(session.mirror_clip(clip));
        assert!(session.clip_transform(clip).unwrap().flip_h);
        assert!(session.mirror_clip(clip));
        assert!(
            !session.clip_transform(clip).unwrap().flip_h,
            "mirroring twice undoes it"
        );
        for expected in [900, 1800 - 3600, -900, 0] {
            assert!(session.rotate_clip(clip));
            assert_eq!(session.clip_transform(clip).unwrap().rotation, expected);
        }
        assert!(!session.mirror_clip(ClipId(9999)));
    }

    #[test]
    fn v_toggles_deactivation_for_the_whole_selection_and_undoes() {
        let (tmp, mut session) = new_session();
        let m = import_one(&tmp, &mut session, "a.mp4");
        let a = session.add_media_to_timeline(m).unwrap();
        session.set_playhead(Ticks::from_seconds_f64(20.0));
        let b = session.add_media_to_timeline(m).unwrap();
        session.select_many(&[a, b]);
        let off = |s: &Session, id| s.doc.timeline.find_clip(id).unwrap().1.disabled;
        assert!(session.toggle_selected_disabled());
        assert!(off(&session, a) && off(&session, b));
        assert!(session.toggle_selected_disabled());
        assert!(!off(&session, a) && !off(&session, b));
        session.toggle_selected_disabled();
        session.undo();
        assert!(!off(&session, a) && !off(&session, b));
    }

    #[test]
    fn group_copy_paste_keeps_spacing_and_is_one_undo_step() {
        let (tmp, mut session) = new_session();
        let m = import_one(&tmp, &mut session, "a.mp4");
        let a = session.add_media_to_timeline(m).unwrap();
        session.set_playhead(Ticks::from_seconds_f64(20.0));
        let b = session.add_media_to_timeline(m).unwrap();
        session.select_many(&[a, b]);
        let copies = session.copy_selection();
        assert_eq!(copies.len(), 2);
        let gap = copies[1].offset;
        assert!(gap > Ticks::ZERO && copies[0].offset == Ticks::ZERO);
        session.set_playhead(Ticks::from_seconds_f64(60.0));
        let before: usize = session
            .doc
            .timeline
            .tracks
            .iter()
            .map(|t| t.clips.len())
            .sum();
        let pasted = session.paste_group(&copies);
        assert_eq!(pasted.len(), 2);
        assert_eq!(session.selection().len(), 2);
        let starts: Vec<Ticks> = pasted
            .iter()
            .map(|id| session.doc.timeline.find_clip(*id).unwrap().1.start)
            .collect();
        assert_eq!(starts[1] - starts[0], gap);
        assert_eq!(starts[0], Ticks::from_seconds_f64(60.0));
        session.undo();
        let after: usize = session
            .doc
            .timeline
            .tracks
            .iter()
            .map(|t| t.clips.len())
            .sum();
        assert_eq!(after, before);
    }

    #[test]
    fn moving_a_group_slides_them_together_and_undoes_once() {
        let (tmp, mut session) = new_session();
        let m = import_one(&tmp, &mut session, "a.mp4");
        let a = session.add_media_to_timeline(m).unwrap();
        session.set_playhead(Ticks::from_seconds_f64(20.0));
        let b = session.add_media_to_timeline(m).unwrap();
        let start = |s: &Session, id| s.doc.timeline.find_clip(id).unwrap().1.start;
        let (sa, sb) = (start(&session, a), start(&session, b));
        let d = Ticks::from_seconds_f64(3.0);
        assert!(session.move_clips_by(&a, &[b], d));
        assert_eq!((start(&session, a), start(&session, b)), (sa + d, sb + d));
        session.undo();
        assert_eq!((start(&session, a), start(&session, b)), (sa, sb));
    }

    #[test]
    fn the_eye_button_hides_and_shows_a_track_with_undo() {
        let (_tmp, mut session) = bare_session();
        let track = session.doc.timeline.tracks[0].id;
        session.toggle_track_hidden(track);
        assert!(session.doc.timeline.track(track).unwrap().hidden);
        session.undo();
        assert!(!session.doc.timeline.track(track).unwrap().hidden);
    }
    #[test]
    fn keyframes_only_capture_animated_props_and_undo_once() {
        use cutroom_timeline::Prop;
        let (tmp, mut session) = new_session();
        let m = import_one(&tmp, &mut session, "a.mp4");
        let id = session.add_media_to_timeline(m).unwrap();
        let start = session.doc.timeline.find_clip(id).unwrap().1.start;
        session.set_playhead(start);
        assert!(session.toggle_keyframes(id, &[Prop::X]));
        assert!(session.is_animated(id, Prop::X));
        assert!(!session.is_animated(id, Prop::Y));
        session.set_playhead(start + Ticks::from_seconds_f64(2.0));
        let mut t = session.transform_at_playhead(id).unwrap();
        t.x = 400;
        t.y = 100;
        assert!(session.edit_transform(id, t));
        let clip = session.doc.timeline.find_clip(id).unwrap().1;
        assert_eq!(
            clip.keyframes.len(),
            2,
            "only X is keyed: one key at each time"
        );
        assert_eq!(session.transform_at_playhead(id).unwrap().x, 400);
        assert_eq!(
            clip.transform.y, 100,
            "Y is not animated so it edits the base"
        );
        session.undo();
        let clip = session.doc.timeline.find_clip(id).unwrap().1;
        assert_eq!(clip.keyframes.len(), 1);
        assert_eq!(clip.transform.y, 0);
    }
    fn sample_lines() -> Vec<cutroom_captions::lines::Line> {
        use cutroom_captions::{lines::Line, TimedWord};
        let w = |t: &str, a: f64, b: f64| TimedWord {
            text: t.into(),
            start: a,
            end: b,
        };
        vec![
            Line {
                start: 1.0,
                end: 2.0,
                words: vec![w(" Hello", 1.0, 1.4), w(" world", 1.5, 2.0)],
            },
            Line {
                start: 3.0,
                end: 4.0,
                words: vec![w(" Second", 3.0, 3.5), w(" line", 3.5, 4.0)],
            },
        ]
    }

    #[test]
    fn captions_land_on_their_own_track_as_one_undo_step() {
        use cutroom_timeline::TextSpec;
        let (_tmp, mut session) = new_session();
        let tracks_before = session.doc.timeline.tracks.len();
        let style = TextSpec {
            highlight: [255, 200, 0, 255],
            ..TextSpec::default()
        };
        assert_eq!(session.add_captions(&sample_lines(), &style, "Classic"), 2);
        assert_eq!(session.doc.timeline.tracks.len(), tracks_before + 1);
        let top = &session.doc.timeline.tracks[0];
        assert_eq!(top.name, "Captions");
        assert_eq!(top.clips.len(), 2);
        assert_eq!(top.clips[0].text.as_ref().unwrap().content, "Hello world");
        assert_eq!(top.clips[0].words.len(), 2);
        assert_eq!(top.clips[0].words[1].start, Ticks::from_seconds_f64(0.5));
        assert_eq!(top.clips[0].transform.y, cutroom_timeline::text::CAPTION_Y);
        // Again over the same time replaces them rather than piling up.
        assert_eq!(session.add_captions(&sample_lines(), &style, "Classic"), 2);
        assert_eq!(session.doc.timeline.tracks.len(), tracks_before + 1);
        assert_eq!(session.doc.timeline.tracks[0].clips.len(), 2);
        session.undo();
        assert_eq!(session.doc.timeline.tracks[0].clips.len(), 2);
        session.undo();
        assert_eq!(session.doc.timeline.tracks.len(), tracks_before);
    }

    #[test]
    fn moving_one_caption_moves_all_when_they_are_shared() {
        let (_tmp, mut session) = new_session();
        let style = cutroom_timeline::TextSpec::default();
        session.add_captions(&sample_lines(), &style, "Classic");
        let ids: Vec<_> = session.doc.timeline.tracks[0]
            .clips
            .iter()
            .map(|c| c.id)
            .collect();
        let mut moved = session.transform_at_playhead(ids[0]).unwrap();
        moved.x += 120;
        moved.y -= 80;
        let targets = session.group_targets(ids[0], true);
        assert_eq!(targets, vec![ids[1]]);
        assert!(session.edit_transform_for(ids[0], moved, &targets));
        for id in &ids {
            let t = session.doc.timeline.find_clip(*id).unwrap().1.transform;
            assert_eq!((t.x, t.y), (moved.x, moved.y));
        }
        session.undo();
        let t = session.doc.timeline.find_clip(ids[1]).unwrap().1.transform;
        assert_eq!(t.y, cutroom_timeline::text::CAPTION_Y);
    }

    #[test]
    fn caption_text_edits_retime_words_and_cues_come_out_in_order() {
        let (_tmp, mut session) = new_session();
        let style = cutroom_timeline::TextSpec::default();
        session.add_captions(&sample_lines(), &style, "Classic");
        let cues = session.caption_cues();
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].2, "Hello world");
        assert!((cues[1].0 - 3.0).abs() < 1e-6);
        let id = session.doc.timeline.tracks[0].clips[0].id;
        let mut spec = session.clip_text(id).unwrap();
        spec.content = "Hello brave new world".into();
        assert!(session.edit_text(id, spec));
        let c = session.doc.timeline.find_clip(id).unwrap().1;
        assert_eq!(c.words.len(), 4);
        assert_eq!(
            cutroom_timeline::text::join_words(&c.words),
            "Hello brave new world"
        );
        // The same edit and its words go back together.
        session.undo();
        let c = session.doc.timeline.find_clip(id).unwrap().1;
        assert_eq!(c.words.len(), 2);
        assert_eq!(c.text.as_ref().unwrap().content, "Hello world");
        // Copies keep the words.
        session.select_only(id);
        let copy = session.copy_selection();
        session.set_playhead(Ticks::from_seconds_f64(20.0));
        let pasted = session.paste_group(&copy);
        assert_eq!(
            session
                .doc
                .timeline
                .find_clip(pasted[0])
                .unwrap()
                .1
                .words
                .len(),
            2
        );
    }

    #[test]
    fn text_clips_go_on_top_and_edit_with_undo() {
        use cutroom_timeline::TextSpec;
        let (_tmp, mut session) = new_session();
        let first = session.add_text_clip(TextSpec::default()).unwrap();
        let top = session.doc.timeline.tracks[0].id;
        assert_eq!(session.doc.timeline.find_clip(first).unwrap().0.id, top);
        // A second at the same moment cannot share the track: it gets a new one above.
        let second = session.add_text_clip(TextSpec::default()).unwrap();
        let track_of = |s: &Session, id| s.doc.timeline.find_clip(id).unwrap().0.id;
        assert_ne!(track_of(&session, second), track_of(&session, first));
        assert_eq!(
            track_of(&session, second),
            session.doc.timeline.tracks[0].id
        );
        let changed = TextSpec {
            content: "Hello".into(),
            ..TextSpec::default()
        };
        assert!(session.set_clip_text(second, changed.clone()));
        assert!(
            !session.set_clip_text(second, changed.clone()),
            "unchanged is a no-op"
        );
        assert_eq!(session.clip_text(second).unwrap().content, "Hello");
        session.undo();
        assert_eq!(session.clip_text(second).unwrap().content, "Your text");
        // Copy and paste keep the words.
        session.select_only(second);
        let copy = session.copy_selection();
        session.set_playhead(Ticks::from_seconds_f64(10.0));
        let ids = session.paste_group(&copy);
        assert_eq!(session.clip_text(ids[0]).unwrap().content, "Your text");
    }

    #[test]
    fn live_text_edits_become_one_undo_step_back_to_the_start() {
        use cutroom_timeline::TextSpec;
        let (_tmp, mut session) = new_session();
        let id = session.add_text_clip(TextSpec::default()).unwrap();
        for word in ["H", "He", "Hel", "Hell", "Hello"] {
            let spec = TextSpec {
                content: word.into(),
                ..TextSpec::default()
            };
            session.preview_clip_text(id, spec);
            assert_eq!(
                session.clip_text(id).unwrap().content,
                word,
                "shown at once"
            );
        }
        let last = session.clip_text(id).unwrap();
        assert!(session.set_clip_text(id, last));
        assert_eq!(session.clip_text(id).unwrap().content, "Hello");
        session.undo();
        assert_eq!(
            session.clip_text(id).unwrap().content,
            "Your text",
            "one step undoes the whole word"
        );
    }

    #[test]
    fn skew_and_perspective_are_recorded_and_survive_a_later_move() {
        let (tmp, mut session) = new_session();
        let m = import_one(&tmp, &mut session, "a.mp4");
        let id = session.add_media_to_timeline(m).unwrap();
        let mut t = session.transform_at_playhead(id).unwrap();
        t.skew_x = 150;
        t.persp_y = -300;
        assert!(session.edit_transform(id, t));
        // Moving the box later (a transform that started from the shown one).
        let mut moved = session.transform_at_playhead(id).unwrap();
        moved.x = 120;
        assert!(session.edit_transform(id, moved));
        let now = session.transform_at_playhead(id).unwrap();
        assert_eq!((now.skew_x, now.persp_y, now.x), (150, -300, 120));
    }

    #[test]
    fn a_bend_is_keyed_as_one_element_and_follows_its_keyframes() {
        use cutroom_timeline::Prop;
        let (tmp, mut session) = new_session();
        let m = import_one(&tmp, &mut session, "a.mp4");
        let id = session.add_media_to_timeline(m).unwrap();
        let start = session.doc.timeline.find_clip(id).unwrap().1.start;
        session.set_playhead(start);
        assert!(session.toggle_keyframes(id, &Prop::WARP));
        assert_eq!(
            session
                .doc
                .timeline
                .find_clip(id)
                .unwrap()
                .1
                .keyframes
                .iter()
                .filter(|k| matches!(k.prop, Prop::Warp(_)))
                .count(),
            40,
            "pin and mesh are keyed together"
        );
        // Bend a corner and a mesh point one second in.
        session.set_playhead(start + Ticks::from_seconds_f64(1.0));
        assert!(session.toggle_keyframes(id, &Prop::WARP));
        let mut t = session.transform_at_playhead(id).unwrap();
        t.warp.pin[2] = -400;
        t.warp.mesh[10] = 200;
        assert!(session.edit_transform(id, t));
        let clip = session.doc.timeline.find_clip(id).unwrap().1;
        assert!(clip.transform.warp.is_identity(), "the clip's own value is kept");
        assert_eq!(clip.transform_at(start + Ticks::from_seconds_f64(1.0)).warp.pin[2], -400);
        assert_eq!(clip.transform_at(start + Ticks::from_seconds_f64(0.5)).warp.pin[2], -200);
        assert_eq!(clip.transform_at(start).warp.mesh[10], 0);
        // Reset the bend on its own: skew stays.
        let mut skew = session.transform_at_playhead(id).unwrap();
        skew.skew_x = 120;
        session.edit_transform(id, skew);
        assert!(session.reset_props(id, &Prop::WARP));
        let clip = session.doc.timeline.find_clip(id).unwrap().1;
        assert!(clip.transform.warp.is_identity());
        assert!(!clip.keyframes.iter().any(|k| matches!(k.prop, Prop::Warp(_))));
        assert_eq!(clip.transform_at(start).skew_x, 120, "skew untouched by a bend reset");
        session.undo();
        let clip = session.doc.timeline.find_clip(id).unwrap().1;
        assert!(clip.keyframes.iter().any(|k| matches!(k.prop, Prop::Warp(_))), "one undo step");
    }

    #[test]
    fn a_bend_belongs_to_its_own_clip_and_is_not_shared_in_a_group_edit() {
        let (tmp, mut session) = new_session();
        let m = import_one(&tmp, &mut session, "a.mp4");
        let a = session.add_media_to_timeline(m).unwrap();
        let b = session.add_media_to_timeline(m).unwrap();
        session.select_many(&[a, b]);
        let mut t = session.transform_at_playhead(a).unwrap();
        t.warp.pin[0] = 100;
        t.x = 50;
        let targets = session.group_targets(a, false);
        session.edit_transform_for(a, t, &targets);
        let (ta, tb) = (
            session.doc.timeline.find_clip(a).unwrap().1.transform,
            session.doc.timeline.find_clip(b).unwrap().1.transform,
        );
        assert_eq!(ta.warp.pin[0], 100);
        assert!(tb.warp.is_identity(), "the other clip is not bent");
    }

    #[test]
    fn reset_clears_values_and_keyframes_and_undoes_in_one_step() {
        use cutroom_timeline::{Prop, TextSpec};
        let (_tmp, mut session) = new_session();
        let id = session.add_text_clip(TextSpec::default()).unwrap();
        let start = session.doc.timeline.find_clip(id).unwrap().1.start;
        session.set_playhead(start);
        session.toggle_keyframes(id, &[Prop::X, Prop::TextSize]);
        session.set_playhead(start + Ticks::from_seconds_f64(1.0));
        let mut t = session.transform_at_playhead(id).unwrap();
        t.x = 300;
        t.skew_x = 100;
        session.edit_transform(id, t);
        let mut spec = session
            .doc
            .timeline
            .find_clip(id)
            .unwrap()
            .1
            .text_at(session.playhead)
            .unwrap();
        spec.size = 200;
        spec.outline = 50;
        session.edit_text(id, spec);
        let clip = session.doc.timeline.find_clip(id).unwrap().1;
        assert_eq!(
            clip.text_at(session.playhead).unwrap().size,
            200,
            "size keyed at 1 s"
        );
        assert_eq!(
            clip.text.as_ref().unwrap().outline,
            50,
            "outline is the clip's own"
        );
        assert!(session.reset_transform(id));
        let clip = session.doc.timeline.find_clip(id).unwrap().1;
        assert!(clip.transform.is_default());
        assert!(!clip
            .keyframes
            .iter()
            .any(|k| Prop::TRANSFORM.contains(&k.prop)));
        assert!(
            clip.keyframes.iter().any(|k| k.prop == Prop::TextSize),
            "text keys untouched"
        );
        assert!(session.reset_text_section(id, false));
        let clip = session.doc.timeline.find_clip(id).unwrap().1;
        assert!(clip.keyframes.is_empty());
        assert_eq!(clip.text.as_ref().unwrap().size, 80);
        assert_eq!(
            clip.text.as_ref().unwrap().outline,
            50,
            "effects were not reset"
        );
        assert!(session.reset_text_section(id, true));
        assert_eq!(
            session
                .doc
                .timeline
                .find_clip(id)
                .unwrap()
                .1
                .text
                .as_ref()
                .unwrap()
                .outline,
            0
        );
        session.undo();
        assert_eq!(
            session
                .doc
                .timeline
                .find_clip(id)
                .unwrap()
                .1
                .text
                .as_ref()
                .unwrap()
                .outline,
            50
        );
    }
}
