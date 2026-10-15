//! Finding, describing, renaming and deleting projects on disk.
//! Used by the home screen; has no UI code.

use crate::error::ProjectError;
use crate::folder::{ProjectFolder, PROJECT_FILE};
use crate::names::folder_name_problem;
use cutroom_timeline::{FrameRate, Ticks};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What the home screen shows about a project.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectSummary {
    pub path: PathBuf,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    pub media_count: usize,
    pub clip_count: usize,
    pub duration: Ticks,
    pub modified: Option<SystemTime>,
}

/// A project folder found on disk: readable, or present but unreadable.
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectEntry {
    Ok(ProjectSummary),
    Broken {
        path: PathBuf,
        error: String,
        modified: Option<SystemTime>,
    },
}

impl ProjectEntry {
    pub fn path(&self) -> &Path {
        match self {
            ProjectEntry::Ok(s) => &s.path,
            ProjectEntry::Broken { path, .. } => path,
        }
    }

    /// Project name, or the folder name if the project can't be read.
    pub fn display_name(&self) -> String {
        match self {
            ProjectEntry::Ok(s) => s.name.clone(),
            ProjectEntry::Broken { path, .. } => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
        }
    }

    pub fn modified(&self) -> Option<SystemTime> {
        match self {
            ProjectEntry::Ok(s) => s.modified,
            ProjectEntry::Broken { modified, .. } => *modified,
        }
    }
}

fn project_file_modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path.join(PROJECT_FILE))
        .and_then(|m| m.modified())
        .ok()
}

pub fn summarize(path: &Path) -> Result<ProjectSummary, ProjectError> {
    let (folder, doc) = ProjectFolder::open(path)?;
    Ok(ProjectSummary {
        path: folder.root().to_path_buf(),
        name: doc.name.clone(),
        width: doc.settings.width,
        height: doc.settings.height,
        frame_rate: doc.settings.frame_rate,
        media_count: doc.media.len(),
        clip_count: doc.timeline.tracks.iter().map(|t| t.clips.len()).sum(),
        duration: doc.timeline.duration(),
        modified: project_file_modified(folder.root()),
    })
}

pub fn read_entry(path: &Path) -> ProjectEntry {
    match summarize(path) {
        Ok(summary) => ProjectEntry::Ok(summary),
        Err(e) => ProjectEntry::Broken {
            path: path.to_path_buf(),
            error: e.to_string(),
            modified: project_file_modified(path),
        },
    }
}

/// Newest first; projects with no timestamp last.
pub fn sort_newest_first(entries: &mut [ProjectEntry]) {
    entries.sort_by(|a, b| b.modified().cmp(&a.modified()));
}

/// Every project folder directly inside `root`. A missing root is just empty.
pub fn scan_projects(root: &Path) -> Vec<ProjectEntry> {
    let Ok(read) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut entries: Vec<ProjectEntry> = read
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join(PROJECT_FILE).is_file())
        .map(|p| read_entry(&p))
        .collect();
    sort_newest_first(&mut entries);
    entries
}

/// Total size in bytes of everything under `path` (symlinks are not followed).
pub fn dir_size(path: &Path) -> u64 {
    let Ok(read) = fs::read_dir(path) else {
        return 0;
    };
    read.filter_map(Result::ok)
        .map(|entry| match entry.file_type() {
            Ok(t) if t.is_dir() => dir_size(&entry.path()),
            Ok(t) if t.is_file() => entry.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// Rename a project. Always changes the name stored in the project; if
/// `rename_folder` is set the folder is renamed to match. Returns the
/// project's (possibly new) location.
pub fn rename_project(
    path: &Path,
    new_name: &str,
    rename_folder: bool,
) -> Result<PathBuf, ProjectError> {
    if let Some(problem) = folder_name_problem(new_name) {
        return Err(ProjectError::InvalidName(problem.into()));
    }
    let new_name = new_name.trim();
    let (folder, mut doc) = ProjectFolder::open(path)?;
    let old_root = folder.root().to_path_buf();

    let new_root = old_root
        .parent()
        .map(|parent| parent.join(new_name))
        .filter(|_| rename_folder);
    if let Some(target) = &new_root {
        let case_only_change =
            target.to_string_lossy().to_lowercase() == old_root.to_string_lossy().to_lowercase();
        if *target != old_root && target.exists() && !case_only_change {
            return Err(ProjectError::AlreadyExists(target.clone()));
        }
    }

    doc.name = new_name.to_owned();
    folder.save(&doc)?;
    match new_root {
        Some(target) if target != old_root => {
            fs::rename(&old_root, &target)?;
            Ok(target)
        }
        _ => Ok(old_root),
    }
}

/// Permanently delete a project folder and everything in it.
///
/// Guarded: the folder must hold a `project.json` written by Kani, and a
/// drive root is never touched, so a wrong path can't wipe an unrelated folder.
pub fn delete_project(path: &Path) -> Result<(), ProjectError> {
    let abs = std::path::absolute(path)?;
    if abs.parent().is_none() {
        return Err(ProjectError::NotDeletable("that is a drive root".into()));
    }
    let bytes = fs::read(abs.join(PROJECT_FILE)).map_err(|_| {
        ProjectError::NotDeletable("there is no project file in that folder".into())
    })?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| ProjectError::NotDeletable("the project file isn't valid JSON".into()))?;
    if !matches!(
        value.get("app").and_then(|v| v.as_str()),
        Some("kani" | "chiyoko" | "pompo" | "cutroom")
    ) {
        return Err(ProjectError::NotDeletable(
            "that folder's project file wasn't written by Kani".into(),
        ));
    }
    fs::remove_dir_all(&abs)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{ProjectDoc, ProjectSettings};
    use std::time::Duration;
    use tempfile::tempdir;

    fn make(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        let doc = ProjectDoc::new(name, ProjectSettings::vertical(FrameRate::FPS_30));
        ProjectFolder::create(&path, &doc).unwrap();
        path
    }

    #[test]
    fn scan_finds_only_project_folders() {
        let tmp = tempdir().unwrap();
        make(tmp.path(), "alpha");
        make(tmp.path(), "beta");
        fs::create_dir_all(tmp.path().join("not-a-project")).unwrap();
        fs::write(tmp.path().join("stray.txt"), b"x").unwrap();
        let mut names: Vec<_> = scan_projects(tmp.path())
            .iter()
            .map(|e| e.display_name())
            .collect();
        names.sort();
        assert_eq!(names, ["alpha", "beta"]);
    }

    #[test]
    fn a_missing_root_is_an_empty_list() {
        let tmp = tempdir().unwrap();
        assert!(scan_projects(&tmp.path().join("nope")).is_empty());
    }

    #[test]
    fn summaries_describe_the_project() {
        let tmp = tempdir().unwrap();
        let path = make(tmp.path(), "alpha");
        let summary = summarize(&path).unwrap();
        assert_eq!(summary.name, "alpha");
        assert_eq!((summary.width, summary.height), (1080, 1920));
        assert_eq!(summary.frame_rate, FrameRate::FPS_30);
        assert_eq!((summary.media_count, summary.clip_count), (0, 0));
        assert!(summary.modified.is_some());
    }

    #[test]
    fn unreadable_projects_are_listed_not_hidden() {
        let tmp = tempdir().unwrap();
        let bad = tmp.path().join("broken");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join(PROJECT_FILE), b"{ not json").unwrap();
        let entries = scan_projects(tmp.path());
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            ProjectEntry::Broken { error, .. } => assert!(!error.is_empty()),
            other => panic!("expected a broken entry, got {other:?}"),
        }
        assert_eq!(entries[0].display_name(), "broken");
    }

    #[test]
    fn newest_projects_sort_first() {
        let t = |secs| Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
        let entry = |name: &str, m| ProjectEntry::Broken {
            path: PathBuf::from(name),
            error: String::new(),
            modified: m,
        };
        let mut entries = vec![
            entry("old", t(10)),
            entry("none", None),
            entry("new", t(99)),
        ];
        sort_newest_first(&mut entries);
        let order: Vec<_> = entries.iter().map(|e| e.display_name()).collect();
        assert_eq!(order, ["new", "old", "none"]);
    }

    #[test]
    fn dir_size_adds_up_nested_files() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("a/b")).unwrap();
        fs::write(tmp.path().join("one"), [0u8; 10]).unwrap();
        fs::write(tmp.path().join("a/two"), [0u8; 20]).unwrap();
        fs::write(tmp.path().join("a/b/three"), [0u8; 30]).unwrap();
        assert_eq!(dir_size(tmp.path()), 60);
        assert_eq!(dir_size(&tmp.path().join("missing")), 0);
    }

    #[test]
    fn rename_changes_the_name_and_optionally_the_folder() {
        let tmp = tempdir().unwrap();
        let path = make(tmp.path(), "alpha");

        let same_folder = rename_project(&path, "Alpha v2", false).unwrap();
        assert_eq!(same_folder, path);
        assert_eq!(summarize(&path).unwrap().name, "Alpha v2");

        let moved = rename_project(&path, "Gamma", true).unwrap();
        assert_eq!(moved, tmp.path().join("Gamma"));
        assert!(!path.exists());
        assert_eq!(summarize(&moved).unwrap().name, "Gamma");
    }

    #[test]
    fn rename_refuses_bad_names_and_existing_folders() {
        let tmp = tempdir().unwrap();
        let a = make(tmp.path(), "alpha");
        make(tmp.path(), "beta");
        assert!(matches!(
            rename_project(&a, "bad/name", true),
            Err(ProjectError::InvalidName(_))
        ));
        assert!(matches!(
            rename_project(&a, "beta", true),
            Err(ProjectError::AlreadyExists(_))
        ));
        // A refused rename must not have changed anything.
        assert_eq!(summarize(&a).unwrap().name, "alpha");
    }

    #[test]
    fn renaming_to_the_same_name_is_a_no_op() {
        let tmp = tempdir().unwrap();
        let a = make(tmp.path(), "alpha");
        assert_eq!(rename_project(&a, "alpha", true).unwrap(), a);
        assert!(a.exists());
    }

    #[test]
    fn delete_removes_a_project_and_everything_in_it() {
        let tmp = tempdir().unwrap();
        let path = make(tmp.path(), "alpha");
        fs::write(path.join("media/clip.mp4"), b"data").unwrap();
        delete_project(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn delete_refuses_folders_that_are_not_cutroom_projects() {
        let tmp = tempdir().unwrap();
        let plain = tmp.path().join("photos");
        fs::create_dir_all(&plain).unwrap();
        fs::write(plain.join("holiday.jpg"), b"x").unwrap();
        assert!(matches!(
            delete_project(&plain),
            Err(ProjectError::NotDeletable(_))
        ));
        assert!(plain.join("holiday.jpg").exists());

        // A project.json from some other program is not ours either.
        let other = tmp.path().join("other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join(PROJECT_FILE), br#"{"name":"not ours"}"#).unwrap();
        assert!(matches!(
            delete_project(&other),
            Err(ProjectError::NotDeletable(_))
        ));
        assert!(other.exists());
    }

    #[test]
    fn delete_can_remove_a_project_whose_file_cannot_be_parsed_by_this_version() {
        let tmp = tempdir().unwrap();
        let path = make(tmp.path(), "future");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(path.join(PROJECT_FILE)).unwrap()).unwrap();
        value["schema_version"] = serde_json::Value::from(999);
        fs::write(path.join(PROJECT_FILE), serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(read_entry(&path), ProjectEntry::Broken { .. }));
        delete_project(&path).unwrap();
        assert!(!path.exists());
    }
}
