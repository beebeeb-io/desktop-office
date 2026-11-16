//! Workbooks (`.xlsx`, `.xls`): read from bytes once, the formulas the file kept no value for
//! calculated once, cells shown as their number formats show them, charts and pictures rendered
//! on request.
//!
//! The path is the GridCraft readers, then `gridcraft-calc` for the formulas without a cached
//! value only (a cached value is Excel's own result and is shown as it is), then
//! `gridcraft-numfmt` for the text of a cell and `gridcraft-chart` for a chart. Nothing is fetched
//! and nothing is written: a value from another workbook is never read, a function that would
//! reach outside the file gives the engine's error value, a linked picture stays a link.
//!
//! The model keeps sizes in pixels at 96 dpi (as GridCraft's reader converts them); this API
//! gives points, `px * 0.75`, converted once at open.

mod draw;

use std::collections::BTreeSet;
use std::io::Read as _;

use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_formula::{Expr, SheetSel};
use gridcraft_model::{Cell, HAlign, Sheet};
use gridcraft_numfmt::NumberFormat;

use crate::raster::check_buffer;
use crate::{Limits, OpenError, RenderError, RenderInfo, probe};

/// Which spreadsheet reader a file is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheetKind {
    /// `.xlsx` (and `.xlsm`).
    Xlsx,
    /// `.xls` (Excel 97-2003).
    Xls,
}

/// One sheet of a workbook.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetInfo {
    pub name: String,
    /// Rows and columns up to the last non-empty cell.
    pub rows: u32,
    pub cols: u32,
}

/// The sheets of a workbook, in tab order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SheetsInfo {
    pub sheets: Vec<SheetInfo>,
}

/// A block of cells: `rows` x `cols` from (`row`, `col`), 0-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellRange {
    pub row: u32,
    pub col: u32,
    pub rows: u32,
    pub cols: u32,
}

/// How a cell's text sits in its box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CellAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// One non-empty cell, its value shown as text the way its number format shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct CellText {
    pub row: u32,
    pub col: u32,
    /// What the cell shows: its value with its number format applied, in the file's date system.
    pub text: String,
    pub align: CellAlign,
    pub bold: bool,
    /// The number behind a numeric or date cell (finite); `None` for text, booleans and errors.
    pub value: Option<f64>,
    /// The formula as the file writes it, with its `=` (`=SUM(B2:B9)`); `None` for a constant.
    pub formula: Option<String>,
}

/// What an anchored object of a sheet is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorKind {
    Chart,
    Picture,
}

/// A chart or picture of a sheet, and where it sits on the grid.
#[derive(Clone, Debug, PartialEq)]
pub struct Anchor {
    /// 0.. per sheet, stable for the open workbook.
    pub id: u32,
    pub kind: AnchorKind,
    /// (row, column) of the cell its top-left corner is in.
    pub from: (u32, u32),
    /// (row, column) of the cell its bottom-right corner is in (inclusive).
    pub to: (u32, u32),
    /// Offset of its top-left corner inside `from`, points.
    pub x_pt: f32,
    pub y_pt: f32,
    /// Its size, points (finite, above 0).
    pub w_pt: f32,
    pub h_pt: f32,
}

/// What a range shows: its non-empty cells, the widths and heights of its columns and rows
/// (points), the merged areas that touch it, and the frozen rows and columns of its sheet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CellBlock {
    pub cells: Vec<CellText>,
    pub col_widths: Vec<f32>,
    pub row_heights: Vec<f32>,
    pub merges: Vec<CellRange>,
    /// (rows, columns) frozen at the top left.
    pub frozen: (u32, u32),
    /// Every chart and picture whose `from..=to` rectangle touches the range.
    pub anchors: Vec<Anchor>,
    /// Something in the range is not shown as the file means it: a value from another workbook
    /// (never read), a function that would fetch from outside the file (never run), a cell or
    /// drawing the reader did not read.
    pub missing_content: bool,
}

/// Points per pixel at 96 dpi: the model's sizes are pixels.
const PT_PER_PX: f32 = 0.75;

/// The content types of the main part of a workbook package this viewer opens (`.xlsx`,
/// `.xlsm`). A template, a binary workbook (`.xlsb`) or any other package is not one.
const WORKBOOK_TYPES: [&str; 2] =
    ["application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml", "application/vnd.ms-excel.sheet.macroenabled.main+xml"];

/// The package part that names the content types, as the readers find it (any case).
const CONTENT_TYPES_PART: &str = "[Content_Types].xml";

/// Most bytes of `[Content_Types].xml` read for the content check. Excel writes a few kB; the
/// pre-scan bounds only the package's total.
const MAX_CONTENT_TYPES_BYTES: u64 = 1024 * 1024;

/// How the `.xls` reader names a workbook protected with a password (a FilePass record) in its
/// `Format` message: it has no variant of its own for it.
const XLS_ENCRYPTED: &str = "password-protected (encrypted)";

/// How the `.xls` reader names an Excel 5.0/95 workbook (BIFF5), an older format this build does
/// not show (as an older Word file is not, `error.rs`).
const XLS_BIFF5: &str = "(BIFF5) are not supported";

/// The `.xlsx` reader's warnings for a drawing it left out (a linked picture, a missing picture
/// part, a chart or object it could not read). The reader does not say on which sheet, so every
/// sheet reports it, as a document reports a dropped picture on every page (`doc.rs`).
const DRAWINGS_LEFT_OUT: [&str; 8] = [
    "a linked (not embedded) picture was skipped",
    "missing picture ",
    "a chart without its chart part was skipped",
    "has no supported plot and was skipped",
    "is missing from the package and was skipped",
    "chart without its chart part was skipped",
    "an embedded object (SmartArt, OLE",
    "grouped shapes are not supported",
];

/// Functions that would reach outside the file (a web service, a real-time data server, an OLAP
/// cube, a picture from a URL, stock data, a DLL). The engine gives its error value for each and
/// fetches nothing; a cell calculated here with one of them is content not shown.
const FETCHING: [&str; 14] = [
    "WEBSERVICE",
    "FILTERXML",
    "RTD",
    "STOCKHISTORY",
    "IMAGE",
    "CUBEVALUE",
    "CUBEMEMBER",
    "CUBESET",
    "CUBERANKEDMEMBER",
    "CUBEKPIMEMBER",
    "CUBEMEMBERPROPERTY",
    "CUBESETCOUNT",
    "CALL",
    "REGISTER.ID",
];

/// A chart or picture of a sheet: its place on the grid and where its content is in the model.
struct Placed {
    anchor: Anchor,
    source: Source,
    /// A picture whose header is over the picture limits: never decoded.
    over_limits: bool,
}

#[derive(Clone, Copy)]
enum Source {
    /// Index into the sheet's charts.
    Chart(usize),
    /// Index into the sheet's pictures.
    Picture(usize),
}

/// What a sheet holds beside the model, worked out once at open.
#[derive(Default)]
struct SheetExtra {
    anchors: Vec<Placed>,
    /// Cells whose value would come from another workbook: shown empty, never read.
    external: BTreeSet<(u32, u32)>,
    /// Cells not shown as the file means them: the external ones, and those calculated here with
    /// a function that would fetch from outside the file.
    flagged: BTreeSet<(u32, u32)>,
    /// Content on the sheet that is not drawn: shapes, text boxes, equations, slicers, controls,
    /// pictures in cells, sparklines.
    undrawn: Vec<RangeRef>,
}

/// An open workbook.
pub struct Workbook {
    book: gridcraft_model::Workbook,
    info: SheetsInfo,
    extra: Vec<SheetExtra>,
    limits: Limits,
    /// The reader left a drawing out and does not say on which sheet: every block reports it.
    drawings_left_out: bool,
}

impl Workbook {
    /// Open a workbook from its bytes.
    ///
    /// In this order: [`crate::prescan`] refuses a container over `limits` before any reader sees
    /// it; the content check lets [`SheetKind::Xlsx`] through only for an OOXML workbook package
    /// and [`SheetKind::Xls`] only for a compound file with a workbook stream (bytes that are no
    /// readable container are [`OpenError::Damaged`], an encrypted package is
    /// [`OpenError::Password`], a readable container of another kind [`OpenError::Unsupported`]); the reader
    /// parses the bytes; more sheets than `limits.max_sheets` or more non-empty cells than
    /// `limits.max_cells` is refused; the formulas without a cached value are calculated, once.
    pub fn open(kind: SheetKind, bytes: &[u8], limits: &Limits) -> Result<Workbook, OpenError> {
        crate::prescan(bytes, limits)?;
        check_content(kind, bytes)?;
        probe::handed(bytes);
        let (mut book, report) = match kind {
            SheetKind::Xlsx => gridcraft_xlsx::read_xlsx(bytes).map_err(from_xlsx)?,
            SheetKind::Xls => gridcraft_xlsx::read_xls(bytes).map_err(from_xls)?,
        };
        if book.sheets.len() as u64 > u64::from(limits.max_sheets) {
            return Err(OpenError::TooLarge(crate::limits::SHEETS));
        }

        // One pass over every cell: the count against the limit, each sheet's extent, the formulas
        // without a cached value, the cells whose value would come from outside the file.
        let mut content: u64 = 0;
        let mut extents = Vec::with_capacity(book.sheets.len());
        let mut uncached: Vec<(usize, CellRef)> = Vec::new();
        let mut extra: Vec<SheetExtra> = Vec::with_capacity(book.sheets.len());
        for (si, sheet) in book.sheets.iter().enumerate() {
            let mut ext = SheetExtra::default();
            let (mut rows, mut cols) = (0u32, 0u32);
            for (at, cell) in sheet.cells.iter() {
                if !cell.has_content() {
                    continue;
                }
                content += 1;
                if content > limits.max_cells {
                    return Err(OpenError::TooLarge(crate::limits::CELLS));
                }
                rows = rows.max(at.row.saturating_add(1));
                cols = cols.max(at.col.saturating_add(1));
                let Some(f) = &cell.formula else { continue };
                let calculated_here = cell.value.is_empty() && cell.py.is_none();
                if calculated_here {
                    uncached.push((si, at));
                }
                let reach = reach_of(&book, f);
                if reach.external {
                    ext.external.insert((at.row, at.col));
                    ext.flagged.insert((at.row, at.col));
                } else if reach.fetch && calculated_here {
                    ext.flagged.insert((at.row, at.col));
                }
            }
            extents.push((rows, cols));
            extra.push(ext);
        }

        recalculate(&mut book, &uncached);

        let drawings_left_out = matches!(kind, SheetKind::Xlsx) && report.warnings.iter().any(|w| DRAWINGS_LEFT_OUT.iter().any(|m| w.contains(m)));
        let mut sheets = Vec::with_capacity(book.sheets.len());
        for ((sheet, ext), (mut rows, mut cols)) in book.sheets.iter().zip(extra.iter_mut()).zip(extents) {
            // Values an uncached dynamic-array formula spilled reach past its own cell.
            for r in sheet.spill_ranges.values() {
                rows = rows.max(r.end.row.saturating_add(1));
                cols = cols.max(r.end.col.saturating_add(1));
            }
            sheets.push(SheetInfo { name: sheet.name.clone(), rows, cols });
            place_objects(sheet, ext, limits);
        }
        Ok(Workbook { info: SheetsInfo { sheets }, extra, limits: *limits, drawings_left_out, book })
    }

    /// The sheets, in tab order.
    pub fn info(&self) -> SheetsInfo {
        self.info.clone()
    }

    /// The cells of `range` on sheet `sheet` (0-based), with the sizes of its columns and rows,
    /// the merges, charts and pictures that touch it, and the sheet's frozen panes. A sheet that
    /// does not exist, an empty range or one that starts outside the grid gives an empty block.
    pub fn cells(&self, sheet: u32, range: CellRange) -> CellBlock {
        let (Some(sh), Some(ext)) = (self.book.sheets.get(sheet as usize), self.extra.get(sheet as usize)) else {
            return CellBlock::default();
        };
        let Some(r) = grid_range(range) else { return CellBlock::default() };
        let mut cells: Vec<CellText> = sh
            .cells
            .iter_range(r)
            .filter(|(_, cell)| cell.has_content())
            .map(|(at, cell)| self.cell_text(sh, ext, at, cell, &cell.value))
            .collect();
        // Values spilled by a formula calculated here live beside the cells.
        let mut spilled = false;
        for sr in sh.spill_ranges.values() {
            let Some(hit) = sr.intersection(&r) else { continue };
            for at in hit.iter() {
                if sh.cells.get(at).is_some_and(Cell::has_content) {
                    continue;
                }
                let Some(v) = sh.spill.get(&at) else { continue };
                if v.is_empty() {
                    continue;
                }
                let style = Cell { style: sh.style_id(at), ..Cell::default() };
                let mut text = self.cell_text(sh, ext, at, &style, v);
                text.formula = None;
                cells.push(text);
                spilled = true;
            }
        }
        if spilled {
            cells.sort_by_key(|c| (c.row, c.col));
        }
        let col_widths = (r.start.col..=r.end.col).map(|c| sh.col_width(c) * PT_PER_PX).collect();
        let row_heights = (r.start.row..=r.end.row).map(|row| sh.row_height(row) * PT_PER_PX).collect();
        let merges = sh
            .merges
            .iter()
            .filter(|m| m.intersects(&r))
            .map(|m| CellRange { row: m.start.row, col: m.start.col, rows: m.end.row - m.start.row + 1, cols: m.end.col - m.start.col + 1 })
            .collect();
        let anchors: Vec<&Placed> = ext.anchors.iter().filter(|p| touches(p.anchor.from, p.anchor.to, &r)).collect();
        let flagged = ext.flagged.range((r.start.row, 0)..=(r.end.row, u32::MAX)).any(|&(_, c)| c >= r.start.col && c <= r.end.col);
        let missing_content = self.drawings_left_out
            || flagged
            || ext.undrawn.iter().any(|u| u.intersects(&r))
            || anchors.iter().any(|p| p.over_limits);
        CellBlock {
            cells,
            col_widths,
            row_heights,
            merges,
            frozen: sh.freeze.unwrap_or((0, 0)),
            anchors: anchors.into_iter().map(|p| p.anchor.clone()).collect(),
            missing_content,
        }
    }

    /// The size in pixels of what [`Workbook::render_anchor`] writes for anchor `anchor` of sheet
    /// `sheet` at `scale`: the caller sizes its buffer from this.
    ///
    /// The page rule: `ceil(w_pt * scale)` x `ceil(h_pt * scale)`, the product in `f32`, at least 1,
    /// at most `max_render_side_px` on a side ([`RenderError::TooLarge`]).
    pub fn anchor_size(&self, sheet: u32, anchor: u32, scale: f32) -> Result<(u32, u32), RenderError> {
        let p = self.placed(sheet, anchor).ok_or(RenderError::NoSuchPage)?;
        page_pixels(p.anchor.w_pt, p.anchor.h_pt, scale, self.limits.max_render_side_px)
    }

    /// Render the chart or picture `anchor_id` of sheet `sheet` at `scale` pixels per point into
    /// `out` as straight RGBA8, row by row, top-left first.
    ///
    /// `out` must be exactly `w * h * 4` bytes for the size [`Workbook::anchor_size`] gives, else
    /// [`RenderError::BufferSize`] and nothing is written. A chart is drawn on white; a picture
    /// keeps its own transparency. A picture over the picture limits is never decoded, and one
    /// that does not decode is drawn as an empty frame: both report `missing_content`.
    pub fn render_anchor(&self, sheet: u32, anchor_id: u32, scale: f32, out: &mut [u8]) -> Result<RenderInfo, RenderError> {
        let (w, h) = self.anchor_size(sheet, anchor_id, scale)?;
        check_buffer(w, h, out)?;
        let p = self.placed(sheet, anchor_id).ok_or(RenderError::NoSuchPage)?;
        let sh = self.book.sheets.get(sheet as usize).ok_or(RenderError::NoSuchPage)?;
        let missing_content = match p.source {
            Source::Chart(i) => {
                let chart = sh.charts.get(i).ok_or(RenderError::NoSuchPage)?;
                draw::chart(&self.book, sheet as usize, chart, (p.anchor.w_pt, p.anchor.h_pt), scale, (w, h), out)?
            }
            Source::Picture(i) => {
                let image = sh.images.get(i).ok_or(RenderError::NoSuchPage)?;
                draw::picture(&image.data, p.over_limits, &self.limits, (w, h), out)
            }
        };
        Ok(RenderInfo { w, h, missing_content })
    }

    fn placed(&self, sheet: u32, anchor: u32) -> Option<&Placed> {
        self.extra.get(sheet as usize)?.anchors.get(anchor as usize)
    }

    /// One cell as the grid shows it. `value` is the cell's own, or the value spilled into it.
    fn cell_text(&self, sh: &Sheet, ext: &SheetExtra, at: CellRef, cell: &Cell, value: &Value) -> CellText {
        let style = self.book.styles.get(cell.style);
        let formula = cell.formula.as_ref().map(|f| format!("={}", f.text));
        let bold = style.font.bold;
        if ext.external.contains(&(at.row, at.col)) {
            // Never read from the other workbook, and the copy the file kept is not shown either.
            return CellText { row: at.row, col: at.col, text: String::new(), align: CellAlign::Left, bold, value: None, formula };
        }
        let fmt = NumberFormat::parse_cached(style.num_fmt.as_str());
        let shown = gridcraft_numfmt::format_value(value, &fmt, self.book.date_system);
        let number = match value {
            Value::Number(n) if n.is_finite() => Some(*n),
            _ => None,
        };
        let text = match (&formula, value) {
            // Show Formulas (Ctrl+`) saved on the sheet: the formula, not its value.
            (Some(f), _) if sh.show_formulas => f.clone(),
            // "Show a zero in cells that have zero value" turned off.
            (_, Value::Number(n)) if !sh.show_zeros && *n == 0.0 => String::new(),
            _ => shown.text,
        };
        let align = match style.align.h {
            HAlign::General => match value {
                Value::Bool(_) | Value::Error(_) => CellAlign::Center,
                _ if shown.numeric => CellAlign::Right,
                _ => CellAlign::Left,
            },
            HAlign::Left | HAlign::Fill | HAlign::Justify | HAlign::Distributed => CellAlign::Left,
            HAlign::Center | HAlign::CenterAcross => CellAlign::Center,
            HAlign::Right => CellAlign::Right,
        };
        CellText { row: at.row, col: at.col, text, align, bold, value: number, formula }
    }
}

/// The page rule ([`Workbook::anchor_size`]), the same as `Document::render_size`'s.
fn page_pixels(w_pt: f32, h_pt: f32, scale: f32, max_side: u32) -> Result<(u32, u32), RenderError> {
    if !(scale.is_finite() && scale > 0.0) {
        return Err(RenderError::BadScale);
    }
    let (w, h) = ((w_pt * scale).ceil().max(1.0), (h_pt * scale).ceil().max(1.0));
    let max = max_side as f32;
    if !(w.is_finite() && h.is_finite()) || w > max || h > max {
        return Err(RenderError::TooLarge);
    }
    Ok((w as u32, h as u32))
}

/// The cells of `range` as a model range, cut to the grid; `None` when it is empty or starts
/// outside the grid.
fn grid_range(range: CellRange) -> Option<RangeRef> {
    if range.rows == 0 || range.cols == 0 || range.row >= gridcraft_core::MAX_ROWS || range.col >= gridcraft_core::MAX_COLS {
        return None;
    }
    let last_row = range.row.saturating_add(range.rows - 1).min(gridcraft_core::MAX_ROWS - 1);
    let last_col = range.col.saturating_add(range.cols - 1).min(gridcraft_core::MAX_COLS - 1);
    Some(RangeRef::new(CellRef::new(range.row, range.col), CellRef::new(last_row, last_col)))
}

/// Whether the cell rectangle `from..=to` touches `r`.
fn touches(from: (u32, u32), to: (u32, u32), r: &RangeRef) -> bool {
    from.0 <= r.end.row && to.0 >= r.start.row && from.1 <= r.end.col && to.1 >= r.start.col
}

/// Calculate the formulas the file kept no value for, and only those: a cached value is Excel's
/// own result and stays as the file has it. The engine runs in its manual mode for this, which
/// evaluates exactly the cells it is handed (reading the cached values of the rest) and settles a
/// circular reference within the workbook's iteration limit (clamped by the model), or gives its
/// error value; it never loops.
fn recalculate(book: &mut gridcraft_model::Workbook, uncached: &[(usize, CellRef)]) {
    if uncached.is_empty() {
        return;
    }
    let mode = book.calc.mode;
    book.calc.mode = gridcraft_model::CalcMode::Manual;
    let mut calc = gridcraft_calc::Calc::new();
    calc.rebuild(book);
    calc.cells_changed(book, uncached);
    book.calc.mode = mode;
}

/// Where a formula's value would come from outside the file.
#[derive(Clone, Copy, Default)]
struct Reach {
    /// It reads another workbook.
    external: bool,
    /// It calls a function that would fetch from outside the file ([`FETCHING`]).
    fetch: bool,
}

fn reach_of(book: &gridcraft_model::Workbook, f: &gridcraft_model::Formula) -> Reach {
    let walked = f.with_expr(|e| {
        let mut reach = Reach::default();
        let missing = |name: &str| book.sheet_index(name).is_none();
        e.walk(&mut |node| match node {
            // A sheet the workbook does not have: Excel stores a deleted sheet's reference as
            // #REF!, so a name that is not here belongs to another workbook (`[1]Sheet1`,
            // `'C:\dir\[Book.xlsx]Sheet1'`, `'file:///…'`).
            Expr::Ref(r) => match &r.sheet {
                SheetSel::Current => {}
                SheetSel::Named(n) => reach.external |= missing(n),
                SheetSel::Span(a, b) => reach.external |= missing(a) || missing(b),
            },
            Expr::Call(name, _) => reach.fetch |= FETCHING.contains(&name.as_str()),
            _ => {}
        });
        reach
    });
    walked.unwrap_or_else(|| Reach {
        external: names_a_link(&f.text),
        fetch: FETCHING.iter().any(|name| calls(&f.text, name)),
    })
}

/// For a formula the parser could not read: does its text, outside string literals, name another
/// workbook (`[1]Sheet1!A1`, or a quoted sheet name holding a book or a path: `'[Book.xlsx]S'!A1`,
/// `'C:\dir\[Book.xlsx]S'!A1`, `'file:///x'!A1`)?
fn names_a_link(text: &str) -> bool {
    let b = text.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => in_string = !in_string,
            b'[' if !in_string => {
                let digits = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
                let close = i + 1 + digits;
                if digits > 0 && b.get(close) == Some(&b']') {
                    // `[1]Sheet1!A1`: a sheet name and its `!` follow, before any operator.
                    let rest = &b[close + 1..];
                    let end = rest.iter().position(|c| b" ,()+-*/&=<>;^".contains(c)).unwrap_or(rest.len());
                    if !rest.is_empty() && rest[0] != b']' && rest[..end].contains(&b'!') {
                        return true;
                    }
                }
            }
            b'\'' if !in_string => {
                // A quoted sheet name: up to the closing quote (`''` is a quote inside it).
                let start = i + 1;
                let mut k = start;
                while k < b.len() {
                    if b[k] == b'\'' {
                        if b.get(k + 1) == Some(&b'\'') {
                            k += 2;
                            continue;
                        }
                        break;
                    }
                    k += 1;
                }
                if k >= b.len() {
                    return false;
                }
                let name = &text[start..k];
                if b.get(k + 1) == Some(&b'!') && (name.contains('[') || name.contains(":/") || name.contains(":\\") || name.starts_with("\\\\")) {
                    return true;
                }
                i = k;
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// Does `text` call the function `name` (any case, the call's `(` right after the name, the name
/// not the end of a longer one)?
fn calls(text: &str, name: &str) -> bool {
    let upper = text.to_ascii_uppercase();
    let mut from = 0;
    while let Some(i) = upper[from..].find(name) {
        let at = from + i;
        let before = upper[..at].chars().next_back();
        let after = upper[at + name.len()..].trim_start();
        if !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') && after.starts_with('(') {
            return true;
        }
        from = at + name.len();
    }
    false
}

/// The charts and pictures of a sheet with their place on the grid (ids in that order: charts,
/// then pictures), and the content that is not drawn.
fn place_objects(sh: &Sheet, ext: &mut SheetExtra, limits: &Limits) {
    let mut add = |kind: AnchorKind, a: &gridcraft_model::Anchor, source: Source, over_limits: bool| {
        let Some(mut anchor) = place(sh, a) else { return false };
        anchor.id = ext.anchors.len() as u32;
        anchor.kind = kind;
        ext.anchors.push(Placed { anchor, source, over_limits });
        true
    };
    let mut unplaced = Vec::new();
    for (i, c) in sh.charts.iter().enumerate() {
        if !add(AnchorKind::Chart, &c.anchor, Source::Chart(i), false) {
            unplaced.push(c.anchor);
        }
    }
    for (i, p) in sh.images.iter().enumerate() {
        let over = draw::over_limits(&p.data, limits);
        if !add(AnchorKind::Picture, &p.anchor, Source::Picture(i), over) {
            unplaced.push(p.anchor);
        }
    }
    // An object with no area is not listed; the cell it starts in still reports it.
    for a in unplaced {
        ext.undrawn.push(RangeRef::new(a.cell, a.cell));
    }
    let anchors = sh.shapes.iter().map(|s| s.anchor).chain(sh.slicers.iter().map(|s| s.anchor)).chain(sh.controls.iter().map(|c| c.anchor));
    for a in anchors {
        let r = match place(sh, &a) {
            Some(p) => RangeRef::new(CellRef::new(p.from.0, p.from.1), CellRef::new(p.to.0, p.to.1)),
            None => RangeRef::new(a.cell, a.cell),
        };
        ext.undrawn.push(r);
    }
    ext.undrawn.extend(sh.cell_pictures.keys().map(|c| RangeRef::new(*c, *c)));
    ext.undrawn.extend(sh.sparklines.iter().map(|s| RangeRef::new(s.cell, s.cell)));
}

/// An object's place on the grid in points, from the model's anchor (pixels): the cells its
/// top-left and bottom-right corners are in, the offset inside the first, the size. `None` for an
/// object without a finite area. `id` and `kind` are left for the caller.
fn place(sh: &Sheet, a: &gridcraft_model::Anchor) -> Option<Anchor> {
    let (w, h) = (a.width, a.height);
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 && a.dx.is_finite() && a.dy.is_finite()) {
        return None;
    }
    let (w_pt, h_pt) = (w * PT_PER_PX, h * PT_PER_PX);
    if !(w_pt.is_finite() && h_pt.is_finite() && w_pt > 0.0 && h_pt > 0.0) {
        return None;
    }
    let x1 = sh.col_left(a.cell.col) + f64::from(a.dx.max(0.0));
    let y1 = sh.row_top(a.cell.row) + f64::from(a.dy.max(0.0));
    let (x2, y2) = (x1 + f64::from(w), y1 + f64::from(h));
    // An offset longer than its cell puts the corner in a later one.
    let (from_col, from_row) = (sh.col_at(x1), sh.row_at(y1));
    // A corner exactly on a cell boundary belongs to the cell before it.
    let (to_col, to_row) = (sh.col_at((x2 - 0.01).max(x1)).max(from_col), sh.row_at((y2 - 0.01).max(y1)).max(from_row));
    let x_pt = ((x1 - sh.col_left(from_col)).max(0.0) as f32) * PT_PER_PX;
    let y_pt = ((y1 - sh.row_top(from_row)).max(0.0) as f32) * PT_PER_PX;
    Some(Anchor { id: 0, kind: AnchorKind::Chart, from: (from_row, from_col), to: (to_row, to_col), x_pt, y_pt, w_pt, h_pt })
}

/// Let only the asked kind of workbook through to the reader, which also reads `.ods`, `.csv`
/// and `.xlsb` (never reachable here). Lead ruling R-D1 (2026-11-15):
///
/// - Bytes that are no container at all (neither a zip nor a compound-file signature), or a zip
///   whose central directory cannot be read: [`OpenError::Damaged`]. No reader sees them.
/// - An encrypted package: [`OpenError::Password`].
/// - A readable container of another kind (a zip package that names no `.xlsx`/`.xlsm` workbook,
///   a compound file without a workbook stream, a workbook of the other kind):
///   [`OpenError::Unsupported`].
///
/// Runs after the pre-scan, which has already refused a compound file whose header, FAT or
/// directory chain cannot be followed ([`OpenError::Damaged`]): one that passes it is readable.
fn check_content(kind: SheetKind, bytes: &[u8]) -> Result<(), OpenError> {
    if bytes.starts_with(&crate::limits::CFB_MAGIC) {
        let format = gridcraft_xlsx::sniff(bytes);
        if format == gridcraft_xlsx::Format::Encrypted {
            return Err(OpenError::Password);
        }
        return if kind == SheetKind::Xls && format == gridcraft_xlsx::Format::Xls { Ok(()) } else { Err(OpenError::Unsupported) };
    }
    if !ZIP_MAGICS.iter().any(|m| bytes.starts_with(m)) {
        return Err(OpenError::Damaged);
    }
    let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) else { return Err(OpenError::Damaged) };
    if kind == SheetKind::Xlsx && names_a_workbook(&mut zip) { Ok(()) } else { Err(OpenError::Unsupported) }
}

/// The first bytes of a zip package: a local file header, an empty archive's end record, a
/// spanned archive's marker (as the readers' own `sniff` takes them).
const ZIP_MAGICS: [&[u8]; 3] = [b"PK\x03\x04", b"PK\x05\x06", b"PK\x07\x08"];

/// Does the package's `[Content_Types].xml` name a workbook main part ([`WORKBOOK_TYPES`])? At
/// most [`MAX_CONTENT_TYPES_BYTES`] of that part are inflated.
fn names_a_workbook(zip: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>) -> bool {
    let found = (0..zip.len()).find(|&i| zip.name_for_index(i).is_some_and(|n| n.trim_start_matches('/').eq_ignore_ascii_case(CONTENT_TYPES_PART)));
    let Some(i) = found else { return false };
    let Ok(entry) = zip.by_index(i) else { return false };
    if entry.size() > MAX_CONTENT_TYPES_BYTES {
        return false;
    }
    let mut raw = Vec::new();
    if entry.take(MAX_CONTENT_TYPES_BYTES + 1).read_to_end(&mut raw).is_err() || raw.len() as u64 > MAX_CONTENT_TYPES_BYTES {
        return false;
    }
    let text = xml_text(&raw).to_ascii_lowercase();
    WORKBOOK_TYPES.iter().any(|t| text.contains(&format!("\"{t}\"")) || text.contains(&format!("'{t}'")))
}

/// The text of an XML part: UTF-16 when it starts with a byte-order mark for it, else UTF-8.
fn xml_text(raw: &[u8]) -> String {
    let utf16 = |be: bool| {
        let units: Vec<u16> = raw[2..].chunks_exact(2).map(|p| if be { u16::from_be_bytes([p[0], p[1]]) } else { u16::from_le_bytes([p[0], p[1]]) }).collect();
        String::from_utf16_lossy(&units)
    };
    match raw {
        [0xFF, 0xFE, ..] => utf16(false),
        [0xFE, 0xFF, ..] => utf16(true),
        _ => String::from_utf8_lossy(raw).into_owned(),
    }
}

/// The `.xlsx` reader's error as a code. Every variant is listed, so a new one upstream is a
/// compile error here and gets read before a subtree bump lands.
fn from_xlsx(e: gridcraft_xlsx::IoError) -> OpenError {
    use gridcraft_xlsx::IoError as E;
    match e {
        E::TooLarge(_) => OpenError::TooLarge(crate::limits::READER),
        E::Zip(_) | E::Xml(_) | E::Format(_) => OpenError::Damaged,
    }
}

/// The `.xls` reader's error as a code (every variant listed, as in [`from_xlsx`]). Its messages
/// stay here: they can quote the file.
fn from_xls(e: gridcraft_xlsx::IoError) -> OpenError {
    use gridcraft_xlsx::IoError as E;
    match e {
        E::TooLarge(_) => OpenError::TooLarge(crate::limits::READER),
        E::Format(m) if m.contains(XLS_ENCRYPTED) => OpenError::Password,
        E::Format(m) if m.contains(XLS_BIFF5) => OpenError::Unsupported,
        E::Zip(_) | E::Xml(_) | E::Format(_) => OpenError::Damaged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stub was `Send` and `Sync`; the open workbook stays so, whatever thread a host keeps it on.
    const _: fn() = || {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<Workbook>();
    };

    #[test]
    fn sheets_link_detection_reads_the_text_of_an_unparsed_formula() {
        assert!(names_a_link("[1]Sheet1!A1"));
        assert!(names_a_link("SUM([2]Data!A1:A3)"));
        assert!(names_a_link("'C:\\dir\\[Book.xlsx]Sheet1'!A1"));
        assert!(names_a_link("'file:///tmp/x'!A1"));
        assert!(!names_a_link("\"[1]Sheet1!A1\""), "inside a string it is text");
        assert!(!names_a_link("Table1[[#This Row],[2020]]"), "a column named 2020");
        assert!(!names_a_link("'My Sheet'!A1"));
    }

    #[test]
    fn sheets_fetch_detection_reads_calls_only() {
        assert!(calls("webservice(\"http://x\")", "WEBSERVICE"));
        assert!(calls("1+RTD (\"a\",,\"b\")", "RTD"));
        assert!(!calls("MYWEBSERVICE(1)", "WEBSERVICE"));
        assert!(!calls("WEBSERVICE_X+1", "WEBSERVICE"));
    }

    #[test]
    fn sheets_page_pixels_refuse_a_bad_scale_and_an_oversized_side() {
        assert_eq!(page_pixels(10.0, 10.0, 0.0, 4096), Err(RenderError::BadScale));
        assert_eq!(page_pixels(10.0, 10.0, f32::NAN, 4096), Err(RenderError::BadScale));
        assert_eq!(page_pixels(5000.0, 10.0, 1.0, 4096), Err(RenderError::TooLarge));
        assert_eq!(page_pixels(0.1, 0.1, 1.0, 4096), Ok((1, 1)));
    }
}
