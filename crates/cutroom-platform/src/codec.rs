use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Codec {
    H264,
    Hevc,
    Vp9,
    Av1,
}

/// Where video encode/decode work runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HwBackend {
    /// CPU. Always available; the fallback of last resort.
    Software,
    /// NVIDIA NVENC / NVDEC.
    Nvenc,
    /// Intel Quick Sync Video.
    Qsv,
    /// VA-API (Linux; Intel and AMD).
    Vaapi,
    /// AMD AMF (Windows).
    Amf,
    /// Apple VideoToolbox.
    VideoToolbox,
}

impl HwBackend {
    pub fn label(self) -> &'static str {
        match self {
            HwBackend::Software => "Software (CPU)",
            HwBackend::Nvenc => "NVIDIA NVENC/NVDEC",
            HwBackend::Qsv => "Intel Quick Sync",
            HwBackend::Vaapi => "VA-API",
            HwBackend::Amf => "AMD AMF",
            HwBackend::VideoToolbox => "Apple VideoToolbox",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Os {
    Windows,
    Linux,
    MacOs,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }

    /// Default order in which hardware backends are tried. Users can override.
    /// `Software` is implicit at the end and not listed.
    pub fn default_backend_order(self) -> &'static [HwBackend] {
        match self {
            Os::Windows => &[HwBackend::Nvenc, HwBackend::Qsv, HwBackend::Amf],
            Os::Linux => &[HwBackend::Nvenc, HwBackend::Vaapi, HwBackend::Qsv],
            Os::MacOs => &[HwBackend::VideoToolbox],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_os_has_a_hardware_order_without_software() {
        for os in [Os::Windows, Os::Linux, Os::MacOs] {
            let order = os.default_backend_order();
            assert!(!order.is_empty());
            assert!(!order.contains(&HwBackend::Software));
        }
    }

    #[test]
    fn platform_specific_backends_stay_on_their_platform() {
        assert!(!Os::Windows
            .default_backend_order()
            .contains(&HwBackend::Vaapi));
        assert!(!Os::Linux.default_backend_order().contains(&HwBackend::Amf));
        assert!(!Os::MacOs
            .default_backend_order()
            .contains(&HwBackend::Nvenc));
    }
}
