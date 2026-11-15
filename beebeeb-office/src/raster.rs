//! What every render call shares: the region type, what a render reports, and the buffer check.
//! Documents, sheets and slides all write RGBA8 straight into a buffer the caller sized.

use crate::RenderError;

/// A region of a page raster, in **pixels at the requested scale**: `x`, `y` from the page's
/// top-left corner, `w` x `h` the size of the result. Pixels, not points, so neighbouring tiles
/// meet exactly at any scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// What a render wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderInfo {
    /// Width of the raster written, pixels.
    pub w: u32,
    /// Height of the raster written, pixels.
    pub h: u32,
    /// Something on this page could not be drawn: a picture the file only links to (never
    /// fetched), one that is missing, too large or does not decode, or characters no installed
    /// font can draw. The app shows "Some content isn't shown." (DW-1564).
    pub missing_content: bool,
}

/// Bytes of a `w` x `h` RGBA8 raster, `None` when that does not fit in memory.
pub(crate) fn rgba_len(w: u32, h: u32) -> Option<usize> {
    let n = u64::from(w).checked_mul(u64::from(h))?.checked_mul(4)?;
    usize::try_from(n).ok()
}

/// `out` must be exactly `w * h * 4` bytes.
pub(crate) fn check_buffer(w: u32, h: u32, out: &[u8]) -> Result<(), RenderError> {
    match rgba_len(w, h) {
        Some(n) if n == out.len() => Ok(()),
        _ => Err(RenderError::BufferSize),
    }
}
