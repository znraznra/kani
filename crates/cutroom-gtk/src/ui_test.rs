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
            let _ = id;
            app.refresh();
            pump_until(|| false);
            shot("text");
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
            assert!(
                pump_for(60, || dialog.export.is_sensitive()),
                "an encoder was found"
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
            // H.264 goes in MP4, MOV and MKV, not WebM; MP4 carries AAC and MP3.
            let formats = items(&d.format);
            assert!(formats.contains(&"MP4".to_string()), "{formats:?}");
            assert!(!formats.contains(&"WebM".to_string()), "{formats:?}");
            assert!(items(&d.audio_format)[0].starts_with("AAC"));
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
            assert_eq!(d.size.selected(), project_size, "back on the project's size");
            // The bit rate presets are listed with what they come to.
            let rates = items(&d.bitrate);
            assert!(rates.iter().any(|r| r.starts_with("Recommended - ")), "{rates:?}");
            assert!(rates.iter().any(|r| r == "Custom"));
            // Another video format brings its own file formats and sound.
            d.codec.set_selected(2); // VP9
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
            assert!(!out_dir.path().join("e2e sound.mp4").exists(), "no video was asked for");
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
            d.export.emit_clicked();
            assert!(
                pump_for(240, || crate::export_ui::active()
                    .is_none_or(|x| x.saved.borrow().is_some())),
                "both files were made"
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
        assert_eq!(clip_count(&app), clips_now);
        assert_eq!(track_count(&app), tracks_now);
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

        // 11. The window's close button goes home instead of quitting.
        app.window.close();
        pump_until(|| page(&app) == "home");
        assert_eq!(page(&app), "home");
        assert!(app.window.is_visible(), "the window stays open");

        app.preview.borrow_mut().shutdown();
    }
}
