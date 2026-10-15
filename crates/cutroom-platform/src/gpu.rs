use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GpuVendor {
    Nvidia,
    Intel,
    Amd,
    Apple,
    Other,
}

impl GpuVendor {
    /// Map a PCI vendor id (as reported by Vulkan/wgpu) to a vendor.
    pub fn from_pci_vendor_id(id: u32) -> GpuVendor {
        match id {
            0x10DE => GpuVendor::Nvidia,
            0x8086 => GpuVendor::Intel,
            0x1002 | 0x1022 => GpuVendor::Amd,
            0x106B => GpuVendor::Apple,
            _ => GpuVendor::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GpuKind {
    Integrated,
    Discrete,
    /// Software rasterizers such as llvmpipe or WARP.
    Virtual,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GpuApi {
    Vulkan,
    Dx12,
    Metal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuInfo {
    /// Stable-enough identifier to remember the user's choice (vendor:device:name).
    pub id: String,
    pub name: String,
    pub vendor: GpuVendor,
    pub kind: GpuKind,
    pub api: GpuApi,
    pub vram_bytes: Option<u64>,
}

/// Implemented later on top of `wgpu` (adapter enumeration).
pub trait GpuEnumerator {
    fn enumerate(&self) -> Vec<GpuInfo>;
}

/// Pick a sensible compositing GPU when the user hasn't chosen one:
/// discrete over integrated over unknown, more VRAM breaks ties, and software
/// rasterizers only when nothing else exists.
pub fn default_compositor_gpu(gpus: &[GpuInfo]) -> Option<&GpuInfo> {
    let rank = |g: &GpuInfo| match g.kind {
        GpuKind::Discrete => 3,
        GpuKind::Integrated => 2,
        GpuKind::Unknown => 1,
        GpuKind::Virtual => 0,
    };
    gpus.iter()
        .max_by_key(|g| (rank(g), g.vram_bytes.unwrap_or(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    fn gpu(name: &str, vendor: GpuVendor, kind: GpuKind, vram: Option<u64>) -> GpuInfo {
        GpuInfo {
            id: name.to_owned(),
            name: name.to_owned(),
            vendor,
            kind,
            api: GpuApi::Vulkan,
            vram_bytes: vram,
        }
    }

    #[test]
    fn vendor_ids_map_correctly() {
        assert_eq!(GpuVendor::from_pci_vendor_id(0x10DE), GpuVendor::Nvidia);
        assert_eq!(GpuVendor::from_pci_vendor_id(0x8086), GpuVendor::Intel);
        assert_eq!(GpuVendor::from_pci_vendor_id(0x1002), GpuVendor::Amd);
        assert_eq!(GpuVendor::from_pci_vendor_id(0xBEEF), GpuVendor::Other);
    }

    #[test]
    fn hybrid_laptop_prefers_the_discrete_gpu() {
        let gpus = [
            gpu(
                "Intel UHD Graphics 630",
                GpuVendor::Intel,
                GpuKind::Integrated,
                None,
            ),
            gpu(
                "NVIDIA Quadro P600",
                GpuVendor::Nvidia,
                GpuKind::Discrete,
                Some(4 * GB),
            ),
        ];
        assert_eq!(
            default_compositor_gpu(&gpus).unwrap().vendor,
            GpuVendor::Nvidia
        );
    }

    #[test]
    fn integrated_only_machine_uses_it() {
        let gpus = [gpu(
            "Intel Iris Xe",
            GpuVendor::Intel,
            GpuKind::Integrated,
            None,
        )];
        assert_eq!(default_compositor_gpu(&gpus).unwrap().name, "Intel Iris Xe");
    }

    #[test]
    fn software_rasterizer_is_last_resort() {
        let only_soft = [gpu("llvmpipe", GpuVendor::Other, GpuKind::Virtual, None)];
        assert!(default_compositor_gpu(&only_soft).is_some());

        let mixed = [
            gpu("llvmpipe", GpuVendor::Other, GpuKind::Virtual, None),
            gpu("Intel Iris Xe", GpuVendor::Intel, GpuKind::Integrated, None),
        ];
        assert_eq!(
            default_compositor_gpu(&mixed).unwrap().name,
            "Intel Iris Xe"
        );
    }

    #[test]
    fn more_vram_wins_between_discrete_gpus() {
        let gpus = [
            gpu("small", GpuVendor::Nvidia, GpuKind::Discrete, Some(4 * GB)),
            gpu("big", GpuVendor::Amd, GpuKind::Discrete, Some(16 * GB)),
        ];
        assert_eq!(default_compositor_gpu(&gpus).unwrap().name, "big");
    }

    #[test]
    fn no_gpus_means_none() {
        assert!(default_compositor_gpu(&[]).is_none());
    }
}
