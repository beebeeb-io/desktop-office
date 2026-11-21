//! Word documents, through the patched `wordcraft-engine` (font store off).
//!
//! An open document is a [`WordSession`] in `EditDoc::engine`: the engine's `Session` (selection, undo,
//! commands), the layout of the current revision and a digest per page, from which an edit's `Dirty` is
//! derived (the engine has no invalidation of its own). Every op maps to engine commands by id with
//! parameters built here; nothing a caller sends names a command, a path or a URL scheme the engine would
//! fetch. A picture arrives as bytes and goes in as base64 `data`, never as a path.
//! A `.doc` opens for editing in memory but never saves (`SaveError::Unsupported`).

use std::any::Any;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use serde_json::{Value, json};
use wordcraft_doc::StoryRef;
use wordcraft_doc::props::Highlight;
use wordcraft_engine::{CmdError, Session};
use wordcraft_layout::{DocLayout, Placed};
use wordcraft_render::RenderOptions;

use super::{Change, Clip, Dirty, EditDoc, EditError, HitMode, ListKind, Mark, Marks, Motion, Op, PageRect, PagesChanged, ParaStyle, RectPt, SaveError, TextAlign};
use crate::doc::{self, DocInfo, DocKind, OutlineEntry};
use crate::{Limits, OpenError, Rect, RenderError, RenderInfo};

/// The protocol's caps (`check.rs`), mirrored: more selection rects are cut, more dirty rects become `All`.
const MAX_SELECTION_RECTS: usize = 1_024;
const MAX_DIRTY_RECTS: usize = 256;
/// Table size bounds, as a person could still use.
const MAX_TABLE_ROWS: u32 = 500;
const MAX_TABLE_COLS: u32 = 63;
/// Indent steps one op takes.
const MAX_INDENT_STEPS: i8 = 12;

/// What one open Word document holds.
pub(super) struct WordSession {
    s: Session,
    layout: Arc<DocLayout>,
    info: DocInfo,
    limits: Limits,
    opts: RenderOptions,
    /// Opened from a `.doc`: viewable and editable in memory, never saved.
    binary: bool,
    digests: Vec<u64>,
}

pub(super) fn open(bytes: &[u8], limits: &Limits) -> Result<Box<dyn Any + Send>, OpenError> {
    let binary = doc::reads_as_doc(DocKind::Docx, bytes);
    let (model, _dropped) = doc::read_model(DocKind::Docx, bytes, limits)?;
    let mut s = Session::new(model);
    let layout = s.layout();
    let info = doc::info_of(&layout)?;
    if layout.pages.len() as u64 > u64::from(limits.max_pages) {
        return Err(OpenError::TooLarge(crate::limits::PAGES));
    }
    let digests = digests(&layout);
    Ok(Box::new(WordSession { s, layout, info, limits: *limits, opts: doc::viewer_render_options(), binary, digests }))
}

fn session(doc: &EditDoc) -> Result<&WordSession, EditError> {
    doc.engine.as_ref().and_then(|e| e.downcast_ref::<WordSession>()).ok_or(EditError::Engine)
}

// ---------------------------------------------------------------- view calls

pub(super) fn info(doc: &EditDoc) -> Result<DocInfo, EditError> {
    Ok(session(doc)?.info.clone())
}

pub(super) fn render_size(doc: &EditDoc, page: u32, scale: f32, region: Option<Rect>) -> Result<(u32, u32), RenderError> {
    let w = session(doc).map_err(|_| RenderError::Engine)?;
    doc::render_size_of(&w.layout, &w.limits, page, scale, region)
}

pub(super) fn render_page(doc: &EditDoc, page: u32, scale: f32, region: Option<Rect>, out: &mut [u8]) -> Result<RenderInfo, RenderError> {
    let w = session(doc).map_err(|_| RenderError::Engine)?;
    doc::render_page_of(&w.s.doc, &w.layout, &w.opts, &w.limits, page, scale, region, out, |n| doc::page_missing(&w.s.doc, &w.layout, &w.opts, n))
}

pub(super) fn outline(doc: &EditDoc) -> Vec<OutlineEntry> {
    session(doc).map(|w| doc::outline_of(&w.s.doc, &w.layout)).unwrap_or_default()
}

pub(super) fn words(doc: &EditDoc) -> u32 {
    session(doc).map(|w| u32::try_from(w.s.doc.word_count()).unwrap_or(u32::MAX)).unwrap_or(0)
}

// ---------------------------------------------------------------- edits

pub(super) fn apply(doc: &mut EditDoc, op: &Op) -> Result<Change, EditError> {
    let w = doc.engine.as_mut().and_then(|e| e.downcast_mut::<WordSession>()).ok_or(EditError::Engine)?;
    let rev_before = w.s.rev();
    let result = w.run_op(op);
    // A command that failed half way may still have changed the document.
    let changed = w.s.rev() != rev_before;
    if changed {
        doc.revision += 1;
    }
    let revision = doc.revision;
    let w = doc.engine.as_mut().and_then(|e| e.downcast_mut::<WordSession>()).ok_or(EditError::Engine)?;
    result?;
    Ok(w.change(revision))
}

pub(super) fn hit(doc: &mut EditDoc, page: u32, x_pt: f32, y_pt: f32, mode: HitMode) -> Result<Change, EditError> {
    let revision = doc.revision;
    let w = doc.engine.as_mut().and_then(|e| e.downcast_mut::<WordSession>()).ok_or(EditError::Engine)?;
    let p = w.layout.pages.get(page as usize).ok_or(EditError::NotApplicable)?;
    if !(x_pt.is_finite() && y_pt.is_finite()) || x_pt < 0.0 || y_pt < 0.0 || x_pt > p.w || y_pt > p.h {
        return Err(EditError::NotApplicable);
    }
    // Where the click lands, in the body; a click outside any text is nothing to place a caret on.
    w.layout.hit(page as usize, x_pt, y_pt, StoryRef::Body).ok_or(EditError::NotApplicable)?;
    let at = json!({"page": page, "x": x_pt, "y": y_pt, "extend": mode == HitMode::Extend});
    w.run("caret.set", &at)?;
    match mode {
        HitMode::Word => {
            w.run("select.word", &json!({}))?;
        }
        HitMode::Paragraph => {
            w.run("select.paragraph", &json!({}))?;
        }
        HitMode::Place | HitMode::Extend => {}
    }
    Ok(w.change(revision))
}

pub(super) fn save(doc: &EditDoc) -> Result<Vec<u8>, SaveError> {
    let w = session(doc).map_err(|_| SaveError::Engine)?;
    if w.binary {
        return Err(SaveError::Unsupported);
    }
    wordcraft_docx::write(&w.s.doc).map_err(|_| SaveError::Engine)
}

pub(super) fn copy(doc: &EditDoc) -> Clip {
    let Ok(w) = session(doc) else { return Clip::default() };
    let (a, b) = w.s.sel.ordered();
    if a == b {
        return Clip::default();
    }
    let text = w.s.selected_text();
    // Simple HTML: one escaped paragraph per line of the selection.
    let mut html = String::new();
    for line in text.split('\n') {
        html.push_str("<p>");
        for c in line.chars() {
            match c {
                '&' => html.push_str("&amp;"),
                '<' => html.push_str("&lt;"),
                '>' => html.push_str("&gt;"),
                '"' => html.push_str("&quot;"),
                c if c.is_control() && c != '\t' => {}
                c => html.push(c),
            }
        }
        html.push_str("</p>");
    }
    Clip { text, html: Some(html) }
}

impl WordSession {
    fn run(&mut self, id: &str, params: &Value) -> Result<Value, EditError> {
        self.s.run(id, params).map_err(|e| match e {
            CmdError::Unknown(_) => EditError::Unsupported,
            CmdError::Params(_) | CmdError::Disabled(_) => EditError::NotApplicable,
            CmdError::Failed(_) => EditError::Engine,
        })
    }

    fn refresh_layout(&mut self) {
        self.layout = self.s.layout();
    }

    fn run_op(&mut self, op: &Op) -> Result<(), EditError> {
        match op {
            Op::InsertText(t) => self.insert_text(t)?,
            // A paste is verbatim: AutoCorrect off for the call, one paragraph per line, one undo step.
            Op::Paste { text } => {
                let was = std::mem::replace(&mut self.s.prefs.autocorrect.enabled, false);
                let r = self.run("text.insert", &json!({"text": text, "raw": true, "paragraphs": true}));
                self.s.prefs.autocorrect.enabled = was;
                r?;
            }
            Op::InsertParagraph => drop(self.run("text.newParagraph", &json!({}))?),
            Op::InsertLineBreak => drop(self.run("text.lineBreak", &json!({}))?),
            Op::Delete { back, word } => {
                let id = match (back, word) {
                    (true, true) => "text.deleteWordBack",
                    (true, false) => "text.backspace",
                    (false, true) => "text.deleteWordForward",
                    (false, false) => "text.delete",
                };
                self.run(id, &json!({}))?;
            }
            Op::Move { to, extend } => {
                let id = match to {
                    Motion::Left => "caret.left",
                    Motion::Right => "caret.right",
                    Motion::Up => "caret.up",
                    Motion::Down => "caret.down",
                    Motion::WordLeft => "caret.wordLeft",
                    Motion::WordRight => "caret.wordRight",
                    Motion::LineStart => "caret.home",
                    Motion::LineEnd => "caret.end",
                    Motion::DocStart => "caret.docStart",
                    Motion::DocEnd => "caret.docEnd",
                    Motion::PageUp => "caret.pageUp",
                    Motion::PageDown => "caret.pageDown",
                };
                self.run(id, &json!({"extend": extend}))?;
            }
            Op::SelectAll => drop(self.run("select.all", &json!({}))?),
            Op::Mark { mark, on } => {
                let id = match mark {
                    Mark::Bold => "format.bold",
                    Mark::Italic => "format.italic",
                    Mark::Underline => "format.underline",
                    Mark::Strike => "format.strikethrough",
                };
                self.run(id, &json!({"value": on}))?;
            }
            Op::FontSize(v) => {
                if !v.is_finite() {
                    return Err(EditError::NotApplicable);
                }
                self.run("format.size", &json!({"size": v}))?;
            }
            Op::FontFamily(name) => {
                if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
                    return Err(EditError::NotApplicable);
                }
                self.run("format.font", &json!({"name": name}))?;
            }
            Op::TextColour(c) => drop(self.run("format.color", &json!({"color": hex(c.r, c.g, c.b)}))?),
            Op::Highlight(c) => {
                let name = match c {
                    Some(c) => nearest_highlight(c.r, c.g, c.b),
                    None => "none".to_string(),
                };
                self.run("format.highlight", &json!({"color": name}))?;
            }
            Op::ParaStyle(st) => match st {
                ParaStyle::Normal => drop(self.run("para.normal", &json!({}))?),
                ParaStyle::Title => drop(self.run("para.style", &json!({"style": "Title"}))?),
                ParaStyle::Heading1 => drop(self.run("para.heading1", &json!({}))?),
                ParaStyle::Heading2 => drop(self.run("para.heading2", &json!({}))?),
                ParaStyle::Heading3 => drop(self.run("para.heading3", &json!({}))?),
            },
            Op::List(kind) => {
                let now = self.marks().list;
                if now != *kind {
                    match kind {
                        ListKind::None => {
                            let id = if now == ListKind::Bullet { "para.bullets" } else { "para.numbering" };
                            self.run(id, &json!({"off": true}))?;
                        }
                        ListKind::Bullet => drop(self.run("para.bullets", &json!({}))?),
                        ListKind::Number => drop(self.run("para.numbering", &json!({}))?),
                    }
                }
            }
            Op::Align(a) => {
                let v = match a {
                    TextAlign::Left => "left",
                    TextAlign::Center => "center",
                    TextAlign::Right => "right",
                    TextAlign::Justify => "justify",
                };
                self.run("para.align", &json!({"value": v}))?;
            }
            Op::Indent(n) => {
                let steps = (*n).clamp(-MAX_INDENT_STEPS, MAX_INDENT_STEPS);
                let id = if steps >= 0 { "para.indent" } else { "para.outdent" };
                for _ in 0..steps.unsigned_abs() {
                    self.run(id, &json!({}))?;
                }
            }
            Op::Table { rows, cols } => {
                if !(1..=MAX_TABLE_ROWS).contains(rows) || !(1..=MAX_TABLE_COLS).contains(cols) {
                    return Err(EditError::NotApplicable);
                }
                self.run("insert.table", &json!({"rows": rows, "cols": cols}))?;
            }
            Op::Link { text, url } => {
                // Only links a person can follow safely: web and mail addresses.
                let lower = url.to_ascii_lowercase();
                let ok = ["http://", "https://", "mailto:"].iter().any(|p| lower.starts_with(p));
                if !ok || url.len() > 4096 || url.chars().any(char::is_control) {
                    return Err(EditError::NotApplicable);
                }
                self.run("insert.link", &json!({"url": url, "text": if text.is_empty() { url } else { text }}))?;
            }
            Op::PageBreak => drop(self.run("insert.pageBreak", &json!({}))?),
            Op::Image { bytes, png } => {
                let magic_ok = if *png { bytes.starts_with(&[0x89, b'P', b'N', b'G']) } else { bytes.starts_with(&[0xFF, 0xD8]) };
                if bytes.is_empty() || !magic_ok {
                    return Err(EditError::NotApplicable);
                }
                self.run("insert.picture", &json!({"data": base64(bytes)}))?;
            }
            Op::Undo => {
                if self.s.can_undo() {
                    self.run("edit.undo", &json!({}))?;
                }
            }
            Op::Redo => {
                if self.s.can_redo() {
                    self.run("edit.redo", &json!({}))?;
                }
            }
            Op::SetCell { .. }
            | Op::SelectCells { .. }
            | Op::NumberFormat(_)
            | Op::FillColour(_)
            | Op::AutoSum
            | Op::Sort { .. }
            | Op::Filter
            | Op::MakeTable
            | Op::RenameSheet { .. }
            | Op::SelectShape { .. }
            | Op::ClearShape
            | Op::NewSlide { .. }
            | Op::DuplicateSlide(_)
            | Op::DeleteSlide(_)
            | Op::MoveSlide { .. }
            | Op::SetLayout { .. } => return Err(EditError::NotApplicable),
        }
        Ok(())
    }

    /// Text with line ends: each line is typed, a new paragraph between them.
    fn insert_text(&mut self, text: &str) -> Result<(), EditError> {
        for (i, line) in text.replace("\r\n", "\n").replace('\r', "\n").split('\n').enumerate() {
            if i > 0 {
                self.run("text.newParagraph", &json!({}))?;
            }
            if !line.is_empty() {
                self.run("text.insert", &json!({"text": line}))?;
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ what changed

    fn change(&mut self, revision: u64) -> Change {
        self.refresh_layout();
        let layout = self.layout.clone();
        let focus = self.s.sel.focus.clone();
        let hint = self.s.page_hint;
        let caret = layout.caret_on(&focus, hint).and_then(|c| page_rect(&layout, c.page, c.x, c.top, c.width, c.height));
        if let Some(c) = &caret {
            self.s.page_hint = c.page as usize;
        }
        let mut selection = Vec::new();
        if !self.s.sel.is_collapsed() {
            let (a, b) = self.s.sel.ordered();
            for (page, r) in layout.selection_rects(&self.s.doc, &a, &b, self.s.page_hint) {
                if let Some(pr) = page_rect(&layout, page, r.x, r.y, r.w, r.h) {
                    selection.push(pr);
                }
                if selection.len() >= MAX_SELECTION_RECTS {
                    break;
                }
            }
        }
        let info = doc::info_of(&layout).unwrap_or_else(|_| self.info.clone());
        let pages = (info != self.info).then(|| PagesChanged { sizes_pt: info.sizes_pt.clone() });
        let now = digests(&layout);
        let dirty = if pages.is_some() || now.len() != self.digests.len() {
            Dirty::All
        } else {
            let rects: Vec<PageRect> = now
                .iter()
                .zip(&self.digests)
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .filter_map(|(i, _)| layout.pages.get(i).map(|p| PageRect { page: i as u32, rect: RectPt { x: 0.0, y: 0.0, w: p.w, h: p.h } }))
                .collect();
            if rects.len() > MAX_DIRTY_RECTS { Dirty::All } else { Dirty::Rects(rects) }
        };
        self.info = info;
        self.digests = now;
        Change {
            revision,
            caret,
            selection,
            can_undo: self.s.can_undo(),
            can_redo: self.s.can_redo(),
            dirty,
            pages,
            cells: Vec::new(),
            slides: None,
            marks: self.marks(),
        }
    }

    /// The toolbar's state at the caret or over the selection.
    fn marks(&mut self) -> Marks {
        let st = self.s.run("format.state", &json!({})).unwrap_or(Value::Null);
        let flag = |k: &str| st.get(k).and_then(Value::as_bool).unwrap_or(false);
        let norm = |k: &str| st.get(k).and_then(Value::as_str).unwrap_or("").to_ascii_lowercase().replace(' ', "");
        let (id, name) = (norm("style"), norm("styleName"));
        let is = |w: &str| id == w || name == w;
        let style = if is("title") {
            ParaStyle::Title
        } else if is("heading1") {
            ParaStyle::Heading1
        } else if is("heading2") {
            ParaStyle::Heading2
        } else if is("heading3") {
            ParaStyle::Heading3
        } else {
            ParaStyle::Normal
        };
        let (mut align, mut list) = (TextAlign::Left, ListKind::None);
        let focus = self.s.sel.focus.clone();
        if let Some(p) = self.s.doc.para_at(&focus) {
            use wordcraft_doc::props::Align;
            let resolved = self.s.doc.styles.resolve_para(&p.props);
            align = match resolved.align {
                Align::Center => TextAlign::Center,
                Align::Right => TextAlign::Right,
                Align::Justify | Align::Distribute => TextAlign::Justify,
                Align::Left => TextAlign::Left,
            };
            if let Some(n) = resolved.numbering.filter(|n| n.num != 0)
                && let Some(l) = self.s.doc.numbering.level(n.num, n.level)
            {
                list = if l.format == wordcraft_doc::section::NumFormat::Bullet { ListKind::Bullet } else { ListKind::Number };
            }
        }
        Marks { bold: flag("bold"), italic: flag("italic"), underline: flag("underline"), strike: flag("strike"), style, align, list }
    }
}

/// A rectangle on `page` clamped inside it; `None` when it is not on a page or not finite.
fn page_rect(layout: &DocLayout, page: usize, x: f32, y: f32, w: f32, h: f32) -> Option<PageRect> {
    let p = layout.pages.get(page)?;
    if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()) {
        return None;
    }
    let x0 = x.clamp(0.0, p.w);
    let y0 = y.clamp(0.0, p.h);
    let x1 = (x + w.max(0.0)).clamp(x0, p.w);
    let y1 = (y + h.max(0.0)).clamp(y0, p.h);
    Some(PageRect { page: page as u32, rect: RectPt { x: x0, y: y0, w: x1 - x0, h: y1 - y0 } })
}

/// One digest per page over what is drawn on it (body, header and footer items). A paragraph the layout
/// cache kept is the same allocation, so an unchanged paragraph in an unchanged place digests the same.
fn digests(layout: &DocLayout) -> Vec<u64> {
    layout
        .pages
        .iter()
        .map(|p| {
            let mut h = DefaultHasher::new();
            (p.w.to_bits(), p.h.to_bits(), p.first_block, p.number).hash(&mut h);
            for it in p.items.iter().chain(&p.header).chain(&p.footer) {
                match it {
                    Placed::Lines { story, path, para, l0, l1, x, y, .. } => {
                        (0u8, *story, path, Arc::as_ptr(para) as usize, l0, l1, x.to_bits(), y.to_bits()).hash(&mut h);
                    }
                    other => format!("{other:?}").hash(&mut h),
                }
            }
            h.finish()
        })
        .collect()
}

fn hex(r: u8, g: u8, b: u8) -> String {
    format!("{r:02X}{g:02X}{b:02X}")
}

/// The engine's highlight palette is fixed: the nearest of its colours.
fn nearest_highlight(r: u8, g: u8, b: u8) -> String {
    let d = |c: wordcraft_doc::props::Rgb| {
        let (dr, dg, db) = (i32::from(c.0) - i32::from(r), i32::from(c.1) - i32::from(g), i32::from(c.2) - i32::from(b));
        dr * dr + dg * dg + db * db
    };
    Highlight::ALL
        .iter()
        .copied()
        .filter_map(|h| h.rgb().map(|c| (d(c), h)))
        .min_by_key(|(dist, _)| *dist)
        .map(|(_, h)| h.ooxml().to_string())
        .unwrap_or_else(|| "yellow".into())
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}
