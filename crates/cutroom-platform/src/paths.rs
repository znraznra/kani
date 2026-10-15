use directories::{ProjectDirs, UserDirs};
use std::path::PathBuf;

/// Per-user locations, resolved with the platform's conventions
/// (XDG on Linux, AppData on Windows, Library on macOS).
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
        let mut projects = base.join("Kani");
        // Earlier names, newest first.
        for old in ["Chiyoko", "Pompo", "Cutroom"] {
            if let Some(c) = config(old) {
                config_dir = keep_old_if_only_old(config_dir, c);
            }
            projects = keep_old_if_only_old(projects, base.join(old));
        }
        Some(AppPaths {
            config_dir,
            suggested_projects_dir: projects,
        })
    }
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
