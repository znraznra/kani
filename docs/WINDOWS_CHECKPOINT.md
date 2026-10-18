# Windows checkpoint (parked 2026-10-18, at 0.5.35)

Windows work is parked here. This is where it stands, and what to pick up when it resumes.

## Machine

Dell Precision 3530: i7-8850H + UHD 630, Quadro P600 4 GB (driver 582.78), Windows, Rust MSVC,
GTK 4.24 + GStreamer 1.28.7 from MSYS2, built with `cargo run` (not the setup script).

## Works on Windows (confirmed by Tako)

* Builds and runs (needs LLVM + `LIBCLANG_PATH` and CMake for speech recognition, or `--no-default-features`).
* With MSYS2's gst-plugins-base/good/bad/ugly + gst-libav installed: import, real clip lengths, video preview,
  hardware decoder detection (d3d11/d3d12, nvcodec, qsv).
* **Preview sound (fixed in 0.5.34):** Kani picks the sound output itself (WASAPI, then DirectSound) and
  falls back instead of muting. Settings > Playback > Play a test tone works, and projects play with sound.

## Open, by phase

| Phase | Item |
|---|---|
| 3 | Drag and drop of files from Explorer: retest. Hardware decoding check (Settings > Playback) on the Dell's footage. |
| 3 | Startup check that warns when GStreamer has no decoders or no sound output (instead of everything failing at once). |
| 4/5 | Graphics-chip compositing: EGL is normally absent on Windows, so preview and export fall back to the processor; a Direct3D/ANGLE path is untried. |
| 5 | Export with NVENC (`nvh264enc` / `nvh265enc`), Quick Sync, D3D12 encoders; real-time numbers on the Dell. |
| 11 | Windows build docs for the MSVC + MSYS2 route (LLVM, LIBCLANG_PATH, CMake, plugin packages, clearing the registry cache), the setup script tested, the exe icon embedded, an installer. |

## To resume

1. Pull `main` (or the latest `kani-source.zip`), `cargo run -p cutroom-gtk --release` on the Dell.
2. Send the console from start-up through playing a clip and opening Export (the encoder test list).
3. Work through the table above, Phase 3 first.
