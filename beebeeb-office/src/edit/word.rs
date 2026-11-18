//! Word documents, through the patched `wordcraft-engine` (font store off). W1 stub: `Unsupported`.

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
