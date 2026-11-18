//! What a save carries from the file it was opened from (`edit/preserve.rs`). A bug here loses a macro or a
//! chart's style without a word, so these check the saved package: a macro project (and its signature) is
//! byte-equal and referenced with the macro-enabled content type; chart style and colour parts come back only
//! for a chart the saved package still has; a part whose source the edit removed does not come back; nothing the
//! engine wrote changes except `[Content_Types].xml` and `.rels`; no corpus file is refused.
//! Fixtures: packages written in code, corpus files with a macro project added, and the corpus itself.
//! Filter: `edit_preserve`. The pptx corpus runs with `--features edit-slides`.
#![cfg(feature = "edit")]

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use beebeeb_office::Limits;
use beebeeb_office::edit::{EditDoc, EditKind, Op};

// The module under test is compiled into this test as it is, so `carry` can be driven with hand-made packages
// (the crate keeps it private). `SaveError` is the crate's own, repeated for `super::SaveError`.
#[allow(dead_code)]
#[path = "../src/edit/preserve.rs"]
mod preserve;
pub use beebeeb_office::edit::SaveError;

type Parts = BTreeMap<String, Vec<u8>>;

const VBA_REL: &str = "http://schemas.microsoft.com/office/2006/relationships/vbaProject";
const VBA_SIG_REL: &str = "http://schemas.microsoft.com/office/2006/relationships/vbaProjectSignature";
const VBA_DATA_REL: &str = "http://schemas.microsoft.com/office/2006/relationships/wordVbaData";
const STYLE_REL: &str = "http://schemas.microsoft.com/office/2011/relationships/chartStyle";
const COLORS_REL: &str = "http://schemas.microsoft.com/office/2011/relationships/chartColorStyle";
const OFFICE_DOC_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
const CHART_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart";
const RELS_CT: &str = "application/vnd.openxmlformats-package.relationships+xml";
const NS_RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const DOCM_CT: &str = "application/vnd.ms-word.document.macroEnabled.main+xml";
const PPTM_CT: &str = "application/vnd.ms-powerpoint.presentation.macroEnabled.main+xml";

// A macro project is an opaque binary; any bytes do for a package test, and these are not valid on purpose.
const VBA: &[u8] = b"\xD0\xCF\x11\xE0 not a real project \x00\x01\x02\xFF";
const SIG: &[u8] = b"signature bytes \x00\xFE";
const VBA_DATA: &[u8] = b"<?xml version=\"1.0\"?><wne:vbaSuppData xmlns:wne=\"x\"/>";

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

fn zip_of(parts: &Parts) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (n, b) in parts {
        z.start_file(n.as_str(), opts).unwrap();
        z.write_all(b).unwrap();
    }
    z.finish().unwrap().into_inner()
}

fn s(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// `(Id, Type, Target)` of every relationship in a `.rels` text (attributes in any order).
fn rels(xml: &str) -> Vec<(String, String, String)> {
    let attr = |tag: &str, k: &str| -> String {
        let key = format!(" {k}=\"");
        let i = tag.find(&key).map(|i| i + key.len()).unwrap_or_else(|| panic!("{k} in {tag}"));
        tag[i..i + tag[i..].find('"').unwrap()].to_string()
    };
    xml.split("<Relationship ").skip(1).map(|t| { let t = format!(" {t}"); (attr(&t, "Id"), attr(&t, "Type"), attr(&t, "Target")) }).collect()
}

/// The part a relationship in `source`'s rels points at.
fn target_of(p: &Parts, source: &str, ty: &str) -> Option<String> {
    let rn = match source.rfind('/') {
        Some(i) => format!("{}/_rels/{}.rels", &source[..i], &source[i + 1..]),
        None => format!("_rels/{source}.rels"),
    };
    let (_, _, t) = rels(&s(p.get(&rn)?)).into_iter().find(|r| r.1 == ty)?;
    let dir = source.rfind('/').map(|i| &source[..=i]).unwrap_or("");
    let mut segs: Vec<&str> = Vec::new();
    let joined = if let Some(abs) = t.strip_prefix('/') { abs.to_string() } else { format!("{dir}{t}") };
    for seg in joined.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                segs.pop();
            }
            x => segs.push(x),
        }
    }
    Some(segs.join("/"))
}

fn content_type(p: &Parts, name: &str) -> Option<String> {
    let ct = s(&p["[Content_Types].xml"]);
    let attr = |tag: &str, k: &str| -> Option<String> {
        let key = format!("{k}=\"");
        let i = tag.find(&key)? + key.len();
        Some(tag[i..i + tag[i..].find('"')?].to_string())
    };
    for t in ct.split("<Override ").skip(1) {
        if attr(t, "PartName").as_deref() == Some(&format!("/{name}")) {
            return attr(t, "ContentType");
        }
    }
    let ext = name.rsplit_once('.')?.1;
    ct.split("<Default ").skip(1).find(|t| attr(t, "Extension").is_some_and(|e| e.eq_ignore_ascii_case(ext))).and_then(|t| attr(t, "ContentType"))
}

/// Every part has a content type; every internal relationship points at a part the package has.
fn assert_valid(p: &Parts, what: &str) {
    for n in p.keys() {
        if n != "[Content_Types].xml" {
            assert!(content_type(p, n).is_some(), "{what}: {n} has no content type");
        }
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
            let head = tag.split('>').next().unwrap();
            if head.contains("TargetMode=\"External\"") {
                continue;
            }
            let t = rels(&format!("<Relationship {head}>")).remove(0).2;
            let mut segs: Vec<&str> = Vec::new();
            let joined = if let Some(abs) = t.strip_prefix('/') { abs.to_string() } else { format!("{dir}{t}") };
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

fn add_before(xml: &mut Vec<u8>, close: &str, tag: &str) {
    let t = s(xml);
    let i = t.rfind(close).unwrap_or_else(|| panic!("{close} in the package text"));
    *xml = format!("{}{tag}{}", &t[..i], &t[i..]).into_bytes();
}

fn replace_in(p: &mut Parts, name: &str, from: &str, to: &str) {
    let t = s(&p[name]);
    assert!(t.contains(from), "{name} holds {from}");
    p.insert(name.to_string(), t.replacen(from, to, 1).into_bytes());
}

/// `bytes` with a macro project added as Word writes it (project, its signature, `vbaData.xml`).
fn with_macro(bytes: &[u8], main: &str, main_ct_from: &str, main_ct_to: &str) -> Vec<u8> {
    let mut p = parts_of(bytes);
    let (dir, file) = main.rsplit_once('/').unwrap();
    let rn = format!("{dir}/_rels/{file}.rels");
    p.insert(format!("{dir}/vbaProject.bin"), VBA.to_vec());
    p.insert(format!("{dir}/vbaProjectSignature.bin"), SIG.to_vec());
    p.insert(format!("{dir}/vbaData.xml"), VBA_DATA.to_vec());
    p.insert(format!("{dir}/_rels/vbaProject.bin.rels"), format!("<?xml version=\"1.0\"?><Relationships xmlns=\"{NS_RELS}\"><Relationship Id=\"rId1\" Type=\"{VBA_SIG_REL}\" Target=\"vbaProjectSignature.bin\"/></Relationships>").into_bytes());
    let mut r = p.remove(&rn).unwrap_or_else(|| format!("<?xml version=\"1.0\"?><Relationships xmlns=\"{NS_RELS}\"></Relationships>").into_bytes());
    add_before(&mut r, "</Relationships>", &format!("<Relationship Id=\"rIdVba\" Type=\"{VBA_REL}\" Target=\"vbaProject.bin\"/><Relationship Id=\"rIdVbaData\" Type=\"{VBA_DATA_REL}\" Target=\"vbaData.xml\"/>"));
    p.insert(rn, r);
    let mut ct = p.remove("[Content_Types].xml").unwrap();
    add_before(
        &mut ct,
        "</Types>",
        &format!(
            "<Override PartName=\"/{dir}/vbaProject.bin\" ContentType=\"application/vnd.ms-office.vbaProject\"/><Override PartName=\"/{dir}/vbaProjectSignature.bin\" ContentType=\"application/vnd.ms-office.vbaProjectSignature\"/><Override PartName=\"/{dir}/vbaData.xml\" ContentType=\"application/vnd.ms-word.vbaData+xml\"/>"
        ),
    );
    p.insert("[Content_Types].xml".into(), ct);
    replace_in(&mut p, "[Content_Types].xml", main_ct_from, main_ct_to);
    zip_of(&p)
}

fn corpus_dir(kind: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus").join(kind)
}

fn corpus(kind: &str) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(corpus_dir(kind))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == kind))
        .map(|p| (p.file_name().unwrap().to_string_lossy().to_string(), std::fs::read(&p).unwrap()))
        .collect();
    v.sort();
    v
}

const DOCX_MAIN_CT: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";

/// A Word 15 file from the corpus, made macro-enabled.
fn docm() -> Vec<u8> {
    let (_, bytes) = corpus("docx").into_iter().find(|(n, _)| n == "59030.docx").expect("corpus/docx/59030.docx");
    with_macro(&bytes, "word/document.xml", DOCX_MAIN_CT, DOCM_CT)
}

fn assert_macro_carried(out: &Parts, what: &str) {
    assert_eq!(out.get("word/vbaProject.bin").map(Vec::as_slice), Some(VBA), "{what}: project byte-equal");
    assert_eq!(out.get("word/vbaProjectSignature.bin").map(Vec::as_slice), Some(SIG), "{what}: signature byte-equal");
    assert_eq!(out.get("word/vbaData.xml").map(Vec::as_slice), Some(VBA_DATA), "{what}: vbaData byte-equal");
    assert_eq!(target_of(out, "word/document.xml", VBA_REL).as_deref(), Some("word/vbaProject.bin"), "{what}: project referenced");
    assert_eq!(target_of(out, "word/document.xml", VBA_DATA_REL).as_deref(), Some("word/vbaData.xml"), "{what}: vbaData referenced");
    assert_eq!(target_of(out, "word/vbaProject.bin", VBA_SIG_REL).as_deref(), Some("word/vbaProjectSignature.bin"), "{what}: signature referenced");
    assert_eq!(content_type(out, "word/vbaProject.bin").as_deref(), Some("application/vnd.ms-office.vbaProject"), "{what}");
    assert_eq!(content_type(out, "word/document.xml").as_deref(), Some(DOCM_CT), "{what}: main part macro-enabled");
    assert_valid(out, what);
}

#[test]
fn edit_preserve_docm_keeps_its_macro_project_through_an_edit_and_save() {
    let bytes = docm();
    let mut doc = EditDoc::open(EditKind::Docx, &bytes, &Limits::default()).expect("a .docm opens for editing");
    doc.apply(&Op::InsertText("ZQXedit ".into())).unwrap();
    let out = doc.save().expect("saves with its macro");
    let p = parts_of(&out);
    assert_macro_carried(&p, "edited docm");
    // The engine's own parts are the engine's: the document is the edited one, and the file reopens.
    assert!(s(&p["word/document.xml"]).contains("ZQXedit"));
    let again = EditDoc::open(EditKind::Docx, &out, &Limits::default()).expect("reopens");
    // A second save of the reopened file (macro now already in the package) keeps it, once.
    let out2 = again.save().expect("saves again");
    let p2 = parts_of(&out2);
    assert_macro_carried(&p2, "second save");
    let rs = rels(&s(&p2["word/_rels/document.xml.rels"]));
    assert_eq!(rs.iter().filter(|r| r.1 == VBA_REL).count(), 1, "one macro relationship");
}

#[test]
fn edit_preserve_a_plain_docx_comes_back_with_no_new_parts() {
    let (_, bytes) = corpus("docx").into_iter().find(|(n, _)| n == "59030.docx").unwrap();
    let mut doc = EditDoc::open(EditKind::Docx, &bytes, &Limits::default()).unwrap();
    doc.apply(&Op::InsertText("x".into())).unwrap();
    let out = doc.save().unwrap();
    let p = parts_of(&out);
    assert!(!p.keys().any(|n| n.contains("vba")), "no macro invented");
    assert!(content_type(&p, "word/document.xml").unwrap().contains("document.main+xml"));
}

#[cfg(feature = "edit-slides")]
#[test]
fn edit_preserve_pptm_keeps_its_macro_project_through_the_deck_engine() {
    let (_, deck) = corpus("pptx").into_iter().find(|(n, _)| n == "copy-slide-demo.pptx").expect("corpus/pptx/copy-slide-demo.pptx");
    let main_ct = "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";
    let bytes = with_macro(&deck, "ppt/presentation.xml", main_ct, PPTM_CT);
    let mut doc = EditDoc::open(EditKind::Pptx, &bytes, &Limits::default()).expect("a .pptm opens for editing");
    let _ = doc.apply(&Op::InsertText("ZQX ".into()));
    let out = parts_of(&doc.save().expect("saves with its macro"));
    assert_eq!(out.get("ppt/vbaProject.bin").map(Vec::as_slice), Some(VBA), "project byte-equal");
    assert_eq!(target_of(&out, "ppt/presentation.xml", VBA_REL).as_deref(), Some("ppt/vbaProject.bin"));
    assert_eq!(content_type(&out, "ppt/presentation.xml").as_deref(), Some(PPTM_CT));
    assert_valid(&out, "edited pptm");
}

#[test]
fn edit_preserve_xlsm_keeps_its_macro_project() {
    // A workbook the sheet engine writes, made macro-enabled; Excel's engine keeps the project itself.
    let mut sh = gridcraft_model::Sheet::new("Data");
    sh.set_cell(gridcraft_core::CellRef::new(0, 0), gridcraft_model::Cell::value(gridcraft_core::Value::Number(1.0)));
    let mut wb = gridcraft_model::Workbook::new();
    wb.sheets = vec![std::sync::Arc::new(sh)];
    let plain = gridcraft_xlsx::write_xlsx(&wb).unwrap();
    let main_ct = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
    let bytes = with_macro(&plain, "xl/workbook.xml", main_ct, "application/vnd.ms-excel.sheet.macroEnabled.main+xml");
    let mut doc = EditDoc::open(EditKind::Xlsx, &bytes, &Limits::default()).expect("opens");
    doc.apply(&Op::SetCell { sheet: 0, row: 1, col: 0, text: "7".into() }).unwrap();
    let out = parts_of(&doc.save().expect("saves with its macro"));
    assert_eq!(out.get("xl/vbaProject.bin").map(Vec::as_slice), Some(VBA), "project byte-equal");
    assert_eq!(target_of(&out, "xl/workbook.xml", VBA_REL).as_deref(), Some("xl/vbaProject.bin"));
    assert_eq!(content_type(&out, "xl/workbook.xml").as_deref(), Some("application/vnd.ms-excel.sheet.macroEnabled.main+xml"));
    assert_valid(&out, "edited xlsm");
}

// ---- hand-made packages, through `carry` ----------------------------------------------------------------

fn types(overrides: &[(&str, &str)]) -> String {
    let o: String = overrides.iter().map(|(n, t)| format!("<Override PartName=\"/{n}\" ContentType=\"{t}\"/>")).collect();
    format!("<?xml version=\"1.0\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"{RELS_CT}\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/>{o}</Types>")
}

fn rels_xml(r: &[(&str, &str, &str)]) -> String {
    let o: String = r.iter().map(|(i, t, g)| format!("<Relationship Id=\"{i}\" Type=\"{t}\" Target=\"{g}\"/>")).collect();
    format!("<?xml version=\"1.0\"?><Relationships xmlns=\"{NS_RELS}\">{o}</Relationships>")
}

const CHART_CT: &str = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml";
const STYLE_CT: &str = "application/vnd.ms-office.chartstyle+xml";
const COLORS_CT: &str = "application/vnd.ms-office.chartcolorstyle+xml";
const STYLE: &str = "<cs:chartStyle xmlns:cs=\"c\" id=\"201\"/>";
const COLORS: &str = "<cs:colorStyle xmlns:cs=\"c\" meth=\"cycle\"/>";

/// A deck with `charts` charts; each original chart has a style and a colour part.
fn deck_with_charts(charts: &[u32], with_style: bool) -> Parts {
    let mut overrides = vec![("ppt/presentation.xml", "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml".to_string())];
    let mut files: Vec<(String, String)> = vec![("_rels/.rels".into(), rels_xml(&[("rId1", OFFICE_DOC_REL, "ppt/presentation.xml")])), ("ppt/presentation.xml".into(), "<p:presentation xmlns:p=\"p\"/>".into())];
    let mut presrels = Vec::new();
    for n in charts {
        overrides.push((Box::leak(format!("ppt/charts/chart{n}.xml").into_boxed_str()), CHART_CT.into()));
        files.push((format!("ppt/charts/chart{n}.xml"), format!("<c:chartSpace xmlns:c=\"c\"><!-- chart {n} --></c:chartSpace>")));
        presrels.push((format!("rIdC{n}"), CHART_REL.to_string(), format!("charts/chart{n}.xml")));
        if with_style {
            overrides.push((Box::leak(format!("ppt/charts/style{n}.xml").into_boxed_str()), STYLE_CT.into()));
            overrides.push((Box::leak(format!("ppt/charts/colors{n}.xml").into_boxed_str()), COLORS_CT.into()));
            files.push((format!("ppt/charts/style{n}.xml"), STYLE.replace("201", &format!("2{n}"))));
            files.push((format!("ppt/charts/colors{n}.xml"), COLORS.into()));
            files.push((
                format!("ppt/charts/_rels/chart{n}.xml.rels"),
                rels_xml(&[("rId1", STYLE_REL, &format!("style{n}.xml")), ("rId2", COLORS_REL, &format!("colors{n}.xml"))]),
            ));
        }
    }
    let pr: Vec<(&str, &str, &str)> = presrels.iter().map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str())).collect();
    files.push(("ppt/_rels/presentation.xml.rels".into(), rels_xml(&pr)));
    let ov: Vec<(&str, &str)> = overrides.iter().map(|(a, b)| (*a, b.as_str())).collect();
    files.push(("[Content_Types].xml".into(), types(&ov)));
    files.into_iter().map(|(n, t)| (n, t.into_bytes())).collect()
}

fn carry(original: &Parts, written: &Parts) -> Result<Parts, SaveError> {
    preserve::carry(&zip_of(original), zip_of(written)).map(|b| parts_of(&b))
}

/// Everything the engine wrote is still byte-equal, except the content types and relationship files.
fn assert_engine_parts_untouched(written: &Parts, out: &Parts) {
    for (n, b) in written {
        if n == "[Content_Types].xml" || n.ends_with(".rels") {
            continue;
        }
        assert_eq!(out.get(n), Some(b), "{n} untouched");
    }
}

#[test]
fn edit_preserve_chart_style_and_colours_come_back_for_a_chart_that_is_still_there() {
    let original = deck_with_charts(&[1, 2], true);
    // The engine wrote both charts (changing their XML) and no style or colour part.
    let mut written = deck_with_charts(&[1, 2], false);
    written.insert("ppt/charts/chart1.xml".into(), b"<c:chartSpace xmlns:c=\"c\"><!-- rewritten --></c:chartSpace>".to_vec());
    let out = carry(&original, &written).unwrap();
    for n in [1, 2] {
        let style = format!("ppt/charts/style{n}.xml");
        assert_eq!(out.get(&style), original.get(&style), "{style} byte-equal");
        assert_eq!(out.get(&format!("ppt/charts/colors{n}.xml")), original.get(&format!("ppt/charts/colors{n}.xml")));
        assert_eq!(target_of(&out, &format!("ppt/charts/chart{n}.xml"), STYLE_REL), Some(style.clone()), "style referenced");
        assert_eq!(target_of(&out, &format!("ppt/charts/chart{n}.xml"), COLORS_REL), Some(format!("ppt/charts/colors{n}.xml")));
        assert_eq!(content_type(&out, &style).as_deref(), Some(STYLE_CT));
        assert_eq!(content_type(&out, &format!("ppt/charts/colors{n}.xml")).as_deref(), Some(COLORS_CT));
    }
    assert_engine_parts_untouched(&written, &out);
    assert_valid(&out, "carried charts");
}

#[test]
fn edit_preserve_a_part_the_edit_removed_does_not_come_back() {
    let original = deck_with_charts(&[1, 2], true);
    // The edit removed chart 2 (and the slide that held it): its style and colours stay gone.
    let mut written = deck_with_charts(&[1], false);
    written.remove("ppt/charts/chart2.xml");
    let out = carry(&original, &written).unwrap();
    // Chart 1 keeps its style only if every original chart is still there: chart 2 is not, so neither is carried.
    assert!(!out.keys().any(|n| n.contains("style") || n.contains("colors")), "nothing brought back: {:?}", out.keys().collect::<Vec<_>>());
    assert_eq!(out, written, "the saved package is exactly what the engine wrote");
    assert!(!out.contains_key("ppt/charts/chart2.xml"));
    // The same with only the removed chart in play: a single-chart original, chart gone.
    let original = deck_with_charts(&[1], true);
    let mut written = deck_with_charts(&[1], false);
    written.remove("ppt/charts/chart1.xml");
    written.insert("ppt/_rels/presentation.xml.rels".into(), rels_xml(&[]).into_bytes());
    let out = carry(&original, &written).unwrap();
    assert_eq!(out, written);
}

#[test]
fn edit_preserve_never_adds_over_a_part_or_a_relationship_the_engine_wrote() {
    let original = deck_with_charts(&[1], true);
    // The engine wrote its own style part under the same name, referenced.
    let mut written = deck_with_charts(&[1], true);
    written.insert("ppt/charts/style1.xml".into(), b"<cs:chartStyle xmlns:cs=\"c\" id=\"engine\"/>".to_vec());
    let out = carry(&original, &written).unwrap();
    assert_eq!(out, written, "the engine's style stays");
    // The engine wrote a style under another name and the relationship: no second style relationship.
    let mut written = deck_with_charts(&[1], false);
    written.insert("ppt/charts/styleX.xml".into(), b"<cs:chartStyle xmlns:cs=\"c\"/>".to_vec());
    written.insert("ppt/charts/_rels/chart1.xml.rels".into(), rels_xml(&[("rId9", STYLE_REL, "styleX.xml")]).into_bytes());
    let out = carry(&original, &written).unwrap();
    let r = rels(&s(&out["ppt/charts/_rels/chart1.xml.rels"]));
    assert_eq!(r.iter().filter(|x| x.1 == STYLE_REL).count(), 1, "one style relationship");
    assert_eq!(r.iter().find(|x| x.1 == STYLE_REL).unwrap().2, "styleX.xml");
}

#[test]
fn edit_preserve_pptm_macro_comes_back_with_the_macro_enabled_main_type() {
    let mut original = deck_with_charts(&[], false);
    original.insert("ppt/vbaProject.bin".into(), VBA.to_vec());
    replace_in(&mut original, "ppt/_rels/presentation.xml.rels", "</Relationships>", &format!("<Relationship Id=\"rId77\" Type=\"{VBA_REL}\" Target=\"vbaProject.bin\"/></Relationships>"));
    replace_in(&mut original, "[Content_Types].xml", "</Types>", "<Override PartName=\"/ppt/vbaProject.bin\" ContentType=\"application/vnd.ms-office.vbaProject\"/></Types>");
    replace_in(&mut original, "[Content_Types].xml", "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml", PPTM_CT);
    // The engine writes the same deck as a plain .pptx.
    let written = deck_with_charts(&[], false);
    let out = carry(&original, &written).unwrap();
    assert_eq!(out.get("ppt/vbaProject.bin").map(Vec::as_slice), Some(VBA));
    assert_eq!(target_of(&out, "ppt/presentation.xml", VBA_REL).as_deref(), Some("ppt/vbaProject.bin"));
    assert_eq!(content_type(&out, "ppt/presentation.xml").as_deref(), Some(PPTM_CT));
    assert_eq!(rels(&s(&out["ppt/_rels/presentation.xml.rels"])).iter().find(|r| r.1 == VBA_REL).unwrap().0, "rId77", "the original id is kept when free");
    assert_engine_parts_untouched(&written, &out);
    assert_valid(&out, "carried pptm");
}

#[test]
fn edit_preserve_a_macro_that_cannot_be_carried_refuses_the_save() {
    // The macro hangs on a part the engine dropped: the save is `Unsupported`, never a silent loss.
    let mut original = deck_with_charts(&[], false);
    original.insert("ppt/vbaProject.bin".into(), VBA.to_vec());
    original.insert("ppt/_rels/gone.xml.rels".into(), rels_xml(&[("rId1", VBA_REL, "vbaProject.bin")]).into_bytes());
    original.insert("ppt/gone.xml".into(), b"<g/>".to_vec());
    replace_in(&mut original, "[Content_Types].xml", "</Types>", "<Override PartName=\"/ppt/vbaProject.bin\" ContentType=\"application/vnd.ms-office.vbaProject\"/></Types>");
    let written = deck_with_charts(&[], false);
    assert_eq!(carry(&original, &written).unwrap_err(), SaveError::Unsupported);
    // And a package that is not a zip.
    assert_eq!(preserve::carry(b"nope", zip_of(&written)).unwrap_err(), SaveError::Unsupported);
}

#[test]
fn edit_preserve_with_nothing_to_carry_the_saved_bytes_are_returned_as_they_are() {
    let original = deck_with_charts(&[1], true);
    let written = zip_of(&deck_with_charts(&[1], true));
    assert_eq!(preserve::carry(&zip_of(&original), written.clone()).unwrap(), written);
}

#[test]
fn edit_preserve_parts_that_would_be_stale_or_dangling_are_left_out() {
    // calcChain, a thumbnail and notes the engine does not write are not brought back, and nothing refuses.
    let mut original = deck_with_charts(&[], false);
    original.insert("docProps/thumbnail.jpeg".into(), b"\xFF\xD8old".to_vec());
    original.insert("xl/calcChain.xml".into(), b"<calcChain/>".to_vec());
    replace_in(&mut original, "_rels/.rels", "</Relationships>", "<Relationship Id=\"rIdT\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail\" Target=\"docProps/thumbnail.jpeg\"/></Relationships>");
    let written = deck_with_charts(&[], false);
    let out = carry(&original, &written).unwrap();
    assert_eq!(out, written);
}

// ---- the corpus ------------------------------------------------------------------------------------------

fn corpus_run(kind: EditKind, ext: &str, op: &Op) -> (usize, usize, usize) {
    let (mut opened, mut saved, mut refused) = (0, 0, 0);
    for (name, bytes) in corpus(ext) {
        let Ok(mut doc) = EditDoc::open(kind, &bytes, &Limits::default()) else { continue };
        opened += 1;
        // An edit may be refused on a file with restricted editing; the save is what is measured.
        let _ = doc.apply(op);
        match doc.save() {
            Ok(out) => {
                saved += 1;
                let (a, b) = (parts_of(&bytes), parts_of(&out));
                // Whatever came back from the original is byte-equal to it.
                let style = |n: &&String| n.contains("/charts/style") || n.contains("/charts/colors");
                let kept = a.keys().filter(style).filter(|n| b.contains_key(*n)).count();
                for n in b.keys().filter(style) {
                    if let Some(orig) = a.get(n) {
                        assert_eq!(&b[n], orig, "{name}: {n} byte-equal");
                    }
                }
                if a.keys().filter(style).count() > 0 {
                    eprintln!("DEBUG {name}: written chart parts {:?}", b.keys().filter(|n| n.contains("chart") || n.contains("embed")).collect::<Vec<_>>());
                    eprintln!("edit_preserve corpus: {name}: chart style parts {} in the original, {kept} kept", a.keys().filter(style).count());
                }
                assert!(EditDoc::open(kind, &out, &Limits::default()).is_ok(), "{name}: reopens");
            }
            Err(e) => {
                refused += 1;
                eprintln!("edit_preserve corpus: {ext} {name} refused: {e}");
            }
        }
    }
    (opened, saved, refused)
}

#[test]
fn edit_preserve_corpus_docx_none_refused() {
    let (opened, saved, refused) = corpus_run(EditKind::Docx, "docx", &Op::InsertText("ZQX ".into()));
    eprintln!("edit_preserve corpus docx: opened {opened}, saved {saved}, refused {refused}");
    assert!(opened >= 20);
    assert_eq!(refused, 0, "no corpus document is refused on save");
}

#[test]
fn edit_preserve_corpus_xlsx_none_refused() {
    let (opened, saved, refused) = corpus_run(EditKind::Xlsx, "xlsx", &Op::SetCell { sheet: 0, row: 0, col: 0, text: "1".into() });
    eprintln!("edit_preserve corpus xlsx: opened {opened}, saved {saved}, refused {refused}");
    assert!(opened >= 20);
    assert_eq!(refused, 0, "no corpus workbook is refused on save");
}

#[cfg(feature = "edit-slides")]
#[test]
fn edit_preserve_corpus_pptx_none_refused() {
    let (opened, saved, refused) = corpus_run(EditKind::Pptx, "pptx", &Op::InsertText("ZQX ".into()));
    eprintln!("edit_preserve corpus pptx: opened {opened}, saved {saved}, refused {refused}");
    assert!(opened >= 20);
    assert_eq!(refused, 0, "no corpus deck is refused on save");
}
