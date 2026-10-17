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
| Import material in new projects (keep in place / copy) | Disabled, Project settings |
| Frame rate, Resolution, Colour space for new projects | Disabled, Project settings |
| Keep backup copies of projects | Disabled, Phase 11 |

### Edit
| Row | State |
|-----|-------|
| Add dropped files to the timeline | Working (moved from the media panel's Import options) |
| Step forward (frames), Adjust values by (units) | Disabled, Phase 11 |
| Image duration | Disabled, Phase 11 |
| Target loudness | Disabled, Phase 11 |
| Play sound while dragging the playhead | Disabled, Phase 3 |
| Layers can be reordered in new projects | Disabled, Phase 11 |
| Snap to clips and the playhead | Disabled, Phase 11 |
| Time code format | Disabled, Phase 11 |

### Playback
| Row | State |
|-----|-------|
| Use proxies in the preview | Working (moved from the media panel's Import options) |
| Create proxies automatically, Proxy quality | Disabled, Phase 3 |
| Decode with the graphics chip | Disabled, Phase 3 (hardware-decode checks) |
| Remux unsupported files on import | Disabled, Phase 3 (Kani has no remux step yet) |
| Preview resolution | Disabled, Phase 3 |

### Performance
| Row | State |
|-----|-------|
| Compositor acceleration (Automatic / Graphics chip / Processor) | Working for export. It is the same stored setting as the Export window's Acceleration row, which stays there as the quick per-export view. |
| Preview compositing on the graphics chip | Disabled, Phase 4 (GPU backend) |
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
  mode (copy or reference) stays there until Project settings arrives.

Added: Ctrl+, shortcut; autosave on/off separate from its interval (old files saying 0 minutes still mean off);
interval any value 1 to 120 minutes; Compositor acceleration and Export folder as settings in one place.

Kept in two views on purpose, over one stored value: Acceleration (Performance tab and the Export window).
Kept where used: Export window choices, Auto-Captions language/model/look, the Library and Home thumbnail/list
toggles (view switches, not preferences).
