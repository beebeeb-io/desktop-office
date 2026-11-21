//! What an UNEDITED open-and-save through each engine loses on real files saved by Microsoft Office.
//!
//! Ignored by default (it reads the whole `corpus/`). Run:
//!   cargo test --locked -- --ignored corpus_roundtrip --nocapture
//! It uses the engine crates directly (WordCraft `io`, GridCraft `io`, DeckCraft `pptx`), not the
//! `edit` API: open, save to bytes, reopen. One `FILE` line per document and one `SUMMARY` block per
//! type go to stdout; the same text goes to `$CORPUS_OUT/report.txt` when that is set, and the saved
//! bytes to `$CORPUS_OUT/saved/` for `scripts/pkgdiff.py` and `scripts/celldiff.py`.
//! Only part names (digits folded to `N`), counts and yes/no are printed, never document text.
#![cfg(feature = "edit")]

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};

use zip::ZipArchive;

type Parts = BTreeMap<String, Vec<u8>>;

fn read_parts(bytes: &[u8]) -> Result<Parts, String> {
    let mut z = ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
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

fn is_xml(name: &str) -> bool {
    name.ends_with(".xml") || name.ends_with(".rels") || name.ends_with(".vml")
}

/// XML with the declaration and the whitespace between tags removed, so a pure re-serialisation is not "changed".
fn norm(name: &str, b: &[u8]) -> Vec<u8> {
    if !is_xml(name) {
        return b.to_vec();
    }
    let s = String::from_utf8_lossy(b);
    let s = s.trim_start_matches('\u{feff}');
    let s = match (s.starts_with("<?xml"), s.find("?>")) {
        (true, Some(i)) => &s[i + 2..],
        _ => s,
    };
    let mut out = String::with_capacity(s.len());
    let mut pending_ws = String::new();
    let mut after_gt = false;
    for c in s.chars() {
        if after_gt && c.is_whitespace() {
            pending_ws.push(c);
            continue;
        }
        if !pending_ws.is_empty() {
            if c != '<' {
                out.push_str(&pending_ws);
            }
            pending_ws.clear();
        }
        out.push(c);
        after_gt = c == '>';
    }
    out.into_bytes()
}

fn text_of(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn count(parts: &Parts, filter: impl Fn(&str) -> bool, needle: &str) -> usize {
    parts.iter().filter(|(n, _)| filter(n)).map(|(_, b)| text_of(b).matches(needle).count()).sum()
}

/// Watched features: how many of each the package carries.
fn features(p: &Parts) -> BTreeMap<&'static str, usize> {
    let has = |f: &dyn Fn(&str) -> bool| p.keys().filter(|n| f(n)).count();
    let slides = |n: &str| n.starts_with("ppt/slides/slide") && n.ends_with(".xml");
    let mut m = BTreeMap::new();
    m.insert("macros", has(&|n| n.ends_with("vbaProject.bin") || n.ends_with("vbaData.xml")));
    m.insert("transitions", count(p, slides, "<p:transition"));
    m.insert(
        "linked_images",
        p.iter()
            .filter(|(n, _)| n.ends_with(".rels"))
            .map(|(_, b)| {
                let t = text_of(b);
                t.split("<Relationship ").filter(|r| r.contains("/image") && r.contains("TargetMode=\"External\"")).count()
            })
            .sum(),
    );
    m.insert("chart_style_parts", has(&|n| n.contains("/charts/style") || n.contains("/charts/colors")));
    m.insert("comments", has(&|n| n.contains("comments") && n.ends_with(".xml") || n.contains("commentAuthors") || n.contains("threadedComments")));
    m.insert("tracked_changes", count(p, |n| n.starts_with("word/") && n.ends_with(".xml"), "<w:ins ") + count(p, |n| n.starts_with("word/") && n.ends_with(".xml"), "<w:del "));
    m.insert("pivots", has(&|n| n.contains("pivotTable") || n.contains("pivotCache")));
    m.insert("smartart", has(&|n| n.contains("/diagrams/")));
    m.insert("equations", count(p, |n| n.ends_with(".xml"), "<m:oMath"));
    m.insert("ole_objects", has(&|n| n.contains("/embeddings/")) + count(p, |n| n.ends_with(".xml"), "<o:OLEObject") + count(p, |n| n.ends_with(".xml"), "<p:oleObj"));
    m.insert("signatures", has(&|n| n.starts_with("_xmlsignatures/")));
    // Extra parts worth knowing about.
    m.insert("charts", has(&|n| n.contains("/charts/chart")));
    m.insert("images", has(&|n| n.contains("/media/")));
    m.insert("notes_slides", has(&|n| n.starts_with("ppt/notesSlides/")));
    m.insert("custom_xml", has(&|n| n.starts_with("customXml/")));
    m.insert("footnotes_endnotes", has(&|n| n.ends_with("footnotes.xml") || n.ends_with("endnotes.xml")));
    m.insert("headers_footers", has(&|n| n.starts_with("word/header") || n.starts_with("word/footer")));
    m.insert("themes", has(&|n| n.contains("/theme/")));
    m.insert("slide_layouts_masters", has(&|n| n.contains("slideLayouts/slideLayout") || n.contains("slideMasters/slideMaster")));
    m.insert("defined_names", count(p, |n| n == "xl/workbook.xml", "<definedName "));
    m.insert("data_validations", count(p, |n| n.starts_with("xl/worksheets/"), "<dataValidation "));
    m.insert("conditional_formats", count(p, |n| n.starts_with("xl/worksheets/"), "<conditionalFormatting"));
    m.insert("drawings", has(&|n| n.contains("/drawings/drawing")));
    m.insert("tables", has(&|n| n.starts_with("xl/tables/")));
    m.insert("doc_properties", has(&|n| n.starts_with("docProps/")));
    m.insert("fonts_embedded", has(&|n| n.contains("/fonts/")));
    m
}

fn tag_texts(xml: &str, open: &str, close: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find(open) {
        let after = &rest[i + open.len()..];
        // `<w:t>` or `<w:t xml:space=..>` but not `<w:tab/>`.
        match after.chars().next() {
            Some('>') | Some(' ') => {}
            _ => {
                rest = after;
                continue;
            }
        }
        let Some(gt) = after.find('>') else { break };
        if after[..gt].ends_with('/') {
            rest = &after[gt..];
            continue;
        }
        let body = &after[gt + 1..];
        let Some(end) = body.find(close) else { break };
        v.push(body[..end].to_string());
        rest = &body[end..];
    }
    v
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let k = format!("{name}=\"");
    let i = tag.find(&k)? + k.len();
    Some(tag[i..i + tag[i..].find('"')?].to_string())
}

/// Content summary: (units, characters, a hash of the content, formula count). Never the content itself.
#[derive(Default, PartialEq, Clone, Debug)]
struct Content {
    units: usize,
    chars: usize,
    hash: u64,
    formulas: usize,
}

fn fnv(h: &mut u64, s: &str) {
    for b in s.bytes().chain([0u8]) {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x100000001b3);
    }
}

fn content(kind: &str, p: &Parts) -> Content {
    let mut c = Content { hash: 0xcbf29ce484222325, ..Default::default() };
    let add = |c: &mut Content, s: &str| {
        c.units += 1;
        c.chars += s.chars().count();
        fnv(&mut c.hash, s);
    };
    match kind {
        "docx" => {
            for (n, b) in p.iter().filter(|(n, _)| n.starts_with("word/document") && n.ends_with(".xml")) {
                let _ = n;
                let x = text_of(b);
                for t in tag_texts(&x, "<w:t", "</w:t>") {
                    add(&mut c, &t);
                }
            }
        }
        "pptx" => {
            let mut slides: Vec<&String> = p.keys().filter(|n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml")).collect();
            slides.sort_by_key(|n| n.trim_start_matches("ppt/slides/slide").trim_end_matches(".xml").parse::<u32>().unwrap_or(0));
            for n in slides {
                let x = text_of(&p[n]);
                for t in tag_texts(&x, "<a:t", "</a:t>") {
                    add(&mut c, &t);
                }
            }
        }
        "xlsx" => {
            let ss: Vec<String> = p
                .get("xl/sharedStrings.xml")
                .map(|b| {
                    let x = text_of(b);
                    x.split("<si>").skip(1).map(|si| tag_texts(si, "<t", "</t>").concat()).collect()
                })
                .unwrap_or_default();
            // Sheets in workbook order, whatever the parts are called.
            let wb = p.get("xl/workbook.xml").map(|b| text_of(b)).unwrap_or_default();
            let rels = p.get("xl/_rels/workbook.xml.rels").map(|b| text_of(b)).unwrap_or_default();
            let mut targets = BTreeMap::new();
            for r in rels.split("<Relationship ").skip(1) {
                if let (Some(id), Some(t)) = (attr(r, "Id"), attr(r, "Target")) {
                    targets.insert(id, t.trim_start_matches('/').trim_start_matches("xl/").to_string());
                }
            }
            for s in wb.split("<sheet ").skip(1) {
                let Some(rid) = attr(s, "r:id") else { continue };
                let Some(t) = targets.get(&rid) else { continue };
                let Some(b) = p.get(&format!("xl/{t}")) else { continue };
                let x = text_of(b);
                fnv(&mut c.hash, "|sheet");
                for cell in x.split("<c ").skip(1) {
                    let head = cell.split('>').next().unwrap_or("");
                    let end = cell.find("</c>").unwrap_or(cell.len());
                    let body = &cell[..end];
                    let has_f = body.contains("<f");
                    if has_f {
                        c.formulas += 1;
                    }
                    let v = tag_texts(body, "<v", "</v>").into_iter().next();
                    let is = tag_texts(body, "<t", "</t>").concat();
                    let val = match (attr(head, "t").as_deref(), v) {
                        (Some("s"), Some(i)) => ss.get(i.parse::<usize>().unwrap_or(usize::MAX)).cloned(),
                        (_, Some(v)) => Some(v),
                        (_, None) if !is.is_empty() => Some(is),
                        _ => None,
                    };
                    if let Some(val) = val {
                        // Formula cells: the cached value may legitimately be recalculated; the value is not compared.
                        let val = if has_f { String::new() } else { val };
                        fnv(&mut c.hash, attr(head, "r").as_deref().unwrap_or(""));
                        add(&mut c, &val);
                    }
                }
            }
        }
        _ => {}
    }
    c
}

fn roundtrip(kind: &str, bytes: &[u8]) -> (Result<Vec<u8>, String>, Result<(), String>) {
    let name = format!("corpus.{kind}");
    let saved: Result<Vec<u8>, String> = match kind {
        "docx" => wordcraft_engine::io::open_bytes(&name, bytes).and_then(|d| wordcraft_engine::io::save_bytes(&name, &d)),
        "xlsx" => gridcraft_engine::io::open_bytes(&name, bytes)
            .map_err(|e| e.to_string())
            .and_then(|(wb, _)| gridcraft_engine::io::save_bytes(&wb, &name).map_err(|e| e.to_string())),
        "pptx" => deckcraft_pptx::import_checked(bytes)
            .map_err(|e| e.to_string())
            .and_then(|(p, _)| deckcraft_pptx::export_as(&p, "pptx").map_err(|e| e.to_string())),
        _ => Err("unsupported".into()),
    };
    let re = match &saved {
        Ok(s) => match kind {
            "docx" => wordcraft_engine::io::open_bytes(&name, s).map(|_| ()),
            "xlsx" => gridcraft_engine::io::open_bytes(&name, s).map(|_| ()).map_err(|e| e.to_string()),
            _ => deckcraft_pptx::import_checked(s).map(|_| ()).map_err(|e| e.to_string()),
        },
        Err(_) => Err("not saved".into()),
    };
    (saved, re)
}

fn guarded(kind: &str, bytes: &[u8]) -> (Result<Vec<u8>, String>, Result<(), String>) {
    match catch_unwind(AssertUnwindSafe(|| roundtrip(kind, bytes))) {
        Ok(r) => r,
        Err(_) => (Err("PANIC".into()), Err("PANIC".into())),
    }
}

#[derive(Default)]
struct Totals {
    files: usize,
    open_save_fail: usize,
    reopen_fail: usize,
    parts_orig: usize,
    parts_lost: usize,
    parts_added: usize,
    parts_changed: usize,
    content_differs: usize,
    formulas_differ: usize,
    lost_classes: BTreeMap<String, usize>,
    changed_classes: BTreeMap<String, usize>,
    feature_loss: BTreeMap<&'static str, (usize, usize)>, // (files losing it, instances lost)
    errors: BTreeMap<String, usize>,
}

fn short(e: &str) -> String {
    // Error texts name no document content; keep the first 60 characters, digits folded.
    class(&e.chars().take(60).collect::<String>())
}

#[test]
#[ignore = "reads the whole corpus; run with --ignored corpus_roundtrip --nocapture"]
fn corpus_roundtrip() {
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
    let mut totals: BTreeMap<&str, Totals> = BTreeMap::new();
    for row in tsv.lines().skip(1) {
        let cols: Vec<&str> = row.split('\t').collect();
        if cols.len() < 5 || !cols[4].contains("Microsoft") {
            continue;
        }
        let path = cols[0];
        let Some(kind) = ["docx", "xlsx", "pptx"].into_iter().find(|k| path.ends_with(&format!(".{k}"))) else { continue };
        let bytes = std::fs::read(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        let t = totals.entry(kind).or_default();
        t.files += 1;
        let (saved, re) = guarded(kind, &bytes);
        let saved = match saved {
            Ok(s) => s,
            Err(e) => {
                t.open_save_fail += 1;
                *t.errors.entry(short(&e)).or_default() += 1;
                say(format!("FILE {path} | producer={} | open_or_save=FAIL({}) ", cols[4], short(&e)));
                continue;
            }
        };
        if let Some(d) = &out_dir {
            std::fs::write(d.join("saved").join(path.replace('/', "__")), &saved).ok();
        }
        if let Err(e) = &re {
            t.reopen_fail += 1;
            *t.errors.entry(format!("reopen: {}", short(e))).or_default() += 1;
        }
        let (a, b) = match (read_parts(&bytes), read_parts(&saved)) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => {
                say(format!("FILE {path} | zip=FAIL({})", short(&e)));
                continue;
            }
        };
        let lost: Vec<&String> = a.keys().filter(|n| !b.contains_key(*n)).collect();
        let added: Vec<&String> = b.keys().filter(|n| !a.contains_key(*n)).collect();
        let changed: Vec<&String> = a.keys().filter(|n| b.get(*n).is_some_and(|y| norm(n, &a[*n]) != norm(n, y))).collect();
        // Renumbered parts are not losses: only the classes that have fewer parts afterwards are.
        let tally = |p: &Parts| {
            let mut m: BTreeMap<String, usize> = BTreeMap::new();
            for n in p.keys() {
                *m.entry(class(n)).or_default() += 1;
            }
            m
        };
        let (ca, cb) = (tally(&a), tally(&b));
        let mut deficits: Vec<String> = Vec::new();
        for (k, n) in &ca {
            let m = cb.get(k).copied().unwrap_or(0);
            if m < *n {
                deficits.push(format!("{k}-{}", n - m));
                *t.lost_classes.entry(k.clone()).or_default() += n - m;
            }
        }
        let mut changed_classes: BTreeSet<String> = BTreeSet::new();
        for n in &changed {
            changed_classes.insert(class(n));
        }
        for k in &changed_classes {
            *t.changed_classes.entry(k.clone()).or_default() += 1;
        }
        let (fa, fb) = (features(&a), features(&b));
        let mut floss: Vec<String> = Vec::new();
        for (k, n) in &fa {
            let m = fb[k];
            if m < *n {
                floss.push(format!("{k} {n}->{m}"));
                let e = t.feature_loss.entry(*k).or_default();
                e.0 += 1;
                e.1 += n - m;
            }
        }
        let (ct_a, ct_b) = (content(kind, &a), content(kind, &b));
        let content_same = ct_a.units == ct_b.units && ct_a.hash == ct_b.hash;
        if !content_same {
            t.content_differs += 1;
        }
        if ct_a.formulas != ct_b.formulas {
            t.formulas_differ += 1;
        }
        t.parts_orig += a.len();
        t.parts_lost += lost.len();
        t.parts_added += added.len();
        t.parts_changed += changed.len();
        say(format!(
            "FILE {path} | producer={} | reopen={} | parts={}->{} lost={} added={} changed={} | class_deficits=[{}] | feature_loss=[{}] | content units {}->{} chars {}->{} same={} formulas {}->{}",
            cols[4],
            if re.is_ok() { "ok" } else { "FAIL" },
            a.len(),
            b.len(),
            lost.len(),
            added.len(),
            changed.len(),
            deficits.join(", "),
            floss.join(", "),
            ct_a.units,
            ct_b.units,
            ct_a.chars,
            ct_b.chars,
            content_same,
            ct_a.formulas,
            ct_b.formulas
        ));
    }
    for (kind, t) in &totals {
        say(format!("SUMMARY {kind}: files={} open_or_save_failed={} reopen_failed={} parts_orig={} lost={} added={} changed={} content_differs={} formula_count_differs={}",
            t.files, t.open_save_fail, t.reopen_fail, t.parts_orig, t.parts_lost, t.parts_added, t.parts_changed, t.content_differs, t.formulas_differ));
        let mut v: Vec<_> = t.lost_classes.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (k, n) in v {
            say(format!("SUMMARY {kind} lost_part_class {k} x{n}"));
        }
        let mut v: Vec<_> = t.changed_classes.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (k, n) in v.into_iter().take(25) {
            say(format!("SUMMARY {kind} changed_part_class {k} in {n} files"));
        }
        for (k, (files, n)) in &t.feature_loss {
            say(format!("SUMMARY {kind} feature_lost {k}: {files} files, {n} instances"));
        }
        for (k, n) in &t.errors {
            say(format!("SUMMARY {kind} error {k} x{n}"));
        }
    }
    if let Some(d) = &out_dir {
        std::fs::write(d.join("report.txt"), report).expect("report");
    }
    assert!(!totals.is_empty(), "no Microsoft Office rows found in corpus/SOURCES.tsv");
}
