//! Text styles the user has saved: a look (font, size, colours, outline,
//! shadow, box, spacing, word lighting) kept under a name and reused in any
//! project. They live next to the settings, one list for the whole app.

use cutroom_timeline::TextSpec;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::Path;

const FILE_NAME: &str = "text-styles.json";
/// The longest name a style may have.
pub const MAX_NAME: usize = 40;
/// The words a saved style shows when it makes a new text clip.
pub const SAMPLE_TEXT: &str = "Your text";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedStyle {
    pub name: String,
    pub spec: TextSpec,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StyleBook {
    pub styles: Vec<SavedStyle>,
}

/// The name as it will be kept: trimmed, one line, not empty, not too long.
pub fn clean_name(name: &str) -> Result<String, String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err("Give the style a name.".into());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("Keep the name to {MAX_NAME} letters or fewer."));
    }
    Ok(name)
}

/// `target`'s words in the look of `style`: how a saved style is applied to a clip.
pub fn restyle(target: &TextSpec, style: &TextSpec) -> TextSpec {
    let mut out = style.clone();
    out.content = target.content.clone();
    out.active = target.active;
    out
}

/// The look of `spec` as it is kept: its words are replaced by a sample.
fn look_of(spec: &TextSpec) -> TextSpec {
    let mut s = spec.clone();
    s.content = SAMPLE_TEXT.into();
    s.active = None;
    s
}

impl StyleBook {
    /// A missing or unreadable file gives an empty list; it is never fatal.
    pub fn load(config_dir: &Path) -> StyleBook {
        fs::read(config_dir.join(FILE_NAME))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Atomic write: temp file, flush, rename.
    pub fn save(&self, config_dir: &Path) -> std::io::Result<()> {
        fs::create_dir_all(config_dir)?;
        let tmp = config_dir.join(format!("{FILE_NAME}.tmp"));
        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            file.sync_all()?;
        }
        fs::rename(tmp, config_dir.join(FILE_NAME))
    }

    fn position(&self, name: &str) -> Option<usize> {
        let lower = name.to_lowercase();
        self.styles
            .iter()
            .position(|s| s.name.to_lowercase() == lower)
    }

    pub fn get(&self, name: &str) -> Option<&SavedStyle> {
        self.position(name).map(|i| &self.styles[i])
    }

    /// Keep `spec`'s look under `name`. A style with that name (any case) is
    /// replaced; the result says so. Returns the name as kept.
    pub fn save_style(&mut self, name: &str, spec: &TextSpec) -> Result<(String, bool), String> {
        let name = clean_name(name)?;
        let look = look_of(spec);
        match self.position(&name) {
            Some(i) => {
                self.styles[i] = SavedStyle {
                    name: name.clone(),
                    spec: look,
                };
                Ok((name, true))
            }
            None => {
                self.styles.push(SavedStyle {
                    name: name.clone(),
                    spec: look,
                });
                Ok((name, false))
            }
        }
    }

    pub fn rename(&mut self, old: &str, new: &str) -> Result<String, String> {
        let new = clean_name(new)?;
        let Some(i) = self.position(old) else {
            return Err("That style is gone.".into());
        };
        if let Some(j) = self.position(&new) {
            if j != i {
                return Err("A style with that name already exists.".into());
            }
        }
        self.styles[i].name = new.clone();
        Ok(new)
    }

    pub fn delete(&mut self, name: &str) -> bool {
        match self.position(name) {
            Some(i) => {
                self.styles.remove(i);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loud() -> TextSpec {
        TextSpec {
            content: "Some words".into(),
            size: 99,
            color: [1, 2, 3, 255],
            outline: 70,
            outline_on: true,
            active: Some((0, 4)),
            ..TextSpec::default()
        }
    }

    #[test]
    fn a_saved_style_keeps_the_look_not_the_words() {
        let mut book = StyleBook::default();
        let (name, replaced) = book.save_style("  Loud   title ", &loud()).unwrap();
        assert_eq!((name.as_str(), replaced), ("Loud title", false));
        let kept = &book.get("loud TITLE").unwrap().spec;
        assert_eq!(kept.content, SAMPLE_TEXT);
        assert_eq!(
            (kept.size, kept.color, kept.outline),
            (99, [1, 2, 3, 255], 70)
        );
        assert_eq!(kept.active, None);
    }

    #[test]
    fn saving_under_a_name_in_use_replaces_it() {
        let mut book = StyleBook::default();
        book.save_style("A", &loud()).unwrap();
        let mut other = loud();
        other.size = 12;
        let (_, replaced) = book.save_style("a", &other).unwrap();
        assert!(replaced);
        assert_eq!(book.styles.len(), 1);
        assert_eq!(book.styles[0].spec.size, 12);
    }

    #[test]
    fn names_are_checked() {
        assert!(clean_name("   ").is_err());
        assert!(clean_name(&"x".repeat(MAX_NAME + 1)).is_err());
        assert_eq!(clean_name("a\tb\nc").unwrap(), "a b c");
        let mut book = StyleBook::default();
        book.save_style("One", &loud()).unwrap();
        book.save_style("Two", &loud()).unwrap();
        assert!(book.rename("One", "two").is_err());
        assert_eq!(book.rename("One", "Uno").unwrap(), "Uno");
        assert!(book.rename("Missing", "X").is_err());
        assert!(book.delete("uno") && !book.delete("uno"));
    }

    #[test]
    fn applying_a_style_keeps_the_clips_own_words() {
        let mut book = StyleBook::default();
        book.save_style("Loud", &loud()).unwrap();
        let target = TextSpec {
            content: "Hello".into(),
            active: Some((1, 2)),
            ..TextSpec::default()
        };
        let styled = restyle(&target, &book.get("Loud").unwrap().spec);
        assert_eq!(styled.content, "Hello");
        assert_eq!(styled.active, Some((1, 2)));
        assert_eq!((styled.size, styled.outline), (99, 70));
    }

    #[test]
    fn the_list_survives_a_round_trip_and_a_bad_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(StyleBook::load(dir.path()), StyleBook::default());
        let mut book = StyleBook::default();
        book.save_style("Loud", &loud()).unwrap();
        book.save(dir.path()).unwrap();
        assert_eq!(StyleBook::load(dir.path()), book);
        std::fs::write(dir.path().join(FILE_NAME), b"not json").unwrap();
        assert_eq!(StyleBook::load(dir.path()), StyleBook::default());
    }
}
