//! Rules for what may be used as a project name (it becomes a folder name).

const FORBIDDEN: &str = "/\\:*?\"<>|";
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Why `name` can't be a project (folder) name, or `None` if it is fine.
/// The rules are the strictest of Windows, macOS and Linux so a project made
/// on one system can be copied to another.
pub fn folder_name_problem(name: &str) -> Option<&'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Some("Give the project a name.");
    }
    if name
        .chars()
        .any(|c| c.is_control() || FORBIDDEN.contains(c))
    {
        return Some("The name can't contain / \\ : * ? \" < > |");
    }
    if name.ends_with('.') {
        return Some("The name can't end with a dot.");
    }
    let stem = name.split('.').next().unwrap_or(name);
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        return Some("Windows reserves that name. Pick another.");
    }
    if name.chars().count() > 100 {
        return Some("Keep the name under 100 characters.");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_names_are_fine() {
        for ok in [
            "My project",
            "Spring promo 2026",
            "a",
            "v1.2 final",
            "日本語のプロジェクト",
        ] {
            assert_eq!(folder_name_problem(ok), None, "{ok}");
        }
    }

    #[test]
    fn empty_and_blank_names_are_rejected() {
        assert!(folder_name_problem("").is_some());
        assert!(folder_name_problem("   ").is_some());
    }

    #[test]
    fn path_characters_are_rejected() {
        for bad in [
            "a/b",
            "a\\b",
            "a:b",
            "a*b",
            "a?b",
            "a\"b",
            "a<b",
            "a>b",
            "a|b",
            "tab\there",
        ] {
            assert!(folder_name_problem(bad).is_some(), "{bad:?}");
        }
    }

    #[test]
    fn dot_names_and_trailing_dots_are_rejected() {
        for bad in [".", "..", "name."] {
            assert!(folder_name_problem(bad).is_some(), "{bad:?}");
        }
    }

    #[test]
    fn windows_device_names_are_rejected_in_any_case_or_with_extension() {
        for bad in ["con", "NUL", "Com1", "lpt9", "aux.txt", "PRN.final"] {
            assert!(folder_name_problem(bad).is_some(), "{bad:?}");
        }
        assert_eq!(folder_name_problem("console"), None);
        assert_eq!(folder_name_problem("COM10"), None);
    }

    #[test]
    fn very_long_names_are_rejected() {
        assert!(folder_name_problem(&"x".repeat(101)).is_some());
        assert_eq!(folder_name_problem(&"x".repeat(100)), None);
    }
}
