//! The home screen: every project in one place, with search, sorting,
//! thumbnail/list views and a right-click menu.

use crate::app::App;
use crate::dialogs;
use crate::icons;
use cutroom_core::home_logic::{
    ago_text, cover_tint, filter_and_sort, format_duration, meta_line, SortKey,
};
use cutroom_core::new_project::aspect_label;
use cutroom_core::settings::ViewMode;
use cutroom_core::shortcuts::Action;
use cutroom_project::{ProjectEntry, ProjectSummary};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{cairo, gdk, glib};
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::SystemTime;

/// The pages the sidebar switches between.
const PAGE_PROJECTS: &str = "projects";
const PAGE_RECENTS: &str = "recents";
const PAGE_BRAND_KITS: &str = "brand-kits";

pub struct HomeUi {
    /// The sidebar and the page beside it.
    pub root: gtk::Box,
    #[cfg_attr(not(test), allow(dead_code))]
    pages: gtk::Stack,
    #[cfg_attr(not(test), allow(dead_code))]
    nav: Vec<(&'static str, gtk::ToggleButton)>,
    recents: gtk::FlowBox,
    recents_stack: gtk::Stack,
    search: gtk::SearchEntry,
    sort: gtk::DropDown,
    thumbs_button: gtk::ToggleButton,
    list_button: gtk::ToggleButton,
    dir_label: gtk::Label,
    content: gtk::Stack,
    grid: gtk::FlowBox,
    list: gtk::Box,
    no_match: gtk::Label,
    /// True while the widgets are being set from state, so their change
    /// signals don't feed back.
    syncing: Cell<bool>,
}

/// A flat icon button; `suggested` makes it the accent-coloured main action.
fn tool_button(icon: &'static str, tip: &str, suggested: bool) -> gtk::Button {
    let b = icons::button(icon, tip);
    if suggested {
        b.remove_css_class("flat");
        b.add_css_class("suggested-action");
    }
    b
}

/// The left strip: Projects, Recents, Brand Kits, and the app's name and
/// version at the bottom.
fn sidebar(
    app: &Rc<App>,
    pages: &gtk::Stack,
) -> (gtk::Box, Vec<(&'static str, gtk::ToggleButton)>) {
    let bar = gtk::Box::new(gtk::Orientation::Vertical, 4);
    bar.add_css_class("cc-sidebar");
    bar.set_size_request(190, -1);
    let mut nav = Vec::new();
    let mut first: Option<gtk::ToggleButton> = None;
    for (page, icon, text) in [
        (PAGE_PROJECTS, icons::PROJECTS, "Projects"),
        (PAGE_RECENTS, icons::RECENTS, "Recents"),
        (PAGE_BRAND_KITS, icons::BRAND_KITS, "Brand Kits"),
    ] {
        let button = gtk::ToggleButton::new();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.append(&icons::icon(icon, crate::ui::ICON));
        let label = gtk::Label::new(Some(text));
        label.set_xalign(0.0);
        row.append(&label);
        button.set_child(Some(&row));
        button.add_css_class("flat");
        button.add_css_class("cc-nav");
        match &first {
            Some(f) => button.set_group(Some(f)),
            None => first = Some(button.clone()),
        }
        {
            let (app, pages) = (app.clone(), pages.clone());
            button.connect_toggled(move |b| {
                if !b.is_active() {
                    return;
                }
                pages.set_visible_child_name(page);
                if page == PAGE_RECENTS {
                    refresh_recents(&app);
                }
            });
        }
        bar.append(&button);
        nav.push((page, button));
    }
    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    bar.append(&spacer);
    let name = gtk::Label::new(Some("Kani"));
    name.set_xalign(0.0);
    name.add_css_class("title-4");
    let version = gtk::Label::new(Some(&format!("Version {}", env!("CARGO_PKG_VERSION"))));
    version.set_xalign(0.0);
    version.add_css_class("dim-label");
    version.add_css_class("caption");
    let about = gtk::Box::new(gtk::Orientation::Vertical, 0);
    about.set_margin_start(10);
    about.set_margin_bottom(6);
    about.append(&name);
    about.append(&version);
    bar.append(&about);
    (bar, nav)
}

/// A page that is not built yet, saying when it will be.
fn coming_soon(icon: &'static str, title: &str, text: &str, phase: &str) -> gtk::Widget {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    page.set_valign(gtk::Align::Center);
    page.set_halign(gtk::Align::Center);
    let glyph = icons::icon(icon, 48);
    glyph.set_opacity(0.5);
    page.append(&glyph);
    let heading = gtk::Label::new(Some(title));
    heading.add_css_class("title-2");
    page.append(&heading);
    let body = gtk::Label::new(Some(text));
    body.set_wrap(true);
    body.set_justify(gtk::Justification::Center);
    body.set_max_width_chars(56);
    body.add_css_class("dim-label");
    page.append(&body);
    let when = gtk::Label::new(Some(phase));
    when.add_css_class("cc-pill");
    when.set_margin_top(6);
    page.append(&when);
    page.upcast()
}

pub fn build(app: &Rc<App>) -> HomeUi {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let pages = gtk::Stack::new();
    pages.set_hexpand(true);
    pages.set_vexpand(true);
    let (side, nav) = sidebar(app, &pages);
    root.append(&side);
    root.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    root.append(&pages);

    // The Projects page.
    let page = gtk::Box::new(gtk::Orientation::Vertical, crate::ui::GAP);
    crate::ui::pad(&page);
    pages.add_named(&page, Some(PAGE_PROJECTS));

    // Search, sort and view on the left; Open and New on the right.
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, crate::ui::GAP);
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search projects"));
    search.set_width_chars(30);
    let sort = gtk::DropDown::from_strings(&["Sort: Last modified", "Sort: Name"]);
    let thumbs_button = icons::toggle(icons::GRID, "Thumbnails");
    let list_button = icons::toggle(icons::LIST, "List");
    list_button.set_group(Some(&thumbs_button));
    let view_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    view_box.add_css_class("linked");
    view_box.append(&thumbs_button);
    view_box.append(&list_button);
    let refresh = icons::button(icons::REFRESH, "Look for projects again");
    bar.append(&search);
    bar.append(&sort);
    bar.append(&view_box);
    bar.append(&refresh);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    let open_folder = tool_button(icons::FOLDER, "Open a project from a folder...", false);
    {
        let app = app.clone();
        open_folder.connect_clicked(move |_| app.run_action(Action::OpenProject));
    }
    let new_project = tool_button(icons::IMPORT, "New project", true);
    {
        let app = app.clone();
        new_project.connect_clicked(move |_| app.run_action(Action::NewProject));
    }
    bar.append(&open_folder);
    bar.append(&new_project);
    page.append(&bar);
    let root_page = page;

    // Where projects are stored.
    let dir_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let stored = gtk::Label::new(Some("Projects are stored in"));
    stored.add_css_class("dim-label");
    let dir_label = gtk::Label::new(None);
    dir_label.set_selectable(true);
    dir_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    dir_label.add_css_class("caption");
    let change = icons::button(icons::SETTINGS, "Change in Settings...");
    dir_row.append(&stored);
    dir_row.append(&dir_label);
    dir_row.append(&change);
    root_page.append(&dir_row);
    root_page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // Content.
    let content = gtk::Stack::new();
    content.set_vexpand(true);

    let grid = gtk::FlowBox::new();
    grid.set_selection_mode(gtk::SelectionMode::None);
    grid.set_homogeneous(true);
    grid.set_min_children_per_line(1);
    grid.set_max_children_per_line(40);
    grid.set_row_spacing(14);
    grid.set_column_spacing(14);
    grid.set_valign(gtk::Align::Start);
    let grid_scroll = gtk::ScrolledWindow::new();
    grid_scroll.set_child(Some(&grid));
    content.add_named(&grid_scroll, Some("grid"));

    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.set_valign(gtk::Align::Start);
    let list_scroll = gtk::ScrolledWindow::new();
    list_scroll.set_child(Some(&list));
    content.add_named(&list_scroll, Some("list"));

    let empty = gtk::Box::new(gtk::Orientation::Vertical, 8);
    empty.set_valign(gtk::Align::Center);
    let empty_title = gtk::Label::new(Some("No projects yet"));
    empty_title.add_css_class("title-2");
    let empty_hint = gtk::Label::new(Some("Create your first project to get started."));
    empty_hint.add_css_class("dim-label");
    let empty_button = tool_button(icons::IMPORT, "New project", true);
    empty_button.set_halign(gtk::Align::Center);
    {
        let app = app.clone();
        empty_button.connect_clicked(move |_| app.run_action(Action::NewProject));
    }
    empty.append(&empty_title);
    empty.append(&empty_hint);
    empty.append(&empty_button);
    content.add_named(&empty, Some("empty"));

    let no_match = gtk::Label::new(None);
    no_match.add_css_class("title-4");
    content.add_named(&no_match, Some("nomatch"));
    root_page.append(&content);

    // The Recents page: the projects worked on last.
    let recents_page = gtk::Box::new(gtk::Orientation::Vertical, crate::ui::GAP);
    crate::ui::pad(&recents_page);
    let recents_title = gtk::Label::new(Some("Recents"));
    recents_title.set_xalign(0.0);
    recents_title.add_css_class("title-3");
    let recents_hint = gtk::Label::new(Some("The projects you changed most recently."));
    recents_hint.set_xalign(0.0);
    recents_hint.add_css_class("dim-label");
    recents_page.append(&recents_title);
    recents_page.append(&recents_hint);
    let recents = gtk::FlowBox::new();
    recents.set_selection_mode(gtk::SelectionMode::None);
    recents.set_homogeneous(true);
    recents.set_min_children_per_line(1);
    recents.set_max_children_per_line(40);
    recents.set_row_spacing(14);
    recents.set_column_spacing(14);
    recents.set_valign(gtk::Align::Start);
    let recents_scroll = gtk::ScrolledWindow::new();
    recents_scroll.set_vexpand(true);
    recents_scroll.set_child(Some(&recents));
    let recents_stack = gtk::Stack::new();
    recents_stack.add_named(&recents_scroll, Some("list"));
    let recents_empty = gtk::Label::new(Some("No projects yet. Make one in Projects."));
    recents_empty.add_css_class("dim-label");
    recents_stack.add_named(&recents_empty, Some("empty"));
    recents_stack.set_vexpand(true);
    recents_page.append(&recents_stack);
    pages.add_named(&recents_page, Some(PAGE_RECENTS));

    // Brand Kits: not built yet.
    pages.add_named(
        &coming_soon(
            icons::BRAND_KITS,
            "Brand Kits",
            "Keep your logo, fonts and colours in one kit and use them in any project. Saved text styles come first; the full kits arrive with the asset libraries.",
            "Planned for Phase 8 (asset libraries)",
        ),
        Some(PAGE_BRAND_KITS),
    );
    pages.set_visible_child_name(PAGE_PROJECTS);
    // Only now that every page exists may the first item be pressed.
    if let Some((_, b)) = nav.first() {
        b.set_active(true);
    }

    let ui = HomeUi {
        root,
        pages,
        nav,
        recents,
        recents_stack,
        search,
        sort,
        thumbs_button,
        list_button,
        dir_label,
        content,
        grid,
        list,
        no_match,
        syncing: Cell::new(false),
    };

    // Signals. They look the widgets up through the app so they stay valid
    // across rebuilds.
    {
        let app = app.clone();
        ui.search.connect_search_changed(move |entry| {
            if app.ui().home.syncing.get() {
                return;
            }
            app.state.borrow_mut().home.query = entry.text().to_string();
            refresh_content(&app);
        });
    }
    {
        let app = app.clone();
        ui.sort.connect_selected_notify(move |dropdown| {
            if app.ui().home.syncing.get() {
                return;
            }
            app.state.borrow_mut().home.sort = if dropdown.selected() == 1 {
                SortKey::Name
            } else {
                SortKey::Modified
            };
            refresh_content(&app);
        });
    }
    for (button, mode) in [
        (&ui.thumbs_button, ViewMode::Thumbnails),
        (&ui.list_button, ViewMode::List),
    ] {
        let app = app.clone();
        button.connect_toggled(move |b| {
            if app.ui().home.syncing.get() || !b.is_active() {
                return;
            }
            {
                let mut st = app.state.borrow_mut();
                st.settings.home_view = mode;
                st.save_settings();
            }
            refresh_content(&app);
        });
    }
    {
        let app = app.clone();
        refresh.connect_clicked(move |_| {
            app.refresh_home_entries();
            refresh_content(&app);
        });
    }
    {
        let app = app.clone();
        change.connect_clicked(move |_| crate::settings_ui::open(&app, Some("draft")));
    }
    ui
}

impl HomeUi {
    /// The page showing: "projects", "recents" or "brand-kits".
    #[cfg(test)]
    pub fn visible_page(&self) -> String {
        self.pages
            .visible_child_name()
            .map(|n| n.to_string())
            .unwrap_or_default()
    }

    /// Press a sidebar item.
    #[cfg(test)]
    pub fn show_page(&self, page: &str) {
        if let Some((_, button)) = self.nav.iter().find(|(p, _)| *p == page) {
            button.set_active(true);
        }
    }
}

/// Sync the toolbar widgets with the state, then rebuild the project list.
pub fn refresh(app: &Rc<App>) {
    let ui = &app.ui().home;
    {
        let st = app.state.borrow();
        ui.syncing.set(true);
        if ui.search.text() != st.home.query {
            ui.search.set_text(&st.home.query);
        }
        ui.sort
            .set_selected(if st.home.sort == SortKey::Name { 1 } else { 0 });
        match st.settings.home_view {
            ViewMode::Thumbnails => ui.thumbs_button.set_active(true),
            ViewMode::List => ui.list_button.set_active(true),
        }
        ui.dir_label
            .set_text(&st.projects_dir().display().to_string());
        ui.syncing.set(false);
    }
    refresh_content(app);
    refresh_recents(app);
}

/// The most recently changed projects, newest first.
fn refresh_recents(app: &Rc<App>) {
    let ui = &app.ui().home;
    let shown: Vec<ProjectEntry> = {
        let st = app.state.borrow();
        filter_and_sort(&st.home.entries, "", SortKey::Modified)
            .into_iter()
            .take(12)
            .cloned()
            .collect()
    };
    while let Some(child) = ui.recents.first_child() {
        ui.recents.remove(&child);
    }
    if shown.is_empty() {
        ui.recents_stack.set_visible_child_name("empty");
        return;
    }
    let (selected, now) = (app.state.borrow().home.selected.clone(), SystemTime::now());
    for entry in &shown {
        let is_selected = selected.as_deref() == Some(entry.path());
        ui.recents
            .append(&project_card(app, entry, is_selected, now));
    }
    ui.recents_stack.set_visible_child_name("list");
}

fn clear(container: &impl IsA<gtk::Widget>) {
    let mut child = container.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if let Some(parent) = widget.parent().and_then(|p| p.downcast::<gtk::Box>().ok()) {
            parent.remove(&widget);
        }
    }
}

/// Rebuild the cards or rows.
pub fn refresh_content(app: &Rc<App>) {
    let ui = &app.ui().home;
    let (entries, mode, selected, total, query) = {
        let st = app.state.borrow();
        let shown: Vec<ProjectEntry> =
            filter_and_sort(&st.home.entries, &st.home.query, st.home.sort)
                .into_iter()
                .cloned()
                .collect();
        (
            shown,
            st.settings.home_view,
            st.home.selected.clone(),
            st.home.entries.len(),
            st.home.query.trim().to_owned(),
        )
    };

    // Clear.
    while let Some(child) = ui.grid.first_child() {
        ui.grid.remove(&child);
    }
    clear(&ui.list);

    if entries.is_empty() {
        if total == 0 {
            ui.content.set_visible_child_name("empty");
        } else {
            ui.no_match
                .set_text(&format!("No projects match \"{query}\""));
            ui.content.set_visible_child_name("nomatch");
        }
        return;
    }

    let now = SystemTime::now();
    match mode {
        ViewMode::Thumbnails => {
            for entry in &entries {
                let selected = selected.as_deref() == Some(entry.path());
                ui.grid.append(&project_card(app, entry, selected, now));
            }
            ui.content.set_visible_child_name("grid");
        }
        ViewMode::List => {
            ui.list.append(&list_header());
            for entry in &entries {
                let selected = selected.as_deref() == Some(entry.path());
                ui.list.append(&project_row(app, entry, selected, now));
            }
            ui.content.set_visible_child_name("list");
        }
    }
}

fn draw_cover(
    cr: &cairo::Context,
    w: f64,
    h: f64,
    summary: Option<&ProjectSummary>,
    picture: Option<&gtk::gdk_pixbuf::Pixbuf>,
) {
    cr.set_source_rgb(0.063, 0.067, 0.082);
    rounded(cr, 0.0, 0.0, w, h, 6.0);
    let _ = cr.fill();
    // The preview as it last was, fitted inside the card.
    if let Some(p) = picture {
        let (pw, ph) = (f64::from(p.width().max(1)), f64::from(p.height().max(1)));
        let k = ((w - 8.0) / pw).min((h - 8.0) / ph);
        let (dw, dh) = (pw * k, ph * k);
        let (x, y) = ((w - dw) / 2.0, (h - dh) / 2.0);
        cr.save().ok();
        rounded(cr, x, y, dw, dh, 3.0);
        cr.clip();
        cr.translate(x, y);
        cr.scale(k, k);
        {
            use gtk::gdk::prelude::GdkCairoContextExt;
            cr.set_source_pixbuf(p, 0.0, 0.0);
        }
        cr.source().set_filter(cairo::Filter::Good);
        let _ = cr.paint();
        cr.restore().ok();
        return;
    }
    match summary {
        Some(s) => {
            let aspect = f64::from(s.width) / f64::from(s.height.max(1));
            let (room_w, room_h) = (w - 20.0, h - 20.0);
            let (cw, ch) = if room_w / room_h > aspect {
                (room_h * aspect, room_h)
            } else {
                (room_w, room_w / aspect)
            };
            let (x, y) = ((w - cw) / 2.0, (h - ch) / 2.0);
            let (r, g, b) = cover_tint(&s.name);
            cr.set_source_rgb(
                f64::from(r) / 255.0,
                f64::from(g) / 255.0,
                f64::from(b) / 255.0,
            );
            rounded(cr, x, y, cw, ch, 3.0);
            let _ = cr.fill();
            let initial: String = s
                .name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .collect();
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.85);
            cr.select_font_face(
                crate::fonts::FAMILY,
                cairo::FontSlant::Normal,
                cairo::FontWeight::Bold,
            );
            let size = (ch * 0.4).clamp(14.0, 40.0);
            cr.set_font_size(size);
            if let Ok(extents) = cr.text_extents(&initial) {
                cr.move_to(
                    x + (cw - extents.width()) / 2.0 - extents.x_bearing(),
                    y + (ch + extents.height()) / 2.0 - (extents.height() + extents.y_bearing()),
                );
                let _ = cr.show_text(&initial);
            }
        }
        None => {
            cr.set_source_rgb(1.0, 0.38, 0.32);
            cr.select_font_face(
                crate::fonts::FAMILY,
                cairo::FontSlant::Normal,
                cairo::FontWeight::Bold,
            );
            cr.set_font_size(36.0);
            cr.move_to(w / 2.0 - 8.0, h / 2.0 + 12.0);
            let _ = cr.show_text("!");
        }
    }
}

fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(
        x + r,
        y + h - r,
        r,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(
        x + r,
        y + r,
        r,
        std::f64::consts::PI,
        3.0 * std::f64::consts::FRAC_PI_2,
    );
    cr.close_path();
}

fn label(text: &str, css: &[&str], xalign: f32) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(xalign);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    for c in css {
        l.add_css_class(c);
    }
    l
}

fn project_card(
    app: &Rc<App>,
    entry: &ProjectEntry,
    selected: bool,
    now: SystemTime,
) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
    card.add_css_class("cutroom-card");
    card.set_widget_name(&entry.path().display().to_string());
    if selected {
        card.add_css_class("selected");
    }
    card.set_size_request(214, -1);

    let cover = gtk::DrawingArea::new();
    cover.set_content_width(198);
    cover.set_content_height(122);
    let summary = match entry {
        ProjectEntry::Ok(s) => Some(s.clone()),
        ProjectEntry::Broken { .. } => None,
    };
    let picture = summary.as_ref().and_then(|_| {
        let path = cutroom_project::cover_path(entry.path());
        gtk::gdk_pixbuf::Pixbuf::from_file(path).ok()
    });
    {
        let summary = summary.clone();
        cover.set_draw_func(move |_, cr, w, h| {
            draw_cover(
                cr,
                f64::from(w),
                f64::from(h),
                summary.as_ref(),
                picture.as_ref(),
            )
        });
    }
    card.append(&cover);
    card.append(&label(&entry.display_name(), &["heading"], 0.0));
    match entry {
        ProjectEntry::Ok(s) => {
            card.append(&label(&meta_line(s), &["dim-label", "caption"], 0.0));
            card.append(&label(
                &format!("{} clip(s)  |  {}", s.clip_count, ago_text(s.modified, now)),
                &["dim-label", "caption"],
                0.0,
            ));
        }
        ProjectEntry::Broken { .. } => {
            card.append(&label(
                "Can't be opened (right-click for details)",
                &["error", "caption"],
                0.0,
            ));
        }
    }
    attach_gestures(app, &card, entry);
    card.upcast()
}

fn list_header() -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_margin_start(10);
    row.set_margin_end(10);
    let name = label("Name", &["dim-label", "caption"], 0.0);
    name.set_hexpand(true);
    row.append(&name);
    for (text, width) in [
        ("Canvas", 190),
        ("Frame rate", 100),
        ("Length", 80),
        ("Modified", 130),
    ] {
        let l = label(text, &["dim-label", "caption"], 0.0);
        l.set_size_request(width, -1);
        row.append(&l);
    }
    row.upcast()
}

fn project_row(
    app: &Rc<App>,
    entry: &ProjectEntry,
    selected: bool,
    now: SystemTime,
) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("cutroom-row");
    row.set_widget_name(&entry.path().display().to_string());
    if selected {
        row.add_css_class("selected");
    }
    let name = label(&entry.display_name(), &[], 0.0);
    name.set_hexpand(true);
    row.append(&name);
    let cells: Vec<(String, i32)> = match entry {
        ProjectEntry::Ok(s) => vec![
            (
                format!(
                    "{} x {}  ({})",
                    s.width,
                    s.height,
                    aspect_label(s.width, s.height)
                ),
                190,
            ),
            (format!("{:.3} fps", s.frame_rate.as_f64()), 100),
            (format_duration(s.duration), 80),
            (ago_text(s.modified, now), 130),
        ],
        ProjectEntry::Broken { .. } => vec![("Can't be opened".to_owned(), 190)],
    };
    for (text, width) in cells {
        let l = label(&text, &["dim-label"], 0.0);
        l.set_size_request(width, -1);
        row.append(&l);
    }
    attach_gestures(app, &row, entry);
    row.upcast()
}

/// Mark the selected project's card or row, and unmark the others.
fn update_selection(app: &Rc<App>) {
    let ui = &app.ui().home;
    let selected = app
        .state
        .borrow()
        .home
        .selected
        .as_ref()
        .map(|p| p.display().to_string());
    let apply = |widget: &gtk::Widget| {
        if selected.as_deref() == Some(widget.widget_name().as_str()) {
            widget.add_css_class("selected");
        } else {
            widget.remove_css_class("selected");
        }
    };
    for flow in [&ui.grid, &ui.recents] {
        let mut child = flow.first_child();
        while let Some(flow_child) = child {
            if let Some(card) = flow_child
                .downcast_ref::<gtk::FlowBoxChild>()
                .and_then(|c| c.child())
            {
                apply(&card);
            }
            child = flow_child.next_sibling();
        }
    }
    let mut child = ui.list.first_child();
    while let Some(row) = child {
        apply(&row);
        child = row.next_sibling();
    }
}

/// Click, double-click and right-click behaviour shared by cards and rows.
fn attach_gestures(app: &Rc<App>, widget: &impl IsA<gtk::Widget>, entry: &ProjectEntry) {
    let path: PathBuf = entry.path().to_path_buf();
    let readable = matches!(entry, ProjectEntry::Ok(_));

    let primary = gtk::GestureClick::builder().button(1).build();
    {
        let app = app.clone();
        let path = path.clone();
        primary.connect_released(move |_, presses, _, _| {
            if presses >= 2 && readable {
                let (app, path) = (app.clone(), path.clone());
                glib::idle_add_local_once(move || app.open_project(&path));
            } else {
                // Only restyle: rebuilding here would destroy the widget under
                // the pointer and break double-clicking.
                app.state.borrow_mut().home.selected = Some(path.clone());
                update_selection(&app);
            }
        });
    }
    widget.add_controller(primary);

    let secondary = gtk::GestureClick::builder().button(3).build();
    {
        let app = app.clone();
        let entry = entry.clone();
        let anchor = widget.clone().upcast::<gtk::Widget>();
        secondary.connect_pressed(move |_, _, x, y| {
            app.state.borrow_mut().home.selected = Some(entry.path().to_path_buf());
            update_selection(&app);
            context_menu(&app, &anchor, &entry, x, y);
        });
    }
    widget.add_controller(secondary);
}

fn context_menu(app: &Rc<App>, anchor: &gtk::Widget, entry: &ProjectEntry, x: f64, y: f64) {
    let popover = gtk::Popover::new();
    popover.set_parent(anchor);
    popover.set_has_arrow(false);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let path = entry.path().to_path_buf();
    let readable = matches!(entry, ProjectEntry::Ok(_));
    let outside = {
        let st = app.state.borrow();
        path.parent() != Some(st.projects_dir().as_path())
    };

    let add = |text: &str, enabled: bool, danger: bool, act: Box<dyn Fn()>| {
        let button = gtk::Button::with_label(text);
        button.add_css_class("flat");
        button.set_sensitive(enabled);
        if danger {
            button.add_css_class("destructive-action");
        }
        if let Some(l) = button.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
            l.set_xalign(0.0);
        }
        let popover = popover.clone();
        button.connect_clicked(move |_| {
            popover.popdown();
            act();
        });
        menu.append(&button);
    };
    {
        let (app, path) = (app.clone(), path.clone());
        add(
            "Open",
            readable,
            false,
            Box::new(move || app.open_project(&path)),
        );
    }
    {
        let (app, entry) = (app.clone(), entry.clone());
        add(
            "Rename...",
            readable,
            false,
            Box::new(move || dialogs::rename_project(&app, &entry)),
        );
    }
    {
        let (app, entry) = (app.clone(), entry.clone());
        add(
            "Show info...",
            true,
            false,
            Box::new(move || dialogs::project_info(&app, &entry)),
        );
    }
    if outside {
        let (app, path) = (app.clone(), path.clone());
        add(
            "Remove from list",
            true,
            false,
            Box::new(move || {
                {
                    let mut st = app.state.borrow_mut();
                    st.settings.forget_project(&path);
                    st.save_settings();
                }
                app.refresh_home_entries();
                refresh_content(&app);
            }),
        );
    }
    {
        let (app, entry) = (app.clone(), entry.clone());
        add(
            "Delete...",
            true,
            true,
            Box::new(move || dialogs::delete_project(&app, &entry)),
        );
    }
    popover.set_child(Some(&menu));
    popover.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    popover.popup();
}
