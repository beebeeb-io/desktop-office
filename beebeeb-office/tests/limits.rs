//! The limits: a container over them is refused from what it declares, before any reader sees it
//! (counted by the crate's probe of bytes handed to readers); a document over the page limit is
//! refused after layout.

mod common;

use beebeeb_office::{DocKind, Document, Limits, OpenError, limits, prescan, probe};
use common::{crc32, dense_doc, zip_declaring, zip_stored};

/// `Document::open` on `bytes`, with the probe reset first: (the error, bytes handed to readers).
fn open_counting(kind: DocKind, bytes: &[u8], limits: &Limits) -> (Option<OpenError>, u64) {
    probe::reset();
    let err = Document::open(kind, bytes, limits).err();
    (err, probe::bytes_to_readers())
}

#[test]
fn the_test_zip_writer_crc_is_standard() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
}

#[test]
fn an_archive_declaring_too_much_is_refused_before_inflating() {
    const MIB: u32 = 1024 * 1024;
    let names = ["word/document.xml", "word/a.xml", "word/b.xml", "word/c.xml", "word/d.xml", "word/e.xml"];
    // Six entries of 100 MiB each: every one under the readers' own 256 MiB part limit, together
    // over 512 MiB, so only the sum can refuse it. The file is a few hundred bytes.
    let bomb = zip_declaring(&names.map(|n| (n, 100 * MIB)));
    assert!(bomb.len() < 2048, "the archive itself is small: {} bytes", bomb.len());
    assert_eq!(prescan(&bomb, &Limits::default()), Err(OpenError::TooLarge(limits::UNPACKED)));
    let (err, handed) = open_counting(DocKind::Docx, &bomb, &Limits::default());
    assert_eq!(err, Some(OpenError::TooLarge(limits::UNPACKED)), "refused by the pre-scan, not by a reader");
    assert_eq!(handed, 0, "no reader saw the archive, so nothing was inflated");
    // The instrument counts: the same entries declaring 80 MiB each (480 MiB) pass the pre-scan
    // and reach the reader. What the reader makes of a package with no document in it is not
    // this test's question.
    let under = zip_declaring(&names.map(|n| (n, 80 * MIB)));
    assert_eq!(prescan(&under, &Limits::default()), Ok(()));
    let (err, handed) = open_counting(DocKind::Docx, &under, &Limits::default());
    assert_eq!(handed, under.len() as u64, "the reader was handed the archive");
    assert_ne!(err, Some(OpenError::TooLarge(limits::UNPACKED)));
}

#[test]
fn too_many_entries_is_refused() {
    let names: Vec<String> = (0..10_001).map(|i| format!("e{i}")).collect();
    let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b""[..])).collect();
    let over = zip_stored(&entries);
    assert_eq!(prescan(&over, &Limits::default()), Err(OpenError::TooLarge(limits::ENTRIES)));
    let (err, handed) = open_counting(DocKind::Docx, &over, &Limits::default());
    assert_eq!(err, Some(OpenError::TooLarge(limits::ENTRIES)));
    assert_eq!(handed, 0, "no reader saw the archive");
    // 10,000 entries are allowed.
    let at_limit = zip_stored(&entries[..10_000]);
    assert_eq!(prescan(&at_limit, &Limits::default()), Ok(()));
}

#[test]
fn a_compound_file_declaring_too_much_is_refused() {
    let doc = dense_doc();
    assert_eq!(prescan(&doc, &Limits::default()), Ok(()), "the fixture itself is inside the limits");
    // Its WordDocument stream (directory entry 1, in sector 1 after the header and the FAT)
    // declares 600 MiB.
    let mut lying = doc.clone();
    let at = 512 + 512 + 128 + 0x78;
    lying[at..at + 8].copy_from_slice(&(600u64 * 1024 * 1024).to_le_bytes());
    assert_eq!(prescan(&lying, &Limits::default()), Err(OpenError::TooLarge(limits::UNPACKED)));
    let (err, handed) = open_counting(DocKind::Doc, &lying, &Limits::default());
    assert_eq!(err, Some(OpenError::TooLarge(limits::UNPACKED)));
    assert_eq!(handed, 0, "no reader saw the file");
    // Its directory has 4 entries (the root, 2 streams, 1 unused): over a limit of 3.
    let three = Limits { max_entries: 3, ..Limits::default() };
    assert_eq!(prescan(&doc, &three), Err(OpenError::TooLarge(limits::ENTRIES)));
    // A directory chain that leaves the file is damaged.
    let mut broken = doc.clone();
    broken[0x30..0x34].copy_from_slice(&0x7FFF_0000u32.to_le_bytes());
    assert_eq!(prescan(&broken, &Limits::default()), Err(OpenError::Damaged));
}

#[test]
fn too_many_pages_is_refused_after_layout() {
    // 2,000 page breaks, each after a line of text: 2,001 pages.
    let model = wordcraft_doc::Document::from_text(&format!("{}end", "x\n\u{000C}\n".repeat(2_000)));
    let bytes = wordcraft_docx::write(&model).expect("write");
    let (err, handed) = open_counting(DocKind::Docx, &bytes, &Limits::default());
    assert_eq!(err, Some(OpenError::TooLarge(limits::PAGES)));
    assert!(handed > 0, "pages are only known after the reader and the layout");
    // Both sides of the limit, on a three-page document.
    let three = wordcraft_docx::write(&wordcraft_doc::Document::from_text("one\n\u{000C}\ntwo\n\u{000C}\nthree")).expect("write");
    let two_pages = Limits { max_pages: 2, ..Limits::default() };
    assert_eq!(Document::open(DocKind::Docx, &three, &two_pages).err(), Some(OpenError::TooLarge(limits::PAGES)));
    let three_pages = Limits { max_pages: 3, ..Limits::default() };
    assert_eq!(Document::open(DocKind::Docx, &three, &three_pages).map(|d| d.info().pages).ok(), Some(3));
}
