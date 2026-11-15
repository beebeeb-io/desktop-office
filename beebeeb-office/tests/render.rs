//! A Word page rendered from bytes below the engine (the viewer contract's first proof), and the
//! render call's refusals.

mod common;

use beebeeb_office::{DocKind, Document, Limits, OpenError, OutlineEntry, Rect, RenderError};
use common::{canary_pixels, colour_stats, dense_doc, dense_docx, dense_model, render, save_png};

/// The first proof on one opened file: the page count equals a layout made here, apart from the
/// crate; page 1 renders at scale 1.0 to its size in points, rounded up, with real content on it
/// (at least 50 colours, none over 99 % of the pixels), and nothing missing.
fn first_proof(name: &str, doc: &Document, own_layout_pages: usize) {
    let info = doc.info();
    assert_eq!(info.pages as usize, own_layout_pages, "{name}: the page count is the layout's");
    assert_eq!(info.pages, 2, "{name}: the fixture has two pages");
    assert_eq!(info.sizes_pt.len(), 2, "{name}: one size per page");
    let (r, px) = render(doc, 0, 1.0);
    let (w_pt, h_pt) = info.sizes_pt[0];
    assert_eq!((r.w, r.h), (w_pt.ceil() as u32, h_pt.ceil() as u32), "{name}: the raster is the page at 1 px per point");
    assert_eq!(px.len(), r.w as usize * r.h as usize * 4);
    save_png(name, r.w, r.h, &px);
    let (distinct, top) = colour_stats(&px);
    assert!(distinct >= 50, "{name}: {distinct} distinct colours, expected at least 50");
    assert!(top < 0.99, "{name}: one colour covers {:.2} % of the page", top * 100.0);
    assert!(!r.missing_content, "{name}: nothing on this page is missing");
    assert_eq!(canary_pixels(&px), 0, "{name}: the fixture has no magenta");
}

#[test]
fn a_docx_page_renders_below_the_engine() {
    let bytes = dense_docx();
    let doc = Document::open(DocKind::Docx, &bytes, &Limits::default()).expect("the dense .docx opens");
    let model = wordcraft_docx::read(&bytes).expect("the upstream reader reads it");
    let own = wordcraft_layout::layout(&model, &mut wordcraft_layout::LayoutCache::new(), &wordcraft_layout::LayoutOptions::default());
    first_proof("docx-page-1", &doc, own.pages.len());
}

#[test]
fn a_doc_page_renders_below_the_engine() {
    let bytes = dense_doc();
    let doc = Document::open(DocKind::Doc, &bytes, &Limits::default()).expect("the dense .doc opens");
    let model = wordcraft_docbin::read(&bytes).expect("the upstream reader reads it");
    let own = wordcraft_layout::layout(&model, &mut wordcraft_layout::LayoutCache::new(), &wordcraft_layout::LayoutOptions::default());
    first_proof("doc-page-1", &doc, own.pages.len());
}

#[test]
fn a_wrong_buffer_size_is_refused() {
    let doc = Document::open(DocKind::Docx, &dense_docx(), &Limits::default()).expect("opens");
    let (w, h) = doc.render_size(0, 1.0, None).expect("page 1 fits");
    let exact = w as usize * h as usize * 4;
    for len in [0, 4, exact - 4, exact - 1, exact + 1, exact + 4] {
        let mut out = vec![7u8; len];
        assert_eq!(doc.render_page(0, 1.0, None, &mut out), Err(RenderError::BufferSize), "a buffer of {len} bytes for {w} x {h}");
        assert!(out.iter().all(|&b| b == 7), "nothing is written when the buffer is refused ({len} bytes)");
    }
    let mut out = vec![0u8; exact];
    let info = doc.render_page(0, 1.0, None, &mut out).expect("the exact size renders");
    assert_eq!((info.w, info.h), (w, h));
    // A region is sized by its own w x h.
    let tile = Rect { x: 0, y: 0, w: 64, h: 32 };
    let mut short = vec![0u8; 64 * 32 * 4 - 4];
    assert_eq!(doc.render_page(0, 1.0, Some(tile), &mut short), Err(RenderError::BufferSize));
    let mut fits = vec![0u8; 64 * 32 * 4];
    assert!(doc.render_page(0, 1.0, Some(tile), &mut fits).is_ok(), "the region's exact size renders");
}

#[test]
fn a_page_over_4096_is_refused() {
    let doc = Document::open(DocKind::Docx, &dense_docx(), &Limits::default()).expect("opens");
    let (w_pt, h_pt) = doc.info().sizes_pt[0];
    let side = w_pt.max(h_pt);
    // The longer side comes out at 4,097 px or more.
    let over = 4097.0 / side;
    assert_eq!(doc.render_size(0, over, None), Err(RenderError::TooLarge));
    let mut out = Vec::new();
    assert_eq!(doc.render_page(0, over, None, &mut out), Err(RenderError::TooLarge));
    // Well under it, the page fits.
    let (w, h) = doc.render_size(0, 4000.0 / side, None).expect("4,000 px fits");
    assert!(w <= 4096 && h <= 4096, "{w} x {h}");
    // A tile of a page larger than 4,096 px is allowed while the tile itself is at most 4,096.
    let big = 8.0;
    let tile = Rect { x: 1000, y: 1000, w: 512, h: 512 };
    assert_eq!(doc.render_size(0, big, Some(tile)), Ok((512, 512)));
    assert_eq!(doc.render_size(0, big, Some(Rect { w: 4097, ..tile })), Err(RenderError::TooLarge));
    // The other refusals of the same call.
    for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(doc.render_size(0, scale, None), Err(RenderError::BadScale), "scale {scale}");
    }
    assert_eq!(doc.render_size(2, 1.0, None), Err(RenderError::NoSuchPage));
    let page_w = w_pt.ceil() as u32;
    assert_eq!(doc.render_size(0, 1.0, Some(Rect { x: page_w - 50, y: 0, w: 100, h: 100 })), Err(RenderError::BadRegion));
    assert_eq!(doc.render_size(0, 1.0, Some(Rect { x: 0, y: 0, w: 0, h: 10 })), Err(RenderError::BadRegion));
}

#[test]
fn a_tile_renders_its_region_of_the_page() {
    let doc = Document::open(DocKind::Docx, &dense_docx(), &Limits::default()).expect("opens");
    let (full, page) = render(&doc, 0, 1.0);
    // The top-left of the body: the shaded heading and the first lines.
    let tile = Rect { x: 72, y: 72, w: 256, h: 128 };
    let mut out = vec![0u8; 256 * 128 * 4];
    let info = doc.render_page(0, 1.0, Some(tile), &mut out).expect("the tile renders");
    assert_eq!((info.w, info.h), (256, 128));
    let (distinct, _) = colour_stats(&out);
    assert!(distinct >= 10, "the tile has content: {distinct} colours");
    // It matches the same pixels of the whole page (a little antialiasing difference allowed).
    let mut differ = 0usize;
    for y in 0..tile.h as usize {
        for x in 0..tile.w as usize {
            let t = (y * tile.w as usize + x) * 4;
            let p = ((tile.y as usize + y) * full.w as usize + tile.x as usize + x) * 4;
            if (0..4).any(|c| out[t + c].abs_diff(page[p + c]) > 24) {
                differ += 1;
            }
        }
    }
    let share = differ as f64 / (tile.w * tile.h) as f64;
    assert!(share < 0.02, "{:.2} % of the tile differs from the page", share * 100.0);
}

#[test]
fn the_outline_and_the_word_count_come_from_the_document() {
    let doc = Document::open(DocKind::Docx, &dense_docx(), &Limits::default()).expect("opens");
    assert_eq!(
        doc.outline(),
        vec![
            OutlineEntry { level: 1, text: "Page one".into(), page: 0 },
            OutlineEntry { level: 1, text: "Page two".into(), page: 1 },
        ]
    );
    assert_eq!(doc.word_count(), 211);
    assert_eq!(dense_model().word_count(), 211, "the model counts the same");
}

#[test]
fn a_password_protected_file_says_password() {
    let plain = dense_docx();
    let locked = wordcraft_docx::encrypt_with_spin_count(&plain, "correct horse", 1).expect("encrypt");
    assert!(locked.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]), "an encrypted package is a compound file");
    assert_eq!(Document::open(DocKind::Docx, &locked, &Limits::default()).err(), Some(OpenError::Password));
}
