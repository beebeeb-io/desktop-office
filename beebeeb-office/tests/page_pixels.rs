//! The page-pixel rule, one for the app and the Office engine: `scale` is device pixels per point
//! and a page side is `ceil(pt * scale)` pixels, the product taken in f32. `page_pixels_vectors.txt`
//! holds the cases (Letter and A4, each side at the scales 1.0, 1.25, 1.5, 2.0 and 4/3); the app's
//! protocol crate reads the same file, byte for byte. Here each page size is opened as a document
//! that states it in twips, as Word does, and `Document::render_size` must give the file's pixels.

mod common;

use beebeeb_office::{DocKind, Document, Limits};
use common::docx_package_on;

/// One row of the vector file.
struct Row {
    label: String,
    /// The side in points, as written (for the twips of the test document).
    pt_text: String,
    pt: f32,
    scale: f32,
    px: u32,
}

/// The rows of the vector file. Each decimal must parse to the f32 bits written beside it.
fn rows() -> Vec<Row> {
    let bits = |s: &str| f32::from_bits(u32::from_str_radix(s.trim_start_matches("0x"), 16).expect("hex bits"));
    let mut rows = Vec::new();
    for line in include_str!("page_pixels_vectors.txt").lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(cols.len(), 6, "label, pt, pt bits, scale, scale bits, px: {line}");
        let (pt, scale) = (bits(cols[2]), bits(cols[4]));
        assert_eq!(cols[1].parse::<f32>().expect("pt").to_bits(), pt.to_bits(), "the pt decimal is its bits: {line}");
        assert_eq!(cols[3].parse::<f32>().expect("scale").to_bits(), scale.to_bits(), "the scale decimal is its bits: {line}");
        rows.push(Row { label: cols[0].to_string(), pt_text: cols[1].to_string(), pt, scale, px: cols[5].parse().expect("px") });
    }
    rows
}

/// A side in points as twips, which must be whole: a page size a document can state.
fn twips(pt_text: &str) -> u32 {
    let tw = pt_text.parse::<f64>().expect("pt") * 20.0;
    assert!((tw - tw.round()).abs() < 1e-6, "{pt_text} pt is not a whole number of twips");
    tw.round() as u32
}

#[test]
fn page_pixels_follow_the_shared_vector() {
    let rows = rows();
    assert_eq!(rows.len(), 20, "Letter and A4, each side at five scales");
    let five_scales = [1.0f32, 1.25, 1.5, 2.0, 4.0 / 3.0].map(f32::to_bits);
    let body = "<w:p><w:r><w:t>The quick brown fox.</w:t></w:r></w:p>";
    for (w_label, h_label) in [("letter-w", "letter-h"), ("a4-w", "a4-h")] {
        let sides: Vec<&Row> = rows.iter().filter(|r| r.label == w_label).collect();
        let heights: Vec<&Row> = rows.iter().filter(|r| r.label == h_label).collect();
        assert_eq!(sides.len(), 5, "{w_label}: five rows");
        assert_eq!(heights.len(), 5, "{h_label}: five rows");
        let (w, h) = (sides[0], heights[0]);
        let bytes = docx_package_on(body, "", &[], (twips(&w.pt_text), twips(&h.pt_text)));
        let doc = Document::open(DocKind::Docx, &bytes, &Limits::default()).expect("the page opens");
        let (w_pt, h_pt) = doc.info().sizes_pt[0];
        assert_eq!((w_pt.to_bits(), h_pt.to_bits()), (w.pt.to_bits(), h.pt.to_bits()), "{w_label}: the document's page is the vector's, to the bit");
        let mut scales = Vec::new();
        for (wr, hr) in sides.iter().zip(&heights) {
            assert_eq!((wr.pt.to_bits(), hr.pt.to_bits()), (w.pt.to_bits(), h.pt.to_bits()), "{w_label}: one page size per label");
            assert_eq!(wr.scale.to_bits(), hr.scale.to_bits(), "{w_label} and {h_label} rows pair up by scale");
            // The file states the rule's own result (f32 product, rounded up, at least 1).
            for r in [wr, hr] {
                assert_eq!(r.px, ((r.pt * r.scale).ceil() as u32).max(1), "{} at scale {}: the file's px is ceil(pt * scale) in f32", r.label, r.scale);
            }
            assert_eq!(doc.render_size(0, wr.scale, None), Ok((wr.px, hr.px)), "{w_label} x {h_label} at scale {}", wr.scale);
            scales.push(wr.scale.to_bits());
        }
        scales.sort_unstable();
        let mut want = five_scales;
        want.sort_unstable();
        assert_eq!(scales, want, "{w_label}: the scales 1.0, 1.25, 1.5, 2.0 and 4/3");
    }
}
