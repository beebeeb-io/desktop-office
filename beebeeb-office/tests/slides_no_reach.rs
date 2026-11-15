//! A picture a slide only links to is never reached: no connection to a loopback listener, no
//! read of a local canary file. Each case runs its controls first: the listener counts a plain
//! connection, and the same picture embedded draws the canary, so a linked variant that shows no
//! canary means the target was not resolved, not that the slide's picture was never drawn.

mod common;

use std::net::TcpStream;
use std::sync::Arc;

use beebeeb_office::{Deck, Limits, RenderInfo};
use common::{Listener, canary_pixels, canary_png, file_url};
use deckcraft_model::style::PictureFill;
use deckcraft_model::{LayoutType, MediaId, MediaItem, Presentation, Shape, ShapeId, ShapeKind, Theme, Xfrm, defaults};

/// A 16:9 deck of one blank slide holding one 2" x 2" picture (144 x 144 pt): the canary, embedded
/// in the package when `link` is `None`, else only linked to `link` (the writer names it through
/// `r:link` with an external target, as PowerPoint writes a linked picture).
fn picture_deck(link: Option<&str>) -> Vec<u8> {
    let mut p: Presentation = defaults::blank_presentation(defaults::WIDE, Theme::default(), false);
    let layout = defaults::layout_of_kind(&p, LayoutType::Blank).expect("a blank layout");
    let slide = defaults::new_slide(&mut p, layout).expect("a slide");
    p.slides.push(Arc::new(slide));
    let id = MediaId(p.alloc_id().expect("an id"));
    let (data, link) = match link {
        None => (canary_png(), None),
        Some(target) => (Vec::new(), Some(target.to_string())),
    };
    p.media.push(MediaItem { id, name: "x.png".into(), content_type: "image/png".into(), data: Arc::new(data), link });
    let picture = Shape {
        id: ShapeId(p.alloc_id().expect("an id")),
        name: "Picture".into(),
        xfrm: Some(Xfrm::new(100.0, 100.0, 144.0, 144.0)),
        kind: ShapeKind::Picture { fill: PictureFill { media: id, ..Default::default() } },
        ..Default::default()
    };
    Arc::make_mut(&mut p.slides[0]).shapes.push(picture);
    deckcraft_pptx::export(&p).expect("the writer writes the deck")
}

/// Open `bytes` and render every slide at 1.0: what each said, and its canary pixels.
fn render_all(what: &str, bytes: &[u8]) -> Vec<(RenderInfo, usize)> {
    let deck = Deck::open(bytes, &Limits::default()).unwrap_or_else(|e| panic!("{what}: opens ({e})"));
    let slides = deck.info().slides;
    assert!(slides >= 1, "{what}: at least one slide");
    (0..slides)
        .map(|n| {
            let (w, h) = deck.render_size(n, 1.0).expect("render size");
            let mut out = vec![0u8; w as usize * h as usize * 4];
            let info = deck.render_slide(n, 1.0, &mut out).unwrap_or_else(|e| panic!("{what}: slide {n} renders ({e})"));
            (info, canary_pixels(&out))
        })
        .collect()
}

/// The control: the same picture embedded is drawn (the 144 x 144 pt canary: 20,736 pixels at
/// 1.0), and nothing is missing.
fn control_draws_the_canary() {
    let slides = render_all("the embedded control", &picture_deck(None));
    let (info, canary) = slides[0];
    assert!(canary > 15_000, "the embedded control draws the canary: {canary} magenta pixels");
    assert!(!info.missing_content, "the embedded control says nothing is missing");
}

/// The linked variant: every slide renders, says content is missing, and shows no canary.
fn renders_without_reaching(what: &str, target: &str) {
    for (n, (info, canary)) in render_all(what, &picture_deck(Some(target))).into_iter().enumerate() {
        assert!(info.missing_content, "{what}: slide {n} says content is missing");
        assert_eq!(canary, 0, "{what}: slide {n} shows no canary");
    }
}

#[test]
fn a_linked_picture_is_not_fetched() {
    let listener = Listener::bind("127.0.0.1:0").expect("listen on loopback");
    // The instrument counts: one connection from here is seen, then the count starts again.
    drop(TcpStream::connect(listener.addr).expect("connect to the listener"));
    assert_eq!(listener.drain(), 1, "the listener counts a connection");
    control_draws_the_canary();
    let url = format!("http://{}/x.png", listener.addr);
    renders_without_reaching("a slide linking a loopback URL", &url);
    assert_eq!(listener.drain(), 0, "no connection to {url}");
}

#[test]
fn a_linked_file_target_is_not_read() {
    let dir = std::env::temp_dir().join(format!("bb-office-slides-canary-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let canary = dir.join("canary.png");
    std::fs::write(&canary, canary_png()).expect("write the canary");
    control_draws_the_canary();
    let url = file_url(&canary);
    let raw = canary.display().to_string();
    for (what, target) in [("a file: URL", url.as_str()), ("a bare path", raw.as_str())] {
        renders_without_reaching(what, target);
    }
    std::fs::remove_dir_all(&dir).ok();
}
