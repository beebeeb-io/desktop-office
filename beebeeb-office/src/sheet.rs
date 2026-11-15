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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellText {
    pub row: u32,
    pub col: u32,
    pub text: String,
    pub align: CellAlign,
    pub bold: bool,
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

    /// Render the chart or picture `anchor_id` of sheet `sheet` at `scale` into `out` (RGBA8,
    /// exactly `w * h * 4` bytes).
    pub fn render_anchor(&self, _sheet: u32, _anchor_id: u32, _scale: f32, _out: &mut [u8]) -> Result<RenderInfo, RenderError> {
        match self.never {}
    }
}
