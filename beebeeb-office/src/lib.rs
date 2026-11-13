//! `beebeeb-office`: the viewer (and later editor) crate on top of WordCraft, GridCraft and
//! DeckCraft.  It lives in `beebeeb-io/desktop-office` and is linked only by the sandboxed
//! helper binary; the app process never depends on it.
//!
//! Public API for now: three functions that open a document from bytes in memory and return
//! page, sheet or slide counts — one per document kind.

/// Errors returned by the three open functions.
#[derive(Debug)]
pub enum OpenError {
    /// The .docx reader failed.
    Docx(wordcraft_docx::DocxError),
    /// The .xlsx reader failed.
    Xlsx(gridcraft_xlsx::IoError),
    /// The .pptx reader failed.
    Pptx(deckcraft_pptx::PptxError),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Docx(e) => write!(f, "docx: {e}"),
            Self::Xlsx(e) => write!(f, "xlsx: {e}"),
            Self::Pptx(e) => write!(f, "pptx: {e}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// Open a `.docx` file from bytes and return its page count.
///
/// The document is parsed and laid out so pages can be counted.  No rendering happens.
/// The function takes bytes, never a path or URL; `wordcraft-fonts` may read the system
/// font directory during layout.
pub fn open_docx(bytes: &[u8]) -> Result<usize, OpenError> {
    let doc = wordcraft_docx::read_with(bytes, &wordcraft_docx::ReadOptions::default())
        .map_err(OpenError::Docx)?;
    let mut cache = wordcraft_layout::LayoutCache::new();
    let laid_out = wordcraft_layout::layout(&doc, &mut cache, &wordcraft_layout::LayoutOptions::default());
    Ok(laid_out.pages.len())
}

/// Open an `.xlsx` file from bytes and return its sheet count.
///
/// The function takes bytes, never a path or URL.
pub fn open_xlsx(bytes: &[u8]) -> Result<usize, OpenError> {
    let (workbook, _report) = gridcraft_xlsx::read_xlsx(bytes).map_err(OpenError::Xlsx)?;
    Ok(workbook.sheets.len())
}

/// Open a `.pptx` file from bytes and return its slide count.
///
/// The function takes bytes, never a path or URL.
pub fn open_pptx(bytes: &[u8]) -> Result<usize, OpenError> {
    let presentation = deckcraft_pptx::import(bytes).map_err(OpenError::Pptx)?;
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
