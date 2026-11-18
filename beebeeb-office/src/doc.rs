//! Word documents (`.docx` and its relatives, `.doc`): read from bytes, laid out once, pages
//! rendered on request.
//!
//! The path is the readers, then `wordcraft_layout::layout`, then `wordcraft_render`: below
//! `wordcraft-engine`, which the viewer never links (it reaches a network client). What the engine
//! would add on top (its font fallback choices, field updates, defaults) is not here; the notes of
//! the branch that added this list what that changes.

use std::sync::OnceLock;

use wordcraft_layout::display::DisplayOptions;
use wordcraft_layout::{DocLayout, LayoutCache, LayoutOptions, StrictOutput};
use wordcraft_render::RenderOptions;

use crate::error::{from_docbin, from_docx};
use crate::raster::check_buffer;
use crate::{Limits, OpenError, Rect, RenderError, RenderInfo, probe};

/// Most outline entries [`Document::outline`] returns.
pub const MAX_OUTLINE_ENTRIES: usize = 10_000;
/// Longest outline entry text, bytes of UTF-8 (cut at a character boundary).
pub const MAX_OUTLINE_TEXT_BYTES: usize = 1024;

/// The Word kind a file's name says. The bytes decide which reader opens it
/// ([`Document::open`]); the kind is used only for bytes that start like no file kind known here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocKind {
    /// `.docx`, and the same package as `.docm`, `.dotx`, `.dotm`.
    Docx,
    /// `.doc` (Word 97-2003).
    Doc,
}

/// The page count and every page's size, in points.
#[derive(Clone, Debug, PartialEq)]
pub struct DocInfo {
    pub pages: u32,
    /// (width, height) of each page, points; one per page, each finite and above 0.
    pub sizes_pt: Vec<(f32, f32)>,
}

/// One heading of the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineEntry {
    /// 1 to 9.
    pub level: u8,
    /// The heading's text, without control characters, at most [`MAX_OUTLINE_TEXT_BYTES`].
    pub text: String,
    /// The page it starts on, 0-based, always below [`DocInfo::pages`].
    pub page: u32,
}

/// An open Word document.
pub struct Document {
    doc: wordcraft_doc::Document,
    layout: DocLayout,
    info: DocInfo,
    opts: RenderOptions,
    limits: Limits,
    /// A picture was left out of the whole document: the reader dropped it (its target is outside
    /// the file, it is only linked, or its part is missing) or it is over the picture limits. The
    /// reader does not record where it stood, so every page reports it.
    dropped_pictures: bool,
    /// Per page, worked out on its first render: a picture on it does not decode, or a character
    /// on it has no glyph in any installed font.
    missing: Vec<OnceLock<bool>>,
}

impl Document {
    /// Open a Word document from its bytes and lay it out.
    ///
    /// In this order: [`crate::prescan`] refuses a container over `limits` before any reader sees
    /// it; what the bytes hold picks the reader, whatever `kind` says (`reader_for`: a `.docx`
    /// package named `.doc` opens, an RTF file or a web page is [`OpenError::Unsupported`]); the
    /// reader parses them; pictures over the picture limits are left out; the document is laid
    /// out once; more than `limits.max_pages` pages is refused. Nothing is fetched and nothing is
    /// written: linked pictures, templates and fields stay unresolved. Fonts embedded in the file
    /// are not loaded; the system's font folder may be read.
    pub fn open(kind: DocKind, bytes: &[u8], limits: &Limits) -> Result<Document, OpenError> {
        let (doc, oversized) = read_model(kind, bytes, limits)?;
        // Laid out once; the paragraph cache is not needed after that.
        let layout = wordcraft_layout::layout(&doc, &mut LayoutCache::new(), &LayoutOptions::default());
        if layout.pages.is_empty() {
            return Err(OpenError::Damaged);
        }
        if layout.pages.len() as u64 > u64::from(limits.max_pages) {
            return Err(OpenError::TooLarge(crate::limits::PAGES));
        }
        let mut sizes_pt = Vec::with_capacity(layout.pages.len());
        for p in &layout.pages {
            if !(p.w.is_finite() && p.h.is_finite() && p.w > 0.0 && p.h > 0.0) {
                return Err(OpenError::Damaged);
            }
            sizes_pt.push((p.w, p.h));
        }
        let pages = layout.pages.len() as u32;
        let missing_pictures = &doc.missing_pictures;
        let dropped_pictures = oversized || missing_pictures.total > 0 || missing_pictures.truncated || !missing_pictures.list.is_empty();
        Ok(Document {
            missing: (0..layout.pages.len()).map(|_| OnceLock::new()).collect(),
            info: DocInfo { pages, sizes_pt },
            opts: viewer_render_options(),
            limits: *limits,
            dropped_pictures,
            doc,
            layout,
        })
    }

    /// The page count and the page sizes.
    pub fn info(&self) -> DocInfo {
        self.info.clone()
    }

    /// The size in pixels of what [`Document::render_page`] writes for `page` at `scale` and
    /// `region`, after every check but the buffer's: the caller sizes its buffer from this.
    ///
    /// The whole page is `ceil(width_pt * scale)` x `ceil(height_pt * scale)` (the renderer's own
    /// rounding) and must be at most `max_render_side_px` on each side. A region is a tile of that
    /// raster ([`Rect`], pixels): at most `max_render_side_px` on each side, inside the page
    /// raster, and the page raster itself at most `wordcraft_render::MAX_SIDE` on a side.
    pub fn render_size(&self, page: u32, scale: f32, region: Option<Rect>) -> Result<(u32, u32), RenderError> {
        render_size_of(&self.layout, &self.limits, page, scale, region)
    }

    /// Render `page` (0-based) at `scale` pixels per point, the whole page or one `region` of it,
    /// into `out` as RGBA8, row by row, top-left first.
    ///
    /// `out` must be exactly `w * h * 4` bytes for the size [`Document::render_size`] gives, else
    /// [`RenderError::BufferSize`] and nothing is written. The renderer works in premultiplied
    /// alpha; every pixel is opaque (the paper is painted first), so the bytes are the same
    /// straight RGBA. The renderer draws into its own raster, which is then copied into `out`
    /// once.
    pub fn render_page(&self, page: u32, scale: f32, region: Option<Rect>, out: &mut [u8]) -> Result<RenderInfo, RenderError> {
        render_page_of(&self.doc, &self.layout, &self.opts, &self.limits, page, scale, region, out, |n| self.missing_on(n))
    }

    /// The headings (outline level 1 to 9) of the body, in order, with the page each starts on;
    /// at most [`MAX_OUTLINE_ENTRIES`].
    pub fn outline(&self) -> Vec<OutlineEntry> {
        outline_of(&self.doc, &self.layout)
    }

    /// Words in the body, counted as Word's status bar counts them.
    pub fn word_count(&self) -> u32 {
        u32::try_from(self.doc.word_count()).unwrap_or(u32::MAX)
    }

    /// Whether something on page `n` cannot be drawn ([`RenderInfo::missing_content`]).
    fn missing_on(&self, n: usize) -> bool {
        if self.dropped_pictures {
            return true;
        }
        let Some(slot) = self.missing.get(n) else { return false };
        *slot.get_or_init(|| page_missing(&self.doc, &self.layout, &self.opts, n))
    }
}

/// The read half of an open, shared with the editor: pre-scan, pick the reader, read, make every story
/// non-empty and leave out oversized pictures. Returns the model and whether a picture was left out.
pub(crate) fn read_model(kind: DocKind, bytes: &[u8], limits: &Limits) -> Result<(wordcraft_doc::Document, bool), OpenError> {
    crate::prescan(bytes, limits)?;
    let reader = reader_for(kind, bytes)?;
    probe::handed(bytes);
    let mut doc = match reader {
        DocKind::Docx => wordcraft_docx::read_with(bytes, &wordcraft_docx::ReadOptions::default()).map_err(from_docx)?,
        DocKind::Doc => wordcraft_docbin::read(bytes).map_err(from_docbin)?,
    };
    // Every story and cell holds at least one paragraph, as the engine makes sure on open: an
    // empty body still lays out as one blank page.
    doc.ensure_nonempty();
    let oversized = drop_oversized_pictures(&mut doc, limits);
    Ok((doc, oversized))
}

/// Whether the bytes are read by the `.doc` reader (the editor opens those for viewing only).
pub(crate) fn reads_as_doc(kind: DocKind, bytes: &[u8]) -> bool {
    matches!(reader_for(kind, bytes), Ok(DocKind::Doc))
}

/// Page sizes of a layout, each finite and above 0 (else `Damaged`).
pub(crate) fn info_of(layout: &DocLayout) -> Result<DocInfo, OpenError> {
    if layout.pages.is_empty() {
        return Err(OpenError::Damaged);
    }
    let mut sizes_pt = Vec::with_capacity(layout.pages.len());
    for p in &layout.pages {
        if !(p.w.is_finite() && p.h.is_finite() && p.w > 0.0 && p.h > 0.0) {
            return Err(OpenError::Damaged);
        }
        sizes_pt.push((p.w, p.h));
    }
    Ok(DocInfo { pages: layout.pages.len() as u32, sizes_pt })
}

/// [`Document::render_size`] over any layout.
pub(crate) fn render_size_of(layout: &DocLayout, limits: &Limits, page: u32, scale: f32, region: Option<Rect>) -> Result<(u32, u32), RenderError> {
    let p = layout.pages.get(page as usize).ok_or(RenderError::NoSuchPage)?;
    if !(scale.is_finite() && scale > 0.0) {
        return Err(RenderError::BadScale);
    }
    // The same expressions `wordcraft_render::render_page` sizes its raster with.
    let (pw, ph) = ((p.w * scale).ceil(), (p.h.min(1e6) * scale).ceil());
    if !(pw.is_finite() && ph.is_finite()) {
        return Err(RenderError::TooLarge);
    }
    let max_side = limits.max_render_side_px as f32;
    match region {
        None => {
            if pw > max_side || ph > max_side {
                return Err(RenderError::TooLarge);
            }
            Ok((pw.max(1.0) as u32, ph.max(1.0) as u32))
        }
        Some(r) => {
            if r.w == 0 || r.h == 0 {
                return Err(RenderError::BadRegion);
            }
            if r.w > limits.max_render_side_px || r.h > limits.max_render_side_px {
                return Err(RenderError::TooLarge);
            }
            let engine_max = wordcraft_render::MAX_SIDE as f32;
            if pw > engine_max || ph > engine_max {
                return Err(RenderError::TooLarge);
            }
            let right = u64::from(r.x) + u64::from(r.w);
            let bottom = u64::from(r.y) + u64::from(r.h);
            if right as f64 > f64::from(pw.max(1.0)) || bottom as f64 > f64::from(ph.max(1.0)) {
                return Err(RenderError::BadRegion);
            }
            Ok((r.w, r.h))
        }
    }
}

/// [`Document::render_page`] over any model and layout; `missing` answers `missing_content` for the page.
pub(crate) fn render_page_of(
    doc: &wordcraft_doc::Document,
    layout: &DocLayout,
    opts: &RenderOptions,
    limits: &Limits,
    page: u32,
    scale: f32,
    region: Option<Rect>,
    out: &mut [u8],
    missing: impl FnOnce(usize) -> bool,
) -> Result<RenderInfo, RenderError> {
    let (w, h) = render_size_of(layout, limits, page, scale, region)?;
    check_buffer(w, h, out)?;
    let n = page as usize;
    let p = layout.pages.get(n).ok_or(RenderError::NoSuchPage)?;
    let rendered = match region {
        None => wordcraft_render::render_page(doc, p, scale, opts),
        Some(r) => {
            // `render_area` takes the area in page points, of the layout's own rectangle type
            // (copied from the page, so no geometry crate is named here). Half a pixel short
            // on each side, so its `ceil(w * scale)` gives exactly `r.w` x `r.h` pixels.
            let mut area = p.body;
            area.x = r.x as f32 / scale;
            area.y = r.y as f32 / scale;
            area.w = (r.w as f32 - 0.5) / scale;
            area.h = (r.h as f32 - 0.5) / scale;
            wordcraft_render::render_area(doc, p, area, scale, opts)
        }
    };
    if rendered.width != w || rendered.height != h || rendered.pixels.len() != out.len() {
        return Err(RenderError::Engine);
    }
    out.copy_from_slice(&rendered.pixels);
    Ok(RenderInfo { w, h, missing_content: missing(n) })
}

/// Whether something on page `n` cannot be drawn: a glyph with no font or a picture that does not decode.
pub(crate) fn page_missing(doc: &wordcraft_doc::Document, layout: &DocLayout, opts: &RenderOptions, n: usize) -> bool {
    let display = &opts.display;
    let glyphs = layout.missing_glyphs(doc, display, Some(n..=n));
    let (pictures, truncated, _) = wordcraft_layout::undecodable_pictures(doc, layout, Some(n..=n), display, StrictOutput::Png);
    !glyphs.is_empty() || !pictures.is_empty() || truncated
}

/// [`Document::outline`] over any model and layout.
pub(crate) fn outline_of(doc: &wordcraft_doc::Document, layout: &DocLayout) -> Vec<OutlineEntry> {
    let mut out = Vec::new();
    for (i, block) in doc.body.iter().enumerate() {
        if out.len() >= MAX_OUTLINE_ENTRIES {
            break;
        }
        let wordcraft_doc::Block::Para(p) = &**block else { continue };
        let level = wordcraft_layout::outline::para_level(doc, p);
        if !(1..=9).contains(&level) {
            continue;
        }
        let text = outline_text(&p.plain_text());
        if text.is_empty() {
            continue;
        }
        let after = layout.pages.partition_point(|pg| pg.first_block <= i);
        let page = after.saturating_sub(1).min(layout.pages.len().saturating_sub(1)) as u32;
        out.push(OutlineEntry { level, text, page });
    }
    out
}

/// The first bytes of a zip package (a local file header): `.docx` and its relatives.
const ZIP_MAGIC: &[u8] = b"PK\x03\x04";
/// The first bytes of an RTF file.
const RTF_MAGIC: &[u8] = b"{\\rtf";
/// A UTF-8 byte-order mark.
const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
/// How much of the start of a file is looked at for a web page.
const SNIFF_BYTES: usize = 4096;

/// Which reader opens `bytes`, picked by what they hold, as Word does: a file saved under the
/// other Word name (a `.docx` package named `.doc`, a Word 97-2003 file named `.docx`) opens, and
/// a readable file of another kind is not called damaged.
///
/// - A zip package: the `.docx` reader.
/// - A compound file: the `.doc` reader; but an encrypted Word package (also a compound file)
///   goes to the `.docx` reader, which says it needs a password ([`OpenError::Password`]).
/// - RTF, a web page, a Word XML document: [`OpenError::Unsupported`]. No reader here shows
///   them, and none is handed the bytes.
/// - Anything else: the reader `kind` names, which says what is wrong with it.
///
/// Runs after the pre-scan: the encrypted-package check opens the compound file's directory.
fn reader_for(kind: DocKind, bytes: &[u8]) -> Result<DocKind, OpenError> {
    if bytes.starts_with(ZIP_MAGIC) {
        return Ok(DocKind::Docx);
    }
    if bytes.starts_with(&crate::limits::CFB_MAGIC) {
        return Ok(if wordcraft_docx::is_encrypted(bytes) { DocKind::Docx } else { DocKind::Doc });
    }
    if bytes.starts_with(RTF_MAGIC) {
        return Err(OpenError::Unsupported);
    }
    if is_web_page(bytes) {
        return Err(OpenError::Unsupported);
    }
    if is_word_xml(bytes) {
        return Err(OpenError::Unsupported);
    }
    Ok(kind)
}

/// Does the file start like a web page (`<!DOCTYPE html …>` or `<html …>`, after a byte-order
/// mark, white space, an XML declaration and comments)? Word's Save as Web Page and many report
/// generators write one under a `.doc` name. Only the first [`SNIFF_BYTES`] are read, as ASCII
/// in any case; a page in UTF-16 is not recognised.
fn is_web_page(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(SNIFF_BYTES)];
    let mut t = head.strip_prefix(UTF8_BOM).unwrap_or(head).trim_ascii_start();
    for _ in 0..64 {
        let skip = if t.starts_with(b"<!--") {
            find(t, b"-->").map(|i| i + 3)
        } else if t.starts_with(b"<?") {
            find(t, b"?>").map(|i| i + 2)
        } else {
            break;
        };
        let Some(end) = skip else { return false };
        t = t[end..].trim_ascii_start();
    }
    starts_with_tag(t, b"<!doctype html") || starts_with_tag(t, b"<html")
}

/// Is this a Word XML document (Word 2003 XML or Flat OPC), by its root element? Only bytes that
/// start with markup (or a UTF-16 byte-order mark) are parsed, as upstream does.
fn is_word_xml(bytes: &[u8]) -> bool {
    let utf16 = bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]);
    let markup = bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes).trim_ascii_start().starts_with(b"<");
    (utf16 || markup) && !matches!(wordcraft_docx::sniff_xml(bytes), wordcraft_docx::XmlKind::Other)
}

/// `t` starts with the tag `name` (ASCII, any case), and the tag's name ends there: `<htmlish>`
/// is some other markup.
fn starts_with_tag(t: &[u8], name: &[u8]) -> bool {
    t.len() >= name.len()
        && t[..name.len()].eq_ignore_ascii_case(name)
        && t.get(name.len()).is_none_or(|c| c.is_ascii_whitespace() || *c == b'>' || *c == b'/')
}

/// Where `needle` (not empty) first occurs in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// How a viewer draws: the upstream defaults (no formatting marks, tracked changes shown as
/// markup, white paper), except that headers and footers are not dimmed: the defaults dim them as
/// while editing the body, and nothing is edited here.
pub(crate) fn viewer_render_options() -> RenderOptions {
    RenderOptions { display: DisplayOptions { dim_header: false, ..DisplayOptions::default() }, ..RenderOptions::default() }
}

/// Leave out every raster picture whose header declares more than `max_image_side_px` on a side
/// or more than `max_image_pixels`: it is never decoded, and is reported as content not shown.
/// Bytes that are not a raster picture the decoder knows (a metafile, or bytes it cannot read) are
/// left to the renderer's own checks. Returns whether any picture was left out.
fn drop_oversized_pictures(doc: &mut wordcraft_doc::Document, limits: &Limits) -> bool {
    let before = doc.media.len();
    doc.media.retain(|_, bytes| match wordcraft_render::image_size(bytes.as_slice()) {
        Some((w, h)) => w <= limits.max_image_side_px && h <= limits.max_image_side_px && u64::from(w) * u64::from(h) <= limits.max_image_pixels,
        None => true,
    });
    doc.media.len() != before
}

/// A heading's text for the outline: control characters and object placeholders out, outer
/// spaces trimmed, cut to at most [`MAX_OUTLINE_TEXT_BYTES`] at a character boundary.
fn outline_text(raw: &str) -> String {
    let clean: String = raw.chars().filter(|c| !c.is_control() && *c != wordcraft_doc::para::OBJ).collect();
    let trimmed = clean.trim();
    let mut end = trimmed.len().min(MAX_OUTLINE_TEXT_BYTES);
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_string()
}
