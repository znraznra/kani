use directories::{ProjectDirs, UserDirs};
use std::path::{Path, PathBuf};

/// Per-user locations, resolved with the platform's conventions
/// (XDG on Linux, Library on macOS). On Windows Kani is portable: its own
/// files live beside the program (see [`portable_dir`]), never in AppData.
/// `KANI_DATA_DIR` overrides the place on every system.
///
/// Only *settings* live in the config dir. Projects, proxies and caches are
/// never placed there by default: they go where the user chooses, with
/// `suggested_projects_dir` only used as the initial value in the file dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub suggested_projects_dir: PathBuf,
}

impl AppPaths {
    pub fn detect() -> Option<AppPaths> {
        let user = UserDirs::new()?;
        let base = user
            .document_dir()
            .map(PathBuf::from)
            .unwrap_or_else(|| user.home_dir().to_path_buf());
        let config =
            |name: &str| ProjectDirs::from("", "", name).map(|d| d.config_dir().to_path_buf());
        let mut config_dir = config("Kani")?;
        let portable = std::env::var_os("KANI_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                cfg!(windows)
                    .then(|| std::env::current_exe().ok().map(|e| portable_dir(&e)))
                    .flatten()
            });
        let mut projects = base.join("Kani");
        // Earlier names, newest first.
        for old in ["Chiyoko", "Pompo", "Cutroom"] {
            if let Some(c) = config(old) {
                config_dir = keep_old_if_only_old(config_dir, c);
            }
            projects = keep_old_if_only_old(projects, base.join(old));
        }
        if let Some(dir) = portable {
            // A program folder that can't be written to (say, Program Files):
            // keep the files with the projects instead, still not in AppData.
            config_dir = if writable(&dir) {
                dir
            } else {
                base.join("Kani").join("Kani settings")
            };
        }
        Some(AppPaths {
            config_dir,
            suggested_projects_dir: projects,
        })
    }

    /// The per-user settings folders earlier versions used (AppData on
    /// Windows), newest name first, that exist and are not the folder in use.
    pub fn old_settings_dirs(&self) -> Vec<PathBuf> {
        ["Kani", "Chiyoko", "Pompo", "Cutroom"]
            .iter()
            .filter_map(|n| ProjectDirs::from("", "", n).map(|d| d.config_dir().to_path_buf()))
            .filter(|d| d.is_dir() && *d != self.config_dir)
            .collect()
    }
}

/// Where a portable Kani keeps its files: `<root>\data` for a packaged copy
/// (`<root>\bin\kani.exe` beside `<root>\lib\gstreamer-1.0`), otherwise a
/// `kani-data` folder beside the program.
pub fn portable_dir(exe: &Path) -> PathBuf {
    let bin = exe.parent().unwrap_or(Path::new("."));
    let packaged = bin
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("bin"))
        && bin
            .parent()
            .is_some_and(|r| r.join("lib").join("gstreamer-1.0").is_dir());
    match (packaged, bin.parent()) {
        (true, Some(root)) => root.join("data"),
        _ => bin.join("kani-data"),
    }
}

/// Can files be made in `dir` (creating it if needed)?
fn writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".kani-write-test");
    let ok = std::fs::write(&probe, b"ok").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Move everything from `old` into `new` (files `new` already has are kept),
/// then remove `old` and its parent if that is left empty. Returns how many
/// entries moved.
pub fn move_settings(old: &Path, new: &Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(new)?;
    let mut moved = 0;
    for entry in std::fs::read_dir(old)? {
        let entry = entry?;
        let to = new.join(entry.file_name());
        if to.exists() {
            continue;
        }
        if std::fs::rename(entry.path(), &to).is_err() {
            copy_all(&entry.path(), &to)?;
        }
        moved += 1;
    }
    let _ = std::fs::remove_dir_all(old);
    if let Some(parent) = old.parent() {
        let _ = std::fs::remove_dir(parent); // only if empty
    }
    Ok(moved)
}

fn copy_all(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_all(&e.path(), &to.join(e.file_name()))?;
        }
    } else {
        std::fs::copy(from, to)?;
    }
    Ok(())
}

/// The app was called Cutroom, then Pompo, then Chiyoko, before it was Kani. Someone who already has the
/// old folder (and no new one) keeps using it, so nothing seems to vanish.
fn keep_old_if_only_old(new: PathBuf, old: PathBuf) -> PathBuf {
    if !new.exists() && old.exists() {
        old
    } else {
        new
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_portable_copy_keeps_its_files_beside_the_program() {
        let dir = std::env::temp_dir().join(format!("kani-portable-{}", std::process::id()));
        let root = dir.join("Kani");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let exe = root.join("bin").join("kani.exe");
        assert_eq!(portable_dir(&exe), root.join("bin").join("kani-data"));
        std::fs::create_dir_all(root.join("lib").join("gstreamer-1.0")).unwrap();
        assert_eq!(portable_dir(&exe), root.join("data"));
        let dev = dir.join("target").join("release").join("kani.exe");
        assert_eq!(
            portable_dir(&dev),
            dir.join("target").join("release").join("kani-data")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_settings_move_over_without_replacing_newer_ones() {
        let dir = std::env::temp_dir().join(format!("kani-move-{}", std::process::id()));
        let old = dir.join("AppData").join("Kani").join("config");
        let new = dir.join("data");
        std::fs::create_dir_all(old.join("models")).unwrap();
        std::fs::write(old.join("settings.json"), "old").unwrap();
        std::fs::write(old.join("models").join("base.bin"), "m").unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("text-styles.json"), "new").unwrap();
        std::fs::write(old.join("text-styles.json"), "older").unwrap();
        assert_eq!(move_settings(&old, &new).unwrap(), 2);
        assert_eq!(
            std::fs::read_to_string(new.join("settings.json")).unwrap(),
            "old"
        );
        assert!(new.join("models").join("base.bin").is_file());
        assert_eq!(
            std::fs::read_to_string(new.join("text-styles.json")).unwrap(),
            "new"
        );
        assert!(
            !old.exists() && !old.parent().unwrap().exists(),
            "nothing left in AppData"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_old_folder_is_kept_until_a_new_one_exists() {
        let dir = std::env::temp_dir().join(format!("pompo-paths-{}", std::process::id()));
        let (old, new) = (dir.join("Chiyoko"), dir.join("Kani"));
        std::fs::create_dir_all(&old).unwrap();
        assert_eq!(keep_old_if_only_old(new.clone(), old.clone()), old);
        std::fs::create_dir_all(&new).unwrap();
        assert_eq!(keep_old_if_only_old(new.clone(), old), new);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn projects_are_not_suggested_inside_the_config_dir() {
        if let Some(paths) = AppPaths::detect() {
            assert!(
                paths.suggested_projects_dir.ends_with("Kani")
                    || paths.suggested_projects_dir.ends_with("Chiyoko")
                    || paths.suggested_projects_dir.ends_with("Pompo")
                    || paths.suggested_projects_dir.ends_with("Cutroom")
            );
            assert!(!paths.suggested_projects_dir.starts_with(&paths.config_dir));
        }
    }
}
