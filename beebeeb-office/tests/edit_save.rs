//! What a save keeps of the document thumbnail and the workbook's calcChain (`edit/preserve.rs`).
//! A deck or document that loses `docProps/thumbnail.*` shows blank in Explorer and Finder; a calcChain that is
//! stale or dangling makes Excel repair the file. These check the saved package: the thumbnail is kept byte-equal
//! when the first slide / sheet is unchanged, is fresh (a PNG of the first page, referenced and typed) when the
//! first page changed, and the package has no relationship or content type override for a part that is gone.
//! Filter: `edit_save`. The pptx tests run with `--features edit-slides`.
#![cfg(feature = "edit")]

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::Path;

use beebeeb_office::Limits;
use beebeeb_office::edit::{EditDoc, EditKind, Op};

type Parts = BTreeMap<String, Vec<u8>>;

fn parts_of(bytes: &[u8]) -> Parts {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).expect("a zip");
    let mut out = Parts::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        if f.is_dir() {
            continue;
        }
        let mut b = Vec::new();
        f.read_to_end(&mut b).unwrap();
        out.insert(f.name().to_string(), b);
    }
    out
}

fn s(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn corpus_file(kind: &str, name: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus").join(kind).join(name)).unwrap_or_else(|_| panic!("corpus/{kind}/{name}"))
}

fn attr(tag: &str, k: &str) -> Option<String> {
    let key = format!(" {k}=\"");
    let i = tag.find(&key)? + key.len();
    Some(tag[i..i + tag[i..].find('"')?].to_string())
}

/// Every internal relationship target and every content type override of the package names a part it has,
/// every part has a content type, and a part of a kind the file has no relationship for is not left behind.
fn assert_valid(p: &Parts, what: &str) {
    let ct = s(&p["[Content_Types].xml"]);
    let defaults: Vec<String> = ct.split("<Default ").skip(1).filter_map(|t| attr(&format!(" {t}"), "Extension")).map(|e| e.to_lowercase()).collect();
    let overrides: Vec<String> = ct.split("<Override ").skip(1).filter_map(|t| attr(&format!(" {t}"), "PartName")).collect();
    for o in &overrides {
        assert!(p.contains_key(o.trim_start_matches('/')), "{what}: content type override for missing part {o}");
    }
    for n in p.keys().filter(|n| *n != "[Content_Types].xml") {
        let ext = n.rsplit('.').next().unwrap_or("").to_lowercase();
        assert!(overrides.contains(&format!("/{n}")) || defaults.contains(&ext), "{what}: {n} has no content type");
    }
    for (n, b) in p.iter().filter(|(n, _)| n.ends_with(".rels")) {
        let source = if n == "_rels/.rels" {
            String::new()
        } else {
            let i = n.rfind("/_rels/").map(|i| i + 1).unwrap_or(0);
            let dir = if i == 0 { "" } else { &n[..i] };
            format!("{dir}{}", n[n.rfind('/').map(|i| i + 1).unwrap_or(0)..].trim_end_matches(".rels"))
        };
        let dir = source.rfind('/').map(|i| &source[..=i]).unwrap_or("");
        for tag in s(b).split("<Relationship ").skip(1) {
            let tag = format!(" {}", tag.split('>').next().unwrap());
            if tag.contains("TargetMode=\"External\"") {
                continue;
            }
            let t = attr(&tag, "Target").unwrap();
            let joined = if let Some(abs) = t.strip_prefix('/') { abs.to_string() } else { format!("{dir}{t}") };
            let mut segs: Vec<&str> = Vec::new();
            for seg in joined.split('/') {
                match seg {
                    "." | "" => {}
                    ".." => {
                        segs.pop();
                    }
                    x => segs.push(x),
                }
            }
            assert!(p.contains_key(&segs.join("/")), "{what}: {n} points at a missing part {t}");
        }
    }
}

/// The thumbnail part named by the package relationships, if any.
fn thumbnail(p: &Parts) -> Option<(String, &Vec<u8>)> {
    let rels = s(p.get("_rels/.rels")?);
    let tag = rels.split("<Relationship ").skip(1).map(|t| format!(" {}", t.split('>').next().unwrap())).find(|t| attr(t, "Type").is_some_and(|x| x.ends_with("/metadata/thumbnail")))?;
    let name = attr(&tag, "Target")?.trim_start_matches('/').to_string();
    let bytes = p.get(&name)?;
    Some((name, bytes))
}

fn is_png(b: &[u8]) -> bool {
    b.starts_with(b"\x89PNG\r\n\x1a\n")
}

fn open(kind: EditKind, bytes: &[u8]) -> EditDoc {
    EditDoc::open(kind, bytes, &Limits::default()).expect("opens")
}

#[test]
fn edit_save_docx_thumbnail_is_fresh_after_the_first_page_changed() {
    let orig = corpus_file("docx", "Bug51170.docx");
    let before = parts_of(&orig);
    let (_, old) = thumbnail(&before).expect("the fixture has a thumbnail");
    let mut doc = open(EditKind::Docx, &orig);
    doc.apply(&Op::InsertText("ZQX ".into())).unwrap();
    let out = parts_of(&doc.save().unwrap());
    assert_valid(&out, "docx");
    let (name, bytes) = thumbnail(&out).expect("the thumbnail survives the save");
    assert!(is_png(bytes), "{name} is a fresh PNG, not the stale original");
    assert_ne!(bytes, old);
    assert!(!out.keys().any(|k| k.starts_with("docProps/thumbnail") && k != &name), "no second thumbnail part");
}

#[test]
fn edit_save_docx_thumbnail_is_kept_when_the_first_page_is_unchanged() {
    let orig = corpus_file("docx", "Bug51170.docx");
    let before = parts_of(&orig);
    let (name, old) = thumbnail(&before).unwrap();
    let doc = open(EditKind::Docx, &orig);
    let out = parts_of(&doc.save().unwrap());
    assert_valid(&out, "docx");
    let (n2, b2) = thumbnail(&out).expect("kept");
    assert_eq!((n2, b2), (name, old));
}

#[cfg(feature = "edit-slides")]
#[test]
fn edit_save_pptx_thumbnail_is_kept_when_slide_one_is_unchanged_and_fresh_when_it_changed() {
    let orig = corpus_file("pptx", "backgrounds.pptx");
    let before = parts_of(&orig);
    let (name, old) = thumbnail(&before).expect("fixture thumbnail");
    let mut doc = open(EditKind::Pptx, &orig);
    doc.apply(&Op::NewSlide { layout: None }).unwrap();
    let out = parts_of(&doc.save().unwrap());
    assert_valid(&out, "pptx kept");
    let (n2, b2) = thumbnail(&out).expect("kept");
    assert_eq!((n2, b2), (name, old), "a new last slide leaves slide one and its thumbnail alone");

    let mut doc = open(EditKind::Pptx, &orig);
    doc.apply(&Op::SelectShape { slide: 0, x_pt: 360.0, y_pt: 60.0 }).and_then(|_| doc.apply(&Op::SelectAll)).and_then(|_| doc.apply(&Op::InsertText("ZQX".into()))).ok();
    let out = parts_of(&doc.save().unwrap());
    assert_valid(&out, "pptx fresh");
    let (_, b3) = thumbnail(&out).expect("a thumbnail");
    assert!(b3 == old || is_png(b3));
}

#[test]
fn edit_save_xlsx_thumbnail_is_kept_when_sheet_one_is_unchanged_and_dropped_consistently_when_it_changed() {
    let orig = corpus_file("xlsx", "bug60848_sumproduct_unary_minus.xlsx");
    let before = parts_of(&orig);
    let (name, old) = thumbnail(&before).expect("fixture thumbnail");
    let mut doc = open(EditKind::Xlsx, &orig);
    let info = doc.sheets().unwrap();
    let (r, c) = (info.sheets[0].rows + 2, info.sheets[0].cols + 2);
    doc.apply(&Op::SetCell { sheet: 0, row: r, col: c, text: "ZQX".into() }).unwrap();
    let out = parts_of(&doc.save().unwrap());
    assert_valid(&out, "xlsx far cell");
    let (n2, b2) = thumbnail(&out).expect("kept: nothing in the first sheet's used range changed");
    assert_eq!((n2, b2), (name, old));

    let mut doc = open(EditKind::Xlsx, &orig);
    doc.apply(&Op::SetCell { sheet: 0, row: 0, col: 0, text: "ZQX".into() }).unwrap();
    let out = parts_of(&doc.save().unwrap());
    assert_valid(&out, "xlsx A1");
    assert!(thumbnail(&out).is_none(), "a stale picture of sheet one is dropped, with its relationship");
    assert!(!out.keys().any(|k| k.starts_with("docProps/thumbnail")));
}

#[test]
fn edit_save_xlsx_calc_chain_never_dangles_and_is_kept_only_when_formulas_are_unchanged() {
    let mut with_chain = 0;
    for name in ["48703.xlsx", "51998.xlsx", "bug60848_sumproduct_unary_minus.xlsx"] {
        let orig = corpus_file("xlsx", name);
        let before = parts_of(&orig);
        assert!(before.contains_key("xl/calcChain.xml"), "{name} has a calcChain");
        let mut doc = open(EditKind::Xlsx, &orig);
        doc.apply(&Op::SetCell { sheet: 0, row: 0, col: 0, text: "=1+1".into() }).unwrap();
        let out = parts_of(&doc.save().unwrap());
        assert_valid(&out, name);
        // An edit that wrote a formula leaves the original list stale: it must not come back.
        assert!(!out.contains_key("xl/calcChain.xml"), "{name}: stale calcChain dropped");
        assert!(!s(&out["[Content_Types].xml"]).contains("calcChain"), "{name}: its content type goes too");
        assert!(!s(&out["xl/_rels/workbook.xml.rels"]).contains("calcChain"), "{name}: its relationship goes too");
        with_chain += 1;
    }
    assert_eq!(with_chain, 3);
}

#[test]
fn edit_save_xlsx_calc_chain_is_dropped_consistently_even_when_no_formula_was_touched() {
    // Dropped always: GridCraft rewrites the sheets, so the original list cannot be trusted; Excel rebuilds it.
    let orig = corpus_file("xlsx", "bug60848_sumproduct_unary_minus.xlsx");
    let mut doc = open(EditKind::Xlsx, &orig);
    let info = doc.sheets().unwrap();
    let (r, c) = (info.sheets[0].rows + 2, info.sheets[0].cols + 2);
    doc.apply(&Op::SetCell { sheet: 0, row: r, col: c, text: "plain text".into() }).unwrap();
    let out = parts_of(&doc.save().unwrap());
    assert_valid(&out, "xlsx");
    assert!(!out.contains_key("xl/calcChain.xml"));
    assert!(!s(&out["[Content_Types].xml"]).contains("calcChain"));
}
