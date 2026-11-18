//! Carries what the engine does not write (macros, custom parts, comments it dropped) from the original
//! package into the saved one. Until w03-do-preserve it refuses whenever the original has a part the engine
//! would drop, so a save can never silently lose parts.

use std::collections::HashSet;
use std::io::Cursor;

use super::SaveError;

pub(super) fn carry(original: &[u8], written: Vec<u8>) -> Result<Vec<u8>, SaveError> {
    // Pass-through only when the original has no extra parts; the real carrying is w03-do-preserve's.
    let names = |bytes: &[u8]| -> Option<HashSet<String>> {
        let zip = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
        Some(zip.file_names().filter(|n| !n.ends_with('/')).map(str::to_owned).collect())
    };
    let (Some(before), Some(after)) = (names(original), names(&written)) else { return Err(SaveError::Unsupported) };
    if before.is_subset(&after) { Ok(written) } else { Err(SaveError::Unsupported) }
}
