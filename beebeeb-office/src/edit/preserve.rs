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
//! - a deck's printer settings and revision info (`ppt/presentation.xml` names neither by `r:id`);
//! - a deck's legacy VML drawing of an OLE object ([`carry_vml`]), on each written slide holding that object;
//! - with a baseline (the engine's no-edit save), charts and SmartArt parts the engine rewrote go back to the
//!   original's bytes when the edit did not touch them ([`restore`]): under the engine's name, so the `r:id` in the
//!   XML the engine wrote stays right, with the original's own relationships and parts (embedded workbook, style,
//!   colours, pictures). One value inside a part changes: SmartArt data's `dataModelExt relId` (an id of the
//!   slide's relationships, which the engine numbers its own way). OLE objects and embedded workbooks the engines
//!   keep byte-equal themselves (DeckCraft under its own names).
//!
//! - the document thumbnail (`docProps/thumbnail.*`, [`thumbnail`]): the original's bytes when the first page, slide
//!   or sheet is unchanged, else a fresh PNG of the first page (documents, decks) or none (workbooks have no
//!   sheet renderer), never the stale picture;
//! - a workbook's `calcChain` is not carried: GridCraft rewrites the sheets, so the original's list may name cells
//!   that moved or lost their formula, and Excel repairs a file with a wrong chain. It is dropped with its
//!   relationship and content type (Excel rebuilds it on open).
//!
//! Parts it deliberately does not carry: media and drawings the document XML names by `r:id` (the engine
//! rewrote that XML) other than the charts and SmartArt above. Notes slides are not carried either: the engine
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

impl Package {
    fn insert(&mut self, name: String, bytes: Vec<u8>) {
        if !self.parts.contains_key(&name) {
            self.order.push(name.clone());
        }
        self.parts.insert(name, bytes);
    }

    fn remove(&mut self, name: &str) {
        if self.parts.remove(name).is_some() {
            self.order.retain(|n| n != name);
        }
    }
}

#[derive(Clone, Debug)]
struct Rel {
    id: String,
    ty: String,
    target: String,
    external: bool,
}

/// Without a baseline (the unit tests and a document no engine reopens).
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn carry(original: &[u8], written: Vec<u8>) -> Result<Vec<u8>, SaveError> {
    carry_with(original, None, written)
}

/// [`carry`], and with `baseline` (the engine's save of the original with no edit) also [`restore`]: a chart or
/// SmartArt part the engine rewrote goes back to the original's bytes when the edit did not touch it.
pub(super) fn carry_with(original: &[u8], baseline: Option<&[u8]>, written: Vec<u8>) -> Result<Vec<u8>, SaveError> {
    let (Some(orig), Some(mut out)) = (read(original), read(&written)) else { return Err(SaveError::Unsupported) };
    let had_macro = orig.parts.iter().any(|(n, b)| is_rels(n) && parse_rels(b).iter().any(|r| !r.external && r.ty == REL_VBA));
    let mut ct = text(out.parts.get("[Content_Types].xml").ok_or(SaveError::Unsupported)?);
    let orig_ct = text(orig.parts.get("[Content_Types].xml").ok_or(SaveError::Unsupported)?);
    let mut changed = false;
    if let Some(base) = baseline.and_then(read) {
        changed |= restore(&orig, &orig_ct, &base, &mut out, &mut ct);
    }
    changed |= carry_vml(&orig, &orig_ct, &mut out, &mut ct);
    let plan = plan(&orig, &out);
    if plan.is_empty() && !changed {
        return if had_macro && !has_macro(&out) { Err(SaveError::Unsupported) } else { Ok(written) };
    }
    let mut rels: BTreeMap<String, String> = BTreeMap::new();
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

// ---- charts and SmartArt the engine rewrote ---------------------------------------------------------------

const REL_CHART: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart";
const REL_CHART_EX: &str = "http://schemas.microsoft.com/office/2014/relationships/chartEx";
const REL_DGM_DATA: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramData";
const REL_DGM_LAYOUT: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramLayout";
const REL_DGM_STYLE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramQuickStyle";
const REL_DGM_COLORS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramColors";
const REL_DGM_DRAWING: &str = "http://schemas.microsoft.com/office/2007/relationships/diagramDrawing";
const REL_VML: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing";
const REL_OLE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/oleObject";
const REL_IMAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";

/// Parts named by `r:id` from XML the engine writes (a slide, a drawing), which the engine writes again itself:
/// [`restore`] puts the original's bytes back under the engine's name, so the `r:id` stays right.
const RESTORED: &[&str] = &[REL_CHART, REL_CHART_EX, REL_DGM_DATA, REL_DGM_LAYOUT, REL_DGM_STYLE, REL_DGM_COLORS];

/// Whether [`carry_with`] can use a baseline for this original (it has a part [`restore`] handles).
pub(super) fn wants_baseline(original: &[u8]) -> bool {
    let Some(p) = read(original) else { return false };
    p.parts.iter().any(|(n, b)| is_rels(n) && parse_rels(b).iter().any(|r| !r.external && RESTORED.contains(&r.ty.as_str())))
}

/// Charts and SmartArt parts the engine rewrote (DeckCraft drops a SmartArt picture fill, GridCraft redraws a chart
/// from its model) go back to the original's bytes, with the original's own relationships and parts (embedded
/// workbook, chart style and colours, pictures), when the edit did not touch them.
///
/// "Did not touch" is measured, not assumed: `base` is the engine's save of the original with no edit. A written
/// part is the original's when its bytes equal the baseline's part at the same place in package order (the
/// same type, the same index in a walk of the relationships in document order, under the same kind of source).
/// A part the edit changed (a renamed sheet in a chart's formulas, new values in its cache) differs from the
/// baseline and stays the engine's. The written part's name is kept, so the `r:id` in the slide or drawing the
/// engine wrote stays right; the engine's own relationship targets of that part go when nothing else uses them.
fn restore(orig: &Package, orig_ct: &str, base: &Package, out: &mut Package, ct: &mut String) -> bool {
    let mut changed = false;
    for ty in RESTORED {
        let in_orig = walk(orig, ty);
        let in_base = walk(base, ty);
        if in_orig.len() != in_base.len() || in_orig.iter().zip(&in_base).any(|(a, b)| class(&a.0) != class(&b.0)) {
            continue;
        }
        for (source, _, part) in rels_of_type(out, ty) {
            let Some(bytes) = out.parts.get(&part) else { continue };
            let from: BTreeSet<&String> = in_base.iter().zip(&in_orig).filter(|(b, _)| base.parts.get(&b.1) == Some(bytes)).map(|(_, o)| &o.1).collect();
            // Two original parts the engine writes alike: which one this is cannot be told.
            let Some(o) = from.iter().next().copied() else { continue };
            if from.iter().any(|x| orig.parts.get(*x) != orig.parts.get(o)) {
                continue;
            }
            if same_with_rels(orig, o, out, &part) {
                continue;
            }
            if *ty == REL_DGM_DATA {
                changed |= restore_diagram_data(orig, orig_ct, o, out, ct, &source, &part);
                continue;
            }
            put_back(orig, orig_ct, o, out, ct, &part, orig.parts[o].clone());
            changed = true;
        }
    }
    changed
}

/// SmartArt data names its drawing by the id of a relationship of the slide (`dsp:dataModelExt relId`), and the
/// engine numbers the slide's relationships its own way: the data comes back with that one id set to the
/// engine's, and the drawing it names comes back with it. Without a drawing on both sides nothing changes.
#[allow(clippy::too_many_arguments)]
fn restore_diagram_data(orig: &Package, orig_ct: &str, o: &str, out: &mut Package, ct: &mut String, source: &str, part: &str) -> bool {
    let (Some(o_bytes), Some(w_bytes)) = (orig.parts.get(o), out.parts.get(part)) else { return false };
    let (o_xml, w_xml) = (String::from_utf8_lossy(o_bytes).into_owned(), String::from_utf8_lossy(w_bytes).into_owned());
    let (Some((os, oe)), Some((ws, we))) = (drawing_rel_id(&o_xml), drawing_rel_id(&w_xml)) else { return false };
    let (o_id, w_id) = (&o_xml[os..oe], &w_xml[ws..we]);
    // The original's drawing: the relationship `o_id` of the part that names this data.
    let Some(o_source) = orig.parts.iter().filter(|(n, _)| is_rels(n)).find_map(|(n, b)| {
        let s = source_of(n)?;
        parse_rels(b).iter().any(|r| !r.external && r.ty == REL_DGM_DATA && resolve(orig, &s, &r.target).as_deref() == Some(o)).then_some(s)
    }) else {
        return false;
    };
    let find = |p: &Package, s: &str, id: &str| -> Option<String> {
        let r = parse_rels(p.parts.get(&rels_name_of(s))?).into_iter().find(|r| r.id == id && !r.external && r.ty == REL_DGM_DRAWING)?;
        resolve(p, s, &r.target)
    };
    let (Some(o_drawing), Some(w_drawing)) = (find(orig, &o_source, o_id), find(out, source, w_id)) else { return false };
    let mut bytes = o_xml.clone();
    if o_id != w_id {
        bytes.replace_range(os..oe, w_id);
    }
    let bytes = if o_id == w_id { o_bytes.clone() } else { bytes.into_bytes() };
    put_back(orig, orig_ct, o, out, ct, part, bytes);
    if !same_with_rels(orig, &o_drawing, out, &w_drawing) {
        put_back(orig, orig_ct, &o_drawing, out, ct, &w_drawing, orig.parts[&o_drawing].clone());
    }
    true
}

/// The byte range of the value of `relId` in a SmartArt data part's `dataModelExt` element.
fn drawing_rel_id(xml: &str) -> Option<(usize, usize)> {
    let at = xml.find("dataModelExt")?;
    let end = at + xml[at..].find('>')?;
    let key = "relId=\"";
    let s = at + xml[at..end].find(key)? + key.len();
    let e = s + xml[s..end].find('"')?;
    Some((s, e))
}

/// `part` in `out` already holds `o`'s bytes and its relationships reach parts with the same bytes.
fn same_with_rels(orig: &Package, o: &str, out: &Package, part: &str) -> bool {
    let inner = |p: &Package, n: &str| -> Option<Vec<(String, Vec<u8>)>> {
        let mut v = Vec::new();
        for r in p.parts.get(&rels_name_of(n)).map(|b| parse_rels(b)).unwrap_or_default() {
            if r.external {
                continue;
            }
            v.push((r.ty.clone(), p.parts.get(&resolve(p, n, &r.target)?)?.clone()));
        }
        v.sort();
        Some(v)
    };
    orig.parts.get(o) == out.parts.get(part) && inner(orig, o).is_some() && inner(orig, o) == inner(out, part)
}

/// `bytes` (the original part `o`) at `part` in `out`, with `o`'s relationships and every part they reach copied
/// from the original. The engine's relationships of `part` go, and so do their targets when nothing else uses them.
fn put_back(orig: &Package, orig_ct: &str, o: &str, out: &mut Package, ct: &mut String, part: &str, bytes: Vec<u8>) {
    let rn = rels_name_of(part);
    let old: Vec<String> = out.parts.get(&rn).map(|b| parse_rels(b)).unwrap_or_default().into_iter().filter(|r| !r.external).filter_map(|r| resolve(out, part, &r.target)).collect();
    out.remove(&rn);
    collect_garbage(out, ct, old);
    out.insert(part.to_string(), bytes);
    if let Some(want) = content_type_of(orig_ct, o)
        && content_type_of(ct, part).as_deref() != Some(want.as_str())
        && let Some(x) = set_override(ct, part, &want)
    {
        *ct = x;
    }
    copy_rels(orig, orig_ct, o, out, ct, part, 0);
}

/// Copies the relationships of the original part `o` to `at` in `out`, and the parts they reach (recursively),
/// each under its own name when free, a fresh name otherwise; a picture already in `out` with the same bytes is
/// shared. Targets are rewritten only in the `.rels` files.
fn copy_rels(orig: &Package, orig_ct: &str, o: &str, out: &mut Package, ct: &mut String, at: &str, depth: usize) {
    let Some(rb) = orig.parts.get(&rels_name_of(o)) else { return };
    let mut list: Vec<(String, String, String, bool)> = Vec::new();
    let mut todo: Vec<(String, String)> = Vec::new();
    for r in parse_rels(rb) {
        if r.external {
            list.push((r.id, r.ty, r.target, true));
            continue;
        }
        let Some(t) = resolve(orig, o, &r.target) else { continue };
        let tb = &orig.parts[&t];
        let name = if r.ty == REL_IMAGE && out.parts.get(&t) == Some(tb) {
            t.clone()
        } else {
            let n = fresh_name(out, &t);
            out.insert(n.clone(), tb.clone());
            if let Some(want) = content_type_of(orig_ct, &t)
                && content_type_of(ct, &n).as_deref() != Some(want.as_str())
                && let Some(x) = set_override(ct, &n, &want)
            {
                *ct = x;
            }
            if depth < MAX_DEPTH {
                todo.push((t.clone(), n.clone()));
            }
            n
        };
        list.push((r.id, r.ty, relative(at, &name), false));
    }
    out.insert(rels_name_of(at), rels_text(&list).into_bytes());
    for (t, n) in todo {
        copy_rels(orig, orig_ct, &t, out, ct, &n, depth + 1);
    }
}

/// `name` when `out` has no such part, else `dir/stem_keptN.ext`.
fn fresh_name(out: &Package, name: &str) -> String {
    if !out.parts.contains_key(name) && !out.parts.contains_key(&rels_name_of(name)) {
        return name.to_string();
    }
    let (stem, ext) = name.rsplit_once('.').map(|(a, b)| (a, format!(".{b}"))).unwrap_or((name, String::new()));
    (1..).map(|i| format!("{stem}_kept{i}{ext}")).find(|n| !out.parts.contains_key(n) && !out.parts.contains_key(&rels_name_of(n))).expect("a name")
}

/// Removes each of `parts` that no relationship in `out` reaches any more, with its relationships, its content
/// type override and, in turn, what only it reached.
fn collect_garbage(out: &mut Package, ct: &mut String, mut parts: Vec<String>) {
    while let Some(p) = parts.pop() {
        if !out.parts.contains_key(&p) || referenced(out, &p) {
            continue;
        }
        out.remove(&p);
        if let Some(x) = remove_override(ct, &p) {
            *ct = x;
        }
        let rn = rels_name_of(&p);
        if let Some(b) = out.parts.get(&rn).cloned() {
            out.remove(&rn);
            for r in parse_rels(&b) {
                if let (false, Some(t)) = (r.external, resolve(out, &p, &r.target)) {
                    parts.push(t);
                }
            }
        }
    }
}

fn referenced(p: &Package, name: &str) -> bool {
    p.parts.iter().filter(|(n, _)| is_rels(n)).any(|(n, b)| {
        let Some(s) = source_of(n) else { return false };
        parse_rels(b).iter().any(|r| !r.external && resolve(p, &s, &r.target).as_deref() == Some(name))
    })
}

/// (source, relationship, target) of every internal relationship of type `ty`, in name order.
fn rels_of_type(p: &Package, ty: &str) -> Vec<(String, Rel, String)> {
    let mut v = Vec::new();
    for (n, b) in p.parts.iter().filter(|(n, _)| is_rels(n)) {
        let Some(s) = source_of(n) else { continue };
        for r in parse_rels(b) {
            if !r.external && r.ty == ty
                && let Some(t) = resolve(p, &s, &r.target)
            {
                v.push((s.clone(), r, t));
            }
        }
    }
    v
}

/// The (source, target) of every part reached by a relationship of type `ty`, in the order a walk from the
/// package root meets them: a part's relationships in the order their ids first appear in its XML, the rest after.
fn walk(p: &Package, ty: &str) -> Vec<(String, String)> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut found: Vec<(String, String)> = Vec::new();
    let mut stack: Vec<String> = vec![String::new()];
    seen.insert(String::new());
    // Depth first, in order: push children in reverse.
    while let Some(part) = stack.pop() {
        let Some(rb) = p.parts.get(&rels_name_of(&part)) else { continue };
        let xml = p.parts.get(&part).filter(|_| part.ends_with(".xml")).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
        let mut rs: Vec<(usize, usize, Rel)> =
            parse_rels(rb).into_iter().enumerate().map(|(i, r)| (xml.find(&format!("\"{}\"", r.id)).unwrap_or(usize::MAX), i, r)).collect();
        rs.sort_by_key(|(at, i, _)| (*at, *i));
        let mut next = Vec::new();
        for (_, _, r) in rs {
            if r.external {
                continue;
            }
            let Some(t) = resolve(p, &part, &r.target) else { continue };
            if r.ty == ty && !found.iter().any(|f| f.1 == t) {
                found.push((part.clone(), t.clone()));
            }
            if seen.insert(t.clone()) {
                next.push(t);
            }
        }
        stack.extend(next.into_iter().rev());
    }
    found
}

/// `ppt/slides/slide12.xml` -> `ppt/slides/slideN.xml`.
fn class(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars() {
        if c.is_ascii_digit() {
            if !s.ends_with('N') {
                s.push('N');
            }
        } else {
            s.push(c);
        }
    }
    s
}

// ---- the VML drawing of a deck's OLE objects --------------------------------------------------------------

/// A slide's legacy VML drawing (the OLE object's VML shape and preview) is named by no `r:id`: PowerPoint links
/// it by shape id. DeckCraft keeps the OLE object and drops the drawing. It comes back, with its picture, on every
/// written slide that holds an OLE object of the original slide (same bytes) and has no VML drawing of its own.
fn carry_vml(orig: &Package, orig_ct: &str, out: &mut Package, ct: &mut String) -> bool {
    let mut changed = false;
    for (source, rel, vml) in rels_of_type(orig, REL_VML) {
        if !source.starts_with("ppt/slides/") {
            continue;
        }
        let oles: Vec<&Vec<u8>> = rels_of_type(orig, REL_OLE).into_iter().filter(|x| x.0 == source).filter_map(|x| orig.parts.get(&x.2)).collect();
        if oles.is_empty() {
            continue;
        }
        let targets: Vec<String> = rels_of_type(out, REL_OLE)
            .into_iter()
            .filter(|x| x.0.starts_with("ppt/slides/") && out.parts.get(&x.2).is_some_and(|b| oles.contains(&b)))
            .map(|x| x.0)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for s in targets {
            let rn = rels_name_of(&s);
            let cur = out.parts.get(&rn).map(|b| text(b)).unwrap_or_else(empty_rels);
            let have = parse_rels(cur.as_bytes());
            if have.iter().any(|r| r.ty == REL_VML) {
                continue;
            }
            let name = fresh_name(out, &vml);
            out.insert(name.clone(), orig.parts[&vml].clone());
            if let Some(want) = content_type_of(orig_ct, &vml)
                && content_type_of(ct, &name).as_deref() != Some(want.as_str())
                && let Some(x) = set_override(ct, &name, &want)
            {
                *ct = x;
            }
            copy_rels(orig, orig_ct, &vml, out, ct, &name, 0);
            let used: BTreeSet<String> = have.into_iter().map(|r| r.id).collect();
            let Some(new) = add_rel(&cur, &fresh_id(&rel.id, &used), REL_VML, &relative(&s, &name)) else { continue };
            out.insert(rn, new.into_bytes());
            changed = true;
        }
    }
    changed
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

fn rels_text(list: &[(String, String, String, bool)]) -> String {
    let mut x = empty_rels();
    let body: String = list
        .iter()
        .map(|(id, ty, t, ext)| format!("<Relationship Id=\"{}\" Type=\"{}\" Target=\"{}\"{}/>", esc(id), esc(ty), esc(t), if *ext { " TargetMode=\"External\"" } else { "" }))
        .collect();
    let i = x.rfind("</Relationships>").expect("the close tag");
    x.insert_str(i, &body);
    x
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

/// `ct` without an override for `name`; `None` when it has none.
fn remove_override(ct: &str, name: &str) -> Option<String> {
    let part = format!("/{name}");
    let (s, e, _) = std::iter::successors(find_element(ct, "Override", 0), |(_, e, _)| find_element(ct, "Override", *e))
        .find(|(_, _, a)| a.iter().any(|(k, v)| k == "PartName" && v.eq_ignore_ascii_case(&part)))?;
    let mut x = ct.to_string();
    x.replace_range(s..e, "");
    Some(x)
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

// ---- the document thumbnail ---------------------------------------------------------------------------------

const REL_THUMBNAIL: &str = "http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail";

/// What [`thumbnail`] puts in the saved package.
pub(super) enum Thumb {
    /// The original's thumbnail, byte for byte (its first page is unchanged).
    Keep,
    /// A PNG of the first page as saved.
    Fresh(Vec<u8>),
    /// None: the original's picture is stale and nothing can draw a new one.
    Drop,
}

/// Whether the original package has a thumbnail.
pub(super) fn has_thumbnail(original: &[u8]) -> bool {
    read(original).is_some_and(|p| thumb_of(&p).is_some())
}

/// The thumbnail relationship of the package: (relationship id, part name).
fn thumb_of(p: &Package) -> Option<(String, String)> {
    let rels = p.parts.get("_rels/.rels")?;
    let r = parse_rels(rels).into_iter().find(|r| !r.external && r.ty == REL_THUMBNAIL)?;
    let name = resolve(p, "", &r.target)?;
    Some((r.id, name))
}

/// Replaces whatever thumbnail the saved package has (the engine keeps a stale one or drops it) by `want`.
/// Only `docProps/thumbnail.*`, `_rels/.rels` and `[Content_Types].xml` change.
pub(super) fn thumbnail(original: &[u8], saved: Vec<u8>, want: Thumb) -> Result<Vec<u8>, SaveError> {
    let (Some(orig), Some(mut out)) = (read(original), read(&saved)) else { return Err(SaveError::Unsupported) };
    let old = thumb_of(&out);
    if matches!(want, Thumb::Keep) && old.is_some() && old == thumb_of(&orig) && old.as_ref().is_some_and(|(_, n)| out.parts.get(n) == orig.parts.get(n)) {
        return Ok(saved);
    }
    if matches!(want, Thumb::Drop) && old.is_none() {
        return Ok(saved);
    }
    let mut ct = text(out.parts.get("[Content_Types].xml").ok_or(SaveError::Unsupported)?);
    let orig_ct = text(orig.parts.get("[Content_Types].xml").ok_or(SaveError::Unsupported)?);
    let mut rels = text(out.parts.get("_rels/.rels").ok_or(SaveError::Unsupported)?);
    // Every thumbnail relationship and its part go.
    let mut from = 0;
    while let Some((s, e, attrs)) = find_element(&rels, "Relationship", from) {
        if attrs.iter().any(|(k, v)| k == "Type" && v == REL_THUMBNAIL) {
            rels.replace_range(s..e, "");
            from = s;
        } else {
            from = e;
        }
    }
    if let Some((_, name)) = &old {
        out.remove(name);
        if let Some(x) = remove_override(&ct, name) {
            ct = x;
        }
    }
    let (name, bytes, ty) = match want {
        Thumb::Drop => (None, Vec::new(), None),
        Thumb::Keep => match thumb_of(&orig) {
            Some((_, n)) => (Some(n.clone()), orig.parts[&n].clone(), content_type_of(&orig_ct, &n)),
            None => (None, Vec::new(), None),
        },
        Thumb::Fresh(b) => (Some("docProps/thumbnail.png".to_string()), b, Some("image/png".to_string())),
    };
    if let (Some(name), Some(ty)) = (name, ty) {
        if out.parts.contains_key(&name) {
            return Err(SaveError::Unsupported);
        }
        let used: BTreeSet<String> = parse_rels(rels.as_bytes()).into_iter().map(|r| r.id).collect();
        let id = fresh_id("rId1", &used);
        rels = add_rel(&rels, &id, REL_THUMBNAIL, &name).ok_or(SaveError::Unsupported)?;
        if content_type_of(&ct, &name).as_deref() != Some(ty.as_str()) {
            ct = set_override(&ct, &name, &ty).ok_or(SaveError::Unsupported)?;
        }
        out.insert(name, bytes);
    }
    out.parts.insert("_rels/.rels".into(), rels.into_bytes());
    out.parts.insert("[Content_Types].xml".into(), ct.into_bytes());
    write(&out).ok_or(SaveError::Engine)
}

/// A PNG (8-bit RGB, stored deflate blocks: no compressor needed for a small thumbnail) of an RGBA raster,
/// composited over white.
pub(super) fn png_of_rgba(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((w as usize * 3 + 1) * h as usize);
    for row in rgba.chunks_exact(w as usize * 4).take(h as usize) {
        raw.push(0);
        for px in row.chunks_exact(4) {
            let a = px[3] as u32;
            for c in &px[..3] {
                raw.push(((*c as u32 * a + 255 * (255 - a)) / 255) as u8);
            }
        }
    }
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(c) = chunks.next() {
        z.push(u8::from(chunks.peek().is_none()));
        z.extend_from_slice(&(c.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        z.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for x in &raw {
        a = (a + *x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    for (ty, data) in [(b"IHDR", ihdr.as_slice()), (b"IDAT", z.as_slice()), (b"IEND", &[][..])] {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = ty.to_vec();
        body.extend_from_slice(data);
        png.extend_from_slice(&body);
        png.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    png
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for b in data {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}
