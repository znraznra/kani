//! The measurements every part of the interface shares, taken from the media
//! panel (see docs/UI_STYLE.md). New panels, windows and rows use these
//! instead of their own numbers, and the stylesheet in `theme.rs` holds the
//! matching text sizes and button padding.

use gtk4 as gtk;
use gtk4::prelude::*;

/// Space between controls in a row or column.
pub const GAP: i32 = 6;
/// Space between buttons that belong together (a toolbar row).
#[allow(dead_code)]
pub const TIGHT: i32 = 4;
/// Inner margin of a panel or page.
pub const PAD: i32 = 8;
/// Inner margin of a dialog window.
pub const DIALOG_PAD: i32 = 14;
/// Icon size inside buttons and lists.
pub const ICON: i32 = 16;

/// Give a panel or page its margins.
pub fn pad(w: &impl IsA<gtk::Widget>) {
    w.set_margin_top(PAD);
    w.set_margin_bottom(PAD);
    w.set_margin_start(PAD);
    w.set_margin_end(PAD);
}

/// Give a dialog's content its margins.
pub fn pad_dialog(w: &impl IsA<gtk::Widget>) {
    w.set_margin_top(DIALOG_PAD - 2);
    w.set_margin_bottom(DIALOG_PAD - 2);
    w.set_margin_start(DIALOG_PAD);
    w.set_margin_end(DIALOG_PAD);
}

fn label_factory(ellipsize: bool) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        if ellipsize {
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_width_chars(1);
        }
        item.set_child(Some(&label));
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let (Some(label), Some(text)) = (
            item.child().and_downcast::<gtk::Label>(),
            item.item().and_downcast::<gtk::StringObject>(),
        ) {
            label.set_text(&text.string());
            label.set_tooltip_text(Some(&text.string()));
        }
    });
    factory
}

/// A drop-down whose chosen name is cut short with "..." when it is
/// narrower than the name, so a long name never widens its panel; the open
/// list shows the names in full.
pub fn dropdown(items: &[&str]) -> gtk::DropDown {
    let d = gtk::DropDown::from_strings(items);
    d.set_hexpand(true);
    d.set_factory(Some(&label_factory(true)));
    d.set_list_factory(Some(&label_factory(false)));
    d
}
