use crate::caps::Direction;
use crate::codec::{Codec, HwBackend};
use cutroom_timeline::{FrameRate, Ticks};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("encode failed: {0}")]
    Encode(String),
    #[error("no usable {0:?} backend")]
    NoBackend(Direction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba8,
    Yuv420p,
    Nv12,
    /// 10-bit semi-planar.
    P010,
}

/// CPU-side frame. GPU-resident frames (zero-copy paths) come later as an
/// additional variant behind the same traits.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    /// Presentation time on the source's own timeline.
    pub pts: Ticks,
    pub format: PixelFormat,
    pub planes: Vec<Vec<u8>>,
    pub strides: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    pub duration: Ticks,
    pub codec: Option<Codec>,
}

#[derive(Debug, Clone, Default)]
pub struct DecoderOptions {
    /// Try this backend first; the factory falls back to software if it fails.
    pub prefer: Option<HwBackend>,
}

pub trait Decoder: Send {
    fn info(&self) -> &VideoInfo;
    /// The backend actually in use (may differ from the one requested).
    fn backend(&self) -> HwBackend;
    /// Position so the next frame returned is the one at or after `to`.
    fn seek(&mut self, to: Ticks) -> Result<(), MediaError>;
    /// `Ok(None)` at end of stream.
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError>;
}

pub trait DecoderFactory {
    fn open(&self, path: &Path, options: &DecoderOptions) -> Result<Box<dyn Decoder>, MediaError>;
}

#[derive(Debug, Clone)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    pub codec: Codec,
    pub backend: HwBackend,
    pub bitrate_kbps: Option<u32>,
}

pub trait Encoder: Send {
    fn push_frame(&mut self, frame: &VideoFrame) -> Result<(), MediaError>;
    /// Flush and close the output.
    fn finish(self: Box<Self>) -> Result<(), MediaError>;
}

pub trait EncoderFactory {
    fn create(&self, output: &Path, config: &EncoderConfig)
        -> Result<Box<dyn Encoder>, MediaError>;
}

/// Audio device output (later backed by `cpal`: WASAPI, ALSA/PipeWire, CoreAudio).
pub trait AudioOutput: Send {
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u16;
    /// Queue interleaved f32 samples for playback.
    fn submit(&mut self, samples: &[f32]) -> Result<(), MediaError>;
    fn play(&mut self) -> Result<(), MediaError>;
    fn pause(&mut self) -> Result<(), MediaError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Compile-time check that every trait stays usable as `dyn`.
    #[allow(dead_code)]
    fn traits_are_object_safe(
        _: &dyn Decoder,
        _: &dyn DecoderFactory,
        _: &dyn Encoder,
        _: &dyn EncoderFactory,
        _: &dyn AudioOutput,
    ) {
    }
}
