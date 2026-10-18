# Kani interface style

One look for every panel, window and row, taken from the media panel (Import,
Local / Import / Library, the media list). New interface follows these numbers.
The stylesheet is in `crates/cutroom-gtk/src/theme.rs`; the spacing constants
are in `crates/cutroom-gtk/src/ui.rs`.

## Text

| Use | Size | Weight |
|---|---|---|
| Body text, list rows, button labels | 13 px | normal |
| Small controls: side lists, tab captions, field labels, pills | 12 px (tabs 10 px) | normal |
| Notes under a control (`.caption`) | 11 px | normal, dimmed |
| Section headings (`.heading`, `.cc-section-title`, `.panel-title`) | 13 px | bold |
| Page titles (`.title-4` / `.title-3` / `.title-2`) | 14 / 16 / 18 px | bold |

Font: Inter (bundled). Numbers that change (timecodes) use tabular figures.

## Buttons

* Text buttons: 24 px tall, 3 x 12 px padding, 6 px corners (`.text-button`).
* Icon buttons: 24 x 24 px, 16 px icon, flat, tooltip with what it does and its shortcut
  (`icons::button`, `icons::toggle`). Prefer an icon with a tooltip over a text button in toolbars.
* Side lists (Local / Import / Library, the home sidebar): 12 px text, 4 x 10 px padding, 5 px corners.
* Drop-downs: 24 px tall, names cut short with "..." instead of widening their panel (`ui::dropdown`).
* Main action: `suggested-action`; destructive: `destructive-action`.

## Spacing

* Between controls: 6 px (`ui::GAP`); between buttons in one toolbar: 4 px (`ui::TIGHT`).
* Panel and page margins: 8 px (`ui::pad`).
* Dialog margins: 12 px top and bottom, 14 px left and right (`ui::pad_dialog`, `dialogs::content_box`).
* Dialogs have a fixed width when their content allows (Replace clip 440 px, hardware check 440 px).

## Icons

Tabler-style outline icons from `assets/icons`, 16 px in buttons and lists (18 px only in the
library category strip), drawn in the text colour so they follow the theme. On dark overlays
(full screen controls) icons and text are white.
