//! The interface typeface: Inter (SIL Open Font License 1.1), bundled with the
//! program so it looks the same on every machine. Call [`install`] before GTK starts.

use std::fs;
use std::path::PathBuf;

const FILES: &[(&str, &[u8])] = &[
    (
        "Inter-Regular.otf",
        include_bytes!("../assets/fonts/Inter-Regular.otf"),
    ),
    (
        "Inter-Medium.otf",
        include_bytes!("../assets/fonts/Inter-Medium.otf"),
    ),
    (
        "Inter-SemiBold.otf",
        include_bytes!("../assets/fonts/Inter-SemiBold.otf"),
    ),
    (
        "Inter-Bold.otf",
        include_bytes!("../assets/fonts/Inter-Bold.otf"),
    ),
];

/// The family name used in CSS and drawing code.
pub const FAMILY: &str = "Inter";

/// Write the font files to a per-version folder (once) and return it.
fn extract() -> Option<PathBuf> {
    let dir = std::env::temp_dir().join(format!("kani-fonts-{}", env!("CARGO_PKG_VERSION")));
    fs::create_dir_all(&dir).ok()?;
    for (name, bytes) in FILES {
        let path = dir.join(name);
        let up_to_date = fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64);
        if !up_to_date {
            fs::write(&path, bytes).ok()?;
        }
    }
    Some(dir)
}

/// The fontconfig file that adds `dir` to whatever fonts the system already has.
/// (Not needed on Windows, where the font is registered with the system instead.)
#[cfg_attr(windows, allow(dead_code))]
pub fn fontconfig_xml(dir: &str, base: Option<&str>) -> String {
    let base = base.unwrap_or("/etc/fonts/fonts.conf");
    format!(
        "<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"fonts.dtd\">\n<fontconfig>\n  <include ignore_missing=\"yes\">{base}</include>\n  <dir>{dir}</dir>\n</fontconfig>\n"
    )
}

/// Make Inter available to the toolkit. Best effort: without it the interface
/// falls back to the system sans-serif font.
pub fn install() {
    let Some(dir) = extract() else {
        return;
    };
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        extern "system" {
            fn AddFontResourceExW(
                name: *const u16,
                flags: u32,
                reserved: *mut core::ffi::c_void,
            ) -> i32;
        }
        const FR_PRIVATE: u32 = 0x10;
        for (name, _) in FILES {
            let wide: Vec<u16> = dir
                .join(name)
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            // SAFETY: `wide` is a NUL-terminated path that outlives the call.
            unsafe { AddFontResourceExW(wide.as_ptr(), FR_PRIVATE, core::ptr::null_mut()) };
        }
    }
    #[cfg(not(windows))]
    {
        let existing = std::env::var("FONTCONFIG_FILE").ok();
        let conf = dir.join("fonts.conf");
        let xml = fontconfig_xml(&dir.to_string_lossy(), existing.as_deref());
        if fs::write(&conf, xml).is_ok() {
            std::env::set_var("FONTCONFIG_FILE", &conf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_weights_are_bundled() {
        assert_eq!(FILES.len(), 4);
        for (name, bytes) in FILES {
            assert!(bytes.len() > 100_000, "{name} is a real font");
            assert_eq!(&bytes[..4], b"OTTO", "{name} is OpenType");
        }
    }

    #[test]
    fn the_config_keeps_system_fonts_and_adds_ours() {
        let xml = fontconfig_xml("/tmp/fonts", None);
        assert!(xml.contains("/etc/fonts/fonts.conf") && xml.contains("<dir>/tmp/fonts</dir>"));
        assert!(fontconfig_xml("/x", Some("/my/own.conf")).contains("/my/own.conf"));
    }
}
