# Changes to the vendored FilmCraft

Commit 1 vendored every `filmcraft-*/` folder exactly as its crates.io tarball unpacks (`verify.sh` checks the tarballs;
`diff -r` against a fresh unpack of `tarballs/` showed nothing). Commit 2 makes each patent-pool codec an optional Cargo
feature, so a build that leaves one out does not contain it: not linked, not reachable, not in the binary. A route that is
turned off at run time is not enough (the legal brief on media codec patents, 2026-11-15, finding F1).

The change is `patches/filmcraft-features.patch` (made with `git diff` against commit 1; it applies with
`git apply` or `patch -p1` from the repository root). Commit 3 adds `patches/filmcraft-dng-avc-order.patch` (made with
`git diff` against commit 2, applied after the first): DNG becomes a feature, and the AVC order helper moves out of `mxf`.

## The features (`filmcraft-codecs`)
`default` keeps every codec, so a build that asks for nothing still gets upstream's behaviour. The media helper asks for
`default-features = false` plus the features cleared for it.

| Feature | Brings | Without it |
|---|---|---|
| `dng` | `filmcraft-media/dng`, so `filmcraft-dng` | DNG stills, CinemaDNG sequences and YCbCr TIFF stills are not read by `filmcraft-media` (a DNG is read as a plain TIFF; a YCbCr TIFF is refused by the `image` crate) |
| `h264` | `filmcraft-h264` | H.264 streams in MP4, Matroska, AVI and MPEG files are refused as "not in this build"; no SPS is parsed |
| `hevc` | `filmcraft-hevc` | the same for HEVC; the container's FourCC still names the stream ("HEVC") |
| `aac` | `filmcraft-aac` and symphonia's `aac` | AAC tracks are refused; nothing in the build decodes AAC |
| `ac3` | `filmcraft-ac3` without its E-AC-3 module | AC-3 tracks are refused |
| `eac3` | `ac3` plus `filmcraft-ac3/eac3` | E-AC-3 headers are not parsed; the E-AC-3 decoder is not built |
| `mpeg2` | `filmcraft-mpeg2v` | MPEG-1/2 video streams are refused |
| `prores` | `filmcraft-prores` | ProRes streams are refused (the container still names them) |
| `dnx` | `filmcraft-dnx` | DNxHD / DNxHR streams are refused |
| `apv` | `filmcraft-apv` and the `apv` module | APV files are not opened; Matroska / MP4 name the stream "APV" |
| `mxf` | `filmcraft-mxf` and the `mxf` module (needs `h264`, `mpeg2`, `dnx`) | MXF files are not opened |

`filmcraft-ac3` gets its own feature `eac3` (on by default): without it `src/eac3.rs` is replaced by `src/eac3_off.rs`,
which keeps the module's types and answers every E-AC-3 call with `Error::Unsupported`.

## Files changed
| File | Change |
|---|---|
| `filmcraft-codecs/Cargo.toml` | the nine codec crates optional; symphonia's `aac` moved behind `aac`; `[features]` above |
| `filmcraft-codecs/src/lib.rs` | `apv` and `mxf` modules and their re-exports behind their features; the decoder factories, openers and reader registry built with each codec's entry behind its feature |
| `filmcraft-codecs/src/video.rs` | the H.264, HEVC, ProRes, APV, DNx and MPEG-2 decoders and their helpers behind their features |
| `filmcraft-codecs/src/audio.rs` | the AAC and AC-3 decoders behind their features; without them the constructors answer `Unsupported` and the frame-size helpers answer `None` |
| `filmcraft-codecs/src/mp4.rs` | the DNx / ProRes probes, the APV labels and the H.264 / HEVC SPS parses behind their features (without one, the sample entry's own fields are used) |
| `filmcraft-codecs/src/mpeg.rs` | the elementary-stream decoders (H.264, HEVC, MPEG-2) behind their features; one function picks the Annex B decoder |
| `filmcraft-codecs/src/avi.rs` | the H.264 / HEVC decoders behind their features; the H.264 reorder pass behind `h264` (it was behind `mxf`; see below) |
| `filmcraft-codecs/src/hw.rs` | the `avcC` / `hvcC` SPS parses behind `h264` / `hevc` (without them the record is refused) |
| `filmcraft-codecs/src/stream_color.rs` | the H.264 / HEVC SPS colour parses behind their features (without them the container's colour is used) |
| `filmcraft-codecs/src/mkv.rs` | the APV profile name behind `apv` |
| `filmcraft-ac3/Cargo.toml` | `[features] default = ["eac3"]` |
| `filmcraft-ac3/src/lib.rs` | `mod eac3` from `eac3.rs` with the feature, from `eac3_off.rs` without |
| `filmcraft-ac3/src/eac3_off.rs` | new: the stub module |
| `filmcraft-media/Cargo.toml` | `filmcraft-dng` optional; `[features] default = ["dng"]`, `dng`; the `tiff_ycbcr` test needs `dng` |
| `filmcraft-media/src/still.rs` | the `dng` and `ycbcr_tiff` modules and their sniffs behind `dng`; without it a DNG is an ordinary TIFF (the `image` crate reads it, preview only) |
| `filmcraft-media/src/sequence.rs` | the DNG frame decode and the CinemaDNG timing behind `dng` |
| `filmcraft-codecs/Cargo.toml` | `filmcraft-media` with `default-features = false`; feature `dng = ["filmcraft-media/dng"]`, in `default` |
| `filmcraft-codecs/src/avc_order.rs` | new: `avc_presentation_order`, moved unchanged from `mxf.rs`; built with `h264` (uses `filmcraft-h264` and `filmcraft-bitstream`, never `filmcraft-mxf`) |
| `filmcraft-codecs/src/mxf.rs` | the function removed; it uses `crate::avc_order` |
| `filmcraft-codecs/src/avi.rs` | the H.264 reorder pass now behind `h264` (was `mxf`) and calls `crate::avc_order` |
| `filmcraft-codecs/src/lib.rs` | `mod avc_order` behind `h264` |

Each changed spot carries a `Modified (filmcraft/PATCHES.md)` comment.

## Known limits (with a feature off)
- The crates' own tests assume the default features (they encode test streams with the AAC, APV and ProRes encoders).
  Run them with the defaults.
- The `dng` feature of `filmcraft-media` is in its `default` (upstream's behaviour); every in-tree crate that depends on
  it asks for `default-features = false` and forwards `dng` in its own `default`, so a build with `default-features =
  false` has no `filmcraft-dng`. A crate outside this tree that names `filmcraft-media` with its defaults brings DNG back.

## Not changed
No decoder, demuxer or parser is changed in behaviour when its feature is on: with the default features the code paths
are upstream's, line for line.
