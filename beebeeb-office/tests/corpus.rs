//! The corpus count: real Office files (`corpus/`, each with a stated origin in `SOURCES.tsv`) opened and
//! rendered through the viewer API ([`Document`], [`Workbook`], [`Deck`]). It decides whether an Office type
//! ships in 0.9.0 (decision 2482 Q7: a type ships only when at least 90 % of its corpus pages render without a
//! missing element). A "page" is a page of a document, a sheet of a workbook, a slide of a presentation.
//!
//! Run at the checkpoint, never in the normal run:
//!   cargo test --locked -- --ignored corpus_counts --nocapture
//! With `BB_OUT` set, every slide and every sheet anchor (chart, picture) rendered is also written there as a
//! PNG (`corpus-<type>_<file>-<n>.png`, `corpus-<type>_<file>-s<sheet>-a<anchor>.png`) for the LOOK.
//!
//! Output, per file: `path status pages_expected pages_got missing_content` with status one of
//! `ok|damaged|too-large|password|unsupported|timeout`; then three truth lines per type. The test fails on a
//! harness error only (a file without a row, a row without a file), never on a rate: the rate is the lead's.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use beebeeb_office::{CellRange, Deck, DocKind, Document, Limits, OpenError, SheetKind, Workbook};
use common::save_png;

/// The per-file bound: the open limit of the viewer contract.
const PER_FILE: Duration = Duration::from_secs(30);

const TYPES: [&str; 5] = ["docx", "doc", "xlsx", "xls", "pptx"];

/// A sheet's cells are asked in blocks of at most this many rows and columns.
const BLOCK_ROWS: u32 = 100;
const BLOCK_COLS: u32 = 50;

/// The page rate (per cent) at or above which a type ships (decision 2482 Q7).
const SHIP_RATE: f64 = 90.0;

#[derive(Clone, Copy, Debug)]
enum Status {
    Ok,
    Damaged,
    TooLarge,
    Password,
    /// Not shown by this build: an older format (Word 6.0/95, `.ppt`), or a kind whose reader is still a
    /// stub on this branch. Its expected pages count as not rendered.
    Unsupported,
    Timeout,
}

impl Status {
    fn word(&self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Damaged => "damaged",
            Status::TooLarge => "too-large",
            Status::Password => "password",
            Status::Unsupported => "unsupported",
            Status::Timeout => "timeout",
        }
    }
}

/// A raster kept for `BB_OUT`: its name inside the file, its size and its RGBA8.
struct Png {
    name: String,
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

struct FileResult {
    status: Status,
    pages_got: u32,
    /// Pages that rendered AND had `missing_content == false`.
    pages_clean: u32,
    /// Pages that rendered at all (clean or not).
    pages_rendered: u32,
    /// Rasters for `BB_OUT` (only when it is set).
    pngs: Vec<Png>,
}

impl FileResult {
    fn failed(status: Status) -> FileResult {
        FileResult { status, pages_got: 0, pages_clean: 0, pages_rendered: 0, pngs: Vec::new() }
    }

    fn opened(pages: u32) -> FileResult {
        FileResult { pages_got: pages, ..FileResult::failed(Status::Ok) }
    }

    /// Count one page: `None` when it was refused, else whether something on it was missing.
    fn page(&mut self, missing: Option<bool>) {
        if let Some(missing) = missing {
            self.pages_rendered += 1;
            if !missing {
                self.pages_clean += 1;
            }
        }
    }
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("corpus")
}

fn status_of(e: &OpenError) -> Status {
    match e {
        OpenError::Password => Status::Password,
        OpenError::TooLarge(_) => Status::TooLarge,
        OpenError::Unsupported => Status::Unsupported,
        OpenError::Damaged => Status::Damaged,
    }
}

/// Renders one page at `scale` into a buffer sized by `Document::render_size`, the crate's one rule for
/// page pixels (`ceil(pt * scale)`). `Some(missing_content)` when it rendered, `None` when it was refused.
fn render_page(doc: &Document, page: u32, scale: f32) -> Option<bool> {
    let (w, h) = doc.render_size(page, scale, None).ok()?;
    let mut out = vec![0u8; w as usize * h as usize * 4];
    doc.render_page(page, scale, None, &mut out).ok().map(|info| info.missing_content)
}

/// Renders slide `n` at 1.0 into a buffer sized by `Deck::render_size`: `Some(missing_content)` when it
/// rendered, `None` when it was refused. The raster is kept for `BB_OUT` when `dump`.
fn render_slide(deck: &Deck, n: u32, dump: bool, pngs: &mut Vec<Png>) -> Option<bool> {
    let (w, h) = deck.render_size(n, 1.0).ok()?;
    let mut out = vec![0u8; w as usize * h as usize * 4];
    let info = deck.render_slide(n, 1.0, &mut out).ok()?;
    if dump {
        pngs.push(Png { name: n.to_string(), w, h, rgba: out });
    }
    Some(info.missing_content)
}

/// One sheet: its cells over the used range (`rows` x `cols`, at least one cell), asked in blocks of
/// at most [`BLOCK_ROWS`] x [`BLOCK_COLS`], then every anchor those blocks list, rendered at 1.0.
/// `Some(missing)` when every anchor rendered (`missing`: a block or an anchor said content is
/// missing), `None` when an anchor was refused.
fn render_sheet(wb: &Workbook, sheet: u32, rows: u32, cols: u32, dump: bool, pngs: &mut Vec<Png>) -> Option<bool> {
    let (rows, cols) = (rows.max(1), cols.max(1));
    let mut missing = false;
    let mut anchors: Vec<u32> = Vec::new();
    let mut row = 0u32;
    while row < rows {
        let mut col = 0u32;
        while col < cols {
            let range = CellRange { row, col, rows: BLOCK_ROWS.min(rows - row), cols: BLOCK_COLS.min(cols - col) };
            let block = wb.cells(sheet, range);
            missing |= block.missing_content;
            for a in &block.anchors {
                if !anchors.contains(&a.id) {
                    anchors.push(a.id);
                }
            }
            col = col.saturating_add(BLOCK_COLS);
        }
        row = row.saturating_add(BLOCK_ROWS);
    }
    for id in anchors {
        let (w, h) = wb.anchor_size(sheet, id, 1.0).ok()?;
        let mut out = vec![0u8; w as usize * h as usize * 4];
        let info = wb.render_anchor(sheet, id, 1.0, &mut out).ok()?;
        missing |= info.missing_content;
        if dump {
            pngs.push(Png { name: format!("s{sheet}-a{id}"), w, h, rgba: out });
        }
    }
    Some(missing)
}

/// Opens one file and renders every page (sheet, slide). Runs on a worker thread so a hang is cut at
/// `PER_FILE`.
fn run_file(kind: &'static str, bytes: Vec<u8>, dump: bool) -> FileResult {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let limits = Limits::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match kind {
            "docx" | "doc" => {
                let k = if kind == "docx" { DocKind::Docx } else { DocKind::Doc };
                match Document::open(k, &bytes, &limits) {
                    Err(e) => FileResult::failed(status_of(&e)),
                    Ok(doc) => {
                        let pages = doc.info().pages;
                        let mut r = FileResult::opened(pages);
                        for p in 0..pages {
                            r.page(render_page(&doc, p, 1.0));
                        }
                        r
                    }
                }
            }
            "xlsx" | "xls" => {
                let k = if kind == "xlsx" { SheetKind::Xlsx } else { SheetKind::Xls };
                match Workbook::open(k, &bytes, &limits) {
                    Err(e) => FileResult::failed(status_of(&e)),
                    Ok(wb) => {
                        let sheets = wb.info().sheets;
                        let mut r = FileResult::opened(u32::try_from(sheets.len()).unwrap_or(u32::MAX));
                        for (i, sheet) in sheets.iter().enumerate() {
                            let mut pngs = std::mem::take(&mut r.pngs);
                            let page = render_sheet(&wb, i as u32, sheet.rows, sheet.cols, dump, &mut pngs);
                            r.pngs = pngs;
                            r.page(page);
                        }
                        r
                    }
                }
            }
            _ => match Deck::open(&bytes, &limits) {
                Err(e) => FileResult::failed(status_of(&e)),
                Ok(deck) => {
                    let slides = deck.info().slides;
                    let mut r = FileResult::opened(slides);
                    for n in 0..slides {
                        let mut pngs = std::mem::take(&mut r.pngs);
                        let page = render_slide(&deck, n, dump, &mut pngs);
                        r.pngs = pngs;
                        r.page(page);
                    }
                    r
                }
            },
        }));
        let _ = tx.send(result.unwrap_or_else(|_| FileResult::failed(Status::Damaged)));
    });
    rx.recv_timeout(PER_FILE).unwrap_or_else(|_| FileResult::failed(Status::Timeout))
}

fn percent(num: u32, den: u32) -> String {
    if den == 0 { "n/a".to_string() } else { format!("{:.1}", f64::from(num) * 100.0 / f64::from(den)) }
}

#[test]
#[ignore = "corpus: run at the checkpoint"]
fn corpus_counts() {
    let dir = corpus_dir();
    let expected_tsv = std::fs::read_to_string(dir.join("EXPECTED.tsv")).expect("corpus/EXPECTED.tsv");
    let expected: BTreeMap<String, String> = expected_tsv
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once('\t'))
        .map(|(p, c)| (p.to_string(), c.trim().to_string()))
        .collect();

    // The instrument must see every file: a file on disk without a row, or a row without a file, is a red.
    let mut on_disk = 0usize;
    for t in TYPES {
        if let Ok(rd) = std::fs::read_dir(dir.join(t)) {
            on_disk += rd.filter_map(Result::ok).filter(|e| e.path().is_file()).count();
        }
    }
    assert_eq!(on_disk, expected.len(), "corpus files on disk vs rows in EXPECTED.tsv");

    let dump = std::env::var_os("BB_OUT").is_some();
    println!("corpus: GAP - missing_content is the renderer's own flag, so a wrong but complete page passes it; O-CP3 adds a LOOK sample (>= 10 pages per type, fixed seed) before the lead reads these rates.");
    println!("corpus: line = path status pages_expected pages_got missing_content (pages without the flag / pages rendered)");
    println!("corpus: a page is a page of a document, a sheet of a workbook, a slide of a presentation; PNGs to BB_OUT: {dump}");

    for kind in TYPES {
        let (mut files, mut ok, mut match_n, mut no_missing, mut unsup) = (0u32, 0u32, 0u32, 0u32, 0u32);
        let (mut pages_expected_total, mut pages_rendered_total, mut pages_clean_total) = (0u32, 0u32, 0u32);
        let prefix = format!("{kind}/");
        for (path, exp) in expected.iter().filter(|(p, _)| p.starts_with(&prefix)) {
            files += 1;
            let bytes = std::fs::read(dir.join(path)).unwrap_or_else(|e| panic!("read {path}: {e}"));
            let r = run_file(kind, bytes, dump);
            let exp_n: Option<u32> = exp.parse().ok();
            let is_ok = matches!(r.status, Status::Ok);
            if matches!(r.status, Status::Unsupported) {
                unsup += 1;
            }
            if is_ok {
                ok += 1;
                if exp_n == Some(r.pages_got) {
                    match_n += 1;
                }
                if r.pages_rendered == r.pages_got && r.pages_clean == r.pages_got {
                    no_missing += 1;
                }
            }
            // A file that fails to open counts all its expected pages as failed; unknown counts what it has
            // (and 1 page when it failed to open and does not say).
            let denom = match (is_ok, exp_n) {
                (true, Some(n)) => n.max(r.pages_got),
                (true, None) => r.pages_got,
                (false, Some(n)) => n,
                (false, None) => 1,
            };
            pages_expected_total += denom;
            pages_rendered_total += r.pages_rendered;
            pages_clean_total += r.pages_clean;
            println!(
                "{path} {} {} {} {}",
                r.status.word(),
                exp,
                r.pages_got,
                if is_ok { format!("{}/{}", r.pages_clean, r.pages_rendered) } else { "-".to_string() },
            );
            let stem = path.replace('/', "_");
            for png in &r.pngs {
                save_png(&format!("corpus-{stem}-{}", png.name), png.w, png.h, &png.rgba);
            }
        }
        println!(
            "corpus {kind}: files {files}, ok {ok}, pages-match {match_n}, no-missing {no_missing}, file-rate {} %",
            percent(no_missing, files)
        );
        println!(
            "corpus {kind}: pages {pages_expected_total}, rendered {pages_rendered_total}, no-missing {pages_clean_total}, page-rate {} %",
            percent(pages_clean_total, pages_expected_total)
        );
        let rate = if pages_expected_total == 0 { 0.0 } else { f64::from(pages_clean_total) * 100.0 / f64::from(pages_expected_total) };
        println!(
            "corpus {kind}: ships {} (page-rate {} % against {SHIP_RATE:.0} %; unsupported {unsup} of {files} files)",
            if rate >= SHIP_RATE { "yes" } else { "no" },
            percent(pages_clean_total, pages_expected_total)
        );
    }
}
