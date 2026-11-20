# Changes to the vendored FilmCraft

Commit 1 vendored every `filmcraft-*/` folder exactly as its crates.io tarball unpacks (`verify.sh` checks the tarballs;
`diff -r` against a fresh unpack of `tarballs/` showed nothing). Commit 2 makes each patent-pool codec an optional Cargo
feature, so a build that leaves one out does not contain it: not linked, not reachable, not in the binary. A route that is
turned off at run time is not enough (the legal brief on media codec patents, 2026-11-15, finding F1).
Commit 3 makes DNG a feature of `filmcraft-media` too, and moves the AVC order helper out of the `mxf` module. Commit 4
quiets the warnings the first two left in builds without some features (lint attributes only) and formats patch 2's
AVI lines.

## The patches, in the order they apply
All three are made with `git diff` and apply with `git apply` or `patch -p1` from the repository root, in this order:

| # | Patch | Made against | What it does |
|---|---|---|---|
| 1 | `patches/filmcraft-features.patch` (commit 2) | commit 1, the unpacked tarballs | each patent-pool codec an optional feature of `filmcraft-codecs`; E-AC-3 a feature of `filmcraft-ac3` |
| 2 | `patches/filmcraft-dng-avc-order.patch` (commit 3) | commit 2, so patch 1 applied | DNG a feature of `filmcraft-media`; the AVC order helper moves out of `mxf` into `avc_order.rs` |
| 3 | `patches/filmcraft-lints.patch` (commit 4) | commit 3, so patches 1 and 2 applied | lint attributes for code a build without some features leaves unused; rustfmt of patch 2's AVI lines. No code path changes |

Patch 2 does not apply on its own: its context is patch 1's text (`filmcraft-codecs/Cargo.toml`, `src/lib.rs`,
`src/avi.rs` carry both); patch 3 likewise needs both. `patches-check.sh` rebuilds the tree from `tarballs/` plus the
three patches in this order and compares it with every `filmcraft-<x>/` here (prints `26 of 26 crates equal …, 0
differ`); naming patch 1 alone shows it failing (2 crates differ), naming patches 2 and 1 shows it refusing (patch 2
does not apply).

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
| Patch | File | Change |
|---|---|---|
| 1 | `filmcraft-codecs/Cargo.toml` | the nine codec crates optional; symphonia's `aac` moved behind `aac`; `[features]` above |
| 1 | `filmcraft-codecs/src/lib.rs` | `apv` and `mxf` modules and their re-exports behind their features; the decoder factories, openers and reader registry built with each codec's entry behind its feature |
| 1 | `filmcraft-codecs/src/video.rs` | the H.264, HEVC, ProRes, APV, DNx and MPEG-2 decoders and their helpers behind their features |
| 1 | `filmcraft-codecs/src/audio.rs` | the AAC and AC-3 decoders behind their features; without them the constructors answer `Unsupported` and the frame-size helpers answer `None` |
| 1 | `filmcraft-codecs/src/mp4.rs` | the DNx / ProRes probes, the APV labels and the H.264 / HEVC SPS parses behind their features (without one, the sample entry's own fields are used) |
| 1 | `filmcraft-codecs/src/mpeg.rs` | the elementary-stream decoders (H.264, HEVC, MPEG-2) behind their features; one function picks the Annex B decoder |
| 1 | `filmcraft-codecs/src/avi.rs` | the H.264 / HEVC decoders behind their features; the H.264 reorder pass behind `mxf` (its order-count helper lived in the MXF module; patch 2 moves it, so a build without `mxf` kept AVI's decode order until then) |
| 1 | `filmcraft-codecs/src/hw.rs` | the `avcC` / `hvcC` SPS parses behind `h264` / `hevc` (without them the record is refused) |
| 1 | `filmcraft-codecs/src/stream_color.rs` | the H.264 / HEVC SPS colour parses behind their features (without them the container's colour is used) |
| 1 | `filmcraft-codecs/src/mkv.rs` | the APV profile name behind `apv` |
| 1 | `filmcraft-ac3/Cargo.toml` | `[features] default = ["eac3"]` |
| 1 | `filmcraft-ac3/src/lib.rs` | `mod eac3` from `eac3.rs` with the feature, from `eac3_off.rs` without |
| 1 | `filmcraft-ac3/src/eac3_off.rs` | new: the stub module |
| 2 | `filmcraft-media/Cargo.toml` | `filmcraft-dng` optional; `[features] default = ["dng"]`, `dng`; the `tiff_ycbcr` test needs `dng` |
| 2 | `filmcraft-media/src/still.rs` | the `dng` and `ycbcr_tiff` modules and their sniffs behind `dng`; without it a DNG is an ordinary TIFF (the `image` crate reads it, preview only) |
| 2 | `filmcraft-media/src/sequence.rs` | the DNG frame decode and the CinemaDNG timing behind `dng` |
| 2 | `filmcraft-codecs/Cargo.toml` | `filmcraft-media` with `default-features = false`; feature `dng = ["filmcraft-media/dng"]`, in `default` |
| 2 | `filmcraft-codecs/src/avc_order.rs` | new: `avc_presentation_order`, moved unchanged from `mxf.rs`; built with `h264` (uses `filmcraft-h264` and `filmcraft-bitstream`, never `filmcraft-mxf`) |
| 2 | `filmcraft-codecs/src/mxf.rs` | the function removed; it uses `crate::avc_order` |
| 2 | `filmcraft-codecs/src/avi.rs` | the H.264 reorder pass now behind `h264` (was `mxf`) and calls `crate::avc_order` |
| 2 | `filmcraft-codecs/src/lib.rs` | `mod avc_order` behind `h264` |
| 3 | `filmcraft-codecs/src/lib.rs` | `builtin_factories` allows `clippy::vec_init_then_push` (its entries sit behind features, which `vec![]` cannot carry) |
| 3 | `filmcraft-codecs/src/hw.rs` | the SPS helpers' import and `read_sets` may go unused without `h264` / `hevc` |
| 3 | `filmcraft-codecs/src/mpeg.rs` | `VKind::Hevc` may go unbuilt without `hevc`; `describe_video`'s MPEG-2 values may go unused without `mpeg2` |
| 3 | `filmcraft-codecs/src/audio.rs` | `aac_to_wav_order` and `upsamples_core` may go unused without `aac` |
| 3 | `filmcraft-codecs/src/avi.rs` | patch 2's reorder condition formatted by rustfmt (the same expression) |

Each changed spot carries a `Modified (filmcraft/PATCHES.md)` comment.

## Known limits (with a feature off)
- The crates' own tests assume the default features (they encode test streams with the AAC, APV and ProRes encoders).
  Run them with the defaults. The integration tests in `tests/` do not compile from the tarballs at all, patched or not:
  they use `filmcraft-testkit`, a path dev-dependency that crates.io's packaging drops.
- The `dng` feature of `filmcraft-media` is in its `default` (upstream's behaviour); every in-tree crate that depends on
  it asks for `default-features = false` and forwards `dng` in its own `default`, so a build with `default-features =
  false` has no `filmcraft-dng`. A crate outside this tree that names `filmcraft-media` with its defaults brings DNG back.
  (In this tree only `filmcraft-codecs` depends on `filmcraft-media`.)
- With `dng` off, `filmcraft-media` reads no DNG and no YCbCr TIFF: a DNG opens as an ordinary TIFF through the `image`
  crate (preview only), and a YCbCr TIFF, which it read through `filmcraft-dng`'s TIFF parser, is refused by the `image`
  crate. The desktop media helper builds without `dng` and never plays a still (its route table knows no still format),
  so its answers do not change (by reading its `routes.rs`; not run).

## Checked (2026-11-15, the 26 crates patched in by path, `cargo test --lib` and `cargo clippy --lib`)
| Crate | Features | Unit tests | Clippy warnings |
|---|---|---|---|
| `filmcraft-codecs` | default | 125 passed, 0 failed (the unmodified tarballs: 125, 0) | 0 (the tarballs: 0) |
| `filmcraft-codecs` | `h264` only (the media helper's build) | do not compile: they name the ProRes, AAC and APV crates (35 errors); the library builds | 0 |
| `filmcraft-codecs` | none | as with `h264` only | 0 |
| `filmcraft-media` | default | 74 passed, 0 failed (the tarballs: 74, 0) | 0 (the tarballs: 0) |
| `filmcraft-media` | without `dng` | 69 passed, 0 failed (the 5 others are the `dng` and `ycbcr_tiff` modules' own) | 0 |
| `filmcraft-ac3` | default | 13 passed, 0 failed (the tarballs: 13, 0) | 0 (the tarballs: 0) |
| `filmcraft-ac3` | without `eac3` | 7 passed, 3 failed: the E-AC-3 tests, answered `Unsupported("E-AC-3 is not in this build")` by the stub | 0 |

Before patch 3, clippy counted 1 warning with the default features (`vec_init_then_push`), 7 more with `h264` only and 8
more with none, all on code that patches 1 and 2 put behind features.

## Not changed
No decoder, demuxer or parser is changed in behaviour when its feature is on: with the default features the code paths
are upstream's, line for line.

One build without a feature behaved unlike upstream until patch 2: with `h264` on and `mxf` off, patch 1 left an AVI's
H.264 pictures in decode order (the reorder pass needed the MXF module). Patch 2 builds the order-count helper with
`h264`, so such a build shows them in presentation order, as upstream does. Measured 2026-11-15 in the desktop media
helper (`h264` only, `mxf` off): the fixture `h264-mp3.avi` (x264 with B-frames) gives frames 0 to 2 at 45 dB or more
against ffmpeg's passthrough decode (below), and 22.7 dB at frame 1 with the reorder put back behind `mxf`. A reference
for an AVI with B-frames must be taken with ffmpeg's `-fps_mode passthrough`: its default constant-rate output gives the
first picture the decoder's delay as its timestamp (an AVI has none) and repeats it, so frames 1 and 2 of such a
reference are copies of frame 0.
