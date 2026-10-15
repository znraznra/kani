//! Project documents and the on-disk project-folder format.
//!
//! A project is a **self-contained folder you choose**:
//!
//! ```text
//! my-project/
//!   project.json    the document (versioned, human-readable)
//!   media/          media copied into the project (optional)
//!   proxies/        low-res editing proxies (location configurable)
//!   cache/          render cache (location configurable)
//!   thumbnails/
//!   autosave/       rotating backups
//! ```
//!
//! Nothing is written to AppData/Application Support unless the user picks it.

mod doc;
mod error;
mod folder;
mod library;
mod migrate;
mod names;

pub use doc::{
    MediaItem, MediaPath, MediaProbe, ProjectDoc, ProjectSettings, StorageSettings, SCHEMA_VERSION,
};
pub use error::ProjectError;
pub use folder::{hash_file, ImportMode, ProjectFolder, PROJECT_FILE};
pub use library::{
    delete_project, dir_size, read_entry, rename_project, scan_projects, sort_newest_first,
    summarize, ProjectEntry, ProjectSummary,
};
pub use migrate::migrate;
pub use names::folder_name_problem;
