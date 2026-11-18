//! Workbooks, through `gridcraft-engine`.
//!
//! An open workbook is a [`SheetSession`] in `EditDoc::engine`: the engine's `Session` holding one document,
//! and a viewer-side `Workbook` rebuilt from the engine's current model when a view call asks after an
//! edit (the viewer's cell texts, widths and anchors stay the viewer's code). Every op maps to engine
//! commands by id with parameters built here; nothing a caller sends names a command or a path. The file
//! commands (`file.open`, `file.save`) are never used: the bytes go in through the viewer's checks and come
//! out through `io::save_bytes`. A `.xls` opens for viewing and editing in memory but never saves.

use std::any::Any;
use std::sync::{Arc, Mutex};

use gridcraft_core::{CellRef, RangeRef};
use gridcraft_engine::{DocState, EngineError, Session, Trust};
use gridcraft_model::HAlign;
use serde_json::{Value, json};

use super::{
    Change, Clip, Dirty, EditDoc, EditError, HitMode, ListKind, Mark, Marks, NumberFormat, Op, ParaStyle, Rgb, SaveError, TextAlign,
};
use crate::sheet::{CellBlock, SheetsInfo, Workbook};
use crate::{CellRange, Limits, OpenError, SheetKind};

/// Most changed ranges one edit reports; past it the changed cells are reported as one bounding range.
const MAX_CHANGED_RANGES: usize = 64;

/// What one open workbook holds.
pub(super) struct SheetSession {
    s: Session,
    limits: Limits,
    drawings_left_out: bool,
    /// Opened from a `.xls`: viewable and editable in memory, never saved.
    binary: bool,
    /// The viewer's view of the engine's workbook as of `.0`.
    view: Mutex<Option<(Arc<gridcraft_model::Workbook>, Arc<Workbook>)>>,
}

pub(super) fn open(bytes: &[u8], limits: &Limits) -> Result<Box<dyn Any + Send>, OpenError> {
    let binary = gridcraft_xlsx::sniff(bytes) == gridcraft_xlsx::Format::Xls;
    let kind = if binary { SheetKind::Xls } else { SheetKind::Xlsx };
    // Pre-scan, content check, reader, limits and the one calculation of uncached formulas: the viewer's.
    let viewer = Workbook::open(kind, bytes, limits)?;
    let (book, drawings_left_out) = viewer.into_book();
    let mut s = Session::new();
    s.add_document(DocState::from_saved(book, None, "Book.xlsx".to_string(), Trust::FromFile));
    let w = SheetSession { s, limits: *limits, drawings_left_out, binary, view: Mutex::new(None) };
    w.sync_view()?;
    Ok(Box::new(w))
}

fn session(doc: &EditDoc) -> Result<&SheetSession, EditError> {
    doc.engine.as_ref().and_then(|e| e.downcast_ref::<SheetSession>()).ok_or(EditError::Engine)
}

impl SheetSession {
    /// The viewer's workbook for the engine's current model, rebuilt only when the model moved.
    fn sync_view(&self) -> Result<Arc<Workbook>, EditError> {
        let d = self.s.doc().map_err(|_| EditError::Engine)?;
        let mut v = self.view.lock().map_err(|_| EditError::Engine)?;
        if let Some((book, view)) = v.as_ref()
            && Arc::ptr_eq(book, &d.wb)
        {
            return Ok(view.clone());
        }
        let view = Arc::new(Workbook::over((*d.wb).clone(), self.drawings_left_out, &self.limits).map_err(|_| EditError::Engine)?);
        *v = Some((d.wb.clone(), view.clone()));
        Ok(view)
    }

    fn run(&mut self, id: &str, params: Value) -> Result<(), EditError> {
        let r = self.s.execute(id, params);
        // Nothing here opens a dialog or a pane; whatever the command asked of an interface is dropped.
        self.s.take_ui_requests();
        match r {
            Ok(_) => Ok(()),
            Err(EngineError::Internal(..)) => Err(EditError::Engine),
            Err(_) => Err(EditError::NotApplicable),
        }
    }

    fn run_op(&mut self, op: &Op) -> Result<(), EditError> {
        match op {
            Op::SetCell { sheet, row, col, text } => {
                if *row >= gridcraft_core::MAX_ROWS || *col >= gridcraft_core::MAX_COLS || *sheet as usize >= self.sheet_count() {
                    return Err(EditError::NotApplicable);
                }
                let cell = CellRef::new(*row, *col).a1();
                self.run("cell.set", json!({"sheet": sheet, "cell": cell, "input": text}))
            }
            Op::SelectCells { sheet, range } => {
                let r = crate::sheet::grid_range(*range).ok_or(EditError::NotApplicable)?;
                if *sheet as usize >= self.sheet_count() {
                    return Err(EditError::NotApplicable);
                }
                self.run("selection.set", json!({"sheet": sheet, "range": r.a1()}))
            }
            // `on` is always passed: without it the command toggles.
            Op::Mark { mark, on } => {
                let id = match mark {
                    Mark::Bold => "home.bold",
                    Mark::Italic => "home.italic",
                    Mark::Underline => "home.underline",
                    Mark::Strike => "home.strikethrough",
                };
                self.run(id, json!({"on": on}))
            }
            Op::NumberFormat(f) => match f {
                NumberFormat::General => self.run("home.numberFormat", json!({"format": "General"})),
                NumberFormat::Currency => self.run("home.numberFormat", json!({"format": "Currency"})),
                NumberFormat::Percent => self.run("home.percent", json!({})),
                NumberFormat::MoreDecimals => self.run("home.increaseDecimal", json!({})),
                NumberFormat::FewerDecimals => self.run("home.decreaseDecimal", json!({})),
            },
            Op::FillColour(c) => {
                let color = match c {
                    Some(Rgb { r, g, b }) => format!("#{r:02X}{g:02X}{b:02X}"),
                    None => "none".to_string(),
                };
                self.run("home.fillColor", json!({"color": color}))
            }
            Op::Align(a) => self.run(
                match a {
                    TextAlign::Left => "home.alignLeft",
                    TextAlign::Center => "home.alignCenter",
                    TextAlign::Right => "home.alignRight",
                    TextAlign::Justify => "home.justify",
                },
                json!({}),
            ),
            Op::AutoSum => self.run("formulas.autoSum", json!({"function": "SUM"})),
            Op::Sort { ascending } => self.run(if *ascending { "data.sortAscending" } else { "data.sortDescending" }, json!({})),
            Op::Filter => self.run("data.filter", json!({})),
            Op::MakeTable => self.run("insert.table", json!({})),
            Op::RenameSheet { sheet, name } => {
                if *sheet as usize >= self.sheet_count() {
                    return Err(EditError::NotApplicable);
                }
                self.run("sheet.rename", json!({"sheet": sheet, "name": name}))
            }
            Op::Undo => {
                if self.s.doc().is_ok_and(|d| d.undo.is_empty()) {
                    return Ok(());
                }
                self.run("edit.undo", json!({}))
            }
            Op::Redo => {
                if self.s.doc().is_ok_and(|d| d.redo.is_empty()) {
                    return Ok(());
                }
                self.run("edit.redo", json!({}))
            }
            _ => Err(EditError::NotApplicable),
        }
    }

    fn sheet_count(&self) -> usize {
        self.s.doc().map(|d| d.wb.sheets.len()).unwrap_or(0)
    }

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
        if let Ok(d) = self.s.doc()
            && let Some(sh) = d.wb.active()
        {
            let st = d.wb.styles.get(sh.style_id(d.selection.active));
            m.bold = st.font.bold;
            m.italic = st.font.italic;
            m.underline = !matches!(st.font.underline, gridcraft_model::Underline::None);
            m.strike = st.font.strike;
            m.align = match st.align.h {
                HAlign::Center | HAlign::CenterAcross => TextAlign::Center,
                HAlign::Right => TextAlign::Right,
                HAlign::Justify | HAlign::Distributed => TextAlign::Justify,
                _ => TextAlign::Left,
            };
        }
        m
    }

    fn change(&self, revision: u64, cells: Vec<(u32, CellRange)>, moved: bool) -> Change {
        let (can_undo, can_redo) = self.s.doc().map(|d| (!d.undo.is_empty(), !d.redo.is_empty())).unwrap_or((false, false));
        Change {
            revision,
            caret: None,
            selection: Vec::new(),
            can_undo,
            can_redo,
            dirty: if moved && cells.is_empty() { Dirty::All } else { Dirty::Rects(Vec::new()) },
            pages: None,
            cells,
            slides: None,
            marks: self.marks(),
        }
    }
}

// ---------------------------------------------------------------- what changed

/// The cells whose content or look differs between two versions of the workbook, per sheet, as ranges.
fn changed_cells(old: &gridcraft_model::Workbook, new: &gridcraft_model::Workbook) -> Vec<(u32, CellRange)> {
    let mut out = Vec::new();
    for (si, nsh) in new.sheets.iter().enumerate() {
        let Some(osh) = old.sheets.get(si) else { continue };
        if Arc::ptr_eq(osh, nsh) {
            continue;
        }
        let mut at: Vec<CellRef> = Vec::new();
        for (c, cell) in nsh.cells.iter() {
            if osh.cells.get(c) != Some(cell) {
                at.push(c);
            }
        }
        for (c, _) in osh.cells.iter() {
            if nsh.cells.get(c).is_none() {
                at.push(c);
            }
        }
        for (c, v) in &nsh.spill {
            if osh.spill.get(c) != Some(v) {
                at.push(*c);
            }
        }
        for c in osh.spill.keys() {
            if !nsh.spill.contains_key(c) {
                at.push(*c);
            }
        }
        at.sort_by_key(|c| (c.row, c.col));
        at.dedup();
        if at.is_empty() {
            continue;
        }
        // Runs along a row first; too many and one bounding range does.
        let mut runs: Vec<CellRange> = Vec::new();
        for c in &at {
            match runs.last_mut() {
                Some(r) if r.row == c.row && r.col + r.cols == c.col => r.cols += 1,
                _ => runs.push(CellRange { row: c.row, col: c.col, rows: 1, cols: 1 }),
            }
        }
        if runs.len() > MAX_CHANGED_RANGES {
            let (r0, r1) = (at.iter().map(|c| c.row).min().unwrap_or(0), at.iter().map(|c| c.row).max().unwrap_or(0));
            let (c0, c1) = (at.iter().map(|c| c.col).min().unwrap_or(0), at.iter().map(|c| c.col).max().unwrap_or(0));
            runs = vec![CellRange { row: r0, col: c0, rows: r1 - r0 + 1, cols: c1 - c0 + 1 }];
        }
        out.extend(runs.into_iter().map(|r| (si as u32, r)));
    }
    out
}

// ---------------------------------------------------------------- view calls

pub(super) fn sheets(doc: &EditDoc) -> Result<SheetsInfo, EditError> {
    Ok(session(doc)?.sync_view()?.info())
}

pub(super) fn cells(doc: &EditDoc, sheet: u32, range: CellRange) -> Result<CellBlock, EditError> {
    Ok(session(doc)?.sync_view()?.cells(sheet, range))
}

pub(super) fn anchor_size(doc: &EditDoc, sheet: u32, anchor: u32, scale: f32) -> Result<(u32, u32), crate::RenderError> {
    session(doc).and_then(|w| w.sync_view()).map_err(|_| crate::RenderError::Engine)?.anchor_size(sheet, anchor, scale)
}

pub(super) fn render_anchor(doc: &EditDoc, sheet: u32, anchor: u32, scale: f32, out: &mut [u8]) -> Result<crate::RenderInfo, crate::RenderError> {
    session(doc).and_then(|w| w.sync_view()).map_err(|_| crate::RenderError::Engine)?.render_anchor(sheet, anchor, scale, out)
}

// ---------------------------------------------------------------- edits

pub(super) fn apply(doc: &mut EditDoc, op: &Op) -> Result<Change, EditError> {
    let w = doc.engine.as_mut().and_then(|e| e.downcast_mut::<SheetSession>()).ok_or(EditError::Engine)?;
    let before = w.s.doc().map_err(|_| EditError::Engine)?.wb.clone();
    let result = w.run_op(op);
    // A command that failed half way may still have changed the workbook.
    let after = w.s.doc().map_err(|_| EditError::Engine)?.wb.clone();
    let moved = !Arc::ptr_eq(&before, &after);
    let cells = if moved { changed_cells(&before, &after) } else { Vec::new() };
    let change = w.change(doc.revision + u64::from(moved), cells, moved);
    if moved {
        doc.revision += 1;
    }
    result?;
    Ok(change)
}

/// A grid has no points to click on here: its geometry and hit test are the app's.
pub(super) fn hit(_doc: &mut EditDoc, _page: u32, _x_pt: f32, _y_pt: f32, _mode: HitMode) -> Result<Change, EditError> {
    Err(EditError::NotApplicable)
}

pub(super) fn save(doc: &EditDoc) -> Result<Vec<u8>, SaveError> {
    let w = session(doc).map_err(|_| SaveError::Engine)?;
    if w.binary {
        return Err(SaveError::Unsupported);
    }
    let d = w.s.doc().map_err(|_| SaveError::Engine)?;
    // The `.xlsx` name picks the package kind: no VBA project is written.
    let bytes = gridcraft_engine::io::save_bytes(&d.wb, "Book.xlsx").map_err(|_| SaveError::Engine)?;
    Ok(bytes)
}

/// The selected cells as tab- and newline-separated text.
pub(super) fn copy(doc: &EditDoc) -> Clip {
    let Ok(w) = session(doc) else { return Clip::default() };
    let (Ok(view), Ok(d)) = (w.sync_view(), w.s.doc()) else { return Clip::default() };
    let Some(r) = d.selection.ranges.last().copied().or(Some(RangeRef::new(d.selection.active, d.selection.active))) else {
        return Clip::default();
    };
    let range = CellRange { row: r.start.row, col: r.start.col, rows: r.end.row - r.start.row + 1, cols: r.end.col - r.start.col + 1 };
    // Bounded: a whole-column selection copies the used part only.
    let used = view.info().sheets.get(d.wb.active_sheet()).map(|s| (s.rows, s.cols)).unwrap_or((0, 0));
    let rows = range.rows.min(used.0.saturating_sub(range.row));
    let cols = range.cols.min(used.1.saturating_sub(range.col));
    if rows == 0 || cols == 0 {
        return Clip::default();
    }
    let block = view.cells(d.wb.active_sheet() as u32, CellRange { rows, cols, ..range });
    let mut text = String::new();
    for row in range.row..range.row + rows {
        for col in range.col..range.col + cols {
            if col > range.col {
                text.push('\t');
            }
            if let Some(c) = block.cells.iter().find(|c| c.row == row && c.col == col) {
                text.push_str(&c.text);
            }
        }
        text.push('\n');
    }
    Clip { text, html: None }
}
