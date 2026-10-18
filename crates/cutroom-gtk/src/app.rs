//! The application object: state, actions, the project lifecycle, importing,
//! autosave, and the timer that drives playback.

use crate::media;
use crate::preview::Preview;
use crate::thumbs::Thumbs;
use crate::{captions_ui, dialogs, editor, home, menu, theme};
use cutroom_core::geom::Pos2;
use cutroom_core::home_logic::SortKey;
use cutroom_core::new_project::NewProjectForm;
use cutroom_core::paths;
use cutroom_core::settings::{Settings, ThemeChoice};
use cutroom_core::shortcuts::{self, Action};
use cutroom_core::state::{collect_media_files, is_supported_media, ClipCopy, Session};
use cutroom_core::timeline_logic::{Drag, DropTarget, Tool};
use cutroom_core::view::TimelineView;
use cutroom_platform::AppPaths;
use cutroom_project::{
    read_entry, scan_projects, sort_newest_first, ImportMode, ProjectEntry, PROJECT_FILE,
};
use cutroom_timeline::{MediaId, Ticks, TrackId};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};
use std::cell::{OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct HomeState {
    pub entries: Vec<ProjectEntry>,
    pub query: String,
    pub sort: SortKey,
    pub selected: Option<PathBuf>,
}

pub struct State {
    pub settings: Settings,
    pub config_dir: Option<PathBuf>,
    pub default_projects_dir: PathBuf,
    pub dark: bool,
    pub session: Option<Session>,
    pub view: TimelineView,
    pub tool: Tool,
    pub snap: bool,
    pub drag: Drag,
    pub snap_guide: Option<Ticks>,
    pub drop_ghost: Option<DropTarget>,
    pub hover: Option<Pos2>,
    /// A media item being dragged from the library right now.
    pub dragging_media: Option<MediaId>,
    /// The library item highlighted in the media panel.
    pub selected_media: Option<MediaId>,
    /// A transform being adjusted with a slider, shown before it is committed.
    pub transform_preview: Option<(cutroom_timeline::ClipId, cutroom_timeline::Transform)>,
    /// Which handles the preview shows over each clip (box, corner pin or mesh).
    pub distort_modes:
        std::collections::HashMap<cutroom_timeline::ClipId, crate::transform_overlay::Distort>,
    /// "Apply to all captions" in the Auto-Captions section: while on, changes
    /// made to one generated caption go to all of them.
    pub apply_all_captions: bool,
    /// The open right-click menu of the media panel.
    pub library_popover: Option<gtk::Popover>,
    /// The open right-click menu of a timeline clip.
    pub clip_popover: Option<gtk::Popover>,
    pub timeline_width: f64,
    pub notice: Option<String>,
    pub home: HomeState,
    pub category: usize,
    pub quitting: bool,
    pub last_tick: Instant,
    pub last_synced_playhead: Ticks,
    pub tick_count: u32,
    /// The UI state last written to the project, to save only on change.
    pub ui_saved: Option<cutroom_core::ui_state::UiState>,
    /// The part of the timeline being picked for export (Shift+Ctrl+X), if any.
    pub export_range: Option<crate::range_select::ExportRange>,
    /// Text styles the user has saved (shared by every project).
    pub text_styles: cutroom_core::text_styles::StyleBook,
}

impl State {
    pub fn projects_dir(&self) -> PathBuf {
        self.settings.projects_dir_or(&self.default_projects_dir)
    }

    pub fn save_text_styles(&mut self) {
        if let Some(dir) = &self.config_dir {
            if let Err(e) = self.text_styles.save(dir) {
                self.notice = Some(format!("Could not save text styles: {e}"));
            }
        }
    }

    pub fn save_settings(&mut self) {
        if let Some(dir) = &self.config_dir {
            if let Err(e) = self.settings.save(dir) {
                self.notice = Some(format!("Could not save settings: {e}"));
            }
        }
    }
}

pub struct Ui {
    pub stack: gtk::Stack,
    pub status: gtk::Label,
    pub autosave: gtk::Label,
    pub home: home::HomeUi,
    pub editor: editor::EditorUi,
    pub menu: menu::MenuUi,
    pub drop_overlay: gtk::Label,
    /// Shortcuts and Export, at the right end of the menu bar while editing.
    pub editor_actions: gtk::Box,
}

pub struct App {
    pub window: gtk::ApplicationWindow,
    pub state: RefCell<State>,
    pub ui: OnceCell<Ui>,
    pub preview: RefCell<Preview>,
    pub thumbs: RefCell<Thumbs>,
    /// The clip taken by Copy or Cut, pasted by Ctrl+V while it is still
    /// the newest thing on the system clipboard.
    pub clip_clipboard: RefCell<Option<Vec<ClipCopy>>>,
    /// Waveforms and filmstrips for the timeline.
    pub analysis: RefCell<crate::strips::Analysis>,
    /// Media whose proxy is being made right now.
    pub proxy_jobs: RefCell<std::collections::HashSet<MediaId>>,
}

impl App {
    pub fn ui(&self) -> &Ui {
        self.ui.get().expect("the UI is built before use")
    }

    pub fn new(gtk_app: &gtk::Application, startup_files: Vec<PathBuf>) -> Rc<App> {
        let paths = AppPaths::detect();
        let config_dir = paths.as_ref().map(|p| p.config_dir.clone());
        let default_projects_dir = paths
            .map(|p| p.suggested_projects_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let settings = config_dir
            .as_deref()
            .map(Settings::load)
            .unwrap_or_default();
        let text_styles = config_dir
            .as_deref()
            .map(cutroom_core::text_styles::StyleBook::load)
            .unwrap_or_default();
        let dark = theme::apply(settings.theme);
        theme::load_css();

        let window = gtk::ApplicationWindow::builder()
            .application(gtk_app)
            .title("Kani")
            .default_width(1440)
            .default_height(860)
            .build();
        theme::mark_window(&window, dark);

        let app = Rc::new(App {
            window,
            state: RefCell::new(State {
                settings,
                config_dir,
                default_projects_dir,
                dark,
                session: None,
                view: TimelineView::default(),
                tool: Tool::Select,
                snap: true,
                drag: Drag::None,
                snap_guide: None,
                drop_ghost: None,
                hover: None,
                dragging_media: None,
                selected_media: None,
                transform_preview: None,
                distort_modes: Default::default(),
                apply_all_captions: false,
                library_popover: None,
                clip_popover: None,
                timeline_width: 800.0,
                notice: None,
                home: HomeState::default(),
                category: 0,
                quitting: false,
                last_tick: Instant::now(),
                last_synced_playhead: Ticks::ZERO,
                tick_count: 0,
                ui_saved: None,
                export_range: None,
                text_styles,
            }),
            ui: OnceCell::new(),
            preview: RefCell::new(Preview::new()),
            thumbs: RefCell::new(Thumbs::default()),
            clip_clipboard: RefCell::new(None),
            analysis: RefCell::new(Default::default()),
            proxy_jobs: RefCell::new(Default::default()),
        });

        app.apply_playback_settings();
        let ui = build_ui(&app);
        let _ = app.ui.set(ui);
        install_controllers(&app);
        captions_ui::connect(&app, &app.ui().editor.captions);
        start_timer(&app);

        app.refresh_home_entries();
        app.refresh();
        app.window.present();
        if !startup_files.is_empty() {
            app.import_startup_files(startup_files);
        }
        app
    }

    // ----- settings and appearance -----------------------------------------

    pub fn set_theme(self: &Rc<Self>, choice: ThemeChoice) {
        let dark = theme::apply(choice);
        theme::mark_window(&self.window, dark);
        {
            let mut st = self.state.borrow_mut();
            st.settings.theme = choice;
            st.dark = dark;
            st.save_settings();
        }
        self.refresh();
    }

    /// Make the playback choices real: which decoders may be used, how sharp
    /// the preview is, where it is put together, and scrub sound.
    pub fn apply_playback_settings(self: &Rc<Self>) {
        let (hw, edge, gpu, scrub) = {
            let st = self.state.borrow();
            let s = &st.settings;
            (
                s.hw_decode,
                s.preview_edge(),
                s.preview_on_gpu,
                s.scrub_audio,
            )
        };
        crate::hwdecode::apply(hw);
        self.preview.borrow_mut().configure(edge, scrub, gpu);
    }

    pub fn change_projects_dir(self: &Rc<Self>, dir: Option<PathBuf>) {
        {
            let mut st = self.state.borrow_mut();
            st.settings.projects_dir = dir;
            st.save_settings();
        }
        self.refresh_home_entries();
        self.refresh();
    }

    // ----- status -----------------------------------------------------------

    pub fn set_status(&self, message: impl Into<String>) {
        let message = message.into();
        let mut st = self.state.borrow_mut();
        match st.session.as_mut() {
            Some(s) => s.status = Some(message),
            None => st.notice = Some(message),
        }
    }

    pub fn update_status(&self) {
        let st = self.state.borrow();
        let ui = self.ui();
        let message = st
            .session
            .as_ref()
            .and_then(|s| s.status.clone())
            .or_else(|| st.notice.clone())
            .or_else(|| self.preview.borrow().problem.clone())
            .unwrap_or_else(|| "Ready".to_owned());
        ui.status.set_text(&message);
        let note = st.session.as_ref().and_then(|s| {
            if s.autosave_running() {
                Some("Autosaving...".to_owned())
            } else {
                s.autosave_note()
            }
        });
        ui.autosave.set_text(note.as_deref().unwrap_or(""));
    }

    fn update_title(&self) {
        let title = self
            .state
            .borrow()
            .session
            .as_ref()
            .map_or_else(|| "Kani".to_owned(), |s| s.title());
        self.window.set_title(Some(&title));
    }

    // ----- refreshing -------------------------------------------------------

    pub fn refresh(self: &Rc<Self>) {
        self.update_title();
        let has_session = self.state.borrow().session.is_some();
        let ui = self.ui();
        ui.stack
            .set_visible_child_name(if has_session { "editor" } else { "home" });
        ui.editor_actions.set_visible(has_session);
        if has_session {
            editor::refresh(self);
            self.sync_preview(true);
        } else {
            home::refresh(self);
        }
        menu::update(self);
        self.update_status();
    }

    pub fn refresh_home_entries(&self) {
        let mut st = self.state.borrow_mut();
        let dir = st.projects_dir();
        let mut entries = scan_projects(&dir);
        let recents = st.settings.recent_projects.clone();
        for path in recents {
            let known = entries.iter().any(|e| e.path() == path);
            let inside = path.parent() == Some(dir.as_path());
            if !known && !inside && path.join(PROJECT_FILE).is_file() {
                entries.push(read_entry(&path));
            }
        }
        sort_newest_first(&mut entries);
        if let Some(selected) = &st.home.selected {
            if !entries.iter().any(|e| e.path() == selected) {
                st.home.selected = None;
            }
        }
        st.home.entries = entries;
    }

    /// Point the preview players at what is under the playhead.
    pub fn sync_preview(&self, moved: bool) {
        let st = self.state.borrow();
        let Some(session) = st.session.as_ref() else {
            return;
        };
        let use_proxies = st.settings.use_proxies;
        let resolve = |id: MediaId| {
            session
                .preview_path(id, use_proxies)
                .map(|path| (path, session.media_kind_of(id)))
        };
        self.preview.borrow_mut().sync(
            &session.doc.timeline,
            &resolve,
            session.playhead,
            session.playing,
            moved,
        );
        drop(st);
        self.render_preview();
    }

    /// Stack the pictures under the playhead and show the result (only
    /// redraws when something changed).
    pub fn render_preview(&self) {
        let (canvas, preview_transform) = {
            let st = self.state.borrow();
            let Some(session) = st.session.as_ref() else {
                return;
            };
            (
                (session.doc.settings.width, session.doc.settings.height),
                st.transform_preview,
            )
        };
        let texture = self.preview.borrow_mut().render(canvas, preview_transform);
        if let Some(texture) = texture {
            self.ui().editor.set_preview_texture(&texture);
        }
    }

    // ----- project lifecycle -----------------------------------------------

    pub fn open_project(self: &Rc<Self>, dir: &Path) {
        match Session::open(dir) {
            Ok(session) => {
                self.remember_if_outside(dir);
                self.install_session(session);
            }
            Err(e) => {
                self.set_status(format!("Could not open project: {e}"));
                self.refresh();
            }
        }
    }

    pub fn create_project(self: &Rc<Self>, form: &NewProjectForm) -> Result<(), String> {
        let root = form.project_root();
        let session = Session::create(&root, form.name.trim(), form.settings())
            .map_err(|e| format!("Could not create project: {e}"))?;
        self.remember_if_outside(&root);
        self.install_session(session);
        Ok(())
    }

    fn remember_if_outside(&self, dir: &Path) {
        let abs = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
        let mut st = self.state.borrow_mut();
        if abs.parent() != Some(st.projects_dir().as_path()) {
            st.settings.remember_project(&abs);
            st.save_settings();
        }
    }

    fn install_session(self: &Rc<Self>, session: Session) {
        let saved = cutroom_core::ui_state::UiState::load(session.folder.root());
        {
            let mut st = self.state.borrow_mut();
            st.session = Some(session);
            st.view = TimelineView::default();
            st.drag = Drag::None;
            st.tool = Tool::Select;
            st.snap_guide = None;
            st.drop_ghost = None;
            st.notice = None;
            st.last_synced_playhead = Ticks::ZERO;
            st.ui_saved = saved.clone();
            if let Some(ui) = &saved {
                st.view = TimelineView {
                    pixels_per_second: ui.pixels_per_second.clamp(
                        cutroom_core::view::MIN_PIXELS_PER_SECOND,
                        cutroom_core::view::MAX_PIXELS_PER_SECOND,
                    ),
                    scroll_seconds: ui.scroll_seconds.max(0.0),
                };
                st.tool = if ui.tool == "split" {
                    Tool::Split
                } else {
                    Tool::Select
                };
                st.snap = ui.snap;
                st.category = ui.category.min(crate::editor::CATEGORY_COUNT - 1);
                if let Some(s) = st.session.as_mut() {
                    s.set_playhead(Ticks(ui.playhead_ticks.max(0)));
                    let ids: Vec<_> = ui
                        .selection
                        .iter()
                        .map(|i| cutroom_timeline::ClipId(*i))
                        .collect();
                    s.select_many(&ids);
                }
            }
        }
        self.preview.borrow_mut().problem = None;
        self.preview.borrow_mut().invalidate();
        *self.analysis.borrow_mut() = Default::default();
        self.refresh();
        // Everything already in the project may still need probing and thumbnails.
        self.probe_unprobed_media();
        crate::strips::request_all(self);
        if let Some(ui) = saved {
            let editor = &self.ui().editor;
            editor.set_pane_positions(ui.panes);
            let app = self.clone();
            glib::idle_add_local_once(move || {
                app.sync_preview(true);
                app.refresh();
                // The scrollable height only exists once the tracks are laid out.
                let app = app.clone();
                glib::idle_add_local_once(move || {
                    app.ui().editor.set_track_scroll(ui.track_scroll);
                });
            });
        }
    }

    /// What the editor looks like right now, for saving with the project.
    fn capture_ui_state(&self) -> Option<cutroom_core::ui_state::UiState> {
        let st = self.state.borrow();
        let session = st.session.as_ref()?;
        let editor = &self.ui().editor;
        Some(cutroom_core::ui_state::UiState {
            pixels_per_second: st.view.pixels_per_second,
            scroll_seconds: st.view.scroll_seconds,
            track_scroll: editor.track_scroll(),
            playhead_ticks: session.playhead.0,
            tool: if st.tool == Tool::Split {
                "split"
            } else {
                "select"
            }
            .into(),
            snap: st.snap,
            selection: session.selection().iter().map(|c| c.0).collect(),
            category: st.category,
            panes: editor.pane_positions(),
        })
    }

    /// Write the editor's layout into the project folder if it changed.
    pub fn save_ui_state(&self) {
        let Some(now) = self.capture_ui_state() else {
            return;
        };
        let mut st = self.state.borrow_mut();
        if st.ui_saved.as_ref() == Some(&now) {
            return;
        }
        if let Some(session) = st.session.as_ref() {
            now.save(session.folder.root());
        }
        st.ui_saved = Some(now);
    }

    /// Save, close the project and return to the home screen. If saving fails
    /// the project stays open so no work is lost.
    pub fn close_project(self: &Rc<Self>) {
        self.save_ui_state();
        let result = {
            let mut st = self.state.borrow_mut();
            match st.session.as_mut() {
                Some(s) => s.close().map_err(|e| e.to_string()),
                None => return,
            }
        };
        match result {
            Ok(()) => {
                self.preview.borrow_mut().pause();
                {
                    let mut st = self.state.borrow_mut();
                    st.session = None;
                    st.drag = Drag::None;
                }
                crate::clip_menu::dismiss(self);
                *self.analysis.borrow_mut() = Default::default();
                self.refresh_home_entries();
                self.refresh();
            }
            Err(e) => {
                self.set_status(format!("Couldn't save before closing: {e}"));
                self.refresh();
            }
        }
    }

    pub fn quit(self: &Rc<Self>) {
        self.save_ui_state();
        let result = {
            let mut st = self.state.borrow_mut();
            st.quitting = true;
            match st.session.as_mut() {
                Some(s) => s.close().map_err(|e| e.to_string()),
                None => Ok(()),
            }
        };
        match result {
            Ok(()) => {
                self.preview.borrow_mut().shutdown();
                self.window.close();
            }
            Err(e) => {
                self.state.borrow_mut().quitting = false;
                self.set_status(format!("Couldn't save before quitting: {e}"));
                self.refresh();
            }
        }
    }

    // ----- importing ---------------------------------------------------------

    /// Import files or folders. `place_at` puts the first imported item on
    /// that track and time (used when dropping onto the timeline).
    pub fn import_paths(self: &Rc<Self>, input: Vec<PathBuf>, place_at: Option<(TrackId, Ticks)>) {
        // Files in containers Kani cannot read are re-wrapped first (when asked).
        let input = if self.state.borrow().settings.remux_on_import {
            let (wrap, rest): (Vec<_>, Vec<_>) = input
                .into_iter()
                .partition(|p| cutroom_core::state::is_remuxable(p));
            if !wrap.is_empty() {
                self.remux_files(wrap);
                if rest.is_empty() {
                    return;
                }
            }
            rest
        } else {
            input
        };
        let (files, skipped) = expand_media_inputs(input);
        let add_to_timeline = self.state.borrow().settings.add_dropped_to_timeline;
        let message;
        let imported: Vec<(MediaId, PathBuf)>;
        {
            let mut st = self.state.borrow_mut();
            let Some(session) = st.session.as_mut() else {
                st.notice = Some("Open a project first, then add the files again.".into());
                drop(st);
                self.refresh();
                return;
            };
            if files.is_empty() {
                session.status = Some(if skipped > 0 {
                    format!("{skipped} file(s) skipped: not a supported media type")
                } else {
                    "No supported media found".into()
                });
                imported = Vec::new();
                message = None;
            } else {
                let ids = session.import_files(&files);
                imported = ids
                    .iter()
                    .filter_map(|id| session.media_path(*id).map(|p| (*id, p)))
                    .collect();
                let mut first = true;
                for id in &ids {
                    let kind_fits = |session: &Session, track: TrackId| {
                        session
                            .doc
                            .timeline
                            .track(track)
                            .is_some_and(|t| session.can_place_on(*id, t.kind))
                    };
                    if let (true, Some((track, start))) = (first, place_at) {
                        if kind_fits(session, track) {
                            session.add_media_at(*id, track, start);
                        } else {
                            session.add_media_to_timeline(*id);
                        }
                    } else if add_to_timeline || place_at.is_some() {
                        session.add_media_to_timeline(*id);
                    }
                    first = false;
                }
                message = (ids.len() == files.len()).then(|| {
                    let mut m = format!("Imported {} file(s)", ids.len());
                    if add_to_timeline || place_at.is_some() {
                        m.push_str(" and added them to the timeline");
                    }
                    if skipped > 0 {
                        m.push_str(&format!(", skipped {skipped} unsupported"));
                    }
                    m
                });
            }
        }
        if let Some(message) = message {
            self.set_status(message);
        }
        for (id, path) in imported {
            self.spawn_probe(id, path);
        }
        self.refresh();
    }

    /// Re-wrap files into Matroska inside the project's media folder (no
    /// re-encoding), then import the result.
    fn remux_files(self: &Rc<Self>, files: Vec<PathBuf>) {
        if !crate::analysis::remux_supported() {
            self.set_status(
                "Remuxing needs the GStreamer Matroska plugins, which are missing here.",
            );
            self.refresh();
            return;
        }
        let dir = match self.state.borrow().session.as_ref() {
            Some(s) => s.folder.media_dir(),
            None => {
                self.set_status("Open a project first, then add the files again.");
                self.refresh();
                return;
            }
        };
        let app = self.clone();
        glib::spawn_future_local(async move {
            let total = files.len();
            for (n, src) in files.into_iter().enumerate() {
                let name = src
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                app.set_status(format!("Remuxing {name} ({} of {total})...", n + 1));
                app.refresh();
                let stem = src
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "video".into());
                let mut dest = dir.join(format!("{stem}.mkv"));
                let mut count = 2;
                while dest.exists() {
                    dest = dir.join(format!("{stem} ({count}).mkv"));
                    count += 1;
                }
                let (from, to) = (src.clone(), dest.clone());
                let result =
                    gio::spawn_blocking(move || crate::analysis::remux(&from, &to, &|_| {})).await;
                match result {
                    Ok(Ok(())) => app.import_paths(vec![dest], None),
                    Ok(Err(e)) => {
                        app.set_status(format!("Could not remux {name}: {e}"));
                        app.refresh();
                    }
                    Err(_) => {
                        app.set_status(format!("Could not remux {name}"));
                        app.refresh();
                    }
                }
            }
        });
    }

    /// Files dropped onto the timeline: placed where they were dropped.
    pub fn handle_timeline_file_drop(
        self: &Rc<Self>,
        files: Vec<PathBuf>,
        place_at: Option<(TrackId, Ticks)>,
    ) {
        if self.state.borrow().session.is_none() {
            self.handle_window_drop(files);
        } else {
            self.import_paths(files, place_at);
        }
    }

    /// Files given on the command line (or via "Open with").
    pub fn import_startup_files(self: &Rc<Self>, files: Vec<PathBuf>) {
        let has_session = self.state.borrow().session.is_some();
        if !has_session && files.len() == 1 && files[0].join(PROJECT_FILE).is_file() {
            self.open_project(&files[0].clone());
        } else if has_session {
            self.import_paths(files, None);
        } else {
            self.set_status(format!(
                "{} file(s) were given on the command line. Open a project, then drop or import them.",
                files.len()
            ));
            self.refresh();
        }
    }

    /// Files dropped onto the window (no timeline position known).
    pub fn handle_window_drop(self: &Rc<Self>, files: Vec<PathBuf>) {
        let has_session = self.state.borrow().session.is_some();
        if !has_session {
            if files.len() == 1 && files[0].join(PROJECT_FILE).is_file() {
                self.open_project(&files[0].clone());
            } else {
                self.set_status("Open a project first, then drop the files again.");
                self.refresh();
            }
            return;
        }
        self.import_paths(files, None);
    }

    /// Read the clipboard (the app's own clip, files, a picture or paths as
    /// text) and paste it at the playhead.
    pub fn paste_from_clipboard(self: &Rc<Self>) {
        let app = self.clone();
        glib::spawn_future_local(async move {
            let clipboard = app.window.clipboard();
            let internal = app.clip_clipboard.borrow().is_some();
            let read = read_clipboard(&clipboard, internal);
            match glib::future_with_timeout(Duration::from_secs(4), read).await {
                Ok(pasted) => app.apply_paste(pasted),
                Err(_) => {
                    app.set_status("Timed out reading the clipboard.");
                    app.refresh();
                }
            }
        });
    }

    pub(crate) fn apply_paste(self: &Rc<Self>, pasted: Pasted) {
        if self.state.borrow().session.is_none() {
            self.set_status("Open a project first, then paste.");
            self.refresh();
            return;
        }
        match pasted {
            Pasted::Clip => {
                let copy = self.clip_clipboard.borrow().clone();
                if let Some(copy) = copy {
                    {
                        let mut st = self.state.borrow_mut();
                        if let Some(session) = st.session.as_mut() {
                            let n = session.paste_group(&copy).len();
                            if n > 0 {
                                session.status = Some(if n == 1 {
                                    "Pasted the clip at the playhead".into()
                                } else {
                                    format!("Pasted {n} clips at the playhead")
                                });
                            }
                        }
                    }
                    self.sync_preview(true);
                    self.refresh();
                }
            }
            Pasted::Files(files) => self.paste_files(files, None),
            Pasted::Image(texture) => match self.save_pasted_image(&texture) {
                // The picture is saved inside the project, so it is linked, not copied again.
                Some(path) => self.paste_files(vec![path], Some(ImportMode::Reference)),
                None => {
                    self.set_status("Could not save the pasted picture into the project.");
                    self.refresh();
                }
            },
            Pasted::Nothing(formats) => {
                self.set_status(format!(
                    "Nothing to paste. The clipboard offers: {}",
                    if formats.is_empty() {
                        "(nothing)"
                    } else {
                        &formats
                    }
                ));
                self.refresh();
            }
        }
    }

    /// Paste files: import them and place them at the playhead (one after
    /// another). Where the playhead rests on a clip, each goes on a new track.
    pub fn paste_files(self: &Rc<Self>, input: Vec<PathBuf>, mode: Option<ImportMode>) {
        let (files, skipped) = expand_media_inputs(input);
        let mut to_probe: Vec<(MediaId, PathBuf)> = Vec::new();
        {
            let mut st = self.state.borrow_mut();
            let Some(session) = st.session.as_mut() else {
                return;
            };
            if files.is_empty() {
                session.status = Some(if skipped > 0 {
                    format!("{skipped} file(s) skipped: not a supported media type")
                } else {
                    "No supported media found on the clipboard".into()
                });
            } else {
                let previous_mode = session.import_mode;
                if let Some(mode) = mode {
                    session.import_mode = mode;
                }
                let ids = session.import_files(&files);
                session.import_mode = previous_mode;
                let original = session.playhead;
                let mut at = original;
                for id in &ids {
                    session.set_playhead(at);
                    if let Some(clip) = session.paste_media(*id) {
                        if let Some((_, c)) = session.doc.timeline.find_clip(clip) {
                            at = c.end();
                        }
                    }
                }
                session.set_playhead(original);
                to_probe = ids
                    .iter()
                    .filter_map(|id| session.media_path(*id).map(|p| (*id, p)))
                    .collect();
                session.status = Some(format!("Pasted {} file(s) at the playhead", ids.len()));
            }
        }
        for (id, path) in to_probe {
            self.spawn_probe(id, path);
        }
        self.sync_preview(true);
        self.refresh();
    }

    /// Save a pasted picture (for example "Copy image" in a browser) into the
    /// project's media folder and return its path.
    fn save_pasted_image(&self, texture: &gdk::Texture) -> Option<PathBuf> {
        let st = self.state.borrow();
        let dir = st.session.as_ref()?.folder.media_dir();
        std::fs::create_dir_all(&dir).ok()?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        for n in 0..1000 {
            let name = if n == 0 {
                format!("pasted-image-{stamp}.png")
            } else {
                format!("pasted-image-{stamp}-{n}.png")
            };
            let path = dir.join(name);
            if !path.exists() {
                return texture.save_to_png(&path).ok().map(|_| path);
            }
        }
        None
    }

    /// Ctrl+C / Ctrl+X on the selected clip.
    fn selected_id(&self) -> Option<cutroom_timeline::ClipId> {
        self.state
            .borrow()
            .session
            .as_ref()
            .and_then(|s| s.selected)
    }

    /// Remember a clip's attributes to paste on clips of the same type.
    pub fn copy_attributes_of(self: &Rc<Self>, clip: cutroom_timeline::ClipId) {
        let copied = self
            .state
            .borrow_mut()
            .session
            .as_mut()
            .is_some_and(|s| s.copy_attributes(clip));
        self.set_status(if copied {
            "Copied attributes: paste them on clips of the same type"
        } else {
            "Nothing to copy"
        });
    }

    /// Paste the copied attributes on `clip` (and the clips selected with it).
    pub fn paste_attributes_on(self: &Rc<Self>, clip: cutroom_timeline::ClipId) {
        let n = self
            .state
            .borrow_mut()
            .session
            .as_mut()
            .map_or(0, |s| s.paste_attributes(clip));
        self.set_status(&if n == 0 {
            "Nothing pasted: attributes only go to clips of the same type".to_string()
        } else {
            format!("Pasted attributes on {n} clip(s)")
        });
        self.sync_preview(false);
        self.refresh();
    }

    fn copy_selected_clip(self: &Rc<Self>, cut: bool) {
        let copy = {
            let st = self.state.borrow();
            st.session
                .as_ref()
                .map(|s| s.copy_selection())
                .filter(|c| !c.is_empty())
        };
        match copy {
            Some(copy) => {
                *self.clip_clipboard.borrow_mut() = Some(copy);
                // Anything copied elsewhere later replaces this marker, which is
                // how a paste knows the clip is still the newest thing.
                self.window.clipboard().set_text(CLIP_MARKER);
                let n = self.clip_clipboard.borrow().as_ref().map_or(0, |c| c.len());
                let what = if n == 1 {
                    "the clip".to_string()
                } else {
                    format!("{n} clips")
                };
                let mut st = self.state.borrow_mut();
                if let Some(session) = st.session.as_mut() {
                    if cut {
                        session.delete_selected(false);
                    }
                    session.status = Some(format!(
                        "{} {what}. Move the playhead and press Ctrl+V.",
                        if cut { "Cut" } else { "Copied" }
                    ));
                }
            }
            None => self.set_status("Select a clip first."),
        }
        self.sync_preview(true);
        self.refresh();
    }

    /// Make a low-resolution proxy of a video in the background.
    pub fn create_proxy(self: &Rc<Self>, id: MediaId) {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;
        if !crate::analysis::proxies_supported() {
            self.set_status(
                "Proxies need the GStreamer VP8, Vorbis and WebM plugins, which are missing here.",
            );
            self.refresh();
            return;
        }
        if self.proxy_jobs.borrow().contains(&id) {
            return;
        }
        let job = {
            let st = self.state.borrow();
            let Some(session) = st.session.as_ref() else {
                return;
            };
            let probe = session
                .doc
                .media_item(id)
                .ok()
                .and_then(|m| m.probe.clone());
            match (
                session.can_have_proxy(id),
                session.media_path(id),
                session.proxy_path(id),
                probe,
            ) {
                (true, Some(src), Some(dest), Some(probe)) if src.is_file() => Some((
                    src,
                    dest,
                    probe.width,
                    probe.height,
                    session
                        .doc
                        .media_item(id)
                        .map(cutroom_core::state::media_label)
                        .unwrap_or_default(),
                    session.folder.root().to_path_buf(),
                )),
                _ => None,
            }
        };
        let Some((src, dest, width, height, name, root)) = job else {
            self.set_status(
                "Only videos that have been read, and whose file is present, can have a proxy.",
            );
            self.refresh();
            return;
        };
        let (edge, kbps) = self.state.borrow().settings.proxy_spec();
        let (w, h) = crate::analysis::proxy_size(width, height, edge);
        self.proxy_jobs.borrow_mut().insert(id);
        let percent = Arc::new(AtomicU32::new(0));
        {
            let (app, percent, name) = (self.clone(), percent.clone(), name.clone());
            glib::timeout_add_local(Duration::from_millis(500), move || {
                if !app.proxy_jobs.borrow().contains(&id) {
                    return glib::ControlFlow::Break;
                }
                app.set_status(format!(
                    "Creating a proxy for {name}... {}%",
                    percent.load(Ordering::Relaxed)
                ));
                glib::ControlFlow::Continue
            });
        }
        self.set_status(format!("Creating a proxy for {name}..."));
        self.refresh();
        let app = self.clone();
        glib::spawn_future_local(async move {
            let counter = percent.clone();
            let result = gio::spawn_blocking(move || {
                crate::analysis::make_proxy(&src, &dest, w, h, kbps, &|p| {
                    counter.store((p * 100.0) as u32, Ordering::Relaxed)
                })
            })
            .await;
            app.proxy_jobs.borrow_mut().remove(&id);
            let still_open = app
                .state
                .borrow()
                .session
                .as_ref()
                .is_some_and(|s| s.folder.root() == root);
            let message = match result {
                Ok(Ok(())) => format!("Proxy ready for {name}"),
                Ok(Err(error)) => format!("Could not make a proxy for {name}: {error}"),
                Err(_) => format!("Could not make a proxy for {name}"),
            };
            if still_open {
                app.set_status(message);
                app.sync_preview(true);
                app.refresh();
            }
        });
    }

    /// Delete a proxy file; the preview goes back to the original.
    pub fn delete_proxy(self: &Rc<Self>, id: MediaId) {
        let proxy = self
            .state
            .borrow()
            .session
            .as_ref()
            .and_then(|s| s.existing_proxy(id));
        if let Some(proxy) = proxy {
            match std::fs::remove_file(&proxy) {
                Ok(()) => self.set_status("Deleted the proxy"),
                Err(error) => self.set_status(format!("Could not delete the proxy: {error}")),
            }
            self.sync_preview(true);
            self.refresh();
        }
    }

    /// Read a (re)linked file again.
    pub fn reprobe(self: &Rc<Self>, id: MediaId) {
        self.analysis.borrow_mut().forget(id);
        let path = self
            .state
            .borrow()
            .session
            .as_ref()
            .and_then(|s| s.media_path(id));
        if let Some(path) = path {
            self.spawn_probe(id, path);
        }
    }

    fn spawn_probe(self: &Rc<Self>, id: MediaId, path: PathBuf) {
        let root = match self.state.borrow().session.as_ref() {
            Some(s) => s.folder.root().to_path_buf(),
            None => return,
        };
        let app = self.clone();
        glib::spawn_future_local(async move {
            let probed = gio::spawn_blocking(move || media::probe_file(&path)).await;
            let Ok(Ok(result)) = probed else { return };
            {
                let mut st = app.state.borrow_mut();
                match st.session.as_mut() {
                    Some(s) if s.folder.root() == root => {
                        s.apply_probe(id, result.duration, result.probe)
                    }
                    _ => return,
                }
            }
            app.refresh();
            crate::strips::request(&app, id);
            app.maybe_make_proxy(id);
        });
    }

    /// With "Create proxies automatically" on, a big video gets its proxy as
    /// soon as it has been read (small ones would gain nothing).
    fn maybe_make_proxy(self: &Rc<Self>, id: MediaId) {
        let wanted = {
            let st = self.state.borrow();
            let (edge, _) = st.settings.proxy_spec();
            st.settings.auto_proxies
                && st.session.as_ref().is_some_and(|s| {
                    s.existing_proxy(id).is_none()
                        && s.doc
                            .media_item(id)
                            .ok()
                            .and_then(|m| m.probe.as_ref())
                            .is_some_and(|p| p.has_video && p.width.max(p.height) > edge)
                })
        };
        if wanted {
            self.create_proxy(id);
        }
    }

    fn probe_unprobed_media(self: &Rc<Self>) {
        let todo: Vec<(MediaId, PathBuf)> = {
            let st = self.state.borrow();
            let Some(session) = st.session.as_ref() else {
                return;
            };
            session
                .doc
                .media
                .iter()
                .filter(|m| m.probe.is_none())
                .filter_map(|m| session.media_path(m.id).map(|p| (m.id, p)))
                .filter(|(_, p)| p.is_file())
                .collect()
        };
        for (id, path) in todo {
            self.spawn_probe(id, path);
        }
    }

    // ----- dialogs ------------------------------------------------------------

    pub fn choose_folder(self: &Rc<Self>, title: &str, then: impl Fn(PathBuf) + 'static) {
        let dialog = gtk::FileDialog::builder().title(title).build();
        dialog.select_folder(Some(&self.window), gio::Cancellable::NONE, move |result| {
            if let Some(path) = result.ok().and_then(|f| f.path()) {
                then(path);
            }
        });
    }

    fn choose_media_files(self: &Rc<Self>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Media files"));
        for ext in cutroom_core::state::MEDIA_EXTENSIONS {
            filter.add_suffix(ext);
        }
        if self.state.borrow().settings.remux_on_import {
            for ext in cutroom_core::state::REMUX_EXTENSIONS {
                filter.add_suffix(ext);
            }
        }
        let dialog = gtk::FileDialog::builder()
            .title("Import media")
            .default_filter(&filter)
            .build();
        let app = self.clone();
        dialog.open_multiple(Some(&self.window), gio::Cancellable::NONE, move |result| {
            let Ok(model) = result else { return };
            let mut files = Vec::new();
            for index in 0..model.n_items() {
                if let Some(file) = model
                    .item(index)
                    .and_then(|o| o.downcast::<gio::File>().ok())
                {
                    if let Some(path) = file.path() {
                        files.push(path);
                    }
                }
            }
            app.import_paths(files, None);
        });
    }

    // ----- actions -----------------------------------------------------------

    pub fn run_action(self: &Rc<Self>, action: Action) {
        match action {
            Action::NewProject => dialogs::new_project(self),
            Action::OpenProject => {
                let app = self.clone();
                self.choose_folder("Open a Kani project folder", move |dir| {
                    app.open_project(&dir)
                });
            }
            Action::ImportMedia => self.choose_media_files(),
            Action::ImportFolder => {
                let app = self.clone();
                self.choose_folder("Import every media file in a folder", move |dir| {
                    app.import_paths(vec![dir], None)
                });
            }
            Action::Paste => self.paste_from_clipboard(),
            Action::Copy => self.copy_selected_clip(false),
            Action::CopyAttributes => {
                if let Some(id) = self.selected_id() {
                    self.copy_attributes_of(id);
                }
            }
            Action::PasteAttributes => {
                if let Some(id) = self.selected_id() {
                    self.paste_attributes_on(id);
                }
            }
            Action::Cut => self.copy_selected_clip(true),
            Action::Quit => self.quit(),
            Action::MoveProject => {
                let app = self.clone();
                self.choose_folder(
                    "Move project to (choose a new or empty folder)",
                    move |dest| {
                        let result = {
                            let mut st = app.state.borrow_mut();
                            st.session.as_mut().map(|s| s.move_project(&dest))
                        };
                        if let Some(Err(e)) = result {
                            app.set_status(format!("Move failed: {e}"));
                        }
                        app.refresh();
                    },
                );
            }
            Action::CloseProject => self.close_project(),
            Action::ShowAbout => dialogs::about(self),
            Action::ShowShortcuts => dialogs::shortcuts(self),
            Action::ShowSettings => crate::settings_ui::open(self, None),
            Action::PreviewFullscreen => crate::editor::toggle_fullscreen(self),
            Action::SelectAll => {
                if let Some(s) = self.state.borrow_mut().session.as_mut() {
                    s.select_all();
                }
                self.refresh();
            }
            // While a range is being picked (Shift+Ctrl+X), Export means that range.
            Action::Export => {
                if crate::range_select::active(self) {
                    crate::range_select::export(self)
                } else {
                    crate::export_ui::open(self)
                }
            }
            Action::ExportRange => crate::range_select::enter(self),
            Action::SelectTool => {
                self.state.borrow_mut().tool = Tool::Select;
                self.refresh();
            }
            Action::SplitTool => {
                let mut st = self.state.borrow_mut();
                if st.session.is_some() {
                    st.tool = Tool::Split;
                }
                drop(st);
                self.refresh();
            }
            Action::ZoomIn | Action::ZoomOut if self.ui().editor.preview_hovered() => {
                let factor = if action == Action::ZoomIn { 1.25 } else { 0.8 };
                editor::zoom_preview(self, factor);
            }
            Action::AddText => {
                {
                    let mut st = self.state.borrow_mut();
                    if let Some(session) = st.session.as_mut() {
                        session.add_text_clip(cutroom_timeline::TextSpec::default());
                    }
                }
                self.refresh();
            }
            Action::ZoomIn | Action::ZoomOut => {
                let mut st = self.state.borrow_mut();
                let anchor = (st.timeline_width / 2.0) as f32;
                let factor = if action == Action::ZoomIn { 1.25 } else { 0.8 };
                st.view.zoom_at(factor, anchor);
                drop(st);
                self.refresh();
            }
            Action::FitTimeline => {
                let mut st = self.state.borrow_mut();
                let width = st.timeline_width as f32;
                let end = st.session.as_ref().map(|s| s.timeline_end());
                if let Some(end) = end {
                    st.view.fit(end, width);
                }
                drop(st);
                self.refresh();
            }
            other => {
                self.run_session_action(other);
                self.refresh();
            }
        }
    }

    fn run_session_action(&self, action: Action) {
        let mut st = self.state.borrow_mut();
        let Some(s) = st.session.as_mut() else {
            return;
        };
        let rate = s.doc.settings.frame_rate;
        match action {
            Action::Undo => s.undo(),
            Action::Redo => s.redo(),
            Action::Split => s.split_selected(),
            Action::SplitAll => s.split_all_at_playhead(),
            Action::DeleteLeft => s.delete_left_of_playhead(),
            Action::DeleteRight => s.delete_right_of_playhead(),
            Action::ToggleActive => {
                s.toggle_selected_disabled();
            }
            Action::Delete => s.delete_selected(false),
            Action::RippleDelete => s.delete_selected(true),
            Action::AddTrack(kind) => s.add_track(kind),
            Action::TogglePlay => s.toggle_play(),
            Action::Save => s.save(),
            Action::StepFrames(n) => {
                if let Some(per_frame) = rate.ticks_per_frame() {
                    s.playing = false;
                    s.set_playhead(Ticks(s.playhead.0 + n * per_frame));
                }
            }
            Action::GoToStart => {
                s.playing = false;
                s.set_playhead(Ticks::ZERO);
            }
            Action::GoToEnd => {
                s.playing = false;
                let end = s.timeline_end();
                s.set_playhead(end);
            }
            _ => {}
        }
        if !s.playing {
            drop(st);
            self.preview.borrow_mut().pause();
        }
    }

    pub fn is_action_enabled(&self, action: Action) -> bool {
        let st = self.state.borrow();
        let has_project = st.session.is_some();
        let (can_undo, can_redo, has_selection) =
            st.session.as_ref().map_or((false, false, false), |s| {
                (
                    s.history.can_undo(),
                    s.history.can_redo(),
                    s.selected.is_some(),
                )
            });
        match action {
            Action::NewProject
            | Action::OpenProject
            | Action::Quit
            | Action::ShowAbout
            | Action::ShowShortcuts
            | Action::ShowSettings => true,
            Action::Undo => can_undo,
            Action::Redo => can_redo,
            Action::Split
            | Action::DeleteLeft
            | Action::DeleteRight
            | Action::Delete
            | Action::RippleDelete => has_selection,
            _ => has_project,
        }
    }

    // ----- the timer -----------------------------------------------------------

    fn tick(self: &Rc<Self>) {
        let now = Instant::now();
        let (playing_before, still_playing, moved) = {
            let mut st = self.state.borrow_mut();
            let dt = now.duration_since(st.last_tick).as_secs_f64();
            st.last_tick = now;
            st.tick_count = st.tick_count.wrapping_add(1);
            let interval = Duration::from_secs(u64::from(st.settings.autosave_every()) * 60);
            let every_second = st.tick_count % 30 == 0;
            let mut last = st.last_synced_playhead;
            let Some(s) = st.session.as_mut() else {
                return;
            };
            let before = s.playing;
            s.tick(dt);
            if every_second {
                s.autosave_if_due(interval);
            } else {
                s.poll_autosave();
            }
            let moved = s.playhead != last;
            last = s.playhead;
            let after = s.playing;
            st.last_synced_playhead = last;
            (before, after, moved)
        };
        self.preview.borrow_mut().poll();
        self.render_preview();
        if still_playing || moved {
            self.sync_preview(moved);
            editor::on_tick(self);
        }
        if playing_before && !still_playing {
            self.preview.borrow_mut().pause();
            self.refresh();
        }
        if self.state.borrow().tick_count % 30 == 0 {
            self.update_status();
        }
        if self.state.borrow().tick_count % 150 == 0 {
            self.save_ui_state();
        }
    }
}

// ----- widget construction -----------------------------------------------------

fn build_ui(app: &Rc<App>) -> Ui {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let menu = menu::build(app);
    // The menu bar, with Shortcuts and Export at its right end.
    let menu_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    menu.bar.set_hexpand(true);
    menu_row.append(&menu.bar);
    let editor_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    editor_actions.set_margin_end(8);
    editor_actions.set_valign(gtk::Align::Center);
    let shortcuts_button = gtk::Button::new();
    shortcuts_button.set_child(Some(&crate::icons::with_label(
        crate::icons::KEYBOARD,
        "Shortcuts",
    )));
    shortcuts_button.add_css_class("flat");
    shortcuts_button.add_css_class("cc-menu-button");
    {
        let app = app.clone();
        shortcuts_button.connect_clicked(move |_| app.run_action(Action::ShowShortcuts));
    }
    editor_actions.append(&shortcuts_button);
    let export_button = gtk::Button::new();
    export_button.set_child(Some(&crate::icons::with_label(
        crate::icons::EXPORT,
        "Export",
    )));
    export_button.add_css_class("cc-export");
    export_button.add_css_class("cc-menu-button");
    export_button.set_tooltip_text(Some("Export the video (Ctrl+E)"));
    {
        let app = app.clone();
        export_button.connect_clicked(move |_| app.run_action(Action::Export));
    }
    editor_actions.append(&export_button);
    menu_row.append(&editor_actions);
    root.append(&menu_row);

    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.set_hexpand(true);
    let home_ui = home::build(app);
    let editor_ui = editor::build(app);
    stack.add_named(&home_ui.root, Some("home"));
    stack.add_named(&editor_ui.root, Some("editor"));

    let drop_overlay = gtk::Label::new(Some("Drop files to import them"));
    drop_overlay.add_css_class("drop-overlay");
    drop_overlay.set_visible(false);
    drop_overlay.set_can_target(false);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&stack));
    overlay.add_overlay(&drop_overlay);
    root.append(&overlay);

    let status_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    status_bar.add_css_class("status-bar");
    let status = gtk::Label::new(Some("Ready"));
    status.set_xalign(0.0);
    status.set_hexpand(true);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let autosave = gtk::Label::new(None);
    autosave.add_css_class("dim-label");
    status_bar.append(&status);
    status_bar.append(&autosave);
    root.append(&status_bar);

    app.window.set_child(Some(&root));
    Ui {
        stack,
        status,
        autosave,
        home: home_ui,
        editor: editor_ui,
        menu,
        drop_overlay,
        editor_actions,
    }
}

fn install_controllers(app: &Rc<App>) {
    // Keyboard shortcuts, handled before any widget so they work everywhere
    // except where typing needs the key.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let app = app.clone();
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(name) = key.name() else {
                return glib::Propagation::Proceed;
            };
            let name = name.to_string();
            let name = if name.chars().count() == 1 {
                name.to_lowercase()
            } else {
                name
            };
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let alt = state.contains(gdk::ModifierType::ALT_MASK);
            // Picking a range to export: Esc leaves that, Enter exports it.
            if crate::range_select::active(&app) && !ctrl && !alt {
                if name == "Escape" {
                    crate::range_select::cancel(&app);
                    return glib::Propagation::Stop;
                }
                if name == "Return" || name == "KP_Enter" {
                    crate::range_select::export(&app);
                    return glib::Propagation::Stop;
                }
            }
            if alt {
                return glib::Propagation::Proceed;
            }
            // Symbols that need Shift to type (like "+") don't count it.
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK) && name != "plus";
            let typing = gtk::prelude::GtkWindowExt::focus(&app.window).is_some_and(|w| {
                w.downcast_ref::<gtk::Editable>().is_some() || w.is::<gtk::TextView>()
            });
            let Some(action) = shortcuts::lookup(&name, ctrl, shift, typing) else {
                return glib::Propagation::Proceed;
            };
            // With no project open, only file-level shortcuts apply.
            let allowed_without_project = matches!(
                action,
                Action::NewProject
                    | Action::OpenProject
                    | Action::Quit
                    | Action::ShowShortcuts
                    | Action::ShowSettings
            );
            if app.state.borrow().session.is_none() && !allowed_without_project {
                return glib::Propagation::Proceed;
            }
            // While a range is being picked, editing keys wait.
            if crate::range_select::active(&app)
                && matches!(
                    action,
                    Action::Split
                        | Action::SplitAll
                        | Action::DeleteLeft
                        | Action::DeleteRight
                        | Action::Delete
                        | Action::RippleDelete
                        | Action::Cut
                        | Action::Paste
                        | Action::Undo
                        | Action::Redo
                        | Action::AddText
                        | Action::ToggleActive
                )
            {
                return glib::Propagation::Stop;
            }
            app.run_action(action);
            glib::Propagation::Stop
        });
    }
    app.window.add_controller(keys);

    // Files dragged in from the file manager (works on Wayland, X11 and Windows).
    let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    {
        let app = app.clone();
        drop.connect_enter(move |_, _, _| {
            app.ui().drop_overlay.set_visible(true);
            gdk::DragAction::COPY
        });
    }
    {
        let app = app.clone();
        drop.connect_leave(move |_| app.ui().drop_overlay.set_visible(false));
    }
    {
        let app = app.clone();
        drop.connect_drop(move |_, value, _, _| {
            app.ui().drop_overlay.set_visible(false);
            let Ok(list) = value.get::<gdk::FileList>() else {
                return false;
            };
            let files: Vec<PathBuf> = list.files().iter().filter_map(|f| f.path()).collect();
            if files.is_empty() {
                return false;
            }
            app.handle_window_drop(files);
            true
        });
    }
    app.window.add_controller(drop);

    // The window's close button leaves the project (saving it) and goes home;
    // closing from the home screen, or File > Quit, really exits.
    {
        let app_weak = Rc::downgrade(app);
        app.window.connect_close_request(move |_| {
            let Some(app) = app_weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let (has_session, quitting) = {
                let st = app.state.borrow();
                (st.session.is_some(), st.quitting)
            };
            if has_session && !quitting {
                app.close_project();
                glib::Propagation::Stop
            } else {
                app.preview.borrow_mut().shutdown();
                glib::Propagation::Proceed
            }
        });
    }
}

fn start_timer(app: &Rc<App>) {
    let weak = Rc::downgrade(app);
    glib::timeout_add_local(Duration::from_millis(33), move || match weak.upgrade() {
        Some(app) => {
            app.tick();
            glib::ControlFlow::Continue
        }
        None => glib::ControlFlow::Break,
    });
    // Follow the desktop's light/dark setting when the choice is "Follow system".
    if let Some(source) = gio::SettingsSchemaSource::default() {
        if source.lookup("org.gnome.desktop.interface", true).is_some() {
            let settings = gio::Settings::new("org.gnome.desktop.interface");
            let weak = Rc::downgrade(app);
            settings.connect_changed(Some("color-scheme"), move |_, _| {
                if let Some(app) = weak.upgrade() {
                    let choice = app.state.borrow().settings.theme;
                    if choice == ThemeChoice::System {
                        app.set_theme(ThemeChoice::System);
                    }
                }
            });
            // Keep the GSettings object alive for the life of the app.
            std::mem::forget(settings);
        }
    }
}

/// What was found on the clipboard.
pub(crate) enum Pasted {
    /// The clip copied with Ctrl+C / Ctrl+X in this app.
    Clip,
    Files(Vec<PathBuf>),
    /// A picture (for example "Copy image" in a browser).
    Image(gdk::Texture),
    /// Nothing usable; the string lists what the clipboard does offer.
    Nothing(String),
}

/// Text put on the system clipboard when a clip is copied inside the app.
const CLIP_MARKER: &str = "cutroom-clip";

/// MIME types file managers use to describe copied files, best first.
const FILE_LIST_MIMES: [&str; 2] = ["text/uri-list", "x-special/gnome-copied-files"];

/// Folders become the media files inside them; unsupported files are counted.
fn expand_media_inputs(input: Vec<PathBuf>) -> (Vec<PathBuf>, usize) {
    let mut files = Vec::new();
    let mut skipped = 0usize;
    for path in input {
        if path.is_dir() {
            files.extend(collect_media_files(&path));
        } else if is_supported_media(&path) {
            files.push(path);
        } else {
            skipped += 1;
        }
    }
    (files, skipped)
}

async fn read_clipboard(clipboard: &gdk::Clipboard, own_clip_available: bool) -> Pasted {
    let formats = clipboard.formats();

    // 0. A clip copied inside the app is still the newest thing.
    if own_clip_available && formats.contains_type(String::static_type()) {
        if let Ok(Some(text)) = clipboard.read_text_future().await {
            if text.as_str() == CLIP_MARKER {
                return Pasted::Clip;
            }
        }
    }

    // 1. The file manager's own lists, read as plain text.
    let offered: Vec<&str> = FILE_LIST_MIMES
        .iter()
        .copied()
        .filter(|m| formats.contain_mime_type(m))
        .collect();
    if !offered.is_empty() {
        if let Ok((stream, _)) = clipboard
            .read_future(&offered, glib::Priority::DEFAULT)
            .await
        {
            let mut data: Vec<u8> = Vec::new();
            loop {
                match stream
                    .read_bytes_future(65536, glib::Priority::DEFAULT)
                    .await
                {
                    Ok(chunk) if !chunk.is_empty() => data.extend_from_slice(&chunk),
                    _ => break,
                }
            }
            let found = paths::paths_from_text(&String::from_utf8_lossy(&data));
            if !found.is_empty() {
                return Pasted::Files(found);
            }
        }
    }
    // 2. GTK's own file-list type (covers other platforms' native formats).
    if formats.contains_type(gdk::FileList::static_type()) {
        if let Ok(value) = clipboard
            .read_value_future(gdk::FileList::static_type(), glib::Priority::DEFAULT)
            .await
        {
            if let Ok(list) = value.get::<gdk::FileList>() {
                let found: Vec<PathBuf> = list.files().iter().filter_map(|f| f.path()).collect();
                if !found.is_empty() {
                    return Pasted::Files(found);
                }
            }
        }
    }
    // 3. A picture: "Copy image" in a browser, a screenshot, an image editor.
    let has_image = formats.contains_type(gdk::Texture::static_type())
        || formats.mime_types().iter().any(|m| m.starts_with("image/"));
    if has_image {
        if let Ok(Some(texture)) = clipboard.read_texture_future().await {
            return Pasted::Image(texture);
        }
    }
    // 4. Paths copied as text.
    if let Ok(Some(text)) = clipboard.read_text_future().await {
        let found = paths::existing(paths::paths_from_text(text.as_str()));
        if !found.is_empty() {
            return Pasted::Files(found);
        }
    }
    Pasted::Nothing(formats.to_str().to_string())
}
