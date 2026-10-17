//! Look and feel: dark/light selection (including following the system) and
//! the colours used for custom-drawn areas (timeline, covers).

use cutroom_core::settings::ThemeChoice;
use gtk4 as gtk;
use gtk4::gio;
use gtk4::prelude::*;

pub type Rgb = (f64, f64, f64);

pub const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)
}

pub const ACCENT: Rgb = rgb(38, 198, 176);
pub const PLAYHEAD: Rgb = rgb(255, 96, 82);
pub const PLAYHEAD_DARK: Rgb = rgb(255, 255, 255);
pub const VIDEO_CLIP: Rgb = rgb(20, 118, 128);
/// Text clips: words, not footage.
pub const TEXT_CLIP: Rgb = rgb(112, 84, 168);
pub const AUDIO_CLIP: Rgb = rgb(30, 78, 138);
pub const INVALID: Rgb = rgb(200, 60, 60);

/// Colours for the custom-drawn timeline.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    #[cfg_attr(not(test), allow(dead_code))]
    pub dark: bool,
    pub canvas_bg: Rgb,
    pub ruler_bg: Rgb,
    pub row_even: Rgb,
    pub row_odd: Rgb,
    pub header_bg: Rgb,
    pub tick_major: Rgb,
    pub tick_minor: Rgb,
    pub ruler_text: Rgb,
    pub header_text: Rgb,
    pub clip_text: Rgb,
    pub clip_subtext: Rgb,
    pub clip_text_muted: Rgb,
    pub snap_guide: Rgb,
    pub selected_outline: Rgb,
    pub playhead: Rgb,
}

impl Palette {
    pub const DARK: Palette = Palette {
        dark: true,
        canvas_bg: rgb(24, 25, 29),
        ruler_bg: rgb(24, 25, 29),
        row_even: rgb(29, 30, 35),
        row_odd: rgb(29, 30, 35),
        header_bg: rgb(27, 28, 33),
        tick_major: rgb(140, 140, 140),
        tick_minor: rgb(80, 80, 80),
        ruler_text: rgb(170, 170, 170),
        header_text: rgb(225, 225, 225),
        clip_text: rgb(255, 255, 255),
        clip_subtext: rgb(214, 220, 230),
        clip_text_muted: rgb(190, 190, 190),
        snap_guide: rgb(255, 214, 90),
        selected_outline: rgb(255, 255, 255),
        playhead: PLAYHEAD_DARK,
    };

    pub const LIGHT: Palette = Palette {
        dark: false,
        canvas_bg: rgb(232, 234, 239),
        ruler_bg: rgb(222, 225, 231),
        row_even: rgb(242, 243, 246),
        row_odd: rgb(236, 238, 242),
        header_bg: rgb(250, 251, 252),
        tick_major: rgb(110, 110, 110),
        tick_minor: rgb(160, 160, 160),
        ruler_text: rgb(70, 70, 70),
        header_text: rgb(40, 40, 40),
        clip_text: rgb(255, 255, 255),
        clip_subtext: rgb(232, 236, 244),
        clip_text_muted: rgb(60, 60, 60),
        snap_guide: rgb(200, 120, 0),
        selected_outline: rgb(0, 0, 0),
        playhead: PLAYHEAD,
    };

    pub fn of(dark: bool) -> Palette {
        if dark {
            Palette::DARK
        } else {
            Palette::LIGHT
        }
    }
}

pub fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}

/// WCAG contrast ratio between two colours (1.0 to 21.0).
#[cfg(test)]
pub fn contrast_ratio(a: Rgb, b: Rgb) -> f64 {
    fn channel(c: f64) -> f64 {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    fn luminance(c: Rgb) -> f64 {
        0.2126 * channel(c.0) + 0.7152 * channel(c.1) + 0.0722 * channel(c.2)
    }
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Does the desktop ask for a dark appearance?
pub fn system_prefers_dark() -> bool {
    if let Some(source) = gio::SettingsSchemaSource::default() {
        if let Some(schema) = source.lookup("org.gnome.desktop.interface", true) {
            if schema.has_key("color-scheme") {
                let settings = gio::Settings::new("org.gnome.desktop.interface");
                return settings.string("color-scheme") == "prefer-dark";
            }
        }
    }
    #[cfg(windows)]
    {
        // 0 means apps should use the dark theme.
        if let Ok(output) = std::process::Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                "/v",
                "AppsUseLightTheme",
            ])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            return text.contains("0x0");
        }
    }
    false
}

/// Whether `choice` resolves to a dark appearance right now.
pub fn is_dark(choice: ThemeChoice, system_dark: bool) -> bool {
    match choice {
        ThemeChoice::Dark => true,
        ThemeChoice::Light => false,
        ThemeChoice::System => system_dark,
    }
}

/// Tag the window so the dark-only styling applies.
pub fn mark_window(window: &impl IsA<gtk::Widget>, dark: bool) {
    if dark {
        window.add_css_class("cc-dark");
    } else {
        window.remove_css_class("cc-dark");
    }
}

/// Switch GTK to the chosen appearance. Returns whether it is dark.
pub fn apply(choice: ThemeChoice) -> bool {
    let dark = is_dark(choice, system_prefers_dark());
    if let Some(settings) = gtk::Settings::default() {
        // GTK 4.20 replaced "prefer dark theme" with an interface colour
        // scheme and warns when the old one is set; use whichever this GTK has.
        let set_new = settings
            .find_property("gtk-interface-color-scheme")
            .and_then(|spec| {
                let class = gtk::glib::EnumClass::with_type(spec.value_type())?;
                class.to_value_by_nick(if dark { "dark" } else { "light" })
            })
            .map(|value| settings.set_property_from_value("gtk-interface-color-scheme", &value))
            .is_some();
        if !set_new {
            settings.set_gtk_application_prefer_dark_theme(dark);
        }
    }
    dark
}

const CSS: &str = r#"
* { font-family: "Inter", sans-serif; }
.monospace { font-family: monospace; }
.timecode, .cc-timecode-now, .cc-timecode-total { font-feature-settings: "tnum"; }
.cutroom-title { font-size: 28px; font-weight: 700; }
.cutroom-card {
    padding: 8px;
    border-radius: 10px;
    background: alpha(currentColor, 0.06);
}
.cutroom-card:hover { background: alpha(currentColor, 0.12); }
.cutroom-card.selected {
    background: alpha(@theme_selected_bg_color, 0.30);
    box-shadow: inset 0 0 0 2px @theme_selected_bg_color;
}
.cutroom-row { padding: 6px 10px; border-radius: 6px; }
.cutroom-row:hover { background: alpha(currentColor, 0.08); }
.cutroom-row.selected { background: alpha(@theme_selected_bg_color, 0.30); }
.preview-stage {
    background: alpha(currentColor, 0.16);
    border-radius: 4px;
    padding: 14px;
}
.cutroom-compact-menu button.flat {
    min-height: 22px;
    padding: 3px 10px;
    border-radius: 4px;
}
/* A menu with one item is as tall as that item. (GTK's overlay scrollbar asks
   for 46 px, which left a gap under the only item of the Settings menu.) */
popover.menu scrollbar.vertical,
popover.menu scrollbar.vertical range,
popover.menu scrollbar.vertical slider,
popover.menu scrollbar.vertical trough {
    min-height: 0;
}
.cutroom-compact-menu modelbutton {
    min-height: 24px;
    padding-top: 2px;
    padding-bottom: 2px;
}
.cc-toast {
    padding: 8px 16px;
    border-radius: 18px;
    background: rgba(20, 20, 24, 0.85);
    color: #ffffff;
}
.preview-fullscreen { background: #000000; }
.cc-fullscreen-bar {
    padding: 8px 16px;
    background: rgba(15, 16, 19, 0.85);
    color: #ffffff;
}
.cc-proxy-badge {
    font-size: 9px;
    font-weight: 700;
    padding: 0 4px;
    border-radius: 3px;
    background: #26c6b0;
    color: #0f1013;
}
.cutroom-selected {
    box-shadow: inset 0 0 0 2px #26c6b0;
    border-radius: 6px;
}
.preview-canvas {
    background: #000000;
    box-shadow: 0 0 0 1px alpha(currentColor, 0.55);
}

/* ---- editor layout (CapCut-like: separate rounded panels on a darker window) ---- */
.cc-panel {
    background: alpha(currentColor, 0.04);
    border: 1px solid alpha(currentColor, 0.12);
    border-radius: 8px;
}
window.cc-dark { background: #0f1013; }
.cc-dark .cc-panel { background: #1b1c21; border-color: #26272d; }
.cc-menu-button { padding: 2px 10px; min-height: 22px; }
.cc-brand { font-weight: 700; font-size: 15px; }
.cc-project-title { font-weight: 600; }
.cc-export {
    background: #26c6b0;
    color: #06231f;
    font-weight: 700;
    border-radius: 6px;
    padding: 4px 14px;
}
.cc-export:disabled { opacity: 0.5; }
.cc-panel-heading { padding: 8px 12px 4px 12px; font-weight: 600; font-size: 12px; opacity: 0.85; }
.cc-tab { padding: 6px 2px; border-radius: 6px; min-width: 0; }
.cc-tab-label { font-size: 10px; }
.cc-tab:checked { background: transparent; color: #26c6b0; }
.cc-tab:disabled { opacity: 0.38; }
.cc-text-tab { padding: 4px 6px; border-radius: 6px; font-weight: 600; font-size: 12px; min-height: 22px; }
.cc-text-tab:checked { background: transparent; color: #26c6b0; box-shadow: inset 0 -2px 0 #26c6b0; border-radius: 0; }
.cc-text-tab:disabled { opacity: 0.38; }
.cc-subtabs { background: alpha(currentColor, 0.07); border-radius: 6px; padding: 2px; }
.cc-subtab { padding: 2px 8px; border-radius: 5px; font-size: 12px; min-height: 20px; }
.cc-subtab:checked { background: alpha(currentColor, 0.16); }
.cc-subtab:disabled { opacity: 0.38; }
.cc-sidebar-item { padding: 4px 10px; border-radius: 5px; font-size: 12px; min-height: 22px; }
.cc-sidebar-item:checked { background: alpha(currentColor, 0.14); }
.cc-sidebar-item:disabled { opacity: 0.38; }
.cc-icon-button { min-width: 24px; min-height: 24px; padding: 3px; border-radius: 5px; }
.cc-icon-button:disabled { opacity: 0.38; }
.cc-icon-button.animated { color: #26c6b0; }
.cc-icon-button:checked { color: #26c6b0; background: alpha(#26c6b0, 0.16); }
.cc-text-style { padding: 6px 10px; }
.cc-text-entry { border: 1px solid alpha(currentColor, 0.18); border-radius: 6px; }
.cc-import-button { background: alpha(currentColor, 0.1); border-radius: 6px; padding: 3px 12px; font-weight: 600; }
.cc-section { padding: 8px 0; border-bottom: 1px solid alpha(currentColor, 0.10); }
.cc-section-title { font-weight: 700; font-size: 13px; }
.cc-section.disabled-section { opacity: 0.45; }
.cc-field-label { font-size: 12px; opacity: 0.75; }
.cc-effect-group { margin: 2px 0; }
.cc-effect-head check { min-height: 16px; min-width: 16px; }
.cc-effect-head label { font-size: 12px; font-weight: 600; }
.cc-fold { min-height: 20px; min-width: 20px; padding: 0 4px; }
.cc-spin { min-width: 0; font-size: 12px; }
spinbutton.cc-spin { min-height: 24px; }
spinbutton.cc-spin button { min-width: 14px; min-height: 0; padding: 0 1px; -gtk-icon-size: 9px; }
spinbutton.cc-spin text { min-width: 0; }
.cc-timecode-now { color: #26c6b0; }
.cc-timecode-total { opacity: 0.7; }
.cc-play { min-width: 26px; min-height: 26px; border-radius: 13px; }
.cc-transport .cc-icon-button { min-width: 22px; min-height: 22px; padding: 2px; }
.cc-transport .cc-play { min-width: 26px; min-height: 26px; padding: 2px; }
button.cc-pill, .cc-pill { font-size: 11px; padding: 0 8px; min-height: 0; min-width: 0; margin: 0; border-radius: 5px; border: 1px solid alpha(currentColor, 0.4); }
.status-bar { padding: 3px 10px; }
.about-compact * { font-size: 8.5pt; }
.about-compact button { padding: 2px 8px; min-height: 0; }
.panel-title { font-weight: 700; }
.timecode { font-feature-settings: "tnum"; }
.drop-overlay {
    background: alpha(#000000, 0.65);
    color: #ffffff;
    font-size: 22px;
    font-weight: 600;
}
"#;

/// Install the application's stylesheet.
pub fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_labels_are_readable_in_both_themes() {
        for pal in [Palette::DARK, Palette::LIGHT] {
            for fill in [VIDEO_CLIP, AUDIO_CLIP] {
                let ratio = contrast_ratio(pal.clip_text, fill);
                assert!(ratio >= 4.5, "label on clip: {ratio}");
            }
        }
    }

    #[test]
    fn ruler_and_header_text_are_readable_in_both_themes() {
        for pal in [Palette::DARK, Palette::LIGHT] {
            assert!(contrast_ratio(pal.ruler_text, pal.ruler_bg) >= 4.5);
            assert!(contrast_ratio(pal.header_text, pal.header_bg) >= 4.5);
        }
    }

    #[test]
    fn muted_clip_labels_are_readable() {
        for pal in [Palette::DARK, Palette::LIGHT] {
            let row = if pal.dark { pal.row_even } else { pal.row_odd };
            for fill in [VIDEO_CLIP, AUDIO_CLIP] {
                let muted = mix(fill, row, 0.6);
                assert!(contrast_ratio(pal.clip_text_muted, muted) >= 3.0);
            }
        }
    }

    #[test]
    fn the_choice_resolves_against_the_system_setting() {
        assert!(is_dark(ThemeChoice::Dark, false));
        assert!(!is_dark(ThemeChoice::Light, true));
        assert!(is_dark(ThemeChoice::System, true));
        assert!(!is_dark(ThemeChoice::System, false));
    }

    #[test]
    fn mix_blends_linearly() {
        assert_eq!(
            mix((0.0, 0.0, 0.0), (1.0, 0.5, 0.25), 0.5),
            (0.5, 0.25, 0.125)
        );
        assert_eq!(
            contrast_ratio((0.0, 0.0, 0.0), (1.0, 1.0, 1.0)).round(),
            21.0
        );
    }
}
