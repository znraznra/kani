use cutroom_timeline::MediaId;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid project file: {0}")]
    Json(#[from] serde_json::Error),
    #[error("project was saved by a newer version (schema {found}, this build supports up to {supported})")]
    TooNew { found: u32, supported: u32 },
    #[error("project file is corrupt: {0}")]
    Corrupt(String),
    #[error("a project already exists at {0}")]
    AlreadyExists(PathBuf),
    #[error("{0} is not a Kani project folder")]
    NotAProject(PathBuf),
    #[error("{0} is not a file")]
    NotAFile(PathBuf),
    #[error("unusable media path: {0}")]
    BadMediaPath(String),
    #[error("cannot move a project into itself or onto a non-empty folder: {0}")]
    InvalidDestination(PathBuf),
    #[error("media {0} not found in project")]
    MediaNotFound(MediaId),
    #[error("{0}")]
    InvalidName(String),
    #[error("refusing to delete: {0}")]
    NotDeletable(String),
}
