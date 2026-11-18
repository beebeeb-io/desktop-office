//! Carries what the engine does not write (macros, custom parts, comments it dropped) from the original
//! package into the saved one. W1 stub: refuses, so a save can never silently lose parts.

use super::SaveError;

pub(super) fn carry(_original: &[u8], _written: Vec<u8>) -> Result<Vec<u8>, SaveError> {
    Err(SaveError::Unsupported)
}
