//! Nothing a workbook names outside itself is reached: a web-service formula makes no connection
//! to a loopback listener, and a link to another workbook reads no local file. Each case runs its
//! positive control first, so a case cannot pass because the instrument does not count.
//! Filter: `sheets` (the file name) or the test names.

mod common;

use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use beebeeb_office::{CellRange, Limits, SheetKind, Workbook};
use common::Listener;
use gridcraft_core::{CellRef, Value};
use gridcraft_model as gm;

/// A workbook of one sheet whose A1..An hold `formulas`, none with a cached value, and B1 = 7.
fn with_formulas(formulas: &[String]) -> Vec<u8> {
    let mut wb = gm::Workbook::new();
    let sh = Arc::make_mut(&mut wb.sheets[0]);
    for (i, f) in formulas.iter().enumerate() {
        sh.set_cell(CellRef::new(i as u32, 0), gm::Cell::formula(gm::Formula::new(f)));
    }
    sh.set_cell(CellRef::new(0, 1), gm::Cell::value(Value::Number(7.0)));
    gridcraft_xlsx::write_xlsx(&wb).expect("the writer writes the workbook")
}

fn open(bytes: &[u8]) -> Workbook {
    Workbook::open(SheetKind::Xlsx, bytes, &Limits::default()).unwrap_or_else(|e| panic!("the workbook opens: {e}"))
}

/// In a module of their own so the filter `sheets` selects every test here by its name.
mod sheets {
    use super::*;

    #[test]
    fn a_webservice_formula_fetches_nothing() {
        let listener = Listener::bind("127.0.0.1:0").expect("listen on loopback");
        // The instrument counts: one connection from here is seen, then the count starts again.
        drop(TcpStream::connect(listener.addr).expect("connect to the listener"));
        assert_eq!(listener.drain(), 1, "the listener counts a connection");

        let url = format!("http://{}/x", listener.addr);
        let book = open(&with_formulas(&[format!("=WEBSERVICE(\"{url}\")"), format!("=FILTERXML(WEBSERVICE(\"{url}\"),\"//a\")")]));
        let block = book.cells(0, CellRange { row: 0, col: 0, rows: 2, cols: 2 });
        assert_eq!(listener.drain(), 0, "no connection to {url}");
        for row in 0..2 {
            let c = block.cells.iter().find(|c| c.row == row && c.col == 0).expect("the formula cell");
            assert!(c.text.starts_with('#'), "row {row}: the engine's error value, not fetched content: {:?}", c.text);
            assert_eq!(c.value, None, "row {row}: no number");
        }
        assert!(block.missing_content, "the block says content is not shown");
        // Control of the flag: a block away from the formulas does not say so.
        assert!(!book.cells(0, CellRange { row: 0, col: 1, rows: 1, cols: 1 }).missing_content, "B1 alone is shown as the file means it");
    }

    /// The canary's last access time, after setting it well before its last change: on a file system
    /// mounted `relatime` (the default) the next read then moves it.
    fn arm(path: &std::path::Path) -> SystemTime {
        let old = SystemTime::now() - Duration::from_secs(10 * 24 * 3600);
        let f = std::fs::File::options().write(true).open(path).expect("open the canary to set its times");
        f.set_times(std::fs::FileTimes::new().set_accessed(old)).expect("set the canary's access time");
        drop(f);
        std::fs::metadata(path).and_then(|m| m.accessed()).expect("the canary's access time")
    }

    fn accessed(path: &std::path::Path) -> SystemTime {
        std::fs::metadata(path).and_then(|m| m.accessed()).expect("the canary's access time")
    }

    #[test]
    fn an_external_workbook_link_reads_nothing() {
        let dir = std::env::temp_dir().join(format!("bb-office-sheets-canary-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let canary = dir.join("Book.xlsx");
        std::fs::write(&canary, b"not a workbook: any read of this file moves its access time").expect("write the canary");

        // Control: a read moves the access time on this file system.
        let before = arm(&canary);
        let _ = std::fs::read(&canary).expect("read the canary");
        assert_ne!(accessed(&canary), before, "the instrument sees a read");

        let path = canary.display().to_string();
        let file_url = format!("file://{path}");
        let dir_text = dir.display().to_string();
        let formulas = [
            format!("='{file_url}'!A1"),
            format!("='{dir_text}/[Book.xlsx]Sheet1'!A1"),
            "=[1]Sheet1!A1".to_string(),
            "=SUM([1]Sheet1!A1:A3)".to_string(),
        ];
        let before = arm(&canary);
        let book = open(&with_formulas(&formulas));
        let block = book.cells(0, CellRange { row: 0, col: 0, rows: formulas.len() as u32, cols: 2 });
        assert_eq!(accessed(&canary), before, "the canary was not read");
        for (row, f) in formulas.iter().enumerate() {
            let c = block.cells.iter().find(|c| c.row == row as u32 && c.col == 0).unwrap_or_else(|| panic!("row {row} ({f}): the cell is listed"));
            assert_eq!((c.text.as_str(), c.value), ("", None), "row {row} ({f}): shown empty");
            assert!(c.formula.is_some(), "row {row}: its formula is still there to show");
        }
        assert!(block.missing_content, "the block says content is not shown");
        assert!(!book.cells(0, CellRange { row: 0, col: 1, rows: 1, cols: 1 }).missing_content, "B1 alone is shown as the file means it");
        std::fs::remove_dir_all(&dir).ok();
    }
}
