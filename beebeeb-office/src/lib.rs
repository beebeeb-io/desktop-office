//! `beebeeb-office`: the viewer (and later editor) crate on top of WordCraft, GridCraft and
//! DeckCraft.  It lives in `beebeeb-io/desktop-office` and is linked only by the sandboxed
//! helper binary; the app process never depends on it.
//!
//! Public API for now: three functions that open a document from bytes in memory and return
//! page, sheet or slide counts.

/// Errors returned by the three open functions.
#[derive(Debug)]
pub enum OpenError {
    /// The .docx reader failed.
    Docx(wordcraft_docx::DocxError),
    /// The .doc (binary) reader failed.
    Docbin(wordcraft_docbin::DocbinError),
    /// The .xlsx reader failed.
    Xlsx(gridcraft_xlsx::IoError),
    /// The .pptx reader failed.
    Pptx(deckcraft_pptx::PptxError),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Docx(e) => write!(f, "docx: {e}"),
            Self::Docbin(e) => write!(f, "doc: {e}"),
            Self::Xlsx(e) => write!(f, "xlsx: {e}"),
            Self::Pptx(e) => write!(f, "pptx: {e}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// Open a `.docx` file from bytes and return its page count.
///
/// The document is parsed and laid out so pages can be counted.  No rendering happens;
/// no file, network or process access occurs.
pub fn open_docx(bytes: &[u8]) -> Result<usize, OpenError> {
    let doc = wordcraft_docx::read_with(bytes, &wordcraft_docx::ReadOptions::default())
        .map_err(OpenError::Docx)?;
    let mut cache = wordcraft_layout::LayoutCache::new();
    let laid_out = wordcraft_layout::layout(&doc, &mut cache, &wordcraft_layout::LayoutOptions::default());
    Ok(laid_out.pages.len())
}

/// Open a `.doc` (binary Word) file from bytes and return its page count.
///
/// The document is parsed and laid out so pages can be counted.  No rendering happens;
/// no file, network or process access occurs.
pub fn open_doc(bytes: &[u8]) -> Result<usize, OpenError> {
    let doc = wordcraft_docbin::read(bytes).map_err(OpenError::Docbin)?;
    let mut cache = wordcraft_layout::LayoutCache::new();
    let laid_out = wordcraft_layout::layout(&doc, &mut cache, &wordcraft_layout::LayoutOptions::default());
    Ok(laid_out.pages.len())
}

/// Open an `.xlsx` file from bytes and return its sheet count.
///
/// No file, network or process access occurs.
pub fn open_xlsx(bytes: &[u8]) -> Result<usize, OpenError> {
    let (workbook, _report) = gridcraft_xlsx::read_xlsx(bytes).map_err(OpenError::Xlsx)?;
    Ok(workbook.sheets.len())
}

/// Open a `.pptx` file from bytes and return its slide count.
///
/// No file, network or process access occurs.
pub fn open_pptx(bytes: &[u8]) -> Result<usize, OpenError> {
    let presentation = deckcraft_pptx::import(bytes).map_err(OpenError::Pptx)?;
    Ok(presentation.slides.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Build a minimal .docx in memory: a ZIP with content types, a rels part and a body with
    /// two sections (one explicit page break), producing at least 2 pages.
    fn make_docx_2_pages() -> Vec<u8> {
        use std::io::Write;
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        // [Content_Types].xml
        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#).unwrap();

        // _rels/.rels
        zip.start_file("_rels/.rels", opts).unwrap();
        zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#).unwrap();

        // word/document.xml — two sections separated by a page break
        zip.start_file("word/document.xml", opts).unwrap();
        zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Page one</w:t></w:r></w:p>
    <w:p><w:r><w:br w:type="page"/></w:r></w:p>
    <w:p><w:r><w:t>Page two</w:t></w:r></w:p>
    <w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr>
  </w:body>
</w:document>"#).unwrap();

        zip.finish().unwrap().into_inner()
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
        assert!(pages >= 2, "expected at least 2 pages, got {pages}");
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
