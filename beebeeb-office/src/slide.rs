//! Presentations (`.pptx`): the final API, not built yet. [`Deck::open`] says
//! [`OpenError::Unsupported`] for every file (`.ppt` stays unsupported for good: there is no
//! reader for it), so no [`Deck`] can exist and the methods below are never reached. The slides
//! slice fills this file in; the types may grow fields there.

use std::convert::Infallible;

use crate::{Limits, OpenError, RenderError, RenderInfo};

/// The slide count and every slide's size, in points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeckInfo {
    pub slides: u32,
    /// (width, height) of each slide, points; one per slide.
    pub sizes_pt: Vec<(f32, f32)>,
    /// The title of each slide (empty when it has none); one per slide.
    pub titles: Vec<String>,
}

/// An open presentation. None can exist yet ([`Deck::open`]).
pub struct Deck {
    never: Infallible,
}

impl Deck {
    /// Open a `.pptx` from its bytes. Not built yet: [`OpenError::Unsupported`] for every file.
    pub fn open(_bytes: &[u8], _limits: &Limits) -> Result<Deck, OpenError> {
        Err(OpenError::Unsupported)
    }

    /// The slide count and sizes.
    pub fn info(&self) -> DeckInfo {
        match self.never {}
    }

    /// The size in pixels of what [`Deck::render_slide`] writes for slide `n` at `scale`: the
    /// caller sizes its buffer from this.
    pub fn render_size(&self, _n: u32, _scale: f32) -> Result<(u32, u32), RenderError> {
        match self.never {}
    }

    /// Render slide `n` (0-based) at `scale` pixels per point into `out` (RGBA8, exactly
    /// `w * h * 4` bytes).
    pub fn render_slide(&self, _n: u32, _scale: f32, _out: &mut [u8]) -> Result<RenderInfo, RenderError> {
        match self.never {}
    }
}
