//! Presentations, through `deckcraft-engine`, only with the non-default feature `edit-slides` (it brings
//! the MPL-2.0 `symphonia` crates; contract Q4). Without the feature every call answers `Unsupported`.
//! W1 stub: `Unsupported` either way.

use super::{Change, Clip, EditDoc, EditError, HitMode, Op, SaveError};
use crate::OpenError;

pub(super) fn open(_bytes: &[u8]) -> Result<(), OpenError> {
    Err(OpenError::Unsupported)
}

pub(super) fn apply(_doc: &mut EditDoc, _op: &Op) -> Result<Change, EditError> {
    Err(EditError::Unsupported)
}

pub(super) fn hit(_doc: &mut EditDoc, _page: u32, _x_pt: f32, _y_pt: f32, _mode: HitMode) -> Result<Change, EditError> {
    Err(EditError::Unsupported)
}

pub(super) fn save(_doc: &EditDoc) -> Result<Vec<u8>, SaveError> {
    Err(SaveError::Unsupported)
}

pub(super) fn copy(_doc: &EditDoc) -> Clip {
    Clip::default()
}

pub(super) fn layouts(_doc: &EditDoc) -> Vec<String> {
    Vec::new()
}
