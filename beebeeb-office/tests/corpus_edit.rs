//! The corpus through OUR edit API: every docx/xlsx/pptx row of `corpus/SOURCES.tsv` (all producers; the
//! Microsoft Office rows are tagged `ms=yes`) is opened with `EditDoc`, three scripted edits are applied, saved,
//! and reopened with a fresh `EditDoc`. Measure only: nothing here fixes or asserts a file green; the red list
//! is the `FILE` lines with a `FAIL` or `false`.
//!
//! Ignored by default (it reads the whole `corpus/`). Run (pptx needs `--features edit-slides`):
//!   cargo test --locked --features edit-slides -- --ignored corpus_edit --nocapture
//! One `FILE` line per document and `SUMMARY` lines per kind go to stdout (and to `$CORPUS_OUT/report-edit.txt`
//! when set; the saved bytes to `$CORPUS_OUT/saved/` for `scripts/pkgdiff.py`). Only part names (digits folded),
//! counts, milliseconds and yes/no are printed, never document text.
//!
//! The three edits: Word: text at the start, text at the end, bold on the first line. Sheet: text, a number and a
//! formula in cells clear of everything the sheet holds. Slide: add a slide, duplicate the first, replace the
//! first slide's title text. Per file: opened, edit present, content equal elsewhere, parts lost, time to open
//! and to save.
#![cfg(feature = "edit")]

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

use beebeeb_office::edit::{EditDoc, EditKind, Mark, Motion, Op};
use beebeeb_office::{CellRange, Limits};

type Parts = BTreeMap<String, Vec<u8>>;

fn read_parts(bytes: &[u8]) -> Result<Parts, String> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut out = Parts::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| e.to_string())?;
        if f.is_dir() {
            continue;
        }
        let mut b = Vec::new();
        f.read_to_end(&mut b).map_err(|e| e.to_string())?;
        out.insert(f.name().to_string(), b);
    }
    Ok(out)
}

/// `ppt/slides/slide12.xml` -> `ppt/slides/slideN.xml`.
fn class(name: &str) -> String {
    let mut s = String::new();
    let mut last_digit = false;
    for c in name.chars() {
        if c.is_ascii_digit() {
            if !last_digit {
                s.push('N');
            }
            last_digit = true;
        } else {
            s.push(c);
            last_digit = false;
        }
    }
    s
}

fn short(e: &str) -> String {
    // Error texts name no document content; the first 60 characters, digits folded.
    class(&e.chars().take(60).collect::<String>())
}

fn ms(n: u128) -> String {
    n.to_string()
}

/// What one file measured. Strings are codes only.
#[derive(Default)]
struct Row {
    opened: bool,
    open_ms: u128,
    edits: String,         // "3/3" or "2/3(...)"
    edit_present: Option<bool>,
    elsewhere_equal: Option<bool>,
    saved: String,         // "ok" | "FAIL(code)"
    save_ms: u128,
    reopened: Option<bool>,
    parts: (usize, usize),
    lost: usize,
    added: usize,
    deficits: Vec<String>,
}

// ---------------------------------------------------------------- Word

/// The whole body as the engine copies it (Select All inside a table grows: repeat until it stops).
fn word_text(doc: &mut EditDoc) -> Result<String, String> {
    let e = |e: beebeeb_office::edit::EditError| format!("{e}");
    let mut t: Option<String> = None;
    for _ in 0..6 {
        doc.apply(&Op::SelectAll).map_err(e)?;
        let now = doc.copy().text;
        if t.as_ref() == Some(&now) {
            break;
        }
        t = Some(now);
    }
    doc.apply(&Op::Move { to: Motion::DocStart, extend: false }).map_err(e)?;
    Ok(t.unwrap_or_default())
}

fn word(bytes: &[u8], row: &mut Row, save_to: &mut Option<Vec<u8>>) -> Result<(), String> {
    let t = Instant::now();
    let mut doc = EditDoc::open(EditKind::Docx, bytes, &Limits::default()).map_err(|e| format!("open: {}", short(&format!("{e:?}"))))?;
    row.opened = true;
    row.open_ms = t.elapsed().as_millis();
    let before = word_text(&mut doc)?;
    let mut done = 0;
    let mut first_err = String::new();
    let steps: [&[Op]; 3] = [
        &[Op::Move { to: Motion::DocStart, extend: false }, Op::InsertText("ZQXstart ".into())],
        &[Op::Move { to: Motion::DocEnd, extend: false }, Op::InsertText("ZQXend".into())],
        &[Op::Move { to: Motion::DocStart, extend: false }, Op::Move { to: Motion::LineEnd, extend: true }, Op::Mark { mark: Mark::Bold, on: true }],
    ];
    for s in steps {
        match s.iter().try_for_each(|o| doc.apply(o).map(|_| ())) {
            Ok(()) => done += 1,
            Err(e) => {
                if first_err.is_empty() {
                    first_err = format!("{e}");
                }
            }
        }
    }
    row.edits = if done == 3 { "3/3".into() } else { format!("{done}/3({first_err})") };
    let edited = word_text(&mut doc)?;
    row.edit_present = Some(edited.contains("ZQXstart ") == (done >= 1) && edited.contains("ZQXend") == (done >= 2));
    let t = Instant::now();
    match doc.save() {
        Ok(out) => {
            row.save_ms = t.elapsed().as_millis();
            row.saved = "ok".into();
            match EditDoc::open(EditKind::Docx, &out, &Limits::default()) {
                Ok(mut again) => {
                    row.reopened = Some(true);
                    let after = word_text(&mut again)?;
                    let present = after.contains("ZQXstart ") == (done >= 1) && after.contains("ZQXend") == (done >= 2);
                    row.edit_present = Some(row.edit_present == Some(true) && present);
                    let stripped = after.replacen("ZQXstart ", "", 1);
                    let stripped = match stripped.rfind("ZQXend") {
                        Some(i) => format!("{}{}", &stripped[..i], &stripped[i + 6..]),
                        None => stripped,
                    };
                    row.elsewhere_equal = Some(stripped.trim_end() == before.trim_end());
                }
                Err(_) => row.reopened = Some(false),
            }
            *save_to = Some(out);
        }
        Err(e) => {
            row.save_ms = t.elapsed().as_millis();
            row.saved = format!("FAIL({e})");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- Sheet

type Cell = (u32, u32, u32, String, Option<String>); // sheet, row, col, shown text, formula

fn dump(doc: &EditDoc) -> Result<Vec<Cell>, String> {
    let info = doc.sheets().map_err(|e| format!("{e}"))?;
    let mut out = Vec::new();
    for (i, sh) in info.sheets.iter().enumerate() {
        let b = doc.cells(i as u32, CellRange { row: 0, col: 0, rows: sh.rows.max(1), cols: sh.cols.max(1) }).map_err(|e| format!("{e}"))?;
        out.extend(b.cells.into_iter().map(|c| (i as u32, c.row, c.col, c.text, c.formula)));
    }
    out.sort_by_key(|c| (c.0, c.1, c.2));
    Ok(out)
}

fn sheet(bytes: &[u8], row: &mut Row, save_to: &mut Option<Vec<u8>>) -> Result<(), String> {
    let t = Instant::now();
    let mut doc = EditDoc::open(EditKind::Xlsx, bytes, &Limits::default()).map_err(|e| format!("open: {}", short(&format!("{e:?}"))))?;
    row.opened = true;
    row.open_ms = t.elapsed().as_millis();
    let before = dump(&doc)?;
    let info = doc.sheets().map_err(|e| format!("{e}"))?;
    let (r, c) = (info.sheets[0].rows + 2, info.sheets[0].cols + 2);
    let mut done = 0;
    let mut first_err = String::new();
    let steps: [Vec<Op>; 3] = [
        vec![Op::SetCell { sheet: 0, row: r, col: c, text: "ZQX".into() }],
        vec![Op::SetCell { sheet: 0, row: r + 1, col: c, text: "42".into() }],
        vec![Op::SetCell { sheet: 0, row: r + 2, col: c, text: format!("=1+{}", 41) }, Op::SelectCells { sheet: 0, range: CellRange { row: r, col: c, rows: 1, cols: 1 } }, Op::Mark { mark: Mark::Bold, on: true }],
    ];
    for s in &steps {
        match s.iter().try_for_each(|o| doc.apply(o).map(|_| ())) {
            Ok(()) => done += 1,
            Err(e) => {
                if first_err.is_empty() {
                    first_err = format!("{e}");
                }
            }
        }
    }
    row.edits = if done == 3 { "3/3".into() } else { format!("{done}/3({first_err})") };
    let find = |d: &[Cell], rr: u32, cc: u32| d.iter().find(|x| x.0 == 0 && x.1 == rr && x.2 == cc).map(|x| x.3.clone());
    let present = |d: &[Cell]| {
        (done < 1 || find(d, r, c).as_deref() == Some("ZQX"))
            && (done < 2 || find(d, r + 1, c).as_deref() == Some("42"))
            && (done < 3 || find(d, r + 2, c).as_deref() == Some("42"))
    };
    row.edit_present = Some(present(&dump(&doc)?));
    let t = Instant::now();
    match doc.save() {
        Ok(out) => {
            row.save_ms = t.elapsed().as_millis();
            row.saved = "ok".into();
            match EditDoc::open(EditKind::Xlsx, &out, &Limits::default()) {
                Ok(again) => {
                    row.reopened = Some(true);
                    let after = dump(&again)?;
                    row.edit_present = Some(row.edit_present == Some(true) && present(&after));
                    // Everything but the cells we wrote is equal (position, shown text, formula).
                    let ours = |x: &Cell| x.0 == 0 && x.2 == c && (r..r + 3).contains(&x.1);
                    let a: Vec<&Cell> = before.iter().filter(|x| !ours(x)).collect();
                    let b: Vec<&Cell> = after.iter().filter(|x| !ours(x)).collect();
                    row.elsewhere_equal = Some(a == b);
                }
                Err(_) => row.reopened = Some(false),
            }
            *save_to = Some(out);
        }
        Err(e) => {
            row.save_ms = t.elapsed().as_millis();
            row.saved = format!("FAIL({e})");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- Slides

#[cfg(feature = "edit-slides")]
mod slides {
    use super::*;
    use beebeeb_office::edit::EditError;

    /// Every slide's text, shape by shape (empty shapes left out).
    fn texts(bytes: &[u8]) -> Result<Vec<Vec<String>>, String> {
        let (p, _) = deckcraft_pptx::import_checked(bytes).map_err(|e| short(&e.to_string()))?;
        Ok(p.slides.iter().map(|s| s.shapes.iter().filter_map(|sh| sh.text.as_ref().map(|t| t.text())).filter(|t| !t.is_empty()).collect()).collect())
    }

    /// The centre of the first slide's title placeholder, and its current text.
    fn title_of_first(bytes: &[u8]) -> Option<(f32, f32, String)> {
        let (p, _) = deckcraft_pptx::import_checked(bytes).ok()?;
        let sh = p.slides.first()?.shapes.iter().find(|s| s.ph.as_ref().is_some_and(|ph| ph.kind.is_title()))?;
        let sel = deckcraft_engine::Selection { slide: 0, ..Default::default() };
        let x = deckcraft_engine::cmd::xfrm_of(&p, &sel, sh);
        Some(((x.x + x.w / 2.0) as f32, (x.y + x.h / 2.0) as f32, sh.text.as_ref().map(|t| t.text()).unwrap_or_default()))
    }

    pub fn run(bytes: &[u8], row: &mut Row, save_to: &mut Option<Vec<u8>>) -> Result<(), String> {
        let t = Instant::now();
        let mut doc = EditDoc::open(EditKind::Pptx, bytes, &Limits::default()).map_err(|e| format!("open: {}", short(&format!("{e:?}"))))?;
        row.opened = true;
        row.open_ms = t.elapsed().as_millis();
        let n = doc.info().map_err(|e| format!("{e}"))?.pages;
        let before = texts(bytes)?;
        let title = title_of_first(bytes);
        let mut done = 0;
        let mut errs: Vec<String> = Vec::new();
        let mut note = |r: Result<(), EditError>, done: &mut u32| match r {
            Ok(()) => *done += 1,
            Err(e) => errs.push(format!("{e}")),
        };
        note(doc.apply(&Op::NewSlide { layout: None }).map(|_| ()), &mut done);
        note(if n > 0 { doc.apply(&Op::DuplicateSlide(0)).map(|_| ()) } else { Err(EditError::NotApplicable) }, &mut done);
        let title_done = match &title {
            Some((x, y, _)) => {
                let r = doc
                    .apply(&Op::SelectShape { slide: 0, x_pt: *x, y_pt: *y })
                    .and_then(|_| doc.apply(&Op::SelectAll))
                    .and_then(|_| doc.apply(&Op::InsertText("ZQX".into())))
                    .map(|_| ());
                let ok = r.is_ok();
                note(r, &mut done);
                ok
            }
            None => {
                errs.push("no title shape".into());
                false
            }
        };
        row.edits = if done == 3 { "3/3".into() } else { format!("{done}/3({})", errs.join(";")) };
        let t = Instant::now();
        match doc.save() {
            Ok(out) => {
                row.save_ms = t.elapsed().as_millis();
                row.saved = "ok".into();
                match EditDoc::open(EditKind::Pptx, &out, &Limits::default()) {
                    Ok(again) => {
                        row.reopened = Some(true);
                        let after = texts(&out)?;
                        let pages_ok = again.info().map(|i| i.pages).ok() == Some(n + done.min(2));
                        let mut got: Vec<String> = after.iter().flatten().cloned().collect();
                        // Expected: the original text, plus the duplicate of slide 0 (when made), the title replaced (when made).
                        let mut want: Vec<String> = before.iter().flatten().cloned().collect();
                        if n > 0 && done >= 2 {
                            want.extend(before[0].iter().cloned());
                        }
                        if title_done {
                            let old = title.as_ref().map(|t| t.2.clone()).unwrap_or_default();
                            if let Some(i) = want.iter().position(|w| *w == old).filter(|_| !old.is_empty()) {
                                want.remove(i);
                            }
                            want.push("ZQX".into());
                        }
                        got.sort();
                        want.sort();
                        row.edit_present = Some(pages_ok && (!title_done || got.iter().any(|g| g == "ZQX")));
                        row.elsewhere_equal = Some(got == want);
                    }
                    Err(_) => row.reopened = Some(false),
                }
                *save_to = Some(out);
            }
            Err(e) => {
                row.save_ms = t.elapsed().as_millis();
                row.saved = format!("FAIL({e})");
            }
        }
        Ok(())
    }
}

fn slide(bytes: &[u8], row: &mut Row, save_to: &mut Option<Vec<u8>>) -> Result<(), String> {
    #[cfg(feature = "edit-slides")]
    return slides::run(bytes, row, save_to);
    #[cfg(not(feature = "edit-slides"))]
    {
        let _ = (bytes, row, save_to);
        Err("pptx not built: run with --features edit-slides".into())
    }
}

// ---------------------------------------------------------------- Run

fn parts_diff(a: &[u8], b: &[u8], row: &mut Row, lost_classes: &mut BTreeMap<String, usize>) {
    let (Ok(a), Ok(b)) = (read_parts(a), read_parts(b)) else { return };
    row.parts = (a.len(), b.len());
    row.lost = a.keys().filter(|n| !b.contains_key(*n)).count();
    row.added = b.keys().filter(|n| !a.contains_key(*n)).count();
    let tally = |p: &Parts| {
        let mut m: BTreeMap<String, usize> = BTreeMap::new();
        for n in p.keys() {
            *m.entry(class(n)).or_default() += 1;
        }
        m
    };
    let (ca, cb) = (tally(&a), tally(&b));
    for (k, n) in &ca {
        let m = cb.get(k).copied().unwrap_or(0);
        if m < *n {
            row.deficits.push(format!("{k}-{}", n - m));
            *lost_classes.entry(k.clone()).or_default() += n - m;
        }
    }
}

#[derive(Default)]
struct Totals {
    files: usize,
    open_failed: usize,
    edit_incomplete: usize,
    edit_missing: usize,
    elsewhere_differs: usize,
    save_failed: usize,
    reopen_failed: usize,
    files_losing_parts: usize,
    parts_lost: usize,
    open_ms: Vec<u128>,
    save_ms: Vec<u128>,
    lost_classes: BTreeMap<String, usize>,
    errors: BTreeMap<String, usize>,
}

#[test]
#[ignore = "reads the whole corpus; run with --ignored corpus_edit --nocapture"]
fn corpus_edit() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus");
    let tsv = std::fs::read_to_string(root.join("SOURCES.tsv")).expect("SOURCES.tsv");
    let out_dir = std::env::var_os("CORPUS_OUT").map(std::path::PathBuf::from);
    if let Some(d) = &out_dir {
        std::fs::create_dir_all(d.join("saved")).expect("CORPUS_OUT");
    }
    let mut report = String::new();
    let mut say = |s: String| {
        println!("{s}");
        report.push_str(&s);
        report.push('\n');
    };
    let mut totals: BTreeMap<String, Totals> = BTreeMap::new();
    for line in tsv.lines().skip(1) {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 5 {
            continue;
        }
        let path = cols[0];
        let Some(kind) = ["docx", "xlsx", "pptx"].into_iter().find(|k| path.ends_with(&format!(".{k}"))) else { continue };
        let is_ms = cols[4].contains("Microsoft");
        let bytes = std::fs::read(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        let mut row = Row::default();
        let mut saved: Option<Vec<u8>> = None;
        let result = catch_unwind(AssertUnwindSafe(|| match kind {
            "docx" => word(&bytes, &mut row, &mut saved),
            "xlsx" => sheet(&bytes, &mut row, &mut saved),
            _ => slide(&bytes, &mut row, &mut saved),
        }));
        let result = match result {
            Ok(r) => r,
            Err(_) => Err("PANIC".into()),
        };
        let mut lost_classes = BTreeMap::new();
        if let Some(out) = &saved {
            parts_diff(&bytes, out, &mut row, &mut lost_classes);
            if let Some(d) = &out_dir {
                std::fs::write(d.join("saved").join(path.replace('/', "__")), out).ok();
            }
        }
        let t = totals.entry(format!("{kind}/{}", if is_ms { "microsoft" } else { "other" })).or_default();
        t.files += 1;
        for (k, n) in lost_classes {
            *t.lost_classes.entry(k).or_default() += n;
        }
        let yn = |b: Option<bool>| b.map(|b| b.to_string()).unwrap_or_else(|| "n/a".into());
        let err = result.err();
        if let Some(e) = &err {
            *t.errors.entry(short(e)).or_default() += 1;
        }
        if !row.opened {
            t.open_failed += 1;
        } else {
            t.open_ms.push(row.open_ms);
            if !row.edits.starts_with("3/3") {
                t.edit_incomplete += 1;
            }
            if row.edit_present == Some(false) {
                t.edit_missing += 1;
            }
            if row.elsewhere_equal == Some(false) {
                t.elsewhere_differs += 1;
            }
            if row.saved != "ok" {
                t.save_failed += 1;
                *t.errors.entry(format!("save {}", short(&row.saved))).or_default() += 1;
            } else {
                t.save_ms.push(row.save_ms);
            }
            if row.reopened == Some(false) {
                t.reopen_failed += 1;
            }
            if row.lost > 0 {
                t.files_losing_parts += 1;
            }
            t.parts_lost += row.lost;
        }
        say(format!(
            "FILE {path} | producer={} | ms={} | opened={} open_ms={} | edits={} | edit_present={} | elsewhere_equal={} | save={} save_ms={} | reopened={} | parts={}->{} lost={} added={} class_deficits=[{}]{}",
            cols[4],
            if is_ms { "yes" } else { "no" },
            if row.opened { "yes".to_string() } else { format!("FAIL({})", err.as_deref().map(short).unwrap_or_default()) },
            ms(row.open_ms),
            if row.opened { row.edits.clone() } else { "n/a".into() },
            yn(row.edit_present),
            yn(row.elsewhere_equal),
            if row.opened { row.saved.clone() } else { "n/a".into() },
            ms(row.save_ms),
            yn(row.reopened),
            row.parts.0,
            row.parts.1,
            row.lost,
            row.added,
            row.deficits.join(", "),
            match (&err, row.opened) {
                (Some(e), true) => format!(" | error={}", short(e)),
                _ => String::new(),
            }
        ));
    }
    let med = |v: &mut Vec<u128>| {
        v.sort();
        (v.get(v.len() / 2).copied().unwrap_or(0), v.last().copied().unwrap_or(0))
    };
    for (kind, t) in totals.iter_mut() {
        let (om, ox) = med(&mut t.open_ms);
        let (sm, sx) = med(&mut t.save_ms);
        say(format!(
            "SUMMARY {kind}: files={} open_failed={} edits_incomplete={} edit_missing={} elsewhere_differs={} save_failed={} reopen_failed={} files_losing_parts={} parts_lost={} open_ms_median={om} open_ms_max={ox} save_ms_median={sm} save_ms_max={sx}",
            t.files, t.open_failed, t.edit_incomplete, t.edit_missing, t.elsewhere_differs, t.save_failed, t.reopen_failed, t.files_losing_parts, t.parts_lost
        ));
        let mut v: Vec<_> = t.lost_classes.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (k, n) in v {
            say(format!("SUMMARY {kind} lost_part_class {k} x{n}"));
        }
        for (k, n) in &t.errors {
            say(format!("SUMMARY {kind} error {k} x{n}"));
        }
    }
    if let Some(d) = &out_dir {
        std::fs::write(d.join("report-edit.txt"), report).expect("report");
    }
    assert!(!totals.is_empty(), "no docx/xlsx/pptx rows found in corpus/SOURCES.tsv");
}
