//! Platform seams for Kani: everything that differs between Windows, Linux
//! and macOS, or between GPU vendors, lives behind the traits and types here.
//!
//! This crate contains **no FFmpeg, wgpu or OS calls yet**. It defines the
//! contracts (decoders, encoders, audio output, GPU enumeration) and the pure
//! decision logic (which hardware backend to use, with graceful fallback), so
//! that logic can be unit-tested on any machine, including CI runners with no GPU.

mod caps;
mod codec;
mod gpu;
mod media;
mod paths;

pub use caps::{choose_backend, CapabilityReport, Direction, HwCapability, Selection};
pub use codec::{Codec, HwBackend, Os};
pub use gpu::{default_compositor_gpu, GpuApi, GpuEnumerator, GpuInfo, GpuKind, GpuVendor};
pub use media::{
    AudioOutput, Decoder, DecoderFactory, DecoderOptions, Encoder, EncoderConfig, EncoderFactory,
    MediaError, PixelFormat, VideoFrame, VideoInfo,
};
pub use paths::{move_settings, portable_dir, AppPaths};
