//! A picture whose fill is markup-compatibility alternate content (`mc:AlternateContent`, ECMA-376 Part 3
//! §10.2): the viewer draws the branch a reader that knows only what this one knows must take. A Choice whose
//! required namespace it does not know is skipped for the Fallback, as Word for Windows skips a Mac-only Choice.
//!
//! `corpus/docx/Bug51170.docx` (Apache POI, see `corpus/SOURCES.tsv`) is the real case: the default header's
//! logo has a Choice that requires the Mac drawing namespace (`ma`) and holds a PDF, and a Fallback that holds a
//! PNG. Taking the PDF left an undecodable picture on every page that shows the header (pages 2 to 7; page 1
//! is a title page with no first-page header), so 6 of its 7 pages said content was missing (O-CP3 corpus).
//! Its pages are written to `BB_OUT` (`bug51170-page-<n>.png`) for the LOOK.

mod common;

use std::path::Path;

use beebeeb_office::{DocKind, Document, Limits};
use common::{NS_A, NS_PIC, REL_IMAGE, canary_pixels, canary_png, docx_package, render};
use wordcraft_layout::display::DisplayOptions;
use wordcraft_layout::{LayoutCache, LayoutOptions, StrictOutput};

const NS_MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
/// The Mac drawing namespace a Choice of Bug51170.docx requires: not one this reader knows.
const NS_MA: &str = "http://schemas.microsoft.com/office/mac/drawingml/2008/main";
/// The Office 2010 drawing namespace: one this reader knows (`a14`).
const NS_A14: &str = "http://schemas.microsoft.com/office/drawing/2010/main";

/// The first bytes of a PDF: no picture decoder here draws it, so a page showing it says content is missing.
const PDF: &[u8] = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";

/// One paragraph with a 2" x 2" inline picture whose `pic:blipFill` sits in alternate content: the Choice
/// requires `prefix` (bound to `ns`) and its blip is `choice_rel`; the Fallback's blip is `fallback_rel`.
fn alternate_picture(prefix: &str, ns: &str, choice_rel: &str, fallback_rel: &str) -> String {
    let fill = |rel: &str| format!(r#"<pic:blipFill><a:blip r:embed="{rel}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>"#);
    format!(
        r#"<w:p><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="1828800" cy="1828800"/><wp:docPr id="1" name="Picture 1"/><a:graphic xmlns:a="{NS_A}"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="{NS_PIC}"><pic:nvPicPr><pic:cNvPr id="1" name="logo"/><pic:cNvPicPr/></pic:nvPicPr><mc:AlternateContent xmlns:mc="{NS_MC}"><mc:Choice xmlns:{prefix}="{ns}" Requires="{prefix}">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1828800" cy="1828800"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#,
        fill(choice_rel),
        fill(fallback_rel),
    )
}

/// A document holding [`alternate_picture`], with the canary at `rIdCanary` and a PDF at `rIdPdf`.
fn alternate_docx(prefix: &str, ns: &str, choice_rel: &str, fallback_rel: &str) -> Vec<u8> {
    let body = alternate_picture(prefix, ns, choice_rel, fallback_rel);
    let rels = format!(
        r#"<Relationship Id="rIdCanary" Type="{REL_IMAGE}" Target="media/canary.png"/><Relationship Id="rIdPdf" Type="{REL_IMAGE}" Target="media/logo.pdf"/>"#
    );
    let canary = canary_png();
    docx_package(&body, &rels, &[("word/media/canary.png", canary.as_slice()), ("word/media/logo.pdf", PDF)])
}

#[test]
fn a_choice_that_needs_an_unknown_namespace_gives_way_to_the_fallback() {
    let bytes = alternate_docx("ma", NS_MA, "rIdPdf", "rIdCanary");
    let doc = Document::open(DocKind::Docx, &bytes, &Limits::default()).expect("opens");
    let (info, rgba) = render(&doc, 0, 1.0);
    let canary = canary_pixels(&rgba);
    assert!(
        !info.missing_content && canary > 10_000,
        "the Fallback PNG is drawn, nothing missing: missing_content {}, canary pixels {canary}",
        info.missing_content
    );
}

#[test]
fn a_choice_that_needs_a_known_namespace_is_taken() {
    // The control: the reader still takes a Choice it understands, over the Fallback.
    let bytes = alternate_docx("a14", NS_A14, "rIdCanary", "rIdPdf");
    let doc = Document::open(DocKind::Docx, &bytes, &Limits::default()).expect("opens");
    let (info, rgba) = render(&doc, 0, 1.0);
    let canary = canary_pixels(&rgba);
    assert!(
        !info.missing_content && canary > 10_000,
        "the Choice PNG is drawn, nothing missing: missing_content {}, canary pixels {canary}",
        info.missing_content
    );
}

fn bug51170() -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("corpus").join("docx").join("Bug51170.docx");
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Per page, what the reader and the layout say cannot be drawn, the inputs of `RenderInfo::missing_content`
/// (`Document::missing_on`): pictures the reader left out (every page), glyphs no font has, pictures that do
/// not decode.
fn what_is_missing(bytes: &[u8]) -> Vec<String> {
    let mut doc = wordcraft_docx::read_with(bytes, &wordcraft_docx::ReadOptions::default()).expect("the docx reader reads it");
    doc.ensure_nonempty();
    let layout = wordcraft_layout::layout(&doc, &mut LayoutCache::new(), &LayoutOptions::default());
    let display = DisplayOptions { dim_header: false, ..DisplayOptions::default() };
    let left_out = &doc.missing_pictures;
    let mut lines = vec![format!("left out by the reader: {} ({:?})", left_out.total, left_out.list)];
    for n in 0..layout.pages.len() {
        let glyphs = layout.missing_glyphs(&doc, &display, Some(n..=n));
        let (pictures, truncated, _) = wordcraft_layout::undecodable_pictures(&doc, &layout, Some(n..=n), &display, StrictOutput::Png);
        let pictures: Vec<String> = pictures.iter().map(|p| format!("{} in {} ({:?})", p.part, p.story, p.reason)).collect();
        lines.push(format!("page {n}: glyphs {:?}, pictures {pictures:?}, truncated {truncated}", glyphs.chars));
    }
    lines
}

/// Pixels of the CNIG logo's green (the fallback PNG's lettering) in the top `rows` rows of a page.
fn logo_green(rgba: &[u8], w: u32, rows: u32) -> usize {
    let end = (w as usize * rows as usize * 4).min(rgba.len());
    rgba[..end].chunks_exact(4).filter(|p| u16::from(p[1]) > u16::from(p[0]) + 40 && u16::from(p[1]) > u16::from(p[2]) + 20).count()
}

#[test]
fn bug51170_every_page_renders_with_nothing_missing() {
    let bytes = bug51170();
    let doc = Document::open(DocKind::Docx, &bytes, &Limits::default()).expect("Bug51170.docx opens");
    let pages = doc.info().pages;
    assert_eq!(pages, 7, "Bug51170.docx lays out as 7 pages (corpus/EXPECTED.tsv)");
    let mut flagged = Vec::new();
    for p in 0..pages {
        let (info, rgba) = render(&doc, p, 1.0);
        common::save_png(&format!("bug51170-page-{p}"), info.w, info.h, &rgba);
        if info.missing_content {
            flagged.push(p);
        }
    }
    let why = what_is_missing(&bytes);
    for line in &why {
        println!("bug51170: {line}");
    }
    println!("bug51170: pages clean {} of {pages}", pages as usize - flagged.len());
    assert!(flagged.is_empty(), "pages flagged as missing content: {flagged:?}\n{}", why.join("\n"));
}

#[test]
fn bug51170_the_header_logo_is_the_fallback_png_and_is_drawn() {
    let bytes = bug51170();
    let model = wordcraft_docx::read_with(&bytes, &wordcraft_docx::ReadOptions::default()).expect("the docx reader reads it");
    // The PDF of the `ma` Choice is never taken, so it is not the logo's media; the PNG of the Fallback is.
    let pdf = model.media.values().filter(|m| m.as_slice().starts_with(b"%PDF")).count();
    let png = model.media.values().filter(|m| m.as_slice().starts_with(b"\x89PNG")).count();
    assert_eq!((pdf, png), (0, 1), "media kept: {pdf} PDF, {png} PNG (the header logo must be the Fallback PNG)");

    // Drawn: the logo's green lettering is in the header band (the top 120 pt) of every page that has the
    // header (pages 2 to 7), and not on the title page, which has none.
    let doc = Document::open(DocKind::Docx, &bytes, &Limits::default()).expect("Bug51170.docx opens");
    let green: Vec<usize> = (0..doc.info().pages)
        .map(|p| {
            let (info, rgba) = render(&doc, p, 1.0);
            logo_green(&rgba, info.w, 120)
        })
        .collect();
    println!("bug51170: logo green pixels per page {green:?}");
    assert!(green[0] == 0 && green[1..].iter().all(|g| *g >= 500), "logo green pixels per page: {green:?}");
}
