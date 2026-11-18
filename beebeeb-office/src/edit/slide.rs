//! Presentations, through `deckcraft-engine`, only with the non-default feature `edit-slides` (it brings
//! the MPL-2.0 `symphonia` crates; contract Q4). Without the feature every call answers `Unsupported`.
//!
//! An open deck is a [`SlideSession`] in `EditDoc::engine`: the engine's `Session` (one document: selection,
//! undo, commands) and the viewer's [`Deck`], refreshed from the engine's model after each change, so the
//! view calls are the viewer's own. Every op maps to engine commands by id with parameters built here;
//! nothing a caller sends names a command, a path or a URL: a picture goes in as base64 `data`, never a
//! `path`. The engine's recovery module, `mkfifo`, media playback and file commands are never run.
//! `SelectShape` and clicks are hit-tested here from the shapes' geometry (the engine's own is private).

#[cfg(feature = "edit-slides")]
pub(super) use real::*;
#[cfg(not(feature = "edit-slides"))]
pub(super) use stub::*;

#[cfg(not(feature = "edit-slides"))]
mod stub {
    use super::super::{Change, Clip, EditDoc, EditError, HitMode, Op, SaveError};
    use crate::doc::{DocInfo, OutlineEntry};
    use crate::{Limits, OpenError, Rect, RenderError, RenderInfo};
    use std::any::Any;

    pub(in super::super) fn open(_bytes: &[u8], _limits: &Limits) -> Result<Box<dyn Any + Send>, OpenError> {
        Err(OpenError::Unsupported)
    }
    pub(in super::super) fn apply(_doc: &mut EditDoc, _op: &Op) -> Result<Change, EditError> {
        Err(EditError::Unsupported)
    }
    pub(in super::super) fn hit(_doc: &mut EditDoc, _page: u32, _x: f32, _y: f32, _mode: HitMode) -> Result<Change, EditError> {
        Err(EditError::Unsupported)
    }
    pub(in super::super) fn save(_doc: &EditDoc) -> Result<Vec<u8>, SaveError> {
        Err(SaveError::Unsupported)
    }
    pub(in super::super) fn copy(_doc: &EditDoc) -> Clip {
        Clip::default()
    }
    pub(in super::super) fn layouts(_doc: &EditDoc) -> Vec<String> {
        Vec::new()
    }
    pub(in super::super) fn info(_doc: &EditDoc) -> Result<DocInfo, EditError> {
        Err(EditError::Unsupported)
    }
    pub(in super::super) fn render_size(_doc: &EditDoc, _page: u32, _scale: f32, _region: Option<Rect>) -> Result<(u32, u32), RenderError> {
        Err(RenderError::Engine)
    }
    pub(in super::super) fn render_page(_doc: &EditDoc, _page: u32, _scale: f32, _region: Option<Rect>, _out: &mut [u8]) -> Result<RenderInfo, RenderError> {
        Err(RenderError::Engine)
    }
    pub(in super::super) fn outline(_doc: &EditDoc) -> Vec<OutlineEntry> {
        Vec::new()
    }
}

#[cfg(feature = "edit-slides")]
mod real {
    use std::any::Any;
    use std::sync::Arc;

    use deckcraft_engine::cmd::text::{caret_line, text_frame, text_pos_at};
    use deckcraft_engine::cmd::{file::save_bytes, xfrm_of};
    use deckcraft_engine::geom::Point;
    use deckcraft_engine::model::edit as ed;
    use deckcraft_engine::model::resolve::{self, Ctx};
    use deckcraft_engine::model::text::{Align, Bullet, Strike};
    use deckcraft_engine::model::{PhType, Shape, ShapeId, Slide};
    use deckcraft_engine::{EngineError, Selection, Session, Target};
    use serde_json::{Value, json};

    use super::super::{
        Change, Clip, Dirty, EditDoc, EditError, HitMode, ListKind, Mark, Marks, Motion, Op, PageRect, PagesChanged, ParaStyle, RectPt, SaveError,
        TextAlign,
    };
    use crate::doc::{DocInfo, OutlineEntry};
    use crate::{Deck, Limits, OpenError, Rect, RenderError, RenderInfo};

    /// The protocol's caps (`check.rs`), mirrored: more selection rects are cut, more dirty rects become `All`.
    const MAX_SELECTION_RECTS: usize = 1_024;
    const MAX_DIRTY_RECTS: usize = 256;

    /// What one open presentation holds.
    pub(in super::super) struct SlideSession {
        s: Session,
        deck: Deck,
        /// The slides as of the last answer, by identity: a slide whose allocation changed is dirty.
        prev: Vec<Arc<Slide>>,
    }

    pub(in super::super) fn open(bytes: &[u8], limits: &Limits) -> Result<Box<dyn Any + Send>, OpenError> {
        let deck = Deck::open(bytes, limits)?;
        let mut s = Session::new();
        let st = deckcraft_engine::DocState::new(deck.presentation().clone(), None, "deck".into());
        s.add_document(st);
        let prev = s.doc().map_err(|_| OpenError::Damaged)?.doc.slides.clone();
        Ok(Box::new(SlideSession { s, deck, prev }))
    }

    fn session(doc: &EditDoc) -> Result<&SlideSession, EditError> {
        doc.engine.as_ref().and_then(|e| e.downcast_ref::<SlideSession>()).ok_or(EditError::Engine)
    }

    fn failed(e: EngineError) -> EditError {
        match e {
            EngineError::Disabled(..) | EngineError::BadParams { .. } | EngineError::NoDocument => EditError::NotApplicable,
            _ => EditError::Engine,
        }
    }

    // ---------------------------------------------------------------- view calls

    pub(in super::super) fn info(doc: &EditDoc) -> Result<DocInfo, EditError> {
        let d = session(doc)?.deck.info();
        Ok(DocInfo { pages: d.slides, sizes_pt: d.sizes_pt })
    }

    pub(in super::super) fn render_size(doc: &EditDoc, page: u32, scale: f32, region: Option<Rect>) -> Result<(u32, u32), RenderError> {
        if region.is_some() {
            return Err(RenderError::Engine);
        }
        session(doc).map_err(|_| RenderError::Engine)?.deck.render_size(page, scale)
    }

    pub(in super::super) fn render_page(doc: &EditDoc, page: u32, scale: f32, region: Option<Rect>, out: &mut [u8]) -> Result<RenderInfo, RenderError> {
        if region.is_some() {
            return Err(RenderError::Engine);
        }
        session(doc).map_err(|_| RenderError::Engine)?.deck.render_slide(page, scale, out)
    }

    pub(in super::super) fn outline(doc: &EditDoc) -> Vec<OutlineEntry> {
        let Ok(w) = session(doc) else { return Vec::new() };
        w.deck.info().titles.into_iter().enumerate().filter(|(_, t)| !t.is_empty()).map(|(i, text)| OutlineEntry { level: 1, text, page: i as u32 }).collect()
    }

    // ---------------------------------------------------------------- edits

    pub(in super::super) fn apply(doc: &mut EditDoc, op: &Op) -> Result<Change, EditError> {
        let w = doc.engine.as_mut().and_then(|e| e.downcast_mut::<SlideSession>()).ok_or(EditError::Engine)?;
        let before = w.doc_ptr();
        let result = w.run_op(op);
        // A command that failed half way may still have changed the document.
        let changed = !Arc::ptr_eq(&before, &w.doc_ptr());
        if changed {
            doc.revision += 1;
        }
        let revision = doc.revision;
        let w = doc.engine.as_mut().and_then(|e| e.downcast_mut::<SlideSession>()).ok_or(EditError::Engine)?;
        if changed {
            w.refresh();
        }
        result?;
        Ok(w.change(revision))
    }

    pub(in super::super) fn hit(doc: &mut EditDoc, page: u32, x_pt: f32, y_pt: f32, mode: HitMode) -> Result<Change, EditError> {
        let revision = doc.revision;
        let w = doc.engine.as_mut().and_then(|e| e.downcast_mut::<SlideSession>()).ok_or(EditError::Engine)?;
        w.click(page, x_pt, y_pt, mode)?;
        Ok(w.change(revision))
    }

    pub(in super::super) fn save(doc: &EditDoc) -> Result<Vec<u8>, SaveError> {
        let w = session(doc).map_err(|_| SaveError::Engine)?;
        let st = w.s.doc().map_err(|_| SaveError::Engine)?;
        save_bytes(&st.doc, "pptx").map_err(|_| SaveError::Engine)
    }

    pub(in super::super) fn copy(doc: &EditDoc) -> Clip {
        let Ok(w) = session(doc) else { return Clip::default() };
        let Ok(st) = w.s.doc() else { return Clip::default() };
        let Some(t) = &st.selection.text else { return Clip::default() };
        if !t.is_range() {
            return Clip::default();
        }
        let Some(body) = st.shape(t.shape).and_then(|s| s.text.as_ref()) else { return Clip::default() };
        let (a, b) = t.ordered();
        Clip { text: ed::text_range(body, a, b), html: None }
    }

    /// The names of the layouts of the master the current slide uses; `NewSlide{layout}` and
    /// `SetLayout{layout}` count into this list.
    pub(in super::super) fn layouts(doc: &EditDoc) -> Vec<String> {
        let Ok(w) = session(doc) else { return Vec::new() };
        w.layout_ids().into_iter().map(|(_, n)| n).collect()
    }

    impl SlideSession {
        fn doc_ptr(&self) -> Arc<deckcraft_engine::model::Presentation> {
            self.s.doc().map(|d| d.doc.clone()).unwrap_or_default()
        }

        fn count(&self) -> usize {
            self.s.doc().map(|d| d.doc.slides.len()).unwrap_or(0)
        }

        fn current(&self) -> usize {
            self.s.doc().map(|d| d.selection.slide).unwrap_or(0)
        }

        /// The layouts (id, name) of the master the current slide's layout belongs to.
        fn layout_ids(&self) -> Vec<(u32, String)> {
            let Ok(st) = self.s.doc() else { return Vec::new() };
            let m = st.doc.slides.get(st.selection.slide).and_then(|sl| st.doc.masters.iter().position(|m| m.layout(sl.layout).is_some())).unwrap_or(0);
            st.doc.masters.get(m).map(|m| m.layouts.iter().map(|l| (l.id.0, l.name.clone())).collect()).unwrap_or_default()
        }

        fn run(&mut self, id: &str, p: &Value) -> Result<Value, EditError> {
            self.s.execute(id, p).map_err(failed)
        }

        /// Bring the viewer's deck to the engine's model.
        fn refresh(&mut self) {
            if let Ok(st) = self.s.doc() {
                self.deck.replace_presentation((*st.doc).clone());
            }
        }

        fn slide_index(&self, n: u32) -> Result<usize, EditError> {
            let n = n as usize;
            if n < self.count() { Ok(n) } else { Err(EditError::NotApplicable) }
        }

        fn need_text(&self) -> Result<(), EditError> {
            let ok = self.s.doc().is_ok_and(|d| d.selection.text.as_ref().is_some_and(|t| !t.notes));
            if ok { Ok(()) } else { Err(EditError::NotApplicable) }
        }

        /// A text selection or a selected shape to format.
        fn need_selection(&self) -> Result<(), EditError> {
            let ok = self.s.doc().is_ok_and(|d| d.selection.text.is_some() || !d.selection.shapes.is_empty());
            if ok { Ok(()) } else { Err(EditError::NotApplicable) }
        }

        fn go(&mut self, slide: usize) -> Result<(), EditError> {
            if self.current() != slide {
                self.run("slide.go", &json!({"index": slide}))?;
            }
            Ok(())
        }

        fn run_op(&mut self, op: &Op) -> Result<(), EditError> {
            match op {
                Op::InsertText(t) | Op::Paste { text: t } => {
                    self.need_text()?;
                    self.run("text.insert", &json!({"text": t}))?;
                }
                Op::InsertParagraph => {
                    self.need_text()?;
                    self.run("text.insert", &json!({"text": "\n"}))?;
                }
                Op::InsertLineBreak => {
                    self.need_text()?;
                    self.run("text.insert", &json!({"text": "\u{b}"}))?;
                }
                Op::Delete { back, word } => {
                    self.need_text()?;
                    let dir = match (back, word) {
                        (true, false) => "backward",
                        (false, false) => "forward",
                        (true, true) => "wordBackward",
                        (false, true) => "wordForward",
                    };
                    self.run("text.delete", &json!({"dir": dir}))?;
                }
                Op::Move { to, extend } => {
                    self.need_text()?;
                    let to = match to {
                        Motion::Left => "left",
                        Motion::Right => "right",
                        Motion::Up | Motion::PageUp => "up",
                        Motion::Down | Motion::PageDown => "down",
                        Motion::WordLeft => "wordLeft",
                        Motion::WordRight => "wordRight",
                        Motion::LineStart => "lineStart",
                        Motion::LineEnd => "lineEnd",
                        Motion::DocStart => "start",
                        Motion::DocEnd => "end",
                    };
                    self.run("text.move", &json!({"to": to, "extend": extend}))?;
                }
                Op::SelectAll => {
                    self.need_text()?;
                    self.run("text.selectAll", &json!({}))?;
                }
                Op::Mark { mark, on } => {
                    let id = match mark {
                        Mark::Bold => "format.bold",
                        Mark::Italic => "format.italic",
                        Mark::Underline => "format.underline",
                        Mark::Strike => "format.strikethrough",
                    };
                    self.need_selection()?;
                    self.run(id, &json!({"on": on}))?;
                }
                Op::FontSize(pt) => {
                    if !(pt.is_finite() && *pt >= 1.0 && *pt <= 999.0) {
                        return Err(EditError::NotApplicable);
                    }
                    self.need_selection()?;
                    self.run("format.size", &json!({"size": pt}))?;
                }
                Op::FontFamily(f) => {
                    self.need_selection()?;
                    self.run("format.font", &json!({"family": f}))?;
                }
                Op::TextColour(c) => {
                    self.need_selection()?;
                    self.run("format.color", &json!({"color": format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)}))?;
                }
                Op::Align(a) => {
                    let id = match a {
                        TextAlign::Left => "format.alignLeft",
                        TextAlign::Center => "format.alignCenter",
                        TextAlign::Right => "format.alignRight",
                        TextAlign::Justify => "format.justify",
                    };
                    self.need_selection()?;
                    self.run(id, &json!({}))?;
                }
                Op::SelectShape { slide, x_pt, y_pt } => {
                    let slide = self.slide_index(*slide)?;
                    self.go(slide)?;
                    self.select_at(slide, *x_pt, *y_pt)?;
                }
                Op::ClearShape => {
                    self.s
                        .select(|_, sel| {
                            sel.text = None;
                            sel.shapes.clear();
                            sel.cells = None;
                        })
                        .map_err(failed)?;
                }
                Op::NewSlide { layout } => {
                    let mut p = json!({});
                    if let Some(i) = layout {
                        let list = self.layout_ids();
                        let (id, _) = list.get(*i as usize).ok_or(EditError::NotApplicable)?;
                        p = json!({"layout": id});
                    }
                    self.run("slide.new", &p)?;
                }
                Op::DuplicateSlide(n) => {
                    let n = self.slide_index(*n)?;
                    self.run("slide.duplicate", &json!({"index": n}))?;
                }
                Op::DeleteSlide(n) => {
                    let n = self.slide_index(*n)?;
                    // A deck keeps at least one slide: nothing to draw or click on otherwise.
                    if self.count() <= 1 {
                        return Err(EditError::NotApplicable);
                    }
                    self.run("slide.delete", &json!({"index": n}))?;
                }
                Op::MoveSlide { from, to } => {
                    let from = self.slide_index(*from)?;
                    let to = self.slide_index(*to)?;
                    self.run("slide.move", &json!({"from": from, "to": to}))?;
                }
                Op::SetLayout { slide, layout } => {
                    let slide = self.slide_index(*slide)?;
                    self.go(slide)?;
                    let list = self.layout_ids();
                    let (id, _) = list.get(*layout as usize).ok_or(EditError::NotApplicable)?;
                    self.run("slide.layout", &json!({"layout": id, "index": slide}))?;
                }
                Op::Image { bytes, png } => {
                    let name = if *png { "picture.png" } else { "picture.jpg" };
                    self.run("insert.picture", &json!({"data": base64(bytes), "name": name}))?;
                }
                Op::Undo => {
                    self.run("edit.undo", &json!({}))?;
                }
                Op::Redo => {
                    self.run("edit.redo", &json!({}))?;
                }
                // Document-flow and sheet ops have no meaning on a slide.
                Op::Highlight(_)
                | Op::ParaStyle(_)
                | Op::List(_)
                | Op::Indent(_)
                | Op::Table { .. }
                | Op::Link { .. }
                | Op::PageBreak
                | Op::SetCell { .. }
                | Op::SelectCells { .. }
                | Op::NumberFormat(_)
                | Op::FillColour(_)
                | Op::AutoSum
                | Op::Sort { .. }
                | Op::Filter
                | Op::MakeTable
                | Op::RenameSheet { .. } => return Err(EditError::Unsupported),
            }
            Ok(())
        }

        /// The topmost visible shape of `slide` under the point (its box, rotation and flips applied).
        fn shape_at(&self, slide: usize, x: f32, y: f32) -> Option<ShapeId> {
            let st = self.s.doc().ok()?;
            let sl = st.doc.slides.get(slide)?;
            let sel = Selection { slide, target: Target::Slides, ..Selection::default() };
            let p = Point::new(f64::from(x), f64::from(y));
            sl.shapes.iter().rev().filter(|s| !s.hidden).find_map(|s| {
                let xf = xfrm_of(&st.doc, &sel, s);
                if !xf.is_finite() || xf.w < 0.0 || xf.h < 0.0 {
                    return None;
                }
                let local = xf.affine().inverse() * p;
                // A hair of slack so a zero-height line can still be hit.
                let slack = 2.0;
                (local.x >= -slack && local.y >= -slack && local.x <= xf.w + slack && local.y <= xf.h + slack).then_some(s.id)
            })
        }

        /// Select the shape under the point and, when it holds (or can hold) text, start editing it with the
        /// caret at the point. No shape there clears the selection.
        fn select_at(&mut self, slide: usize, x: f32, y: f32) -> Result<(), EditError> {
            if !(x.is_finite() && y.is_finite()) {
                return Err(EditError::NotApplicable);
            }
            let Some(id) = self.shape_at(slide, x, y) else {
                self.s
                    .select(|_, sel| {
                        sel.text = None;
                        sel.shapes.clear();
                    })
                    .map_err(failed)?;
                return Ok(());
            };
            self.s
                .select(|_, sel| {
                    sel.text = None;
                    sel.cells = None;
                    sel.shapes = vec![id];
                })
                .map_err(failed)?;
            let text_capable = self.s.doc().ok().and_then(|d| d.shape(id)).is_some_and(can_hold_text);
            if text_capable && self.run("text.edit", &json!({"id": id.0})).is_ok() {
                // The caret goes where the click was, if it fell on the text.
                self.place_caret(Point::new(f64::from(x), f64::from(y)), false);
            }
            Ok(())
        }

        /// Move the caret (or extend the selection) to the text position under a slide point.
        fn place_caret(&mut self, p: Point, extend: bool) -> Option<(usize, usize)> {
            let st = self.s.doc().ok()?;
            let t = st.selection.text.clone()?;
            let at = text_pos_at(st, &t, p);
            let anchor = if extend { t.anchor } else { at };
            self.run("text.select", &json!({"anchor": [anchor.0, anchor.1], "caret": [at.0, at.1]})).ok()?;
            Some(at)
        }

        fn click(&mut self, page: u32, x: f32, y: f32, mode: HitMode) -> Result<(), EditError> {
            let slide = self.slide_index(page)?;
            if !(x.is_finite() && y.is_finite()) {
                return Err(EditError::NotApplicable);
            }
            self.go(slide)?;
            let p = Point::new(f64::from(x), f64::from(y));
            // A click on the shape being edited works the text; elsewhere it selects what is there.
            let editing = self.s.doc().ok().and_then(|d| d.selection.text.as_ref().map(|t| t.shape));
            let on_it = editing.is_some() && editing == self.shape_at(slide, x, y);
            if !on_it {
                if mode == HitMode::Extend && editing.is_some() {
                    // Dragging out of the box still extends the selection to the nearest text position.
                    self.place_caret(p, true);
                    return Ok(());
                }
                return self.select_at(slide, x, y);
            }
            let at = self.place_caret(p, mode == HitMode::Extend).ok_or(EditError::NotApplicable)?;
            match mode {
                HitMode::Word => {
                    self.run("text.selectWord", &json!({"at": [at.0, at.1]}))?;
                }
                HitMode::Paragraph => {
                    self.run("text.selectParagraph", &json!({"at": [at.0, at.1]}))?;
                }
                HitMode::Place | HitMode::Extend => {}
            }
            Ok(())
        }

        // ---------------------------------------------------------------- the answer

        fn change(&mut self, revision: u64) -> Change {
            let count = self.count();
            let (dirty, pages) = self.dirty_since_last(count);
            let (caret, selection) = self.carets();
            let can_undo = self.s.doc().is_ok_and(|d| !d.history.undo.is_empty());
            let can_redo = self.s.doc().is_ok_and(|d| !d.history.redo.is_empty());
            let slides = pages.is_some().then_some(count as u32);
            Change { revision, caret, selection, can_undo, can_redo, dirty, pages, cells: Vec::new(), slides, marks: self.marks() }
        }

        /// Slides whose allocation changed since the last answer are dirty (whole slide); a different
        /// count is `All` with the new sizes.
        fn dirty_since_last(&mut self, count: usize) -> (Dirty, Option<PagesChanged>) {
            let Ok(st) = self.s.doc() else { return (Dirty::Rects(Vec::new()), None) };
            let now = st.doc.slides.clone();
            let size = (st.doc.slide_size.width as f32, st.doc.slide_size.height as f32);
            let prev = std::mem::replace(&mut self.prev, now.clone());
            if prev.len() != count {
                return (Dirty::All, Some(PagesChanged { sizes_pt: vec![size; count] }));
            }
            let rects: Vec<PageRect> = now
                .iter()
                .zip(&prev)
                .enumerate()
                .filter(|(_, (a, b))| !Arc::ptr_eq(a, b))
                .map(|(i, _)| PageRect { page: i as u32, rect: RectPt { x: 0.0, y: 0.0, w: size.0, h: size.1 } })
                .collect();
            if rects.len() > MAX_DIRTY_RECTS { (Dirty::All, None) } else { (Dirty::Rects(rects), None) }
        }

        /// The caret and the selection, in points on the current slide, clamped inside it.
        fn carets(&self) -> (Option<PageRect>, Vec<PageRect>) {
            let Ok(st) = self.s.doc() else { return (None, Vec::new()) };
            let page = st.selection.slide as u32;
            let (pw, ph) = (st.doc.slide_size.width, st.doc.slide_size.height);
            let clamp = |x0: f64, y0: f64, x1: f64, y1: f64| -> Option<PageRect> {
                if ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) {
                    return None;
                }
                let (a, b) = (x0.min(x1).clamp(0.0, pw), x0.max(x1).clamp(0.0, pw));
                let (c, d) = (y0.min(y1).clamp(0.0, ph), y0.max(y1).clamp(0.0, ph));
                Some(PageRect { page, rect: RectPt { x: a as f32, y: c as f32, w: (b - a) as f32, h: (d - c) as f32 } })
            };
            if let Some(t) = st.selection.text.as_ref().filter(|t| !t.notes) {
                let caret = caret_line(st, t).and_then(|(a, b)| clamp(a.x, a.y, b.x, b.y));
                let mut sel = Vec::new();
                if t.is_range()
                    && let Some((layout, m)) = text_frame(st, t)
                {
                    let (a, b) = t.ordered();
                    // `Pos` is the text crate's own type; take one from the layout and set its fields.
                    let mut pa = layout.hit(Point::ZERO);
                    let mut pb = pa;
                    (pa.para, pa.ch) = a;
                    (pb.para, pb.ch) = b;
                    for r in layout.selection_rects(pa, pb).into_iter().take(MAX_SELECTION_RECTS) {
                        let r = m.transform_rect_bbox(r);
                        sel.extend(clamp(r.x0, r.y0, r.x1, r.y1));
                    }
                }
                return (caret, sel);
            }
            let sel_sel = Selection { slide: st.selection.slide, target: Target::Slides, ..Selection::default() };
            let boxes = st
                .selection
                .shapes
                .iter()
                .filter_map(|id| st.shape(*id))
                .filter_map(|s| {
                    let b = xfrm_of(&st.doc, &sel_sel, s).bounds();
                    clamp(b.x0, b.y0, b.x1, b.y1)
                })
                .take(MAX_SELECTION_RECTS)
                .collect();
            (None, boxes)
        }

        /// The marks at the caret (or the start of the selection), as the text resolves them.
        fn marks(&self) -> Marks {
            let mut m = Marks {
                bold: false,
                italic: false,
                underline: false,
                strike: false,
                style: ParaStyle::Normal,
                align: TextAlign::Left,
                list: ListKind::None,
            };
            let Ok(st) = self.s.doc() else { return m };
            // The text being edited, else the first selected shape's first paragraph.
            let (shape, pos) = match &st.selection.text {
                Some(t) if !t.notes => (st.shape(t.shape), t.ordered().0),
                _ => (st.selection.shapes.first().and_then(|id| st.shape(*id)), (0, 0)),
            };
            let Some(sh) = shape else { return m };
            if sh.ph_type().is_some_and(|k| k.is_title()) {
                m.style = ParaStyle::Title;
            }
            let (Some(body), Some(slide)) = (sh.text.as_ref(), st.doc.slides.get(st.selection.slide)) else { return m };
            let (Some(ctx), Some(para)) = (Ctx::for_slide(&st.doc, slide), body.paragraphs.get(pos.0)) else { return m };
            let run = resolve::run(&ctx, sh, para, &ed::props_at(body, pos));
            m.bold = run.bold == Some(true);
            m.italic = run.italic == Some(true);
            m.underline = run.underline.as_deref().is_some_and(|u| u != "none");
            m.strike = matches!(run.strike, Some(Strike::Single | Strike::Double));
            let pp = resolve::para(&ctx, sh, para);
            m.align = match pp.align {
                Some(Align::Center) => TextAlign::Center,
                Some(Align::Right) => TextAlign::Right,
                Some(Align::Justify | Align::JustLow | Align::Distributed | Align::ThaiDistributed) => TextAlign::Justify,
                _ => TextAlign::Left,
            };
            m.list = match pp.bullet {
                Some(Bullet::None) | None => ListKind::None,
                Some(Bullet::AutoNum { .. }) => ListKind::Number,
                Some(_) => ListKind::Bullet,
            };
            m
        }
    }

    /// A shape text can be typed into: it has a body, or is a text placeholder or a text box.
    fn can_hold_text(s: &Shape) -> bool {
        s.text.is_some() || s.ph_type().is_some_and(|k| !matches!(k, PhType::Picture | PhType::Chart | PhType::Media))
    }

    /// Standard base64 with padding, for a picture's bytes (the engine reads `data` as base64).
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
}
