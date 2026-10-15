//! Pure-Rust timeline model for Kani.
//!
//! This crate has **no I/O, no GPU, no FFmpeg**. It only knows about tracks,
//! clips and the edit operations that change them. Every change goes through
//! [`Edit`], which makes undo/redo, autosave logs and testing straightforward.

mod edit;
mod error;
mod history;
mod ids;
pub mod keyframes;
mod model;
mod snap;
pub mod text;
mod time;
pub mod transition;

pub use edit::{apply, Edit};
pub use error::EditError;
pub use history::History;
pub use ids::{ClipId, MediaId, TrackId};
pub use keyframes::{Keyframe, Prop};
pub use model::{Clip, Timeline, Track, TrackKind, Transform, DEFAULT_VOLUME, MAX_VOLUME};
pub use snap::{
    snap, snap_points, snap_points_with_extra, snap_span, snap_span_with_extra, snap_with_extra,
};
pub use text::{caption_styles, CaptionStyle, HighlightMode, TextAlign, TextSpec, Word};
pub use time::{FrameRate, Ticks, TICKS_PER_SECOND};
