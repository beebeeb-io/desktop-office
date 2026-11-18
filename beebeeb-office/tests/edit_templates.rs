//! New document from a built-in template: each Docs template opens, saves and reopens with its headings;
//! the Blank of each kind has one page, sheet or slide; other kinds know Blank only.
#![cfg(feature = "edit")]

use beebeeb_office::Limits;
use beebeeb_office::edit::{EditDoc, EditError, EditKind, Motion, Op, Template};

fn text_of(doc: &mut EditDoc) -> String {
    let mut t: Option<String> = None;
    for _ in 0..6 {
        doc.apply(&Op::SelectAll).unwrap();
        let now = doc.copy().text;
        if t.as_ref() == Some(&now) {
            break;
        }
        t = Some(now);
    }
    doc.apply(&Op::Move { to: Motion::DocStart, extend: false }).unwrap();
    t.unwrap()
}

#[test]
fn edit_templates_docs_keep_their_headings_through_save_and_reopen() {
    let cases: [(Template, &[&str]); 5] = [
        (Template::Blank, &[]),
        (Template::Letter, &["Your Name", "Your Address", "Dear [Recipient],", "Sincerely,"]),
        (Template::Invoice, &["Invoice", "Bill to: [Client name]", "Invoice #: 0001"]),
        (Template::MeetingNotes, &["Meeting notes", "Attendees", "Agenda", "Action items"]),
        (Template::Report, &["Report title", "Prepared by", "Summary", "Details"]),
    ];
    for (template, wants) in cases {
        let mut doc = EditDoc::new_from(EditKind::Docx, template).unwrap_or_else(|e| panic!("{template:?}: {e:?}"));
        let text = text_of(&mut doc);
        for w in wants {
            assert!(text.contains(w), "{template:?} lacks {w:?} in {text:?}");
        }
        let bytes = doc.save().expect("saves");
        let mut again = EditDoc::open(EditKind::Docx, &bytes, &Limits::default()).expect("reopens");
        let text2 = text_of(&mut again);
        for w in wants {
            assert!(text2.contains(w), "{template:?} reopened lacks {w:?} in {text2:?}");
        }
        if template == Template::Invoice {
            // 4 x 4 table: 16 cells, so at least 15 tab/line separators beyond the heading lines.
            assert!(text2.matches('\t').count() + text2.matches('\n').count() >= 15, "invoice table lost: {text2:?}");
        }
        assert_eq!(again.info().unwrap().pages, 1, "{template:?} is one page");
        // Editable from the first keystroke.
        again.apply(&Op::InsertText("ZQX".into())).unwrap();
    }
}

#[test]
fn edit_templates_blank_of_each_kind_is_one_page_sheet_slide() {
    let d = EditDoc::new_from(EditKind::Docx, Template::Blank).unwrap();
    assert_eq!(d.info().unwrap().pages, 1);
    let s = EditDoc::new_from(EditKind::Xlsx, Template::Blank).unwrap();
    assert_eq!(s.sheets().unwrap().sheets.len(), 1);
    // Slides need the non-default feature `edit-slides`; without it they answer Unsupported.
    match EditDoc::new_from(EditKind::Pptx, Template::Blank) {
        Ok(p) => assert_eq!(p.info().unwrap().pages, 1),
        Err(e) => assert_eq!(e, EditError::Unsupported),
    }
}

#[test]
fn edit_templates_sheets_and_slides_know_blank_only() {
    for kind in [EditKind::Xlsx, EditKind::Pptx] {
        for t in [Template::Letter, Template::Invoice, Template::MeetingNotes, Template::Report] {
            assert_eq!(EditDoc::new_from(kind, t).err(), Some(EditError::Unsupported), "{kind:?} {t:?}");
        }
    }
}
