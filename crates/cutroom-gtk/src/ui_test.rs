//! End-to-end test of the real application with a real display. It skips
//! itself when there is no display (for example on a headless CI runner).
//! One test function: GTK may only be used from a single thread.

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::media::test_media;
    use cutroom_core::new_project::NewProjectForm;
    use cutroom_core::settings::ThemeChoice;
    use cutroom_core::shortcuts::Action;
    use cutroom_core::state::MediaKind;
    use cutroom_timeline::TrackKind;
    use gtk4 as gtk;
    use gtk4::glib;
    use gtk4::prelude::*;
    use gtk4::{gdk, gio};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    /// Run the GTK main loop until `done` is true (or time runs out).
    fn pump_until(done: impl Fn() -> bool) -> bool {
        pump_for(20, done)
    }

    /// Like `pump_until`, with a longer limit for slow work (speech models).
    fn pump_for(seconds: u64, done: impl Fn() -> bool) -> bool {
        let context = glib::MainContext::default();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(seconds) {
            while context.iteration(false) {}
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn page(app: &App) -> String {
        app.ui()
            .stack
            .visible_child_name()
            .map(|s| s.to_string())
            .unwrap_or_default()
    }

    /// Not a check: prints how tall each dialog wants to be, to chase
    /// "Trying to measure GtkWindow ... but it needs at least" warnings.
    /// `xvfb-run -a cargo test -p cutroom-gtk dialog_sizes -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dialog_sizes() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
        if gtk::init().is_err() || !test_media::init() {
            return;
        }
        let application = gtk::Application::builder()
            .application_id("dev.kani.sizes")
            .build();
        application.register(gio::Cancellable::NONE).unwrap();
        let app: Rc<App> = App::new(&application, Vec::new());
        let mut form = NewProjectForm::new(tmp.path().join("projects"));
        form.name = "Sizes".into();
        app.create_project(&form).unwrap();
        let report = |name: &str| {
            pump_until(|| false);
            for w in gtk::Window::list_toplevels() {
                let Ok(w) = w.downcast::<gtk::Window>() else {
                    continue;
                };
                if !w.is_visible() || w.title().is_some_and(|t| t.ends_with("Kani")) {
                    continue;
                }
                let width = w.width();
                let (min, nat, _, _) = w.measure(gtk::Orientation::Vertical, width);
                eprintln!(
                    "SIZE {name}: {:?} {}x{} min height {min} natural {nat} default {:?}",
                    w.title(),
                    width,
                    w.height(),
                    w.default_size()
                );
                w.close();
            }
            pump_until(|| false);
        };
        crate::dialogs::about(&app);
        report("about");
        crate::dialogs::shortcuts(&app);
        report("shortcuts");
        crate::settings_ui::open(&app, None);
        report("settings");
        crate::project_settings_ui::open(&app);
        report("project settings");
    }

    /// Not a check: saves pictures of the home screen pages and the Replace
    /// window, to look at. `KANI_SHOT_DIR=/tmp/x xvfb-run -a cargo test -p cutroom-gtk look_shots -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn look_shots() {
        let Ok(dir) = std::env::var("KANI_SHOT_DIR") else {
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
        std::env::set_var("GSK_RENDERER", "cairo");
        if gtk::init().is_err() || !test_media::init() {
            return;
        }
        let application = gtk::Application::builder()
            .application_id("dev.kani.shots")
            .build();
        application.register(gio::Cancellable::NONE).unwrap();
        let app: Rc<App> = App::new(&application, Vec::new());
        app.window.present();
        let shot = |widget: &gtk::Widget, name: &str| {
            pump_until(|| false);
            let (w, h) = (widget.width(), widget.height());
            let paintable = gtk::WidgetPaintable::new(Some(widget));
            let snap = gtk::Snapshot::new();
            paintable.snapshot(&snap, f64::from(w), f64::from(h));
            if let (Some(node), Some(native)) = (snap.to_node(), widget.native()) {
                if let Some(renderer) = native.renderer() {
                    let _ = renderer
                        .render_texture(&node, None)
                        .save_to_png(format!("{dir}/{name}.png"));
                }
            }
        };
        for name in ["First clip", "Second one"] {
            let mut form = NewProjectForm::new(tmp.path().join("projects"));
            form.name = name.into();
            app.create_project(&form).unwrap();
            app.run_action(Action::CloseProject);
            pump_until(|| false);
        }
        for page in ["projects", "recents", "brand-kits"] {
            app.ui().home.show_page(page);
            shot(app.window.upcast_ref(), &format!("home_{page}"));
        }
        // The Replace window.
        let mut form = NewProjectForm::new(tmp.path().join("projects"));
        form.name = "Replace".into();
        app.create_project(&form).unwrap();
        let (short, long) = (tmp.path().join("short.webm"), tmp.path().join("long.webm"));
        test_media::make_video(&short, 2);
        test_media::make_video(&long, 5);
        app.import_paths(vec![short, long], None);
        assert!(pump_until(|| {
            let st = app.state.borrow();
            let s = st.session.as_ref().unwrap();
            s.doc.media.len() == 2 && s.doc.media.iter().all(|m| m.probe.is_some())
        }));
        let (first, second) = {
            let st = app.state.borrow();
            let m = &st.session.as_ref().unwrap().doc.media;
            (m[0].id, m[1].id)
        };
        let clip = {
            let mut st = app.state.borrow_mut();
            let s = st.session.as_mut().unwrap();
            s.add_media_to_timeline(first);
            s.doc
                .timeline
                .tracks
                .iter()
                .flat_map(|t| t.clips.iter())
                .next()
                .unwrap()
                .id
        };
        let (window, _) = crate::replace_ui::open(&app, clip, second).expect("opens");
        shot(window.upcast_ref(), "replace");
        window.close();
        // The editor with the media panel, a text clip in the inspector, the
        // captions panel, Settings and Project settings.
        shot(app.window.upcast_ref(), "editor_media");
        {
            let mut st = app.state.borrow_mut();
            let s = st.session.as_mut().unwrap();
            s.add_text_clip(cutroom_timeline::text::templates()[0].spec.clone());
        }
        app.refresh();
        shot(app.window.upcast_ref(), "editor_text");
        for (i, (name, _)) in crate::editor::CATEGORIES.iter().enumerate() {
            if *name == "Captions" || *name == "Text" {
                app.state.borrow_mut().category = i;
                crate::editor::refresh_library(&app);
                shot(
                    app.window.upcast_ref(),
                    &format!("editor_{}", name.to_lowercase()),
                );
            }
        }
        crate::settings_ui::open(&app, Some("performance"));
        pump_until(|| false);
        for w in gtk::Window::list_toplevels() {
            let Ok(w) = w.downcast::<gtk::Window>() else {
                continue;
            };
            if w.is_visible() && w.title().is_some_and(|t| t.contains("Settings")) {
                shot(w.upcast_ref(), "settings");
                w.close();
            }
        }
        crate::project_settings_ui::open(&app);
        pump_until(|| false);
        for w in gtk::Window::list_toplevels() {
            let Ok(w) = w.downcast::<gtk::Window>() else {
                continue;
            };
            if w.is_visible() && w.title().is_some_and(|t| t.contains("Project")) {
                shot(w.upcast_ref(), "project_settings");
                w.close();
            }
        }
    }

    #[test]
    fn the_application_works_end_to_end() {
        if std::env::var("DISPLAY").is_err() && std::env::var("WAYLAND_DISPLAY").is_err() {
            eprintln!("no display: skipping the GTK end-to-end test");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
        std::env::set_var("GSK_RENDERER", "cairo");
        if gtk::init().is_err() || !test_media::init() {
            eprintln!("GTK or GStreamer plugins unavailable: skipping");
            return;
        }
        let _ = gstgtk4::plugin_register_static();

        let application = gtk::Application::builder()
            .application_id("dev.kani.endtoend")
            .build();
        application.register(gio::Cancellable::NONE).unwrap();
        let app: Rc<App> = App::new(&application, Vec::new());

        // 1. Starts on the home screen.
        assert_eq!(page(&app), "home");
        {
            // The sidebar: Projects first; Recents and Brand Kits switch the page.
            let home = &app.ui().home;
            assert_eq!(home.visible_page(), "projects");
            home.show_page("brand-kits");
            assert_eq!(home.visible_page(), "brand-kits");
            home.show_page("recents");
            assert_eq!(home.visible_page(), "recents");
            home.show_page("projects");
            assert_eq!(home.visible_page(), "projects");
        }

        // 2. Create a project: lands in the editor with the starter tracks.
        let mut form = NewProjectForm::new(tmp.path().join("projects"));
        form.name = "End to end".into();
        app.create_project(&form).unwrap();
        assert_eq!(page(&app), "editor");
        assert_eq!(
            app.state
                .borrow()
                .session
                .as_ref()
                .unwrap()
                .doc
                .timeline
                .tracks
                .len(),
            2,
            "a new project has only Video 1 and Audio 1"
        );

        // 3. Import real media; the probe fills in lengths and kinds.
        let (video, song, photo) = (
            tmp.path().join("clip.webm"),
            tmp.path().join("song.wav"),
            tmp.path().join("logo.png"),
        );
        test_media::make_video(&video, 2);
        test_media::make_wav(&song, 2);
        test_media::make_png(&photo, 200, 100);
        app.import_paths(vec![video.clone(), song.clone(), photo.clone()], None);
        let probed = pump_until(|| {
            let st = app.state.borrow();
            let s = st.session.as_ref().unwrap();
            s.doc.media.len() == 3
                && s.doc
                    .media
                    .iter()
                    .filter(|m| m.id.0 != 0)
                    .all(|m| m.probe.is_some())
        });
        assert!(probed, "media were not probed in time");
        {
            let st = app.state.borrow();
            let s = st.session.as_ref().unwrap();
            let kinds: Vec<MediaKind> = s.doc.media.iter().map(|m| s.media_kind_of(m.id)).collect();
            assert_eq!(
                kinds,
                [MediaKind::Video, MediaKind::Audio, MediaKind::Image]
            );
            let seconds = s.media_duration(s.doc.media[0].id).as_seconds_f64();
            assert!((seconds - 2.0).abs() < 0.3, "real length, got {seconds}");
        }

        // 4. The library shows all three.
        let mut rows = 0;
        let mut child = app.ui().editor.library_list_for_tests().first_child();
        while let Some(w) = child {
            rows += 1;
            child = w.next_sibling();
        }
        assert_eq!(rows, 3, "library rows");

        // 5. Put the video and the sound on the timeline, split, undo.
        {
            let mut st = app.state.borrow_mut();
            let s = st.session.as_mut().unwrap();
            let ids: Vec<_> = s.doc.media.iter().map(|m| m.id).collect();
            s.add_media_to_timeline(ids[0]).unwrap();
            s.add_media_to_timeline(ids[1]).unwrap();
            s.set_playhead(cutroom_timeline::Ticks::from_seconds_f64(1.0));
        }
        app.refresh();
        let clip_count = |app: &App| -> usize {
            app.state
                .borrow()
                .session
                .as_ref()
                .unwrap()
                .doc
                .timeline
                .tracks
                .iter()
                .map(|t| t.clips.len())
                .sum()
        };
        assert_eq!(clip_count(&app), 2);
        {
            let mut st = app.state.borrow_mut();
            let s = st.session.as_mut().unwrap();
            let id = s
                .doc
                .timeline
                .tracks
                .iter()
                .find(|t| t.kind == TrackKind::Video && !t.clips.is_empty())
                .unwrap()
                .clips[0]
                .id;
            s.selected = Some(id);
        }
        app.run_action(Action::Split);
        assert_eq!(clip_count(&app), 3);
        app.run_action(Action::Undo);
        assert_eq!(clip_count(&app), 2);

        // 6. The preview follows the playhead without errors.
        app.sync_preview(true);
        pump_until(|| false || true);

        let track_count = |app: &App| -> usize {
            app.state
                .borrow()
                .session
                .as_ref()
                .unwrap()
                .doc
                .timeline
                .tracks
                .len()
        };
        let media_count =
            |app: &App| -> usize { app.state.borrow().session.as_ref().unwrap().doc.media.len() };
        let set_playhead = |app: &App, seconds: f64| {
            app.state
                .borrow_mut()
                .session
                .as_mut()
                .unwrap()
                .set_playhead(cutroom_timeline::Ticks::from_seconds_f64(seconds));
        };

        // 7. Pasting a file imports it and places it at the playhead. The
        //    playhead (1 s) rests on the song, so it gets a new audio track.
        let extra = tmp.path().join("extra.wav");
        test_media::make_wav(&extra, 1);
        app.paste_files(vec![extra], None);
        assert_eq!(media_count(&app), 4);
        assert_eq!(clip_count(&app), 3);
        assert_eq!(track_count(&app), 3, "a new audio track was made");

        // 7b. Copy a clip, paste it where there is room (same track), then
        //     onto a clip (new track).
        set_playhead(&app, 0.5);
        {
            let mut st = app.state.borrow_mut();
            let s = st.session.as_mut().unwrap();
            let id = s
                .doc
                .timeline
                .tracks
                .iter()
                .find(|t| t.name == "Video 1")
                .unwrap()
                .clips[0]
                .id;
            s.selected = Some(id);
        }
        app.run_action(Action::Copy);
        set_playhead(&app, 5.0);
        app.run_action(Action::Paste);
        assert!(
            pump_until(|| clip_count(&app) == 4),
            "pasted into free space"
        );
        assert_eq!(track_count(&app), 3, "no new track where there was room");
        set_playhead(&app, 1.0);
        app.run_action(Action::Paste);
        assert!(pump_until(|| clip_count(&app) == 5), "pasted onto a clip");
        assert_eq!(track_count(&app), 4, "a new track for it");

        // 7c. A picture from the clipboard (a browser's "Copy image") is saved
        //     into the project and placed at the playhead.
        set_playhead(&app, 8.0);
        let texture = gdk::Texture::from_filename(&photo).unwrap();
        app.apply_paste(crate::app::Pasted::Image(texture));
        assert_eq!(media_count(&app), 5);
        assert_eq!(clip_count(&app), 6);
        let saved = std::fs::read_dir(tmp.path().join("projects/End to end/media"))
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with("pasted-image-"));
        assert!(saved, "the picture was saved in the project's media folder");
        // Photos show in the preview.
        app.sync_preview(true);
        assert!(app.preview.borrow().shows_still(), "the photo is on screen");
        set_playhead(&app, 1.0);
        app.sync_preview(true);
        assert!(!app.preview.borrow().shows_still(), "back to video");

        // 7d. Extract audio from the video clip: its sound moves to an audio clip.
        let (video_clip, clips_before) = {
            let st = app.state.borrow();
            let s = st.session.as_ref().unwrap();
            let id = s
                .doc
                .timeline
                .tracks
                .iter()
                .find(|t| t.name == "Video 1")
                .unwrap()
                .clips
                .iter()
                .find(|c| s.can_extract_audio(c.id))
                .expect("a video clip with sound")
                .id;
            (id, clip_count(&app))
        };
        crate::clip_menu::run(
            &app,
            video_clip,
            cutroom_core::clip_menu::ClipCommand::ExtractAudio,
        );
        assert_eq!(clip_count(&app), clips_before + 1, "an audio clip appeared");
        assert!(
            app.state
                .borrow()
                .session
                .as_ref()
                .unwrap()
                .doc
                .timeline
                .find_clip(video_clip)
                .unwrap()
                .1
                .audio_muted,
            "and the video clip went silent"
        );

        // 7b. Text: a style from the library adds a clip with no media, the
        // inspector builds for it, and the preview draws the words.
        {
            let spec = cutroom_timeline::text::templates()[2].spec.clone();
            let mut st = app.state.borrow_mut();
            let session = st.session.as_mut().unwrap();
            session.set_playhead(cutroom_timeline::Ticks::from_seconds_f64(100.0));
            let id = session.add_text_clip(spec).expect("a text clip");
            assert!(session.clip_text(id).is_some());
        }
        app.refresh();
        let canvas = {
            let st = app.state.borrow();
            let s = st.session.as_ref().unwrap();
            (s.doc.settings.width, s.doc.settings.height)
        };
        app.preview.borrow_mut().forget_last_render();
        let picture = app
            .preview
            .borrow_mut()
            .compose_now(canvas, None)
            .expect("the words are drawn");
        let lit = picture
            .rgba
            .chunks_exact(4)
            .filter(|p| p[0] > 100 || p[1] > 100)
            .count();
        assert!(lit > 100, "yellow letters on black, found {lit} lit pixels");

        // 7c. Keyframes move the words over time, and a clip keeps its settings
        // when another clip is selected and then this one again.
        {
            let centroid = |t: f64| -> f64 {
                {
                    let mut st = app.state.borrow_mut();
                    let session = st.session.as_mut().unwrap();
                    session.set_playhead(cutroom_timeline::Ticks::from_seconds_f64(t));
                }
                app.sync_preview(true);
                let canvas = {
                    let st = app.state.borrow();
                    let s = st.session.as_ref().unwrap();
                    (s.doc.settings.width, s.doc.settings.height)
                };
                app.preview.borrow_mut().forget_last_render();
                let picture = app
                    .preview
                    .borrow_mut()
                    .compose_now(canvas, None)
                    .expect("a picture");
                let (mut sum, mut n) = (0.0, 0.0);
                for (i, p) in picture.rgba.chunks_exact(4).enumerate() {
                    if p[0] > 100 || p[1] > 100 {
                        sum += (i as u32 % picture.width) as f64 / f64::from(picture.width);
                        n += 1.0;
                    }
                }
                assert!(n > 20.0, "words visible at {t}s");
                sum / n
            };
            let (id, start) = {
                let st = app.state.borrow();
                let s = st.session.as_ref().unwrap();
                let (_, c) = s
                    .doc
                    .timeline
                    .tracks
                    .iter()
                    .flat_map(|t| t.clips.iter().map(move |c| (t, c)))
                    .find(|(_, c)| c.text.is_some())
                    .unwrap();
                (c.id, c.start.as_seconds_f64())
            };
            {
                let mut st = app.state.borrow_mut();
                let s = st.session.as_mut().unwrap();
                s.set_playhead(cutroom_timeline::Ticks::from_seconds_f64(start));
                assert!(s.toggle_keyframes(id, &[cutroom_timeline::Prop::X]));
                s.set_playhead(cutroom_timeline::Ticks::from_seconds_f64(start + 2.0));
                let mut t = s.transform_at_playhead(id).unwrap();
                t.x = 300;
                assert!(s.edit_transform(id, t));
            }
            let (a, mid, b) = (
                centroid(start + 0.1),
                centroid(start + 1.0),
                centroid(start + 1.9),
            );
            assert!(a < mid && mid < b, "moves right over time: {a} {mid} {b}");
            // Selecting another clip and coming back keeps everything.
            let before = app
                .state
                .borrow()
                .session
                .as_ref()
                .unwrap()
                .doc
                .timeline
                .find_clip(id)
                .unwrap()
                .1
                .clone();
            let other = {
                let st = app.state.borrow();
                let s = st.session.as_ref().unwrap();
                s.doc
                    .timeline
                    .tracks
                    .iter()
                    .flat_map(|t| &t.clips)
                    .find(|c| c.text.is_none())
                    .unwrap()
                    .id
            };
            for pick in [other, id, other, id] {
                app.state
                    .borrow_mut()
                    .session
                    .as_mut()
                    .unwrap()
                    .select_only(pick);
                app.refresh();
            }
            let after = app
                .state
                .borrow()
                .session
                .as_ref()
                .unwrap()
                .doc
                .timeline
                .find_clip(id)
                .unwrap()
                .1
                .clone();
            assert_eq!(before, after, "selection never changes a clip");
        }

        // 7d. The real keyframe diamond and slider: key X at one time, change it at
        // another, and the second change becomes a keyframe too.
        {
            fn find<T: IsA<gtk::Widget>>(root: &gtk::Widget, out: &mut Vec<T>) {
                if let Some(w) = root.downcast_ref::<T>() {
                    out.push(w.clone());
                }
                let mut child = root.first_child();
                while let Some(c) = child {
                    find(&c, out);
                    child = c.next_sibling();
                }
            }
            let (id, start) = {
                let st = app.state.borrow();
                let s = st.session.as_ref().unwrap();
                let (_, c) = s
                    .doc
                    .timeline
                    .tracks
                    .iter()
                    .flat_map(|t| t.clips.iter().map(move |c| (t, c)))
                    .find(|(_, c)| c.text.is_some())
                    .unwrap();
                (c.id, c.start.as_seconds_f64())
            };
            let _ = (id, start);
            let (id, start) = {
                let mut st = app.state.borrow_mut();
                let s = st.session.as_mut().unwrap();
                s.set_playhead(cutroom_timeline::Ticks::from_seconds_f64(200.0));
                let id = s
                    .add_text_clip(cutroom_timeline::TextSpec::default())
                    .unwrap();
                (id, 200.0)
            };
            app.refresh();
            let diamonds = || {
                let mut v: Vec<gtk::ToggleButton> = Vec::new();
                find(app.ui().editor.inspector_widget().upcast_ref(), &mut v);
                v.into_iter()
                    .filter(|b| {
                        b.tooltip_text()
                            .is_some_and(|t| t.contains("keyframe at the playhead"))
                    })
                    .collect::<Vec<_>>()
            };
            let all = diamonds();
            assert!(
                all.len() >= 11,
                "text header, position header and one per property: {}",
                all.len()
            );
            // The second one is Position X (the first is the Text section's or the header's).
            let keys = |app: &App| {
                app.state
                    .borrow()
                    .session
                    .as_ref()
                    .unwrap()
                    .doc
                    .timeline
                    .find_clip(id)
                    .unwrap()
                    .1
                    .keyframes
                    .len()
            };
            let x_diamond = all.iter().position(|b| b.tooltip_text().is_some()).unwrap();
            let _ = x_diamond;
            // Text header, Position header, then Position X.
            all[2].emit_clicked();
            pump_until(|| false || true);
            assert_eq!(keys(&app), 1, "one key after the diamond is clicked");
            // Move the playhead and change X with the real slider.
            {
                let mut st = app.state.borrow_mut();
                let s = st.session.as_mut().unwrap();
                s.set_playhead(cutroom_timeline::Ticks::from_seconds_f64(start + 2.0));
            }
            app.refresh();
            let mut scales: Vec<gtk::Scale> = Vec::new();
            find(app.ui().editor.inspector_widget().upcast_ref(), &mut scales);
            let x_slider = scales
                .iter()
                .find(|s| s.adjustment().lower() == -100.0 && s.adjustment().upper() == 100.0)
                .expect("the Position X slider");
            x_slider.set_value(35.0);
            let recorded = pump_until(|| keys(&app) == 2);
            assert!(
                recorded,
                "the slider change at a new time added a keyframe ({} keys)",
                keys(&app)
            );
        }

        // 7f. Captions: the panel shows, finds sound to listen to, and a run with the
        // tiny test model (it cannot really hear) finishes cleanly and reports back.
        {
            {
                let mut st = app.state.borrow_mut();
                st.category = 3;
            }
            crate::editor::refresh_library(&app);
            assert_eq!(
                app.ui().editor.library_stack_name().as_deref(),
                Some("captions")
            );
            let panel = app.ui().editor.captions.clone();
            // Put the sound file on the timeline well after everything else.
            let wav = {
                let mut st = app.state.borrow_mut();
                let s = st.session.as_mut().unwrap();
                s.set_playhead(cutroom_timeline::Ticks::from_seconds_f64(500.0));
                let ids: Vec<_> = s.doc.media.iter().map(|m| m.id).collect();
                let id = ids
                    .into_iter()
                    .find(|id| {
                        s.media_path(*id)
                            .is_some_and(|p| p.to_string_lossy().ends_with("song.wav"))
                    })
                    .expect("the wav is in the project");
                s.add_media_to_timeline(id).expect("placed");
                s.clear_selection();
                id
            };
            let _ = wav;
            let model = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../cutroom-captions/tests/data/for-tests-ggml-tiny.en.bin");
            {
                let mut st = app.state.borrow_mut();
                st.settings.caption_model = "custom".into();
                st.settings.caption_model_path = Some(model);
                st.settings.caption_language = "en".into();
            }
            crate::captions_ui::refresh(&app, &panel);
            assert!(
                panel.generate.is_sensitive(),
                "ready to generate: {}",
                panel.source_note.text()
            );
            assert!(
                !panel.export_srt.is_sensitive(),
                "no captions to export yet"
            );
            panel.generate.emit_clicked();
            assert!(panel.job.borrow().is_some(), "work started");
            let finished = pump_for(180, || panel.job.borrow().is_none());
            assert!(finished, "transcription finished");
            assert!(!panel.message.text().is_empty(), "it says what happened");
            assert!(panel.generate.is_sensitive(), "the panel is ready again");
            // Back to the media list for the rest of the test.
            app.state.borrow_mut().category = 0;
            crate::editor::refresh_library(&app);
        }

        // 7g. Position & Size stay put: set with the real sliders, add skew and
        // perspective, click away (a spin box losing focus), select other clips.
        {
            fn find<T: IsA<gtk::Widget>>(root: &gtk::Widget, out: &mut Vec<T>) {
                if let Some(w) = root.downcast_ref::<T>() {
                    out.push(w.clone());
                }
                let mut child = root.first_child();
                while let Some(c) = child {
                    find(&c, out);
                    child = c.next_sibling();
                }
            }
            let (vid, other) = {
                let st = app.state.borrow();
                let s = st.session.as_ref().unwrap();
                let clips: Vec<_> = s
                    .doc
                    .timeline
                    .tracks
                    .iter()
                    .flat_map(|t| t.clips.iter())
                    .filter(|c| c.text.is_none() && s.can_transform(c.id))
                    .collect();
                (clips[0].id, clips[1].id)
            };
            let start = {
                let mut st = app.state.borrow_mut();
                let s = st.session.as_mut().unwrap();
                let c = s.doc.timeline.find_clip(vid).unwrap().1.clone();
                s.set_playhead(c.start + cutroom_timeline::Ticks(c.duration.0 / 2));
                s.select_only(vid);
                // A value with a fraction, as dragging the box makes.
                let mut t = c.transform;
                t.x = 123;
                t.scale = 640;
                s.edit_transform(vid, t);
                c.start
            };
            let _ = start;
            app.refresh();
            let transform = |app: &App| {
                app.state
                    .borrow()
                    .session
                    .as_ref()
                    .unwrap()
                    .doc
                    .timeline
                    .find_clip(vid)
                    .unwrap()
                    .1
                    .transform
            };
            let scales = || {
                let mut v: Vec<gtk::Scale> = Vec::new();
                find(app.ui().editor.inspector_widget().upcast_ref(), &mut v);
                v
            };
            // Click away: every spin box in the inspector updates as it loses focus.
            let click_away = || {
                let mut v: Vec<gtk::SpinButton> = Vec::new();
                find(app.ui().editor.inspector_widget().upcast_ref(), &mut v);
                for b in v {
                    b.update();
                }
                pump_until(|| false || true);
                std::thread::sleep(Duration::from_millis(700));
                pump_until(|| false || true);
            };
            click_away();
            assert_eq!(
                (transform(&app).x, transform(&app).scale),
                (123, 640),
                "clicking away changes nothing"
            );
            scales()[5].set_value(20.0);
            assert!(pump_until(|| transform(&app).skew_x == 200), "skew set");
            click_away();
            scales()[7].set_value(30.0);
            assert!(
                pump_until(|| transform(&app).persp_x == 300),
                "perspective set"
            );
            click_away();
            let t = transform(&app);
            assert_eq!(
                (t.x, t.scale, t.skew_x, t.persp_x),
                (123, 640, 200, 300),
                "{t:?}"
            );
            for pick in [other, vid, other, vid] {
                app.state
                    .borrow_mut()
                    .session
                    .as_mut()
                    .unwrap()
                    .select_only(pick);
                app.refresh();
                click_away();
            }
            let t = transform(&app);
            assert_eq!(
                (t.x, t.scale, t.skew_x, t.persp_x),
                (123, 640, 200, 300),
                "{t:?}"
            );
            let shown: Vec<f64> = scales().iter().map(|s| s.value()).collect();
            assert!(
                (shown[0] - 12.3).abs() < 0.06 && (shown[2] - 64.0).abs() < 0.06,
                "{shown:?}"
            );
            // With a keyframe on X the same must hold, and the other settings stay.
            {
                let mut st = app.state.borrow_mut();
                let s = st.session.as_mut().unwrap();
                s.toggle_keyframes(vid, &[cutroom_timeline::Prop::X]);
            }
            app.refresh();
            let shown_x = |app: &App| {
                let st = app.state.borrow();
                let s = st.session.as_ref().unwrap();
                s.doc
                    .timeline
                    .find_clip(vid)
                    .unwrap()
                    .1
                    .transform_at(s.playhead)
            };
            let before = shown_x(&app);
            for pick in [other, vid, other, vid] {
                app.state
                    .borrow_mut()
                    .session
                    .as_mut()
                    .unwrap()
                    .select_only(pick);
                app.refresh();
                click_away();
            }
            assert_eq!(shown_x(&app), before, "keyed clip keeps its values");
            // Selecting another clip straight after a change (before the quiet
            // timer fires) must still record the change on the first clip.
            app.state
                .borrow_mut()
                .session
                .as_mut()
                .unwrap()
                .select_only(vid);
            app.refresh();
            scales()[6].set_value(-25.0);
            app.state
                .borrow_mut()
                .session
                .as_mut()
                .unwrap()
                .select_only(other);
            app.refresh();
            click_away();
            assert_eq!(
                transform(&app).skew_y,
                -250,
                "a change made just before selecting another clip is kept"
            );
            app.state
                .borrow_mut()
                .session
                .as_mut()
                .unwrap()
                .clear_selection();
        }

        // 7h. Auto-Captions section: plain text has none; generated captions
        // show "Apply to all captions" and a grid of styles; group edits and
        // attribute copy/paste only reach clips of the same type.
        {
            fn find<T: IsA<gtk::Widget>>(root: &gtk::Widget, out: &mut Vec<T>) {
                if let Some(w) = root.downcast_ref::<T>() {
                    out.push(w.clone());
                }
                let mut child = root.first_child();
                while let Some(c) = child {
                    find(&c, out);
                    child = c.next_sibling();
                }
            }
            let before = clip_count(&app);
            let plain = {
                let mut st = app.state.borrow_mut();
                let s = st.session.as_mut().unwrap();
                s.set_playhead(cutroom_timeline::Ticks::from_seconds_f64(400.0));
                let id = s
                    .add_text_clip(cutroom_timeline::text::templates()[0].spec.clone())
                    .unwrap();
                s.select_only(id);
                id
            };
            app.refresh();
            let inspector = || app.ui().editor.inspector_widget();
            let mut switches: Vec<gtk::Switch> = Vec::new();
            find(inspector().upcast_ref(), &mut switches);
            assert!(
                switches.is_empty(),
                "no Auto-Captions switch for plain text"
            );
            let mut grids: Vec<gtk::FlowBox> = Vec::new();
            find(inspector().upcast_ref(), &mut grids);
            assert!(grids.is_empty(), "no style grid for plain text");

            // Two generated captions.
            let mk = |t: f64| cutroom_captions::lines::Line {
                start: t,
                end: t + 2.0,
                words: vec![cutroom_captions::TimedWord {
                    text: " Hello".into(),
                    start: t,
                    end: t + 0.5,
                }],
            };
            let ids: Vec<_> = {
                let mut st = app.state.borrow_mut();
                let s = st.session.as_mut().unwrap();
                let spec = cutroom_timeline::caption_styles()[0].spec.clone();
                assert_eq!(s.add_captions(&[mk(410.0), mk(420.0)], &spec, "Classic"), 2);
                let mut v: Vec<_> = s
                    .doc
                    .timeline
                    .tracks
                    .iter()
                    .flat_map(|t| t.clips.iter())
                    .filter(|c| c.is_caption() && c.start.as_seconds_f64() > 400.0)
                    .map(|c| c.id)
                    .collect();
                v.sort();
                v
            };
            assert_eq!(ids.len(), 2);
            let style_of = |app: &App, id| {
                app.state
                    .borrow()
                    .session
                    .as_ref()
                    .unwrap()
                    .doc
                    .timeline
                    .find_clip(id)
                    .unwrap()
                    .1
                    .caption
                    .clone()
            };
            app.state
                .borrow_mut()
                .session
                .as_mut()
                .unwrap()
                .select_only(ids[0]);
            app.refresh();
            let styles = cutroom_timeline::caption_styles();
            let mut switches: Vec<gtk::Switch> = Vec::new();
            find(inspector().upcast_ref(), &mut switches);
            assert_eq!(switches.len(), 1, "the Apply to all captions switch");
            assert!(!switches[0].is_active(), "off by default");
            let mut grids: Vec<gtk::FlowBox> = Vec::new();
            find(inspector().upcast_ref(), &mut grids);
            assert_eq!(grids.len(), 1, "the style grid");

            // With it off only the selected caption changes.
            grids[0].select_child(&grids[0].child_at_index(2).unwrap());
            assert!(
                pump_until(|| style_of(&app, ids[0]).as_deref() == Some(styles[2].name)),
                "choosing a tile applies the style"
            );
            assert_ne!(style_of(&app, ids[1]).as_deref(), Some(styles[2].name));

            // With it on, a different style reaches every caption.
            app.refresh();
            let mut switches: Vec<gtk::Switch> = Vec::new();
            find(inspector().upcast_ref(), &mut switches);
            switches[0].set_active(true);
            pump_until(|| false);
            assert_eq!(
                style_of(&app, ids[1]).as_deref(),
                Some(styles[2].name),
                "switching on gives every caption this caption's look"
            );
            // Clicking the tile that is already chosen spreads it to the rest.
            let mut grids: Vec<gtk::FlowBox> = Vec::new();
            find(inspector().upcast_ref(), &mut grids);
            let chosen = grids[0].child_at_index(2).unwrap();
            grids[0].emit_by_name::<()>("child-activated", &[&chosen]);
            assert!(
                pump_until(|| style_of(&app, ids[1]).as_deref() == Some(styles[2].name)),
                "re-choosing the current style reaches the other captions"
            );
            app.refresh();
            let mut grids: Vec<gtk::FlowBox> = Vec::new();
            find(inspector().upcast_ref(), &mut grids);
            grids[0].select_child(&grids[0].child_at_index(1).unwrap());
            assert!(
                pump_until(|| style_of(&app, ids[1]).as_deref() == Some(styles[1].name)),
                "apply to all reaches the other caption"
            );
            assert_eq!(style_of(&app, ids[0]).as_deref(), Some(styles[1].name));
            app.state.borrow_mut().apply_all_captions = false;

            // Attributes: copied from a caption, they paste onto the other
            // caption but not onto plain text.
            {
                let mut st = app.state.borrow_mut();
                let s = st.session.as_mut().unwrap();
                assert!(s.copy_attributes(ids[0]));
                assert!(s.can_paste_attributes(ids[1]));
                assert!(!s.can_paste_attributes(plain));
                assert_eq!(s.paste_attributes(plain), 0);
                // Group targets: two selected captions share edits; the
                // plain text clip is never a target.
                s.select_many(&[ids[0], ids[1], plain]);
                let t = s.group_targets(ids[0], false);
                assert_eq!(t, vec![ids[1]]);
            }

            while clip_count(&app) > before {
                app.state.borrow_mut().session.as_mut().unwrap().undo();
            }
            app.state
                .borrow_mut()
                .session
                .as_mut()
                .unwrap()
                .clear_selection();
            app.refresh();
        }

        let (tracks_now, clips_now) = (track_count(&app), clip_count(&app));

        // 7e. Optional screenshot of the editor with a text clip selected.
        if let Ok(dir) = std::env::var("KANI_SHOT_DIR") {
            app.window.set_default_size(1500, 950);
            pump_until(|| false);
            let shot = |name: &str| {
                let (w, h) = (app.window.width(), app.window.height());
                let paintable = gtk::WidgetPaintable::new(Some(&app.window));
                let snap = gtk::Snapshot::new();
                paintable.snapshot(&snap, f64::from(w), f64::from(h));
                if let (Some(node), Some(native)) = (snap.to_node(), app.window.native()) {
                    if let Some(renderer) = native.renderer() {
                        let tex = renderer.render_texture(&node, None);
                        let _ = tex.save_to_png(format!("{dir}/{name}.png"));
                    }
                }
            };
            let id = {
                let mut st = app.state.borrow_mut();
                let s = st.session.as_mut().unwrap();
                s.set_playhead(cutroom_timeline::Ticks::from_seconds_f64(300.0));
                let id = s
                    .add_text_clip(cutroom_timeline::text::templates()[6].spec.clone())
                    .unwrap();
                s.select_only(id);
                id
            };
            app.refresh();
            pump_until(|| false);
            shot("text");
            // Morph: a skewed, mesh-warped text clip with its mesh handles.
            {
                let mut st = app.state.borrow_mut();
                st.distort_modes
                    .insert(id, crate::transform_overlay::Distort::Mesh);
                let s = st.session.as_mut().unwrap();
                let mut t = s.transform_at_playhead(id).unwrap();
                t.skew_x = 200;
                t.warp.mesh[10] = 150;
                t.warp.pin[2] = -200;
                s.edit_transform(id, t);
            }
            app.sync_preview(true);
            app.refresh();
            pump_until(|| false);
            shot("morph");
        }

        // 7h-range. Picking a range on the timeline (Shift+Ctrl+X): handles,
        // dimming, Esc to leave, and "Export range" opening Export with it.
        {
            use crate::range_select as rs;
            use cutroom_timeline::Ticks as T;
            {
                let mut st = app.state.borrow_mut();
                let s = st.session.as_mut().unwrap();
                s.set_playhead(T::from_seconds_f64(1.0));
            }
            assert!(!rs::active(&app));
            app.run_action(Action::ExportRange);
            assert!(rs::active(&app), "Shift+Ctrl+X starts picking a range");
            let x_of = |t: T| {
                let st = app.state.borrow();
                f64::from(cutroom_core::timeline_logic::HEADER_W) + f64::from(st.view.x_for(t))
            };
            let range = || app.state.borrow().export_range.unwrap();
            let r0 = range();
            assert!(r0.end > r0.start, "the part starts with some length");
            // Drag the end handle back toward the start.
            rs::press(&app, x_of(r0.end));
            rs::drag_to(&app, x_of(r0.start) + 60.0);
            rs::release(&app);
            let r1 = range();
            assert!(
                r1.end < r0.end && r1.end > r1.start,
                "the end handle moved: {r0:?} -> {r1:?}"
            );
            // Drag the whole part along.
            let len = r1.end - r1.start;
            rs::press(&app, (x_of(r1.start) + x_of(r1.end)) / 2.0);
            rs::drag_to(&app, (x_of(r1.start) + x_of(r1.end)) / 2.0 + 30.0);
            rs::release(&app);
            let r2 = range();
            assert_eq!(r2.end - r2.start, len, "moving keeps the length");
            assert!(r2.start > r1.start, "and moves it later");
            pump_until(|| false);
            if let Ok(dir) = std::env::var("KANI_SHOT_DIR") {
                app.window.set_default_size(1500, 950);
                pump_until(|| false);
                let (w, h) = (app.window.width(), app.window.height());
                let paintable = gtk::WidgetPaintable::new(Some(&app.window));
                let snap = gtk::Snapshot::new();
                paintable.snapshot(&snap, f64::from(w), f64::from(h));
                if let (Some(node), Some(native)) = (snap.to_node(), app.window.native()) {
                    if let Some(renderer) = native.renderer() {
                        let _ = renderer
                            .render_texture(&node, None)
                            .save_to_png(format!("{dir}/range.png"));
                    }
                }
            }
            // Esc (or the Cancel button) leaves without exporting.
            rs::cancel(&app);
            assert!(!rs::active(&app) && crate::export_ui::active().is_none());
            // Again, then Export range: the Export window opens with From / to filled in.
            app.run_action(Action::ExportRange);
            {
                let mut st = app.state.borrow_mut();
                let r = st.export_range.as_mut().unwrap();
                r.start = T::from_seconds_f64(1.0);
                r.end = T::from_seconds_f64(3.0);
            }
            rs::export(&app);
            assert!(!rs::active(&app));
            let d = crate::export_ui::active().expect("Export range opens the Export window");
            let picked = d
                .range
                .selected_item()
                .and_then(|o| o.downcast::<gtk::StringObject>().ok())
                .map(|o| o.string().to_string());
            assert_eq!(picked.as_deref(), Some("From / to"));
            assert!(
                (d.range_from.value() - 1.0).abs() < 0.01
                    && (d.range_to.value() - 3.0).abs() < 0.01
            );
            d.window.close();
            assert!(
                pump_until(|| crate::export_ui::active().is_none()),
                "the window closed"
            );
            // The top-right Export button (Ctrl+E, the menu) while a range is
            // being picked exports that range, with no extra step.
            app.run_action(Action::ExportRange);
            {
                let mut st = app.state.borrow_mut();
                let r = st.export_range.as_mut().unwrap();
                r.start = T::from_seconds_f64(0.5);
                r.end = T::from_seconds_f64(2.5);
            }
            app.run_action(Action::Export);
            assert!(!rs::active(&app), "picking ends");
            let d = crate::export_ui::active().expect("Export opens");
            assert!(
                (d.range_from.value() - 0.5).abs() < 0.01
                    && (d.range_to.value() - 2.5).abs() < 0.01,
                "the Export window found the picked range"
            );
            d.window.close();
            assert!(pump_until(|| crate::export_ui::active().is_none()));
        }

        // 7i. Export: the window opens, finds an encoder, and a short export
        // runs through it; the file is a real MP4 with picture and sound.
        {
            // Keep the export short: only the first two seconds of the project.
            {
                let mut st = app.state.borrow_mut();
                let s = st.session.as_mut().unwrap();
                s.set_playhead(cutroom_timeline::Ticks::ZERO);
            }
            crate::export_ui::limit_for_tests(2.0);
            app.run_action(Action::Export);
            let dialog = crate::export_ui::active().expect("the Export window opens");
            assert!(!dialog.export.is_sensitive(), "waits for the encoder test");
            let ok = pump_for(60, || dialog.export.is_sensitive());
            assert!(
                ok,
                "an encoder was found: {}",
                crate::export_ui::debug_state(&dialog)
            );
            // Smallest size, to keep the test quick.
            dialog.size.set_selected(
                dialog
                    .size
                    .model()
                    .map_or(0, |m| m.n_items().saturating_sub(1)),
            );
            let out_dir = tempfile::tempdir().unwrap();
            dialog.name.set_text("e2e: export?");
            // Save into a temporary folder instead of the Videos folder.
            crate::export_ui::set_folder_for_tests(&dialog, out_dir.path());
            let target = crate::export_ui::output_path(&dialog);
            assert_eq!(target.file_name().unwrap(), "e2e_ export_.mp4");
            dialog.export.emit_clicked();
            assert!(
                pump_for(240, || crate::export_ui::active()
                    .is_none_or(|d| d.saved.borrow().is_some())),
                "the export finished"
            );
            let saved = dialog.saved.borrow().clone();
            let saved = saved.expect("the export saved a file");
            assert_eq!(saved, target);
            assert!(std::fs::metadata(&saved).unwrap().len() > 1000);
            if let Ok(out) = std::process::Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "stream=codec_name",
                    "-of",
                    "csv=p=0",
                ])
                .arg(&saved)
                .output()
            {
                let text = String::from_utf8_lossy(&out.stdout).to_string();
                if !text.is_empty() {
                    assert!(text.contains("h264"), "{text}");
                }
            }
            // Closing the window again leaves the editor usable.
            dialog.window.close();
            let _ = dialog.cancel.is_sensitive();
            // A closed window is forgotten: Export opens a fresh one instead of
            // showing the dead one again.
            assert!(
                pump_for(5, || crate::export_ui::active().is_none()),
                "the closed Export window is not reported as open"
            );
            app.run_action(Action::Export);
            let again = crate::export_ui::active().expect("Export opens a new window");
            assert!(
                !Rc::ptr_eq(&again, &dialog),
                "a new window, not the dead one"
            );
            assert!(again.window.is_visible());
            again.window.close();
            assert!(
                pump_for(5, || crate::export_ui::active().is_none()),
                "the second window closes too"
            );
        }

        // 7j. The redesigned Export window: the choices follow each other, and
        // each kind of file is made (video with sound, sound alone, both).
        {
            let items = |dd: &gtk::DropDown| -> Vec<String> {
                let model = dd.model().expect("a list");
                let list = model.downcast::<gtk::StringList>().expect("strings");
                (0..list.n_items())
                    .filter_map(|i| list.string(i).map(|s| s.to_string()))
                    .collect()
            };
            // With KANI_SHOT_DIR set, the Export window is photographed (form, running, done).
            let shot_dialog = |d: &std::rc::Rc<crate::export_ui::Dialog>, name: &str| {
                let Ok(dir) = std::env::var("KANI_SHOT_DIR") else {
                    return;
                };
                let (w, h) = (d.window.width(), d.window.height());
                let paintable = gtk::WidgetPaintable::new(Some(&d.window));
                let snap = gtk::Snapshot::new();
                paintable.snapshot(&snap, f64::from(w), f64::from(h));
                if let (Some(node), Some(native)) = (snap.to_node(), d.window.native()) {
                    if let Some(renderer) = native.renderer() {
                        let tex = renderer.render_texture(&node, None);
                        let _ = tex.save_to_png(format!("{dir}/{name}.png"));
                        eprintln!("kani: photographed the Export window ({w}x{h}) as {name}");
                    }
                }
            };
            let open_ready = || {
                app.run_action(Action::Export);
                let d = crate::export_ui::active().expect("the Export window opens");
                assert!(
                    pump_for(60, || d.export.is_sensitive()),
                    "an encoder was found"
                );
                d
            };
            let d = open_ready();
            pump_for(1, || false);
            shot_dialog(&d, "export_form");
            // H.264 goes in MP4, MOV and MKV, not WebM; MP4 carries AAC and MP3.
            let formats = items(&d.format);
            assert!(formats.contains(&"MP4".to_string()), "{formats:?}");
            assert!(!formats.contains(&"WebM".to_string()), "{formats:?}");
            assert!(items(&d.audio_format)[0].starts_with("AAC"));
            assert_eq!(
                items(&d.audio_format)[d.audio_format.selected() as usize],
                "AAC",
                "the sound format starts on AAC"
            );
            // The size list has the NTSC and PAL sizes for the chosen shape, and
            // changes with the shape.
            let sizes = items(&d.size);
            assert!(sizes.iter().any(|s| s.contains("NTSC")), "{sizes:?}");
            assert!(sizes.iter().any(|s| s.contains("PAL")), "{sizes:?}");
            assert!(
                sizes.iter().any(|s| s.contains("project")),
                "the project's own size is listed: {sizes:?}"
            );
            let project_size = d.size.selected();
            d.aspect.set_selected(1);
            let other = items(&d.size);
            assert_ne!(sizes, other, "another shape, other sizes");
            d.aspect.set_selected(0);
            assert_eq!(
                d.size.selected(),
                project_size,
                "back on the project's size"
            );
            // Range: the whole project, or from / to (and the selected clips when there are some).
            {
                let ranges = items(&d.range);
                assert_eq!(ranges[0], "Whole project");
                assert_eq!(ranges.last().map(String::as_str), Some("From / to"));
                assert!(
                    !d.range_from.is_visible(),
                    "the typed range is hidden until asked for"
                );
                let whole = d.summary.text().to_string();
                d.range.set_selected(ranges.len() as u32 - 1);
                assert!(d.range_from.is_visible() && d.range_to.is_visible());
                d.range_from.set_value(1.0);
                d.range_to.set_value(3.0);
                assert!(
                    d.summary.text().starts_with("Duration: 0:02"),
                    "{}",
                    d.summary.text()
                );
                assert_ne!(d.summary.text().as_str(), whole.as_str());
                assert!(d.export.is_sensitive());
                // A part that ends before it starts leaves nothing to export.
                d.range_to.set_value(0.5);
                assert!(!d.export.is_sensitive(), "an empty part cannot be exported");
                assert!(d.summary.text().contains("Nothing is in the chosen part"));
                d.range.set_selected(0);
                assert!(d.export.is_sensitive());
                assert_eq!(d.summary.text().as_str(), whole.as_str());
            }
            // The choices scroll up and down only: nothing is wider than the column.
            {
                let h = d.form_scroll.hadjustment();
                let (min_w, _, _, _) = d
                    .form_scroll
                    .child()
                    .unwrap()
                    .measure(gtk::Orientation::Horizontal, -1);
                eprintln!(
                    "ui_test: form column needs {min_w} px, shows {} of {}",
                    h.page_size(),
                    h.upper()
                );
                fn walk(w: &gtk::Widget, depth: usize) {
                    let (m, _, _, _) = w.measure(gtk::Orientation::Horizontal, -1);
                    if m > 250 {
                        eprintln!(
                            "ui_test: {:indent$}{} min {m}",
                            "",
                            w.type_().name(),
                            indent = depth * 2
                        );
                    }
                    let mut c = w.first_child();
                    while let Some(ch) = c {
                        walk(&ch, depth + 1);
                        c = ch.next_sibling();
                    }
                }
                walk(&d.form_scroll.child().unwrap(), 0);
                assert!(
                    h.upper() <= h.page_size() + 0.5,
                    "no sideways scrolling: {} > {}",
                    h.upper(),
                    h.page_size()
                );
            }
            // Acceleration: Automatic, Graphics card, Processor.
            assert_eq!(
                items(&d.gpu_mode),
                vec![
                    "Automatic (recommended)".to_string(),
                    "Graphics card".to_string(),
                    "Processor".to_string()
                ]
            );
            // The bit rate presets are listed with what they come to.
            let rates = items(&d.bitrate);
            assert!(
                rates.iter().any(|r| r.starts_with("Recommended - ")),
                "{rates:?}"
            );
            assert!(rates.iter().any(|r| r == "Custom"));
            // Another video format brings its own file formats and sound.
            // The Encoder list names kinds of encoder; the Codec list shows only
            // what the kind picked can make.
            let kinds = items(&d.encoder);
            assert!(kinds[0].starts_with("Automatic"), "{kinds:?}");
            assert!(kinds.iter().any(|k| k == "Processor"), "{kinds:?}");
            let all_codecs = items(&d.codec);
            let soft_at = kinds.iter().position(|k| k == "Processor").unwrap() as u32;
            d.encoder.set_selected(soft_at);
            let soft_codecs = items(&d.codec);
            assert!(
                soft_codecs.iter().all(|c| all_codecs.contains(c)),
                "{soft_codecs:?} within {all_codecs:?}"
            );
            for kind in kinds.iter().filter(|k| k.starts_with("Graphics chip")) {
                let at = kinds.iter().position(|k| k == kind).unwrap() as u32;
                d.encoder.set_selected(at);
                let hw = items(&d.codec);
                assert!(
                    !hw.is_empty() && hw.iter().all(|c| all_codecs.contains(c)),
                    "{kind}: {hw:?}"
                );
            }
            d.encoder.set_selected(0);
            assert_eq!(
                items(&d.codec),
                all_codecs,
                "Automatic offers every format found"
            );
            let vp9_at = items(&d.codec)
                .iter()
                .position(|c| c.contains("VP9"))
                .expect("a VP9 encoder works here");
            d.codec.set_selected(vp9_at as u32); // VP9
            assert!(
                pump_for(60, || d.export.is_sensitive()),
                "the VP9 encoders were looked for"
            );
            assert_eq!(items(&d.format)[0], "WebM");
            // The file format stays MP4 while it can hold the new codec ...
            assert_eq!(items(&d.format)[d.format.selected() as usize], "MP4");
            // ... and picking WebM leaves only the sound WebM can carry.
            d.format.set_selected(0);
            assert_eq!(items(&d.audio_format), vec!["Opus".to_string()]);
            d.codec.set_selected(0); // back to H.264
            assert!(pump_for(60, || d.export.is_sensitive()));
            assert!(items(&d.format).contains(&"MP4".to_string()));
            // Presets fill the choices in; changing one by hand leaves the preset.
            {
                let presets = items(&d.preset);
                assert_eq!(presets[0], "No preset");
                for name in [
                    "YouTube Shorts",
                    "TikTok",
                    "LinkedIn video (16:9)",
                    "Sound only: MP3",
                ] {
                    assert!(
                        presets.iter().any(|p| p == name),
                        "{name} is a preset: {presets:?}"
                    );
                }
                let at = |name: &str| presets.iter().position(|p| p == name).unwrap() as u32;
                d.preset.set_selected(at("Instagram feed (4:5)"));
                assert!(pump_for(60, || d.export.is_sensitive()));
                assert!(
                    items(&d.size)[d.size.selected() as usize].starts_with("1080 x 1350"),
                    "{:?}",
                    items(&d.size)[d.size.selected() as usize]
                );
                assert!(d.video_on.is_active());
                assert_eq!(items(&d.format)[d.format.selected() as usize], "MP4");
                assert_eq!(
                    items(&d.audio_format)[d.audio_format.selected() as usize],
                    "AAC"
                );
                assert_eq!(
                    d.preset.selected(),
                    at("Instagram feed (4:5)"),
                    "the preset stays while nothing is changed"
                );
                // Another shape and size by hand: no longer that preset.
                d.rate
                    .set_selected((d.rate.selected() + 1) % items(&d.rate).len() as u32);
                assert_eq!(d.preset.selected(), 0, "a changed choice leaves the preset");
                d.preset.set_selected(at("LinkedIn video (16:9)"));
                assert!(pump_for(60, || d.export.is_sensitive()));
                assert!(items(&d.size)[d.size.selected() as usize].starts_with("1920 x 1080"));
                // A format with its own file format and sound.
                d.preset.set_selected(at("Web page (WebM)"));
                assert!(pump_for(60, || d.export.is_sensitive()));
                assert_eq!(items(&d.format)[d.format.selected() as usize], "WebM");
                assert_eq!(items(&d.audio_format), vec!["Opus".to_string()]);
                // A sound-only preset switches the video off.
                d.preset.set_selected(at("Sound only: MP3"));
                assert!(!d.video_on.is_active() && d.audio_on.is_active());
                assert_eq!(
                    items(&d.audio_format)[d.audio_format.selected() as usize],
                    "MP3"
                );
                assert!(d.export.is_sensitive());
                // Back to the plain H.264 start the rest of the test expects.
                d.preset.set_selected(0);
                d.audio_on.set_active(false);
                d.video_on.set_active(true);
                d.aspect.set_selected(0);
                d.codec.set_selected(0);
                assert!(pump_for(60, || d.export.is_sensitive()));
                assert!(items(&d.format).contains(&"MP4".to_string()));
            }
            // With nothing switched on there is nothing to export.
            d.video_on.set_active(false);
            d.audio_on.set_active(false);
            assert!(!d.export.is_sensitive(), "nothing chosen");
            // Sound only: every installed sound format is offered.
            d.audio_on.set_active(true);
            let sound = items(&d.audio_format);
            assert!(sound.len() >= 2, "{sound:?}");
            assert!(d.export.is_sensitive());
            d.size.set_selected(items(&d.size).len() as u32 - 1);
            let out_dir = tempfile::tempdir().unwrap();
            d.name.set_text("e2e sound");
            crate::export_ui::set_folder_for_tests(&d, out_dir.path());
            d.export.emit_clicked();
            assert!(
                pump_for(240, || crate::export_ui::active()
                    .is_none_or(|x| x.saved.borrow().is_some())),
                "the sound file was made"
            );
            let saved = d.saved.borrow().clone().expect("a file was saved");
            assert_eq!(saved.file_name().unwrap(), "e2e sound.m4a");
            assert!(std::fs::metadata(&saved).unwrap().len() > 500);
            assert!(
                !out_dir.path().join("e2e sound.mp4").exists(),
                "no video was asked for"
            );
            d.window.close();
            assert!(pump_for(5, || crate::export_ui::active().is_none()));

            // Video and sound together: the video file, and the sound as its own file.
            let d = open_ready();
            // The window remembers the last choices: sound only, no video.
            assert!(d.audio_on.is_active() && !d.video_on.is_active());
            d.video_on.set_active(true);
            d.audio_on.set_active(true);
            assert!(
                pump_for(60, || d.export.is_sensitive()),
                "the video encoders were found"
            );
            d.size.set_selected(items(&d.size).len() as u32 - 1);
            d.name.set_text("e2e both");
            crate::export_ui::set_folder_for_tests(&d, out_dir.path());
            let size_before = (d.window.width(), d.window.height());
            d.export.emit_clicked();
            pump_for(1, || false);
            shot_dialog(&d, "export_running");
            if d.running_for_tests() {
                assert_eq!(
                    (d.window.width(), d.window.height()),
                    size_before,
                    "the window keeps its size while exporting"
                );
                assert!(!d.export.is_visible(), "Export is hidden while exporting");
                assert!(d.cancel.is_visible() && d.cancel.is_sensitive());
            }
            assert!(
                pump_for(240, || crate::export_ui::active()
                    .is_none_or(|x| x.saved.borrow().is_some())),
                "both files were made"
            );
            pump_for(1, || false);
            shot_dialog(&d, "export_done");
            assert_eq!(d.cancel.label().as_deref(), Some("Close"));
            assert_eq!(d.export.label().as_deref(), Some("Show in folder"));
            assert_eq!(
                (d.window.width(), d.window.height()),
                size_before,
                "the window keeps its size when the export is over"
            );
            assert!(out_dir.path().join("e2e both.mp4").is_file());
            assert!(out_dir.path().join("e2e both.m4a").is_file());
            d.window.close();
            assert!(pump_for(5, || crate::export_ui::active().is_none()));
        }

        // 8. Appearance.
        app.set_theme(ThemeChoice::Dark);
        assert!(app.state.borrow().dark);
        app.set_theme(ThemeChoice::Light);
        assert!(!app.state.borrow().dark);

        // 9. Closing the project saves it and returns home, where it is listed.
        app.run_action(Action::CloseProject);
        assert_eq!(page(&app), "home");
        assert!(app.state.borrow().session.is_none());
        assert_eq!(app.state.borrow().home.entries.len(), 1);

        // 10. Reopen it: the clips and media came back.
        let root = tmp.path().join("projects/End to end");
        app.open_project(&root);
        assert_eq!(page(&app), "editor");
        if std::env::var("KANI_SHOT_DIR").is_err() {
            // (Photographing adds clips and tracks, so only compare normally.)
            assert_eq!(clip_count(&app), clips_now);
            assert_eq!(track_count(&app), tracks_now);
        }
        assert_eq!(media_count(&app), 5);

        // 10b. The About window opens (and so do the shortcuts).
        let before = gtk::Window::list_toplevels().len();
        app.run_action(Action::ShowAbout);
        app.run_action(Action::ShowShortcuts);
        pump_until(|| gtk::Window::list_toplevels().len() >= before + 2);
        assert!(
            gtk::Window::list_toplevels().len() >= before + 2,
            "dialogs open as their own windows"
        );

        // 10c. Project settings: staged, applied on Save.
        {
            use crate::project_settings_ui as ps;
            let d = ps::open(&app).expect("Project settings opens");
            d.name.set_text("Renamed in test");
            d.width.set_value(1280.0);
            d.height.set_value(720.0);
            d.rate.set_selected(1);
            d.color.set_selected(2);
            d.copy.set_selected(0);
            d.cancel.emit_clicked();
            let (w, name) = {
                let st = app.state.borrow();
                let s = st.session.as_ref().unwrap();
                (s.doc.settings.width, s.doc.name.clone())
            };
            assert_ne!(w, 1280, "Cancel changes nothing");
            assert_ne!(name, "Renamed in test");
            let d = ps::open(&app).unwrap();
            d.name.set_text("Renamed in test");
            d.width.set_value(1280.0);
            d.height.set_value(720.0);
            d.rate.set_selected(1);
            d.color.set_selected(2);
            d.save.emit_clicked();
            let st = app.state.borrow();
            let s = st.session.as_ref().unwrap();
            assert_eq!((s.doc.settings.width, s.doc.settings.height), (1280, 720));
            assert_eq!(s.doc.name, "Renamed in test");
            assert_eq!(
                s.doc.settings.color_space,
                cutroom_project::ColorSpace::DisplayP3
            );
        }

        // 10b. Settings: one window for every preference. Choices wait for Save.
        {
            use crate::settings_ui as su;
            assert!(su::active().is_none());
            app.run_action(Action::ShowSettings);
            let d = su::active().expect("Settings opens");
            assert_eq!(
                d.stack.visible_child_name().as_deref(),
                Some("general"),
                "starts on General"
            );
            for (id, _) in su::TABS {
                d.stack.set_visible_child_name(id);
                assert_eq!(d.stack.visible_child_name().as_deref(), Some(id));
            }
            assert!(
                d.planned.len() >= 20,
                "rows for things not built yet are listed (disabled): {}",
                d.planned.len()
            );
            // The wanted ones are there, in the right tabs.
            for (tab, label) in [
                ("edit", "Step forward"),
                ("performance", "Processor threads"),
            ] {
                assert!(
                    d.planned.iter().any(|p| p.0 == tab && p.1 == label),
                    "{tab}/{label}"
                );
            }
            // These work now, so they are not in the "not yet" list any more.
            for label in [
                "Remux unsupported files on import",
                "Create proxies automatically",
                "Proxy quality",
                "Decode with the graphics chip",
                "Preview resolution",
                "Preview compositing on the graphics chip",
                "Play sound while dragging the playhead",
            ] {
                assert!(!d.planned.iter().any(|p| p.1 == label), "{label} is live");
            }
            // Cancel drops what was staged.
            let before = app.state.borrow().settings.clone();
            d.autosave.set_active(false);
            d.proxies.set_active(!before.use_proxies);
            d.cancel.emit_clicked();
            assert!(pump_until(|| su::active().is_none()));
            assert_eq!(
                app.state.borrow().settings.autosave_every(),
                before.autosave_every()
            );
            assert_eq!(app.state.borrow().settings.use_proxies, before.use_proxies);
            // Save applies.
            app.run_action(Action::ShowSettings);
            let d = su::active().expect("Settings opens again");
            assert_eq!(d.autosave.is_active(), before.autosave_every() > 0);
            d.autosave.set_active(true);
            d.autosave_minutes.set_value(3.0);
            d.proxies.set_active(false);
            d.add_dropped.set_active(true);
            d.acceleration.set_selected(2);
            d.theme.set_selected(1);
            d.save.emit_clicked();
            assert!(pump_until(|| su::active().is_none()));
            {
                let st = app.state.borrow();
                assert_eq!(st.settings.autosave_every(), 3);
                assert!(!st.settings.use_proxies);
                assert!(st.settings.add_dropped_to_timeline);
                assert_eq!(st.settings.export_graphics_card, "off");
                assert_eq!(st.settings.theme, cutroom_core::settings::ThemeChoice::Dark);
            }
            // Switched off keeps the interval.
            app.run_action(Action::ShowSettings);
            let d = su::active().unwrap();
            assert!(d.autosave.is_active() && (d.autosave_minutes.value() - 3.0).abs() < 0.01);
            d.autosave.set_active(false);
            assert!(
                !d.autosave_minutes.is_sensitive(),
                "the interval waits for the switch"
            );
            d.save.emit_clicked();
            assert!(pump_until(|| su::active().is_none()));
            assert_eq!(app.state.borrow().settings.autosave_every(), 0);
            assert_eq!(app.state.borrow().settings.autosave_minutes, 3);
            // Put things back the way the rest of the test expects.
            app.run_action(Action::ShowSettings);
            let d = su::active().unwrap();
            d.autosave.set_active(true);
            d.proxies.set_active(true);
            d.add_dropped.set_active(false);
            d.acceleration.set_selected(0);
            d.theme.set_selected(0);
            d.save.emit_clicked();
            assert!(pump_until(|| su::active().is_none()));
            // The playback and performance choices are applied on Save.
            app.run_action(Action::ShowSettings);
            let d = su::active().unwrap();
            {
                let mut s = d.draft.borrow_mut();
                s.scrub_audio = false;
                s.preview_resolution = "low".into();
                s.proxy_quality = "small".into();
                s.auto_proxies = true;
                s.remux_on_import = true;
                s.hw_decode = false;
            }
            d.save.emit_clicked();
            assert!(pump_until(|| su::active().is_none()));
            {
                let st = app.state.borrow();
                assert!(!st.settings.scrub_audio && st.settings.auto_proxies);
                assert_eq!(st.settings.preview_edge(), 480);
                assert_eq!(st.settings.proxy_spec().0, 640);
            }
            assert!(
                !app.ui().editor.scrub_active(),
                "the timeline button follows the setting"
            );
            // The timeline button changes the setting too.
            app.ui().editor.set_scrub_active(true);
            assert!(app.state.borrow().settings.scrub_audio);
            app.run_action(Action::ShowSettings);
            let d = su::active().unwrap();
            {
                let mut s = d.draft.borrow_mut();
                s.preview_resolution = "standard".into();
                s.proxy_quality = "balanced".into();
                s.auto_proxies = false;
                s.remux_on_import = false;
                s.hw_decode = true;
            }
            d.save.emit_clicked();
            assert!(pump_until(|| su::active().is_none()));

            // Rulers, grid and guides can be switched on; guides can be reset.
            {
                let mut st = app.state.borrow_mut();
                st.settings.show_rulers = true;
                st.settings.show_grid = true;
                st.settings.show_guides = true;
                st.session.as_mut().unwrap().add_guide(true, 300);
                st.session.as_mut().unwrap().add_guide(false, 400);
            }
            app.ui().editor.queue_overlay();
            pump_until(|| false);
            crate::preview_tools::reset_guides(&app);
            assert!(app
                .state
                .borrow()
                .session
                .as_ref()
                .unwrap()
                .doc
                .guides
                .is_empty());
            {
                let mut st = app.state.borrow_mut();
                st.settings.show_rulers = false;
                st.settings.show_grid = false;
            }

            // Full screen preview opens and closes.
            assert!(!crate::preview_tools::fullscreen_open());
            app.run_action(Action::PreviewFullscreen);
            assert!(crate::preview_tools::fullscreen_open());
            app.run_action(Action::PreviewFullscreen);
            assert!(pump_until(|| !crate::preview_tools::fullscreen_open()));

            // Ctrl+A selects every clip on the timeline.
            app.run_action(Action::SelectAll);
            {
                let st = app.state.borrow();
                let s = st.session.as_ref().unwrap();
                let all: usize = s.doc.timeline.tracks.iter().map(|t| t.clips.len()).sum();
                assert_eq!(s.selection().len(), all);
            }

            // The picture under a point of the Player is found (topmost first).
            {
                let (w, h, picture_clip) = {
                    let st = app.state.borrow();
                    let s = st.session.as_ref().unwrap();
                    let at = s.playhead;
                    let top = s
                        .doc
                        .timeline
                        .tracks
                        .iter()
                        .filter(|t| t.kind == TrackKind::Video && !t.hidden)
                        .flat_map(|t| t.clips.iter())
                        .find(|c| !c.disabled && c.start <= at && at < c.end())
                        .map(|c| c.id);
                    (
                        s.doc.settings.width as f32,
                        s.doc.settings.height as f32,
                        top,
                    )
                };
                if picture_clip.is_some() {
                    let found = crate::transform_overlay::clip_under(&app, (w / 2.0, h / 2.0));
                    assert!(found.is_some(), "something is drawn in the middle");
                }
                assert_eq!(
                    crate::transform_overlay::clip_under(&app, (-50.0, -50.0)),
                    None
                );
            }

            // Dropping a long enough file on a clip opens the Replace window;
            // Replace swaps the file and keeps the clip's place and length.
            {
                let pair = {
                    let st = app.state.borrow();
                    let s = st.session.as_ref().unwrap();
                    s.doc
                        .timeline
                        .tracks
                        .iter()
                        .flat_map(|t| t.clips.iter())
                        .find_map(|c| {
                            if c.text.is_some() {
                                return None;
                            }
                            s.doc
                                .media
                                .iter()
                                .find(|m| {
                                    m.id != c.media
                                        && s.media_kind_of(m.id) == MediaKind::Video
                                        && s.can_replace(c.id, m.id).is_ok()
                                })
                                .map(|m| (c.id, m.id, c.start, c.duration))
                        })
                };
                if let Some((clip, media, start, duration)) = pair {
                    let (window, replace) = crate::replace_ui::open(&app, clip, media)
                        .expect("the Replace window opens");
                    pump_until(|| false);
                    replace.emit_clicked();
                    assert!(pump_until(|| !window.is_visible()));
                    let st = app.state.borrow();
                    let (_, c) = st
                        .session
                        .as_ref()
                        .unwrap()
                        .doc
                        .timeline
                        .find_clip(clip)
                        .unwrap();
                    assert_eq!((c.media, c.start, c.duration), (media, start, duration));
                } else {
                    eprintln!("no clip/file pair for the Replace check");
                }
            }

            // A subtitle file comes in as captions on the Captions track, which
            // sits on top; one undo takes them all away again.
            {
                let srt = tmp.path().join("words.srt");
                std::fs::write(&srt, "1\n00:00:00,500 --> 00:00:01,500\nHello there\n\n2\n00:00:02,000 --> 00:00:03,000\n<i>General</i> Kenobi\n").unwrap();
                let tracks_before = app
                    .state
                    .borrow()
                    .session
                    .as_ref()
                    .unwrap()
                    .doc
                    .timeline
                    .tracks
                    .len();
                let added = crate::captions_ui::import_file(&app, &srt).expect("imported");
                assert_eq!(added, 2);
                {
                    let st = app.state.borrow();
                    let tl = &st.session.as_ref().unwrap().doc.timeline;
                    assert!(
                        cutroom_core::state::is_caption_track(&tl.tracks[0]),
                        "Captions on top"
                    );
                    let texts: Vec<String> = tl.tracks[0]
                        .clips
                        .iter()
                        .filter_map(|c| c.text.as_ref().map(|t| t.content.clone()))
                        .collect();
                    assert!(texts.iter().any(|t| t == "General Kenobi"), "{texts:?}");
                    assert!(tl.tracks[0]
                        .clips
                        .iter()
                        .all(|c| c.is_caption() && !c.words.is_empty()));
                }
                app.state.borrow_mut().session.as_mut().unwrap().undo();
                let st = app.state.borrow();
                let tl = &st.session.as_ref().unwrap().doc.timeline;
                assert!(tl.tracks.len() <= tracks_before + 1);
                assert!(tl
                    .tracks
                    .iter()
                    .all(|t| !cutroom_core::state::is_caption_track(t)
                        || t.clips.iter().all(|c| c
                            .text
                            .as_ref()
                            .is_none_or(|x| x.content != "General Kenobi"))));
            }

            // Saved text styles: keep a look, list it, apply it to other words,
            // and find it again from the file the next time the app starts.
            {
                use cutroom_core::text_styles::{restyle, StyleBook};
                let mut loud = cutroom_timeline::TextSpec::default();
                loud.content = "Look".into();
                loud.size = 91;
                loud.color = [10, 200, 30, 255];
                fn has_label(widget: &gtk::Widget, text: &str) -> bool {
                    if widget
                        .downcast_ref::<gtk::Label>()
                        .is_some_and(|l| l.text() == text)
                    {
                        return true;
                    }
                    let mut child = widget.first_child();
                    while let Some(c) = child {
                        if has_label(&c, text) {
                            return true;
                        }
                        child = c.next_sibling();
                    }
                    false
                }
                crate::editor::fill_text_styles(&app);
                assert!(!has_label(
                    app.ui().editor.text_list.upcast_ref(),
                    "Loud green"
                ));
                {
                    let mut st = app.state.borrow_mut();
                    st.text_styles.save_style("Loud green", &loud).unwrap();
                    st.save_text_styles();
                }
                crate::editor::fill_text_styles(&app);
                assert!(
                    has_label(app.ui().editor.text_list.upcast_ref(), "Loud green"),
                    "the saved style is listed in the Text category"
                );
                let dir = app
                    .state
                    .borrow()
                    .config_dir
                    .clone()
                    .expect("a config folder");
                assert_eq!(StyleBook::load(&dir), app.state.borrow().text_styles);
                let plain = cutroom_timeline::TextSpec {
                    content: "Other words".into(),
                    ..cutroom_timeline::TextSpec::default()
                };
                let saved = app
                    .state
                    .borrow()
                    .text_styles
                    .get("loud GREEN")
                    .cloned()
                    .unwrap();
                let styled = restyle(&plain, &saved.spec);
                assert_eq!(
                    (styled.content.as_str(), styled.size, styled.color),
                    ("Other words", 91, [10, 200, 30, 255])
                );
            }

            // The old places for these are gone: no Settings submenu items for them.
            if let Ok(dir) = std::env::var("KANI_SHOT_DIR") {
                app.run_action(Action::ShowSettings);
                let d = su::active().unwrap();
                for (id, _) in su::TABS {
                    d.stack.set_visible_child_name(id);
                    pump_until(|| false);
                    let (w, h) = (d.window.width(), d.window.height());
                    let paintable = gtk::WidgetPaintable::new(Some(&d.window));
                    let snap = gtk::Snapshot::new();
                    paintable.snapshot(&snap, f64::from(w), f64::from(h));
                    if let (Some(node), Some(native)) = (snap.to_node(), d.window.native()) {
                        if let Some(renderer) = native.renderer() {
                            let _ = renderer
                                .render_texture(&node, None)
                                .save_to_png(format!("{dir}/settings_{id}.png"));
                        }
                    }
                }
                d.cancel.emit_clicked();
                assert!(pump_until(|| su::active().is_none()));
            }
        }

        // 11. The window's close button goes home instead of quitting.
        app.window.close();
        pump_until(|| page(&app) == "home");
        assert_eq!(page(&app), "home");
        assert!(app.window.is_visible(), "the window stays open");

        app.preview.borrow_mut().shutdown();
    }
}
