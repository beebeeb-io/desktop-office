//! Workbooks through `Workbook`: sheets, cells as their formats show them, values and formulas,
//! the recalculation of formulas without a cached value only, ranges, merges and frozen panes,
//! charts and pictures as pixels, the limits, and the content check. Every fixture but the corpus
//! file is built here with GridCraft's own writer (or by hand, for the packages it cannot write).
//! Filter: `sheets`.

mod common;

use std::io::Read as _;
use std::sync::Arc;

use beebeeb_office::{AnchorKind, CellAlign, CellBlock, CellRange, CellText, Limits, OpenError, RenderError, SheetKind, Workbook, probe};
use common::{colour_stats, zip_stored};
use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_model as gm;

// ---------------------------------------------------------------- fixtures

fn sheet(name: &str) -> gm::Sheet {
    gm::Sheet::new(name)
}

fn book(sheets: Vec<gm::Sheet>) -> gm::Workbook {
    let mut wb = gm::Workbook::new();
    wb.sheets = sheets.into_iter().map(Arc::new).collect();
    wb
}

fn xlsx(wb: &gm::Workbook) -> Vec<u8> {
    gridcraft_xlsx::write_xlsx(wb).expect("the writer writes the workbook")
}

fn open(bytes: &[u8]) -> Workbook {
    Workbook::open(SheetKind::Xlsx, bytes, &Limits::default()).unwrap_or_else(|e| panic!("the workbook opens: {e}"))
}

fn num(v: f64) -> gm::Cell {
    gm::Cell::value(Value::Number(v))
}

fn text(t: &str) -> gm::Cell {
    gm::Cell::value(Value::Text(Arc::from(t)))
}

/// A cell holding `formula` with `cached` as the value the file keeps (`None`: no cached value).
fn formula(f: &str, cached: Option<f64>) -> gm::Cell {
    let mut c = gm::Cell::formula(gm::Formula::new(f));
    if let Some(v) = cached {
        c.value = Value::Number(v);
    }
    c
}

/// A style with the number format `code`.
fn style(wb: &mut gm::Workbook, code: &str) -> gm::StyleId {
    let s = gm::Style { num_fmt: gm::NumFmt::new(code), ..gm::Style::default() };
    wb.styles.intern(s)
}

fn set(wb: &mut gm::Workbook, sheet: usize, row: u32, col: u32, cell: gm::Cell) {
    Arc::make_mut(&mut wb.sheets[sheet]).set_cell(CellRef::new(row, col), cell);
}

fn whole(rows: u32, cols: u32) -> CellRange {
    CellRange { row: 0, col: 0, rows, cols }
}

fn cell(block: &CellBlock, row: u32, col: u32) -> &CellText {
    block.cells.iter().find(|c| c.row == row && c.col == col).unwrap_or_else(|| panic!("a cell at ({row}, {col}) in {:?}", block.cells))
}

fn anchor(row: u32, col: u32, w_px: f32, h_px: f32) -> gm::Anchor {
    gm::Anchor { cell: CellRef::new(row, col), width: w_px, height: h_px, ..gm::Anchor::default() }
}

/// A clustered column chart of B2:B5 by A2:A5 on sheet "Data", with a title.
fn column_chart(id: u32, at: gm::Anchor) -> gm::Chart {
    gm::Chart {
        id,
        kind: gm::ChartKind::ColumnClustered,
        anchor: at,
        title: Some("Revenue".into()),
        series: vec![gm::Series {
            name: Some("Revenue".into()),
            categories: Some("Data!$A$2:$A$5".into()),
            values: "Data!$B$2:$B$5".into(),
            ..gm::Series::default()
        }],
        legend: gm::LegendPos::Bottom,
        data_labels: false,
        data_table: None,
        gridlines: true,
        style: 2,
        hole_size: 75,
        first_slice_angle: 0,
        x_title: None,
        y_title: Some("EUR".into()),
        source: None,
        by_rows: false,
        pivot: None,
    }
}

/// A PNG of `w` x `h` pixels, one colour.
fn png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
    let pixels = rgba.repeat(w as usize * h as usize);
    let png = wordcraft_render::Rendered { width: w, height: h, pixels }.to_png();
    assert!(!png.is_empty(), "the {w} x {h} PNG encodes");
    png
}

fn picture(id: u32, at: gm::Anchor, data: Vec<u8>) -> gm::Image {
    gm::Image { id, anchor: at, data: Arc::new(data), mime: "image/png".into(), alt: String::new() }
}

/// Sheet "Data": quarters in A2:A5, numbers in B2:B5, a chart over D2:H12 and a picture over
/// A20:B25 (default column width 64 px, row height 20 px).
fn charted() -> gm::Workbook {
    let mut wb = book(vec![sheet("Data")]);
    for (i, (q, v)) in [("Q1", 120.0), ("Q2", 340.0), ("Q3", 280.0), ("Q4", 410.0)].into_iter().enumerate() {
        set(&mut wb, 0, i as u32 + 1, 0, text(q));
        set(&mut wb, 0, i as u32 + 1, 1, num(v));
    }
    let sh = Arc::make_mut(&mut wb.sheets[0]);
    // D2 (row 1, column 3), five columns by eleven rows: its corner is on the H|I and 12|13 lines.
    sh.charts.push(column_chart(1, anchor(1, 3, 320.0, 220.0)));
    // A20 (row 19), two columns by six rows.
    sh.images.push(picture(2, anchor(19, 0, 128.0, 120.0), png(8, 8, [0x20, 0x80, 0x40, 0xFF])));
    wb
}

/// Render anchor `id` of sheet 0 at `scale`.
fn render(book: &Workbook, id: u32, scale: f32) -> (beebeeb_office::RenderInfo, Vec<u8>) {
    let (w, h) = book.anchor_size(0, id, scale).expect("the anchor's size");
    let mut out = vec![0u8; w as usize * h as usize * 4];
    let info = book.render_anchor(0, id, scale, &mut out).expect("the anchor renders");
    (info, out)
}

/// The entries of a zip package, inflated.
fn entries(package: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(package)).expect("a zip");
    (0..zip.len())
        .map(|i| {
            let mut e = zip.by_index(i).expect("an entry");
            let mut data = Vec::new();
            e.read_to_end(&mut data).expect("the entry inflates");
            (e.name().to_string(), data)
        })
        .collect()
}

/// In a module of their own so the filter `sheets` selects every test here by its name.
mod sheets {
    use super::*;

    // ---------------------------------------------------------------- sheets and cells

    #[test]
    fn a_workbook_opens_with_its_sheets_in_tab_order() {
        let mut wb = book(vec![sheet("2026"), sheet("2025"), sheet("Notes")]);
        set(&mut wb, 0, 3, 2, num(1.0)); // C4
        set(&mut wb, 1, 1, 1, text("x")); // B2
        let got = open(&xlsx(&wb)).info();
        let want: Vec<(&str, u32, u32)> = vec![("2026", 4, 3), ("2025", 2, 2), ("Notes", 0, 0)];
        let got: Vec<(&str, u32, u32)> = got.sheets.iter().map(|s| (s.name.as_str(), s.rows, s.cols)).collect();
        assert_eq!(got, want, "tab order, each with rows and columns up to its last non-empty cell");
    }

    /// Five cells in column A with their formats.
    fn formatted() -> gm::Workbook {
        let mut wb = book(vec![sheet("Formats")]);
        let cells = [
            (1234.5, "#,##0.00 €"),
            (0.125, "0.0%"),
            // 3 November 2026 in the 1900 date system.
            (46329.0, "d mmm yyyy"),
            (3600.0, "#,##0"),
            (0.1, "General"),
        ];
        for (row, (v, code)) in cells.into_iter().enumerate() {
            let id = style(&mut wb, code);
            set(&mut wb, 0, row as u32, 0, gm::Cell::styled(Value::Number(v), id));
        }
        set(&mut wb, 0, 5, 0, text("Total"));
        wb
    }

    #[test]
    fn cells_show_their_number_format() {
        let block = open(&xlsx(&formatted())).cells(0, whole(6, 1));
        let shown: Vec<&str> = (0..5).map(|r| cell(&block, r, 0).text.as_str()).collect();
        assert_eq!(shown, ["1,234.50 €", "12.5%", "3 Nov 2026", "3,600", "0.1"], "the texts Excel shows");
        assert_eq!(cell(&block, 0, 0).align, CellAlign::Right, "a number sits right");
        assert_eq!(cell(&block, 5, 0).align, CellAlign::Left, "text sits left");
    }

    #[test]
    fn a_numeric_cell_carries_its_value_and_a_text_cell_none() {
        let block = open(&xlsx(&formatted())).cells(0, whole(6, 1));
        assert_eq!(cell(&block, 1, 0).value, Some(0.125), "12.5 % is 0.125");
        assert_eq!(cell(&block, 2, 0).value, Some(46329.0), "a date is its serial");
        assert_eq!(cell(&block, 5, 0).value, None, "\"Total\" has no number");
    }

    /// B2:B9 summing to 13,220 and B10 `=SUM(B2:B9)` keeping `cached`; B11 `=B10*2` with no cached
    /// value, so the recalculation always runs.
    fn summed(cached: Option<f64>) -> gm::Workbook {
        let mut wb = book(vec![sheet("Sum")]);
        let thousands = style(&mut wb, "#,##0");
        for (i, v) in [1000.0, 2000.0, 1500.0, 2500.0, 1220.0, 3000.0, 1000.0, 1000.0].into_iter().enumerate() {
            set(&mut wb, 0, i as u32 + 1, 1, num(v));
        }
        let mut total = formula("=SUM(B2:B9)", cached);
        total.style = thousands;
        set(&mut wb, 0, 9, 1, total);
        set(&mut wb, 0, 10, 1, formula("=B10*2", None));
        wb
    }

    #[test]
    fn a_formula_cell_carries_its_formula() {
        let block = open(&xlsx(&summed(Some(13220.0)))).cells(0, whole(11, 2));
        assert_eq!(cell(&block, 9, 1).formula.as_deref(), Some("=SUM(B2:B9)"));
        assert_eq!(cell(&block, 1, 1).formula, None, "a constant has none");
    }

    #[test]
    fn a_formula_without_a_cached_value_is_calculated() {
        let block = open(&xlsx(&summed(None))).cells(0, whole(11, 2));
        let total = cell(&block, 9, 1);
        assert_eq!((total.text.as_str(), total.value), ("13,220", Some(13220.0)), "calculated at open");
        assert_eq!(cell(&block, 10, 1).value, Some(26440.0), "a formula on a calculated one");
    }

    #[test]
    fn a_cached_value_is_shown_as_the_file_has_it() {
        // Deliberately not what the formula gives: the file's value is Excel's result and is shown.
        let block = open(&xlsx(&summed(Some(99999.0)))).cells(0, whole(11, 2));
        let total = cell(&block, 9, 1);
        assert_eq!((total.text.as_str(), total.value), ("99,999", Some(99999.0)), "the cached value, not 13,220");
        assert_eq!(cell(&block, 10, 1).value, Some(199998.0), "the uncached formula reads the cached value");
    }

    #[test]
    fn a_range_returns_only_its_cells_and_its_sizes() {
        let mut wb = book(vec![sheet("Grid")]);
        for row in 0..6 {
            for col in 0..6 {
                set(&mut wb, 0, row, col, num(f64::from(row * 10 + col)));
            }
        }
        let sh = Arc::make_mut(&mut wb.sheets[0]);
        sh.rows.insert(2, gm::LineInfo { size: Some(40.0), custom: true, ..gm::LineInfo::default() });
        let block = open(&xlsx(&wb)).cells(0, CellRange { row: 1, col: 1, rows: 3, cols: 3 });
        assert_eq!(block.cells.len(), 9, "B2:D4 holds nine cells: {:?}", block.cells);
        assert!(block.cells.iter().all(|c| (1..=3).contains(&c.row) && (1..=3).contains(&c.col)), "all inside B2:D4");
        assert_eq!(block.col_widths, vec![48.0; 3], "three default columns, 64 px = 48 pt");
        assert_eq!(block.row_heights, vec![15.0, 30.0, 15.0], "row 3 is 40 px = 30 pt, the others 15 pt");
    }

    #[test]
    fn merges_and_frozen_panes_come_from_the_model() {
        let mut wb = book(vec![sheet("Panes")]);
        set(&mut wb, 0, 0, 0, text("Title"));
        let sh = Arc::make_mut(&mut wb.sheets[0]);
        sh.merges.push(RangeRef::new(CellRef::new(0, 0), CellRef::new(0, 2))); // A1:C1
        sh.merges.push(RangeRef::new(CellRef::new(9, 4), CellRef::new(10, 5))); // E10:F11
        sh.freeze = Some((1, 1));
        let block = open(&xlsx(&wb)).cells(0, whole(5, 4));
        assert_eq!(block.merges, vec![CellRange { row: 0, col: 0, rows: 1, cols: 3 }], "A1:C1 only; E10:F11 is outside A1:D5");
        assert_eq!(block.frozen, (1, 1), "one row and one column frozen");
    }

    /// The negative control of the external-reference check: a reference to a sheet of this
    /// workbook (quoted, in another case, its own sheet named, a 3-D span) keeps the value the file
    /// has and is not flagged.
    #[test]
    fn a_reference_to_a_sheet_of_the_workbook_is_not_external() {
        let mut wb = book(vec![sheet("Data"), sheet("My Sheet")]);
        set(&mut wb, 1, 0, 1, num(42.0)); // 'My Sheet'!B1
        set(&mut wb, 0, 1, 1, num(7.0)); // Data!B2
        let refs = [("='My Sheet'!B1", 42.0), ("='MY SHEET'!B1", 42.0), ("=Data!B2", 7.0), ("=SUM(Data:'My Sheet'!B1)", 42.0)];
        for (row, (f, v)) in refs.iter().enumerate() {
            set(&mut wb, 0, row as u32 + 3, 0, formula(f, Some(*v)));
        }
        let block = open(&xlsx(&wb)).cells(0, whole(8, 2));
        for (row, (f, v)) in refs.iter().enumerate() {
            let c = cell(&block, row as u32 + 3, 0);
            assert_eq!(c.value, Some(*v), "{f}: the value the file has");
            assert_eq!(c.text, v.to_string(), "{f}: shown");
        }
        assert!(!block.missing_content, "nothing here comes from outside the file");
    }

    // ---------------------------------------------------------------- charts and pictures

    #[test]
    fn anchors_touching_the_range_are_listed() {
        let book = open(&xlsx(&charted()));
        assert!(book.cells(0, whole(10, 3)).anchors.is_empty(), "A1:C10 touches neither");
        let block = book.cells(0, CellRange { row: 0, col: 3, rows: 5, cols: 2 });
        assert_eq!(block.anchors.len(), 1, "D1:E5 touches the chart only: {:?}", block.anchors);
        let a = &block.anchors[0];
        assert_eq!((a.kind, a.from, a.to), (AnchorKind::Chart, (1, 3), (11, 7)), "the chart over D2:H12");
        assert_eq!((a.w_pt, a.h_pt, a.x_pt, a.y_pt), (240.0, 165.0, 0.0, 0.0), "320 x 220 px in points");
        let low = book.cells(0, CellRange { row: 21, col: 1, rows: 1, cols: 1 });
        assert_eq!(low.anchors.iter().map(|a| (a.kind, a.from, a.to)).collect::<Vec<_>>(), vec![(AnchorKind::Picture, (19, 0), (24, 1))], "B22 is under the picture");
    }

    #[test]
    fn a_chart_renders_as_pixels_of_its_size() {
        let book = open(&xlsx(&charted()));
        assert_eq!(book.anchor_size(0, 0, 1.0), Ok((240, 165)));
        assert_eq!(book.anchor_size(0, 0, 1.5), Ok((360, 248)), "247.5 rounds up");
        let (info, px) = render(&book, 0, 1.5);
        common::save_png("sheets-chart", info.w, info.h, &px);
        let (colours, top) = colour_stats(&px);
        assert!(colours >= 10, "a drawn chart has bars, axes and text: {colours} colours");
        assert!(top < 0.98, "not one flat colour: {top}");
        assert!(!info.missing_content, "everything of the chart is drawn");
    }

    #[test]
    fn an_anchor_size_follows_the_page_rule() {
        // A Letter-sized chart (816 x 1056 px = 612 x 792 pt) and a 99 px picture (74.25 pt).
        let mut wb = charted();
        let sh = Arc::make_mut(&mut wb.sheets[0]);
        sh.charts[0].anchor = anchor(0, 0, 816.0, 1056.0);
        sh.images[0].anchor = anchor(60, 0, 99.0, 99.0);
        let book = open(&xlsx(&wb));
        let bits = |s: &str| f32::from_bits(u32::from_str_radix(s.trim_start_matches("0x"), 16).expect("hex bits"));
        let mut rows = 0;
        for line in include_str!("page_pixels_vectors.txt").lines().filter(|l| l.starts_with("letter-w")) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            let scale = bits(cols[4]);
            let want_w: u32 = cols[5].parse().expect("px");
            let want_h = include_str!("page_pixels_vectors.txt")
                .lines()
                .filter(|l| l.starts_with("letter-h"))
                .map(|l| l.split_whitespace().collect::<Vec<_>>())
                .find(|c| bits(c[4]).to_bits() == scale.to_bits())
                .map(|c| c[5].parse::<u32>().expect("px"))
                .expect("the letter-h row at the same scale");
            assert_eq!(book.anchor_size(0, 0, scale), Ok((want_w, want_h)), "Letter at scale {scale}");
            rows += 1;
        }
        assert_eq!(rows, 5, "five scales");
        assert_eq!(book.anchor_size(0, 1, 1.0), Ok((75, 75)), "74.25 pt rounds up, not to the nearest");
    }

    #[test]
    fn a_wrong_buffer_or_scale_is_refused() {
        let book = open(&xlsx(&charted()));
        for id in [0, 1] {
            let (w, h) = book.anchor_size(0, id, 1.0).expect("size");
            let mut short = vec![0u8; (w * h * 4 - 4) as usize];
            assert_eq!(book.render_anchor(0, id, 1.0, &mut short), Err(RenderError::BufferSize), "anchor {id}: a short buffer");
            let mut out = vec![0u8; (w * h * 4) as usize];
            assert_eq!(book.render_anchor(0, id, 0.0, &mut out), Err(RenderError::BadScale), "anchor {id}: scale 0");
            assert_eq!(book.render_anchor(0, id, f32::NAN, &mut out), Err(RenderError::BadScale), "anchor {id}: scale NaN");
        }
        let mut out = vec![0u8; 4];
        assert_eq!(book.render_anchor(0, 99, 1.0, &mut out), Err(RenderError::NoSuchPage), "anchor 99");
        assert_eq!(book.anchor_size(1, 0, 1.0), Err(RenderError::NoSuchPage), "sheet 1");
    }

    #[test]
    fn a_picture_over_the_image_limits_is_not_decoded() {
        // Control: the 8 x 8 picture is handed to the decoder and drawn.
        let book = open(&xlsx(&charted()));
        probe::reset();
        let (info, px) = render(&book, 1, 1.0);
        assert!(!info.missing_content, "the small picture is shown");
        assert!(probe::bytes_to_readers() > 0, "the decoder was handed the small picture");
        assert!(px.chunks_exact(4).filter(|p| p == &[0x20, 0x80, 0x40, 0xFF]).count() > 100, "its colour is drawn");

        // 20,000 x 10 px: over the 16,384 px side limit by its header.
        let mut wb = charted();
        Arc::make_mut(&mut wb.sheets[0]).images[0].data = Arc::new(png(20_000, 10, [0x20, 0x80, 0x40, 0xFF]));
        let book = open(&xlsx(&wb));
        probe::reset();
        let (info, px) = render(&book, 1, 1.0);
        assert!(info.missing_content, "the large picture is not shown");
        assert_eq!(probe::bytes_to_readers(), 0, "and never handed to the decoder");
        assert_eq!(px.chunks_exact(4).filter(|p| p == &[0x20, 0x80, 0x40, 0xFF]).count(), 0, "none of it is drawn");
        assert!(book.cells(0, CellRange { row: 19, col: 0, rows: 1, cols: 1 }).missing_content, "the block under it says so");
        assert!(!book.cells(0, whole(10, 3)).missing_content, "a block away from it does not");
    }

    #[test]
    fn over_the_sheet_or_cell_limit_is_too_large() {
        let three = xlsx(&book(vec![sheet("a"), sheet("b"), sheet("c")]));
        let two_sheets = Limits { max_sheets: 2, ..Limits::default() };
        assert_eq!(Workbook::open(SheetKind::Xlsx, &three, &two_sheets).err(), Some(OpenError::TooLarge(beebeeb_office::limits::SHEETS)));
        let cells = |n: u32| {
            let mut wb = book(vec![sheet("n")]);
            for i in 0..n {
                set(&mut wb, 0, i, 0, num(f64::from(i)));
            }
            xlsx(&wb)
        };
        let ten = Limits { max_cells: 10, ..Limits::default() };
        assert_eq!(Workbook::open(SheetKind::Xlsx, &cells(11), &ten).err(), Some(OpenError::TooLarge(beebeeb_office::limits::CELLS)), "11 cells over 10");
        assert!(Workbook::open(SheetKind::Xlsx, &cells(10), &ten).is_ok(), "10 cells at 10 open");
    }

    #[test]
    fn only_a_workbook_of_the_asked_kind_opens() {
        let workbook = xlsx(&formatted());
        assert!(Workbook::open(SheetKind::Xlsx, &workbook, &Limits::default()).is_ok(), "control: the .xlsx opens as Xlsx");
        let xls = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus/xls/SimpleWithColours.xls")).expect("the corpus .xls");
        assert!(Workbook::open(SheetKind::Xls, &xls, &Limits::default()).is_ok(), "control: the .xls opens as Xls");
        // Readable containers of another kind (lead ruling R-D1): Unsupported, and no reader sees them.
        let ods = zip_stored(&[
            ("mimetype", &b"application/vnd.oasis.opendocument.spreadsheet"[..]),
            ("content.xml", &br#"<?xml version="1.0"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"/>"#[..]),
        ]);
        let xlsb_types = br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/workbook.bin" ContentType="application/vnd.ms-excel.sheet.binary.macroEnabled.main"/></Types>"#;
        let xlsb = zip_stored(&[("[Content_Types].xml", &xlsb_types[..]), ("xl/workbook.bin", &b"\x83\x01\x00"[..])]);
        for (what, kind, bytes) in [
            ("an .ods package as Xlsx", SheetKind::Xlsx, &ods),
            ("an .xlsb package as Xlsx", SheetKind::Xlsx, &xlsb),
            ("an .xlsx as Xls", SheetKind::Xls, &workbook),
            ("an .xls as Xlsx", SheetKind::Xlsx, &xls),
        ] {
            probe::reset();
            assert_eq!(Workbook::open(kind, bytes, &Limits::default()).err(), Some(OpenError::Unsupported), "{what}");
            assert_eq!(probe::bytes_to_readers(), 0, "{what}: no reader saw it");
        }
    }

    /// Lead ruling R-D1: bytes that cannot be read as a container at all (no zip or compound-file
    /// signature, or a broken one) are Damaged, for either kind, and no reader sees them.
    #[test]
    fn bytes_that_are_no_container_are_damaged() {
        let workbook = xlsx(&formatted());
        let garbage: Vec<u8> = (0u8..64).map(|i| i.wrapping_mul(37).wrapping_add(11)).collect();
        let csv = b"Quarter,Revenue\nQ1,120\nQ2,340\n".to_vec();
        // The package's first half: a zip signature, no central directory.
        let truncated = workbook[..workbook.len() / 2].to_vec();
        let mut zip_then_garbage = b"PK\x03\x04".to_vec();
        zip_then_garbage.extend_from_slice(&garbage);
        let mut ole_then_garbage = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
        ole_then_garbage.extend_from_slice(&garbage);
        for (what, bytes) in [
            ("64 garbage bytes", &garbage),
            ("CSV text", &csv),
            ("a truncated .xlsx", &truncated),
            ("a zip signature on garbage", &zip_then_garbage),
            ("a compound-file signature on garbage", &ole_then_garbage),
        ] {
            for kind in [SheetKind::Xlsx, SheetKind::Xls] {
                probe::reset();
                assert_eq!(Workbook::open(kind, bytes, &Limits::default()).err(), Some(OpenError::Damaged), "{what} as {kind:?}");
                assert_eq!(probe::bytes_to_readers(), 0, "{what} as {kind:?}: no reader saw it");
            }
        }
    }

    #[test]
    fn a_protected_or_damaged_workbook_says_so() {
        let workbook = xlsx(&formatted());
        let locked = wordcraft_docx::encrypt(&workbook, "a password").expect("the package encrypts");
        assert_eq!(Workbook::open(SheetKind::Xlsx, &locked, &Limits::default()).err(), Some(OpenError::Password), "an encrypted package");
        let kept: Vec<(String, Vec<u8>)> = entries(&workbook).into_iter().filter(|(n, _)| n != "xl/workbook.xml").collect();
        assert_eq!(kept.len() + 1, entries(&workbook).len(), "the workbook part was there to take out");
        let refs: Vec<(&str, &[u8])> = kept.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
        assert_eq!(Workbook::open(SheetKind::Xlsx, &zip_stored(&refs), &Limits::default()).err(), Some(OpenError::Damaged), "a package without its workbook part");
    }

    #[test]
    fn an_xls_from_the_corpus_opens_with_its_cells() {
        // corpus/EXPECTED.tsv: xls/SimpleWithColours.xls has 3 sheets.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus/xls/SimpleWithColours.xls");
        let bytes = std::fs::read(&path).expect("the corpus file");
        let book = Workbook::open(SheetKind::Xls, &bytes, &Limits::default()).unwrap_or_else(|e| panic!("the .xls opens: {e}"));
        let info = book.info();
        assert_eq!(info.sheets.len(), 3, "EXPECTED.tsv's sheet count");
        let first = &info.sheets[0];
        let block = book.cells(0, whole(first.rows, first.cols));
        assert!(!block.cells.is_empty(), "the first sheet has cells");
        assert_eq!(cell(&block, 0, 0).text, "I'm plain", "A1 of the first sheet");
        assert!(block.anchors.is_empty(), "the .xls reader reads no drawings");
    }
}
