//! Carries whole parts the engines do not write from the original package into the saved one.
//!
//! What it does (and the README, "Editor: what a save keeps", lists what it does not):
//! - macros: the macro project (`vbaProject.bin`, its signature parts and `vbaData.xml`) of `.docm` and `.pptm`
//!   (Excel's engine keeps it already), with its relationship and content type, and the macro-enabled content
//!   type of the main part;
//! - chart style and colour parts (`chartStyle`, `chartColorStyle`), for a chart the saved package still has.
//!
//! The rules that keep the result a valid package that no edit has made wrong:
//! - Only parts reached by an *implicit* relationship: nothing in any part's XML names them by `r:id`, so no
//!   XML inside a part is read or written. Only `[Content_Types].xml` and `.rels` files (package plumbing) change.
//! - The relationship's source part must still be in the saved package under the same name. A part whose source
//!   the edit removed (a deleted slide, a chart the engine dropped) is not brought back.
//! - A part is never added over a part the saved package already has under that name, nor next to a
//!   relationship of the same type the engine wrote itself.
//! - Everything the engine wrote stays byte for byte; with nothing to carry the saved bytes come back as they are.
//!
//! - a deck's printer settings and revision info (`ppt/presentation.xml` names neither by `r:id`).
//!
//! Parts it deliberately does not carry: `calcChain` and thumbnails (stale after any edit, and a thumbnail of
//! a deleted slide would keep showing it), media and drawings the document XML names by `r:id` (the engine
//! rewrote that XML), embedded workbooks and OLE objects. Notes slides are not carried either: the engine
//! writes a slide's notes itself (the text survives an edit); only a notes slide with no text is not written,
//! and the slides renumber under an edit, so a part kept by name could land on the wrong slide. A macro project that cannot be carried makes the save `Unsupported`: a macro is
//! never dropped silently.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};

use super::SaveError;

const MAX_PACKAGE_BYTES: u64 = 1 << 30;
const MAX_DEPTH: usize = 4;

const REL_OFFICE_DOCUMENT: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
const REL_VBA: &str = "http://schemas.microsoft.com/office/2006/relationships/vbaProject";
const REL_WORD_VBA_DATA: &str = "http://schemas.microsoft.com/office/2006/relationships/wordVbaData";
const REL_CHART_STYLE: &str = "http://schemas.microsoft.com/office/2011/relationships/chartStyle";
const REL_CHART_COLORS: &str = "http://schemas.microsoft.com/office/2011/relationships/chartColorStyle";

const REL_PRINTER_SETTINGS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/printerSettings";
const REL_REVISION_INFO: &str = "http://schemas.microsoft.com/office/2015/10/relationships/revisionInfo";

/// Relationship types whose target is carried.
const CARRIED: &[&str] = &[REL_VBA, REL_WORD_VBA_DATA, REL_CHART_STYLE, REL_CHART_COLORS, REL_PRINTER_SETTINGS, REL_REVISION_INFO];

/// Types carried only from the presentation part: in a workbook or a document the XML names a printer
/// settings part by `r:id` (the engine rewrote that XML), in a deck nothing does.
fn only_from_presentation(ty: &str) -> bool {
    ty == REL_PRINTER_SETTINGS || ty == REL_REVISION_INFO
}

type Parts = BTreeMap<String, Vec<u8>>;

struct Package {
    /// Entry order as in the file.
    order: Vec<String>,
    parts: Parts,
}

#[derive(Clone, Debug)]
struct Rel {
    id: String,
    ty: String,
    target: String,
    external: bool,
}

pub(super) fn carry(original: &[u8], written: Vec<u8>) -> Result<Vec<u8>, SaveError> {
    let (Some(orig), Some(mut out)) = (read(original), read(&written)) else { return Err(SaveError::Unsupported) };
    let had_macro = orig.parts.iter().any(|(n, b)| is_rels(n) && parse_rels(b).iter().any(|r| !r.external && r.ty == REL_VBA));
    let plan = plan(&orig, &out);
    if plan.is_empty() {
        return if had_macro && !has_macro(&out) { Err(SaveError::Unsupported) } else { Ok(written) };
    }
    let mut rels: BTreeMap<String, String> = BTreeMap::new();
    let mut ct = text(out.parts.get("[Content_Types].xml").ok_or(SaveError::Unsupported)?);
    let orig_ct = text(orig.parts.get("[Content_Types].xml").ok_or(SaveError::Unsupported)?);
    let mut carried_macro = false;
    for c in &plan {
        for name in &c.parts {
            out.order.push(name.clone());
            out.parts.insert(name.clone(), orig.parts[name].clone());
            let want = content_type_of(&orig_ct, name).ok_or(SaveError::Unsupported)?;
            if content_type_of(&ct, name).as_deref() != Some(want.as_str()) {
                ct = set_override(&ct, name, &want).ok_or(SaveError::Unsupported)?;
            }
        }
        for (name, bytes) in &c.rels_files {
            out.order.push(name.clone());
            out.parts.insert(name.clone(), bytes.clone());
        }
        let rels_name = rels_name_of(&c.source);
        let cur = match rels.get(&rels_name) {
            Some(x) => x.clone(),
            None => out.parts.get(&rels_name).map(|b| text(b)).unwrap_or_else(empty_rels),
        };
        let used: BTreeSet<String> = parse_rels(cur.as_bytes()).into_iter().map(|r| r.id).collect();
        let id = fresh_id(&c.rel.id, &used);
        let new = add_rel(&cur, &id, &c.rel.ty, &relative(&c.source, &c.target)).ok_or(SaveError::Unsupported)?;
        rels.insert(rels_name, new);
        carried_macro |= c.rel.ty == REL_VBA;
    }
    for (name, xml) in rels {
        if !out.parts.contains_key(&name) {
            out.order.push(name.clone());
        }
        out.parts.insert(name, xml.into_bytes());
    }
    if carried_macro {
        make_main_macro_enabled(&orig, &orig_ct, &mut out, &mut ct)?;
    }
    out.parts.insert("[Content_Types].xml".into(), ct.into_bytes());
    if had_macro && !has_macro(&out) {
        return Err(SaveError::Unsupported);
    }
    write(&out).ok_or(SaveError::Engine)
}

/// One relationship to bring back: the whole part, what hangs off it, and the relationship from its source.
struct Carried {
    source: String,
    rel: Rel,
    /// Resolved name of the relationship's target.
    target: String,
    /// The target and every part reached from it, all from the original.
    parts: Vec<String>,
    /// The `.rels` files of those parts, verbatim.
    rels_files: Vec<(String, Vec<u8>)>,
}

fn plan(orig: &Package, out: &Package) -> Vec<Carried> {
    let mut plan: Vec<Carried> = Vec::new();
    let mut taken: BTreeSet<String> = BTreeSet::new();
    for (rels_name, bytes) in orig.parts.iter().filter(|(n, _)| is_rels(n)) {
        let Some(source) = source_of(rels_name) else { continue };
        for rel in parse_rels(bytes) {
            if rel.external || !CARRIED.contains(&rel.ty.as_str()) {
                continue;
            }
            let Some(target) = resolve(orig, &source, &rel.target) else { continue };
            if only_from_presentation(&rel.ty) && source != "ppt/presentation.xml" {
                continue;
            }
            // The edit removed (or renamed) the part the relationship hangs on.
            if !source.is_empty() && !out.parts.contains_key(&source) {
                continue;
            }
            // Charts are numbered: only when every chart of the original is still there under its name is
            // "chart 3" in the saved package the chart it was.
            if is_chart(&source) && !orig.parts.keys().filter(|n| is_chart(n)).all(|n| out.parts.contains_key(n)) {
                continue;
            }
            // The engine wrote this itself.
            let written_rels = out.parts.get(&rels_name_of(&source)).map(|b| parse_rels(b)).unwrap_or_default();
            if written_rels.iter().any(|r| !r.external && r.ty == rel.ty) {
                continue;
            }
            let Some((parts, rels_files)) = closure(orig, &target) else { continue };
            if parts.iter().chain(rels_files.iter().map(|(n, _)| n)).any(|n| out.parts.contains_key(n) && !taken.contains(n)) {
                continue;
            }
            // Two sources pointing at one part: the part goes in once.
            let parts_new: Vec<String> = parts.into_iter().filter(|n| !taken.contains(n)).collect();
            let rels_new: Vec<(String, Vec<u8>)> = rels_files.into_iter().filter(|(n, _)| !taken.contains(n)).collect();
            taken.extend(parts_new.iter().cloned());
            taken.extend(rels_new.iter().map(|(n, _)| n.clone()));
            plan.push(Carried { source: source.clone(), rel: rel.clone(), target, parts: parts_new, rels_files: rels_new });
        }
    }
    plan
}

/// The target and everything its own relationships reach (signatures of a macro project). `None` when a
/// relationship inside points at a part the original does not have (the result would dangle).
fn closure(orig: &Package, target: &str) -> Option<(Vec<String>, Vec<(String, Vec<u8>)>)> {
    let mut parts = vec![target.to_string()];
    let mut rels_files = Vec::new();
    let mut queue = vec![(target.to_string(), 0usize)];
    while let Some((part, depth)) = queue.pop() {
        let rn = rels_name_of(&part);
        let Some(bytes) = orig.parts.get(&rn) else { continue };
        if depth >= MAX_DEPTH {
            return None;
        }
        rels_files.push((rn, bytes.clone()));
        for r in parse_rels(bytes) {
            if r.external {
                continue;
            }
            let t = resolve(orig, &part, &r.target)?;
            if !parts.contains(&t) {
                parts.push(t.clone());
                queue.push((t, depth + 1));
            }
        }
    }
    Some((parts, rels_files))
}

fn make_main_macro_enabled(orig: &Package, orig_ct: &str, out: &mut Package, ct: &mut String) -> Result<(), SaveError> {
    let main = |p: &Package| -> Option<String> {
        let r = parse_rels(p.parts.get("_rels/.rels")?).into_iter().find(|r| !r.external && r.ty == REL_OFFICE_DOCUMENT)?;
        resolve(p, "", &r.target)
    };
    let (Some(from), Some(to)) = (main(orig), main(out)) else { return Err(SaveError::Unsupported) };
    let Some(want) = content_type_of(orig_ct, &from) else { return Err(SaveError::Unsupported) };
    if !want.contains("macroEnabled") {
        return Ok(());
    }
    if content_type_of(ct, &to).as_deref() != Some(want.as_str()) {
        *ct = set_override(ct, &to, &want).ok_or(SaveError::Unsupported)?;
    }
    Ok(())
}

fn has_macro(p: &Package) -> bool {
    p.parts.iter().any(|(n, b)| {
        is_rels(n)
            && source_of(n).is_some_and(|s| parse_rels(b).iter().any(|r| !r.external && r.ty == REL_VBA && resolve(p, &s, &r.target).is_some()))
    })
}

fn is_chart(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("ppt/charts/").or_else(|| name.strip_prefix("xl/charts/")).or_else(|| name.strip_prefix("word/charts/")) else {
        return false;
    };
    rest.starts_with("chart") && rest.ends_with(".xml") && !rest.contains('/')
}

// ---- the zip --------------------------------------------------------------------------------------------

fn read(bytes: &[u8]) -> Option<Package> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut order = Vec::new();
    let mut parts = Parts::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let f = zip.by_index(i).ok()?;
        if f.is_dir() {
            continue;
        }
        let name = f.name().to_string();
        total += f.size();
        if total > MAX_PACKAGE_BYTES {
            return None;
        }
        let mut b = Vec::with_capacity(f.size().min(1 << 24) as usize);
        f.take(MAX_PACKAGE_BYTES).read_to_end(&mut b).ok()?;
        if parts.insert(name.clone(), b).is_none() {
            order.push(name);
        }
    }
    Some(Package { order, parts })
}

fn write(p: &Package) -> Option<Vec<u8>> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    // The content types first, as every Office writer does.
    let first = std::iter::once("[Content_Types].xml".to_string()).chain(p.order.iter().filter(|n| *n != "[Content_Types].xml").cloned());
    for name in first {
        let b = p.parts.get(&name)?;
        z.start_file(name, opts).ok()?;
        z.write_all(b).ok()?;
    }
    Some(z.finish().ok()?.into_inner())
}

// ---- names and relationships ----------------------------------------------------------------------------

fn is_rels(name: &str) -> bool {
    name.ends_with(".rels") && (name.starts_with("_rels/") || name.contains("/_rels/"))
}

/// `word/_rels/document.xml.rels` -> `word/document.xml`; `_rels/.rels` -> "" (the package).
fn source_of(rels: &str) -> Option<String> {
    let (dir, file) = match rels.rfind("/_rels/") {
        Some(i) => (&rels[..=i], &rels[i + 7..]),
        None => ("", rels.strip_prefix("_rels/")?),
    };
    let file = file.strip_suffix(".rels")?;
    Some(format!("{dir}{file}"))
}

fn rels_name_of(source: &str) -> String {
    match source.rfind('/') {
        Some(i) => format!("{}/_rels/{}.rels", &source[..i], &source[i + 1..]),
        None => format!("_rels/{source}.rels"),
    }
}

/// The name of the part a relationship from `source` points at, if the package has it.
fn resolve(p: &Package, source: &str, target: &str) -> Option<String> {
    let target = target.split('#').next().unwrap_or(target);
    let joined = match target.strip_prefix('/') {
        Some(abs) => abs.to_string(),
        None => {
            let dir = source.rfind('/').map(|i| &source[..=i]).unwrap_or("");
            format!("{dir}{target}")
        }
    };
    let mut segs: Vec<&str> = Vec::new();
    for s in joined.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                segs.pop()?;
            }
            s => segs.push(s),
        }
    }
    let name = segs.join("/");
    if p.parts.contains_key(&name) {
        return Some(name);
    }
    let decoded = percent_decode(&name);
    p.parts.contains_key(&decoded).then_some(decoded)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = if b[i] == b'%' && i + 2 < b.len() { s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) } else { None };
        match hex {
            Some(v) => {
                out.push(v);
                i += 3;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The relative target from `source` to the part `target`.
fn relative(source: &str, target: &str) -> String {
    let dir: Vec<&str> = source.split('/').collect();
    let dir = &dir[..dir.len() - 1];
    let t: Vec<&str> = target.split('/').collect();
    let mut common = 0;
    while common < dir.len() && common + 1 < t.len() && dir[common] == t[common] {
        common += 1;
    }
    let mut s = "../".repeat(dir.len() - common);
    s.push_str(&t[common..].join("/"));
    s
}

fn parse_rels(bytes: &[u8]) -> Vec<Rel> {
    let xml = String::from_utf8_lossy(bytes);
    elements(&xml, "Relationship")
        .into_iter()
        .filter_map(|a| {
            let get = |k: &str| a.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
            Some(Rel {
                id: get("Id")?,
                ty: get("Type")?,
                target: get("Target")?,
                external: get("TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External")),
            })
        })
        .collect()
}

fn empty_rels() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"></Relationships>".into()
}

fn fresh_id(want: &str, used: &BTreeSet<String>) -> String {
    if !want.is_empty() && !used.contains(want) {
        return want.to_string();
    }
    (1..).map(|i| format!("rIdKept{i}")).find(|i| !used.contains(i)).expect("an id")
}

fn add_rel(xml: &str, id: &str, ty: &str, target: &str) -> Option<String> {
    let tag = format!("<Relationship Id=\"{}\" Type=\"{}\" Target=\"{}\"/>", esc(id), esc(ty), esc(target));
    insert_into_root(xml, "Relationships", &tag)
}

// ---- [Content_Types].xml -------------------------------------------------------------------------------

/// The content type of part `name`: its override, else the default of its extension.
fn content_type_of(ct: &str, name: &str) -> Option<String> {
    let part = format!("/{name}");
    for a in elements(ct, "Override") {
        let get = |k: &str| a.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        if get("PartName").is_some_and(|p| p.eq_ignore_ascii_case(&part)) {
            return get("ContentType");
        }
    }
    let ext = name.rsplit_once('.')?.1;
    for a in elements(ct, "Default") {
        let get = |k: &str| a.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        if get("Extension").is_some_and(|e| e.eq_ignore_ascii_case(ext)) {
            return get("ContentType");
        }
    }
    None
}

/// `ct` with an override for `name` (any earlier one for the same part removed).
fn set_override(ct: &str, name: &str, content_type: &str) -> Option<String> {
    let part = format!("/{name}");
    let mut cleaned = ct.to_string();
    let mut from = 0;
    while let Some((s, e, attrs)) = find_element(&cleaned, "Override", from) {
        if attrs.iter().any(|(k, v)| k == "PartName" && v.eq_ignore_ascii_case(&part)) {
            cleaned.replace_range(s..e, "");
            from = s;
        } else {
            from = e;
        }
    }
    let tag = format!("<Override PartName=\"{}\" ContentType=\"{}\"/>", esc(&part), esc(content_type));
    insert_into_root(&cleaned, "Types", &tag)
}

// ---- a small reader for the flat XML of package plumbing ---------------------------------------------------

type Attrs = Vec<(String, String)>;

fn elements(xml: &str, name: &str) -> Vec<Attrs> {
    let mut v = Vec::new();
    let mut from = 0;
    while let Some((_, e, a)) = find_element(xml, name, from) {
        v.push(a);
        from = e;
    }
    v
}

/// The next `<name .../>` or `<name ...>` at or after `from`: start, end (just past `>`), attributes.
fn find_element(xml: &str, name: &str, from: usize) -> Option<(usize, usize, Attrs)> {
    let open = format!("<{name}");
    let mut at = from;
    loop {
        let i = at + xml.get(at..)?.find(&open)?;
        let after = i + open.len();
        match xml.as_bytes().get(after) {
            Some(b' ' | b'\t' | b'\r' | b'\n' | b'/' | b'>') => {}
            _ => {
                at = after;
                continue;
            }
        }
        let (end, attrs) = parse_attrs(xml, after)?;
        return Some((i, end, attrs));
    }
}

/// Attributes from `at` (just past the element name) to the closing `>`; returns the index past it.
fn parse_attrs(xml: &str, mut at: usize) -> Option<(usize, Attrs)> {
    let b = xml.as_bytes();
    let mut attrs = Attrs::new();
    loop {
        while at < b.len() && b[at].is_ascii_whitespace() {
            at += 1;
        }
        match b.get(at)? {
            b'>' => return Some((at + 1, attrs)),
            b'/' => at += 1,
            _ => {
                let ks = at;
                while at < b.len() && !b[at].is_ascii_whitespace() && !matches!(b[at], b'=' | b'/' | b'>') {
                    at += 1;
                }
                let key = xml[ks..at].to_string();
                while at < b.len() && b[at].is_ascii_whitespace() {
                    at += 1;
                }
                if b.get(at) != Some(&b'=') {
                    if at == ks {
                        return None;
                    }
                    continue;
                }
                at += 1;
                while at < b.len() && b[at].is_ascii_whitespace() {
                    at += 1;
                }
                let q = *b.get(at)?;
                if q != b'"' && q != b'\'' {
                    return None;
                }
                let vs = at + 1;
                let ve = vs + xml[vs..].find(q as char)?;
                attrs.push((key, unesc(&xml[vs..ve])));
                at = ve + 1;
            }
        }
    }
}

/// `tag` just before the root's closing tag (the root may be written `<Root .../>` or `<Root ...></Root>`).
fn insert_into_root(xml: &str, root: &str, tag: &str) -> Option<String> {
    let close = format!("</{root}>");
    if let Some(i) = xml.rfind(&close) {
        return Some(format!("{}{tag}{}", &xml[..i], &xml[i..]));
    }
    let (s, e, _) = find_element(xml, root, 0)?;
    let head = &xml[s..e];
    let inner = head.strip_suffix("/>")?;
    Some(format!("{}{}>{tag}{close}{}", &xml[..s], inner.trim_end(), &xml[e..]))
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim_start_matches('\u{feff}').to_string()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn unesc(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_preserve_names_and_targets() {
        assert_eq!(source_of("word/_rels/document.xml.rels").as_deref(), Some("word/document.xml"));
        assert_eq!(source_of("_rels/.rels").as_deref(), Some(""));
        assert_eq!(rels_name_of("ppt/charts/chart1.xml"), "ppt/charts/_rels/chart1.xml.rels");
        assert_eq!(rels_name_of(""), "_rels/.rels");
        assert_eq!(relative("word/document.xml", "word/vbaProject.bin"), "vbaProject.bin");
        assert_eq!(relative("ppt/charts/chart1.xml", "ppt/charts/style1.xml"), "style1.xml");
        assert_eq!(relative("", "customUI/x.xml"), "customUI/x.xml");
        assert_eq!(relative("ppt/charts/chart1.xml", "ppt/media/a.png"), "../media/a.png");
    }

    #[test]
    fn edit_preserve_flat_xml_reader() {
        let ct = "<Types xmlns='x'><Default Extension=\"xml\" ContentType=\"a/xml\"/><Override PartName=\"/a/b.xml\" ContentType=\"t/b\"/></Types>";
        assert_eq!(content_type_of(ct, "a/b.xml").as_deref(), Some("t/b"));
        assert_eq!(content_type_of(ct, "z.XML").as_deref(), Some("a/xml"));
        let ct2 = set_override(ct, "a/b.xml", "t/c").unwrap();
        assert_eq!(content_type_of(&ct2, "a/b.xml").as_deref(), Some("t/c"));
        assert_eq!(ct2.matches("/a/b.xml").count(), 1);
        let empty = "<?xml version=\"1.0\"?><Relationships xmlns=\"r\"/>";
        let r = add_rel(empty, "rId1", "T", "t.bin").unwrap();
        assert_eq!(parse_rels(r.as_bytes()).len(), 1);
    }

    #[test]
    fn edit_preserve_unreadable_packages_are_unsupported() {
        assert_eq!(carry(b"a", b"b".to_vec()).unwrap_err(), SaveError::Unsupported);
    }
}
