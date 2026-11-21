// Modified for the Beebeeb desktop app's media helper, 2026-11-15 (filmcraft/PATCHES.md): a file of our own, built in
// place of `eac3.rs` when the crate's `eac3` feature is off.
//! E-AC-3 left out of this build: an E-AC-3 syncframe (bsid 11-16) is `Unsupported`, never decoded. The AC-3 decoder's
//! E-AC-3 hooks keep their types; a [`FrameInfo`] is never made, so they are never taken.

#![allow(dead_code)]

use crate::{Bits, Error, Header, Result};

fn left_out() -> Error {
    Error::Unsupported("E-AC-3 is not in this build".into())
}

pub(crate) fn parse_header(_b: &[u8], _bsid: u8) -> Result<Header> {
    Err(left_out())
}

pub(crate) struct Bsi {
    pub strmtyp: u8,
    pub fscod: usize,
    pub numblkscod: usize,
}

pub(crate) fn read_bsi(_r: &mut Bits) -> Result<Bsi> {
    Err(left_out())
}

pub(crate) struct FrameInfo {
    pub strmtyp: u8,
    pub blkswe: bool,
    pub dithflage: bool,
    pub bamode: bool,
    pub frmfgaincode: bool,
    pub dbaflde: bool,
    pub skipflde: bool,
    pub snroffststr: u8,
    pub frmcsnroffst: i32,
    pub frmfsnroffst: i32,
    pub cplstre: [bool; 6],
    pub cplinu: [bool; 6],
    pub cplexpstr: [u8; 6],
    pub chexpstr: [[u8; 5]; 6],
    pub lfeexpstr: [u8; 6],
    pub spxatten: [Option<usize>; 5],
    pub firstcplcos: [bool; 5],
    pub firstcplleak: bool,
    pub cplbndstrc: [bool; 18],
    pub spx: Spx,
}

impl FrameInfo {
    pub(crate) fn read(
        _r: &mut Bits,
        _bsi: &Bsi,
        _acmod: usize,
        _lfeon: bool,
        _nblocks: usize,
        _words: usize,
    ) -> Result<FrameInfo> {
        Err(left_out())
    }
}

pub(crate) struct Spx {
    pub inu: bool,
    pub chinspx: [bool; 5],
    pub begf: usize,
}

impl Spx {
    pub fn begin_bin(&self) -> usize {
        0
    }

    pub(crate) fn read(
        &mut self,
        _r: &mut Bits,
        _blk: usize,
        _acmod: usize,
        _nf: usize,
    ) -> Result<()> {
        Err(left_out())
    }

    pub(crate) fn synthesize(
        &self,
        _ch: usize,
        _atten: Option<usize>,
        _tc: &mut [f32; 256],
        _noise: impl FnMut() -> f32,
    ) {
    }
}
