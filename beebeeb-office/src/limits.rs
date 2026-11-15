//! The limits a file must stay inside (the viewer contract §5.2), and the pre-scan that refuses a
//! container over them before any reader inflates a byte of it.
//!
//! The input size limit (100 MB) is the app's: it is checked before the file is decrypted for the
//! viewer, so it is not repeated here. The memory, open and per-request time limits belong to the
//! helper's job object and the app's timers.

use crate::OpenError;

/// Largest declared unpacked size of a container: the sum of every entry of a zip package, or of
/// every stream of a compound file (512 MiB).
pub const MAX_UNPACKED_BYTES: u64 = 512 * 1024 * 1024;
/// Most entries in a container: zip entries, or directory entries of a compound file.
pub const MAX_ENTRIES: u64 = 10_000;
/// Most pages of a document or slides of a presentation.
pub const MAX_PAGES: u32 = 2_000;
/// Most sheets of a workbook.
pub const MAX_SHEETS: u32 = 256;
/// Most non-empty cells of a workbook.
pub const MAX_CELLS: u64 = 2_000_000;
/// Widest or tallest decoded picture, pixels.
pub const MAX_IMAGE_SIDE_PX: u32 = 16_384;
/// Most pixels of one decoded picture (64 Mi, 8,192 x 8,192).
pub const MAX_IMAGE_PIXELS: u64 = 64 * 1024 * 1024;
/// Widest or tallest rendered page, slide or tile, pixels (64 MiB of RGBA8 at the most).
pub const MAX_RENDER_SIDE_PX: u32 = 4_096;

/// [`OpenError::TooLarge`] for a container whose declared unpacked size is over the limit.
pub const UNPACKED: &str = "unpacked size";
/// [`OpenError::TooLarge`] for a container with too many entries.
pub const ENTRIES: &str = "entries";
/// [`OpenError::TooLarge`] for a document with too many pages after layout.
pub const PAGES: &str = "pages";
/// [`OpenError::TooLarge`] for a file a reader refused under one of its own limits.
pub const READER: &str = "reader limit";

/// The limits one open document is held to. [`Limits::default`] is the viewer contract's table;
/// tests may lower a field to reach a limit with a small file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_unpacked_bytes: u64,
    pub max_entries: u64,
    pub max_pages: u32,
    pub max_sheets: u32,
    pub max_cells: u64,
    pub max_image_side_px: u32,
    pub max_image_pixels: u64,
    pub max_render_side_px: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_unpacked_bytes: MAX_UNPACKED_BYTES,
            max_entries: MAX_ENTRIES,
            max_pages: MAX_PAGES,
            max_sheets: MAX_SHEETS,
            max_cells: MAX_CELLS,
            max_image_side_px: MAX_IMAGE_SIDE_PX,
            max_image_pixels: MAX_IMAGE_PIXELS,
            max_render_side_px: MAX_RENDER_SIDE_PX,
        }
    }
}

/// The first bytes of an OLE compound file ([MS-CFB] §2.2): `.doc`, `.xls`, `.ppt`, and an
/// encrypted Office package.
pub(crate) const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// Refuse a container over `limits` from what it declares, before any reader inflates it.
///
/// - A compound file (`.doc`, `.xls`, `.ppt`, an encrypted package): its directory is walked from
///   the header and the FAT only; the directory entries are counted and the declared stream sizes
///   summed. No stream is read.
/// - Anything else is looked at as a zip package: the end records are checked first (the readers'
///   own guard, at our entry limit), then the central directory is read by the same `zip` crate
///   the readers use and the declared sizes are summed. No entry is decompressed.
///
/// Bytes that are neither pass: the reader decides what they are ([`OpenError::Damaged`]).
pub fn prescan(bytes: &[u8], limits: &Limits) -> Result<(), OpenError> {
    if bytes.starts_with(&CFB_MAGIC) { prescan_cfb(bytes, limits) } else { prescan_zip(bytes, limits) }
}

fn prescan_zip(bytes: &[u8], limits: &Limits) -> Result<(), OpenError> {
    // `zip` parses (and keeps) every central directory header the end record declares before its
    // caller can look at the count: the readers' guard refuses an over-long declaration first.
    wordcraft_doc::zip_guard::check(bytes, limits.max_entries).map_err(|_| OpenError::TooLarge(ENTRIES))?;
    let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) else {
        // No central directory `zip` can read: the reader will say the file is damaged.
        return Ok(());
    };
    if archive.len() as u64 > limits.max_entries {
        return Err(OpenError::TooLarge(ENTRIES));
    }
    let mut unpacked: u64 = 0;
    for i in 0..archive.len() {
        // `by_index_raw` reads the entry's local header only; nothing is inflated. An entry it
        // cannot find is one the readers skip too.
        let Ok(entry) = archive.by_index_raw(i) else { continue };
        unpacked = unpacked.saturating_add(entry.size());
        if unpacked > limits.max_unpacked_bytes {
            return Err(OpenError::TooLarge(UNPACKED));
        }
    }
    Ok(())
}

/// A FAT entry: the sector ends a chain.
const END_OF_CHAIN: u32 = 0xFFFF_FFFE;
/// A FAT or DIFAT entry: unused.
const FREE_SECT: u32 = 0xFFFF_FFFF;
/// Directory entries per 512 bytes ([MS-CFB] §2.6: 128 bytes each).
const DIR_ENTRY_LEN: usize = 128;
/// Directory entry object type of a stream.
const STREAM: u8 = 2;
/// DIFAT entries in the header ([MS-CFB] §2.2).
const HEADER_DIFAT: usize = 109;

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    bytes.get(at..at.checked_add(2)?).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    bytes.get(at..at.checked_add(4)?).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    let b = bytes.get(at..at.checked_add(8)?)?;
    Some(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

/// Walk a compound file's directory chain ([MS-CFB] §2.2 header, §2.3 FAT, §2.5 DIFAT, §2.6
/// directory) and refuse it when it has more than `max_entries` directory entries or its streams
/// declare more than `max_unpacked_bytes`. A chain that leaves the file, loops, or cannot be
/// followed is [`OpenError::Damaged`]: `cfb` would refuse it too.
fn prescan_cfb(bytes: &[u8], limits: &Limits) -> Result<(), OpenError> {
    // The readers' own guard first: it refuses a DIFAT that would make `cfb` build a huge FAT.
    wordcraft_doc::cfb_guard::check(bytes).map_err(|_| OpenError::Damaged)?;
    let sector_len: usize = match u16_at(bytes, 0x1A) {
        Some(3) => 512,
        Some(4) => 4096,
        _ => return Err(OpenError::Damaged),
    };
    if bytes.len() < sector_len {
        return Err(OpenError::Damaged);
    }
    // Sectors after the header sector, the last one possibly partial (as `cfb` counts them).
    let num_sectors = bytes.len().div_ceil(sector_len) - 1;
    let sector_at = |s: u32| (s as usize).checked_add(1).and_then(|k| k.checked_mul(sector_len));
    let per_sector = sector_len / 4;

    // The FAT's own sectors, from the DIFAT: the header's 109 entries (up to the first free one,
    // as `cfb` reads them), then the DIFAT chain, at most one step per sector of the file.
    let mut fat_sectors: Vec<u32> = (0..HEADER_DIFAT).map_while(|i| u32_at(bytes, 0x4C + i * 4).filter(|&s| s != FREE_SECT)).collect();
    let mut next_difat = u32_at(bytes, 0x44).unwrap_or(END_OF_CHAIN);
    let mut steps = 0usize;
    while next_difat != END_OF_CHAIN && next_difat != FREE_SECT {
        steps += 1;
        if steps > num_sectors || next_difat as usize >= num_sectors {
            return Err(OpenError::Damaged);
        }
        let at = sector_at(next_difat).ok_or(OpenError::Damaged)?;
        for i in 0..per_sector - 1 {
            match u32_at(bytes, at + i * 4) {
                Some(FREE_SECT) | None => {}
                Some(s) => fat_sectors.push(s),
            }
        }
        next_difat = u32_at(bytes, at + (per_sector - 1) * 4).unwrap_or(END_OF_CHAIN);
    }
    // The next sector of a chain: entry `s` of the FAT, which lives in FAT sector `s / per_sector`.
    let next = |s: u32| -> Option<u32> {
        let fat_sector = *fat_sectors.get(s as usize / per_sector)?;
        if fat_sector as usize >= num_sectors {
            return None;
        }
        u32_at(bytes, sector_at(fat_sector)? + (s as usize % per_sector) * 4)
    };

    let v3 = sector_len == 512;
    let mut entries: u64 = 0;
    let mut streams_len: u64 = 0;
    let mut sector = u32_at(bytes, 0x30).ok_or(OpenError::Damaged)?;
    let mut steps = 0usize;
    while sector != END_OF_CHAIN {
        steps += 1;
        if steps > num_sectors || sector as usize >= num_sectors {
            return Err(OpenError::Damaged);
        }
        let base = sector_at(sector).ok_or(OpenError::Damaged)?;
        for e in 0..sector_len / DIR_ENTRY_LEN {
            let at = base + e * DIR_ENTRY_LEN;
            let kind = *bytes.get(at + 0x42).ok_or(OpenError::Damaged)?;
            // Every slot counts, used or not: `cfb` parses and keeps each one.
            entries += 1;
            if entries > limits.max_entries {
                return Err(OpenError::TooLarge(ENTRIES));
            }
            if kind == STREAM {
                let declared = u64_at(bytes, at + 0x78).ok_or(OpenError::Damaged)?;
                // A version 3 file may carry junk in the high half ([MS-CFB] §2.6.3); only the
                // low 32 bits are its size.
                let declared = if v3 { declared & 0xFFFF_FFFF } else { declared };
                streams_len = streams_len.saturating_add(declared);
                if streams_len > limits.max_unpacked_bytes {
                    return Err(OpenError::TooLarge(UNPACKED));
                }
            }
        }
        sector = next(sector).ok_or(OpenError::Damaged)?;
    }
    Ok(())
}
