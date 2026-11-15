//! Workbooks (`.xlsx`, `.xls`): the final API, not built yet. [`Workbook::open`] says
//! [`OpenError::Unsupported`] for every file, so no [`Workbook`] can exist and the methods below
//! are never reached. The sheets slice fills this file in; the types may grow fields there.

use std::convert::Infallible;

use crate::{Limits, OpenError, RenderError, RenderInfo};

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

/// An open workbook. None can exist yet ([`Workbook::open`]).
pub struct Workbook {
    never: Infallible,
}

impl Workbook {
    /// Open a workbook from its bytes. Not built yet: [`OpenError::Unsupported`] for every file.
    pub fn open(_kind: SheetKind, _bytes: &[u8], _limits: &Limits) -> Result<Workbook, OpenError> {
        Err(OpenError::Unsupported)
    }

    /// The sheets.
    pub fn info(&self) -> SheetsInfo {
        match self.never {}
    }

    /// The cells of `range` on sheet `sheet` (0-based).
    pub fn cells(&self, _sheet: u32, _range: CellRange) -> CellBlock {
        match self.never {}
    }

    /// The size in pixels of what [`Workbook::render_anchor`] writes for anchor `anchor` of sheet
    /// `sheet` at `scale`: the caller sizes its buffer from this.
    pub fn anchor_size(&self, _sheet: u32, _anchor: u32, _scale: f32) -> Result<(u32, u32), RenderError> {
        match self.never {}
    }

    /// Render the chart or picture `anchor_id` of sheet `sheet` at `scale` into `out` (RGBA8,
    /// exactly `w * h * 4` bytes).
    pub fn render_anchor(&self, _sheet: u32, _anchor_id: u32, _scale: f32, _out: &mut [u8]) -> Result<RenderInfo, RenderError> {
        match self.never {}
    }
}
