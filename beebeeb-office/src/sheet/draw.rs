//! Drawing a sheet's charts and pictures into a buffer the caller sized.
//!
//! A chart is laid out once in points by `gridcraft-chart` (with text measured in a bundled face),
//! its shapes scaled to pixels and rasterised by the chart crate, and its text drawn here: the
//! chart rasteriser skips text, so each label is rendered as one line by `wordcraft-render` in the
//! same bundled face and laid over the shapes, turned as the chart asks (the value axis title is a
//! quarter turn). No font is downloaded and no system font is needed for a chart.
//!
//! A picture is checked from its header against the picture limits before anything decodes it,
//! decoded by the decoder the slides renderer links, and resampled to the anchor's pixels.

use std::sync::Arc;

use gridcraft_chart::{HAlign, Prim, Rgba, VAlign};
use wordcraft_fonts::FontFace;

use crate::{Limits, RenderError, probe};

/// The background a chart is drawn on (Excel's default chart area).
const WHITE: Rgba = [0xFF, 0xFF, 0xFF, 0xFF];
/// The fill and edge of the frame drawn where a picture cannot be shown.
const FRAME_FILL: Rgba = [0xF2, 0xF2, 0xF2, 0xFF];
const FRAME_EDGE: Rgba = [0xBF, 0xBF, 0xBF, 0xFF];

/// Draw `chart` (of sheet `sheet`) laid out in `size_pt` at `scale` into `out`, `px` pixels.
/// Returns whether something of it could not be drawn: no layout at all (the frame is left
/// white), a label no bundled glyph covers, a label that did not render.
pub(super) fn chart(
    book: &gridcraft_model::Workbook,
    sheet: usize,
    chart: &gridcraft_model::Chart,
    size_pt: (f32, f32),
    scale: f32,
    px: (u32, u32),
    out: &mut [u8],
) -> Result<bool, RenderError> {
    let (w, h) = px;
    let data = gridcraft_chart::resolve(book, sheet, chart);
    let faces = Faces::new();
    let prims = gridcraft_chart::render(chart, &data, size_pt.0, size_pt.1, &faces);
    if prims.is_empty() {
        fill(out, WHITE);
        return Ok(true);
    }
    let shapes: Vec<Prim> = prims.iter().filter(|p| !matches!(p, Prim::Text { .. })).map(|p| scaled(p, scale)).collect();
    let raster = gridcraft_chart::rasterize(&shapes, w, h, WHITE);
    if raster.len() != out.len() {
        return Err(RenderError::Engine);
    }
    out.copy_from_slice(&raster);
    let mut missing = false;
    for p in &prims {
        if let Prim::Text { .. } = p {
            missing |= !text(out, (w, h), scale, &faces, p);
        }
    }
    Ok(missing)
}

/// Whether a picture's header says it is over the picture limits. Bytes that are not a raster
/// picture the header reader knows (a metafile) are checked after decoding instead.
pub(super) fn over_limits(data: &[u8], limits: &Limits) -> bool {
    match wordcraft_render::image_size(data) {
        Some((w, h)) => !within(w, h, limits),
        None => false,
    }
}

fn within(w: u32, h: u32, limits: &Limits) -> bool {
    w > 0 && h > 0 && w <= limits.max_image_side_px && h <= limits.max_image_side_px && u64::from(w) * u64::from(h) <= limits.max_image_pixels
}

/// Draw a picture into `out`, `px` pixels, resampled to fill it. A picture over the limits
/// (`over`, from its header) is never handed to the decoder; one that does not decode, or decodes
/// over the limits, is drawn as an empty frame. Returns whether the picture is not shown.
pub(super) fn picture(data: &[u8], over: bool, limits: &Limits, px: (u32, u32), out: &mut [u8]) -> bool {
    let (w, h) = px;
    if over {
        frame(out, w, h);
        return true;
    }
    probe::handed(data);
    let Some(img) = deckcraft_render::decode_image(data) else {
        frame(out, w, h);
        return true;
    };
    let (iw, ih) = (img.width(), img.height());
    let src = img.as_raw();
    if !within(iw, ih, limits) || crate::raster::rgba_len(iw, ih) != Some(src.len()) {
        frame(out, w, h);
        return true;
    }
    resample(src, (iw, ih), out, (w, h));
    false
}

/// The two bundled faces chart text is measured and drawn in (Source Sans 3, the readers' own
/// fallback family): Regular and Bold.
struct Faces {
    regular: Arc<FontFace>,
    bold: Arc<FontFace>,
}

impl Faces {
    fn new() -> Faces {
        let db = wordcraft_fonts::FontDb::global();
        Faces { regular: db.face(wordcraft_fonts::FALLBACK_FAMILY, "Regular"), bold: db.face(wordcraft_fonts::FALLBACK_FAMILY, "Bold") }
    }

    fn face(&self, bold: bool) -> &FontFace {
        if bold { &self.bold } else { &self.regular }
    }
}

/// Width of `text` at `size` points in `face`, from its shaped advances.
fn text_width(face: &FontFace, text: &str, size: f32) -> f32 {
    let upem = if face.upem.is_finite() && face.upem > 0.0 { face.upem } else { 1000.0 };
    let advance: f64 = wordcraft_fonts::shape(face, text, &[], |c| c).iter().map(|g| f64::from(g.x_advance)).sum();
    (advance * f64::from(size) / upem) as f32
}

impl gridcraft_chart::Measure for Faces {
    fn text_width(&self, text: &str, size: f32, bold: bool) -> f32 {
        text_width(self.face(bold), text, size)
    }
}

/// A shape primitive in points, scaled to pixels. Every variant is listed: a new one upstream is
/// a compile error here.
fn scaled(p: &Prim, s: f32) -> Prim {
    let pt = |q: &[f32; 2]| [q[0] * s, q[1] * s];
    let edge = |e: &Option<(Rgba, f32)>| e.map(|(c, w)| (c, w * s));
    match p {
        Prim::Rect { x, y, w, h, fill, stroke, radius } => {
            Prim::Rect { x: x * s, y: y * s, w: w * s, h: h * s, fill: *fill, stroke: edge(stroke), radius: radius * s }
        }
        Prim::Line { pts, color, width, dash } => Prim::Line { pts: pts.iter().map(pt).collect(), color: *color, width: width * s, dash: *dash },
        Prim::Polygon { pts, fill, stroke } => Prim::Polygon { pts: pts.iter().map(pt).collect(), fill: *fill, stroke: edge(stroke) },
        Prim::Wedge { cx, cy, r_outer, r_inner, a0, a1, fill, stroke } => Prim::Wedge {
            cx: cx * s,
            cy: cy * s,
            r_outer: r_outer * s,
            r_inner: r_inner * s,
            a0: *a0,
            a1: *a1,
            fill: *fill,
            stroke: edge(stroke),
        },
        Prim::Circle { cx, cy, r, fill, stroke } => Prim::Circle { cx: cx * s, cy: cy * s, r: r * s, fill: *fill, stroke: edge(stroke) },
        Prim::Text { .. } => p.clone(),
    }
}

/// Lay one text primitive (chart points) over `out` at `scale`. Returns false when it could not be
/// drawn as the chart means it: a character no bundled glyph covers, or no raster for the line.
fn text(out: &mut [u8], px: (u32, u32), scale: f32, faces: &Faces, p: &Prim) -> bool {
    let Prim::Text { x, y, text, size, color, bold, align, valign, rotation } = p else { return true };
    let face = faces.face(*bold);
    let covered = text.chars().all(|c| c.is_whitespace() || c.is_control() || face.covers(c));
    let tw = text_width(face, text, *size);
    // Room around the line so no glyph is cut: a quarter of the size each side, 1.4 x the size tall.
    let pad = size * 0.25;
    let (box_w, box_h) = (tw + 2.0 * pad, size * 1.4);
    if !(tw.is_finite() && box_w.is_finite() && box_h.is_finite() && box_w > 0.0 && box_h > 0.0) {
        return false;
    }
    let line = wordcraft_render::render_text_line(face, text, *size, box_w, box_h, pad, scale);
    if line.width == 0 || line.height == 0 || crate::raster::rgba_len(line.width, line.height) != Some(line.pixels.len()) {
        return false;
    }
    let ax = match align {
        HAlign::Left => 0.0,
        HAlign::Center => 0.5,
        HAlign::Right => 1.0,
    };
    let ay = match valign {
        VAlign::Top => 0.0,
        VAlign::Middle => 0.5,
        VAlign::Bottom => 1.0,
    };
    // The line's box in text-local points (u along the text, v down it), origin at the anchor.
    let (left, top) = (-tw * ax - pad, -box_h * ay);
    // Line raster pixels per local point (the renderer may have fitted a huge line smaller).
    let (kx, ky) = (line.width as f32 / box_w, line.height as f32 / box_h);
    let (cos, sin) = (rotation.cos(), rotation.sin());
    // The turned box's bounds in output pixels.
    let corners = [(left, top), (left + box_w, top), (left, top + box_h), (left + box_w, top + box_h)];
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (u, v) in corners {
        let (cx, cy) = ((x + u * cos - v * sin) * scale, (y + u * sin + v * cos) * scale);
        (x0, y0, x1, y1) = (x0.min(cx), y0.min(cy), x1.max(cx), y1.max(cy));
    }
    if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
        return false;
    }
    let (w, h) = px;
    let span = |lo: f32, hi: f32, n: u32| (lo.floor().clamp(0.0, n as f32) as u32, hi.ceil().clamp(0.0, n as f32) as u32);
    let ((px0, px1), (py0, py1)) = (span(x0, x1, w), span(y0, y1, h));
    for oy in py0..py1 {
        for ox in px0..px1 {
            // The output pixel's centre in text-local points, turned back.
            let (dx, dy) = ((ox as f32 + 0.5) / scale - x, (oy as f32 + 0.5) / scale - y);
            let (u, v) = (dx * cos + dy * sin, -dx * sin + dy * cos);
            let (su, sv) = ((u - left) * kx, (v - top) * ky);
            if !(su >= 0.0 && sv >= 0.0) {
                continue;
            }
            let (si, sj) = (su as u32, sv as u32);
            if si >= line.width || sj >= line.height {
                continue;
            }
            // Black on white, premultiplied: the red channel is 255 minus the coverage.
            let s = (sj as usize * line.width as usize + si as usize) * 4;
            let coverage = 255 - line.pixels[s];
            if coverage == 0 {
                continue;
            }
            let a = f32::from(coverage) / 255.0 * f32::from(color[3]) / 255.0;
            let d = (oy as usize * w as usize + ox as usize) * 4;
            for c in 0..3 {
                let under = f32::from(out[d + c]);
                out[d + c] = (under + (f32::from(color[c]) - under) * a).round().clamp(0.0, 255.0) as u8;
            }
            let under = f32::from(out[d + 3]);
            out[d + 3] = (under + (255.0 - under) * a).round().clamp(0.0, 255.0) as u8;
        }
    }
    covered
}

fn fill(out: &mut [u8], c: Rgba) {
    for px in out.chunks_exact_mut(4) {
        px.copy_from_slice(&c);
    }
}

/// The frame drawn where a picture is not shown: a light fill with a one-pixel edge.
fn frame(out: &mut [u8], w: u32, h: u32) {
    fill(out, FRAME_FILL);
    let (w, h) = (w as usize, h as usize);
    for (i, px) in out.chunks_exact_mut(4).enumerate() {
        let (x, y) = (i % w, i / w);
        if x == 0 || y == 0 || x + 1 == w || y + 1 == h {
            px.copy_from_slice(&FRAME_EDGE);
        }
    }
}

/// Resample straight RGBA `src` (`sw` x `sh`) to `out` (`dw` x `dh`), in premultiplied alpha so
/// transparent edges do not darken: the average of the source pixels under each output pixel
/// when shrinking, bilinear when growing.
fn resample(src: &[u8], (sw, sh): (u32, u32), out: &mut [u8], (dw, dh): (u32, u32)) {
    let (fx, fy) = (sw as f32 / dw as f32, sh as f32 / dh as f32);
    let at = |x: u32, y: u32| (y as usize * sw as usize + x as usize) * 4;
    let mut write = |x: u32, y: u32, acc: [f32; 4]| {
        let d = (y as usize * dw as usize + x as usize) * 4;
        let a = acc[3];
        for c in 0..3 {
            out[d + c] = if a > 0.0 { (acc[c] / a).round().clamp(0.0, 255.0) as u8 } else { 0 };
        }
        out[d + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
    };
    let add = |acc: &mut [f32; 4], i: usize, weight: f32| {
        let a = f32::from(src[i + 3]) / 255.0 * weight;
        for c in 0..3 {
            acc[c] += f32::from(src[i + c]) * a;
        }
        acc[3] += a;
    };
    for y in 0..dh {
        for x in 0..dw {
            let mut acc = [0.0f32; 4];
            if fx > 1.0 || fy > 1.0 {
                let (sx0, sy0) = (((x as f32 * fx) as u32).min(sw - 1), ((y as f32 * fy) as u32).min(sh - 1));
                let sx1 = (((x + 1) as f32 * fx).ceil() as u32).clamp(sx0 + 1, sw);
                let sy1 = (((y + 1) as f32 * fy).ceil() as u32).clamp(sy0 + 1, sh);
                let n = ((sx1 - sx0) * (sy1 - sy0)) as f32;
                for yy in sy0..sy1 {
                    for xx in sx0..sx1 {
                        add(&mut acc, at(xx, yy), 1.0 / n);
                    }
                }
            } else {
                let sxf = ((x as f32 + 0.5) * fx - 0.5).clamp(0.0, (sw - 1) as f32);
                let syf = ((y as f32 + 0.5) * fy - 0.5).clamp(0.0, (sh - 1) as f32);
                let (sx, sy) = (sxf.floor() as u32, syf.floor() as u32);
                let (sx2, sy2) = ((sx + 1).min(sw - 1), (sy + 1).min(sh - 1));
                let (tx, ty) = (sxf - sx as f32, syf - sy as f32);
                add(&mut acc, at(sx, sy), (1.0 - tx) * (1.0 - ty));
                add(&mut acc, at(sx2, sy), tx * (1.0 - ty));
                add(&mut acc, at(sx, sy2), (1.0 - tx) * ty);
                add(&mut acc, at(sx2, sy2), tx * ty);
            }
            write(x, y, acc);
        }
    }
}
