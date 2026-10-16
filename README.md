# Kani

*Time-bending edits made simple.*

A clean-room, cross-platform (Linux / Windows / macOS) short-form video editor written in
Rust on **GTK4 and GStreamer**. Local only: no cloud AI, no accounts.

GTK gives real OS windows and popups, native file dialogs and native drag and drop on
Wayland, X11 and Windows. GStreamer does the media work: reading files, decoding, hardware
decoders where available, and the preview.

## Status

The editing model, project folders and the app shell are done, and media is now real:

* Files are **probed with GStreamer** (true length, video/audio streams, size, frame rate).
* The **preview shows the actual frame** under the playhead and plays video and sound.
* The library shows **real thumbnails** (video frames and images).

Not built yet: effects and LUTs, asset libraries, stickers. See the roadmap.

| Crate | What it is |
|-------|-----------|
| `cutroom-timeline` | Tracks, clips, reversible edits, undo/redo, snapping. Pure Rust, no I/O. |
| `cutroom-project` | Project **folder** format, atomic save, autosave, migration, relative media, collect/move/relink, project library (scan, rename, delete). |
| `cutroom-platform` | Platform paths and GPU/codec description types. |
| `cutroom-compose` | The compositor: stacks clips' pictures into one canvas image, each moved, scaled, turned, mirrored and faded. Plain pixel buffers, so the preview and the future exporter share it. CPU for now, behind an interface a GPU backend can fill. |
| `cutroom-core` | Toolkit-independent app logic: the editing session, timeline mouse logic, settings, new-project form, shortcut table, home-screen helpers. Heavily unit tested. |
| `cutroom-gtk` | The `cutroom` application: GTK4 UI, GStreamer probing, thumbnails and preview. |

## Build and run

You need a recent stable Rust (rustup) plus the GTK4 and GStreamer development packages.

**Arch / CachyOS**
```
sudo pacman -S --needed base-devel cmake clang pkgconf gtk4 gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-libav gst-plugin-va
cargo run -p cutroom-gtk --release
```
(`gst-plugin-va` adds Intel/AMD hardware decoders; NVIDIA users want `gst-plugins-bad`'s NVDEC support.)

**Ubuntu / Debian**
```
sudo apt install build-essential cmake libclang-dev pkg-config libgtk-4-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav
cargo run -p cutroom-gtk --release
```

**Windows (one step)** Double-click `tools\windows\setup-windows.bat` (or run `tools\windows\setup-windows.ps1` in PowerShell).
It installs whatever is missing (MSYS2 with winget or a direct download, then the compiler, Rust, GTK4, GStreamer and its
plugins, cmake, clang and the icon themes), builds Kani, and writes `dist\Kani` (start it with `Kani.bat`) and
`dist\Kani-windows.zip` to carry to another PC. It is safe to run again, and `-NoWhisper` skips speech recognition for a much
faster build. The first run takes a while and needs about 6 GB. This has not been run on real Windows yet, so if a step
fails, the message names the command; send it over.

**Windows (by hand, MSYS2)** Install [MSYS2](https://www.msys2.org), open the *MSYS2 MINGW64* shell, then:
```
pacman -S --needed mingw-w64-x86_64-toolchain mingw-w64-x86_64-cmake mingw-w64-x86_64-clang mingw-w64-x86_64-pkgconf mingw-w64-x86_64-gtk4 \
  mingw-w64-x86_64-gstreamer mingw-w64-x86_64-gst-plugins-base mingw-w64-x86_64-gst-plugins-good \
  mingw-w64-x86_64-gst-plugins-bad mingw-w64-x86_64-gst-libav
rustup toolchain install stable-x86_64-pc-windows-gnu && rustup default stable-x86_64-pc-windows-gnu
cargo run -p cutroom-gtk --release
```
GTK4 needs the GNU toolchain with MSYS2's libraries; the MSVC toolchain would need gvsbuild instead. The one-step script above
copies the program and the DLLs it needs into `dist\Kani`, so the result runs outside MSYS2.

Tests (they skip the window test when there is no display):
```
cargo test --workspace
# headless: xvfb-run -a env GSK_RENDERER=cairo cargo test --workspace
```
The GTK video sink is compiled in (`gst-plugin-gtk4`), so no extra GStreamer plugin is needed for the preview.

## What you can do

**Home screen** (at start, and whenever you close a project)
* Lists every project in your **projects folder** (default `Documents/Kani`; change it with *Change...*), plus projects
  opened from elsewhere. Search, sort by last modified or name, thumbnail or list view.
* Right-click a project: **Open**, **Rename** (optionally renames the folder), **Show info**, **Delete** (permanent, with
  a clear warning; refuses to touch folders that aren't Kani projects). Double-click opens.

**New project:** name, location (defaults to the projects folder), canvas preset (9:16, 4:5, 3:4, 1:1, 16:9, 4:3, 21:9,
HD and 4K) or a **custom size**, and a frame rate (23.976 to 120).

**Saving:** background **autosave every 10 minutes** (Settings > Autosave) to `project.json` plus rotating backups. Closing
a project, or the window's close button, saves and returns to the home screen; File > Quit exits.

**Getting media in:** Import, Import folder, **drag files or folders onto the window**, **paste files** copied in your file
manager (Ctrl+V), or pass files on the command line (also "Open with"). Dropping onto the timeline places the first
file where you dropped it.

**Copy, cut, paste:** Ctrl+C / Ctrl+X on a clip, then Ctrl+V (also for files and pictures from outside). The paste goes
**at the playhead**: if the track has room there it goes on that track, but if the playhead rests on a clip (or the clip would run
into the next one) it gets a **new track** (video on top, audio at the bottom). Pasted pictures are saved into the project's
`media` folder. One undo removes the clip and its new track together.

**Library:** list or thumbnails. **Right-click an item** for the usual media-panel commands: Add to Timeline, Show in
Timeline (with the clip count), Rename (display name only), Relink Media (for moved or missing files), Create/Delete Proxy,
Show in File Manager, Copy File Path, Properties (size, streams, frame rate, the decoder in use and whether it is hardware)
and Remove from Project (warns when clips use it; only files the project itself stores can be deleted from disk). Drag an item onto the timeline: video and images go on video tracks, audio on audio
tracks, with a ghost showing where it lands.

**Right-click a clip** for the full editor menu (sub-menus open as hover flyouts): Copy, Cut, Copy/Paste attributes, Delete; Edit (Freeze, Reverse, Mirror, Rotate,
Crop); Split scenes; Transcript; Generate captions; Isolate voice; Extract audio; Remove background; Enhance audio and visuals;
Adjust color; Enhance motion; Group; Pre-compose; Save preset; Export selection; Deactivate; Clip actions (Replace, Trim, Open
file location); Render (Whole timeline, In, Out). The menu is complete but only **Copy, Cut, Delete, Extract audio and Open file
location** work so far; the rest are greyed out until their phase. Extract audio puts the clip's sound on an audio track and
silences the video clip, in one undo step.

**Smooth cuts:** while playing, the next clip is loaded in a spare player a moment before the cut, started silently about an
eighth of a second early so its video and sound devices are already running, and swapped in at the cut; cuts that simply
continue the same file keep playing without a reload. (A true single-clock render, which removes the last trace of this,
comes with the compositor in Phase 4.) Where no video is under the playhead the preview is blank.

**Empty tracks go away:** when an edit leaves a track with no clips it is removed in the same undo step. "Video 1" and
"Audio 1" always stay, and so do tracks you added yourself and haven't emptied.

**Timeline pictures:** video clips show a filmstrip and every clip with sound shows its waveform (cached in the project's
`cache` folder), all made in the background after a file is read.

**Proxies:** "Create Proxy" makes a small WebM copy (longest side 960 px) in the project's `proxies` folder. The preview plays it
when "Use proxies in the preview" is ticked; export will always use the original.

**Timeline:** drag clips (snaps to edges and the playhead); drop on another track of the same kind, or **over the ruler or
below the last track to give the clip a track of its own** (a teal line shows where it will go). A video clip with sound dropped on an
audio track becomes sound only. Drag edges to trim, mute/lock tracks, Ctrl+scroll to zoom,
Shift+scroll to pan. **Split tool** (B) cuts where you click; **Select** (A) goes back.

**Layers and transforms:** every video clip and photo under the playhead is a layer, stacked by track (the top track is in front).
Select a clip and use the inspector's **Transform** controls: position X/Y, scale, rotation, opacity, mirror and flip. Dragging a slider
shows the change live and records it as one undo step when you let go; Reset puts it back. The clip menu's **Mirror** and **Rotate**
(quarter turn) work too. Pictures are fitted inside the canvas first (letterboxed), so a wide clip in a vertical project
shows bars that lower layers show through.

**Preview:** the frame under the playhead (also while scrubbing), Play/Pause with sound. The picture comes from the
topmost video clip, shown inside a black canvas of the project's exact shape on a neutral surround, so you can see the
video's boundary. **Sound is mixed**: every unmuted clip under the playhead plays together, including audio tracks and the
sound of video clips stacked below the top one. Muting a track silences it.

**Other:** Settings > Appearance (Follow system / Dark / Light), Inspector (clip details, proxy/cache locations, collect
media), File > Move project, Help > Keyboard shortcuts, **Help > About Kani** (icon, version, build date, author, then system information including
the GStreamer hardware decoders available on your machine, with a button to copy it for bug reports).

### Keyboard shortcuts

Defaults follow CapCut's desktop shortcuts where published guides agree ("Convention" = a common editor shortcut we
couldn't confirm for CapCut). The single table lives in `crates/cutroom-core/src/shortcuts.rs`.

| Keys | Action | Source |
|------|--------|--------|
| Space | Play / pause | CapCut |
| Ctrl+B / Ctrl+Shift+B | Split selected clip / split all tracks at the playhead | CapCut |
| Delete | Delete the selected clip (leaves a gap) | CapCut |
| Q / W | Delete everything left / right of the playhead | CapCut |
| A / B | Select tool / Split tool | CapCut |
| Left / Right | Previous / next frame | CapCut |
| Ctrl+Z, Ctrl+Shift+Z | Undo / redo | CapCut |
| Shift+Delete | Ripple delete | Convention |
| Home / End | Go to start / end | Convention |
| Ctrl+= / Ctrl+- | Zoom timeline | Convention |
| Ctrl+C / Ctrl+X / Ctrl+V | Copy, cut, paste (clips, files, pictures) | Convention |
| Ctrl+I, Ctrl+S, Ctrl+N, Ctrl+O, Ctrl+Q | Import, save, new, open, quit | Convention |
| Ctrl+E | Export the video | CapCut |

Ctrl+E opens Export. Not enabled yet: select all, full-screen preview.

## Export

**File > Export video** (or the Export button, or Ctrl+E) opens the Export window, laid out like a typical editor's
export dialog: a picture of the playhead frame and the list of files this will make on the left, the choices on the right.

- **Video** and **Audio** each have a tick box. Video on makes the video file (with its sound inside). Audio on makes the
  project's sound alone as its own file. Both on makes both files.
- **Video**: aspect ratio, and a resolution list (from NTSC to PAL sizes up to 4K, plus the project's own size, which is
  the default for the project's shape). When the chosen shape differs from the project's, pick black bars or crop. Bit
  rate presets (Low, Recommended, High, Maximum, Custom), codec (H.264, H.265, VP9, AV1), file format (MP4, MOV, MKV,
  WebM, only those that can hold the codec), frame rate, and the colour space (Rec. 709 SDR, which is what Kani works in).
- **Audio**: format (AAC, MP3, Opus, FLAC, WAV, as the file format allows), bit rate, and channels (Stereo or Mono;
  Kani mixes to stereo, so surround is not offered).
- **Captions**: save the caption layer as `.srt`, `.vtt` or `.ass` (the `.ass` file takes its font, colours and outline
  from the first caption).
- **Encoder and hardware acceleration**: the encoder (Automatic or a specific one) and whether to put pictures together on
  the graphics card (Automatic / Always / Never).

Kani renders every frame itself with the same compositor as the preview and mixes all audible sound. The window shows
progress and the time left; Cancel (or closing it) stops and removes the half-written files. The originals are always
used, never proxies.

The encoder defaults to **Automatic**: the first one that really works on the machine, tried in this order - NVIDIA
NVENC, Intel Quick Sync, VA-API on Linux (NVENC, Quick Sync, AMD AMF on Windows, VideoToolbox on a Mac), then
software (x264). Each is test-encoded once when the window opens, so a driver that is installed but broken is not offered; if the
chosen one fails during the export, the next one is tried. Needs GStreamer's `x264enc` (gst-plugins-ugly), an AAC encoder
(`avenc_aac` from gst-libav, or `voaacenc`) and `mp4mux`.

## Design rules

1. All timeline changes are `Edit` values; `apply` returns the inverse. Undo/redo, autosave logs and tests use that one path.
2. Time is integer `Ticks` (254,016,000,000 per second): 23.976/29.97/59.94 fps and 44.1/48 kHz audio are exact.
3. Projects are folders the user chooses; media paths are relative and `/`-separated so projects move between OSes.
4. Editing behaviour lives in `cutroom-core` and is tested without a window; the GTK crate is a thin layer.
5. Anything slow (probing, thumbnails, autosave) runs off the UI thread.

## Tools

`tools/footage-report.sh` / `.ps1` list what your footage is (codec, size, frame rate, colour, audio) for tuning;
`tools/make-samples.sh` / `.ps1` cut 10-second samples without re-encoding; `tools/get-sample-video.sh` downloads Big Buck Bunny, and
`CUTROOM_SAMPLE_VIDEO=/path/to/it.mp4 cargo test -p cutroom-gtk preview` plays it across jump cuts in real time. Both need FFmpeg's `ffprobe`/`ffmpeg`.

## Known limits at this stage

* The panels are fixed resizable panes (no docking/tear-off tabs).
* The preview is drawn by a CPU compositor at up to 960 px on the long side; very heavy stacks of layers may play below full speed until the GPU backend exists. In the preview every video layer is decoded even if a layer above hides it (export skips those).
* Playback follows the UI timer, so it can drift slightly from the audio clock on long plays.
* Windows/macOS builds are untried; native Wayland, X11 and Windows drag and drop are GTK's own and unverified on real desktops.
* Clip kind for a new import is read when probing finishes (a moment after import).
* Audio device output couldn't be exercised in the build environment.

## Interface notes

The editor follows a CapCut-like layout. Controls for features that do not exist yet are shown greyed out (tabs,
inspector sections, microphone, group/link, preview ratio). Icons are named in one place
(`crates/cutroom-gtk/src/icons.rs`) and are the Maho icon library (MIT, from Tabler Icons), bundled as SVG and drawn in the current text colour.
The typeface is Inter (SIL OFL 1.1), bundled in `crates/cutroom-gtk/assets/fonts` and registered at start-up (fontconfig on Linux, `AddFontResourceEx` on Windows - the Windows path is untested).
The bar under the timeline is a scroll bar that also zooms: slide the handle to scroll, drag its ends to zoom.
Inspector values can be dragged on the slider or typed into the number beside it.

## License

Kani is free software, licensed under the **GNU General Public License, version 3 or (at your option) any later
version** (`GPL-3.0-or-later`). See [`LICENSE`](LICENSE). Third-party components and their licences are listed in
**Help > About Kani > Credits** (source: `crates/cutroom-core/src/credits.rs`).

## Auto captions

Open the **Captions** category in the library. Pick the spoken language (or let it detect), pick a speech
model and press **Download** (about 78 / 148 / 488 / 539 / 574 MB for Tiny / Base / Small / Medium / Large v3 Turbo, the last two compressed; fetched once into
`<config dir>/models`, or use **Choose file...** with any `ggml-*.bin` Whisper model you already have), pick a look,
and press **Generate captions**. Kani listens to the selected clips (or everything with sound if none are
selected) entirely on your computer and puts one caption per line on a **Captions** track. Each caption is an
ordinary text clip: edit its words, restyle it, split it or move it. The spoken word colour and the highlight
mode (only the spoken word, or the line filling up) are in the clip's Text section. **Export .srt / .vtt** saves
the captions on the timeline as a subtitle file.

Speech recognition is [whisper.cpp](https://github.com/ggml-org/whisper.cpp) through `whisper-rs`; building it
needs `cmake`, a C++ compiler and `libclang`. To build without it: `cargo build -p cutroom-gtk --no-default-features`.
Tiny and Base are fast but often wrong, especially with noise or non-English speech; Medium and Large v3 Turbo are far more accurate and slower on a CPU. Word times are fitted to where the sound actually is, and phrases the model hears in silence are dropped.

## Roadmap

| Phase | Deliverable |
|------:|-------------|
| 1 | Timeline, project folder, platform contracts |
| 2 | App shell: home screen, editing, autosave (done, now on GTK4) |
| 3 | Media core: probing, thumbnails, preview, mixed sound, stills, waveforms, filmstrips, proxies, scrub throttling, decoder reporting (done); still to do: check hardware decoding on real machines and footage, proxy auto-creation, proxy encoder choice per machine |
| 4 | Compositor: **layers, transforms, opacity, on-canvas transform box, crossfades and keyframes (done, CPU)**; deferred: GPU backend, blend modes, skipping decode of fully covered layers |
| 5 | Export: **MP4 (H.264 + AAC) with the same pictures and sound as the preview, optional graphics-card compositing (Automatic / Always / Never in the Export window; Automatic uses the card only when a check found a real graphics chip clearly faster, and falls back to the processor for anything it cannot draw), a redesigned Export window (video and/or separate sound file, NTSC-to-PAL sizes by aspect ratio, bit rate presets, H.264 / H.265 / VP9 / AV1, MP4 / MOV / MKV / WebM, frame rate, AAC / MP3 / Opus / FLAC / WAV, mono or stereo, .srt / .vtt / .ass caption files, encoder and graphics-card choices), progress and cancel, and the best encoder that works on the machine (NVENC, Quick Sync, VA-API, AMF, VideoToolbox, else x264) with automatic fallback**; still to do: the new H.265 / VP9 / AV1 hardware encoders checked on real machines, export of a range or of selected clips, presets for named sites |
| 6 | Text engine and templates: **text clips (font, size, colour, outline, shadow, box, spacing), nine built-in styles, skew and perspective (done)**; still to do: mesh warp / free-form corner pinning, animating skew and perspective, user-saved text styles |
| 7 | Captions with local Whisper, styled word highlighting: **generate captions from the sound of selected clips (or the whole timeline), five caption looks, the spoken word or the line so far lit up as it is said, editable word by word, .srt / .vtt export (done)**; still to do: real-world accuracy checks on your machine, speaker labels, translation, GPU-accelerated recognition |
| 8 | Asset libraries and the pack format, with license tracking |
| 9 | `.cube` LUTs and basic grading |
| 10 | Stickers (SVG, Lottie) |
| 11 | Polish and distribution (installers, signing) |

## Version numbers

`major.phase.iteration`, for example `0.5.16`:

- **major**: the approved official release (0 until one is approved)
- **phase**: the development phase being worked on (4 = compositor and editing UI, 5 = export, 6 = text, 7 = captions)
- **iteration**: the build within that phase, starting at 1

The version lives in one place, `version` in the workspace `Cargo.toml`.
