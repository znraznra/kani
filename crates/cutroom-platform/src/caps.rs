use crate::codec::{Codec, HwBackend};
use crate::gpu::GpuInfo;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    Encode,
    Decode,
}

/// One thing the hardware can do, as found by a startup probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HwCapability {
    pub backend: HwBackend,
    pub codec: Codec,
    pub direction: Direction,
    pub max_width: u32,
    pub max_height: u32,
    pub ten_bit: bool,
}

/// What this machine can actually do. Built by a probe (later: by trying to
/// open real FFmpeg hardware contexts), shown in settings, and fed to
/// [`choose_backend`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityReport {
    pub gpus: Vec<GpuInfo>,
    pub hardware: Vec<HwCapability>,
}

impl CapabilityReport {
    pub fn supports(&self, direction: Direction, backend: HwBackend, codec: Codec) -> bool {
        backend == HwBackend::Software
            || self
                .hardware
                .iter()
                .any(|c| c.backend == backend && c.codec == codec && c.direction == direction)
    }
}

impl fmt::Display for CapabilityReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "GPUs:")?;
        if self.gpus.is_empty() {
            writeln!(f, "  (none found)")?;
        }
        for gpu in &self.gpus {
            writeln!(
                f,
                "  {} [{:?}, {:?}, {:?}]",
                gpu.name, gpu.vendor, gpu.kind, gpu.api
            )?;
        }
        writeln!(f, "Hardware codecs:")?;
        if self.hardware.is_empty() {
            writeln!(f, "  (none; software only)")?;
        }
        for cap in &self.hardware {
            writeln!(
                f,
                "  {} {:?} {:?} up to {}x{}{}",
                cap.backend.label(),
                cap.codec,
                cap.direction,
                cap.max_width,
                cap.max_height,
                if cap.ten_bit { " (10-bit)" } else { "" }
            )?;
        }
        Ok(())
    }
}

/// The backend to use, and why it isn't the one the user asked for (if so).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub backend: HwBackend,
    /// `Some` when we fell back; suitable to show the user as-is.
    pub fallback_reason: Option<String>,
}

/// Choose where to run encode or decode work.
///
/// * `forced`: a backend the user picked in settings.
/// * `preference`: ordered hardware backends to try (see `Os::default_backend_order`).
///
/// Never fails: software is always available, and any fallback carries a
/// human-readable reason so the UI can explain it instead of just erroring.
pub fn choose_backend(
    report: &CapabilityReport,
    direction: Direction,
    codec: Codec,
    preference: &[HwBackend],
    forced: Option<HwBackend>,
) -> Selection {
    let verb = match direction {
        Direction::Encode => "encoding",
        Direction::Decode => "decoding",
    };

    let mut note = None;
    if let Some(requested) = forced {
        if report.supports(direction, requested, codec) {
            return Selection {
                backend: requested,
                fallback_reason: None,
            };
        }
        note = Some(format!(
            "{} cannot do {codec:?} {verb} on this machine",
            requested.label()
        ));
    }

    let auto = preference
        .iter()
        .copied()
        .find(|b| *b != HwBackend::Software && report.supports(direction, *b, codec));

    match auto {
        Some(backend) => Selection {
            backend,
            fallback_reason: note.map(|n| format!("{n}; using {}", backend.label())),
        },
        None => Selection {
            backend: HwBackend::Software,
            fallback_reason: Some(match note {
                Some(n) => format!("{n}; using software"),
                None => format!("no hardware backend supports {codec:?} {verb}; using software"),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::Os;

    fn cap(backend: HwBackend, codec: Codec, direction: Direction) -> HwCapability {
        HwCapability {
            backend,
            codec,
            direction,
            max_width: 4096,
            max_height: 4096,
            ten_bit: false,
        }
    }

    /// Dell Precision 3530 style: NVENC (H.264/HEVC) plus Intel QSV.
    fn hybrid_laptop() -> CapabilityReport {
        let mut hardware = Vec::new();
        for codec in [Codec::H264, Codec::Hevc] {
            for direction in [Direction::Encode, Direction::Decode] {
                hardware.push(cap(HwBackend::Nvenc, codec, direction));
                hardware.push(cap(HwBackend::Qsv, codec, direction));
            }
        }
        CapabilityReport {
            gpus: vec![],
            hardware,
        }
    }

    /// ThinkPad Iris Xe style: QSV with AV1 decode only.
    fn xe_laptop() -> CapabilityReport {
        let mut hardware = Vec::new();
        for codec in [Codec::H264, Codec::Hevc] {
            for direction in [Direction::Encode, Direction::Decode] {
                hardware.push(cap(HwBackend::Qsv, codec, direction));
            }
        }
        hardware.push(cap(HwBackend::Qsv, Codec::Av1, Direction::Decode));
        CapabilityReport {
            gpus: vec![],
            hardware,
        }
    }

    #[test]
    fn auto_follows_preference_order() {
        let order = Os::Windows.default_backend_order();
        let sel = choose_backend(
            &hybrid_laptop(),
            Direction::Encode,
            Codec::H264,
            order,
            None,
        );
        assert_eq!(sel.backend, HwBackend::Nvenc);
        assert_eq!(sel.fallback_reason, None);

        let sel = choose_backend(&xe_laptop(), Direction::Encode, Codec::H264, order, None);
        assert_eq!(sel.backend, HwBackend::Qsv);
    }

    #[test]
    fn unsupported_codec_falls_back_to_software_with_reason() {
        let order = Os::Windows.default_backend_order();
        let sel = choose_backend(&xe_laptop(), Direction::Encode, Codec::Av1, order, None);
        assert_eq!(sel.backend, HwBackend::Software);
        assert!(sel.fallback_reason.unwrap().contains("Av1"));
    }

    #[test]
    fn decode_and_encode_are_judged_separately() {
        let order = Os::Linux.default_backend_order();
        let dec = choose_backend(&xe_laptop(), Direction::Decode, Codec::Av1, order, None);
        assert_eq!(dec.backend, HwBackend::Qsv);
        let enc = choose_backend(&xe_laptop(), Direction::Encode, Codec::Av1, order, None);
        assert_eq!(enc.backend, HwBackend::Software);
    }

    #[test]
    fn honoured_when_the_forced_backend_works() {
        let order = Os::Windows.default_backend_order();
        let sel = choose_backend(
            &hybrid_laptop(),
            Direction::Encode,
            Codec::Hevc,
            order,
            Some(HwBackend::Qsv),
        );
        assert_eq!(sel.backend, HwBackend::Qsv);
        assert_eq!(sel.fallback_reason, None);
    }

    #[test]
    fn broken_forced_backend_falls_back_and_explains() {
        let order = Os::Windows.default_backend_order();
        let sel = choose_backend(
            &xe_laptop(),
            Direction::Encode,
            Codec::H264,
            order,
            Some(HwBackend::Nvenc),
        );
        assert_eq!(sel.backend, HwBackend::Qsv);
        let reason = sel.fallback_reason.unwrap();
        assert!(
            reason.contains("NVENC") && reason.contains("Quick Sync"),
            "{reason}"
        );
    }

    #[test]
    fn forcing_software_is_always_honoured() {
        let sel = choose_backend(
            &CapabilityReport::default(),
            Direction::Encode,
            Codec::H264,
            &[],
            Some(HwBackend::Software),
        );
        assert_eq!(sel.backend, HwBackend::Software);
        assert_eq!(sel.fallback_reason, None);
    }

    #[test]
    fn empty_report_means_software() {
        let sel = choose_backend(
            &CapabilityReport::default(),
            Direction::Decode,
            Codec::H264,
            Os::current().default_backend_order(),
            None,
        );
        assert_eq!(sel.backend, HwBackend::Software);
        assert!(sel.fallback_reason.is_some());
    }

    #[test]
    fn report_prints_a_readable_summary() {
        let text = xe_laptop().to_string();
        assert!(text.contains("Intel Quick Sync") && text.contains("Av1"));
    }
}
