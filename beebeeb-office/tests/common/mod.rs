//! Fixtures for the integration tests, every one built in code: no file produced by an office
//! suite is committed. The zip and compound-file writers follow APPNOTE.TXT and [MS-CFB]; the
//! `.doc` builder is adapted from WordCraft's own docbin test fixtures (`wordcraft/crates/docbin/
//! src/tests.rs`, MIT OR Apache-2.0), written from [MS-DOC].
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::time::Duration;

use beebeeb_office::{Document, RenderInfo};

/// One line of filler text: 44 characters, nine words, no wrapping at 12 pt on a Letter or A4 page.
pub const LINE: &str = "The quick brown fox jumps over the lazy dog.";

// ---------------------------------------------------------------- pixels

/// Render the whole of `page` at `scale` into a buffer sized by `render_size`.
pub fn render(doc: &Document, page: u32, scale: f32) -> (RenderInfo, Vec<u8>) {
    let (w, h) = doc.render_size(page, scale, None).expect("render size");
    let mut out = vec![0u8; w as usize * h as usize * 4];
    let info = doc.render_page(page, scale, None, &mut out).expect("render");
    (info, out)
}

/// (distinct RGBA colours, share of the pixels the most common colour covers).
pub fn colour_stats(rgba: &[u8]) -> (usize, f64) {
    let mut counts: HashMap<[u8; 4], usize> = HashMap::new();
    for px in rgba.chunks_exact(4) {
        *counts.entry([px[0], px[1], px[2], px[3]]).or_default() += 1;
    }
    let total = (rgba.len() / 4).max(1);
    let top = counts.values().copied().max().unwrap_or(0);
    (counts.len(), top as f64 / total as f64)
}

/// Write the raster as `$BB_OUT/<name>.png` when `BB_OUT` is set, so the checkpoint can look at
/// it; nothing is written otherwise.
pub fn save_png(name: &str, w: u32, h: u32, rgba: &[u8]) {
    let Some(dir) = std::env::var_os("BB_OUT") else { return };
    let png = wordcraft_render::Rendered { width: w, height: h, pixels: rgba.to_vec() }.to_png();
    assert!(!png.is_empty(), "the PNG of {name} did not encode");
    std::fs::create_dir_all(&dir).expect("create BB_OUT");
    std::fs::write(Path::new(&dir).join(format!("{name}.png")), png).expect("write the PNG to BB_OUT");
}

/// The canary picture: 64 x 64 of pure magenta, a colour nothing else in these fixtures uses.
pub fn canary_png() -> Vec<u8> {
    let pixels = [0xFF, 0x00, 0xFF, 0xFF].repeat(64 * 64);
    let png = wordcraft_render::Rendered { width: 64, height: 64, pixels }.to_png();
    assert!(!png.is_empty(), "the canary PNG did not encode");
    png
}

/// Pixels of the canary's colour in a rendered page.
pub fn canary_pixels(rgba: &[u8]) -> usize {
    rgba.chunks_exact(4).filter(|p| p[0] >= 200 && p[1] <= 80 && p[2] >= 200).count()
}

// ---------------------------------------------------------------- zip

/// CRC-32 (ISO-HDLC, as zip uses it).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// One zip entry as written: `method` 0 (stored) or 8 (deflate), the bytes as stored, their CRC,
/// and the unpacked size the headers declare.
pub struct RawEntry<'a> {
    pub name: &'a str,
    pub method: u16,
    pub data: &'a [u8],
    pub crc: u32,
    pub unpacked: u32,
}

/// A zip archive of `entries` (APPNOTE.TXT §4.3.7 local headers, §4.3.12 central directory,
/// §4.3.16 end record), no ZIP64.
pub fn zip_raw(entries: &[RawEntry<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for e in entries {
        let offset = out.len() as u32;
        let name_len = e.name.len() as u16;
        let packed = e.data.len() as u32;
        let method = e.method.to_le_bytes();
        // Local file header: version needed 2.0, no flags, method, time 0, date 1980-01-01.
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&[20, 0, 0, 0, method[0], method[1], 0, 0, 0x21, 0]);
        out.extend_from_slice(&e.crc.to_le_bytes());
        out.extend_from_slice(&packed.to_le_bytes());
        out.extend_from_slice(&e.unpacked.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(e.name.as_bytes());
        out.extend_from_slice(e.data);
        // Its central directory header.
        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, method[0], method[1], 0, 0, 0x21, 0]);
        central.extend_from_slice(&e.crc.to_le_bytes());
        central.extend_from_slice(&packed.to_le_bytes());
        central.extend_from_slice(&e.unpacked.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        // Extra and comment lengths, disk, internal and external attributes: all 0.
        central.extend_from_slice(&[0; 12]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(e.name.as_bytes());
    }
    let cd_offset = out.len() as u32;
    let cd_len = central.len() as u32;
    out.extend_from_slice(&central);
    let count = entries.len() as u16;
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// A zip archive of stored (uncompressed) entries.
pub fn zip_stored(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let raw: Vec<RawEntry<'_>> =
        entries.iter().map(|(name, data)| RawEntry { name, method: 0, data, crc: crc32(data), unpacked: data.len() as u32 }).collect();
    zip_raw(&raw)
}

/// An empty deflate stream: one final stored block of length 0.
pub const EMPTY_DEFLATE: [u8; 5] = [0x01, 0x00, 0x00, 0xFF, 0xFF];

/// A zip bomb's declaration: each entry is 5 bytes of deflate that inflate to nothing, but its
/// headers declare `unpacked` bytes. The archive is a few hundred bytes.
pub fn zip_declaring(entries: &[(&str, u32)]) -> Vec<u8> {
    let raw: Vec<RawEntry<'_>> =
        entries.iter().map(|(name, unpacked)| RawEntry { name, method: 8, data: &EMPTY_DEFLATE, crc: 0, unpacked: *unpacked }).collect();
    zip_raw(&raw)
}

// ---------------------------------------------------------------- docx

pub const NS_W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
pub const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub const NS_WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
pub const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
pub const NS_PIC: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
pub const NS_V: &str = "urn:schemas-microsoft-com:vml";
pub const NS_O: &str = "urn:schemas-microsoft-com:office:office";
pub const NS_PKG_RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
pub const NS_TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
pub const REL_DOCUMENT: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
pub const REL_IMAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
pub const REL_HYPERLINK: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";
pub const REL_SETTINGS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings";
pub const REL_TEMPLATE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/attachedTemplate";
pub const XML_DECL: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;

/// `s` safe inside an XML attribute or text.
pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// A WordprocessingML package written by hand: `body` inside `w:body` (a Letter page with 1"
/// margins follows it), `rels` inside `word/_rels/document.xml.rels`, and the extra `parts`.
pub fn docx_package(body: &str, rels: &str, parts: &[(&str, &[u8])]) -> Vec<u8> {
    let settings_type = if parts.iter().any(|(name, _)| *name == "word/settings.xml") {
        r#"<Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/>"#
    } else {
        ""
    };
    let content_types = format!(
        r#"{XML_DECL}<Types xmlns="{NS_TYPES}"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>{settings_type}</Types>"#
    );
    let root_rels =
        format!(r#"{XML_DECL}<Relationships xmlns="{NS_PKG_RELS}"><Relationship Id="rId1" Type="{REL_DOCUMENT}" Target="word/document.xml"/></Relationships>"#);
    let document = format!(
        r#"{XML_DECL}<w:document xmlns:w="{NS_W}" xmlns:r="{NS_R}" xmlns:wp="{NS_WP}" xmlns:a="{NS_A}" xmlns:pic="{NS_PIC}" xmlns:v="{NS_V}" xmlns:o="{NS_O}"><w:body>{body}<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    );
    let document_rels = format!(r#"{XML_DECL}<Relationships xmlns="{NS_PKG_RELS}">{rels}</Relationships>"#);
    let mut entries: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", content_types.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("word/document.xml", document.as_bytes()),
        ("word/_rels/document.xml.rels", document_rels.as_bytes()),
    ];
    entries.extend_from_slice(parts);
    zip_stored(&entries)
}

/// A paragraph holding one inline DrawingML picture, 2" x 2" (144 x 144 pt), whose `a:blip`
/// carries `blip` (`r:embed="…"` or `r:link="…"`).
pub fn picture_paragraph(blip: &str, id: u32) -> String {
    format!(
        r#"<w:p><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="1828800" cy="1828800"/><wp:docPr id="{id}" name="Picture {id}"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="{id}" name="x.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip {blip}/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1828800" cy="1828800"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
    )
}

/// A document that names `target` in every way a Word file can point outside itself: a DrawingML
/// picture (`r:embed`), a linked-only picture (`r:link`), a VML picture (`v:imagedata`), an
/// `INCLUDEPICTURE` field, a hyperlink and an attached template. The three picture
/// relationships are `TargetMode="External"` when `external`; the hyperlink and the template
/// always are. With `canary_part`, the package also holds the canary at `word/media/canary.png`,
/// so `linked_docx("media/canary.png", false, true)` is the same document with its pictures
/// embedded: the positive control, which draws the canary.
pub fn linked_docx(target: &str, external: bool, canary_part: bool) -> Vec<u8> {
    let t = xml_escape(target);
    let mode = if external { r#" TargetMode="External""# } else { "" };
    let intro = "<w:p><w:r><w:t>The pictures below point outside this file.</w:t></w:r></w:p>";
    let embedded = picture_paragraph(r#"r:embed="rIdPic""#, 1);
    let linked = picture_paragraph(r#"r:link="rIdLink""#, 2);
    let vml = r#"<w:p><w:r><w:pict><v:shape id="s3" style="width:144pt;height:144pt"><v:imagedata r:id="rIdVml" o:title=""/></v:shape></w:pict></w:r></w:p>"#;
    let field = format!(r#"<w:p><w:fldSimple w:instr=" INCLUDEPICTURE &quot;{t}&quot; \d "><w:r><w:t>linked</w:t></w:r></w:fldSimple></w:p>"#);
    let href = r#"<w:p><w:hyperlink r:id="rIdHref"><w:r><w:t>a link</w:t></w:r></w:hyperlink></w:p>"#;
    let body = format!("{intro}{embedded}{linked}{vml}{field}{href}");
    let rels = format!(
        r#"<Relationship Id="rIdPic" Type="{REL_IMAGE}" Target="{t}"{mode}/><Relationship Id="rIdLink" Type="{REL_IMAGE}" Target="{t}"{mode}/><Relationship Id="rIdVml" Type="{REL_IMAGE}" Target="{t}"{mode}/><Relationship Id="rIdHref" Type="{REL_HYPERLINK}" Target="{t}" TargetMode="External"/><Relationship Id="rIdSettings" Type="{REL_SETTINGS}" Target="settings.xml"/>"#
    );
    let settings = format!(r#"{XML_DECL}<w:settings xmlns:w="{NS_W}" xmlns:r="{NS_R}"><w:attachedTemplate r:id="rIdTpl"/></w:settings>"#);
    let settings_rels = format!(
        r#"{XML_DECL}<Relationships xmlns="{NS_PKG_RELS}"><Relationship Id="rIdTpl" Type="{REL_TEMPLATE}" Target="{t}" TargetMode="External"/></Relationships>"#
    );
    let canary = canary_png();
    let mut parts: Vec<(&str, &[u8])> = vec![("word/settings.xml", settings.as_bytes()), ("word/_rels/settings.xml.rels", settings_rels.as_bytes())];
    if canary_part {
        parts.push(("word/media/canary.png", canary.as_slice()));
    }
    docx_package(&body, &rels, &parts)
}

/// A `file:` URL for `path` (`file:///C:/…` on Windows, `file:///tmp/…` elsewhere).
pub fn file_url(path: &Path) -> String {
    let s = path.display().to_string().replace('\\', "/");
    if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
}

/// The dense two-page document of the first proof, as a model: page 1 is a shaded, coloured
/// heading and 20 lines of text in two colours, then a page break; page 2 a heading and 3 lines.
/// Both headings are outline level 1. Word count: 2 + 20 x 9 + 2 + 3 x 9 = 211.
pub fn dense_model() -> wordcraft_doc::Document {
    use wordcraft_doc::{CharProps, Paragraph, Rgb, TextColor, para_block};
    let heading = CharProps {
        size: Some(28.0),
        bold: Some(true),
        color: Some(TextColor::Rgb(Rgb(0x1F, 0x4E, 0x79))),
        shading: Some(Rgb(0xFF, 0xE0, 0x99)),
        ..CharProps::default()
    };
    let body = CharProps { size: Some(12.0), ..CharProps::default() };
    let red = CharProps { size: Some(12.0), color: Some(TextColor::Rgb(Rgb(0xC0, 0x20, 0x20))), ..CharProps::default() };
    let mut first = Paragraph::with_text("Page one", heading.clone());
    first.props.outline_level = Some(0);
    let mut second = Paragraph::with_text("Page two", heading);
    second.props.outline_level = Some(0);
    let mut blocks = vec![para_block(first)];
    for i in 0..20 {
        blocks.push(para_block(Paragraph::with_text(LINE, if i % 2 == 0 { body.clone() } else { red.clone() })));
    }
    blocks.push(para_block(Paragraph::with_text("\u{000C}", CharProps::default())));
    blocks.push(para_block(second));
    for _ in 0..3 {
        blocks.push(para_block(Paragraph::with_text(LINE, body.clone())));
    }
    let mut doc = wordcraft_doc::Document::new();
    doc.body = blocks;
    doc
}

/// [`dense_model`] written by the upstream `.docx` writer.
pub fn dense_docx() -> Vec<u8> {
    wordcraft_docx::write(&dense_model()).expect("write the dense docx")
}

// ---------------------------------------------------------------- compound file and .doc

const CFB_FREE: u32 = 0xFFFF_FFFF;
const CFB_END_OF_CHAIN: u32 = 0xFFFF_FFFE;
const CFB_FAT_SECTOR: u32 = 0xFFFF_FFFD;
const CFB_NO_STREAM: u32 = 0xFFFF_FFFF;
const CFB_SECTOR: usize = 512;
/// Streams below this size live in the mini stream; the writer pads every stream up to it, so
/// none does.
const CFB_MINI_CUTOFF: usize = 4096;

fn put16(v: &mut [u8], at: usize, x: u16) {
    v[at..at + 2].copy_from_slice(&x.to_le_bytes());
}

fn put32(v: &mut [u8], at: usize, x: u32) {
    v[at..at + 4].copy_from_slice(&x.to_le_bytes());
}

fn put64(v: &mut [u8], at: usize, x: u64) {
    v[at..at + 8].copy_from_slice(&x.to_le_bytes());
}

/// A compound file ([MS-CFB] version 3, 512-byte sectors) holding `streams` at its root, in that
/// order as directory entries 1, 2, …: sector 0 is the FAT, sector 1 the directory, then each
/// stream in consecutive sectors, padded to at least 4,096 bytes so no mini stream is needed. At
/// most 3 streams and 128 sectors (one FAT sector, one directory sector).
pub fn cfb_file(streams: &[(&str, Vec<u8>)]) -> Vec<u8> {
    assert!(streams.len() <= CFB_SECTOR / 128 - 1, "one directory sector holds the root and 3 streams");
    let data: Vec<Vec<u8>> = streams
        .iter()
        .map(|(_, d)| {
            let mut d = d.clone();
            if d.len() < CFB_MINI_CUTOFF {
                d.resize(CFB_MINI_CUTOFF, 0);
            }
            d
        })
        .collect();
    let mut fat: Vec<u32> = vec![CFB_FAT_SECTOR, CFB_END_OF_CHAIN];
    let mut starts = Vec::new();
    for d in &data {
        let n = d.len().div_ceil(CFB_SECTOR);
        let start = fat.len() as u32;
        starts.push(start);
        for k in 0..n {
            fat.push(if k + 1 == n { CFB_END_OF_CHAIN } else { start + k as u32 + 1 });
        }
    }
    assert!(fat.len() <= CFB_SECTOR / 4, "one FAT sector covers 128 sectors");
    fat.resize(CFB_SECTOR / 4, CFB_FREE);

    // Header ([MS-CFB] §2.2).
    let mut out = vec![0u8; CFB_SECTOR];
    out[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    put16(&mut out, 0x18, 0x003E); // minor version
    put16(&mut out, 0x1A, 0x0003); // major version 3
    put16(&mut out, 0x1C, 0xFFFE); // byte order
    put16(&mut out, 0x1E, 9); // sector shift: 512
    put16(&mut out, 0x20, 6); // mini sector shift: 64
    put32(&mut out, 0x2C, 1); // FAT sectors
    put32(&mut out, 0x30, 1); // first directory sector
    put32(&mut out, 0x38, CFB_MINI_CUTOFF as u32);
    put32(&mut out, 0x3C, CFB_END_OF_CHAIN); // first mini FAT sector: none
    put32(&mut out, 0x44, CFB_END_OF_CHAIN); // first DIFAT sector: none
    put32(&mut out, 0x4C, 0); // DIFAT[0]: the FAT is sector 0
    for i in 1..109 {
        put32(&mut out, 0x4C + i * 4, CFB_FREE);
    }

    // Sector 0: the FAT.
    for e in &fat {
        out.extend_from_slice(&e.to_le_bytes());
    }

    // Sector 1: the directory ([MS-CFB] §2.6). Siblings must be ordered by name (shorter first,
    // then upper-cased): each stream's left sibling is the one before it in that order, and the
    // root's child is the last.
    let mut dir = vec![0u8; CFB_SECTOR];
    let entry = |dir: &mut Vec<u8>, id: usize, name: &str, kind: u8, start: u32, len: u64| {
        let at = id * 128;
        let units: Vec<u16> = name.encode_utf16().collect();
        for (i, u) in units.iter().enumerate() {
            put16(dir, at + i * 2, *u);
        }
        put16(dir, at + 0x40, ((units.len() + 1) * 2) as u16);
        dir[at + 0x42] = kind;
        dir[at + 0x43] = 1; // black
        put32(dir, at + 0x44, CFB_NO_STREAM);
        put32(dir, at + 0x48, CFB_NO_STREAM);
        put32(dir, at + 0x4C, CFB_NO_STREAM);
        put32(dir, at + 0x74, start);
        put64(dir, at + 0x78, len);
    };
    entry(&mut dir, 0, "Root Entry", 5, CFB_END_OF_CHAIN, 0);
    for (i, ((name, _), d)) in streams.iter().zip(&data).enumerate() {
        entry(&mut dir, i + 1, name, 2, starts[i], d.len() as u64);
    }
    for id in streams.len() + 1..CFB_SECTOR / 128 {
        put32(&mut dir, id * 128 + 0x44, CFB_NO_STREAM);
        put32(&mut dir, id * 128 + 0x48, CFB_NO_STREAM);
        put32(&mut dir, id * 128 + 0x4C, CFB_NO_STREAM);
    }
    let mut order: Vec<usize> = (0..streams.len()).collect();
    order.sort_by_key(|&i| (streams[i].0.encode_utf16().count(), streams[i].0.to_uppercase()));
    for pair in order.windows(2) {
        put32(&mut dir, (pair[1] + 1) * 128 + 0x44, (pair[0] + 1) as u32);
    }
    if let Some(&last) = order.last() {
        put32(&mut dir, 0x4C, (last + 1) as u32);
    }
    out.extend_from_slice(&dir);

    // The streams, each padded to whole sectors.
    for d in &data {
        out.extend_from_slice(d);
        out.resize(out.len().div_ceil(CFB_SECTOR) * CFB_SECTOR, 0);
    }
    out
}

/// Offset in the WordDocument stream where the text is placed (past the FIB).
const DOC_TEXT_FC: u32 = 0x600;
/// Offset in the Table stream where the Clx (piece table) is placed.
const DOC_CLX_AT: u32 = 0x40;

/// A minimal Word 97 FIB ([MS-DOC] §2.5.1): FibBase, csw/rgw, cslw/rglw with `ccp_text`, and
/// 0x5D fc/lcb pairs of which pair 33 (`fcClx`/`lcbClx`) points at the piece table.
fn fib_bytes(ccp_text: u32, clx_len: u32) -> Vec<u8> {
    let mut v = vec![0u8; 0x9A + 0x5D * 8 + 2];
    put16(&mut v, 0x00, 0xA5EC); // wIdent
    put16(&mut v, 0x02, 0x00C1); // nFib
    put16(&mut v, 0x0A, 0x1000); // flags: fExtChar; fWhichTblStm 0, so the table is 0Table
    put16(&mut v, 0x0C, 0x00BF); // nFibBack
    put16(&mut v, 0x20, 0x000E); // csw
    put16(&mut v, 0x3E, 0x0016); // cslw
    put32(&mut v, 0x40, 0x1000); // cbMac
    put32(&mut v, 0x4C, ccp_text); // rglw: ccpText
    put16(&mut v, 0x98, 0x005D); // cbRgFcLcb
    put32(&mut v, 0x9A + 33 * 8, DOC_CLX_AT);
    put32(&mut v, 0x9A + 33 * 8 + 4, clx_len);
    v
}

/// A `.doc` whose main text is `text`: one compressed (Windows-1252) piece, `\r` ending each
/// paragraph, U+000C a page break; no styles, so the reader's defaults apply. ASCII only.
pub fn doc_from_text(text: &str) -> Vec<u8> {
    assert!(text.is_ascii(), "the fixture stores one byte per character");
    let chars = text.len() as u32;
    let mut word = vec![0u8; DOC_TEXT_FC as usize];
    word.extend_from_slice(text.as_bytes());
    assert!(word.len() <= 0x1000, "the text must end inside cbMac (4,096 bytes)");
    // PlcPcd: aCp[2] then one Pcd: 2 bytes of flags, the fc (compressed: twice the byte offset,
    // with bit 30 set), 2 bytes of prm.
    let mut plc = Vec::new();
    plc.extend_from_slice(&0u32.to_le_bytes());
    plc.extend_from_slice(&chars.to_le_bytes());
    plc.extend_from_slice(&[0, 0]);
    plc.extend_from_slice(&((2 * DOC_TEXT_FC) | 0x4000_0000).to_le_bytes());
    plc.extend_from_slice(&0u16.to_le_bytes());
    // Clx: a Pcdt (0x02, its length, the PlcPcd).
    let mut clx = vec![0x02u8];
    clx.extend_from_slice(&(plc.len() as u32).to_le_bytes());
    clx.extend_from_slice(&plc);
    let mut table = vec![0u8; DOC_CLX_AT as usize];
    table.extend_from_slice(&clx);
    let fib = fib_bytes(chars, clx.len() as u32);
    word[..fib.len()].copy_from_slice(&fib);
    cfb_file(&[("WordDocument", word), ("0Table", table)])
}

/// The dense two-page `.doc` of the first proof: a title and 34 lines of text (over 1,500
/// characters) on page 1, a page break, a title and 3 lines on page 2.
pub fn dense_doc() -> Vec<u8> {
    let mut text = String::from("Page one\r");
    for _ in 0..34 {
        text.push_str(LINE);
        text.push('\r');
    }
    text.push_str("\u{000C}\r");
    text.push_str("Page two\r");
    for _ in 0..3 {
        text.push_str(LINE);
        text.push('\r');
    }
    doc_from_text(&text)
}

// ---------------------------------------------------------------- network

/// A loopback listener that counts the connections made to it.
pub struct Listener {
    inner: TcpListener,
    pub addr: SocketAddr,
}

impl Listener {
    pub fn bind(addr: &str) -> std::io::Result<Listener> {
        let inner = TcpListener::bind(addr)?;
        inner.set_nonblocking(true)?;
        let addr = inner.local_addr()?;
        Ok(Listener { inner, addr })
    }

    /// Connections made since the last call. Waits 300 ms first, so a late attempt still counts.
    pub fn drain(&self) -> usize {
        std::thread::sleep(Duration::from_millis(300));
        let mut n = 0;
        loop {
            match self.inner.accept() {
                Ok(_) => n += 1,
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => panic!("the listener failed: {e}"),
            }
        }
        n
    }
}
