//! The application icon: the official Kani artwork (`assets/kani.svg` in the
//! repository root), rendered to PNGs that are compiled in so no SVG loader
//! is needed at run time.

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;

const PNG_512: &[u8] = include_bytes!("../assets/icons/kani-512.png");
const PNG_256: &[u8] = include_bytes!("../assets/icons/kani-256.png");
const PNG_128: &[u8] = include_bytes!("../assets/icons/kani-128.png");
const PNG_64: &[u8] = include_bytes!("../assets/icons/kani-64.png");

/// The icon as a picture for dialogs.
pub fn texture() -> gdk::Texture {
    gdk::Texture::from_bytes(&glib::Bytes::from_static(PNG_256))
        .expect("the compiled-in icon is a valid PNG")
}

/// The icon name, the same as the application id, so that GNOME can match a
/// running window to its launcher and show this icon in the taskbar.
const ICON_NAME: &str = "dev.cutroom.Kani";

const SIZES: [(u32, &[u8]); 4] = [(512, PNG_512), (256, PNG_256), (128, PNG_128), (64, PNG_64)];

/// Write the icons as `<root>/hicolor/<size>/apps/<ICON_NAME>.png`.
fn write_theme(root: &std::path::Path) {
    for (size, bytes) in SIZES {
        let dir = root.join(format!("hicolor/{size}x{size}/apps"));
        if std::fs::create_dir_all(&dir).is_ok() {
            let file = dir.join(format!("{ICON_NAME}.png"));
            if std::fs::read(&file).map_or(true, |old| old != bytes) {
                let _ = std::fs::write(&file, bytes);
            }
        }
    }
}

/// The launcher file GNOME needs to show the icon of a running window. Its
/// name must be the application id.
#[cfg(target_os = "linux")]
fn desktop_entry(exe: &std::path::Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Kani\nComment=Time-bending edits made simple.\n\
         Exec=\"{}\" %F\nIcon={ICON_NAME}\nTerminal=false\nCategories=AudioVideo;Video;AudioVideoEditing;\n\
         StartupWMClass={ICON_NAME}\nStartupNotify=true\n",
        exe.display()
    )
}

/// Put the launcher and the icons in the user's own folders
/// (`~/.local/share/applications` and `~/.local/share/icons`), once and again
/// whenever the program moves. Nothing is touched outside the user's home.
#[cfg(target_os = "linux")]
fn install_launcher() {
    if std::env::var_os("KANI_NO_LAUNCHER").is_some() {
        return;
    }
    let Some(data) = glib::user_data_dir().to_str().map(std::path::PathBuf::from) else {
        return;
    };
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    write_theme(&data.join("icons"));
    let apps = data.join("applications");
    if std::fs::create_dir_all(&apps).is_ok() {
        let file = apps.join(format!("{ICON_NAME}.desktop"));
        let text = desktop_entry(&exe);
        if std::fs::read_to_string(&file).map_or(true, |old| old != text) {
            let _ = std::fs::write(&file, text);
        }
    }
}

/// Make Kani's icon the icon of every window: the PNGs go to the user's icon
/// folder (and a temporary theme as a fallback), with a launcher entry so the
/// taskbar shows it too.
pub fn install_as_default() {
    let root = std::env::temp_dir().join(format!("kani-icons-{}", env!("CARGO_PKG_VERSION")));
    write_theme(&root);
    #[cfg(target_os = "linux")]
    install_launcher();
    if let Some(display) = gdk::Display::default() {
        gtk::IconTheme::for_display(&display).add_search_path(&root);
    }
    gtk::Window::set_default_icon_name(ICON_NAME);
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_compiled_in_icons_are_real_pictures() {
        for bytes in [
            super::PNG_512,
            super::PNG_256,
            super::PNG_128,
            super::PNG_64,
        ] {
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod launcher_tests {
    #[test]
    fn the_launcher_is_named_after_the_application_id() {
        let text = super::desktop_entry(std::path::Path::new("/opt/kani/kani"));
        assert!(text.contains("Exec=\"/opt/kani/kani\" %F"));
        assert!(text.contains("Icon=dev.cutroom.Kani"));
        assert!(text.contains("StartupWMClass=dev.cutroom.Kani"));
    }
}
