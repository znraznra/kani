//! The editing page: library, preview, inspector and the timeline panel.

use crate::app::App;
use crate::icons;
use crate::theme::{self, Palette};
use crate::{thumbs, timeline};
use cutroom_core::settings::ViewMode;
use cutroom_core::shortcuts::Action;
use cutroom_core::state::{media_label, MediaKind, Session, StorageKind};
use cutroom_core::timeline_logic::Tool;
use cutroom_core::view::{self, MAX_PIXELS_PER_SECOND, MIN_PIXELS_PER_SECOND};
use cutroom_project::ImportMode;
use cutroom_timeline::{MediaId, TrackKind};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{cairo, gdk, glib};
use std::cell::Cell;
use std::rc::Rc;

pub const CATEGORY_COUNT: usize = 9;
const CATEGORIES: [(&str, Option<(&str, &str)>); 9] = [
    ("Media", None),
    (
        "Audio",
        Some((
            "Phase 8",
            "Music and sound-effect library with licence and attribution tracking.",
        )),
    ),
    ("Text", None),
    ("Captions", None),
    (
        "Stickers",
        Some((
            "Phase 10",
            "SVG and animated stickers from permissively licensed packs.",
        )),
    ),
    (
        "Effects",
        Some(("Phase 4", "GPU effects built on the compositor.")),
    ),
    (
        "Transitions",
        Some(("Phase 4", "Crossfades and shader transitions.")),
    ),
    ("Filters", Some(("Phase 9", "`.cube` LUT filters."))),
    (
        "Adjust",
        Some((
            "Phase 9",
            "Exposure, contrast, saturation, temperature, lift/gamma/gain.",
        )),
    ),
];

pub struct EditorUi {
    pub root: gtk::Box,
    pub timeline: timeline::TimelineUi,
    timeline_scroll: gtk::ScrolledWindow,
    /// Dividers: library | preview, preview | inspector, top | timeline.
    panes: [gtk::Paned; 3],
    categories: Vec<gtk::ToggleButton>,
    library_stack: gtk::Stack,
    planned_title: gtk::Label,
    text_list: gtk::Box,
    pub captions: Rc<crate::captions_ui::Panel>,
    planned_text: gtk::Label,
    media_views: gtk::Stack,
    media_list: gtk::Box,
    media_grid: gtk::FlowBox,
    import_mode: gtk::DropDown,
    list_toggle: gtk::ToggleButton,
    grid_toggle: gtk::ToggleButton,
    preview_picture: gtk::Picture,
    transform_overlay: gtk::DrawingArea,
    preview_frame: gtk::AspectFrame,
    preview_scroller: gtk::ScrolledWindow,
    preview_overlay: gtk::Overlay,
    /// 1.0 = the picture fills the pane; more is zoomed in, less zoomed out.
    preview_zoom: Cell<f64>,
    /// Is the pointer over the preview? (Ctrl+= and Ctrl+- then zoom it.)
    preview_hover: Cell<bool>,
    /// The pane size and zoom the picture was last laid out for.
    zoom_layout: Cell<(i32, i32, i64)>,
    play_button: gtk::Button,
    timecode: gtk::Label,
    inspector: gtk::Box,
    tool_select: gtk::ToggleButton,
    tool_split: gtk::ToggleButton,
    split_button: gtk::Button,
    delete_button: gtk::Button,
    ripple_button: gtk::Button,
    undo_button: gtk::Button,
    redo_button: gtk::Button,
    snap: gtk::ToggleButton,
    navigator: gtk::DrawingArea,
    info: gtk::Box,
    edit_tab: gtk::ToggleButton,
    info_tab: gtk::ToggleButton,
    /// The clip that was selected last time, to notice a change of selection.
    last_selected: Cell<Option<Option<cutroom_timeline::ClipId>>>,
    zoom: gtk::Scale,
    toolbar_timecode: gtk::Label,
    /// True while widgets are being set from state.
    syncing: Cell<bool>,
    /// Where the playhead was when the inspector last followed it (for keyframed values).
    key_playhead: Cell<cutroom_timeline::Ticks>,
    /// The playhead the timeline view last made sure was visible.
    follow_playhead: Cell<cutroom_timeline::Ticks>,
}

fn button(app: &Rc<App>, text: &str, tip: &str, action: Action) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    b.set_tooltip_text(Some(tip));
    let app = app.clone();
    b.connect_clicked(move |_| app.run_action(action));
    b
}

pub fn build(app: &Rc<App>) -> EditorUi {
    // ---- library
    let library = gtk::Box::new(gtk::Orientation::Vertical, 4);
    library.set_size_request(380, -1);
    let category_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    category_row.set_homogeneous(true);
    category_row.set_margin_top(6);
    category_row.set_margin_start(6);
    category_row.set_margin_end(6);
    let mut categories: Vec<gtk::ToggleButton> = Vec::new();
    for (i, (name, planned)) in CATEGORIES.iter().enumerate() {
        let b = icons::tab(icons::category(name), name);
        if let Some(first) = categories.first() {
            b.set_group(Some(first));
        }
        if let Some((phase, _)) = planned {
            b.set_sensitive(false);
            b.set_tooltip_text(Some(&format!("{name}: planned for {phase}")));
        }
        let app = app.clone();
        b.connect_toggled(move |b| {
            if b.is_active() && !app.ui().editor.syncing.get() {
                app.state.borrow_mut().category = i;
                refresh_library(&app);
            }
        });
        category_row.append(&b);
        categories.push(b);
    }
    library.append(&category_row);
    library.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let library_stack = gtk::Stack::new();
    library_stack.set_vexpand(true);

    // Media: a source sidebar (Local / Import / Library) beside the items.
    let media_page = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 4);
    sidebar.set_size_request(96, -1);
    sidebar.set_margin_top(8);
    sidebar.set_margin_start(8);
    sidebar.set_margin_end(8);
    let local_item = gtk::ToggleButton::with_label("Local");
    local_item.add_css_class("flat");
    local_item.add_css_class("cc-sidebar-item");
    local_item.set_active(true);
    let import_item = button(app, "Import", "Ctrl+I", Action::ImportMedia);
    import_item.add_css_class("flat");
    import_item.add_css_class("cc-sidebar-item");
    let library_item = gtk::ToggleButton::with_label("Library");
    library_item.add_css_class("flat");
    library_item.add_css_class("cc-sidebar-item");
    icons::disable(&library_item, "Stock media library");
    for w in [
        local_item.upcast_ref::<gtk::Widget>(),
        import_item.upcast_ref(),
        library_item.upcast_ref(),
    ] {
        sidebar.append(w);
    }
    media_page.append(&sidebar);
    media_page.append(&gtk::Separator::new(gtk::Orientation::Vertical));

    let media_content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    media_content.set_hexpand(true);
    media_content.set_margin_top(8);
    media_content.set_margin_start(8);
    media_content.set_margin_end(8);
    let import_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let import_big = gtk::Button::new();
    {
        import_big.set_child(Some(&icons::with_label(icons::IMPORT, "Import")));
        import_big.add_css_class("flat");
        import_big.add_css_class("cc-import-button");
        import_big.set_tooltip_text(Some("Ctrl+I"));
        let app = app.clone();
        import_big.connect_clicked(move |_| app.run_action(Action::ImportMedia));
    }
    import_row.append(&import_big);
    let folder_button = icons::button(icons::FOLDER, "Import every media file in a folder");
    {
        let app = app.clone();
        folder_button.connect_clicked(move |_| app.run_action(Action::ImportFolder));
    }
    import_row.append(&folder_button);
    let paste_button = icons::button(
        icons::PASTE,
        "Paste files, a picture, or a copied clip at the playhead (Ctrl+V)",
    );
    {
        let app = app.clone();
        paste_button.connect_clicked(move |_| app.run_action(Action::Paste));
    }
    import_row.append(&paste_button);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    import_row.append(&spacer);
    let all_filter = gtk::Button::new();
    all_filter.set_child(Some(&icons::with_label(icons::FILTER, "All")));
    all_filter.add_css_class("flat");
    icons::disable(&all_filter, "Filter by type");
    import_row.append(&all_filter);
    let list_toggle = icons::toggle(icons::LIST, "List");
    let grid_toggle = icons::toggle(icons::GRID, "Thumbnails");
    grid_toggle.set_group(Some(&list_toggle));
    for (b, mode) in [
        (&list_toggle, ViewMode::List),
        (&grid_toggle, ViewMode::Thumbnails),
    ] {
        let app = app.clone();
        b.connect_toggled(move |b| {
            if app.ui().editor.syncing.get() || !b.is_active() {
                return;
            }
            {
                let mut st = app.state.borrow_mut();
                st.settings.library_view = mode;
                st.save_settings();
            }
            refresh_library(&app);
        });
        import_row.append(b);
    }
    media_content.append(&import_row);

    let media_views = gtk::Stack::new();
    media_views.set_vexpand(true);
    let media_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let list_scroll = gtk::ScrolledWindow::new();
    list_scroll.set_child(Some(&media_list));
    media_views.add_named(&list_scroll, Some("list"));
    let media_grid = gtk::FlowBox::new();
    media_grid.set_selection_mode(gtk::SelectionMode::None);
    media_grid.set_homogeneous(true);
    media_grid.set_min_children_per_line(2);
    media_grid.set_max_children_per_line(4);
    media_grid.set_valign(gtk::Align::Start);
    media_grid.set_row_spacing(8);
    media_grid.set_column_spacing(8);
    let grid_scroll = gtk::ScrolledWindow::new();
    grid_scroll.set_child(Some(&media_grid));
    media_views.add_named(&grid_scroll, Some("grid"));
    let empty = gtk::Label::new(Some(
        "No media yet. Import or drop files to begin.\nDrag an item onto the timeline to place it.",
    ));
    empty.add_css_class("dim-label");
    empty.set_wrap(true);
    empty.set_valign(gtk::Align::Start);
    media_views.add_named(&empty, Some("empty"));
    media_content.append(&media_views);

    // The rarely-changed import options live in a fold-out at the bottom.
    let options = gtk::Expander::new(Some("Import options"));
    let options_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    options_box.set_margin_top(4);
    options_box.set_margin_bottom(6);
    let import_mode = gtk::DropDown::from_strings(&["Link to original", "Copy into project"]);
    {
        let app = app.clone();
        import_mode.connect_selected_notify(move |d| {
            if app.ui().editor.syncing.get() {
                return;
            }
            if let Some(s) = app.state.borrow_mut().session.as_mut() {
                s.import_mode = if d.selected() == 1 {
                    ImportMode::Copy
                } else {
                    ImportMode::Reference
                };
            }
        });
    }
    options_box.append(&import_mode);
    options.set_child(Some(&options_box));
    media_content.append(&options);
    media_page.append(&media_content);
    library_stack.add_named(&media_page, Some("media"));

    let planned = gtk::Box::new(gtk::Orientation::Vertical, 6);
    planned.set_valign(gtk::Align::Start);
    planned.set_margin_top(10);
    planned.set_margin_start(12);
    planned.set_margin_end(12);
    let planned_title = gtk::Label::new(None);
    planned_title.add_css_class("panel-title");
    planned_title.set_xalign(0.0);
    let planned_text = gtk::Label::new(None);
    planned_text.set_xalign(0.0);
    planned_text.set_wrap(true);
    planned.append(&planned_title);
    planned.append(&planned_text);
    library_stack.add_named(&planned, Some("planned"));

    // Text: ready-made styles; clicking one puts a text clip at the playhead.
    let text_list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    text_list.set_margin_top(8);
    text_list.set_margin_bottom(8);
    text_list.set_margin_start(10);
    text_list.set_margin_end(10);
    let text_scroll = gtk::ScrolledWindow::new();
    text_scroll.set_vexpand(true);
    text_scroll.set_child(Some(&text_list));
    library_stack.add_named(&text_scroll, Some("text"));
    let captions = crate::captions_ui::build();
    let captions_scroll = gtk::ScrolledWindow::new();
    captions_scroll.set_vexpand(true);
    captions_scroll.set_child(Some(&captions.root));
    library_stack.add_named(&captions_scroll, Some("captions"));
    library.append(&library_stack);
    library.add_css_class("cc-panel");

    // ---- preview
    let preview = gtk::Box::new(gtk::Orientation::Vertical, 4);
    preview.add_css_class("cc-panel");
    let player_head = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let player_title = gtk::Label::new(Some("Player"));
    player_title.add_css_class("cc-panel-heading");
    player_title.set_hexpand(true);
    player_title.set_xalign(0.0);
    player_head.append(&player_title);
    let player_menu = icons::placeholder(icons::MENU, "Player options");
    player_menu.set_margin_end(6);
    player_head.append(&player_menu);
    preview.append(&player_head);
    let stage = gtk::Box::new(gtk::Orientation::Vertical, 0);
    stage.add_css_class("preview-stage");
    stage.set_vexpand(true);
    stage.set_hexpand(true);
    stage.set_margin_start(6);
    stage.set_margin_end(6);
    let preview_frame = gtk::AspectFrame::new(0.5, 0.5, 9.0 / 16.0, false);
    preview_frame.set_hexpand(true);
    preview_frame.set_vexpand(true);
    let preview_picture = gtk::Picture::new();
    preview_picture.set_content_fit(gtk::ContentFit::Contain);
    preview_picture.set_can_shrink(true);
    // The black canvas is the ratio-constrained child, so its edge is the video's edge.
    let canvas = gtk::Box::new(gtk::Orientation::Vertical, 0);
    canvas.add_css_class("preview-canvas");
    preview_picture.set_hexpand(true);
    preview_picture.set_vexpand(true);
    canvas.append(&preview_picture);
    preview_frame.set_child(Some(&canvas));
    // The transform box floats over the whole stage, so its handles can be
    // reached even where the picture sticks out past the canvas.
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&preview_frame));
    let transform_overlay = crate::transform_overlay::build(app, canvas.upcast_ref());
    overlay.add_overlay(&transform_overlay);
    // The picture can be zoomed beyond the pane: the pane then scrolls.
    let preview_scroller = gtk::ScrolledWindow::new();
    preview_scroller.set_hexpand(true);
    preview_scroller.set_vexpand(true);
    preview_scroller.set_child(Some(&overlay));
    {
        // Ctrl + wheel zooms; a plain wheel scrolls a zoomed-in picture.
        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
        let wheel_app = app.clone();
        wheel.connect_scroll(move |c, _, dy| {
            if !c
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK)
            {
                return glib::Propagation::Proceed;
            }
            // A wheel click is 1.0; a touchpad sends many small steps.
            zoom_preview(&wheel_app, 1.15f64.powf(-dy.clamp(-4.0, 4.0)));
            glib::Propagation::Stop
        });
        preview_scroller.add_controller(wheel);
        // Pinching in or out on a touchpad or touch screen zooms too.
        let pinch = gtk::GestureZoom::new();
        pinch.set_propagation_phase(gtk::PropagationPhase::Capture);
        let started = Rc::new(Cell::new(1.0f64));
        {
            let (app, started) = (app.clone(), started.clone());
            pinch.connect_begin(move |_, _| started.set(app.ui().editor.preview_zoom.get()));
        }
        {
            let app = app.clone();
            pinch.connect_scale_changed(move |_, scale| {
                set_preview_zoom(&app, started.get() * scale);
            });
        }
        preview_scroller.add_controller(pinch);
        let motion = gtk::EventControllerMotion::new();
        let (enter, leave) = (app.clone(), app.clone());
        motion.connect_enter(move |_, _, _| enter.ui().editor.preview_hover.set(true));
        motion.connect_leave(move |_| leave.ui().editor.preview_hover.set(false));
        preview_scroller.add_controller(motion);
    }
    stage.append(&preview_scroller);
    preview.append(&stage);

    let transport = gtk::CenterBox::new();
    transport.set_margin_start(12);
    transport.set_margin_end(12);
    transport.set_margin_bottom(4);
    transport.set_margin_top(2);
    transport.add_css_class("cc-transport");
    let timecode = gtk::Label::new(None);
    timecode.set_use_markup(true);
    timecode.set_xalign(0.0);
    transport.set_start_widget(Some(&timecode));
    let centre = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    let to_start = icons::button(icons::TO_START, "Go to start (Home)");
    {
        let app = app.clone();
        to_start.connect_clicked(move |_| app.run_action(Action::GoToStart));
    }
    let play_button = icons::button(icons::PLAY, "Play / pause (Space)");
    play_button.set_widget_name(icons::PLAY);
    play_button.add_css_class("cc-play");
    {
        let app = app.clone();
        play_button.connect_clicked(move |_| app.run_action(Action::TogglePlay));
    }
    let to_end = icons::button(icons::TO_END, "Go to end (End)");
    {
        let app = app.clone();
        to_end.connect_clicked(move |_| app.run_action(Action::GoToEnd));
    }
    centre.append(&to_start);
    centre.append(&play_button);
    centre.append(&to_end);
    transport.set_center_widget(Some(&centre));
    let view_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    for (icon, tip, action) in [
        (
            icons::ZOOM_OUT,
            "Zoom the preview out (Ctrl+- or Ctrl+wheel over it)",
            0,
        ),
        (icons::RESET, "Reset the preview zoom", 1),
        (
            icons::ZOOM_IN,
            "Zoom the preview in (Ctrl+= or Ctrl+wheel over it)",
            2,
        ),
    ] {
        let b = icons::button(icon, tip);
        let app = app.clone();
        b.connect_clicked(move |_| match action {
            0 => zoom_preview(&app, 1.0 / 1.25),
            1 => reset_preview_zoom(&app),
            _ => zoom_preview(&app, 1.25),
        });
        view_buttons.append(&b);
    }
    view_buttons.append(&icons::placeholder(icons::FIT, "Fit the picture"));
    let original = gtk::Button::with_label("Original");
    original.add_css_class("flat");
    original.add_css_class("cc-pill");
    original.set_valign(gtk::Align::Center);
    icons::disable(&original, "Preview ratio");
    view_buttons.append(&original);
    view_buttons.append(&icons::placeholder(
        icons::FULLSCREEN,
        "Full screen preview",
    ));
    transport.set_end_widget(Some(&view_buttons));
    preview.append(&transport);

    // ---- inspector
    let inspector = gtk::Box::new(gtk::Orientation::Vertical, 6);
    inspector.set_margin_top(8);
    inspector.set_margin_start(12);
    inspector.set_margin_end(12);
    let edit_scroll = gtk::ScrolledWindow::new();
    edit_scroll.set_child(Some(&inspector));
    edit_scroll.set_vexpand(true);
    let info = gtk::Box::new(gtk::Orientation::Vertical, 6);
    info.set_margin_top(8);
    info.set_margin_start(12);
    info.set_margin_end(12);
    let info_scroll = gtk::ScrolledWindow::new();
    info_scroll.set_child(Some(&info));
    info_scroll.set_vexpand(true);
    info_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    let right_stack = gtk::Stack::new();
    right_stack.add_named(&edit_scroll, Some("edit"));
    right_stack.add_named(&info_scroll, Some("info"));
    right_stack.set_vexpand(true);
    let edit_tab = icons::text_tab("Edit");
    let info_tab = icons::text_tab("Info");
    info_tab.set_group(Some(&edit_tab));
    edit_tab.set_active(true);
    for (tab, page) in [(&edit_tab, "edit"), (&info_tab, "info")] {
        let stack = right_stack.clone();
        tab.connect_toggled(move |t| {
            if t.is_active() {
                stack.set_visible_child_name(page);
            }
        });
    }
    let right_tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    right_tabs.set_margin_top(6);
    right_tabs.set_margin_start(12);
    right_tabs.set_margin_end(12);
    right_tabs.append(&edit_tab);
    right_tabs.append(&info_tab);
    let inspector_scroll = gtk::Box::new(gtk::Orientation::Vertical, 0);
    inspector_scroll.set_size_request(340, -1);
    inspector_scroll.add_css_class("cc-panel");
    inspector_scroll.append(&right_tabs);
    inspector_scroll.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    inspector_scroll.append(&right_stack);

    // ---- timeline panel
    let timeline_ui = timeline::build(app);
    let timeline_scroll = gtk::ScrolledWindow::new();
    timeline_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    timeline_scroll.set_child(Some(&timeline_ui.area));
    timeline_scroll.set_vexpand(true);
    {
        let area = timeline_ui.area.clone();
        timeline_scroll
            .vadjustment()
            .connect_value_changed(move |_| area.queue_draw());
    }

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    toolbar.set_margin_start(8);
    toolbar.set_margin_end(8);
    toolbar.set_margin_top(4);
    toolbar.set_margin_bottom(4);
    let tool_select = icons::toggle(icons::SELECT_TOOL, "Select tool (A)");
    let tool_split = icons::toggle(
        icons::SPLIT_TOOL,
        "Split tool: click a clip to cut it there (B)",
    );
    tool_split.set_group(Some(&tool_select));
    for b in [&tool_select, &tool_split] {
        b.add_css_class("flat");
    }
    for (b, action) in [
        (&tool_select, Action::SelectTool),
        (&tool_split, Action::SplitTool),
    ] {
        let app = app.clone();
        b.connect_toggled(move |b| {
            if b.is_active() && !app.ui().editor.syncing.get() {
                app.run_action(action);
            }
        });
    }
    let tools = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tools.add_css_class("linked");
    tools.append(&tool_select);
    tools.append(&tool_split);
    let icon_action = |icon: &'static str, tip: &str, action: Action| {
        let b = icons::button(icon, tip);
        let app = app.clone();
        b.connect_clicked(move |_| app.run_action(action));
        b
    };
    toolbar.append(&icon_action(
        icons::ADD_VIDEO_TRACK,
        "Add a video track",
        Action::AddTrack(TrackKind::Video),
    ));
    toolbar.append(&icon_action(
        icons::ADD_AUDIO_TRACK,
        "Add an audio track",
        Action::AddTrack(TrackKind::Audio),
    ));
    let sep = gtk::Separator::new(gtk::Orientation::Vertical);
    sep.set_margin_start(4);
    sep.set_margin_end(4);
    toolbar.append(&sep);
    toolbar.append(&tools);
    let undo_button = icon_action(icons::UNDO, "Undo (Ctrl+Z)", Action::Undo);
    let redo_button = icon_action(icons::REDO, "Redo (Ctrl+Shift+Z)", Action::Redo);
    let split_button = icon_action(
        icons::SPLIT,
        "Split at the playhead (Ctrl+B)",
        Action::Split,
    );
    let delete_button = icon_action(
        icons::DELETE,
        "Delete the selected clip (Del)",
        Action::Delete,
    );
    let ripple_button = icon_action(
        icons::RIPPLE_DELETE,
        "Delete and close the gap (Shift+Del)",
        Action::RippleDelete,
    );
    for b in [
        &undo_button,
        &redo_button,
        &split_button,
        &delete_button,
        &ripple_button,
    ] {
        toolbar.append(b);
    }
    toolbar.append(&icons::placeholder(icons::GROUP, "Group clips"));
    let toolbar_gap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    toolbar_gap.set_hexpand(true);
    toolbar.append(&toolbar_gap);
    let toolbar_timecode = gtk::Label::new(None);
    toolbar_timecode.add_css_class("timecode");
    toolbar_timecode.add_css_class("dim-label");
    toolbar_timecode.set_margin_end(8);
    toolbar.append(&toolbar_timecode);
    toolbar.append(&icons::placeholder(icons::MIC, "Record a voice-over"));
    toolbar.append(&icons::placeholder(icons::LINK, "Link clips"));
    let snap = icons::toggle(
        icons::SNAP,
        "Snap to clip edges, the start, and the playhead",
    );
    snap.set_active(true);
    {
        let app = app.clone();
        snap.connect_toggled(move |c| {
            if !app.ui().editor.syncing.get() {
                app.state.borrow_mut().snap = c.is_active();
            }
        });
    }
    toolbar.append(&snap);
    toolbar.append(&icon_action(
        icons::ZOOM_OUT,
        "Zoom out (Ctrl+minus)",
        Action::ZoomOut,
    ));
    let zoom = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        MIN_PIXELS_PER_SECOND.ln(),
        MAX_PIXELS_PER_SECOND.ln(),
        0.05,
    );
    zoom.set_draw_value(false);
    zoom.set_size_request(120, -1);
    zoom.set_tooltip_text(Some(
        "Zoom (Ctrl+scroll over the timeline, or Ctrl+plus / Ctrl+minus)",
    ));
    {
        let app = app.clone();
        zoom.connect_value_changed(move |s| {
            if app.ui().editor.syncing.get() {
                return;
            }
            app.state.borrow_mut().view.pixels_per_second = s.value().exp();
            timeline::queue_draw(&app);
        });
    }
    toolbar.append(&zoom);
    toolbar.append(&icon_action(
        icons::ZOOM_IN,
        "Zoom in (Ctrl+plus)",
        Action::ZoomIn,
    ));
    toolbar.append(&icon_action(
        icons::FIT_TIMELINE,
        "Fit the whole timeline",
        Action::FitTimeline,
    ));

    let timeline_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    timeline_panel.add_css_class("cc-panel");
    timeline_panel.append(&toolbar);
    timeline_panel.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    timeline_panel.append(&timeline_scroll);
    let navigator = crate::navigator::build(app);
    timeline_panel.append(&navigator);

    // ---- arrange
    let right = gtk::Paned::new(gtk::Orientation::Horizontal);
    right.set_start_child(Some(&preview));
    right.set_end_child(Some(&inspector_scroll));
    right.set_resize_start_child(true);
    right.set_shrink_start_child(false);
    right.set_resize_end_child(false);
    right.set_shrink_end_child(false);
    let top = gtk::Paned::new(gtk::Orientation::Horizontal);
    top.set_start_child(Some(&library));
    top.set_end_child(Some(&right));
    top.set_resize_start_child(false);
    top.set_shrink_start_child(false);
    top.set_resize_end_child(true);
    top.set_shrink_end_child(false);
    let main = gtk::Paned::new(gtk::Orientation::Vertical);
    main.set_start_child(Some(&top));
    main.set_end_child(Some(&timeline_panel));
    main.set_resize_start_child(true);
    main.set_shrink_start_child(false);
    main.set_resize_end_child(true);
    main.set_shrink_end_child(false);
    main.set_position(470);
    top.set_position(310);
    right.set_position(760);

    for (w, m) in [
        (library.upcast_ref::<gtk::Widget>(), (4, 2, 2, 4)),
        (preview.upcast_ref(), (4, 2, 2, 2)),
        (inspector_scroll.upcast_ref(), (4, 4, 2, 2)),
        (timeline_panel.upcast_ref(), (2, 4, 4, 4)),
    ] {
        w.set_margin_top(m.0);
        w.set_margin_end(m.1);
        w.set_margin_bottom(m.2);
        w.set_margin_start(m.3);
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main.set_vexpand(true);
    root.append(&main);

    EditorUi {
        root,
        timeline: timeline_ui,
        timeline_scroll,
        panes: [top.clone(), right.clone(), main.clone()],
        categories,
        library_stack,
        planned_title,
        text_list,
        captions,
        planned_text,
        media_views,
        media_list,
        media_grid,
        import_mode,
        list_toggle,
        grid_toggle,
        preview_picture,
        transform_overlay,
        preview_frame,
        preview_scroller,
        preview_overlay: overlay,
        preview_zoom: Cell::new(1.0),
        preview_hover: Cell::new(false),
        zoom_layout: Cell::new((0, 0, 0)),
        play_button,
        timecode,
        inspector,
        tool_select,
        tool_split,
        split_button,
        delete_button,
        ripple_button,
        undo_button,
        redo_button,
        snap,
        navigator,
        info,
        edit_tab,
        info_tab,
        last_selected: Cell::new(None),
        zoom,
        toolbar_timecode,
        syncing: Cell::new(false),
        key_playhead: Cell::new(cutroom_timeline::Ticks::ZERO),
        follow_playhead: Cell::new(cutroom_timeline::Ticks(-1)),
    }
}

// ----- refreshing --------------------------------------------------------------------

fn clear_box(b: &gtk::Box) {
    while let Some(child) = b.first_child() {
        b.remove(&child);
    }
}

/// The timeline's zoom or scroll changed outside the zoom slider (for example by
/// the navigator bar): bring the slider and every view of it up to date.
pub fn view_changed(app: &Rc<App>) {
    let ui = &app.ui().editor;
    let pps = app.state.borrow().view.pixels_per_second;
    ui.syncing.set(true);
    ui.zoom.set_value(pps.ln());
    ui.syncing.set(false);
    timeline::queue_draw(app);
}

impl EditorUi {
    pub fn pane_positions(&self) -> [i32; 3] {
        [
            self.panes[0].position(),
            self.panes[1].position(),
            self.panes[2].position(),
        ]
    }

    pub fn set_pane_positions(&self, positions: [i32; 3]) {
        for (pane, pos) in self.panes.iter().zip(positions) {
            if pos > 0 {
                pane.set_position(pos);
            }
        }
    }

    pub fn track_scroll(&self) -> f64 {
        self.timeline_scroll.vadjustment().value()
    }

    pub fn set_track_scroll(&self, value: f64) {
        self.timeline_scroll.vadjustment().set_value(value);
    }

    /// Redraw the navigator bar.
    pub fn queue_navigator(&self) {
        self.navigator.queue_draw();
    }

    /// The inspector's widget tree (tests drive the real controls).
    #[cfg(test)]
    pub fn inspector_widget(&self) -> gtk::Box {
        self.inspector.clone()
    }

    /// Is the pointer over the preview pane?
    pub fn preview_hovered(&self) -> bool {
        self.preview_hover.get()
    }

    /// Redraw the transform box (it follows live edits).
    pub fn queue_overlay(&self) {
        self.transform_overlay.queue_draw();
    }

    /// Show the compositor's latest picture.
    pub fn set_preview_texture(&self, texture: &gdk::Texture) {
        self.preview_picture.set_paintable(Some(texture));
    }
}

/// Zoom the preview picture by `factor` (kept between 20% and 800%).
pub fn zoom_preview(app: &Rc<App>, factor: f64) {
    set_preview_zoom(app, app.ui().editor.preview_zoom.get() * factor);
}

/// Zoom the preview picture to exactly `to` (kept between 20% and 800%).
pub fn set_preview_zoom(app: &Rc<App>, to: f64) {
    let ui = &app.ui().editor;
    let z = to.clamp(0.2, 8.0);
    // Snap back to exactly "fits the pane" when close to it.
    ui.preview_zoom
        .set(if (z - 1.0).abs() < 0.04 { 1.0 } else { z });
    layout_preview_zoom(app);
}

/// Back to the picture filling the pane.
pub fn reset_preview_zoom(app: &Rc<App>) {
    app.ui().editor.preview_zoom.set(1.0);
    layout_preview_zoom(app);
}

/// Size the picture for the current zoom and pane (does nothing if neither changed).
fn layout_preview_zoom(app: &Rc<App>) {
    let ui = &app.ui().editor;
    let (vw, vh) = (ui.preview_scroller.width(), ui.preview_scroller.height());
    let z = ui.preview_zoom.get();
    let key = (vw, vh, (z * 1000.0).round() as i64);
    if ui.zoom_layout.get() == key || vw < 10 || vh < 10 {
        return;
    }
    let before = ui.zoom_layout.get();
    ui.zoom_layout.set(key);
    let ratio = f64::from(ui.preview_frame.ratio()).max(0.01);
    let overlay = &ui.preview_overlay;
    if (z - 1.0).abs() < 0.001 {
        overlay.set_size_request(-1, -1);
        overlay.set_halign(gtk::Align::Fill);
        overlay.set_valign(gtk::Align::Fill);
    } else {
        // The picture that fits the pane, then scaled by the zoom.
        let (fw, fh) = if f64::from(vw) / f64::from(vh) > ratio {
            (f64::from(vh) * ratio, f64::from(vh))
        } else {
            (f64::from(vw), f64::from(vw) / ratio)
        };
        overlay.set_size_request((fw * z).round() as i32, (fh * z).round() as i32);
        overlay.set_halign(gtk::Align::Center);
        overlay.set_valign(gtk::Align::Center);
    }
    // Keep what is in the middle of the pane in the middle.
    let old_z = if before.2 == 0 {
        1.0
    } else {
        before.2 as f64 / 1000.0
    };
    let (h, v) = (
        ui.preview_scroller.hadjustment(),
        ui.preview_scroller.vadjustment(),
    );
    let scale = z / old_z;
    let centre =
        |a: &gtk::Adjustment| (a.value() + a.page_size() / 2.0) * scale - a.page_size() / 2.0;
    let (nh, nv) = (centre(&h), centre(&v));
    glib::idle_add_local_once(move || {
        h.set_value(nh.max(0.0));
        v.set_value(nv.max(0.0));
    });
    ui.transform_overlay.queue_draw();
}

pub fn refresh(app: &Rc<App>) {
    app.ui().editor.transform_overlay.queue_draw();
    let ui = &app.ui().editor;
    {
        let st = app.state.borrow();
        let Some(session) = st.session.as_ref() else {
            return;
        };
        ui.syncing.set(true);
        ui.tool_select.set_active(st.tool == Tool::Select);
        ui.tool_split.set_active(st.tool == Tool::Split);
        ui.snap.set_active(st.snap);
        ui.zoom.set_value(st.view.pixels_per_second.ln());
        ui.split_button.set_sensitive(session.selected.is_some());
        ui.delete_button.set_sensitive(session.selected.is_some());
        ui.ripple_button.set_sensitive(session.selected.is_some());
        ui.undo_button.set_sensitive(session.history.can_undo());
        ui.redo_button.set_sensitive(session.history.can_redo());
        ui.import_mode
            .set_selected(if session.import_mode == ImportMode::Copy {
                1
            } else {
                0
            });
        match st.settings.library_view {
            ViewMode::List => ui.list_toggle.set_active(true),
            ViewMode::Thumbnails => ui.grid_toggle.set_active(true),
        }
        if let Some(b) = ui.categories.get(st.category) {
            b.set_active(true);
        }
        let settings = &session.doc.settings;
        ui.preview_frame
            .set_ratio(settings.width as f32 / settings.height.max(1) as f32);
        app.render_preview();
        ui.timeline
            .area
            .set_content_height(timeline::content_height(session.doc.timeline.tracks.len()));
        ui.syncing.set(false);
    }
    refresh_library(app);
    refresh_inspector(app);
    on_tick(app);
}

impl EditorUi {
    #[cfg(test)]
    pub fn library_stack_name(&self) -> Option<String> {
        self.library_stack
            .visible_child_name()
            .map(|n| n.to_string())
    }

    #[cfg(test)]
    pub fn library_list_for_tests(&self) -> &gtk::Box {
        &self.media_list
    }
}

/// "now / total" with the current time in the accent colour.
fn timecode_markup(text: &str) -> String {
    let (now, total) = text.split_once(" / ").unwrap_or((text, ""));
    format!(
        "<span foreground=\"#26c6b0\" font_features=\"tnum\">{now}</span>  <span alpha=\"65%\" font_features=\"tnum\">{total}</span>"
    )
}

/// Update the parts that change while playing: timecodes, play button, timeline.
pub fn on_tick(app: &Rc<App>) {
    let ui = &app.ui().editor;
    ui.transform_overlay.queue_draw();
    layout_preview_zoom(app);
    let (text, playing) = {
        let mut st = app.state.borrow_mut();
        let width =
            f64::from(ui.timeline.area.width()) - f64::from(cutroom_core::timeline_logic::HEADER_W);
        if width > 100.0 {
            st.timeline_width = width;
        }
        let Some(session) = st.session.as_ref() else {
            return;
        };
        let rate = session.doc.settings.frame_rate;
        let text = format!(
            "{} / {}",
            view::timecode(session.playhead, rate),
            view::timecode(session.timeline_end(), rate)
        );
        let (playing, playhead) = (session.playing, session.playhead);
        // Keep the playhead in view while playing, and after it jumps (keys,
        // clicks, Home/End). A drag scrolls smoothly by itself at the edges.
        let previous = ui.follow_playhead.get();
        let jumped = previous.0 >= 0 && playhead != previous;
        ui.follow_playhead.set(playhead);
        let dragging = !matches!(st.drag, cutroom_core::timeline_logic::Drag::None);
        if (playing || jumped) && !dragging {
            let w = st.timeline_width as f32;
            st.view.follow(playhead, w);
        }
        (text, playing)
    };
    // Keyframed values follow the playhead: rebuild the inspector when it moved.
    let follow = {
        let st = app.state.borrow();
        st.session.as_ref().is_some_and(|s| {
            s.playhead != ui.key_playhead.get()
                && s.selection().len() == 1
                && s.doc
                    .timeline
                    .find_clip(s.selection()[0])
                    .is_some_and(|(_, c)| !c.keyframes.is_empty())
        })
    };
    if follow {
        let playhead = app
            .state
            .borrow()
            .session
            .as_ref()
            .map_or(cutroom_timeline::Ticks::ZERO, |s| s.playhead);
        ui.key_playhead.set(playhead);
        refresh_inspector(app);
    }
    ui.timecode.set_markup(&timecode_markup(&text));
    ui.toolbar_timecode.set_text(&text);
    icons::set_button_icon(
        &ui.play_button,
        if playing { icons::PAUSE } else { icons::PLAY },
        16,
    );
    timeline::queue_draw(app);
}

/// The Text category: one button per built-in style.
fn fill_text_styles(app: &Rc<App>) {
    let ui = &app.ui().editor;
    clear_box(&ui.text_list);
    let hint = gtk::Label::new(Some("Click a style to add text at the playhead."));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("dim-label");
    ui.text_list.append(&hint);
    for template in cutroom_timeline::text::templates() {
        let button = gtk::Button::new();
        button.add_css_class("cc-text-style");
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let name = gtk::Label::new(Some(template.name));
        name.set_xalign(0.0);
        name.add_css_class("panel-title");
        let blurb = gtk::Label::new(Some(template.blurb));
        blurb.set_xalign(0.0);
        blurb.add_css_class("dim-label");
        blurb.add_css_class("caption");
        inner.append(&name);
        inner.append(&blurb);
        button.set_child(Some(&inner));
        let (app, spec) = (app.clone(), template.spec.clone());
        button.connect_clicked(move |_| {
            {
                let mut st = app.state.borrow_mut();
                if let Some(session) = st.session.as_mut() {
                    session.add_text_clip(spec.clone());
                }
            }
            app.refresh();
        });
        ui.text_list.append(&button);
    }
}

pub fn refresh_library(app: &Rc<App>) {
    crate::library_menu::dismiss(app);
    let ui = &app.ui().editor;
    let category = app.state.borrow().category;
    if let (_, Some((phase, description))) = CATEGORIES[category.min(CATEGORIES.len() - 1)] {
        ui.planned_title.set_text(CATEGORIES[category].0);
        ui.planned_text
            .set_text(&format!("{description}\n\nPlanned for {phase}."));
        ui.library_stack.set_visible_child_name("planned");
        return;
    }
    if CATEGORIES[category.min(CATEGORIES.len() - 1)].0 == "Captions" {
        crate::captions_ui::refresh(app, &ui.captions);
        ui.library_stack.set_visible_child_name("captions");
        return;
    }
    if CATEGORIES[category.min(CATEGORIES.len() - 1)].0 == "Text" {
        fill_text_styles(app);
        ui.library_stack.set_visible_child_name("text");
        return;
    }
    ui.library_stack.set_visible_child_name("media");

    clear_box(&ui.media_list);
    while let Some(child) = ui.media_grid.first_child() {
        ui.media_grid.remove(&child);
    }

    let (mode, items): (
        ViewMode,
        Vec<(MediaId, String, MediaKind, bool, Option<std::path::PathBuf>)>,
    ) = {
        let st = app.state.borrow();
        let Some(session) = st.session.as_ref() else {
            return;
        };
        let items = session
            .doc
            .media
            .iter()
            .map(|m| {
                (
                    m.id,
                    media_label(m),
                    session.media_kind_of(m.id),
                    session.missing.contains(&m.id),
                    session.media_path(m.id),
                )
            })
            .collect();
        (st.settings.library_view, items)
    };
    if items.is_empty() {
        ui.media_views.set_visible_child_name("empty");
        return;
    }
    let dark = app.state.borrow().dark;
    let selected = app.state.borrow().selected_media;
    for (id, name, kind, missing, path) in items {
        let widget = match mode {
            ViewMode::List => media_row(app, id, &name, kind, missing),
            ViewMode::Thumbnails => media_card(app, id, &name, kind, missing, path, dark),
        };
        if selected == Some(id) {
            widget.add_css_class("cutroom-selected");
        }
        match mode {
            ViewMode::List => ui.media_list.append(&widget),
            ViewMode::Thumbnails => ui.media_grid.append(&widget),
        }
    }
    ui.media_views
        .set_visible_child_name(if mode == ViewMode::List {
            "list"
        } else {
            "grid"
        });
}

fn make_draggable(app: &Rc<App>, widget: &impl IsA<gtk::Widget>, id: MediaId) {
    let source = gtk::DragSource::builder()
        .actions(gdk::DragAction::COPY)
        .build();
    source.connect_prepare(move |_, _, _| {
        Some(gdk::ContentProvider::for_value(
            &format!("cutroom-media:{}", id.0).to_value(),
        ))
    });
    {
        let (app, widget) = (app.clone(), widget.clone().upcast::<gtk::Widget>());
        source.connect_drag_begin(move |s, _| {
            app.state.borrow_mut().dragging_media = Some(id);
            let paintable = gtk::WidgetPaintable::new(Some(&widget));
            s.set_icon(Some(&paintable), 0, 0);
        });
    }
    {
        let app = app.clone();
        source.connect_drag_end(move |_, _, _| {
            let mut st = app.state.borrow_mut();
            st.dragging_media = None;
            st.drop_ghost = None;
            drop(st);
            timeline::queue_draw(&app);
        });
    }
    widget.add_controller(source);

    let click = gtk::GestureClick::builder().button(1).build();
    {
        let app = app.clone();
        click.connect_released(move |_, presses, _, _| {
            if presses >= 2 {
                let app = app.clone();
                glib::idle_add_local_once(move || add_to_timeline(&app, id));
            }
        });
    }
    widget.add_controller(click);

    // Left click selects (highlight); right click selects and opens the menu.
    let select = gtk::GestureClick::builder().button(0).build();
    {
        let (app, widget) = (app.clone(), widget.clone().upcast::<gtk::Widget>());
        select.connect_pressed(move |gesture, _, x, y| {
            let button = gesture.current_button();
            if button != 1 && button != 3 {
                return;
            }
            app.state.borrow_mut().selected_media = Some(id);
            highlight_only(&widget);
            if button == 3 {
                crate::library_menu::show(&app, &widget, id, x, y);
            }
        });
    }
    widget.add_controller(select);
}

/// Mark `widget` as the selected library item and clear the mark on its siblings.
fn highlight_only(widget: &gtk::Widget) {
    let siblings_of = |w: &gtk::Widget| {
        let holder = w
            .parent()
            .filter(|p| p.is::<gtk::FlowBoxChild>())
            .unwrap_or_else(|| w.clone());
        let mut found = Vec::new();
        let mut next = holder.parent().and_then(|p| p.first_child());
        while let Some(sibling) = next {
            // Cards sit inside a FlowBoxChild; rows are direct children.
            found.push(
                sibling
                    .downcast_ref::<gtk::FlowBoxChild>()
                    .and_then(|c| c.child())
                    .unwrap_or(sibling.clone()),
            );
            next = sibling.next_sibling();
        }
        found
    };
    for sibling in siblings_of(widget) {
        sibling.remove_css_class("cutroom-selected");
    }
    widget.add_css_class("cutroom-selected");
}

fn add_to_timeline(app: &Rc<App>, id: MediaId) {
    if let Some(s) = app.state.borrow_mut().session.as_mut() {
        s.add_media_to_timeline(id);
    }
    app.refresh();
}

fn media_row(
    app: &Rc<App>,
    id: MediaId,
    name: &str,
    kind: MediaKind,
    missing: bool,
) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("cutroom-row");
    let label = gtk::Label::new(Some(name));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    if missing {
        label.add_css_class("error");
    }
    row.append(&label);
    let kind_label = gtk::Label::new(Some(kind.label()));
    kind_label.add_css_class("dim-label");
    kind_label.add_css_class("caption");
    row.append(&kind_label);
    if missing {
        let m = gtk::Label::new(Some("missing"));
        m.add_css_class("error");
        m.add_css_class("caption");
        row.append(&m);
    }
    let add = gtk::Button::with_label("Add");
    add.add_css_class("flat");
    add.set_tooltip_text(Some("Add to the timeline at the playhead"));
    {
        let app = app.clone();
        add.connect_clicked(move |_| add_to_timeline(&app, id));
    }
    row.append(&add);
    make_draggable(app, &row, id);
    row.upcast()
}

fn media_card(
    app: &Rc<App>,
    id: MediaId,
    name: &str,
    kind: MediaKind,
    missing: bool,
    path: Option<std::path::PathBuf>,
    dark: bool,
) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 3);
    card.set_size_request(112, -1);
    card.set_halign(gtk::Align::Start);
    card.set_tooltip_text(Some(
        "Drag onto the timeline, or double-click to add at the playhead",
    ));

    let texture = match (&path, kind, missing) {
        (Some(p), MediaKind::Video | MediaKind::Image, false) => {
            let existing = app.thumbs.borrow().get(p);
            if existing.is_none() {
                thumbs::request(app, p.clone(), kind);
            }
            existing
        }
        _ => None,
    };
    match texture {
        Some(texture) => {
            let picture = gtk::Picture::for_paintable(&texture);
            picture.set_content_fit(gtk::ContentFit::Contain);
            picture.set_can_shrink(true);
            picture.set_size_request(108, 80);
            card.append(&picture);
        }
        None => {
            let placeholder = gtk::DrawingArea::new();
            placeholder.set_halign(gtk::Align::Start);
            placeholder.set_content_width(108);
            placeholder.set_content_height(80);
            let ext = name.rsplit('.').next().unwrap_or("").to_uppercase();
            let base = match kind {
                MediaKind::Audio => theme::AUDIO_CLIP,
                MediaKind::Video => theme::VIDEO_CLIP,
                MediaKind::Image => (0.35, 0.35, 0.35),
            };
            let bg = Palette::of(dark).canvas_bg;
            let fill = theme::mix(base, bg, 0.35);
            let label = kind.label().to_owned();
            placeholder.set_draw_func(move |_, cr, w, h| {
                let (w, h) = (f64::from(w), f64::from(h));
                cr.set_source_rgb(fill.0, fill.1, fill.2);
                cr.rectangle(0.0, 0.0, w, h);
                let _ = cr.fill();
                cr.set_source_rgb(1.0, 1.0, 1.0);
                cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
                cr.set_font_size(18.0);
                cr.move_to(w / 2.0 - 4.5 * ext.chars().count() as f64, h / 2.0);
                let _ = cr.show_text(&ext);
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.8);
                cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
                cr.set_font_size(11.0);
                cr.move_to(w / 2.0 - 3.0 * label.chars().count() as f64, h / 2.0 + 18.0);
                let _ = cr.show_text(&label);
            });
            card.append(&placeholder);
        }
    }
    let label = gtk::Label::new(Some(name));
    label.add_css_class("caption");
    label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    label.set_max_width_chars(14);
    if missing {
        label.add_css_class("error");
    }
    card.append(&label);
    make_draggable(app, &card, id);
    card.upcast()
}

/// The inspector's tab strips: Video / Audio / Speed / Animate / Adjust, then
/// Basic / Cutout / Mask / Canvas. Only Video > Basic exists so far.
fn inspector_tabs() -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let mut first: Option<gtk::ToggleButton> = None;
    for name in ["Video", "Audio", "Speed", "Animate", "Adjust"] {
        let tab = icons::text_tab(name);
        if let Some(f) = &first {
            tab.set_group(Some(f));
        } else {
            tab.set_active(true);
            first = Some(tab.clone());
        }
        if name != "Video" {
            icons::disable(&tab, name);
        }
        top.append(&tab);
    }
    column.append(&top);
    let sub = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    sub.add_css_class("cc-subtabs");
    let mut first: Option<gtk::ToggleButton> = None;
    for name in ["Basic", "Cutout", "Mask", "Canvas"] {
        let tab = gtk::ToggleButton::with_label(name);
        tab.add_css_class("flat");
        tab.add_css_class("cc-subtab");
        tab.set_hexpand(true);
        if let Some(f) = &first {
            tab.set_group(Some(f));
        } else {
            tab.set_active(true);
            first = Some(tab.clone());
        }
        if name != "Basic" {
            icons::disable(&tab, name);
        }
        sub.append(&tab);
    }
    column.append(&sub);
    column
}

/// A section that is shown but not available yet: greyed out, nothing to click.
fn placeholder_section(title: &str) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    section.add_css_class("cc-section");
    section.add_css_class("disabled-section");
    let check = gtk::CheckButton::new();
    let label = gtk::Label::new(Some(title));
    label.add_css_class("cc-section-title");
    label.set_xalign(0.0);
    label.set_hexpand(true);
    section.append(&check);
    section.append(&label);
    section.append(&icons::button(icons::RESET, "Reset"));
    section.append(&icons::button(icons::KEYFRAME, "Keyframe"));
    icons::disable(&section, title);
    section
}

fn refresh_inspector(app: &Rc<App>) {
    let ui = &app.ui().editor;
    // Taking the focused control out of the window would upset GTK's focus tracking.
    if let Some(focus) = gtk::prelude::GtkWindowExt::focus(&app.window) {
        if focus.is_ancestor(&ui.inspector) || focus.is_ancestor(&ui.info) {
            gtk::prelude::GtkWindowExt::set_focus(&app.window, None::<&gtk::Widget>);
        }
    }
    clear_box(&ui.inspector);
    clear_box(&ui.info);
    {
        // A preview belongs to the clip that was selected when it began.
        let mut st = app.state.borrow_mut();
        let selected = st.session.as_ref().and_then(|s| s.selected);
        if let Some((c, t)) = st.transform_preview {
            if Some(c) != selected {
                // Not yet recorded: record it for the clip it belongs to rather
                // than lose it.
                st.transform_preview = None;
                let all = st.apply_all_captions;
                if let Some(session) = st.session.as_mut() {
                    let targets = session.group_targets(c, all);
                    session.edit_transform_for(c, t, &targets);
                }
            }
        }
    }
    let st = app.state.borrow();
    let Some(session) = st.session.as_ref() else {
        return;
    };
    let rate = session.doc.settings.frame_rate;

    let heading = |text: &str| {
        let l = gtk::Label::new(Some(text));
        l.add_css_class("panel-title");
        l.set_xalign(0.0);
        l
    };
    let grid = |rows: &[(&str, String)]| {
        let g = gtk::Grid::new();
        g.set_column_spacing(12);
        g.set_row_spacing(4);
        for (i, (k, v)) in rows.iter().enumerate() {
            let key = gtk::Label::new(Some(k));
            key.set_xalign(0.0);
            key.add_css_class("dim-label");
            let value = gtk::Label::new(Some(v));
            value.set_xalign(0.0);
            value.set_wrap(true);
            value.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            value.set_max_width_chars(26);
            value.set_hexpand(true);
            g.attach(&key, 0, i as i32, 1, 1);
            g.attach(&value, 1, i as i32, 1, 1);
        }
        g
    };

    if let Some((_, clip)) = session.selected_clip() {
        ui.inspector.append(&inspector_tabs());
        let shared = session.group_targets(clip.id, st.apply_all_captions);
        if !shared.is_empty() {
            let note = gtk::Label::new(Some(&if st.apply_all_captions && clip.is_caption() {
                format!("Changes apply to all {} auto-captions.", shared.len() + 1)
            } else {
                format!(
                    "Editing {} selected clips of the same type: changes apply to all of them.",
                    shared.len() + 1
                )
            }));
            note.set_wrap(true);
            note.set_xalign(0.0);
            note.add_css_class("dim-label");
            note.add_css_class("caption");
            ui.inspector.append(&note);
        }
        let position = |ui: &EditorUi| {
            if session.can_transform(clip.id) {
                ui.inspector.append(&crate::transform_ui::controls(
                    app,
                    clip.id,
                    clip.transform_at(session.playhead),
                ));
            } else {
                ui.inspector.append(&placeholder_section("Position & Size"));
            }
        };
        match clip.text_at(session.playhead) {
            Some(spec) => {
                // Text first, then Position & Size, then Filters and Effects.
                let caption = clip
                    .is_caption()
                    .then(|| clip.caption.clone().unwrap_or_default());
                let (text, captions, effects) =
                    crate::text_ui::sections(app, clip.id, spec, caption, !clip.words.is_empty());
                ui.inspector.append(&text);
                if let Some(captions) = &captions {
                    ui.inspector.append(captions);
                }
                position(ui);
                ui.inspector.append(&placeholder_section("Filters"));
                ui.inspector.append(&effects);
            }
            None => {
                // Pictures: Position & Size always first.
                position(ui);
                ui.inspector.append(&placeholder_section("Filters"));
                ui.inspector.append(&placeholder_section("Effects"));
            }
        }
    } else {
        let hint = gtk::Label::new(Some("Select a clip on the timeline to edit it."));
        hint.set_xalign(0.0);
        hint.set_wrap(true);
        hint.add_css_class("dim-label");
        ui.inspector.append(&hint);
    }
    // Selecting a clip shows its controls; clearing the selection shows the info.
    let selected_now = session.selected;
    if ui.last_selected.get() != Some(selected_now) {
        ui.last_selected.set(Some(selected_now));
        if selected_now.is_some() {
            ui.edit_tab.set_active(true);
        } else {
            ui.info_tab.set_active(true);
        }
    }
    ui.info.append(&heading("Clip details"));
    match session.selected_clip() {
        None => {
            let l = gtk::Label::new(Some("Nothing selected."));
            l.set_xalign(0.0);
            l.add_css_class("dim-label");
            ui.info.append(&l);
        }
        Some((track, clip)) => {
            let name = if clip.text.is_some() {
                "Text (drawn by the app)".to_string()
            } else {
                session
                    .doc
                    .media_item(clip.media)
                    .map(media_label)
                    .unwrap_or_else(|_| "missing media".into())
            };
            ui.info.append(&grid(&[
                ("File", name),
                ("Track", track.name.clone()),
                ("Start", view::timecode(clip.start, rate)),
                ("Duration", view::timecode(clip.duration, rate)),
                ("End", view::timecode(clip.end(), rate)),
                ("Source in", view::timecode(clip.source_in, rate)),
            ]));
            if clip.text.is_none() && session.known_media_duration(clip.media).is_none() {
                let note = gtk::Label::new(Some(
                    "This file's length hasn't been read yet, so the clip is a 5 s placeholder that can be freely extended.",
                ));
                note.set_wrap(true);
                note.set_xalign(0.0);
                note.add_css_class("dim-label");
                note.add_css_class("caption");
                ui.info.append(&note);
            }
        }
    }

    ui.info
        .append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    ui.info.append(&heading("Project"));
    let settings = &session.doc.settings;
    ui.info.append(&grid(&[
        (
            "Canvas",
            format!("{} x {}", settings.width, settings.height),
        ),
        ("Frame rate", format!("{:.3} fps", rate.as_f64())),
        ("Audio", format!("{} Hz", settings.sample_rate)),
        ("Folder", session.folder.root().display().to_string()),
    ]));

    for (label, which, dir) in [
        (
            "Proxy files",
            StorageKind::Proxy,
            session.folder.proxy_dir(&session.doc),
        ),
        (
            "Render cache",
            StorageKind::Cache,
            session.folder.cache_dir(&session.doc),
        ),
    ] {
        let caption = gtk::Label::new(Some(label));
        caption.set_xalign(0.0);
        caption.add_css_class("dim-label");
        ui.info.append(&caption);
        let path = gtk::Label::new(Some(&dir.display().to_string()));
        path.set_xalign(0.0);
        path.set_wrap(true);
        path.add_css_class("caption");
        path.set_selectable(true);
        ui.info.append(&path);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let change = gtk::Button::with_label("Change...");
        let reset = gtk::Button::with_label("Use project folder");
        {
            let app = app.clone();
            change.connect_clicked(move |_| {
                let app2 = app.clone();
                app.choose_folder("Choose a folder", move |dir| {
                    app2.set_storage_dir(which, Some(dir))
                });
            });
        }
        {
            let app = app.clone();
            reset.connect_clicked(move |_| app.set_storage_dir(which, None));
        }
        row.append(&change);
        row.append(&reset);
        ui.info.append(&row);
    }

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let collect = gtk::Button::with_label("Collect media");
    collect.set_tooltip_text(Some("Copy all linked media into the project folder"));
    {
        let app = app.clone();
        collect.connect_clicked(move |_| {
            let message = {
                let mut st = app.state.borrow_mut();
                st.session.as_mut().map(|s| match s.collect_media() {
                    Ok(n) => format!("Collected {n} file(s) into the project"),
                    Err(e) => format!("Collect failed: {e}"),
                })
            };
            if let Some(m) = message {
                app.set_status(m);
            }
            app.refresh();
        });
    }
    let check = gtk::Button::with_label("Check media");
    {
        let app = app.clone();
        check.connect_clicked(move |_| {
            let message = {
                let mut st = app.state.borrow_mut();
                st.session.as_mut().map(|s| {
                    s.refresh_missing();
                    format!("{} file(s) missing", s.missing.len())
                })
            };
            if let Some(m) = message {
                app.set_status(m);
            }
            app.refresh();
        });
    }
    actions.append(&collect);
    actions.append(&check);
    ui.info.append(&actions);
    let _ = ui.timeline_scroll.hadjustment();
    let _ = Session::title;
}
