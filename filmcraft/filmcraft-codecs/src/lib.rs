//! The container + codec hub.
//!
//! - [`VideoDecoder`]: the trait every video codec implements (our own H.264/HEVC/VP9/ProRes/…, MJPEG, and
//!   OS hardware decoders registered by the platform layer). Factories are tried in registration
//!   order, so a hardware decoder can take precedence over the pure-Rust one.
//! - [`Mp4Source`]: a [`MediaSource`](filmcraft_media::MediaSource) over MP4/MOV using
//!   `filmcraft-isobmff`: GOP-aware random access (seek to the preceding sync sample and decode
//!   forward, caching every decoded frame of the GOP), sequential fast path for playback, and
//!   packet-cached audio decoding.
//! - [`MkvSource`], [`MxfSource`] (OP1a / OP-Atom: AVC, MPEG-2 incl. D-10 / XDCAM, VC-3, ProRes,
//!   PCM) and [`OggSource`] (Ogg Opus with granule-position seeking, Ogg Vorbis): the same
//!   GOP-aware video access and packet-cached audio.
//! - [`MpegSource`]: MPEG-2 transport streams (`.ts`, `.m2ts`, `.mts`), program streams (`.mpg`,
//!   `.vob`, `.mod`) and MPEG-1/2 video elementary streams: MPEG-1/2, H.264 and HEVC video; MPEG
//!   audio, AAC (ADTS / LATM), AC-3 and LPCM.
//! - [`AviSource`]: AVI (OpenDML included): Motion JPEG, H.264, HEVC and uncompressed video; PCM,
//!   MP3 / MP2 and AC-3 audio.
//! - [`AudioFileSource`]: standalone compressed audio files (MP3, FLAC, AIFF, …).
//! - [`Y4mSource`]: uncompressed YUV4MPEG2 (`.y4m`) video, frames read on demand.
//! - [`openers`]: the openers to register with the engine's media pool.
//!
//! Modified for the Beebeeb desktop app's media helper, 2026-11-15 (filmcraft/PATCHES.md): the H.264, HEVC, AAC, AC-3,
//! E-AC-3, MPEG-1/2 video, ProRes, DNxHD and APV decoders and the MXF container are Cargo features (all on by default);
//! a build without one does not contain it, and a stream that needs it is `CodecError::Unsupported`.

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo,
        clippy::unreachable
    )
)]

#[cfg(feature = "apv")]
pub mod apv;
pub mod audio;
// Modified (filmcraft/PATCHES.md): the AVC picture-order helper, shared by `avi` and `mxf`.
#[cfg(feature = "h264")]
mod avc_order;
pub mod avi;
pub mod gop;
pub mod hw;
mod hw_frame;
pub mod mkv;
pub mod mp4;
pub mod mpeg;
#[cfg(feature = "mxf")]
pub mod mxf;
pub mod ogg;
mod stream_color;
pub mod video;
mod webm_alpha;
pub mod y4m;

use std::sync::{Arc, RwLock};

#[cfg(feature = "apv")]
pub use apv::ApvSource;
pub use audio::AudioFileSource;
pub use avi::AviSource;
pub use gop::{FRAME_BUDGET, GopStats, cached_bytes, gop_stats, live_decoders, set_frame_budget};
pub use mkv::MkvSource;
pub use mp4::Mp4Source;
pub use mpeg::MpegSource;
#[cfg(feature = "mxf")]
pub use mxf::MxfSource;
pub use ogg::OggSource;
pub use video::{DecodedFrame, VideoDecoder, VideoDecoderFactory};
pub use y4m::Y4mSource;

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("unsupported codec: {0}")]
    Unsupported(String),
    #[error("decode error: {0}")]
    Decode(String),
    #[error("container: {0}")]
    Container(String),
    /// The frame is no longer wanted (`filmcraft_media::cancel`).
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, CodecError>;

impl From<CodecError> for filmcraft_media::MediaError {
    fn from(e: CodecError) -> Self {
        match e {
            CodecError::Unsupported(s) => filmcraft_media::MediaError::Unsupported(s),
            CodecError::Cancelled => filmcraft_media::MediaError::Cancelled,
            other => filmcraft_media::MediaError::Decode(other.to_string()),
        }
    }
}

/// Our own (pure-Rust) decoders in this build, in the order they are tried.
// Modified (filmcraft/PATCHES.md, patch 3): the entries sit behind their features, which `vec![]` cannot carry.
#[allow(clippy::vec_init_then_push)]
fn builtin_factories() -> Vec<VideoDecoderFactory> {
    let mut f: Vec<VideoDecoderFactory> = Vec::new();
    #[cfg(feature = "h264")]
    f.push(video::h264_factory);
    #[cfg(feature = "hevc")]
    f.push(video::hevc_factory);
    f.push(video::vp8_factory);
    f.push(video::vp9_factory);
    f.push(video::av1_factory);
    #[cfg(feature = "apv")]
    f.push(video::apv_factory);
    #[cfg(feature = "prores")]
    f.push(video::prores_factory);
    #[cfg(feature = "dnx")]
    f.push(video::dnx_factory);
    f.push(video::mjpeg_factory);
    #[cfg(feature = "mpeg2")]
    f.push(video::mpeg2_factory);
    f
}

fn factories() -> &'static RwLock<Vec<VideoDecoderFactory>> {
    static F: std::sync::OnceLock<RwLock<Vec<VideoDecoderFactory>>> = std::sync::OnceLock::new();
    F.get_or_init(|| RwLock::new(builtin_factories()))
}

/// Register a video decoder factory (tried before previously registered ones).
pub fn register_video_decoder(f: VideoDecoderFactory) {
    let mut g = factories().write().unwrap_or_else(|e| e.into_inner());
    if !g.iter().any(|x| std::ptr::fn_addr_eq(*x, f)) {
        g.insert(0, f);
    }
}

/// Whether `f` is among the registered video decoder factories (startup diagnostics, tests).
pub fn video_decoder_registered(f: VideoDecoderFactory) -> bool {
    factories()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|x| std::ptr::fn_addr_eq(*x, f))
}

/// Create a decoder for a sample entry.
pub fn make_video_decoder(entry: &filmcraft_isobmff::SampleEntry) -> Result<Box<dyn VideoDecoder>> {
    let g = factories().read().unwrap_or_else(|e| e.into_inner());
    for f in g.iter() {
        if let Some(r) = f(entry) {
            return r;
        }
    }
    Err(CodecError::Unsupported(format!(
        "no decoder for {} video",
        entry.codec.name()
    )))
}

/// Create one of our own (software) decoders for a sample entry, skipping registered factories:
/// what a hardware decoder falls back to when it fails mid-stream.
pub fn software_video_decoder(
    entry: &filmcraft_isobmff::SampleEntry,
) -> Result<Box<dyn VideoDecoder>> {
    for f in builtin_factories() {
        if let Some(r) = f(entry) {
            return r;
        }
    }
    Err(CodecError::Unsupported(format!(
        "no decoder for {} video",
        entry.codec.name()
    )))
}

/// Openers for the engine's media pool (MP4/MOV, Matroska/WebM, MXF, Ogg Opus/Vorbis, MPEG TS/PS and
/// MPEG-1/2 video elementary streams, APV raw bitstreams, YUV4MPEG2, standalone audio).
pub fn openers() -> Vec<filmcraft_media::Opener> {
    let mut o: Vec<filmcraft_media::Opener> = vec![mp4::opener, mkv::opener];
    #[cfg(feature = "mxf")]
    o.push(mxf::opener);
    o.push(ogg::opener);
    o.push(mpeg::opener);
    #[cfg(feature = "apv")]
    o.push(apv::opener);
    o.extend([
        avi::opener as filmcraft_media::Opener,
        y4m::opener,
        audio::opener,
    ]);
    o
}

fn reader_registry() -> &'static RwLock<Vec<filmcraft_media::ReaderOpener>> {
    static R: std::sync::OnceLock<RwLock<Vec<filmcraft_media::ReaderOpener>>> =
        std::sync::OnceLock::new();
    R.get_or_init(|| {
        let mut r: Vec<filmcraft_media::ReaderOpener> =
            vec![mp4::reader_opener, mkv::reader_opener];
        #[cfg(feature = "mxf")]
        r.push(mxf::reader_opener);
        r.push(ogg::reader_opener);
        r.push(mpeg::reader_opener);
        #[cfg(feature = "apv")]
        r.push(apv::reader_opener);
        r.push(y4m::reader_opener);
        r.push(avi::reader_opener);
        RwLock::new(r)
    })
}

/// Openers that read containers through a [`filmcraft_media::ByteReader`] (index now, samples on
/// demand) instead of the whole file, in the order they are tried.
pub fn reader_openers() -> Vec<filmcraft_media::ReaderOpener> {
    reader_registry()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Register a reader opener tried before the built-in ones (platform media sources such as the
/// web app's WebCodecs-decoded MP4).
pub fn register_reader_opener(f: filmcraft_media::ReaderOpener) {
    let mut g = reader_registry().write().unwrap_or_else(|e| e.into_inner());
    if !g.iter().any(|x| std::ptr::fn_addr_eq(*x, f)) {
        g.insert(0, f);
    }
}

/// A media reader as the demuxers' byte source.
#[derive(Clone)]
pub(crate) struct Src(pub filmcraft_media::SharedReader);

impl filmcraft_isobmff::ByteSource for Src {
    fn len(&self) -> u64 {
        self.0.len()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.0.read_at(offset, buf)
    }
}

impl filmcraft_avi::ByteSource for Src {
    fn len(&self) -> u64 {
        self.0.len()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.0.read_at(offset, buf)
    }
}

impl filmcraft_matroska::ByteSource for Src {
    fn len(&self) -> u64 {
        self.0.len()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.0.read_at(offset, buf)
    }
}

/// Convenience: an `Arc` media source from bytes (tries MP4/MOV then audio files).
pub fn open_bytes(
    name: &str,
    bytes: Arc<[u8]>,
) -> std::result::Result<filmcraft_media::SharedSource, filmcraft_media::MediaError> {
    filmcraft_media::open_bytes(name, bytes, &openers())
}

#[cfg(test)]
mod audio_timing_tests;
#[cfg(test)]
mod avi_tests;
#[cfg(test)]
mod rounded_pts_tests;
#[cfg(test)]
mod stream_color_tests;
#[cfg(test)]
mod tests;
