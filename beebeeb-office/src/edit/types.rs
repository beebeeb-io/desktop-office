//! The edit API's data types. They mirror the protocol crate's (`beebeeb-office-protocol`, `office/edit.rs`)
//! one for one; the helper maps them in `src/edit_map.rs`. This crate does not depend on the protocol.

pub use crate::CellRange;

/// Which kind of document is edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    Docx,
    Xlsx,
    Pptx,
}

/// The built-in starting points of a new document. Sheets and Slides know `Blank` only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Template {
    Blank,
    Letter,
    Invoice,
    MeetingNotes,
    Report,
}

/// What a click does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitMode {
    Place,
    Extend,
    Word,
    Paragraph,
}

/// Where a caret move goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    PageUp,
    PageDown,
}

/// A character mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Bold,
    Italic,
    Underline,
    Strike,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParaStyle {
    Normal,
    Title,
    Heading1,
    Heading2,
    Heading3,
}

/// `None` is "not a list item".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListKind {
    None,
    Bullet,
    Number,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberFormat {
    General,
    Currency,
    Percent,
    MoreDecimals,
    FewerDecimals,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// A rectangle on a page (slide), in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectPt {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageRect {
    pub page: u32,
    pub rect: RectPt,
}

/// One edit. A closed list: nothing names an engine command. An image is [`Op::Image`].
#[derive(Clone, PartialEq)]
pub enum Op {
    // Text.
    InsertText(String),
    InsertParagraph,
    InsertLineBreak,
    Delete { back: bool, word: bool },
    Move { to: Motion, extend: bool },
    SelectAll,
    // Format.
    Mark { mark: Mark, on: bool },
    FontSize(f32),
    FontFamily(String),
    TextColour(Rgb),
    Highlight(Option<Rgb>),
    ParaStyle(ParaStyle),
    List(ListKind),
    Align(TextAlign),
    Indent(i8),
    // Insert.
    Table { rows: u32, cols: u32 },
    Link { text: String, url: String },
    PageBreak,
    /// Picture bytes the host read from a grant; `png` says PNG (else JPEG).
    Image { bytes: Vec<u8>, png: bool },
    // Sheets.
    SetCell { sheet: u32, row: u32, col: u32, text: String },
    SelectCells { sheet: u32, range: CellRange },
    NumberFormat(NumberFormat),
    FillColour(Option<Rgb>),
    AutoSum,
    Sort { ascending: bool },
    Filter,
    MakeTable,
    RenameSheet { sheet: u32, name: String },
    // Slides.
    SelectShape { slide: u32, x_pt: f32, y_pt: f32 },
    ClearShape,
    NewSlide { layout: Option<u32> },
    DuplicateSlide(u32),
    DeleteSlide(u32),
    MoveSlide { from: u32, to: u32 },
    SetLayout { slide: u32, layout: u32 },
    /// Plain text only.
    Paste { text: String },
    Undo,
    Redo,
}

/// The text and bytes of an op are document content, so `Debug` shows what kind and how long.
impl std::fmt::Debug for Op {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Op::InsertText(s) | Op::Paste { text: s } => write!(f, "Text(len {})", s.len()),
            Op::FontFamily(s) => write!(f, "FontFamily(len {})", s.len()),
            Op::Link { text, url } => write!(f, "Link(text len {}, url len {})", text.len(), url.len()),
            Op::Image { bytes, png } => write!(f, "Image(len {}, png {png})", bytes.len()),
            Op::SetCell { sheet, row, col, text } => write!(f, "SetCell({sheet}, {row}, {col}, len {})", text.len()),
            Op::RenameSheet { sheet, name } => write!(f, "RenameSheet({sheet}, len {})", name.len()),
            Op::InsertParagraph => f.write_str("InsertParagraph"),
            Op::InsertLineBreak => f.write_str("InsertLineBreak"),
            Op::Delete { back, word } => write!(f, "Delete(back {back}, word {word})"),
            Op::Move { to, extend } => write!(f, "Move({to:?}, extend {extend})"),
            Op::SelectAll => f.write_str("SelectAll"),
            Op::Mark { mark, on } => write!(f, "Mark({mark:?}, {on})"),
            Op::FontSize(v) => write!(f, "FontSize({v})"),
            Op::TextColour(c) => write!(f, "TextColour({c:?})"),
            Op::Highlight(c) => write!(f, "Highlight({c:?})"),
            Op::ParaStyle(s) => write!(f, "ParaStyle({s:?})"),
            Op::List(k) => write!(f, "List({k:?})"),
            Op::Align(a) => write!(f, "Align({a:?})"),
            Op::Indent(n) => write!(f, "Indent({n})"),
            Op::Table { rows, cols } => write!(f, "Table({rows}x{cols})"),
            Op::PageBreak => f.write_str("PageBreak"),
            Op::SelectCells { sheet, range } => write!(f, "SelectCells({sheet}, {range:?})"),
            Op::NumberFormat(n) => write!(f, "NumberFormat({n:?})"),
            Op::FillColour(c) => write!(f, "FillColour({c:?})"),
            Op::AutoSum => f.write_str("AutoSum"),
            Op::Sort { ascending } => write!(f, "Sort({ascending})"),
            Op::Filter => f.write_str("Filter"),
            Op::MakeTable => f.write_str("MakeTable"),
            Op::SelectShape { slide, x_pt, y_pt } => write!(f, "SelectShape({slide}, {x_pt}, {y_pt})"),
            Op::ClearShape => f.write_str("ClearShape"),
            Op::NewSlide { layout } => write!(f, "NewSlide({layout:?})"),
            Op::DuplicateSlide(n) => write!(f, "DuplicateSlide({n})"),
            Op::DeleteSlide(n) => write!(f, "DeleteSlide({n})"),
            Op::MoveSlide { from, to } => write!(f, "MoveSlide({from}, {to})"),
            Op::SetLayout { slide, layout } => write!(f, "SetLayout({slide}, {layout})"),
            Op::Undo => f.write_str("Undo"),
            Op::Redo => f.write_str("Redo"),
        }
    }
}

/// The marks and styles at the caret or over the selection, for the toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marks {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub style: ParaStyle,
    pub align: TextAlign,
    pub list: ListKind,
}

/// What to redraw.
#[derive(Clone, Debug, PartialEq)]
pub enum Dirty {
    All,
    Rects(Vec<PageRect>),
}

/// Pages or slides: count and sizes (points), when they changed.
#[derive(Clone, Debug, PartialEq)]
pub struct PagesChanged {
    pub sizes_pt: Vec<(f32, f32)>,
}

/// What an edit changed.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub revision: u64,
    pub caret: Option<PageRect>,
    pub selection: Vec<PageRect>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub dirty: Dirty,
    pub pages: Option<PagesChanged>,
    /// Changed ranges: (sheet, range).
    pub cells: Vec<(u32, CellRange)>,
    /// Set when the slide count changed.
    pub slides: Option<u32>,
    pub marks: Marks,
}

/// What a copy puts on the clipboard.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Clip {
    pub text: String,
    pub html: Option<String>,
}

/// Why an edit was not applied. Each is a code; engine messages (which can quote content) are dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditError {
    /// This build or this kind does not do that (yet).
    Unsupported,
    /// The op does not apply to this kind of document, or to what is selected.
    NotApplicable,
    TooLarge,
    Engine,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "unsupported",
            Self::NotApplicable => "not applicable",
            Self::TooLarge => "too large",
            Self::Engine => "engine",
        })
    }
}

impl std::error::Error for EditError {}

/// Why a document could not be serialised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveError {
    Unsupported,
    TooLarge,
    Engine,
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "unsupported",
            Self::TooLarge => "too large",
            Self::Engine => "engine",
        })
    }
}

impl std::error::Error for SaveError {}
