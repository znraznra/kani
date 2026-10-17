//! Credits and licences for everything Kani is built from or uses.
//! Shown in the About window's Credits tab. Keep this list in step with
//! the dependencies and any bundled asset.

/// One credited element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Credit {
    pub name: &'static str,
    pub what: &'static str,
    pub license: &'static str,
    pub url: &'static str,
}

/// A titled group of credits.
#[derive(Debug, Clone, Copy)]
pub struct Section {
    pub title: &'static str,
    pub note: &'static str,
    pub entries: &'static [Credit],
}

pub const LICENSE_NAME: &str = "GPL-3.0-or-later";
pub const LICENSE_SUMMARY: &str = "Kani is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. It is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY.";

const fn c(
    name: &'static str,
    what: &'static str,
    license: &'static str,
    url: &'static str,
) -> Credit {
    Credit {
        name,
        what,
        license,
        url,
    }
}

pub const SECTIONS: &[Section] = &[
    Section {
        title: "Kani",
        note: "Developed by znraznra. An independent, clean-room project.",
        entries: &[
            c("Kani", "The editor itself", LICENSE_NAME, ""),
            c(
                "Kani logo and app icon",
                "The logo, hand-drawn by znraznra",
                LICENSE_NAME,
                "",
            ),
        ],
    },
    Section {
        title: "Toolkit and media framework",
        note: "Linked or loaded at run time.",
        entries: &[
            c(
                "GTK 4",
                "User interface toolkit",
                "LGPL-2.1-or-later",
                "https://www.gtk.org",
            ),
            c(
                "GStreamer",
                "Decoding, playback and (soon) export",
                "LGPL-2.1-or-later",
                "https://gstreamer.freedesktop.org",
            ),
            c(
                "GStreamer plugins (base, good, bad)",
                "Demuxers, decoders, encoders",
                "LGPL-2.1-or-later (some plugins GPL)",
                "https://gstreamer.freedesktop.org",
            ),
            c(
                "FFmpeg / libav",
                "Codecs used through GStreamer's libav plugin",
                "LGPL-2.1-or-later (GPL if built with GPL parts)",
                "https://ffmpeg.org",
            ),
            c(
                "GLib, GObject, Pango, Cairo",
                "Core libraries beneath GTK",
                "LGPL-2.1-or-later / MPL-1.1",
                "https://www.gtk.org",
            ),
        ],
    },
    Section {
        title: "Rust language bindings and libraries",
        note: "Compiled into the program.",
        entries: &[
            c(
                "Rust and its standard library",
                "Language and runtime",
                "MIT OR Apache-2.0",
                "https://www.rust-lang.org",
            ),
            c(
                "gtk4-rs",
                "Rust bindings for GTK 4",
                "MIT",
                "https://gtk-rs.org",
            ),
            c(
                "gstreamer-rs",
                "Rust bindings for GStreamer",
                "MIT OR Apache-2.0",
                "https://gitlab.freedesktop.org/gstreamer/gstreamer-rs",
            ),
            c(
                "gst-plugin-gtk4",
                "Video sink that draws into GTK",
                "MPL-2.0",
                "https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs",
            ),
            c(
                "whisper.cpp",
                "Speech recognition that runs on this computer (auto captions)",
                "MIT",
                "https://github.com/ggml-org/whisper.cpp",
            ),
            c(
                "whisper-rs",
                "Rust bindings for whisper.cpp",
                "Unlicense",
                "https://codeberg.org/tazz4843/whisper-rs",
            ),
            c(
                "Silero VAD",
                "Small voice detector (tells speech from music and silence), downloaded when you generate captions",
                "MIT",
                "https://github.com/snakers4/silero-vad",
            ),
            c(
                "OpenAI Whisper models",
                "Speech models, downloaded only when you ask for them (not bundled)",
                "MIT",
                "https://github.com/openai/whisper",
            ),
            c(
                "ureq",
                "Downloads the speech models",
                "MIT OR Apache-2.0",
                "https://github.com/algesten/ureq",
            ),
            c(
                "serde, serde_json",
                "Project file reading and writing",
                "MIT OR Apache-2.0",
                "https://serde.rs",
            ),
            c(
                "thiserror",
                "Error types",
                "MIT OR Apache-2.0",
                "https://github.com/dtolnay/thiserror",
            ),
            c(
                "blake3",
                "File fingerprints",
                "CC0-1.0 OR Apache-2.0",
                "https://github.com/BLAKE3-team/BLAKE3",
            ),
            c(
                "directories",
                "Standard folder locations",
                "MIT OR Apache-2.0",
                "https://github.com/dirs-dev/directories-rs",
            ),
            c(
                "tempfile (tests only)",
                "Temporary folders in tests",
                "MIT OR Apache-2.0",
                "https://github.com/Stebalien/tempfile",
            ),
        ],
    },
    Section {
        title: "Test material and inspiration",
        note: "Not shipped inside the program.",
        entries: &[
            c(
                "Big Buck Bunny",
                "Sample clip used to test playback",
                "CC BY 3.0, (c) Blender Foundation | peach.blender.org",
                "https://peach.blender.org",
            ),
            c(
                "CapCut",
                "Layout and shortcut conventions followed for familiarity. No CapCut code or artwork is used.",
                "Trademark of its owner; no affiliation",
                "",
            ),
        ],
    },
    Section {
        title: "Typeface",
        note: "Bundled with the program (crates/cutroom-gtk/assets/fonts, with the licence text).",
        entries: &[c(
            "Inter",
            "The interface typeface, Copyright 2016-2023 The Inter Project Authors",
            "SIL Open Font License 1.1",
            "https://rsms.me/inter",
        )],
    },
    Section {
        title: "Icons",
        note: "Bundled with the program (crates/cutroom-gtk/assets/icons, with the licence text).",
        entries: &[
            c(
                "Maho icon library",
                "Interface icons (an optimised copy of Tabler Icons), Copyright (c) 2020-2024 Pawel Kuna",
                "MIT",
                "https://github.com/MahoCommerce/icons",
            ),
            c(
                "Tabler Icons",
                "The original icon set the Maho library is built from",
                "MIT",
                "https://tabler.io/icons",
            ),
        ],
    },
];

/// The credits as plain text, for copying.
pub fn as_text() -> String {
    let mut out = format!("Kani is licensed under {LICENSE_NAME}.\n\n");
    for s in SECTIONS {
        out.push_str(&format!("{}\n{}\n", s.title, s.note));
        for e in s.entries {
            out.push_str(&format!("  - {}: {} [{}]", e.name, e.what, e.license));
            if !e.url.is_empty() {
                out.push_str(&format!(" {}", e.url));
            }
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_credit_names_what_it_is_and_its_licence() {
        for s in SECTIONS {
            assert!(!s.title.is_empty());
            for e in s.entries {
                assert!(!e.name.is_empty() && !e.what.is_empty() && !e.license.is_empty());
                assert!(e.url.is_empty() || e.url.starts_with("https://"));
            }
        }
    }

    #[test]
    fn the_text_form_mentions_the_project_licence_and_each_entry() {
        let t = as_text();
        assert!(t.contains(LICENSE_NAME));
        for s in SECTIONS {
            for e in s.entries {
                assert!(t.contains(e.name));
            }
        }
    }

    #[test]
    fn every_workspace_dependency_is_credited() {
        let t = as_text();
        for dep in [
            "gtk4-rs",
            "gstreamer-rs",
            "gst-plugin-gtk4",
            "serde",
            "thiserror",
            "blake3",
            "directories",
        ] {
            assert!(t.contains(dep), "{dep}");
        }
    }
}
