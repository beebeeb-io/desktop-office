//! The corpus count: real Office files (`corpus/`, each with a stated origin in `SOURCES.tsv`) opened and
//! rendered through the V1-C API. It decides whether an Office type ships in 0.9.0 (decision 2482 Q7: a type
//! ships only when at least 90 % of its corpus pages render without a missing element).
//!
//! Run at the checkpoint, never in the normal run:
//!   cargo test --locked -- --ignored corpus_counts --nocapture
//!
//! Output, per file: `path status pages_expected pages_got missing_content` with status one of
//! `ok|damaged|too-large|password|unsupported|timeout`; then two truth lines per type.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use beebeeb_office::{Deck, DocKind, Document, Limits, OpenError, SheetKind, Workbook};

/// The per-file bound: the open limit of the viewer contract.
const PER_FILE: Duration = Duration::from_secs(30);

const TYPES: [&str; 5] = ["docx", "doc", "xlsx", "xls", "pptx"];

#[derive(Clone, Debug)]
enum Status {
    Ok,
    Damaged,
    TooLarge,
    Password,
    /// Sheets and slides are stubs until V3: counted as unsupported-yet, never as a pass or a failure.
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

#[derive(Clone, Debug)]
struct FileResult {
    status: Status,
    pages_got: u32,
    /// Pages that rendered AND had `missing_content == false`.
    pages_clean: u32,
    /// Pages that rendered at all (clean or not).
    pages_rendered: u32,
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("corpus")
}

fn status_of(e: &OpenError) -> Status {
    match e {
        OpenError::Password => Status::Password,
        OpenError::TooLarge(_) => Status::TooLarge,
        OpenError::Unsupported => Status::Unsupported,
        _ => Status::Damaged,
    }
}

/// Pixel size of a page at `scale`. The API asks for a buffer of exactly `w*h*4`; the rounding is the
/// renderer's, so the candidates are tried in turn and the first one it does not call a size error is used.
fn render_one(doc: &Document, page: u32, size_pt: (f32, f32), scale: f32) -> Option<bool> {
    let w = size_pt.0 * scale;
    let h = size_pt.1 * scale;
    let candidates = [
        (w.round() as usize, h.round() as usize),
        (w.ceil() as usize, h.ceil() as usize),
        (w.floor() as usize, h.floor() as usize),
    ];
    for (pw, ph) in candidates {
        if pw == 0 || ph == 0 || pw > 4096 || ph > 4096 {
            continue;
        }
        let mut out = vec![0u8; pw * ph * 4];
        match doc.render_page(page, scale, None, &mut out) {
            Ok(info) => return Some(info.missing_content),
            Err(beebeeb_office::RenderError::BufferSize) => continue,
            Err(_) => return None,
        }
    }
    None
}

/// Opens one file and renders every page. Runs on a worker thread so a hang is cut at `PER_FILE`.
fn run_file(kind: &'static str, bytes: Vec<u8>) -> FileResult {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let limits = Limits::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match kind {
            "docx" | "doc" => {
                let k = if kind == "docx" { DocKind::Docx } else { DocKind::Doc };
                match Document::open(k, &bytes, &limits) {
                    Err(e) => FileResult { status: status_of(&e), pages_got: 0, pages_clean: 0, pages_rendered: 0 },
                    Ok(doc) => {
                        let info = doc.info();
                        let (mut clean, mut rendered) = (0u32, 0u32);
                        for p in 0..info.pages {
                            let size = info.sizes_pt.get(p as usize).copied().unwrap_or((612.0, 792.0));
                            if let Some(missing) = render_one(&doc, p, size, 1.0) {
                                rendered += 1;
                                if !missing {
                                    clean += 1;
                                }
                            }
                        }
                        FileResult { status: Status::Ok, pages_got: info.pages, pages_clean: clean, pages_rendered: rendered }
                    }
                }
            }
            "xlsx" | "xls" => {
                let k = if kind == "xlsx" { SheetKind::Xlsx } else { SheetKind::Xls };
                let status = match Workbook::open(k, &bytes, &limits) {
                    Err(e) => status_of(&e),
                    // V3-2 fills the sheet body; until then an Ok here is still not counted as a render.
                    Ok(_) => Status::Unsupported,
                };
                FileResult { status, pages_got: 0, pages_clean: 0, pages_rendered: 0 }
            }
            _ => {
                let status = match Deck::open(&bytes, &limits) {
                    Err(e) => status_of(&e),
                    Ok(_) => Status::Unsupported,
                };
                FileResult { status, pages_got: 0, pages_clean: 0, pages_rendered: 0 }
            }
        }));
        let _ = tx.send(result.unwrap_or(FileResult { status: Status::Damaged, pages_got: 0, pages_clean: 0, pages_rendered: 0 }));
    });
    rx.recv_timeout(PER_FILE)
        .unwrap_or(FileResult { status: Status::Timeout, pages_got: 0, pages_clean: 0, pages_rendered: 0 })
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

    println!("corpus: GAP - missing_content is the renderer's own flag, so a wrong but complete page passes it; O-CP3 adds a LOOK sample (>= 10 pages per type, fixed seed) before the lead reads these rates.");
    println!("corpus: line = path status pages_expected pages_got missing_content (pages without the flag / pages rendered)");

    for kind in TYPES {
        let (mut files, mut ok, mut match_n, mut no_missing, mut unsup) = (0u32, 0u32, 0u32, 0u32, 0u32);
        let (mut pages_expected_total, mut pages_rendered_total, mut pages_clean_total) = (0u32, 0u32, 0u32);
        let prefix = format!("{kind}/");
        for (path, exp) in expected.iter().filter(|(p, _)| p.starts_with(&prefix)) {
            files += 1;
            let bytes = std::fs::read(dir.join(path)).unwrap_or_else(|e| panic!("read {path}: {e}"));
            let r = run_file(kind, bytes);
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
            // Page denominators count only evaluable files (docs); sheets/slides are unsupported-yet.
            if matches!(kind, "docx" | "doc") {
                // a file that fails to open counts all its expected pages as failed; unknown counts what it has
                // (and 1 page when it failed to open and does not say)
                let denom = match (is_ok, exp_n) {
                    (true, Some(n)) => n.max(r.pages_got),
                    (true, None) => r.pages_got,
                    (false, Some(n)) => n,
                    (false, None) => 1,
                };
                pages_expected_total += denom;
                pages_rendered_total += r.pages_rendered;
                pages_clean_total += r.pages_clean;
            }
            println!(
                "{path} {} {} {} {}",
                r.status.word(),
                exp,
                r.pages_got,
                if is_ok { format!("{}/{}", r.pages_clean, r.pages_rendered) } else { "-".to_string() },
            );
        }
        if matches!(kind, "docx" | "doc") {
            println!(
                "corpus {kind}: files {files}, ok {ok}, pages-match {match_n}, no-missing {no_missing}, file-rate {} %",
                percent(no_missing, files)
            );
            println!(
                "corpus {kind}: pages {pages_expected_total}, rendered {pages_rendered_total}, no-missing {pages_clean_total}, page-rate {} %",
                percent(pages_clean_total, pages_expected_total)
            );
        } else {
            // Not evaluable until V3: counted as unsupported-yet, never a pass or a failure.
            println!(
                "corpus {kind}: files {files}, ok {ok}, pages-match {match_n}, no-missing {no_missing}, file-rate n/a (unsupported-yet {unsup} of {files})"
            );
            println!("corpus {kind}: pages 0, rendered 0, no-missing 0, page-rate n/a (unsupported-yet)");
        }
    }
}
