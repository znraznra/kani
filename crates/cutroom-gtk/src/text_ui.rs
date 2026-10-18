//! The inspector's "Text" and "Effects" sections for a text clip: the words,
//! font, size, colours, and the outline, shadow and box. Every change is shown at once; the edit is recorded
//! (one undo step) when things go quiet for a moment, so typing a sentence is
//! one step, not one per letter.

use crate::app::App;
use crate::dialogs;
use crate::icons;
use cutroom_core::text_styles::{restyle, MAX_NAME};
use cutroom_timeline::{ClipId, Prop, TextAlign, TextSpec};
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// How long to wait after the last change before recording it.
const QUIET: Duration = Duration::from_millis(400);

fn rgba_of(c: [u8; 4]) -> gdk::RGBA {
    gdk::RGBA::new(
        f32::from(c[0]) / 255.0,
        f32::from(c[1]) / 255.0,
        f32::from(c[2]) / 255.0,
        f32::from(c[3]) / 255.0,
    )
}

fn colour_of(c: &gdk::RGBA) -> [u8; 4] {
    let b = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    [b(c.red()), b(c.green()), b(c.blue()), b(c.alpha())]
}

/// Installed font families, sorted, without duplicates.
fn font_families() -> Vec<String> {
    let map = pangocairo::FontMap::default();
    let mut names: Vec<String> = map
        .list_families()
        .iter()
        .map(|f| f.name().to_string())
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

/// A section's header: title, Reset and (optionally) the keyframe diamond.
fn header(
    app: &Rc<App>,
    clip: ClipId,
    title: &str,
    effects: bool,
    keys: Option<&[Prop]>,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let label = gtk::Label::new(Some(title));
    label.add_css_class("cc-section-title");
    label.set_xalign(0.0);
    label.set_hexpand(true);
    row.append(&label);
    let reset = crate::icons::button(
        crate::icons::RESET,
        if effects {
            "Reset the outline, shadow and box"
        } else {
            "Reset the font, size, style, alignment, colour and spacing (the words stay)"
        },
    );
    {
        let app = app.clone();
        reset.connect_clicked(move |_| {
            let app = app.clone();
            glib::idle_add_local_once(move || {
                {
                    let mut st = app.state.borrow_mut();
                    if let Some(session) = st.session.as_mut() {
                        session.reset_text_section(clip, effects);
                    }
                }
                app.sync_preview(false);
                app.refresh();
            });
        });
    }
    row.append(&reset);
    if let Some(props) = keys {
        row.append(&crate::transform_ui::key_button(app, clip, props));
    }
    row
}

fn grid() -> (gtk::Grid, impl Fn(&str, &gtk::Widget)) {
    let g = gtk::Grid::new();
    g.set_column_spacing(8);
    g.set_row_spacing(6);
    let row = Cell::new(0i32);
    let add = {
        let g = g.clone();
        move |label: &str, widget: &gtk::Widget| {
            let caption = gtk::Label::new(Some(label));
            caption.set_xalign(0.0);
            caption.add_css_class("cc-field-label");
            let r = row.get();
            row.set(r + 1);
            g.attach(&caption, 0, r, 1, 1);
            widget.set_hexpand(true);
            g.attach(widget, 1, r, 1, 1);
        }
    };
    (g, add)
}

/// Ask for a style's name; `done` keeps it, or says why not (shown in the window).
pub fn ask_name(
    app: &Rc<App>,
    title: &str,
    ok_label: &str,
    initial: &str,
    done: Rc<dyn Fn(&str) -> Result<(), String>>,
) {
    let window = dialogs::dialog_window(app, title, 380);
    let body = dialogs::content_box();
    body.append(&dialogs::caption("Name of the style"));
    let entry = gtk::Entry::new();
    entry.set_text(initial);
    entry.set_max_length(MAX_NAME as i32);
    body.append(&entry);
    let problem = dialogs::error_label();
    body.append(&problem);
    let cancel = gtk::Button::with_label("Cancel");
    let ok = gtk::Button::with_label(ok_label);
    ok.add_css_class("suggested-action");
    body.append(&dialogs::buttons(&ok, &cancel));
    window.set_child(Some(&body));
    let accept: Rc<dyn Fn()> = {
        let (entry, problem, window) = (entry.clone(), problem.clone(), window.clone());
        Rc::new(move || match done(entry.text().as_str()) {
            Ok(()) => window.close(),
            Err(why) => problem.set_text(&why),
        })
    };
    {
        let accept = accept.clone();
        ok.connect_clicked(move |_| accept());
    }
    entry.connect_activate(move |_| accept());
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    window.present();
    entry.grab_focus();
}

/// A name not used yet: "My style", "My style 2", ...
fn free_name(app: &Rc<App>) -> String {
    let st = app.state.borrow();
    let taken = |n: &str| st.text_styles.get(n).is_some();
    if !taken("My style") {
        return "My style".into();
    }
    (2..)
        .map(|i| format!("My style {i}"))
        .find(|n| !taken(n))
        .unwrap_or_default()
}

/// The user's saved styles in the Text category: click one to add text in
/// that look; rename or delete beside it.
pub fn fill_saved_styles(app: &Rc<App>, list: &gtk::Box) {
    let title = gtk::Label::new(Some("My styles"));
    title.set_xalign(0.0);
    title.add_css_class("cc-section-title");
    title.set_margin_top(10);
    list.append(&title);
    let styles = app.state.borrow().text_styles.styles.clone();
    if styles.is_empty() {
        let hint = gtk::Label::new(Some(
            "Select a text clip and press the save icon next to \"Saved style\" in its Text section to keep its look here.",
        ));
        hint.set_xalign(0.0);
        hint.set_wrap(true);
        hint.add_css_class("dim-label");
        list.append(&hint);
        return;
    }
    for style in styles {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        let button = gtk::Button::new();
        button.add_css_class("cc-text-style");
        button.set_hexpand(true);
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let name = gtk::Label::new(Some(&style.name));
        name.set_xalign(0.0);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.add_css_class("panel-title");
        inner.append(&name);
        inner.append(&style_picture(&style.spec));
        button.set_child(Some(&inner));
        button.set_tooltip_text(Some("Add text in this style at the playhead"));
        {
            let (app, spec) = (app.clone(), style.spec.clone());
            button.connect_clicked(move |_| {
                {
                    let mut st = app.state.borrow_mut();
                    if let Some(session) = st.session.as_mut() {
                        session.add_text_clip(spec.clone());
                    }
                }
                app.refresh();
            });
        }
        let rename = icons::button(icons::RENAME, "Rename this style");
        {
            let (app, old) = (app.clone(), style.name.clone());
            rename.connect_clicked(move |_| {
                let (app2, old2) = (app.clone(), old.clone());
                ask_name(
                    &app,
                    "Rename style",
                    "Rename",
                    &old,
                    Rc::new(move |new| {
                        let kept = app2.state.borrow_mut().text_styles.rename(&old2, new)?;
                        {
                            app2.state.borrow_mut().save_text_styles();
                        }
                        app2.set_status(format!("Renamed the style to \"{kept}\""));
                        crate::editor::refresh_library(&app2);
                        Ok(())
                    }),
                );
            });
        }
        let delete = icons::button(icons::DELETE, "Delete this style");
        {
            let (app, name) = (app.clone(), style.name.clone());
            delete.connect_clicked(move |_| {
                {
                    let mut st = app.state.borrow_mut();
                    st.text_styles.delete(&name);
                    st.save_text_styles();
                }
                app.set_status(format!("Deleted the style \"{name}\""));
                app.refresh();
            });
        }
        row.append(&button);
        let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
        side.set_valign(gtk::Align::Center);
        side.append(&rename);
        side.append(&delete);
        row.append(&side);
        list.append(&row);
    }
}

/// A small picture of how a caption style looks, "Abc1234" on a dark tile.
fn style_picture(spec: &TextSpec) -> gtk::Picture {
    const W: u32 = 132;
    const H: u32 = 44;
    let mut s = spec.clone();
    s.content = "Abc1234".into();
    s.size = 320;
    s.align = TextAlign::Center;
    if s.highlight[3] > 0 {
        s.active = Some((0, 3));
    }
    let frame = crate::text_render::render(&s, W, H);
    let mut rgba = vec![0u8; (W * H * 4) as usize];
    for px in rgba.chunks_exact_mut(4) {
        px.copy_from_slice(&[34, 34, 40, 255]);
    }
    let (ox, oy) = (
        (W as i64 - frame.width as i64) / 2,
        (H as i64 - frame.height as i64) / 2,
    );
    for y in 0..frame.height as i64 {
        for x in 0..frame.width as i64 {
            let (tx, ty) = (x + ox, y + oy);
            if tx < 0 || ty < 0 || tx >= W as i64 || ty >= H as i64 {
                continue;
            }
            let src = ((y as usize) * frame.width as usize + x as usize) * 4;
            let dst = ((ty as usize) * W as usize + tx as usize) * 4;
            let a = u32::from(frame.rgba[src + 3]);
            for c in 0..3 {
                let over =
                    u32::from(frame.rgba[src + c]) * a + u32::from(rgba[dst + c]) * (255 - a);
                rgba[dst + c] = (over / 255) as u8;
            }
        }
    }
    let texture = gdk::MemoryTexture::new(
        W as i32,
        H as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(rgba),
        (W * 4) as usize,
    );
    let picture = gtk::Picture::for_paintable(&texture);
    picture.set_size_request(W as i32, H as i32);
    picture.set_can_shrink(false);
    picture.add_css_class("cc-style-thumb");
    picture
}

/// The "Text" and "Effects" sections for `clip`, starting from `initial`
/// (the text as it is at the playhead).
pub fn sections(
    app: &Rc<App>,
    clip: ClipId,
    initial: TextSpec,
    caption: Option<String>,
    has_words: bool,
) -> (gtk::Box, Option<gtk::Box>, gtk::Box) {
    let text_section = gtk::Box::new(gtk::Orientation::Vertical, 6);
    text_section.add_css_class("cc-section");
    let effects_section = gtk::Box::new(gtk::Orientation::Vertical, 6);
    effects_section.add_css_class("cc-section");
    text_section.append(&header(app, clip, "Text", false, Some(&Prop::TEXT)));
    effects_section.append(&header(app, clip, "Effects", true, None));

    let current = Rc::new(RefCell::new(initial.clone()));
    let building = Rc::new(Cell::new(true));
    let generation = Rc::new(Cell::new(0u32));

    let record = {
        let (app, current) = (app.clone(), current.clone());
        Rc::new(move || {
            let spec = current.borrow().clone();
            {
                let mut st = app.state.borrow_mut();
                let all = st.apply_all_captions;
                if let Some(session) = st.session.as_mut() {
                    let targets = session.group_targets(clip, all);
                    session.edit_text_for(clip, spec, &targets);
                }
            }
            // The inspector itself is left alone: rebuilding it would take the
            // keyboard focus away from the box being typed in.
            app.sync_preview(false);
            crate::timeline::queue_draw(&app);
            crate::menu::update(&app);
        })
    };
    let changed: Rc<dyn Fn(Box<dyn FnOnce(&mut TextSpec)>)> = {
        let (app, current, building, generation, record) = (
            app.clone(),
            current.clone(),
            building.clone(),
            generation.clone(),
            record.clone(),
        );
        Rc::new(move |edit| {
            if building.get() {
                return;
            }
            edit(&mut current.borrow_mut());
            let clamped = current.borrow().clone().clamped();
            *current.borrow_mut() = clamped.clone();
            // Show it now (without an undo step yet).
            if let Some(session) = app.state.borrow_mut().session.as_mut() {
                session.preview_clip_text(clip, clamped);
            }
            app.sync_preview(false);
            // The transform box follows the new size of the words.
            app.ui().editor.queue_overlay();
            let id = generation.get().wrapping_add(1);
            generation.set(id);
            let (generation, record) = (generation.clone(), record.clone());
            glib::timeout_add_local_once(QUIET, move || {
                if generation.get() == id {
                    record();
                }
            });
        })
    };

    // The words.
    let view = gtk::TextView::new();
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_top_margin(4);
    view.set_bottom_margin(4);
    view.set_left_margin(6);
    view.set_right_margin(6);
    view.buffer().set_text(&initial.content);
    {
        let changed = changed.clone();
        view.buffer().connect_changed(move |b| {
            let text = b.text(&b.start_iter(), &b.end_iter(), false).to_string();
            changed(Box::new(move |s| s.content = text));
        });
    }
    let frame = gtk::ScrolledWindow::new();
    frame.set_min_content_height(72);
    frame.set_max_content_height(140);
    frame.set_child(Some(&view));
    frame.add_css_class("cc-text-entry");
    text_section.append(&frame);

    let (g1, add1) = grid();

    // Saved styles: pick one to dress this text, or keep this look.
    {
        let names: Vec<String> = app
            .state
            .borrow()
            .text_styles
            .styles
            .iter()
            .map(|s| s.name.clone())
            .collect();
        let mut items = vec!["Choose a saved style...".to_string()];
        items.extend(names);
        let refs: Vec<&str> = items.iter().map(String::as_str).collect();
        let pick = gtk::DropDown::from_strings(&refs);
        pick.set_hexpand(true);
        {
            let (app, current, building, items) = (
                app.clone(),
                current.clone(),
                building.clone(),
                items.clone(),
            );
            pick.connect_selected_notify(move |d| {
                if building.get() || d.selected() == 0 {
                    return;
                }
                let Some(name) = items.get(d.selected() as usize) else {
                    return;
                };
                let Some(saved) = app.state.borrow().text_styles.get(name).cloned() else {
                    return;
                };
                let styled = restyle(&current.borrow(), &saved.spec);
                *current.borrow_mut() = styled.clone();
                {
                    let mut st = app.state.borrow_mut();
                    let all = st.apply_all_captions;
                    if let Some(session) = st.session.as_mut() {
                        let targets = session.group_targets(clip, all);
                        session.edit_text_for(clip, styled, &targets);
                        session.status = Some(format!("Applied the style \"{}\"", saved.name));
                    }
                }
                app.sync_preview(false);
                crate::timeline::queue_draw(&app);
                crate::menu::update(&app);
                // The boxes below show the new look.
                let app = app.clone();
                glib::idle_add_local_once(move || app.refresh());
            });
        }
        let save = icons::button(icons::SAVE, "Keep this look as a saved style");
        {
            let (app, current) = (app.clone(), current.clone());
            save.connect_clicked(move |_| {
                let (app2, current) = (app.clone(), current.clone());
                ask_name(
                    &app,
                    "Save text style",
                    "Save",
                    &free_name(&app),
                    Rc::new(move |name| {
                        let spec = current.borrow().clone();
                        let (kept, replaced) = app2
                            .state
                            .borrow_mut()
                            .text_styles
                            .save_style(name, &spec)?;
                        app2.state.borrow_mut().save_text_styles();
                        app2.set_status(if replaced {
                            format!("Replaced the style \"{kept}\"")
                        } else {
                            format!("Saved the style \"{kept}\"")
                        });
                        let app3 = app2.clone();
                        glib::idle_add_local_once(move || app3.refresh());
                        Ok(())
                    }),
                );
            });
        }
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        row.append(&pick);
        row.append(&save);
        add1("Saved style", row.upcast_ref());
    }

    // Font: the app's own first, then everything installed.
    let mut families = vec!["Inter (default)".to_string()];
    families.extend(
        font_families()
            .into_iter()
            .filter(|f| f != crate::fonts::FAMILY),
    );
    let refs: Vec<&str> = families.iter().map(String::as_str).collect();
    let font = gtk::DropDown::from_strings(&refs);
    font.set_enable_search(true);
    font.set_selected(
        families
            .iter()
            .position(|f| *f == initial.font)
            .unwrap_or(0) as u32,
    );
    {
        let (changed, families) = (changed.clone(), families.clone());
        font.connect_selected_notify(move |d| {
            let name = if d.selected() == 0 {
                String::new()
            } else {
                families
                    .get(d.selected() as usize)
                    .cloned()
                    .unwrap_or_default()
            };
            changed(Box::new(move |s| s.font = name));
        });
    }
    add1("Font", font.upcast_ref());

    // A number box: `value` thousandths shown as `value / 10`.
    let spin = |value: i32, range: (f64, f64), set: fn(&mut TextSpec, i32)| -> gtk::SpinButton {
        let b = gtk::SpinButton::with_range(range.0, range.1, 1.0);
        b.set_digits(0);
        b.set_value(f64::from(value) / 10.0);
        b.add_css_class("cc-spin");
        let changed = changed.clone();
        b.connect_value_changed(move |b| {
            let v = (b.value() * 10.0).round() as i32;
            changed(Box::new(move |s| set(s, v)));
        });
        b
    };
    add1(
        "Size (% of height)",
        spin(initial.size, (1.0, 60.0), |s, v| s.size = v).upcast_ref(),
    );

    // Bold and italic, then the alignment: three icons, one active at a time.
    let style = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    for (label, tip, active, set) in [
        (
            "B",
            "Bold",
            initial.bold,
            (|s: &mut TextSpec, on| s.bold = on) as fn(&mut TextSpec, bool),
        ),
        (
            "I",
            "Italic",
            initial.italic,
            (|s: &mut TextSpec, on| s.italic = on) as fn(&mut TextSpec, bool),
        ),
    ] {
        let b = gtk::ToggleButton::with_label(label);
        b.add_css_class("flat");
        b.add_css_class("cc-icon-button");
        b.set_tooltip_text(Some(tip));
        b.set_active(active);
        let changed = changed.clone();
        b.connect_toggled(move |b| {
            let on = b.is_active();
            changed(Box::new(move |s| set(s, on)));
        });
        style.append(&b);
    }
    let separator = gtk::Separator::new(gtk::Orientation::Vertical);
    separator.set_margin_start(4);
    separator.set_margin_end(4);
    style.append(&separator);
    let mut first: Option<gtk::ToggleButton> = None;
    for (icon, tip, value) in [
        (crate::icons::ALIGN_LEFT, "Align left", TextAlign::Left),
        (
            crate::icons::ALIGN_CENTER,
            "Align centre",
            TextAlign::Center,
        ),
        (crate::icons::ALIGN_RIGHT, "Align right", TextAlign::Right),
    ] {
        let b = crate::icons::toggle(icon, tip);
        match &first {
            Some(f) => b.set_group(Some(f)),
            None => first = Some(b.clone()),
        }
        b.set_active(initial.align == value);
        let changed = changed.clone();
        b.connect_toggled(move |b| {
            if b.is_active() {
                changed(Box::new(move |s| s.align = value));
            }
        });
        style.append(&b);
    }
    add1("Style", style.upcast_ref());

    let colour = |value: [u8; 4], set: fn(&mut TextSpec, [u8; 4])| -> gtk::ColorDialogButton {
        let dialog = gtk::ColorDialog::new();
        dialog.set_with_alpha(true);
        let b = gtk::ColorDialogButton::new(Some(dialog));
        b.set_rgba(&rgba_of(value));
        let changed = changed.clone();
        b.connect_rgba_notify(move |b| {
            let c = colour_of(&b.rgba());
            changed(Box::new(move |s| set(s, c)));
        });
        b
    };
    add1(
        "Colour",
        colour(initial.color, |s, c| s.color = c).upcast_ref(),
    );
    add1(
        "Letter spacing (%)",
        spin(initial.spacing, (-20.0, 100.0), |s, v| s.spacing = v).upcast_ref(),
    );
    text_section.append(&g1);

    // ----- the Auto-Captions section (captions made by Generate captions only) ----------
    let captions_section = has_words.then(|| gtk::Box::new(gtk::Orientation::Vertical, 6));
    if let Some(captions_section) = &captions_section {
        captions_section.add_css_class("cc-section");
        let title = gtk::Label::new(Some("Auto-Captions"));
        title.add_css_class("cc-section-title");
        title.set_xalign(0.0);
        captions_section.append(&title);
        let (cg, cadd) = grid();
        let all = gtk::Switch::new();
        all.set_active(app.state.borrow().apply_all_captions);
        all.set_halign(gtk::Align::Start);
        all.set_valign(gtk::Align::Center);
        all.set_tooltip_text(Some(
            "While on, every change you make here, to the text or to the position, is applied to all auto-captions",
        ));
        {
            let (app, building) = (app.clone(), building.clone());
            all.connect_active_notify(move |sw| {
                if building.get() {
                    return;
                }
                let on = sw.is_active();
                {
                    let mut st = app.state.borrow_mut();
                    st.apply_all_captions = on;
                    // Switching on makes every caption look like this one.
                    if on {
                        if let Some(session) = st.session.as_mut() {
                            let n = session.match_all_captions(clip);
                            if n > 0 {
                                session.status = Some(format!(
                                    "Applied this caption's look to {n} other caption(s)"
                                ));
                            }
                        }
                    }
                }
                app.sync_preview(false);
                crate::timeline::queue_draw(&app);
                let app = app.clone();
                glib::idle_add_local_once(move || app.refresh());
            });
        }
        cadd("Apply to all captions", all.upcast_ref());
        captions_section.append(&cg);
        let current_name = caption.as_deref().unwrap_or("");
        let styles = cutroom_timeline::caption_styles();
        let chosen = styles.iter().find(|c| c.name == current_name);
        // The styles as a grid of pictures; the chosen one is highlighted.
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::Single);
        flow.set_min_children_per_line(2);
        flow.set_max_children_per_line(2);
        flow.set_homogeneous(true);
        flow.set_row_spacing(6);
        flow.set_column_spacing(6);
        flow.set_activate_on_single_click(true);
        flow.add_css_class("cc-style-grid");
        for style in &styles {
            let tile = gtk::Box::new(gtk::Orientation::Vertical, 2);
            tile.set_margin_top(4);
            tile.set_margin_bottom(4);
            tile.set_margin_start(4);
            tile.set_margin_end(4);
            tile.append(&style_picture(&style.spec));
            let name = gtk::Label::new(Some(style.name));
            name.add_css_class("caption");
            tile.append(&name);
            tile.set_tooltip_text(Some(style.blurb));
            flow.append(&tile);
        }
        if let Some(i) = styles.iter().position(|c| c.name == current_name) {
            if let Some(child) = flow.child_at_index(i as i32) {
                flow.select_child(&child);
            }
        }
        {
            let names: Vec<String> = styles.iter().map(|c| c.name.to_string()).collect();
            // Picking a tile applies its style. Clicking the tile that is
            // already chosen counts too, which is how the style reaches the
            // other captions once "Apply to all captions" is switched on.
            let pick: Rc<dyn Fn(i32)> = {
                let app = app.clone();
                Rc::new(move |index: i32| {
                    let Some(name) = names.get(index as usize).cloned() else {
                        return;
                    };
                    let app = app.clone();
                    glib::idle_add_local_once(move || {
                        {
                            let mut st = app.state.borrow_mut();
                            st.settings.caption_style = name.clone();
                            let all = st.apply_all_captions;
                            if let Some(session) = st.session.as_mut() {
                                let targets = session.group_targets(clip, all);
                                session.force_caption_style_for(clip, &name, &targets);
                            }
                        }
                        app.sync_preview(false);
                        crate::timeline::queue_draw(&app);
                        app.refresh();
                    });
                })
            };
            {
                let (pick, building) = (pick.clone(), building.clone());
                flow.connect_selected_children_changed(move |f| {
                    if building.get() {
                        return;
                    }
                    if let Some(child) = f.selected_children().into_iter().next() {
                        pick(child.index());
                    }
                });
            }
            flow.connect_child_activated(move |_, child| pick(child.index()));
        }
        captions_section.append(&flow);
        let (sg, sadd) = grid();
        // What this style lets you change.
        let lights = chosen.map_or(initial.highlight[3] > 0, |c| c.spec.highlight[3] > 0);
        if has_words && lights {
            sadd(
                "Spoken word colour",
                colour(initial.highlight, |s, c| s.highlight = c).upcast_ref(),
            );
            let mode =
                gtk::DropDown::from_strings(&["Only the spoken word", "Fills as it is said"]);
            mode.set_selected(match initial.highlight_mode {
                cutroom_timeline::HighlightMode::Word => 0,
                cutroom_timeline::HighlightMode::Karaoke => 1,
            });
            let changed = changed.clone();
            mode.connect_selected_notify(move |d| {
                let m = if d.selected() == 1 {
                    cutroom_timeline::HighlightMode::Karaoke
                } else {
                    cutroom_timeline::HighlightMode::Word
                };
                changed(Box::new(move |s| s.highlight_mode = m));
            });
            sadd("Highlight", mode.upcast_ref());
        }
        captions_section.append(&sg);
        let note = if !has_words {
            Some("Spoken-word highlighting only works on captions made with Generate captions.")
        } else if lights {
            Some("Make the spoken word colour fully transparent to turn highlighting off.")
        } else {
            Some("This style does not light up the spoken word.")
        };
        if let Some(note) = note {
            let hint = gtk::Label::new(Some(note));
            hint.set_wrap(true);
            hint.set_xalign(0.0);
            hint.add_css_class("dim-label");
            hint.add_css_class("caption");
            captions_section.append(&hint);
        }
    }

    // Each effect is a group: a check box to switch it on or off (its settings
    // are kept), and an arrow to fold its settings away.
    type Setter = Rc<dyn Fn(&mut TextSpec, bool)>;
    // (field index, value to start from when it is empty; the colour box ignores the number)
    type Start = (usize, f64);
    let group = |title: &str,
                 on: bool,
                 set_on: Setter,
                 starts: Vec<Start>,
                 fields: Vec<(&str, gtk::Widget)>|
     -> gtk::Box {
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 4);
        outer.add_css_class("cc-effect-group");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        head.add_css_class("cc-effect-head");
        let check = gtk::CheckButton::with_label(title);
        check.set_active(on);
        check.set_hexpand(true);
        let fold = gtk::ToggleButton::new();
        fold.set_icon_name("pan-down-symbolic");
        fold.add_css_class("flat");
        fold.add_css_class("cc-fold");
        fold.set_valign(gtk::Align::Center);
        fold.set_tooltip_text(Some("Show or hide this effect's settings"));
        head.append(&check);
        head.append(&fold);
        outer.append(&head);
        let (g, add) = grid();
        for (label, widget) in &fields {
            add(label, widget);
        }
        let revealer = gtk::Revealer::new();
        revealer.set_child(Some(&g));
        revealer.set_reveal_child(on);
        fold.set_active(on);
        g.set_sensitive(on);
        outer.append(&revealer);
        {
            let revealer = revealer.clone();
            fold.connect_toggled(move |f| revealer.set_reveal_child(f.is_active()));
        }
        {
            let (changed, g, fold, fields) = (
                changed.clone(),
                g.clone(),
                fold.clone(),
                fields.iter().map(|f| f.1.clone()).collect::<Vec<_>>(),
            );
            check.connect_toggled(move |c| {
                let on = c.is_active();
                g.set_sensitive(on);
                fold.set_active(on);
                {
                    let set_on = set_on.clone();
                    changed(Box::new(move |s| set_on(s, on)));
                }
                if on {
                    // First time on: give empty settings something visible
                    // to start from. The boxes report their own changes.
                    for (index, value) in &starts {
                        match &fields[*index] {
                            w if w.is::<gtk::SpinButton>() => {
                                let spin = w.downcast_ref::<gtk::SpinButton>().unwrap();
                                if spin.value() == 0.0 {
                                    spin.set_value(*value);
                                }
                            }
                            w if w.is::<gtk::ColorDialogButton>() => {
                                let b = w.downcast_ref::<gtk::ColorDialogButton>().unwrap();
                                if b.rgba().alpha() == 0.0 {
                                    b.set_rgba(&rgba_of([0, 0, 0, 180]));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            });
        }
        outer
    };
    let tint = |w: &gtk::ColorDialogButton| -> gtk::Widget { w.clone().upcast() };
    let outline_fields = vec![
        (
            "Size (% of text)",
            spin(initial.outline, (0.0, 40.0), |s, v| s.outline = v).upcast::<gtk::Widget>(),
        ),
        (
            "Colour",
            tint(&colour(initial.outline_color, |s, c| s.outline_color = c)),
        ),
    ];
    let shadow_fields = vec![
        (
            "Distance",
            spin(initial.shadow, (0.0, 60.0), |s, v| s.shadow = v).upcast::<gtk::Widget>(),
        ),
        (
            "Softness",
            spin(initial.shadow_blur, (0.0, 60.0), |s, v| s.shadow_blur = v)
                .upcast::<gtk::Widget>(),
        ),
        (
            "Colour",
            tint(&colour(initial.shadow_color, |s, c| s.shadow_color = c)),
        ),
    ];
    let box_fields = vec![
        (
            "Colour",
            tint(&colour(initial.background, |s, c| s.background = c)),
        ),
        (
            "Padding",
            spin(initial.padding, (0.0, 100.0), |s, v| s.padding = v).upcast::<gtk::Widget>(),
        ),
        (
            "Corners (% round)",
            spin(initial.corner, (0.0, 50.0), |s, v| s.corner = v).upcast::<gtk::Widget>(),
        ),
    ];
    effects_section.append(&group(
        "Outline",
        initial.outline_on && initial.outline > 0,
        Rc::new(|s, on| s.outline_on = on),
        vec![(0, 6.0)],
        outline_fields,
    ));
    effects_section.append(&group(
        "Shadow",
        initial.shadow_on && initial.shadow > 0,
        Rc::new(|s, on| s.shadow_on = on),
        vec![(0, 4.0), (1, 4.0)],
        shadow_fields,
    ));
    effects_section.append(&group(
        "Background box",
        initial.box_on && initial.background[3] > 0,
        Rc::new(|s, on| s.box_on = on),
        vec![(0, 0.0), (1, 30.0), (2, 15.0)],
        box_fields,
    ));

    building.set(false);
    (text_section, captions_section, effects_section)
}
