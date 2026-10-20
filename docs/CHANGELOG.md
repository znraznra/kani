# What changed, build by build

Newest first, in plain words. "Build" is the number of the package Tako tests; the version is
`major.phase.iteration` (0.5.37 = still before the first official release, phase-5 numbering, 37th build of it).
Each line says which roadmap phase it belongs to.

| Build | Version | Date | Phase | What changed |
|---:|---|---|---|---|
| 73 | 0.5.37 | 2026-10-19 | 11 | Windows: Kani keeps its settings, logs and caches beside the program, never in AppData; old AppData settings are moved over. |
| 72 | 0.5.36 | 2026-10-19 | 11 | Windows: a script makes a self-contained Kani folder; a start-up log; automatic safe mode after a crash (from a full-system freeze on the Dell). |
| 71 | 0.5.35 | 2026-10-18 | 2 | Project cards on the home screen show the project's last picture. Windows set aside at a checkpoint. |
| 70 | 0.5.34 | 2026-10-18 | 3 | Fixed: no sound in the preview on Windows. Kani now picks the sound output itself; a test tone in Settings. |
| 69 | 0.5.33 | 2026-10-18 | 2 | Dragging a track scrolls the list; the hardware check window no longer causes warnings. |
| 68 | 0.5.32 | 2026-10-18 | 2, 6, 7 | Rename and rearrange tracks; new tracks between rows; Captions track always on top; pictures snap to guides and the grid; import .srt/.vtt/.ass captions; one consistent look across the app. |
| 67 | 0.5.31 | 2026-10-18 | 2, 3, 6 | Home sidebar (Projects, Recents, Brand Kits); hardware decoding check; saved text styles; smoother graphics-chip preview; proxies follow their folder. |
| 66 | 0.5.30 | 2026-10-18 | 2, 4, 6 | Replace a clip by dragging a file onto it; click a picture in the Player to select it; full-screen controls; Ctrl+A; lock guides. |
| 65 | 0.5.29 | 2026-10-17 | 3, 4, 7 | Automatic proxies, hardware-decode switch, preview resolution, remux on import, scrub sound; rulers, grid and guides; full screen; Captions track only for captions. |
| 64 | 0.5.28 | 2026-10-17 | 2 | Project settings window (name, location, size, frame rate, colour space). |
| 62-63 | 0.5.26-27 | 2026-10-17 | 2 | The Settings window. |
| 60-61 | 0.5.24-25 | 2026-10-17 | 6 | Corner pin and mesh warp; the Morph section with keyframes. |
| 37-59 | 0.5.1-23 | 2026-10-15 to 17 | 5 | Export: MP4 to WebM, H.264 to AV1, hardware encoders, graphics-card compositing (a 3:54 video went from 6 min to 42 s on the ThinkPad), presets, ranges. |
| 25-36 | 0.7.1-12 | 2026-10-13 to 15 | 7 | Automatic captions with Whisper on your own computer, word-by-word highlighting, caption looks. |
| 23-24 | 0.6.1-2 | 2026-10-13 | 6 | Text clips with fonts, outline, shadow, box, skew and perspective. |
| up to 22 | 0.4.x | 2026-10 | 1-4 | Timeline and project folders, the app shell (moved from egui to GTK4), real media through GStreamer, the layer compositor and keyframes. |
