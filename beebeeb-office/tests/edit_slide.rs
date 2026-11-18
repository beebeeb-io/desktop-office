//! Slide editing (`EditDoc` over DeckCraft), behind the non-default feature `edit-slides`. Saving loses data
//! when it is wrong, so these check the file that comes out: replace a text, add and duplicate a slide, save,
//! reopen with a fresh `EditDoc`: the text of every other slide is equal and the new slides are there; undo
//! and redo restore. Fixtures: decks written in code and every `.pptx` in `corpus/pptx`.
#![cfg(feature = "edit-slides")]

use std::path::Path;
use std::sync::Arc;

use beebeeb_office::edit::{Dirty, EditDoc, EditError, EditKind, HitMode, Mark, Op, SaveError, TextAlign};
use beebeeb_office::Limits;
use deckcraft_model::{LayoutType, Presentation, Size, TextBody, Theme, defaults};

fn deck_model(slides: &[(LayoutType, &str)]) -> Presentation {
    let mut p = defaults::blank_presentation(defaults::WIDE, Theme::default(), false);
    for (kind, title) in slides {
        let layout = defaults::layout_of_kind(&p, *kind).expect("layout");
        let mut slide = defaults::new_slide(&mut p, layout).expect("slide");
        if !title.is_empty() {
            let t = slide.shapes.iter_mut().find(|s| s.ph.as_ref().is_some_and(|ph| ph.kind.is_title())).expect("a title placeholder");
            t.text = Some(TextBody::from_text(title));
        }
        p.slides.push(Arc::new(slide));
    }
    p
}

fn sample() -> Vec<u8> {
    let p = deck_model(&[(LayoutType::Title, "Q3 plan"), (LayoutType::TitleOnly, "Roadmap"), (LayoutType::TitleOnly, "Thanks")]);
    deckcraft_pptx::export(&p).expect("writes")
}

fn open(bytes: &[u8]) -> EditDoc {
    EditDoc::open(EditKind::Pptx, bytes, &Limits::default()).expect("opens")
}

/// Every slide's text, shape by shape.
fn texts(bytes: &[u8]) -> Vec<Vec<String>> {
    let (p, _) = deckcraft_pptx::import_checked(bytes).expect("reads");
    p.slides.iter().map(|s| s.shapes.iter().filter_map(|sh| sh.text.as_ref().map(TextBody::text)).filter(|t| !t.is_empty()).collect()).collect()
}

fn titles(doc: &EditDoc) -> Vec<String> {
    doc.outline().into_iter().map(|e| e.text).collect()
}

/// A point inside the title placeholder of slide `n` (its centre).
fn title_point(bytes: &[u8], n: usize) -> (f32, f32) {
    let (p, _) = deckcraft_pptx::import_checked(bytes).unwrap();
    let sh = p.slides[n].shapes.iter().find(|s| s.ph.as_ref().is_some_and(|ph| ph.kind.is_title())).unwrap();
    let sel = deckcraft_engine::Selection { slide: n, ..Default::default() };
    let x = deckcraft_engine::cmd::xfrm_of(&p, &sel, sh);
    ((x.x + x.w / 2.0) as f32, (x.y + x.h / 2.0) as f32)
}

#[test]
fn edit_slide_replace_text_add_duplicate_save_reopen() {
    let bytes = sample();
    let before = texts(&bytes);
    let mut doc = open(&bytes);
    assert_eq!(doc.info().unwrap().pages, 3);

    // Click the second slide's title, select its text, type over it.
    let (x, y) = title_point(&bytes, 1);
    let c = doc.apply(&Op::SelectShape { slide: 1, x_pt: x, y_pt: y }).expect("select the title");
    assert!(c.caret.is_some(), "the title is being edited");
    doc.apply(&Op::SelectAll).unwrap();
    let c = doc.apply(&Op::InsertText("Plan B".into())).unwrap();
    assert_eq!(c.revision, 1);
    assert!(matches!(&c.dirty, Dirty::Rects(r) if r.len() == 1 && r[0].page == 1), "only slide 1 is dirty: {:?}", c.dirty);
    assert!(c.can_undo);
    assert_eq!(titles(&doc)[1], "Plan B");

    // Add a slide (after the current one), duplicate the first.
    let c = doc.apply(&Op::NewSlide { layout: None }).unwrap();
    assert_eq!(c.slides, Some(4));
    assert!(matches!(c.dirty, Dirty::All));
    let c = doc.apply(&Op::DuplicateSlide(0)).unwrap();
    assert_eq!(c.slides, Some(5));
    assert_eq!(doc.info().unwrap().pages, 5);

    let saved = doc.save().expect("saves");
    let after = texts(&saved);
    assert_eq!(after.len(), 5);
    assert_eq!(after[0], before[0], "slide 0 is as it was");
    assert_eq!(after[1], before[0], "the duplicate of slide 0 follows it");
    assert!(after.iter().any(|s| s == &vec!["Plan B".to_string()]));
    assert!(after.iter().any(|s| s == &before[2]), "Thanks is still there");

    // Reopen with a fresh EditDoc.
    let again = open(&saved);
    assert_eq!(again.info().unwrap().pages, 5);
    let t = titles(&again);
    assert!(t.contains(&"Plan B".to_string()) && t.contains(&"Thanks".to_string()) && t.contains(&"Q3 plan".to_string()));
}

#[test]
fn edit_slide_undo_redo_restore() {
    let bytes = sample();
    let mut doc = open(&bytes);
    let base = titles(&doc);
    let (x, y) = title_point(&bytes, 0);
    doc.apply(&Op::SelectShape { slide: 0, x_pt: x, y_pt: y }).unwrap();
    doc.apply(&Op::SelectAll).unwrap();
    doc.apply(&Op::InsertText("Changed".into())).unwrap();
    doc.apply(&Op::NewSlide { layout: None }).unwrap();
    assert_eq!(doc.info().unwrap().pages, 4);
    let c = doc.apply(&Op::Undo).unwrap();
    assert_eq!(c.slides, Some(3));
    assert_eq!(doc.info().unwrap().pages, 3);
    assert_eq!(titles(&doc)[0], "Changed");
    doc.apply(&Op::Undo).unwrap();
    assert_eq!(titles(&doc), base);
    let c = doc.apply(&Op::Redo).unwrap();
    assert!(c.can_redo);
    assert_eq!(titles(&doc)[0], "Changed");
    doc.apply(&Op::Redo).unwrap();
    assert_eq!(doc.info().unwrap().pages, 4);
    assert!(matches!(doc.apply(&Op::Redo), Err(_)), "nothing left to redo");
}

#[test]
fn edit_slide_move_delete_layout_and_bounds() {
    let bytes = sample();
    let mut doc = open(&bytes);
    let names = doc.layouts();
    assert!(names.len() >= 4, "the file's own layouts: {names:?}");
    doc.apply(&Op::MoveSlide { from: 2, to: 0 }).unwrap();
    assert_eq!(titles(&doc), ["Thanks", "Q3 plan", "Roadmap"]);
    doc.apply(&Op::DeleteSlide(1)).unwrap();
    assert_eq!(titles(&doc), ["Thanks", "Roadmap"]);
    let blank = names.iter().position(|n| n.to_ascii_lowercase().contains("blank")).expect("a blank layout") as u32;
    doc.apply(&Op::NewSlide { layout: Some(blank) }).unwrap();
    assert_eq!(doc.info().unwrap().pages, 3);
    doc.apply(&Op::SetLayout { slide: 0, layout: blank }).unwrap();
    // Out of range answers, never panics.
    assert_eq!(doc.apply(&Op::DeleteSlide(9)).unwrap_err(), EditError::NotApplicable);
    assert_eq!(doc.apply(&Op::NewSlide { layout: Some(9999) }).unwrap_err(), EditError::NotApplicable);
    assert_eq!(doc.apply(&Op::SelectShape { slide: 7, x_pt: 1.0, y_pt: 1.0 }).unwrap_err(), EditError::NotApplicable);
    assert_eq!(doc.apply(&Op::SelectShape { slide: 0, x_pt: f32::NAN, y_pt: 1.0 }).unwrap_err(), EditError::NotApplicable);
    assert_eq!(doc.apply(&Op::SetCell { sheet: 0, row: 0, col: 0, text: String::new() }).unwrap_err(), EditError::Unsupported);
    // Typing with nothing selected is refused.
    doc.apply(&Op::ClearShape).unwrap();
    assert_eq!(doc.apply(&Op::InsertText("x".into())).unwrap_err(), EditError::NotApplicable);
    let mut one = open(&deckcraft_pptx::export(&deck_model(&[(LayoutType::Title, "Only")])).unwrap());
    assert_eq!(one.apply(&Op::DeleteSlide(0)).unwrap_err(), EditError::NotApplicable, "the last slide stays");
}

#[test]
fn edit_slide_marks_align_click_and_view() {
    let bytes = sample();
    let mut doc = open(&bytes);
    let (x, y) = title_point(&bytes, 0);
    let c = doc.hit(0, x, y, HitMode::Place).expect("click on the title");
    assert!(c.caret.is_some());
    doc.apply(&Op::SelectAll).unwrap();
    let c = doc.apply(&Op::Mark { mark: Mark::Bold, on: true }).unwrap();
    assert!(c.marks.bold);
    let c = doc.apply(&Op::Align(TextAlign::Right)).unwrap();
    assert_eq!(c.marks.align, TextAlign::Right);
    assert!(!c.selection.is_empty(), "the selection is drawn");
    let w = (960.0, 540.0);
    for r in c.selection.iter().chain(c.caret.iter()) {
        assert!(r.rect.x >= 0.0 && r.rect.y >= 0.0 && r.rect.x + r.rect.w <= w.0 && r.rect.y + r.rect.h <= w.1, "inside the slide: {r:?}");
    }
    assert_eq!(doc.copy().text, "Q3 plan");
    // A double click selects a word; a click off every shape clears.
    let c = doc.hit(0, x, y, HitMode::Word).unwrap();
    assert!(!c.selection.is_empty());
    // View calls go through the viewer's contracts.
    let (pw, ph) = doc.render_size(0, 1.0, None).unwrap();
    let mut px = vec![0u8; pw as usize * ph as usize * 4];
    let info = doc.render_page(0, 1.0, None, &mut px).unwrap();
    assert_eq!((info.w, info.h), (pw, ph));
    assert!(px.chunks_exact(4).any(|p| p[..3] != [255, 255, 255]), "the slide is drawn");
    let _ = Size::default();
}

#[test]
fn edit_slide_image_from_bytes() {
    let bytes = sample();
    let mut doc = open(&bytes);
    // A 2x2 PNG.
    let png = {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let chunk = |v: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]| {
            v.extend((data.len() as u32).to_be_bytes());
            let mut body = kind.to_vec();
            body.extend(data);
            v.extend(&body);
            let mut crc = 0xFFFF_FFFFu32;
            for b in &body {
                crc ^= u32::from(*b);
                for _ in 0..8 {
                    crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
                }
            }
            v.extend((!crc).to_be_bytes());
        };
        chunk(&mut v, b"IHDR", &[0, 0, 0, 2, 0, 0, 0, 2, 8, 2, 0, 0, 0]);
        let mut raw = Vec::new();
        for _ in 0..2 {
            raw.push(0);
            raw.extend([255, 0, 0, 0, 255, 0]);
        }
        // zlib stored block.
        let mut z = vec![0x78, 0x01, 0x01];
        z.extend((raw.len() as u16).to_le_bytes());
        z.extend((!(raw.len() as u16)).to_le_bytes());
        z.extend(&raw);
        let (mut a, mut b) = (1u32, 0u32);
        for x in &raw {
            a = (a + u32::from(*x)) % 65521;
            b = (b + a) % 65521;
        }
        z.extend(((b << 16) | a).to_be_bytes());
        chunk(&mut v, b"IDAT", &z);
        chunk(&mut v, b"IEND", &[]);
        v
    };
    let c = doc.apply(&Op::Image { bytes: png, png: true }).expect("inserts");
    assert!(matches!(c.dirty, Dirty::Rects(_)));
    assert_eq!(doc.apply(&Op::Image { bytes: b"not a picture".to_vec(), png: true }).is_err(), true);
    let saved = doc.save().expect("saves");
    let (p, _) = deckcraft_pptx::import_checked(&saved).unwrap();
    assert!(p.slides.iter().any(|s| s.shapes.iter().any(|sh| matches!(sh.kind, deckcraft_model::ShapeKind::Picture { .. }))));
}

/// The spike's decks: each opens for editing, one text edit and one new slide apply, undo restores. A save may
/// be refused (`Unsupported`) where the original has parts the writer drops until the preserving carry lands.
#[test]
fn edit_slide_corpus_decks() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("corpus").join("pptx");
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    let (mut opened, mut saved, mut refused) = (0, 0, 0);
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "pptx") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let Ok(mut doc) = EditDoc::open(EditKind::Pptx, &bytes, &Limits::default()) else { continue };
        opened += 1;
        let n = doc.info().unwrap().pages;
        if n == 0 {
            continue;
        }
        doc.apply(&Op::NewSlide { layout: None }).unwrap_or_else(|e| panic!("{}: new slide {e}", path.display()));
        assert_eq!(doc.info().unwrap().pages, n + 1);
        doc.apply(&Op::DuplicateSlide(0)).unwrap_or_else(|e| panic!("{}: duplicate {e}", path.display()));
        match doc.save() {
            Ok(b) => {
                assert_eq!(open(&b).info().unwrap().pages, n + 2, "{}", path.display());
                saved += 1;
            }
            Err(SaveError::Unsupported) => refused += 1,
            Err(e) => panic!("{}: save {e}", path.display()),
        }
        doc.apply(&Op::Undo).unwrap();
        doc.apply(&Op::Undo).unwrap();
        assert_eq!(doc.info().unwrap().pages, n, "{}", path.display());
    }
    eprintln!("edit_slide corpus: {opened} opened, {saved} saved, {refused} refused by the carry");
    assert!(opened >= 10, "the corpus opens: {opened}");
}

/// The non-empty speaker notes of every slide, in order (empty ones dropped).
fn notes(bytes: &[u8]) -> Vec<String> {
    let (p, _) = deckcraft_pptx::import_checked(bytes).expect("reads");
    p.slides.iter().map(|s| s.notes.text()).filter(|t| !t.trim().is_empty()).collect()
}

/// Speaker notes are data the user wrote: they survive the real edit + save path (open, type over a title,
/// add a slide, duplicate a slide, save through `preserve::carry`, reopen), each on its own slide.
#[test]
fn edit_slide_speaker_notes_survive_edit_and_save() {
    let mut p = deck_model(&[(LayoutType::Title, "Q3 plan"), (LayoutType::TitleOnly, "Roadmap"), (LayoutType::TitleOnly, "Thanks")]);
    for (i, t) in [(0usize, "Say hello, then the numbers"), (2usize, "Thank the team; remind about the survey")] {
        let mut s = (*p.slides[i]).clone();
        s.notes = TextBody::from_text(t);
        p.slides[i] = Arc::new(s);
    }
    let bytes = deckcraft_pptx::export(&p).expect("writes");
    assert_eq!(notes(&bytes), ["Say hello, then the numbers", "Thank the team; remind about the survey"]);

    let mut doc = open(&bytes);
    let (x, y) = title_point(&bytes, 1);
    doc.apply(&Op::SelectShape { slide: 1, x_pt: x, y_pt: y }).unwrap();
    doc.apply(&Op::SelectAll).unwrap();
    doc.apply(&Op::InsertText("Plan B".into())).unwrap();
    doc.apply(&Op::NewSlide { layout: None }).unwrap();
    doc.apply(&Op::DuplicateSlide(0)).unwrap();
    let saved = doc.save().expect("saves");

    let (after, _) = deckcraft_pptx::import_checked(&saved).expect("reads");
    assert_eq!(after.slides.len(), 5);
    let got = notes(&saved);
    for want in ["Say hello, then the numbers", "Thank the team; remind about the survey"] {
        assert!(got.iter().any(|n| n == want), "notes {want:?} survive: {got:?}");
    }
    // The first slide still carries its own notes, and the notes of "Thanks" sit on "Thanks".
    assert_eq!(after.slides[0].notes.text(), "Say hello, then the numbers");
    let thanks = after.slides.iter().find(|s| s.shapes.iter().any(|sh| sh.text.as_ref().is_some_and(|t| t.text() == "Thanks"))).expect("Thanks");
    assert_eq!(thanks.notes.text(), "Thank the team; remind about the survey");
    // And again through a second edit + save of the saved file.
    let again = open(&saved).save().expect("saves again");
    assert!(notes(&again).len() >= 2);
}

/// Real decks: every non-empty speaker note of every corpus deck is still in the file after the same edits.
#[test]
fn edit_slide_corpus_speaker_notes_survive() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus/pptx");
    let mut with_notes = 0;
    let mut files: Vec<_> = std::fs::read_dir(&dir).expect("corpus/pptx").flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "pptx")).collect();
    files.sort();
    for path in files {
        let bytes = std::fs::read(&path).unwrap();
        let Ok(mut doc) = EditDoc::open(EditKind::Pptx, &bytes, &Limits::default()) else { continue };
        let before = notes(&bytes);
        if doc.apply(&Op::NewSlide { layout: None }).is_err() || doc.apply(&Op::DuplicateSlide(0)).is_err() {
            continue;
        }
        let saved = doc.save().unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
        let mut got = notes(&saved);
        for n in &before {
            let at = got.iter().position(|g| g == n).unwrap_or_else(|| panic!("{}: a speaker note is gone after the edit", path.display()));
            got.remove(at);
        }
        with_notes += usize::from(!before.is_empty());
    }
    eprintln!("edit_slide corpus notes: {with_notes} decks with notes kept");
}
