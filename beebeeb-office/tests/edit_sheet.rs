//! Workbook editing (`EditDoc` over GridCraft). Saving loses data when it is wrong, so these check the file
//! that comes out: set a value and a formula, change an input, save, reopen with a fresh `EditDoc`: values,
//! formulas and every other cell are equal; undo and redo; sort and filter survive the save. Fixtures: books
//! written in code and every `.xlsx` in `corpus/xlsx`. Filter: `edit_sheet`.
#![cfg(feature = "edit")]

use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;

use beebeeb_office::edit::{EditDoc, EditError, EditKind, HitMode, Mark, NumberFormat, Op, SaveError};
use beebeeb_office::{CellRange, Limits};
use gridcraft_core::Value;
use gridcraft_model as gm;

fn book() -> Vec<u8> {
    let mut sh = gm::Sheet::new("Data");
    let mut put = |r: u32, c: u32, cell: gm::Cell| sh.set_cell(gridcraft_core::CellRef::new(r, c), cell);
    put(0, 0, gm::Cell::value(Value::Text("Name".into())));
    put(0, 1, gm::Cell::value(Value::Text("Qty".into())));
    for (i, (n, q)) in [("pear", 30.0), ("apple", 10.0), ("fig", 20.0)].into_iter().enumerate() {
        put(i as u32 + 1, 0, gm::Cell::value(Value::Text(n.into())));
        put(i as u32 + 1, 1, gm::Cell::value(Value::Number(q)));
    }
    let mut wb = gm::Workbook::new();
    wb.sheets = vec![Arc::new(sh)];
    gridcraft_xlsx::write_xlsx(&wb).expect("writes")
}

fn open(bytes: &[u8]) -> EditDoc {
    EditDoc::open(EditKind::Xlsx, bytes, &Limits::default()).expect("opens")
}

fn set(doc: &mut EditDoc, row: u32, col: u32, text: &str) -> beebeeb_office::edit::Change {
    doc.apply(&Op::SetCell { sheet: 0, row, col, text: text.into() }).expect("set")
}

fn select(doc: &mut EditDoc, row: u32, col: u32, rows: u32, cols: u32) {
    doc.apply(&Op::SelectCells { sheet: 0, range: CellRange { row, col, rows, cols } }).expect("select");
}

/// Every non-empty cell of sheet `s` as (row, col, shown text, formula).
fn dump(doc: &EditDoc, s: u32) -> Vec<(u32, u32, String, Option<String>)> {
    let info = doc.sheets().expect("sheets");
    let sh = &info.sheets[s as usize];
    let b = doc.cells(s, CellRange { row: 0, col: 0, rows: sh.rows.max(1), cols: sh.cols.max(1) }).expect("cells");
    b.cells.into_iter().map(|c| (c.row, c.col, c.text, c.formula)).collect()
}

fn text(doc: &EditDoc, row: u32, col: u32) -> String {
    dump(doc, 0).into_iter().find(|c| c.0 == row && c.1 == col).map(|c| c.2).unwrap_or_default()
}

#[test]
fn edit_sheet_value_formula_and_input_change_survive_save_and_reopen() {
    let mut doc = open(&book());
    let before = dump(&doc, 0);
    set(&mut doc, 4, 1, "5");
    let ch = set(&mut doc, 5, 1, "=SUM(B2:B5)");
    assert_eq!(text(&doc, 5, 1), "65");
    assert!(ch.cells.iter().any(|(s, r)| *s == 0 && r.row == 5 && r.col == 1));
    // Change an input: the formula recalculates and is listed as changed.
    let ch = set(&mut doc, 4, 1, "15");
    assert_eq!(text(&doc, 5, 1), "75");
    assert!(ch.cells.iter().any(|(_, r)| r.row == 5 && r.col == 1), "{:?}", ch.cells);
    assert!(ch.cells.iter().any(|(_, r)| r.row == 4 && r.col == 1));
    assert!(ch.revision >= 3);
    let saved = doc.save().expect("saves");
    let again = open(&saved);
    let mut expected = before.clone();
    expected.push((4, 1, "15".into(), None));
    expected.push((5, 1, "75".into(), Some("=SUM(B2:B5)".into())));
    expected.sort_by_key(|c| (c.0, c.1));
    let mut got = dump(&again, 0);
    got.sort_by_key(|c| (c.0, c.1));
    assert_eq!(got, expected);
}

#[test]
fn edit_sheet_undo_and_redo_restore() {
    let mut doc = open(&book());
    let before = dump(&doc, 0);
    set(&mut doc, 1, 1, "99");
    let ch = set(&mut doc, 2, 1, "98");
    assert!(ch.can_undo && !ch.can_redo);
    let ch = doc.apply(&Op::Undo).unwrap();
    assert!(ch.can_redo);
    assert_eq!(text(&doc, 2, 1), "10");
    assert_eq!(text(&doc, 1, 1), "99");
    doc.apply(&Op::Undo).unwrap();
    assert_eq!(dump(&doc, 0), before);
    assert!(!doc.apply(&Op::Undo).unwrap().can_undo);
    doc.apply(&Op::Redo).unwrap();
    doc.apply(&Op::Redo).unwrap();
    assert_eq!(text(&doc, 2, 1), "98");
    assert!(!doc.apply(&Op::Redo).unwrap().can_redo);
}

#[test]
fn edit_sheet_format_ops_and_marks() {
    let mut doc = open(&book());
    select(&mut doc, 1, 1, 3, 1);
    doc.apply(&Op::Mark { mark: Mark::Bold, on: true }).unwrap();
    let ch = doc.apply(&Op::Mark { mark: Mark::Bold, on: true }).unwrap();
    assert!(ch.marks.bold, "an explicit `on` does not toggle");
    doc.apply(&Op::NumberFormat(NumberFormat::Percent)).unwrap();
    assert_eq!(text(&doc, 1, 1), "3000%");
    doc.apply(&Op::NumberFormat(NumberFormat::General)).unwrap();
    doc.apply(&Op::FillColour(Some(beebeeb_office::edit::Rgb { r: 255, g: 255, b: 0 }))).unwrap();
    doc.apply(&Op::Align(beebeeb_office::edit::TextAlign::Center)).unwrap();
    doc.apply(&Op::RenameSheet { sheet: 0, name: "Fruit".into() }).unwrap();
    assert_eq!(doc.sheets().unwrap().sheets[0].name, "Fruit");
    let again = open(&doc.save().unwrap());
    assert_eq!(again.sheets().unwrap().sheets[0].name, "Fruit");
    let b = again.cells(0, CellRange { row: 1, col: 1, rows: 1, cols: 1 }).unwrap();
    assert!(b.cells[0].bold);
    // Text ops and a click are not for a grid.
    assert_eq!(doc.apply(&Op::InsertText("x".into())).unwrap_err(), EditError::NotApplicable);
    assert_eq!(doc.hit(0, 1.0, 1.0, HitMode::Place).unwrap_err(), EditError::NotApplicable);
    assert_eq!(doc.apply(&Op::SetCell { sheet: 9, row: 0, col: 0, text: "x".into() }).unwrap_err(), EditError::NotApplicable);
}

#[test]
fn edit_sheet_autosum() {
    let mut doc = open(&book());
    select(&mut doc, 4, 1, 1, 1);
    doc.apply(&Op::AutoSum).unwrap();
    assert_eq!(text(&doc, 4, 1), "60");
}

fn sheet_xml(bytes: &[u8]) -> String {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut s = String::new();
    z.by_name("xl/worksheets/sheet1.xml").unwrap().read_to_string(&mut s).unwrap();
    s
}

#[test]
fn edit_sheet_sort_and_filter_survive_the_save() {
    let mut doc = open(&book());
    select(&mut doc, 1, 0, 3, 2);
    let ch = doc.apply(&Op::Sort { ascending: true }).unwrap();
    assert!(!ch.cells.is_empty());
    assert_eq!((text(&doc, 1, 0), text(&doc, 2, 0), text(&doc, 3, 0)), ("apple".into(), "fig".into(), "pear".into()));
    assert_eq!(text(&doc, 1, 1), "10");
    select(&mut doc, 0, 0, 4, 2);
    doc.apply(&Op::Filter).unwrap();
    let saved = doc.save().unwrap();
    assert!(sheet_xml(&saved).contains("<autoFilter"));
    let again = open(&saved);
    assert_eq!((text(&again, 1, 0), text(&again, 3, 0)), ("apple".into(), "pear".into()));
    assert_eq!(text(&again, 3, 1), "30");
}

#[test]
fn edit_sheet_make_table_survives_the_save() {
    let mut doc = open(&book());
    select(&mut doc, 0, 0, 4, 2);
    doc.apply(&Op::MakeTable).unwrap();
    let saved = doc.save().unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(saved.as_slice())).unwrap();
    assert!(z.file_names().any(|n| n.starts_with("xl/tables/")));
}

#[test]
fn edit_sheet_xls_opens_but_never_saves() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus/xls");
    let mut tried = 0;
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let Ok(bytes) = std::fs::read(e.path()) else { continue };
        let Ok(mut doc) = EditDoc::open(EditKind::Xlsx, &bytes, &Limits::default()) else { continue };
        assert_eq!(doc.save().unwrap_err(), SaveError::Unsupported, "{:?}", e.path());
        let _ = doc.apply(&Op::SetCell { sheet: 0, row: 0, col: 0, text: "1".into() });
        tried += 1;
        if tried == 3 {
            break;
        }
    }
    assert!(tried > 0, "at least one .xls opens");
}

#[test]
fn edit_sheet_corpus_edit_save_reopen_keeps_every_other_cell() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus/xlsx");
    let (mut saved, mut refused) = (0, 0);
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("xlsx") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let Ok(mut doc) = EditDoc::open(EditKind::Xlsx, &bytes, &Limits::default()) else { continue };
        let info = doc.sheets().unwrap();
        let (rows, cols) = (info.sheets[0].rows, info.sheets[0].cols);
        let before = dump(&doc, 0);
        // A cell clear of everything the sheet holds.
        let (r, c) = (rows + 2, cols + 2);
        if doc.apply(&Op::SetCell { sheet: 0, row: r, col: c, text: "ZQX".into() }).is_err() {
            refused += 1;
            continue;
        }
        let Ok(out) = doc.save() else {
            refused += 1;
            continue;
        };
        let again = open(&out);
        let mut got = dump(&again, 0);
        let mut want = before;
        want.push((r, c, "ZQX".into(), None));
        got.sort_by_key(|x| (x.0, x.1));
        want.sort_by_key(|x| (x.0, x.1));
        assert_eq!(got.len(), want.len(), "{path:?}");
        for (g, w) in got.iter().zip(&want) {
            assert_eq!((g.0, g.1, &g.3), (w.0, w.1, &w.3), "{path:?}");
            assert_eq!(g.2, w.2, "{path:?} cell {}:{}", w.0, w.1);
        }
        saved += 1;
    }
    eprintln!("corpus: {saved} saved and reopened, {refused} refused");
    assert!(saved > 0);
}
