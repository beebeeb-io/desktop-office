//! What a Word file holds, not the kind its name says, picks the reader: a `.docx` package named
//! `.doc` (and the reverse) opens, an encrypted package named `.doc` says password, RTF, a web page
//! or a Word XML document named `.doc` is unsupported, and so is a Word 6.0 / Word 95 file. None of
//! these readable files is called damaged.

mod common;

use beebeeb_office::{DocKind, Document, Limits, OpenError, probe};
use common::{cfb_file, dense_doc, dense_docx};

/// `Document::open` on `bytes`, with the probe reset first: (the error, bytes handed to readers).
fn open_counting(kind: DocKind, bytes: &[u8]) -> (Option<OpenError>, u64) {
    probe::reset();
    let err = Document::open(kind, bytes, &Limits::default()).err();
    (err, probe::bytes_to_readers())
}

/// The page count of `bytes` opened as `kind`, or the error.
fn pages(kind: DocKind, bytes: &[u8]) -> Result<u32, OpenError> {
    Document::open(kind, bytes, &Limits::default()).map(|d| d.info().pages)
}

#[test]
fn a_docx_package_named_doc_opens() {
    let bytes = dense_docx();
    assert_eq!(pages(DocKind::Docx, &bytes), Ok(2), "the control: the package opens as what it is");
    assert_eq!(pages(DocKind::Doc, &bytes), Ok(2), "named .doc, it opens with the .docx reader");
}

#[test]
fn a_word_97_file_named_docx_opens() {
    let bytes = dense_doc();
    assert_eq!(pages(DocKind::Doc, &bytes), Ok(2), "the control: the file opens as what it is");
    assert_eq!(pages(DocKind::Docx, &bytes), Ok(2), "named .docx, it opens with the .doc reader");
}

#[test]
fn an_encrypted_package_named_doc_says_password() {
    let locked = wordcraft_docx::encrypt_with_spin_count(&dense_docx(), "correct horse", 1).expect("encrypt");
    assert!(locked.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]), "an encrypted package is a compound file");
    assert_eq!(Document::open(DocKind::Doc, &locked, &Limits::default()).err(), Some(OpenError::Password));
    // A compound file that is not an encrypted package still goes to the .doc reader.
    assert_eq!(pages(DocKind::Doc, &dense_doc()), Ok(2));
}

#[test]
fn an_rtf_file_named_doc_is_unsupported() {
    let rtf = b"{\\rtf1\\ansi\\deff0 {\\fonttbl {\\f0 Arial;}}\\f0\\fs24 The quick brown fox.\\par }";
    for kind in [DocKind::Doc, DocKind::Docx] {
        let (err, handed) = open_counting(kind, rtf);
        assert_eq!(err, Some(OpenError::Unsupported), "RTF named as {kind:?}");
        assert_eq!(handed, 0, "no reader was handed the RTF file ({kind:?})");
    }
}

#[test]
fn a_web_page_named_doc_is_unsupported() {
    let pages_saved_by_word_or_a_report_tool: [&[u8]; 4] = [
        b"<!DOCTYPE html>\n<html><body><p>The quick brown fox.</p></body></html>",
        b"\xEF\xBB\xBF  <html xmlns:o=\"urn:schemas-microsoft-com:office:office\">\r\n<body>Report</body></html>",
        b"<?xml version=\"1.0\"?>\n<!-- generated -->\n<HTML><BODY>Report</BODY></HTML>",
        b"<!doctype HTML PUBLIC \"-//W3C//DTD HTML 4.01//EN\"><html><body>x</body></html>",
    ];
    for (i, page) in pages_saved_by_word_or_a_report_tool.iter().enumerate() {
        let (err, handed) = open_counting(DocKind::Doc, page);
        assert_eq!(err, Some(OpenError::Unsupported), "web page {i} named .doc");
        assert_eq!(handed, 0, "no reader was handed web page {i}");
    }
    // Control: markup that is not a web page (`<htmlish>`) is not taken for one; the .doc reader
    // sees it and says it is damaged.
    let (err, handed) = open_counting(DocKind::Doc, b"<htmlish>not a page</htmlish>");
    assert_eq!(err, Some(OpenError::Damaged), "other markup is not a web page");
    assert!(handed > 0, "the .doc reader was handed the other markup");
}

#[test]
fn a_word_xml_document_named_doc_is_unsupported() {
    let word_2003 = concat!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        r#"<w:wordDocument xmlns:w="http://schemas.microsoft.com/office/word/2003/wordml">"#,
        r#"<w:body><w:p><w:r><w:t>The quick brown fox.</w:t></w:r></w:p></w:body></w:wordDocument>"#,
    );
    let flat_opc = concat!(
        r#"<?xml version="1.0" standalone="yes"?>"#,
        r#"<pkg:package xmlns:pkg="http://schemas.microsoft.com/office/2006/xmlPackage"></pkg:package>"#,
    );
    for (what, xml) in [("Word 2003 XML", word_2003), ("Flat OPC", flat_opc)] {
        let (err, handed) = open_counting(DocKind::Doc, xml.as_bytes());
        assert_eq!(err, Some(OpenError::Unsupported), "{what} named .doc");
        assert_eq!(handed, 0, "no reader was handed the {what} document");
    }
}

/// The start of the WordDocument stream (the FIB's 32-byte fixed part) of the three Word 6.0 /
/// Word 95 files of the corpus, with that stream's length, copied from the files' headers: the
/// test needs no corpus file. Each starts with the older magic `0xA5DC` and `nFib` 101.
const OLDER_WORD_HEADS: [(&str, usize, &str); 3] = [
    ("doc/Word6.doc", 2579, "dca565002dc0090400000000650000000000000000000000000300002c030000"),
    ("doc/Word6_sections.doc", 2647, "dca565002dc0090400000000650000000000000000000000000300002b030000"),
    ("doc/Word95.doc", 2660, "dca565002dc0090400000000650000000000000000000000000300008f030000"),
];

fn from_hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex")).collect()
}

/// A compound file whose WordDocument stream starts with `head`, zero-filled to `len` bytes.
fn doc_with_head(head: &[u8], len: usize) -> Vec<u8> {
    let mut word = head.to_vec();
    word.resize(len, 0);
    cfb_file(&[("WordDocument", word)])
}

#[test]
fn a_word_6_or_95_file_is_unsupported() {
    for (name, len, hex) in OLDER_WORD_HEADS {
        let head = from_hex(hex);
        assert_eq!(head.len(), 32, "{name}: the FIB's fixed part");
        assert_eq!(&head[..4], &[0xDC, 0xA5, 0x65, 0x00], "{name}: the older magic and nFib 101");
        let bytes = doc_with_head(&head, len);
        for kind in [DocKind::Doc, DocKind::Docx] {
            assert_eq!(pages(kind, &bytes), Err(OpenError::Unsupported), "{name} named as {kind:?}");
        }
        // Control: the same stream with the Word 97 magic is a damaged Word 97 file. So it is the
        // older format that says unsupported, not every file the reader turns down.
        let mut w97 = head.clone();
        w97[..2].copy_from_slice(&[0xEC, 0xA5]);
        assert_eq!(pages(DocKind::Doc, &doc_with_head(&w97, len)), Err(OpenError::Damaged), "{name} with the Word 97 magic");
    }
}
