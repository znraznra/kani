use crate::edit::{apply, Edit};
use crate::error::EditError;
use crate::model::Timeline;

struct Entry {
    redo: Edit,
    undo: Edit,
}

/// Linear undo/redo stack. All timeline changes should go through
/// [`History::execute`].
pub struct History {
    undo_stack: Vec<Entry>,
    redo_stack: Vec<Entry>,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::with_limit(500)
    }
}

impl History {
    pub fn with_limit(limit: usize) -> Self {
        History {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            limit: limit.max(1),
        }
    }

    pub fn execute(&mut self, tl: &mut Timeline, edit: Edit) -> Result<(), EditError> {
        let undo = apply(tl, edit.clone())?;
        self.redo_stack.clear();
        self.undo_stack.push(Entry { redo: edit, undo });
        if self.undo_stack.len() > self.limit {
            self.undo_stack.remove(0);
        }
        Ok(())
    }

    /// Forget everything (used when a change can't be undone).
    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    /// Returns `Ok(false)` if there was nothing to undo.
    pub fn undo(&mut self, tl: &mut Timeline) -> Result<bool, EditError> {
        let Some(entry) = self.undo_stack.pop() else {
            return Ok(false);
        };
        match apply(tl, entry.undo.clone()) {
            Ok(_) => {
                self.redo_stack.push(entry);
                Ok(true)
            }
            Err(e) => {
                self.undo_stack.push(entry);
                Err(e)
            }
        }
    }

    /// Returns `Ok(false)` if there was nothing to redo.
    pub fn redo(&mut self, tl: &mut Timeline) -> Result<bool, EditError> {
        let Some(entry) = self.redo_stack.pop() else {
            return Ok(false);
        };
        match apply(tl, entry.redo.clone()) {
            Ok(_) => {
                self.undo_stack.push(entry);
                Ok(true)
            }
            Err(e) => {
                self.redo_stack.push(entry);
                Err(e)
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::MediaId;
    use crate::model::TrackKind;
    use crate::time::Ticks;

    fn setup() -> (Timeline, History, crate::ids::TrackId) {
        let mut tl = Timeline::new();
        let mut history = History::default();
        let track = tl.new_track(TrackKind::Video, "V1");
        let id = track.id;
        history
            .execute(&mut tl, Edit::InsertTrack { index: 0, track })
            .unwrap();
        (tl, history, id)
    }

    #[test]
    fn undo_redo_walk_the_stack() {
        let (mut tl, mut history, track) = setup();
        let clip = tl.new_clip(MediaId(1), Ticks(0), Ticks(0), Ticks(10));
        history
            .execute(&mut tl, Edit::InsertClip { track, clip })
            .unwrap();
        assert_eq!(tl.track(track).unwrap().clips.len(), 1);

        assert!(history.undo(&mut tl).unwrap());
        assert!(tl.track(track).unwrap().clips.is_empty());
        assert!(history.can_redo());

        assert!(history.redo(&mut tl).unwrap());
        assert_eq!(tl.track(track).unwrap().clips.len(), 1);
    }

    #[test]
    fn new_edit_clears_redo() {
        let (mut tl, mut history, track) = setup();
        let a = tl.new_clip(MediaId(1), Ticks(0), Ticks(0), Ticks(10));
        history
            .execute(&mut tl, Edit::InsertClip { track, clip: a })
            .unwrap();
        history.undo(&mut tl).unwrap();
        let b = tl.new_clip(MediaId(1), Ticks(0), Ticks(20), Ticks(10));
        history
            .execute(&mut tl, Edit::InsertClip { track, clip: b })
            .unwrap();
        assert!(!history.can_redo());
    }

    #[test]
    fn failed_edit_is_not_recorded() {
        let (mut tl, mut history, track) = setup();
        let bad = tl.new_clip(MediaId(1), Ticks(0), Ticks(0), Ticks(0));
        assert!(history
            .execute(&mut tl, Edit::InsertClip { track, clip: bad })
            .is_err());
        // Only the track insertion is on the stack.
        assert!(history.undo(&mut tl).unwrap());
        assert!(!history.can_undo());
    }

    #[test]
    fn nothing_to_undo_or_redo_is_ok() {
        let mut tl = Timeline::new();
        let mut history = History::default();
        assert_eq!(history.undo(&mut tl), Ok(false));
        assert_eq!(history.redo(&mut tl), Ok(false));
    }

    #[test]
    fn limit_drops_oldest_entries() {
        let mut tl = Timeline::new();
        let mut history = History::with_limit(2);
        for i in 0..4 {
            let track = tl.new_track(TrackKind::Audio, format!("A{i}"));
            history
                .execute(&mut tl, Edit::InsertTrack { index: i, track })
                .unwrap();
        }
        assert!(history.undo(&mut tl).unwrap());
        assert!(history.undo(&mut tl).unwrap());
        assert!(!history.undo(&mut tl).unwrap());
        assert_eq!(tl.tracks.len(), 2);
    }
}
