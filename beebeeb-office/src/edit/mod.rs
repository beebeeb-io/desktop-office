//! The editing API (W1 skeleton). Behind the default feature `edit`.
//!
//! Everything takes and returns bytes: no path, no URL, no process. The engines' commands that touch
//! files, processes, print, speech, fonts, templates from disk or recovery are not reachable from here:
//! [`Op`] is a closed list, and [`EditDoc`] is the only way in. `Op`, `Change`, `Marks` and `Dirty`
//! mirror the protocol crate's types (the helper maps them); this crate never depends on the protocol.
//!
//! Per engine: `word.rs` (WordCraft, font store off), `sheet.rs` (GridCraft), `slide.rs` (DeckCraft, only
//! with the non-default feature `edit-slides`), `templates.rs`, `preserve.rs`. In W1 each answers
//! `Unsupported`.

mod preserve;
mod sheet;
mod slide;
mod templates;
mod types;
mod word;

pub use types::*;

use crate::{CellBlock, CellRange, DocInfo, Limits, OpenError, OutlineEntry, Rect, RenderError, RenderInfo};

/// A document open for editing: the bytes it was opened from, the engine's session and a revision that
/// rises with every change.
pub struct EditDoc {
    kind: EditKind,
    /// What the file held when it was opened (or built), for [`preserve::carry`] at save.
    original: Vec<u8>,
    revision: u64,
    /// The engine's session for this kind (WordCraft's, for Docx); each `edit/<kind>.rs` makes and downcasts its own.
    engine: Option<Box<dyn std::any::Any + Send>>,
}

impl EditDoc {
    /// Open `bytes` for editing. The file is pre-scanned against `limits` like the viewer's.
    pub fn open(kind: EditKind, bytes: &[u8], limits: &Limits) -> Result<EditDoc, OpenError> {
        let engine: Option<Box<dyn std::any::Any + Send>> = match kind {
            EditKind::Docx => Some(word::open(bytes, limits)?),
            EditKind::Xlsx => Some(sheet::open(bytes, limits)?),
            EditKind::Pptx => Some(slide::open(bytes, limits)?),
        };
        Ok(EditDoc { kind, original: bytes.to_vec(), revision: 0, engine })
    }

    /// A new document from a built-in template (never from disk). Only Docs have more than `Blank`.
    pub fn new_from(kind: EditKind, template: Template) -> Result<EditDoc, EditError> {
        let bytes = templates::build(kind, template)?;
        let limits = Limits::default();
        let engine: Option<Box<dyn std::any::Any + Send>> = match kind {
            EditKind::Docx => Some(word::open(&bytes, &limits).map_err(|_| EditError::Engine)?),
            EditKind::Xlsx => Some(sheet::open(&bytes, &limits).map_err(|_| EditError::Engine)?),
            // Without `edit-slides` the slide engine answers Unsupported.
            EditKind::Pptx => Some(slide::open(&bytes, &limits).map_err(|e| match e {
                OpenError::Unsupported => EditError::Unsupported,
                _ => EditError::Engine,
            })?),
        };
        Ok(EditDoc { kind, original: bytes, revision: 0, engine })
    }

    pub fn kind(&self) -> EditKind {
        self.kind
    }

    /// Rises with every change; 0 until the first one.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Apply one edit.
    pub fn apply(&mut self, op: &Op) -> Result<Change, EditError> {
        match self.kind {
            EditKind::Docx => word::apply(self, op),
            EditKind::Xlsx => sheet::apply(self, op),
            EditKind::Pptx => slide::apply(self, op),
        }
    }

    /// A click or drag at a point on a page (slide), in points.
    pub fn hit(&mut self, page: u32, x_pt: f32, y_pt: f32, mode: HitMode) -> Result<Change, EditError> {
        match self.kind {
            EditKind::Docx => word::hit(self, page, x_pt, y_pt, mode),
            EditKind::Xlsx => sheet::hit(self, page, x_pt, y_pt, mode),
            EditKind::Pptx => slide::hit(self, page, x_pt, y_pt, mode),
        }
    }

    /// Serialise the document. The engine's output goes through [`preserve::carry`] with the original.
    pub fn save(&self) -> Result<Vec<u8>, SaveError> {
        let written = match self.kind {
            EditKind::Docx => word::save(self)?,
            EditKind::Xlsx => sheet::save(self)?,
            EditKind::Pptx => slide::save(self)?,
        };
        preserve::carry(&self.original, written)
    }

    /// The selection as plain text and, where the engine has it, HTML.
    pub fn copy(&self) -> Clip {
        match self.kind {
            EditKind::Docx => word::copy(self),
            EditKind::Xlsx => sheet::copy(self),
            EditKind::Pptx => slide::copy(self),
        }
    }

    /// Names of the slide layouts (Slides; empty elsewhere).
    pub fn layouts(&self) -> Vec<String> {
        match self.kind {
            EditKind::Pptx => slide::layouts(self),
            _ => Vec::new(),
        }
    }

    // View calls, with the viewer's contracts (same arguments, same results, same errors).

    /// Pages, sheets or slides, as the viewer reports them for this kind.
    pub fn info(&self) -> Result<DocInfo, EditError> {
        if self.kind == EditKind::Docx {
            return word::info(self);
        }
        if self.kind == EditKind::Pptx {
            return slide::info(self);
        }
        Err(EditError::Unsupported)
    }

    pub fn render_size(&self, page: u32, scale: f32, region: Option<Rect>) -> Result<(u32, u32), RenderError> {
        if self.kind == EditKind::Docx {
            return word::render_size(self, page, scale, region);
        }
        if self.kind == EditKind::Pptx {
            return slide::render_size(self, page, scale, region);
        }
        Err(RenderError::Engine)
    }

    pub fn render_page(&self, page: u32, scale: f32, region: Option<Rect>, out: &mut [u8]) -> Result<RenderInfo, RenderError> {
        if self.kind == EditKind::Docx {
            return word::render_page(self, page, scale, region, out);
        }
        if self.kind == EditKind::Pptx {
            return slide::render_page(self, page, scale, region, out);
        }
        Err(RenderError::Engine)
    }

    pub fn outline(&self) -> Vec<OutlineEntry> {
        if self.kind == EditKind::Docx {
            return word::outline(self);
        }
        if self.kind == EditKind::Pptx {
            return slide::outline(self);
        }
        Vec::new()
    }

    pub fn words(&self) -> u32 {
        if self.kind == EditKind::Docx {
            return word::words(self);
        }
        0
    }

    /// The sheets and their used extents (Sheets only).
    pub fn sheets(&self) -> Result<crate::SheetsInfo, EditError> {
        if self.kind == EditKind::Xlsx {
            return sheet::sheets(self);
        }
        Err(EditError::Unsupported)
    }

    pub fn cells(&self, sheet: u32, range: CellRange) -> Result<CellBlock, EditError> {
        if self.kind == EditKind::Xlsx {
            return sheet::cells(self, sheet, range);
        }
        Err(EditError::Unsupported)
    }

    pub fn anchor_size(&self, sheet: u32, anchor: u32, scale: f32) -> Result<(u32, u32), RenderError> {
        if self.kind == EditKind::Xlsx {
            return sheet::anchor_size(self, sheet, anchor, scale);
        }
        Err(RenderError::Engine)
    }

    pub fn render_anchor(&self, sheet: u32, anchor: u32, scale: f32, out: &mut [u8]) -> Result<RenderInfo, RenderError> {
        if self.kind == EditKind::Xlsx {
            return sheet::render_anchor(self, sheet, anchor, scale, out);
        }
        Err(RenderError::Engine)
    }
}

impl std::fmt::Debug for EditDoc {
    /// Sizes and counters only; the document's content never reaches a log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditDoc").field("kind", &self.kind).field("revision", &self.revision).field("original_len", &self.original.len()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_is_unsupported_in_w1() {
        for kind in [EditKind::Docx, EditKind::Xlsx, EditKind::Pptx] {
            assert!(EditDoc::open(kind, b"PK", &Limits::default()).is_err());
            assert_eq!(EditDoc::new_from(kind, Template::Blank).unwrap_err(), EditError::Unsupported);
        }
        assert_eq!(preserve::carry(b"a", b"b".to_vec()).unwrap_err(), SaveError::Unsupported);
    }

    #[test]
    fn op_debug_never_shows_content() {
        let s = format!("{:?}", Op::InsertText("secret words".into()));
        assert!(!s.contains("secret"));
        let s = format!("{:?}", Op::Image { bytes: vec![1, 2, 3], png: true });
        assert!(s.contains("len 3"));
    }
}
