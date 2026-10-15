use crate::ids::{ClipId, TrackId};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EditError {
    #[error("track {0} not found")]
    TrackNotFound(TrackId),
    #[error("clip {0} not found")]
    ClipNotFound(ClipId),
    #[error("track {0} is locked")]
    TrackLocked(TrackId),
    #[error("id {0} is already in use")]
    DuplicateId(u64),
    #[error("invalid range: {0}")]
    InvalidRange(&'static str),
    #[error("clip would overlap {with} on {track}")]
    Overlap { track: TrackId, with: ClipId },
    #[error("split point must be strictly inside the clip")]
    SplitOutOfRange,
    #[error("track index out of range")]
    IndexOutOfRange,
}
