//! Word editing (`EditDoc` over WordCraft). Saving loses data when it is wrong, so these check the file that
//! comes out: three edits, save, reopen with a fresh `EditDoc`, the edits are there and the rest of the body
//! text is equal; undo and redo restore; a caret and the click on it agree. Fixtures: documents written in code
//! and every `.docx` in `corpus/docx`.
#![cfg(feature = "edit")]

mod common;

use std::path::Path;

use beebeeb_office::edit::{Change, Dirty, EditDoc, EditKind, HitMode, ListKind, Mark, Motion, Op, ParaStyle, SaveError, TextAlign};
use beebeeb_office::{DocKind, Document, Limits};
use common::docx_package;

fn para(text: &str) -> String {
    format!("<w:p><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>", common::xml_escape(text))
}

fn sample() -> Vec<u8> {
    let body: String = ["First paragraph of the sample.", "Second one, a little longer than the first.", "Third and last."].iter().map(|t| para(t)).collect();
    docx_package(&body, "", &[])
}

fn open(bytes: &[u8]) -> EditDoc {
    EditDoc::open(EditKind::Docx, bytes, &Limits::default()).expect("opens")
}

/// The whole body as the engine copies it.
fn text_of(doc: &mut EditDoc) -> String {
    // Select All inside a table grows cell, table, body: repeat until the text stops growing.
    let mut t: Option<String> = None;
    for _ in 0..6 {
        doc.apply(&Op::SelectAll).unwrap();
        let now = doc.copy().text;
        if t.as_ref() == Some(&now) {
            break;
        }
        t = Some(now);
    }
    let t = t.unwrap();
    doc.apply(&Op::Move { to: Motion::DocStart, extend: false }).unwrap();
    t
}

/// Three edits: a word at the start, a word at the end, bold on the first line.
fn three_edits(doc: &mut EditDoc) -> Result<(), beebeeb_office::edit::EditError> {
    doc.apply(&Op::Move { to: Motion::DocStart, extend: false })?;
    doc.apply(&Op::InsertText("ZQXstart ".into()))?;
    doc.apply(&Op::Move { to: Motion::DocEnd, extend: false })?;
    doc.apply(&Op::InsertText("ZQXend".into()))?;
    doc.apply(&Op::Move { to: Motion::DocStart, extend: false })?;
    doc.apply(&Op::Move { to: Motion::LineEnd, extend: true })?;
    doc.apply(&Op::Mark { mark: Mark::Bold, on: true })?;
    Ok(())
}

#[test]
fn edit_word_three_edits_survive_save_and_reopen() {
    let bytes = sample();
    let mut doc = open(&bytes);
    let before = text_of(&mut doc);
    three_edits(&mut doc).unwrap();
    let edited = text_of(&mut doc);
    assert!(edited.starts_with("ZQXstart ") && edited.ends_with("ZQXend"), "{edited:?}");
    assert!(edited.contains(before.trim_end()), "rest of the body text is kept");
    let saved = doc.save().expect("saves");
    let mut again = open(&saved);
    assert_eq!(text_of(&mut again), edited);
    // The bold reached the file: the first line reads as bold in the reopened document.
    again.apply(&Op::Move { to: Motion::DocStart, extend: false }).unwrap();
    let ch = again.apply(&Op::Move { to: Motion::Right, extend: true }).unwrap();
    assert!(ch.marks.bold);
}

#[test]
fn edit_word_undo_and_redo_restore() {
    let mut doc = open(&sample());
    let before = text_of(&mut doc);
    doc.apply(&Op::InsertText("ZQXone".into())).unwrap();
    doc.apply(&Op::InsertParagraph).unwrap();
    doc.apply(&Op::InsertText("ZQXtwo".into())).unwrap();
    let edited = text_of(&mut doc);
    assert_ne!(edited, before);
    let mut last = None;
    for _ in 0..8 {
        let ch = doc.apply(&Op::Undo).unwrap();
        let done = !ch.can_undo;
        last = Some(ch);
        if done {
            break;
        }
    }
    assert!(!last.unwrap().can_undo);
    assert_eq!(text_of(&mut doc), before);
    let mut redo = None;
    for _ in 0..8 {
        let ch = doc.apply(&Op::Redo).unwrap();
        let done = !ch.can_redo;
        redo = Some(ch);
        if done {
            break;
        }
    }
    assert!(!redo.unwrap().can_redo);
    assert_eq!(text_of(&mut doc), edited);
}

#[test]
fn edit_word_revision_rises_with_content_only() {
    let mut doc = open(&sample());
    assert_eq!(doc.revision(), 0);
    let ch = doc.apply(&Op::Move { to: Motion::DocEnd, extend: false }).unwrap();
    assert_eq!(ch.revision, 0, "a caret move is not a change");
    let ch = doc.apply(&Op::InsertText("x".into())).unwrap();
    assert_eq!(ch.revision, 1);
    assert!(ch.can_undo);
}

fn assert_caret_hits(doc: &mut EditDoc, ch: &Change) {
    let c = ch.caret.expect("a caret");
    let again = doc.hit(c.page, c.rect.x, c.rect.y + c.rect.h / 2.0, HitMode::Place).expect("hit");
    let d = again.caret.expect("a caret after the click");
    assert_eq!(d.page, c.page);
    assert!((d.rect.x - c.rect.x).abs() < 1.0 && (d.rect.y - c.rect.y).abs() < 1.0, "{c:?} vs {d:?}");
}

#[test]
fn edit_word_caret_then_hit_round_trips() {
    let mut doc = open(&sample());
    for to in [Motion::DocStart, Motion::DocEnd] {
        let ch = doc.apply(&Op::Move { to, extend: false }).unwrap();
        assert_caret_hits(&mut doc, &ch);
    }
    // In the middle of a line too.
    doc.apply(&Op::Move { to: Motion::DocStart, extend: false }).unwrap();
    for _ in 0..7 {
        doc.apply(&Op::Move { to: Motion::Right, extend: false }).unwrap();
    }
    let ch = doc.apply(&Op::Move { to: Motion::Right, extend: false }).unwrap();
    assert_caret_hits(&mut doc, &ch);
    // A word and a paragraph select; the selection has rectangles inside the page.
    let c = ch.caret.unwrap();
    let w = doc.hit(c.page, c.rect.x, c.rect.y + 2.0, HitMode::Word).unwrap();
    assert!(!w.selection.is_empty());
    let p = doc.hit(c.page, c.rect.x, c.rect.y + 2.0, HitMode::Paragraph).unwrap();
    assert!(!p.selection.is_empty());
    let info = doc.info().unwrap();
    for r in p.selection.iter().chain(&p.caret) {
        let (pw, ph) = info.sizes_pt[r.page as usize];
        assert!(r.rect.x >= 0.0 && r.rect.y >= 0.0 && r.rect.x + r.rect.w <= pw && r.rect.y + r.rect.h <= ph);
    }
    // Off the page and not-a-number are refused.
    assert!(doc.hit(9, 10.0, 10.0, HitMode::Place).is_err());
    assert!(doc.hit(0, f32::NAN, 10.0, HitMode::Place).is_err());
}

#[test]
fn edit_word_dirty_names_the_changed_page_and_pages_when_the_count_moves() {
    let body: String = (0..3).map(|i| para(&format!("Page {i}"))).collect::<Vec<_>>().join("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>");
    let mut doc = open(&docx_package(&body, "", &[]));
    let pages = doc.info().unwrap().pages;
    assert!(pages >= 3);
    doc.apply(&Op::Move { to: Motion::DocEnd, extend: false }).unwrap();
    let ch = doc.apply(&Op::InsertText("tail".into())).unwrap();
    match ch.dirty {
        Dirty::Rects(r) => assert!(r.iter().all(|p| p.page == pages - 1), "{r:?}"),
        Dirty::All => panic!("one page changed"),
    }
    assert!(ch.pages.is_none());
    let ch = doc.apply(&Op::PageBreak).unwrap();
    assert!(ch.pages.is_some());
    assert_eq!(ch.dirty, Dirty::All);
    assert_eq!(doc.info().unwrap().pages, pages + 1);
}

#[test]
fn edit_word_format_and_insert_ops_apply() {
    let mut doc = open(&sample());
    doc.apply(&Op::SelectAll).unwrap();
    for op in [
        Op::FontSize(14.0),
        Op::FontFamily("Arial".into()),
        Op::TextColour(beebeeb_office::edit::Rgb { r: 200, g: 0, b: 0 }),
        Op::Highlight(Some(beebeeb_office::edit::Rgb { r: 255, g: 255, b: 0 })),
        Op::Highlight(None),
        Op::Mark { mark: Mark::Italic, on: true },
        Op::Mark { mark: Mark::Underline, on: true },
        Op::Mark { mark: Mark::Strike, on: true },
        Op::Align(TextAlign::Center),
        Op::Indent(1),
        Op::Indent(-1),
    ] {
        doc.apply(&op).unwrap_or_else(|e| panic!("{op:?}: {e}"));
    }
    let ch = doc.apply(&Op::ParaStyle(ParaStyle::Heading1)).unwrap();
    assert_eq!(ch.marks.style, ParaStyle::Heading1);
    assert_eq!(ch.marks.align, TextAlign::Center);
    assert!(ch.marks.italic && ch.marks.underline && ch.marks.strike);
    assert!(doc.outline().iter().any(|o| o.level == 1));
    let ch = doc.apply(&Op::List(ListKind::Bullet)).unwrap();
    assert_eq!(ch.marks.list, ListKind::Bullet);
    let ch = doc.apply(&Op::List(ListKind::None)).unwrap();
    assert_eq!(ch.marks.list, ListKind::None);
    doc.apply(&Op::Move { to: Motion::DocEnd, extend: false }).unwrap();
    doc.apply(&Op::PageBreak).unwrap();
    doc.apply(&Op::Image { bytes: common::canary_png(), png: true }).unwrap();
    doc.apply(&Op::Table { rows: 2, cols: 3 }).unwrap();
    doc.apply(&Op::Link { text: "site".into(), url: "https://example.com".into() }).unwrap();
    // Refused: a file link, a table past the cap, a picture that is no picture, a sheet op.
    assert!(doc.apply(&Op::Link { text: "x".into(), url: "file:///etc/passwd".into() }).is_err());
    assert!(doc.apply(&Op::Table { rows: 0, cols: 2 }).is_err());
    assert!(doc.apply(&Op::Image { bytes: vec![1, 2, 3], png: true }).is_err());
    assert!(doc.apply(&Op::AutoSum).is_err());
    // The result is a file that opens, with the table and the picture.
    let saved = doc.save().unwrap();
    let again = open(&saved);
    assert!(again.info().unwrap().pages >= 2);
    assert!(Document::open(DocKind::Docx, &saved, &Limits::default()).is_ok());
}

#[test]
fn edit_word_view_calls_follow_the_edit() {
    let mut doc = open(&sample());
    let w = doc.words();
    assert!(w >= 15);
    doc.apply(&Op::InsertText("alpha beta gamma ".into())).unwrap();
    assert_eq!(doc.words(), w + 3);
    let (pw, ph) = doc.render_size(0, 1.0, None).unwrap();
    let mut buf = vec![0u8; (pw * ph * 4) as usize];
    let info = doc.render_page(0, 1.0, None, &mut buf).unwrap();
    assert_eq!((info.w, info.h), (pw, ph));
    assert!(common::colour_stats(&buf).0 > 1, "text is drawn");
    assert!(doc.render_size(7, 1.0, None).is_err());
}

#[test]
fn edit_word_paste_splits_lines_into_paragraphs() {
    let mut doc = open(&sample());
    doc.apply(&Op::Paste { text: "one\r\ntwo\nthree".into() }).unwrap();
    let t = text_of(&mut doc);
    assert!(t.starts_with("one\ntwo\nthree"), "{t:?}");
}

#[test]
fn edit_word_copy_gives_text_and_html() {
    let mut doc = open(&sample());
    assert_eq!(doc.copy().text, "");
    doc.apply(&Op::Move { to: Motion::LineEnd, extend: true }).unwrap();
    let clip = doc.copy();
    assert_eq!(clip.text, "First paragraph of the sample.");
    assert!(clip.html.unwrap().contains("<p>First paragraph"));
}

#[test]
fn edit_word_binary_doc_opens_but_never_saves() {
    let mut doc = EditDoc::open(EditKind::Docx, &common::dense_doc(), &Limits::default()).unwrap();
    doc.apply(&Op::InsertText("x".into())).unwrap();
    assert_eq!(doc.save().unwrap_err(), SaveError::Unsupported);
}

fn corpus_files() -> Vec<std::path::PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus/docx");
    let mut v: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "docx")).collect();
    v.sort();
    v
}

/// Every corpus file that opens: three edits, save, reopen. A save the carry step refuses (the file has a part
/// the engine does not write) must say `Unsupported`, never lose it silently; at least one file saves.
#[test]
fn edit_word_corpus_edits_survive_save() {
    let (mut saved, mut refused, mut opened, mut skipped) = (0, 0, 0, 0);
    for path in corpus_files() {
        let bytes = std::fs::read(&path).unwrap();
        let Ok(mut doc) = EditDoc::open(EditKind::Docx, &bytes, &Limits::default()) else { continue };
        opened += 1;
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let before = text_of(&mut doc);
        // A document with editing restricted refuses the edit (NotApplicable) and is left as it was.
        if three_edits(&mut doc).is_err() {
            skipped += 1;
            continue;
        }
        let edited = text_of(&mut doc);
        assert!(edited.contains("ZQXstart ") && edited.contains("ZQXend"), "{name}: edits present {edited:?}");
        assert!(edited.contains(before.trim_end()) || before.trim().is_empty(), "{name}: body text kept\n{before:?}\n{edited:?}");
        // Undo and redo restore, on the real file too.
        let mut steps = 0;
        while doc.apply(&Op::Undo).map(|c| c.can_undo).unwrap_or(false) && steps < 50 {
            steps += 1;
        }
        assert_eq!(text_of(&mut doc), before, "{name}: undo");
        steps = 0;
        while doc.apply(&Op::Redo).map(|c| c.can_redo).unwrap_or(false) && steps < 50 {
            steps += 1;
        }
        assert_eq!(text_of(&mut doc), edited, "{name}: redo");
        match doc.save() {
            Ok(out) => {
                saved += 1;
                let mut again = EditDoc::open(EditKind::Docx, &out, &Limits::default()).unwrap_or_else(|e| panic!("{name}: reopen: {e:?}"));
                assert_eq!(text_of(&mut again), edited, "{name}: reopened text");
            }
            Err(SaveError::Unsupported) => refused += 1,
            Err(e) => panic!("{name}: save: {e}"),
        }
    }
    eprintln!("edit_word corpus: opened {opened}, saved {saved}, carry refused {refused}, edit refused {skipped}");
    assert!(opened >= 10 && saved >= 1);
}
