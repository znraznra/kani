//! Switch the graphics chip's video decoders on or off for this program.
//!
//! GStreamer picks decoders by rank. Hardware decoders (VA, NVIDIA, Direct3D,
//! Intel QSV, V4L2 stateless) normally outrank software ones. Turning
//! "Decode with the graphics chip" off drops their rank to "never", so the
//! processor decodes. It applies to everything this program decodes (preview
//! and export) from the next time a file is opened.

use gstreamer as gst;
use gstreamer::prelude::*;
use std::collections::HashMap;
use std::sync::Mutex;

/// Ranks before they were lowered, so turning decoding back on restores them.
static ORIGINAL: Mutex<Option<HashMap<String, gst::Rank>>> = Mutex::new(None);

/// Is this decoder element one that runs on the graphics chip?
pub fn is_hardware_decoder(name: &str) -> bool {
    const PREFIXES: [&str; 8] = [
        "vaapi", "va", "nv", "d3d11", "d3d12", "qsv", "msdk", "v4l2sl",
    ];
    PREFIXES.iter().any(|p| {
        name.strip_prefix(p).is_some_and(|rest| {
            // "va" must not catch "vahdr" style neighbours: a decoder name
            // always ends in "dec".
            rest.ends_with("dec") || rest.contains("dec")
        })
    })
}

/// Turn hardware decoding on (`true`) or off. Returns how many decoders it changed.
pub fn apply(enabled: bool) -> usize {
    if gst::init().is_err() {
        return 0;
    }
    let mut guard = ORIGINAL.lock().unwrap_or_else(|e| e.into_inner());
    let saved = guard.get_or_insert_with(HashMap::new);
    let mut changed = 0;
    for feature in gst::Registry::get().features(gst::ElementFactory::static_type()) {
        let Some(factory) = feature.downcast_ref::<gst::ElementFactory>() else {
            continue;
        };
        let klass = factory.klass();
        if !(klass.contains("Decoder") && klass.contains("Video")) {
            continue;
        }
        let name = feature.name().to_string();
        if !is_hardware_decoder(&name) {
            continue;
        }
        if enabled {
            if let Some(rank) = saved.remove(&name) {
                feature.set_rank(rank);
                changed += 1;
            }
        } else if feature.rank() != gst::Rank::NONE {
            saved.insert(name, feature.rank());
            feature.set_rank(gst::Rank::NONE);
            changed += 1;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_decoder_names_are_told_from_software_ones() {
        for hw in [
            "vah264dec",
            "vaapih264dec",
            "nvh264dec",
            "nvh264sldec",
            "d3d11h265dec",
            "qsvh264dec",
            "v4l2slh264dec",
            "vavp9dec",
        ] {
            assert!(is_hardware_decoder(hw), "{hw}");
        }
        for sw in [
            "avdec_h264",
            "vp8dec",
            "vp9dec",
            "dav1d",
            "openh264dec",
            "theoradec",
        ] {
            assert!(!is_hardware_decoder(sw), "{sw}");
        }
    }

    #[test]
    fn turning_it_off_and_on_again_restores_every_rank() {
        let _ = apply(true);
        let before: Vec<(String, gst::Rank)> = gst::Registry::get()
            .features(gst::ElementFactory::static_type())
            .iter()
            .map(|f| (f.name().to_string(), f.rank()))
            .collect();
        let _ = apply(false);
        let _ = apply(true);
        let after: Vec<(String, gst::Rank)> = gst::Registry::get()
            .features(gst::ElementFactory::static_type())
            .iter()
            .map(|f| (f.name().to_string(), f.rank()))
            .collect();
        assert_eq!(before, after);
    }
}
