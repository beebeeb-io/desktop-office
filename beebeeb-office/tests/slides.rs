//! Presentations through [`Deck`]: a deck opens with its slides, their sizes and titles; a slide
//! renders as pixels of exactly the page rule's size; every refusal says which; a slide with
//! video draws its poster and says content is missing; the viewer links no media or engine crate.
//! The decks are written in code by DeckCraft's own `.pptx` writer, except one real corpus file.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use beebeeb_office::{Deck, DeckInfo, Limits, OpenError, RenderError, limits, probe};
use common::{NS_PKG_RELS, NS_TYPES, XML_DECL, canary_pixels, canary_png, cfb_file, colour_stats, dense_docx, save_png, zip_stored};
use deckcraft_model::style::PictureFill;
use deckcraft_model::{
    Geom, LayoutType, MediaClip, MediaId, MediaItem, Presentation, SchemeSlot, Shape, ShapeId, ShapeKind, ShapeStyle, Size, TextBody,
    Theme, Xfrm, defaults,
};

// ---------------------------------------------------------------- fixtures

/// A deck of `size` with one slide per entry: the slide's layout, and the text of its title
/// placeholder ("" leaves the placeholder empty).
fn deck_model(size: Size, slides: &[(LayoutType, &str)]) -> Presentation {
    let mut p = defaults::blank_presentation(size, Theme::default(), false);
    for (kind, title) in slides {
        let layout = defaults::layout_of_kind(&p, *kind).expect("the default master has this layout");
        let mut slide = defaults::new_slide(&mut p, layout).expect("a new slide");
        if !title.is_empty() {
            let t = slide.shapes.iter_mut().find(|s| s.ph.as_ref().is_some_and(|ph| ph.kind.is_title())).expect("a title placeholder");
            t.text = Some(TextBody::from_text(title));
        }
        p.slides.push(Arc::new(slide));
    }
    p
}

/// The three-slide deck of the spec, 16:9 (960 x 540 pt): "Q3 plan" (title slide), an empty
/// title, "Thanks".
fn three_slides() -> Presentation {
    deck_model(defaults::WIDE, &[(LayoutType::Title, "Q3 plan"), (LayoutType::TitleAndContent, ""), (LayoutType::TitleOnly, "Thanks")])
}

fn write(p: &Presentation) -> Vec<u8> {
    deckcraft_pptx::export(p).expect("the writer writes the deck")
}

fn open(bytes: &[u8]) -> Deck {
    Deck::open(bytes, &Limits::default()).unwrap_or_else(|e| panic!("the deck opens ({e})"))
}

fn new_id(p: &mut Presentation) -> u32 {
    p.alloc_id().expect("a free id")
}

/// An accent-filled ellipse: smooth edges, so the slide has many distinct colours.
fn ellipse(p: &mut Presentation) -> Shape {
    Shape {
        id: ShapeId(new_id(p)),
        name: "Oval".into(),
        xfrm: Some(Xfrm::new(100.0, 300.0, 240.0, 160.0)),
        geom: Geom::preset("ellipse"),
        style: Some(ShapeStyle::accent(SchemeSlot::Accent1)),
        ..Default::default()
    }
}

/// Add a media item and return its id.
fn add_media(p: &mut Presentation, name: &str, content_type: &str, data: Vec<u8>, link: Option<String>) -> MediaId {
    let id = MediaId(new_id(p));
    p.media.push(MediaItem { id, name: name.into(), content_type: content_type.into(), data: Arc::new(data), link });
    id
}

/// Put `shape` on slide `n`.
fn place(p: &mut Presentation, n: usize, shape: Shape) {
    Arc::make_mut(&mut p.slides[n]).shapes.push(shape);
}

/// The canary picture as a video's poster frame on slide 0, 2" x 2" (144 x 144 pt); slide 1
/// holds only an ellipse. The video's bytes are not a real video: nothing may try to decode them.
fn video_deck() -> Vec<u8> {
    let mut p = deck_model(defaults::WIDE, &[(LayoutType::Blank, ""), (LayoutType::Blank, "")]);
    let video = add_media(&mut p, "clip.mp4", "video/mp4", b"\x00\x00\x00\x18ftypmp42 not a real video".to_vec(), None);
    let poster = add_media(&mut p, "poster.png", "image/png", canary_png(), None);
    let clip = Shape {
        id: ShapeId(new_id(&mut p)),
        name: "Video".into(),
        xfrm: Some(Xfrm::new(100.0, 100.0, 144.0, 144.0)),
        kind: ShapeKind::Media(MediaClip { media: video, video: true, poster: Some(poster), volume: 1.0, ..Default::default() }),
        ..Default::default()
    };
    place(&mut p, 0, clip);
    let oval = ellipse(&mut p);
    place(&mut p, 1, oval);
    write(&p)
}

/// Slide 0: one 144 x 144 pt picture of `data`, embedded; slide 1: only an ellipse.
fn embedded_picture_deck(data: Vec<u8>) -> Vec<u8> {
    let mut p = deck_model(defaults::WIDE, &[(LayoutType::Blank, ""), (LayoutType::Blank, "")]);
    let media = add_media(&mut p, "x.png", "image/png", data, None);
    let picture = Shape {
        id: ShapeId(new_id(&mut p)),
        name: "Picture".into(),
        xfrm: Some(Xfrm::new(100.0, 100.0, 144.0, 144.0)),
        kind: ShapeKind::Picture { fill: PictureFill { media, ..Default::default() } },
        ..Default::default()
    };
    place(&mut p, 0, picture);
    let oval = ellipse(&mut p);
    place(&mut p, 1, oval);
    write(&p)
}

/// The content type of a presentation's main part, as PowerPoint names it in `[Content_Types].xml`.
const CT_PRESENTATION: &str = "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";

/// A zip whose content types and root relationships name a presentation part that is not in it.
fn presentation_without_its_part() -> Vec<u8> {
    let types = format!(
        r#"{XML_DECL}<Types xmlns="{NS_TYPES}"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="{CT_PRESENTATION}"/></Types>"#
    );
    let rels = format!(
        r#"{XML_DECL}<Relationships xmlns="{NS_PKG_RELS}"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#
    );
    zip_stored(&[("[Content_Types].xml", types.as_bytes()), ("_rels/.rels", rels.as_bytes())])
}

/// An OpenDocument presentation's package: a `mimetype` entry first, no `[Content_Types].xml`.
fn odp_package() -> Vec<u8> {
    let content = format!(
        r#"{XML_DECL}<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" office:version="1.3"><office:body><office:presentation/></office:body></office:document-content>"#
    );
    zip_stored(&[("mimetype", b"application/vnd.oasis.opendocument.presentation".as_slice()), ("content.xml", content.as_bytes())])
}

/// One row of `page_pixels_vectors.txt` (the page-pixel rule shared with the app).
struct Row {
    label: String,
    pt: f32,
    scale: f32,
    px: u32,
}

/// The rows of the vector file; each decimal must parse to the f32 bits written beside it.
fn vector_rows() -> Vec<Row> {
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
        rows.push(Row { label: cols[0].to_string(), pt, scale, px: cols[5].parse().expect("px") });
    }
    rows
}

/// Render slide `n` at `scale` into a buffer sized by `render_size`.
fn render(deck: &Deck, n: u32, scale: f32) -> (beebeeb_office::RenderInfo, Vec<u8>) {
    let (w, h) = deck.render_size(n, scale).expect("render size");
    let mut out = vec![0u8; w as usize * h as usize * 4];
    let info = deck.render_slide(n, scale, &mut out).expect("render");
    (info, out)
}

/// The corpus directory (`corpus/` beside this crate) and its expected counts.
fn corpus_expected() -> BTreeMap<String, String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("corpus");
    let tsv = std::fs::read_to_string(dir.join("EXPECTED.tsv")).expect("corpus/EXPECTED.tsv");
    tsv.lines().skip(1).filter_map(|l| l.split_once('\t')).map(|(p, c)| (p.to_string(), c.trim().to_string())).collect()
}

// ---------------------------------------------------------------- tests

#[test]
fn a_deck_opens_with_its_slides_sizes_and_titles() {
    let deck = open(&write(&three_slides()));
    let want = DeckInfo {
        slides: 3,
        sizes_pt: vec![(960.0, 540.0); 3],
        titles: vec!["Q3 plan".to_string(), String::new(), "Thanks".to_string()],
    };
    assert_eq!(deck.info(), want, "slides, sizes and titles");
}

#[test]
fn a_slide_title_is_folded_and_cut_at_255_characters() {
    let long = "x".repeat(300);
    let p = deck_model(defaults::WIDE, &[(LayoutType::TitleOnly, "  Two\nlines\t here  "), (LayoutType::TitleOnly, long.as_str()), (LayoutType::TitleOnly, "é")]);
    let titles = open(&write(&p)).info().titles;
    assert_eq!(titles.len(), 3, "one title per slide");
    assert_eq!(titles[0], "Two lines here", "white space folded to single spaces, trimmed");
    assert_eq!(titles[1].chars().count(), 255, "cut at 255 characters");
    assert_eq!(titles[2], "é", "a character beyond ASCII is kept whole");
}

#[test]
fn a_slide_renders_as_pixels_of_its_size() {
    let mut p = three_slides();
    let oval = ellipse(&mut p);
    place(&mut p, 0, oval);
    let deck = open(&write(&p));
    for (scale, want) in [(1.0f32, (960u32, 540u32)), (4.0 / 3.0, (1280, 720))] {
        assert_eq!(deck.render_size(0, scale), Ok(want), "render_size at {scale}: the page rule");
        let (info, px) = render(&deck, 0, scale);
        assert_eq!((info.w, info.h), want, "the raster written at {scale}");
        let (distinct, _) = colour_stats(&px);
        assert!(distinct >= 10, "slide 0 at {scale} is drawn: {distinct} distinct RGBA values");
        assert!(px.chunks_exact(4).all(|p| p[3] == 255), "every pixel at {scale} is opaque (straight RGBA)");
        assert!(!info.missing_content, "nothing on slide 0 is missing at {scale}");
        save_png(&format!("slides-render-{}x{}", want.0, want.1), want.0, want.1, &px);
    }
}

#[test]
fn render_size_follows_the_shared_vector() {
    let rows = vector_rows();
    assert_eq!(rows.len(), 20, "Letter and A4, each side at five scales");
    let mut scales: Vec<f32> = Vec::new();
    for r in &rows {
        if !scales.iter().any(|s| s.to_bits() == r.scale.to_bits()) {
            scales.push(r.scale);
        }
    }
    assert_eq!(scales.len(), 5, "five scales in the vector file");
    // The vector's own pages as slides: the px column is the answer (Letter at 4/3 and A4 at 1.0
    // tell the f32 product and `ceil` from an f64 product or rounding).
    for (w_label, h_label) in [("letter-w", "letter-h"), ("a4-w", "a4-h")] {
        let widths: Vec<&Row> = rows.iter().filter(|r| r.label == w_label).collect();
        let heights: Vec<&Row> = rows.iter().filter(|r| r.label == h_label).collect();
        assert_eq!((widths.len(), heights.len()), (5, 5), "{w_label}/{h_label}: five rows each");
        let size = Size::new(f64::from(widths[0].pt), f64::from(heights[0].pt));
        let deck = open(&write(&deck_model(size, &[(LayoutType::Blank, "")])));
        let (w_pt, h_pt) = deck.info().sizes_pt[0];
        assert_eq!((w_pt.to_bits(), h_pt.to_bits()), (widths[0].pt.to_bits(), heights[0].pt.to_bits()), "{w_label}: the slide is the vector's page");
        for (wr, hr) in widths.iter().zip(&heights) {
            assert_eq!(wr.scale.to_bits(), hr.scale.to_bits(), "{w_label} and {h_label} rows pair up by scale");
            assert_eq!(deck.render_size(0, wr.scale), Ok((wr.px, hr.px)), "{w_label} x {h_label} at scale {}", wr.scale);
        }
    }
    // The two usual slide sizes at the same scales, by the rule itself: f32 product, ceil, at least 1.
    for (w_pt, h_pt) in [(960.0f32, 540.0f32), (720.0, 540.0)] {
        let deck = open(&write(&deck_model(Size::new(f64::from(w_pt), f64::from(h_pt)), &[(LayoutType::Blank, "")])));
        assert_eq!(deck.info().sizes_pt[0], (w_pt, h_pt), "the slide size in points");
        for &s in &scales {
            let want = (((w_pt * s).ceil() as u32).max(1), ((h_pt * s).ceil() as u32).max(1));
            assert_eq!(deck.render_size(0, s), Ok(want), "{w_pt} x {h_pt} at scale {s}");
        }
    }
}

#[test]
fn a_renderer_image_of_another_size_writes_nothing() {
    let deck = open(&write(&three_slides()));
    let (w, h) = deck.render_size(0, 1.0).expect("render size");
    let len = w as usize * h as usize * 4;
    for (what, rw, rh) in [("1 px wider", w + 1, h), ("1 px taller", w, h + 1), ("1 px narrower", w - 1, h)] {
        let mut out = vec![0xABu8; len];
        let got = deck.render_slide_with(0, 1.0, &mut out, |_, _| (rw, rh, vec![0x11; rw as usize * rh as usize * 4]));
        assert_eq!(got, Err(RenderError::Engine), "an image {what} is an engine error");
        assert!(out.iter().all(|b| *b == 0xAB), "an image {what}: nothing was written");
    }
    // The control: an image of exactly the size is written as it is.
    let mut out = vec![0xABu8; len];
    let opaque = [0x11u8, 0x22, 0x33, 0xFF];
    let got = deck.render_slide_with(0, 1.0, &mut out, |rw, rh| (rw, rh, opaque.repeat(rw as usize * rh as usize)));
    assert_eq!(got.map(|i| (i.w, i.h)), Ok((w, h)), "an image of the size is accepted");
    assert!(out.chunks_exact(4).all(|p| p == opaque), "and written into the buffer");
}

#[test]
fn a_wrong_buffer_scale_or_slide_is_refused() {
    let deck = open(&write(&three_slides()));
    let (w, h) = deck.render_size(0, 1.0).expect("render size");
    let len = w as usize * h as usize * 4;
    for wrong in [0, 10, len - 4, len + 4] {
        let mut out = vec![0x5Au8; wrong];
        assert_eq!(deck.render_slide(0, 1.0, &mut out), Err(RenderError::BufferSize), "a buffer of {wrong} bytes for {len}");
        assert!(out.iter().all(|b| *b == 0x5A), "a buffer of {wrong} bytes: nothing written");
    }
    let mut out = vec![0u8; len];
    for scale in [0.0f32, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(deck.render_size(0, scale), Err(RenderError::BadScale), "render_size at scale {scale}");
        assert_eq!(deck.render_slide(0, scale, &mut out), Err(RenderError::BadScale), "render_slide at scale {scale}");
    }
    for n in [3u32, 4, u32::MAX] {
        assert_eq!(deck.render_size(n, 1.0), Err(RenderError::NoSuchPage), "render_size of slide {n} of 3");
        assert_eq!(deck.render_slide(n, 1.0, &mut out), Err(RenderError::NoSuchPage), "render_slide of slide {n} of 3");
    }
    // 960 pt at 4.25 is 4,080 px (inside 4,096); at 4.5 it is 4,320 px.
    assert_eq!(deck.render_size(0, 4.25), Ok((4080, 2295)), "just inside the side limit");
    assert_eq!(deck.render_size(0, 4.5), Err(RenderError::TooLarge), "over 4,096 px on a side");
    assert_eq!(deck.render_slide(0, 4.5, &mut out), Err(RenderError::TooLarge), "render_slide over 4,096 px on a side");
    let tight = Limits { max_render_side_px: 959, ..Limits::default() };
    let small = Deck::open(&write(&three_slides()), &tight).expect("opens under a lower side limit");
    assert_eq!(small.render_size(0, 1.0), Err(RenderError::TooLarge), "the side limit is the deck's own Limits");
}

#[test]
fn over_the_slide_limit_is_too_large() {
    let bytes = write(&three_slides());
    let two = Limits { max_pages: 2, ..Limits::default() };
    assert_eq!(Deck::open(&bytes, &two).err(), Some(OpenError::TooLarge(limits::PAGES)), "3 slides over a limit of 2");
    let three = Limits { max_pages: 3, ..Limits::default() };
    assert_eq!(Deck::open(&bytes, &three).map(|d| d.info().slides).ok(), Some(3), "3 slides at a limit of 3 open");
}

#[test]
fn only_a_presentation_package_opens() {
    let ppt = cfb_file(&[("PowerPoint Document", vec![0u8; 64]), ("Current User", vec![0u8; 64])]);
    for (what, bytes) in [
        (".ppt (a compound file)", ppt),
        (".odp (an OpenDocument package)", odp_package()),
        (".docx (a Word package)", dense_docx()),
        ("plain text", b"Q3 plan\nThanks\n".to_vec()),
    ] {
        probe::reset();
        assert_eq!(Deck::open(&bytes, &Limits::default()).err(), Some(OpenError::Unsupported), "{what}");
        assert_eq!(probe::bytes_to_readers(), 0, "{what}: no reader was handed the bytes");
    }
    // The control: the deck writer's own package opens, and the reader was handed it.
    let pptx = write(&three_slides());
    probe::reset();
    assert!(Deck::open(&pptx, &Limits::default()).is_ok(), "a .pptx opens");
    assert_eq!(probe::bytes_to_readers(), pptx.len() as u64, "the .pptx went to the reader");
}

#[test]
fn a_protected_or_damaged_deck_says_so() {
    let encrypted = cfb_file(&[("EncryptionInfo", vec![4u8, 0, 4, 0, 0x40, 0, 0, 0]), ("EncryptedPackage", vec![0u8; 64])]);
    assert_eq!(Deck::open(&encrypted, &Limits::default()).err(), Some(OpenError::Password), "an encrypted presentation");
    assert_eq!(Deck::open(&presentation_without_its_part(), &Limits::default()).err(), Some(OpenError::Damaged), "a package without its presentation part");
    let pptx = write(&three_slides());
    let cut = &pptx[..pptx.len() / 2];
    assert_eq!(Deck::open(cut, &Limits::default()).err(), Some(OpenError::Damaged), "a package cut in half");
}

#[test]
fn a_slide_with_video_shows_its_poster_and_says_content_is_missing() {
    let deck = open(&video_deck());
    assert_eq!(deck.info().slides, 2, "two slides");
    let (info, px) = render(&deck, 0, 1.0);
    assert!(info.missing_content, "the video slide says content is missing");
    let poster = canary_pixels(&px);
    // The poster is 144 x 144 pt at scale 1.0: 20,736 pixels.
    assert!(poster > 15_000, "the poster frame is drawn: {poster} canary pixels");
    save_png("slides-video-poster", info.w, info.h, &px);
    let (info, px) = render(&deck, 1, 1.0);
    assert!(!info.missing_content, "the slide without video says nothing is missing");
    assert_eq!(canary_pixels(&px), 0, "and shows no poster");
}

#[test]
fn a_picture_over_the_limits_or_that_does_not_decode_is_missing() {
    let canary = embedded_picture_deck(canary_png());
    // The control: within the limits the 64 x 64 canary is drawn, and nothing is missing.
    let deck = open(&canary);
    let (info, px) = render(&deck, 0, 1.0);
    assert!(!info.missing_content, "the canary within the limits: nothing missing");
    assert!(canary_pixels(&px) > 15_000, "the canary within the limits is drawn");
    // One pixel over the side limit, or one pixel over the pixel limit: left out before decoding.
    for (what, lim) in [
        ("a side over max_image_side_px", Limits { max_image_side_px: 63, ..Limits::default() }),
        ("more than max_image_pixels", Limits { max_image_pixels: 64 * 64 - 1, ..Limits::default() }),
    ] {
        let deck = Deck::open(&canary, &lim).unwrap_or_else(|e| panic!("{what}: opens ({e})"));
        let (w, h) = deck.render_size(0, 1.0).expect("render size");
        let mut out = vec![0u8; w as usize * h as usize * 4];
        let info = deck.render_slide(0, 1.0, &mut out).expect("render");
        assert!(info.missing_content, "{what}: the slide says content is missing");
        assert_eq!(canary_pixels(&out), 0, "{what}: the picture is not drawn");
        let mut out = vec![0u8; w as usize * h as usize * 4];
        assert!(!deck.render_slide(1, 1.0, &mut out).expect("render").missing_content, "{what}: the slide without it is complete");
    }
    // Bytes that start like a PNG but do not decode.
    let broken = open(&embedded_picture_deck(b"\x89PNG\r\n\x1a\n but not a picture".to_vec()));
    assert!(render(&broken, 0, 1.0).0.missing_content, "a picture that does not decode: content is missing");
    assert!(!render(&broken, 1, 1.0).0.missing_content, "the slide without it is complete");
}

#[test]
fn the_viewer_links_no_media_or_engine_crate() {
    let lock = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock")).expect("read beebeeb-office/Cargo.lock");
    let names: Vec<&str> = lock
        .lines()
        .filter_map(|l| l.strip_prefix("name = \""))
        .filter_map(|l| l.strip_suffix('"'))
        .collect();
    let packages = lock.lines().filter(|l| l.trim() == "[[package]]").count();
    // The instrument reads the right file: a full lock with the crates the viewer does link.
    assert!(packages >= 100, "the lock lists {packages} packages");
    assert_eq!(names.len(), packages, "one name per package");
    for linked in ["beebeeb-office", "deckcraft-pptx", "deckcraft-render", "deckcraft-model"] {
        assert!(names.contains(&linked), "the lock lists {linked}");
    }
    let banned = [
        "deckcraft-engine",
        "deckcraft-media",
        "deckcraft-ui-egui",
        "deckcraft-h264",
        "deckcraft-hevc",
        "deckcraft-vp9",
        "deckcraft-av1",
        "ureq",
        "rustls",
        "reqwest",
        "hyper",
    ];
    let found: Vec<&str> = names.iter().copied().filter(|n| banned.contains(n) || n.starts_with("symphonia")).collect();
    assert!(found.is_empty(), "media, engine or network crates in the lock: {found:?}");
}

#[test]
fn a_corpus_pptx_renders_every_slide() {
    let file = "pptx/shapes.pptx";
    let expected: u32 = corpus_expected().get(file).unwrap_or_else(|| panic!("{file} has a row in EXPECTED.tsv")).parse().expect("a known count");
    assert!(expected >= 2, "{file}: a deck of several slides ({expected})");
    let bytes = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("corpus").join(file)).expect("read the corpus file");
    let deck = open(&bytes);
    let info = deck.info();
    assert_eq!(info.slides, expected, "{file}: the slide count EXPECTED.tsv states");
    assert_eq!((info.sizes_pt.len(), info.titles.len()), (expected as usize, expected as usize), "{file}: one size and one title per slide");
    for n in 0..info.slides {
        let (w, h) = deck.render_size(n, 1.0).unwrap_or_else(|e| panic!("{file} slide {n}: render size ({e})"));
        let mut out = vec![0u8; w as usize * h as usize * 4];
        let r = deck.render_slide(n, 1.0, &mut out).unwrap_or_else(|e| panic!("{file} slide {n}: renders ({e})"));
        assert_eq!((r.w, r.h), (w, h), "{file} slide {n}: the size asked for");
        save_png(&format!("slides-corpus-shapes-{n}"), w, h, &out);
    }
}
