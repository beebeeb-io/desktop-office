//! `beebeeb-office`: the viewer (and later editor) crate on top of WordCraft, GridCraft and
//! DeckCraft.  It lives in `beebeeb-io/desktop-office` and is linked only by the sandboxed
//! helper binary; the app process never depends on it.
//!
//! Every function takes the file's bytes, never a path or a URL, and nothing in this crate opens
//! a file or a connection: a target named inside a document (a linked picture, a template, an
//! `INCLUDEPICTURE` field) is never resolved. The readers below may read the system's font folder.
//!
//! - [`Document`]: Word documents (`.docx`, `.doc`): pages rendered as RGBA8 into a buffer the
//!   caller sized, the outline, the word count.
//! - [`Workbook`]: workbooks (`.xlsx`, `.xlsm`, `.xls`): sheets in tab order, cell blocks with their shown
//!   text, value and formula, merges, frozen panes, and charts and pictures rendered as RGBA8.
//! - [`Deck`]: presentations (`.pptx`, `.pptm`, `.ppsx`): slides rendered as RGBA8, their sizes and titles.
//! - `edit` (feature `edit`, on by default): the editing API, [`edit::EditDoc`]; slides only with `edit-slides`.
//! - [`Limits`] and [`prescan`]: the viewer contract's limits; a container over them is refused
//!   from what it declares, before any reader inflates it.
//! - [`open_docx`], [`open_xlsx`], [`open_pptx`]: page, sheet and slide counts.

mod doc;
#[cfg(feature = "edit")]
pub mod edit;
mod error;
pub mod limits;
#[doc(hidden)]
pub mod probe;
mod raster;
mod sheet;
mod slide;

pub use doc::{DocInfo, DocKind, Document, MAX_OUTLINE_ENTRIES, MAX_OUTLINE_TEXT_BYTES, OutlineEntry};
pub use error::{OpenError, RenderError};
pub use limits::{Limits, prescan};
pub use raster::{Rect, RenderInfo};
pub use sheet::{Anchor, AnchorKind, CellAlign, CellBlock, CellRange, CellText, SheetInfo, SheetKind, SheetsInfo, Workbook};
pub use slide::{Deck, DeckInfo};

/// Open a `.docx` file from bytes and return its page count.
///
/// [`Document::open`] with the default [`Limits`]: pre-scanned, parsed and laid out. No page is
/// rendered.
pub fn open_docx(bytes: &[u8]) -> Result<usize, OpenError> {
    Document::open(DocKind::Docx, bytes, &Limits::default()).map(|d| d.info().pages as usize)
}

/// Open an `.xlsx` file from bytes and return its sheet count.
///
/// Pre-scanned with the default [`Limits`] first.
pub fn open_xlsx(bytes: &[u8]) -> Result<usize, OpenError> {
    prescan(bytes, &Limits::default())?;
    probe::handed(bytes);
    let (workbook, _report) = gridcraft_xlsx::read_xlsx(bytes).map_err(|_| OpenError::Damaged)?;
    Ok(workbook.sheets.len())
}

/// Open a `.pptx` file from bytes and return its slide count.
///
/// Pre-scanned with the default [`Limits`] first.
pub fn open_pptx(bytes: &[u8]) -> Result<usize, OpenError> {
    prescan(bytes, &Limits::default())?;
    probe::handed(bytes);
    let presentation = deckcraft_pptx::import(bytes).map_err(|_| OpenError::Damaged)?;
    Ok(presentation.slides.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Build a 2-page .docx using the upstream Document model and writer.
    /// Page 1 has "Page one", a page break (U+000C), then page 2 has "Page two".
    fn make_docx_2_pages() -> Vec<u8> {
        use wordcraft_doc::{Paragraph, para_block};
        use wordcraft_doc::props::CharProps;

        let mut doc = wordcraft_doc::Document::new();
        // First paragraph with text, then a page break character (U+000C), then more text.
        let p1 = Paragraph::with_text("Page one", CharProps::default());
        let p_break = Paragraph::with_text("\x0c", CharProps::default());
        let p2 = Paragraph::with_text("Page two", CharProps::default());
        doc.body = vec![para_block(p1), para_block(p_break), para_block(p2)];
        wordcraft_docx::write(&doc).expect("write docx")
    }

    /// Build a minimal .xlsx with 3 sheets.
    fn make_xlsx_3_sheets() -> Vec<u8> {
        let mut wb = gridcraft_model::Workbook::default();
        // Workbook::default() gives 1 sheet; add 2 more.
        let sheet2 = gridcraft_model::Sheet::new(String::from("Sheet2"));
        let sheet3 = gridcraft_model::Sheet::new(String::from("Sheet3"));
        wb.sheets.push(Arc::new(sheet2));
        wb.sheets.push(Arc::new(sheet3));
        gridcraft_xlsx::write_xlsx(&wb).expect("write_xlsx")
    }

    /// Build a minimal .pptx with 2 slides.
    fn make_pptx_2_slides() -> Vec<u8> {
        let mut pres = deckcraft_model::Presentation::default();
        // Default gives 1 title slide; add 1 more for a total of 2.
        pres.slides.push(Arc::new(deckcraft_model::Slide::default()));
        deckcraft_pptx::export(&pres).expect("export pptx")
    }

    #[test]
    fn docx_page_count() {
        let docx = make_docx_2_pages();
        let pages = open_docx(&docx).expect("open_docx");
        assert_eq!(pages, 2, "expected 2 pages, got {pages}");
    }

    #[test]
    fn xlsx_sheet_count() {
        let xlsx = make_xlsx_3_sheets();
        let sheets = open_xlsx(&xlsx).expect("open_xlsx");
        assert_eq!(sheets, 3, "expected 3 sheets, got {sheets}");
    }

    #[test]
    fn pptx_slide_count() {
        let pptx = make_pptx_2_slides();
        let slides = open_pptx(&pptx).expect("open_pptx");
        assert_eq!(slides, 2, "expected 2 slides, got {slides}");
    }
}
