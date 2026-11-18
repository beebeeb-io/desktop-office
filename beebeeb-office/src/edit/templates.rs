//! The built-in templates, built by the engines at New (never read from disk). W1 stub: `Unsupported`.

use super::{EditError, EditKind, Template};

pub(super) fn build(_kind: EditKind, _template: Template) -> Result<Vec<u8>, EditError> {
    Err(EditError::Unsupported)
}
