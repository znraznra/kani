//! Test-only: builds a small sample project (real media, probed, on the
//! timeline) so the editor can be launched against something realistic.

#[cfg(test)]
mod tests {
    use crate::media::{self, test_media};
    use cutroom_core::state::Session;
    use cutroom_project::ProjectSettings;
    use cutroom_timeline::{FrameRate, Ticks};
    use std::path::PathBuf;

    /// Run with `CUTROOM_SAMPLE_DIR=/some/dir cargo test -p cutroom-gtk make_sample -- --ignored`.
    #[test]
    #[ignore]
    fn make_sample_project() {
        let dir = PathBuf::from(std::env::var("CUTROOM_SAMPLE_DIR").expect("CUTROOM_SAMPLE_DIR"));
        assert!(
            test_media::init(),
            "GStreamer plugins for test media are missing"
        );
        std::fs::create_dir_all(&dir).unwrap();
        let media_dir = dir.join("footage");
        std::fs::create_dir_all(&media_dir).unwrap();
        let (clip, song, photo) = (
            media_dir.join("beach-walk.webm"),
            media_dir.join("theme-song.wav"),
            media_dir.join("logo.png"),
        );
        test_media::make_video(&clip, 6);
        test_media::make_wav(&song, 10);
        test_media::make_png(&photo, 640, 360);

        let root = dir.join("projects/Summer promo");
        let mut session = Session::create(
            &root,
            "Summer promo",
            ProjectSettings::vertical(FrameRate::FPS_30),
        )
        .unwrap();
        let ids = session.import_files(&[clip.clone(), song.clone(), photo.clone()]);
        for (id, path) in ids.iter().zip([&clip, &song, &photo]) {
            let result = media::probe_file(path).unwrap();
            session.apply_probe(*id, result.duration, result.probe);
        }
        session.add_media_to_timeline(ids[0]).unwrap();
        session.set_playhead(Ticks::from_seconds_f64(0.5));
        session.add_media_to_timeline(ids[1]).unwrap();
        session.set_playhead(Ticks::from_seconds_f64(2.0));
        session.selected = None;
        session.save();
        println!("sample project at {}", root.display());
    }
}
