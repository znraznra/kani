//! Captions: turning speech into timed words, grouping them into lines, and
//! writing subtitle files. Independent of the editor and the timeline.

pub mod align;
pub mod lines;
pub mod models;
pub mod srt;
#[cfg(feature = "whisper")]
pub mod whisper;

/// One spoken word. `text` carries the space before it, as Whisper writes it.
#[derive(Debug, Clone, PartialEq)]
pub struct TimedWord {
    pub text: String,
    /// Seconds from the start of the audio.
    pub start: f64,
    pub end: f64,
}

/// Whether transcription is compiled in.
pub const TRANSCRIPTION_BUILT_IN: bool = cfg!(feature = "whisper");

#[derive(Debug, thiserror::Error)]
pub enum CaptionError {
    #[error("{0}")]
    Message(String),
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
