//! Presentations (`.pptx`, and the same package as `.pptm` and `.ppsx`): read from bytes once,
//! slides rendered on request.
//!
//! The path is DeckCraft's reader, then `deckcraft_render`: below `deckcraft-engine`, which the
//! viewer never links, and without `deckcraft-media` or any video decoder (decision 2482 Q10). An
//! audio or video clip shows the poster frame the file stores (else the renderer's placeholder),
//! and its slide says content is missing. `.ppt` has no reader here: it is unsupported for good.
//! Transitions and animations are not played: a slide is drawn in its final state.

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, OnceLock};

use deckcraft_model::resolve::{self, Ctx};
use deckcraft_model::style::{Fill, Line};
use deckcraft_model::text::{Bullet, ListStyle, ParaProps, RunProps, TextBody};
use deckcraft_model::{MediaId, Presentation, Rgba, Shape, ShapeKind, Slide};
use deckcraft_render::RenderOpts;

use crate::raster::check_buffer;
use crate::{Limits, OpenError, RenderError, RenderInfo, probe};

/// Longest title [`DeckInfo::titles`] holds, in characters.
const MAX_TITLE_CHARS: usize = 255;

/// The slide count, and every slide's size (points) and title.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeckInfo {
    pub slides: u32,
    /// (width, height) of each slide, points; one per slide, each finite and above 0.
    pub sizes_pt: Vec<(f32, f32)>,
    /// The title of each slide (empty when it has none); one per slide: the text of its title or
    /// centred-title placeholder, white space folded to single spaces, at most 255 characters.
    pub titles: Vec<String>,
}

/// An open presentation.
pub struct Deck {
    pres: Presentation,
    info: DeckInfo,
    limits: Limits,
    /// The reader left out slides the file lists (a part missing or unreadable). It does not say
    /// where they stood, so every slide reports content as missing.
    lost_slides: bool,
    /// Per slide, worked out on its first render: something on it cannot be shown.
    missing: Vec<OnceLock<bool>>,
    /// Per embedded file, worked out the first time a slide shows it as a picture: it decodes.
    decodes: HashMap<MediaId, OnceLock<bool>>,
}

impl Deck {
    /// Open a presentation from its bytes.
    ///
    /// In this order: [`crate::prescan`] refuses a container over `limits` before any reader sees
    /// it; the bytes must be an OOXML presentation package (a zip whose content types name the
    /// main part of a `.pptx`, `.pptm` or `.ppsx`): an encrypted package is
    /// [`OpenError::Password`], a `.ppt`, `.odp`, `.key` or anything else
    /// [`OpenError::Unsupported`], and no reader is handed them; DeckCraft's reader parses them;
    /// more than `limits.max_pages` slides is refused; pictures over the picture limits are left
    /// out. Nothing is fetched and nothing is written: a linked picture or clip stays unresolved.
    pub fn open(bytes: &[u8], limits: &Limits) -> Result<Deck, OpenError> {
        crate::prescan(bytes, limits)?;
        presentation_package(bytes)?;
        probe::handed(bytes);
        let (mut pres, lost) = deckcraft_pptx::import_checked(bytes).map_err(from_pptx)?;
        if pres.slides.len() as u64 > u64::from(limits.max_pages) {
            return Err(OpenError::TooLarge(crate::limits::PAGES));
        }
        // One size for every slide, as the file states it.
        let size = (pres.slide_size.width as f32, pres.slide_size.height as f32);
        if !(size.0.is_finite() && size.1.is_finite() && size.0 > 0.0 && size.1 > 0.0) {
            return Err(OpenError::Damaged);
        }
        drop_oversized_pictures(&mut pres, limits);
        let slides = pres.slides.len();
        let info = DeckInfo {
            slides: slides as u32,
            sizes_pt: vec![size; slides],
            titles: pres.slides.iter().map(|s| title_text(&s.title())).collect(),
        };
        let decodes = pres.media.iter().filter(|m| !m.data.is_empty()).map(|m| (m.id, OnceLock::new())).collect();
        Ok(Deck {
            missing: (0..slides).map(|_| OnceLock::new()).collect(),
            info,
            limits: *limits,
            lost_slides: !lost.is_empty(),
            decodes,
            pres,
        })
    }

    /// The slide count, the slide sizes and the titles.
    pub fn info(&self) -> DeckInfo {
        self.info.clone()
    }

    /// The size in pixels of what [`Deck::render_slide`] writes for slide `n` at `scale`: the
    /// caller sizes its buffer from this.
    ///
    /// The page rule, one for the app and the Office engine: `ceil(width_pt * scale)` x
    /// `ceil(height_pt * scale)`, the product taken in f32, at least 1, and at most
    /// `max_render_side_px` on each side.
    pub fn render_size(&self, n: u32, scale: f32) -> Result<(u32, u32), RenderError> {
        let &(w_pt, h_pt) = self.info.sizes_pt.get(n as usize).ok_or(RenderError::NoSuchPage)?;
        if !(scale.is_finite() && scale > 0.0) {
            return Err(RenderError::BadScale);
        }
        let (pw, ph) = ((w_pt * scale).ceil(), (h_pt * scale).ceil());
        let max_side = self.limits.max_render_side_px as f32;
        if !(pw.is_finite() && ph.is_finite()) || pw > max_side || ph > max_side {
            return Err(RenderError::TooLarge);
        }
        Ok((pw.max(1.0) as u32, ph.max(1.0) as u32))
    }

    /// Render slide `n` (0-based) at `scale` pixels per point into `out` as RGBA8, row by row,
    /// top-left first.
    ///
    /// `out` must be exactly `w * h * 4` bytes for the size [`Deck::render_size`] gives, else
    /// [`RenderError::BufferSize`] and nothing is written. The renderer draws at exactly that size
    /// on white (so every pixel is opaque); a raster of any other size is
    /// [`RenderError::Engine`] and nothing is written: it is never cropped or stretched. The
    /// renderer runs on the calling thread and starts no threads of its own.
    pub fn render_slide(&self, n: u32, scale: f32, out: &mut [u8]) -> Result<RenderInfo, RenderError> {
        self.render_slide_with(n, scale, out, |w, h| {
            let opts = RenderOpts {
                scale: f64::from(scale),
                size: Some((w, h)),
                clear: Some(Rgba::WHITE),
                threads: 0,
                ..RenderOpts::default()
            };
            let image = deckcraft_render::render_slide(&self.pres, n as usize, &opts);
            (image.width, image.height, image.pixels)
        })
    }

    /// [`Deck::render_slide`] with the drawing step given: `draw(w, h)` returns the raster
    /// (width, height, premultiplied RGBA8) for a slide asked at `w` x `h`. A test seam, not part of
    /// the API: every check around the drawing is the one [`Deck::render_slide`] makes.
    #[doc(hidden)]
    pub fn render_slide_with(&self, n: u32, scale: f32, out: &mut [u8], draw: impl FnOnce(u32, u32) -> (u32, u32, Vec<u8>)) -> Result<RenderInfo, RenderError> {
        let (w, h) = self.render_size(n, scale)?;
        check_buffer(w, h, out)?;
        // The renderer turns a panic of its own into a blank slide of the right size: counted, so
        // that slide says its content is missing instead of passing as drawn.
        let panics = deckcraft_render::panics_caught();
        let (rw, rh, pixels) = draw(w, h);
        let panicked = deckcraft_render::panics_caught() != panics;
        if rw != w || rh != h || pixels.len() != out.len() {
            return Err(RenderError::Engine);
        }
        out.copy_from_slice(&pixels);
        unpremultiply(out);
        Ok(RenderInfo { w, h, missing_content: panicked || self.missing_on(n as usize) })
    }

    /// Whether something on slide `n` cannot be shown ([`RenderInfo::missing_content`]).
    fn missing_on(&self, n: usize) -> bool {
        if self.lost_slides {
            return true;
        }
        let Some(slot) = self.missing.get(n) else { return false };
        // A panic while looking is an answer too: then something on the slide is not understood.
        *slot.get_or_init(|| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.slide_misses_content(n))).unwrap_or(true))
    }

    /// Slide `n` holds audio, video, a 3-D model or an object with no picture of its own; or a
    /// picture it shows is only linked, missing, over the picture limits or does not decode; or a
    /// character on it has no glyph in any installed font.
    fn slide_misses_content(&self, n: usize) -> bool {
        let Some(slide) = self.pres.slides.get(n) else { return false };
        let refs = Refs::of_slide(&self.pres, slide);
        refs.not_drawn || refs.pictures.iter().any(|id| !self.picture_shows(*id)) || missing_glyphs(&self.pres, n)
    }

    /// The picture `id` is embedded and decodes. A linked one has no bytes (it is never fetched),
    /// nor has one left out at open for its size.
    fn picture_shows(&self, id: MediaId) -> bool {
        let Some(m) = self.pres.media(id) else { return false };
        if m.link.is_some() || m.data.is_empty() {
            return false;
        }
        let Some(cell) = self.decodes.get(&id) else { return false };
        // Decoded once by the renderer's own decoder, so "decodes here" is "draws there".
        *cell.get_or_init(|| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| deckcraft_render::decode_image(&m.data).is_some())).unwrap_or(false))
    }
}

/// The first bytes of a zip package (a local file header).
const ZIP_MAGIC: &[u8] = b"PK\x03\x04";
/// The part of an OOXML package that names the content type of every part.
const CONTENT_TYPES: &str = "[Content_Types].xml";
/// Largest `[Content_Types].xml` read (bytes); a real one is a few kilobytes.
const MAX_CONTENT_TYPES_BYTES: u64 = 4 * 1024 * 1024;
/// The content type of a presentation's main part: `.pptx`, `.pptm`, `.ppsx`.
const PRESENTATION_MAIN: [&str; 3] = [
    "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml",
    "application/vnd.ms-powerpoint.presentation.macroEnabled.main+xml",
    "application/vnd.openxmlformats-officedocument.presentationml.slideshow.main+xml",
];

/// Is `bytes` an OOXML presentation package? Runs after the pre-scan, before any reader.
///
/// - A compound file: an encrypted package ([`OpenError::Password`]) or a `.ppt`
///   ([`OpenError::Unsupported`]: there is no reader for it).
/// - A zip whose `[Content_Types].xml` names a presentation's main part: yes. One that names
///   none (a `.docx`, an `.xlsx`), or has no content types at all (an `.odp`, a `.key`):
///   [`OpenError::Unsupported`]. A zip whose directory or content types cannot be read:
///   [`OpenError::Damaged`].
/// - Anything else: [`OpenError::Unsupported`].
fn presentation_package(bytes: &[u8]) -> Result<(), OpenError> {
    if bytes.starts_with(&crate::limits::CFB_MAGIC) {
        return Err(if deckcraft_pptx::is_encrypted(bytes) { OpenError::Password } else { OpenError::Unsupported });
    }
    if !bytes.starts_with(ZIP_MAGIC) {
        return Err(OpenError::Unsupported);
    }
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| OpenError::Damaged)?;
    let Some(name) = zip.file_names().find(|n| n.trim_start_matches('/').eq_ignore_ascii_case(CONTENT_TYPES)).map(str::to_owned) else {
        return Err(OpenError::Unsupported);
    };
    let entry = zip.by_name(&name).map_err(|_| OpenError::Damaged)?;
    let mut xml = Vec::new();
    entry.take(MAX_CONTENT_TYPES_BYTES + 1).read_to_end(&mut xml).map_err(|_| OpenError::Damaged)?;
    if xml.len() as u64 > MAX_CONTENT_TYPES_BYTES {
        return Err(OpenError::Damaged);
    }
    let types = deckcraft_pptx::parse_xml(&xml).map_err(|_| OpenError::Damaged)?;
    let names_a_presentation = types.root.is("Types")
        && types.root.elements().any(|e| {
            (e.is("Override") || e.is("Default"))
                && e.attr("ContentType").is_some_and(|ct| PRESENTATION_MAIN.iter().any(|main| ct.trim().eq_ignore_ascii_case(main)))
        });
    if names_a_presentation { Ok(()) } else { Err(OpenError::Unsupported) }
}

/// The reader's error as a code. Every variant is listed, so a new one upstream is a compile error
/// here and gets read before a subtree bump lands. The reader has no password or limit variant: a
/// password is found from the container first ([`presentation_package`]), and the container limits
/// are the pre-scan's.
fn from_pptx(e: deckcraft_pptx::PptxError) -> OpenError {
    use deckcraft_pptx::PptxError as E;
    match e {
        E::Unsupported(_) => OpenError::Unsupported,
        E::NotPptx(_) | E::Xml(_) | E::Write(_) => OpenError::Damaged,
    }
}

/// Leave out every raster picture whose header declares more than `max_image_side_px` on a side
/// or more than `max_image_pixels`: its bytes are dropped, so it is never decoded, and a slide
/// showing it reports content as missing. The header is read by the same image crate the renderer
/// decodes with; bytes that are not a raster picture it knows (a metafile, a clip, a font) are left
/// to the renderer's own checks.
fn drop_oversized_pictures(pres: &mut Presentation, limits: &Limits) {
    for m in &mut pres.media {
        let Some((w, h)) = wordcraft_render::image_size(m.data.as_slice()) else { continue };
        if w > limits.max_image_side_px || h > limits.max_image_side_px || u64::from(w) * u64::from(h) > limits.max_image_pixels {
            m.data = Arc::new(Vec::new());
        }
    }
}

/// A slide's title for [`DeckInfo::titles`]: control characters as spaces, white space folded to
/// single spaces and trimmed, cut to at most [`MAX_TITLE_CHARS`] characters.
fn title_text(raw: &str) -> String {
    let spaced: String = raw.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let folded = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = folded.chars().take(MAX_TITLE_CHARS).collect();
    cut.trim_end().to_string()
}

/// Straight RGBA from the renderer's premultiplied RGBA, in place. Every pixel is opaque (the
/// slide is drawn on white), so this changes nothing unless a pixel is not.
fn unpremultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = u32::from(px[3]);
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

/// A character on slide `n` that no installed font can draw: the text as the renderer lays it out
/// shows `.notdef` (glyph 0) for it. A font the slide names that is not installed is replaced by
/// another that draws the same characters, which is not missing content.
fn missing_glyphs(pres: &Presentation, n: usize) -> bool {
    deckcraft_render::place_slide(pres, n).texts.iter().any(|text| {
        text.layout.runs.iter().any(|run| {
            run.glyphs
                .iter()
                .enumerate()
                .any(|(i, glyph)| glyph.0 == 0 && run.cells.get(i).is_none_or(|&(_, c)| !c.is_whitespace() && !c.is_control()))
        })
    })
}

/// Groups nested deeper than this are not walked (the reader bounds nesting itself).
const MAX_GROUP_DEPTH: usize = 256;

/// What one slide draws that the viewer may not be able to show.
#[derive(Default)]
struct Refs {
    /// Every picture it names, from a picture shape, a picture fill, a picture bullet, a poster
    /// frame or an object's preview.
    pictures: Vec<MediaId>,
    /// It holds something the viewer never draws: audio, video, a 3-D model, an object with no
    /// picture of its own, or shapes on a slide with no master to draw them on.
    not_drawn: bool,
}

impl Refs {
    /// The slide's own shapes on the backdrop the renderer draws them on: the slide's background,
    /// then the master's and the layout's own graphics (not their placeholders) when the slide shows
    /// them. Plus the master's text styles, whose picture bullets the slide's text uses. Hidden
    /// shapes are not drawn, so not walked. Not walked either: chart and table-style fills.
    fn of_slide(pres: &Presentation, slide: &Slide) -> Refs {
        let mut r = Refs::default();
        let Some(ctx) = Ctx::for_slide(pres, slide) else {
            r.not_drawn = !slide.shapes.is_empty();
            return r;
        };
        let (background, _) = resolve::background(&ctx, Some(slide));
        r.fill(&background);
        let (show_layout, show_master) = resolve::show_master_shapes(slide, ctx.layout);
        if show_master {
            for s in ctx.master.shapes.iter().filter(|s| s.ph.is_none()) {
                r.shape(s, 0);
            }
        }
        if show_layout && let Some(layout) = ctx.layout {
            for s in layout.shapes.iter().filter(|s| s.ph.is_none()) {
                r.shape(s, 0);
            }
        }
        for style in [&ctx.master.title_style, &ctx.master.body_style, &ctx.master.other_style] {
            r.list_style(style);
        }
        for s in &slide.shapes {
            r.shape(s, 0);
        }
        r
    }

    fn shape(&mut self, s: &Shape, depth: usize) {
        if s.hidden || depth > MAX_GROUP_DEPTH {
            return;
        }
        if let Some(f) = &s.fill {
            self.fill(f);
        }
        if let Some(l) = &s.line {
            self.line(l);
        }
        if let Some(t) = &s.text {
            self.text(t);
        }
        match &s.kind {
            ShapeKind::Picture { fill } => self.pictures.push(fill.media),
            ShapeKind::Group { children, .. } => {
                for c in children {
                    self.shape(c, depth + 1);
                }
            }
            ShapeKind::Table(t) => {
                if let Some(f) = &t.background {
                    self.fill(f);
                }
                for row in &t.rows {
                    for cell in &row.cells {
                        if let Some(f) = &cell.fill {
                            self.fill(f);
                        }
                        self.text(&cell.text);
                    }
                }
            }
            ShapeKind::Media(clip) => {
                self.not_drawn = true;
                self.pictures.extend(clip.poster);
            }
            ShapeKind::Model3d { poster, .. } => {
                self.not_drawn = true;
                self.pictures.extend(*poster);
            }
            ShapeKind::Opaque { preview: Some(p), .. } => self.pictures.push(*p),
            ShapeKind::Opaque { preview: None, .. } => self.not_drawn = true,
            ShapeKind::Shape | ShapeKind::Connector { .. } | ShapeKind::Chart(_) | ShapeKind::Ink { .. } => {}
        }
    }

    fn fill(&mut self, f: &Fill) {
        if let Fill::Picture(pf) = f {
            self.pictures.push(pf.media);
        }
    }

    fn line(&mut self, l: &Line) {
        if let Some(f) = &l.fill {
            self.fill(f);
        }
    }

    fn text(&mut self, t: &TextBody) {
        self.list_style(&t.list_style);
        for p in &t.paragraphs {
            self.para(&p.props);
            for r in &p.runs {
                self.run(&r.props);
            }
        }
    }

    fn list_style(&mut self, l: &ListStyle) {
        for level in l.levels.iter().flatten() {
            self.para(&level.para);
            self.run(&level.run);
        }
    }

    fn para(&mut self, p: &ParaProps) {
        if let Some(Bullet::Picture { media }) = &p.bullet {
            self.pictures.push(*media);
        }
    }

    fn run(&mut self, r: &RunProps) {
        if let Some(f) = &r.fill {
            self.fill(f);
        }
        if let Some(l) = &r.outline {
            self.line(l);
        }
    }
}
