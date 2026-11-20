//! The built-in templates, built by the engines at New (never read from disk, no network, no clock).
//!
//! Docs: a blank WordCraft document gets the template's content through the same ops a person's keystrokes
//! use (the web editor's `templates.ts`: same headings, placeholder lines and the 4 x 4 Invoice table), is
//! written with the .docx writer, and `EditDoc::new_from` reopens those bytes, so the content is editable
//! from the first keystroke. Sheets and Slides: Blank only; any other template is `Unsupported`.

use super::{EditDoc, EditError, EditKind, Op, ParaStyle, Template};
use crate::Limits;

pub(super) fn build(kind: EditKind, template: Template) -> Result<Vec<u8>, EditError> {
    match kind {
        EditKind::Docx => docx(template),
        EditKind::Xlsx | EditKind::Pptx if template != Template::Blank => Err(EditError::Unsupported),
        EditKind::Xlsx => {
            let wb = gridcraft_model::Workbook::new();
            gridcraft_engine::io::save_bytes(&wb, "Book.xlsx").map_err(|_| EditError::Engine)
        }
        EditKind::Pptx => {
            let p = deckcraft_model::defaults::new_presentation(None);
            deckcraft_pptx::export(&p).map_err(|_| EditError::Engine)
        }
    }
}

/// A line of the template: a paragraph in `style`, or the Invoice table.
enum Item {
    Para(ParaStyle, &'static str),
    Empty,
    Table(u32, u32),
}

fn items(template: Template) -> Vec<Item> {
    use Item::*;
    use ParaStyle::{Heading1, Heading2, Normal};
    match template {
        Template::Blank => vec![],
        Template::Letter => vec![
            Para(Normal, "Your Name"),
            Para(Normal, "Your Address"),
            Empty,
            Para(Normal, "Date"),
            Empty,
            Para(Normal, "Dear [Recipient],"),
            Empty,
            Para(Normal, "Sincerely,"),
            Para(Normal, "Your Name"),
        ],
        Template::Invoice => vec![
            Para(Heading1, "Invoice"),
            Para(Normal, "Bill to: [Client name]        Invoice #: 0001        Date: "),
            Empty,
            Table(4, 4),
        ],
        Template::MeetingNotes => {
            vec![Para(Heading1, "Meeting notes"), Para(Heading2, "Attendees"), Para(Heading2, "Agenda"), Para(Heading2, "Action items")]
        }
        Template::Report => vec![
            Para(Heading1, "Report title"),
            Para(Normal, "Prepared by"),
            Empty,
            Para(Heading2, "Summary"),
            Para(Heading2, "Details"),
        ],
    }
}

fn docx(template: Template) -> Result<Vec<u8>, EditError> {
    let blank = wordcraft_docx::write(&wordcraft_doc::Document::new()).map_err(|_| EditError::Engine)?;
    if template == Template::Blank {
        return Ok(blank);
    }
    let engine = super::word::open(&blank, &Limits::default()).map_err(|_| EditError::Engine)?;
    let mut doc = EditDoc { kind: EditKind::Docx, original: blank, revision: 0, engine: Some(engine) };
    let list = items(template);
    let last = list.len().saturating_sub(1);
    for (i, item) in list.iter().enumerate() {
        match item {
            Item::Para(style, text) => {
                doc.apply(&Op::ParaStyle(*style))?;
                doc.apply(&Op::InsertText((*text).to_string()))?;
            }
            Item::Empty => {
                doc.apply(&Op::ParaStyle(ParaStyle::Normal))?;
            }
            Item::Table(rows, cols) => {
                doc.apply(&Op::Table { rows: *rows, cols: *cols })?;
            }
        }
        if i != last && !matches!(item, Item::Table(..)) {
            doc.apply(&Op::InsertParagraph)?;
        }
    }
    doc.save().map_err(|_| EditError::Engine)
}
