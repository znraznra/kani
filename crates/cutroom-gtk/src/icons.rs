//! Interface icons: the Maho icon library (MIT, from Tabler Icons), bundled as
//! SVG in `assets/icons`. They are drawn in the widget's own text colour, so they
//! follow the light/dark theme and the accent colour of a selected tab.
//! Every icon the editor uses is named here.

use gtk4 as gtk;
use gtk4::gdk::prelude::GdkCairoContextExt;
use gtk4::gdk_pixbuf::Pixbuf;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use std::cell::RefCell;
use std::collections::HashMap;

macro_rules! bundle {
    ($($name:literal),* $(,)?) => {
        const BUNDLE: &[(&str, &str)] = &[
            $(($name, include_str!(concat!("../assets/icons/", $name, ".svg")))),*
        ];
    };
}

bundle!(
    "plus",
    "arrow-back-up",
    "arrow-forward-up",
    "trash",
    "cut",
    "minus",
    "player-skip-back",
    "player-skip-forward",
    "arrows-maximize",
    "zoom-scan",
    "microphone",
    "zoom-in",
    "zoom-out",
    "magnet",
    "list",
    "layout-grid",
    "rotate",
    "menu-2",
    "keyboard",
    "upload",
    "box-multiple",
    "link",
    "rhombus",
    "rhombus-filled",
    "align-left",
    "align-center",
    "align-right",
    "flip-horizontal",
    "flip-vertical",
    "folder-open",
    "clipboard",
    "movie",
    "music",
    "typography",
    "badge-cc",
    "mood-smile",
    "sparkles",
    "arrows-exchange",
    "color-filter",
    "adjustments",
    "filter",
    "player-play-filled",
    "player-pause-filled",
    "pointer",
    "blade",
    "video-plus",
    "music-plus",
    "arrows-horizontal",
    "eye",
    "eye-off",
    "volume",
    "volume-off",
    "lock",
    "lock-open",
);

pub const IMPORT: &str = "plus";
pub const UNDO: &str = "arrow-back-up";
pub const REDO: &str = "arrow-forward-up";
pub const DELETE: &str = "trash";
pub const RIPPLE_DELETE: &str = "minus";
pub const SPLIT: &str = "cut";
pub const SELECT_TOOL: &str = "pointer";
pub const SPLIT_TOOL: &str = "blade";
pub const ADD_VIDEO_TRACK: &str = "video-plus";
pub const ADD_AUDIO_TRACK: &str = "music-plus";
pub const PLAY: &str = "player-play-filled";
pub const PAUSE: &str = "player-pause-filled";
pub const TO_START: &str = "player-skip-back";
pub const TO_END: &str = "player-skip-forward";
pub const FULLSCREEN: &str = "arrows-maximize";
pub const FIT: &str = "zoom-scan";
pub const FIT_TIMELINE: &str = "arrows-horizontal";
pub const MIC: &str = "microphone";
pub const ZOOM_IN: &str = "zoom-in";
pub const ZOOM_OUT: &str = "zoom-out";
pub const SNAP: &str = "magnet";
pub const SCRUB_SOUND: &str = "volume";
pub const LIST: &str = "list";
pub const GRID: &str = "layout-grid";
pub const RESET: &str = "rotate";
pub const MENU: &str = "menu-2";
pub const KEYBOARD: &str = "keyboard";
pub const EXPORT: &str = "upload";
pub const GROUP: &str = "box-multiple";
pub const LINK: &str = "link";
/// A plain diamond (not the gem): a keyframe.
pub const KEYFRAME: &str = "rhombus";
/// The same, filled: there is a keyframe at the playhead.
pub const KEYFRAME_ON: &str = "rhombus-filled";
pub const ALIGN_LEFT: &str = "align-left";
pub const ALIGN_CENTER: &str = "align-center";
pub const ALIGN_RIGHT: &str = "align-right";
pub const MIRROR: &str = "flip-horizontal";
pub const FLIP: &str = "flip-vertical";
pub const FOLDER: &str = "folder-open";
pub const PASTE: &str = "clipboard";
pub const FILTER: &str = "filter";

/// The icon for a library tab.
pub fn category(name: &str) -> &'static str {
    match name {
        "Media" => "movie",
        "Audio" => "music",
        "Text" => "typography",
        "Captions" => "badge-cc",
        "Stickers" => "mood-smile",
        "Effects" => "sparkles",
        "Transitions" => "arrows-exchange",
        "Filters" => "color-filter",
        _ => "adjustments",
    }
}

fn svg_for(name: &str) -> Option<&'static str> {
    BUNDLE.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

/// The SVG text with its colour and pixel size filled in.
fn render_svg(svg: &str, hex: &str, pixels: i32) -> String {
    svg.replace("currentColor", hex)
        .replacen("width=\"24\"", &format!("width=\"{pixels}\""), 1)
        .replacen("height=\"24\"", &format!("height=\"{pixels}\""), 1)
}

fn hex_of(c: &gtk::gdk::RGBA) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        byte(c.red()),
        byte(c.green()),
        byte(c.blue())
    )
}

thread_local! {
    static CACHE: RefCell<HashMap<(String, String, i32), Pixbuf>> = RefCell::new(HashMap::new());
}

fn pixbuf(name: &str, hex: &str, pixels: i32) -> Option<Pixbuf> {
    let key = (name.to_owned(), hex.to_owned(), pixels);
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return Some(hit);
    }
    let svg = render_svg(svg_for(name)?, hex, pixels);
    let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(svg.into_bytes()));
    let pb = Pixbuf::from_stream(&stream, None::<&gio::Cancellable>).ok()?;
    CACHE.with(|c| c.borrow_mut().insert(key, pb.clone()));
    Some(pb)
}

/// Paint an icon straight onto a cairo context (for the timeline canvas).
pub fn paint(
    cr: &gtk::cairo::Context,
    name: &str,
    color: (f64, f64, f64),
    x: f64,
    y: f64,
    size: i32,
    scale: i32,
) {
    let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let hex = format!(
        "#{:02x}{:02x}{:02x}",
        byte(color.0),
        byte(color.1),
        byte(color.2)
    );
    if let Some(pb) = pixbuf(name, &hex, size * scale) {
        cr.save().ok();
        cr.translate(x, y);
        cr.scale(1.0 / f64::from(scale), 1.0 / f64::from(scale));
        cr.set_source_pixbuf(&pb, 0.0, 0.0);
        let _ = cr.paint();
        cr.restore().ok();
    }
}

/// An icon at `size` logical pixels, in the colour of the text around it.
pub fn icon(name: &'static str, size: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(size);
    area.set_content_height(size);
    area.set_halign(gtk::Align::Center);
    area.set_valign(gtk::Align::Center);
    area.set_can_target(false);
    area.set_draw_func(move |a, cr, w, h| {
        let scale = a.scale_factor().max(1);
        let hex = hex_of(&a.color());
        if let Some(pb) = pixbuf(name, &hex, size * scale) {
            cr.save().ok();
            let (dx, dy) = (f64::from(w - size) / 2.0, f64::from(h - size) / 2.0);
            cr.translate(dx, dy);
            cr.scale(1.0 / f64::from(scale), 1.0 / f64::from(scale));
            cr.set_source_pixbuf(&pb, 0.0, 0.0);
            let _ = cr.paint();
            cr.restore().ok();
        }
    });
    // A tab turning accent-coloured, a button going grey: repaint on any state change.
    area.connect_state_flags_changed(|a, _| a.queue_draw());
    area
}

/// A flat icon-only button.
pub fn button(name: &'static str, tip: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.set_child(Some(&icon(name, 16)));
    b.add_css_class("flat");
    b.add_css_class("cc-icon-button");
    b.set_tooltip_text(Some(tip));
    b
}

/// Swap the icon of a button made with [`button`]; does nothing if unchanged.
pub fn set_button_icon(b: &gtk::Button, name: &'static str, size: i32) {
    if b.widget_name() != name {
        b.set_widget_name(name);
        b.set_child(Some(&icon(name, size)));
    }
}

/// An icon-only button for something that does not exist yet: shown, greyed out.
pub fn placeholder(name: &'static str, what: &str) -> gtk::Button {
    let b = button(name, &format!("{what} (not available yet)"));
    b.set_sensitive(false);
    b
}

/// A toggle button showing only an icon.
pub fn toggle(name: &'static str, tip: &str) -> gtk::ToggleButton {
    let b = gtk::ToggleButton::new();
    b.set_child(Some(&icon(name, 16)));
    b.add_css_class("flat");
    b.add_css_class("cc-icon-button");
    b.set_tooltip_text(Some(tip));
    b
}

/// An icon with a text label beside it, for buttons such as Import and Export.
pub fn with_label(name: &'static str, label: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.append(&icon(name, 16));
    row.append(&gtk::Label::new(Some(label)));
    row
}

/// An icon above a small caption, for the tab strips.
pub fn tab(name: &'static str, label: &str) -> gtk::ToggleButton {
    let b = gtk::ToggleButton::new();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let text = gtk::Label::new(Some(label));
    text.add_css_class("cc-tab-label");
    column.append(&icon(name, 18));
    column.append(&text);
    b.set_child(Some(&column));
    b.add_css_class("flat");
    b.add_css_class("cc-tab");
    b.set_hexpand(true);
    b
}

/// A text-only tab (the inspector's Video / Audio / Speed strip).
pub fn text_tab(label: &str) -> gtk::ToggleButton {
    let b = gtk::ToggleButton::with_label(label);
    b.add_css_class("flat");
    b.add_css_class("cc-text-tab");
    b.set_hexpand(true);
    b
}

/// Make a widget look and act unavailable.
pub fn disable(w: &impl IsA<gtk::Widget>, what: &str) {
    w.set_sensitive(false);
    w.set_tooltip_text(Some(&format!("{what} (not available yet)")));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_icon_is_bundled_and_recolourable() {
        for name in [
            IMPORT,
            UNDO,
            REDO,
            DELETE,
            RIPPLE_DELETE,
            SPLIT,
            PLAY,
            PAUSE,
            TO_START,
            TO_END,
            FULLSCREEN,
            FIT,
            MIC,
            ZOOM_IN,
            ZOOM_OUT,
            SNAP,
            LIST,
            GRID,
            RESET,
            MENU,
            KEYBOARD,
            EXPORT,
            GROUP,
            LINK,
            KEYFRAME,
            KEYFRAME_ON,
            ALIGN_LEFT,
            ALIGN_CENTER,
            ALIGN_RIGHT,
            MIRROR,
            FLIP,
            FOLDER,
            PASTE,
            FILTER,
        ] {
            let svg = svg_for(name).unwrap_or_else(|| panic!("{name} missing"));
            assert!(
                svg.contains("currentColor"),
                "{name} follows the text colour"
            );
        }
        for tab in [
            "Media",
            "Audio",
            "Text",
            "Captions",
            "Stickers",
            "Effects",
            "Transitions",
            "Filters",
            "Adjust",
        ] {
            assert!(svg_for(category(tab)).is_some(), "{tab}");
        }
    }

    #[test]
    fn rendering_sets_colour_and_size() {
        let svg = render_svg(svg_for("plus").unwrap(), "#ff0000", 48);
        assert!(svg.contains("#ff0000") && !svg.contains("currentColor"));
        assert!(svg.contains("width=\"48\"") && svg.contains("height=\"48\""));
    }
}
