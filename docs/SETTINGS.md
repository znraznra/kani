# Settings

Open it from **Settings > Settings...** or with **Ctrl+,**. Choices are staged: they take effect with **Save**
and are dropped by **Cancel**, Esc or closing the window. Rows for things Kani cannot do yet are shown but
disabled, each naming what brings it ("Not available yet - Phase 3").

Preferences are stored in `settings.json` in the OS config folder (never in a project). Settings that belong to
one place of work stay where they are used: the Export window keeps its own format, quality, sound and caption
choices, and the Auto-Captions panel keeps the spoken language, speech model and look. Their tabs say so.

Phase: the window itself is part of Phase 2 (app shell, done); its rows follow the phase that makes them real.

## Sections and rows (left sidebar, content on the right)

Working = does something now. Disabled = shown, waiting for the phase named.

### General
| Row | State |
|-----|-------|
| Appearance (Follow system / Dark / Light) | Working (moved here from the Settings menu) |
| Keyboard shortcuts (View...) | Working, opens the shortcut list |
| Language | Disabled, Phase 11 |
| Check for updates automatically | Disabled, Phase 11 |

### Draft
| Row | State |
|-----|-------|
| Projects folder (Change..., Use default) | Working (moved from the home screen, which now points here) |
| Autosave on/off | Working (moved from the Settings menu; switching off keeps the interval) |
| Autosave interval, 1 to 120 minutes | Working (the menu offered only 1, 5, 10, 30) |
| Import material (keep in place / copy) | Working, per project: Info tab > Project settings (moved from the media panel) |
| Frame rate, Resolution, Colour space for new projects | Disabled, Project settings |
| Keep backup copies of projects | Disabled, Phase 11 |

### Edit
| Row | State |
|-----|-------|
| Add dropped files to the timeline | Working (moved from the media panel's Import options) |
| Step forward (frames), Adjust values by (units) | Disabled, Phase 11 |
| Image duration | Disabled, Phase 11 |
| Target loudness | Disabled, Phase 11 |
| Play sound while dragging the playhead | Working (also the speaker button on the timeline toolbar, next to Snap): a short piece of sound plays at each move of the playhead |
| Layers can be reordered in new projects | Disabled, Phase 11 |
| Snap to clips and the playhead | Disabled, Phase 11 |
| Time code format | Disabled, Phase 11 |

### Playback
| Row | State |
|-----|-------|
| Use proxies in the preview | Working (moved from the media panel's Import options) |
| Create proxies automatically | Working: a video bigger than the proxy size gets a proxy as soon as it has been read |
| Proxy quality (Small 640 px / Balanced 960 px / Sharp 1280 px) | Working, for proxies made from now on |
| Decode with the graphics chip | Working: off lowers the rank of VA-API / NVIDIA / Direct3D / QSV / V4L2 video decoders so the processor decodes. It applies to preview and export, to files opened after the change. Not yet checked on real machines. |
| Remux unsupported files on import | Working: .ts .mts .m2ts .flv .f4v .wmv .asf .mpg .mpeg .vob .3gp .3g2 files are re-wrapped into .mkv in the project's media folder (no re-encoding) and imported from there |
| Preview resolution (Low 480 / Standard 960 / High 1440 / Full 1920 px) | Working: longest side of decoded and drawn preview pictures; export is unaffected |

### Performance
| Row | State |
|-----|-------|
| Compositor acceleration (Automatic / Graphics chip / Processor) | Working for export. It is the same stored setting as the Export window's Acceleration row, which stays there as the quick per-export view. |
| Preview compositing on the graphics chip | Working (off by default): the preview picture is put together on the graphics chip on a thread of its own; if the chip cannot be used it falls back to the processor and says so in the status bar. Corner pin and mesh warp are always done by the processor. |
| Processor threads, Memory limit, Cache folder, Clear the cache | Disabled, Phase 11 |

### Export
| Row | State |
|-----|-------|
| Export folder (Change..., Use default) | Working |
| Play a sound / open the folder when an export finishes | Disabled, Phase 11 |

### Captions
Language, model and look: in the Auto-Captions panel. Disabled here: Label speakers, Translate captions,
Recognise speech on the graphics chip, Speech model folder (Phase 7 open items).

### Libraries
All disabled: asset library folder and packs (Phase 8), LUT folder and grading colour space (Phase 9),
sticker folder (Phase 10).

### Advanced
| Row | State |
|-----|-------|
| Where settings are stored | Shown |
| Restore defaults | Working (fills the window with the starting values; Save applies them; format choices and history are kept) |
| Diagnostic log, crash reports | Disabled, Phase 11 |

## What changed (build 62, 0.5.26)

Moved into the Settings window (and removed from where they were):
- Settings menu: Appearance and Autosave submenus. The menu now has one entry, "Settings...".
- Home screen: "Change..." / "Use default" for the projects folder. The home screen still shows the folder, with a
  "Change in Settings..." button that opens the Draft section.
- Media panel, Import options: "Also add dropped files to the timeline" and "Use proxies in the preview". Import
  mode (copy or reference) now lives in Project settings.

Added: Ctrl+, shortcut; autosave on/off separate from its interval (old files saying 0 minutes still mean off);
interval any value 1 to 120 minutes; Compositor acceleration and Export folder as settings in one place.

Kept in two views on purpose, over one stored value: Acceleration (Performance tab and the Export window).
Kept where used: Export window choices, Auto-Captions language/model/look, the Library and Home thumbnail/list
toggles (view switches, not preferences).

## Project settings (0.5.28)

Info tab > "Project settings" opens a window with two tabs, staged Save/Cancel:

- Details: project name, project path (Move... relocates the whole folder, applied on Save), import material (keep in original place / copy into the project; optionally copy and relink the media already in the project), resolution (presets or custom, even numbers 64-8192), frame rate, color space (Rec. 709, sRGB, Display P3, Rec. 2020, Rec. 2100 PQ/HLG).
- Performance: placeholders (Preview resolution, Render cache, Proxy files, Hardware decoding), disabled until defined.

Color space is stored in the project only; colour-managed rendering is Phase 9. Format and name changes are not undoable. The media panel's "Import options" fold-out was removed as redundant.

## 0.5.33 changes

- The hardware decoding check window keeps one size from start to report (no more "Trying to measure GtkWindow" warnings from it).
- Dragging a track by its name scrolls the track list near the top or bottom.

## 0.5.32 changes

- The hardware decoding check window has a fixed width and a progress bar; processor time is compared per picture.
- Interface text sizes, button padding and spacing follow docs/UI_STYLE.md everywhere (Settings margins 14 px).

## 0.5.31 changes

- Playback > **Check hardware decoding** > Test on a video...: decodes a real file with and without the graphics chip and reports the
  decoders used, speed and processor time (also in the media right-click menu).
- Changing the **Proxy files** or **Render cache** folder in Project settings moves this project's existing proxies and waveform
  caches there (nothing is left behind or made twice; a file already in the new folder is kept and the old copy removed).
- Preview compositing on the graphics chip no longer makes the window wait for each picture.

## 0.5.30 changes

- The **Proxy files** and **Render cache** folders moved from the Info tab into **Project settings > Performance**
  (Change... / Use project folder, applied on Save). The Info tab keeps Collect media and Check media.
- Player menu: **Lock guides** (saved as a preference), above Reset guides.
- Library items with a proxy show a small **Proxy** label (top-right of the thumbnail, or beside the name in the list).
- Preview compositing on the graphics chip now draws corner pin and mesh warp on the chip too (a fine triangle grid), so
  a warped layer no longer sends the whole picture back to the processor. Export's graphics-chip mode benefits the same way.
