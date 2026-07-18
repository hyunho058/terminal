//! Glyph rasterization via `swash` (D6).
//!
//! Produces alpha-coverage bitmaps ready to pack into a GPU glyph atlas. The
//! atlas + textured shader is the first *render* feature (see
//! `docs/architecture.md`); this module proves the swash path compiles and
//! rasterizes, and gives that feature a concrete starting point. Ligature
//! shaping (config `font.ligatures`) plugs in here via `swash::shape`.

use swash::scale::{Render, ScaleContext, Source};
use swash::FontRef;

/// A rasterized glyph: alpha coverage plus placement offsets, in pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphBitmap {
    pub width: u32,
    pub height: u32,
    /// Left/top bearing relative to the pen origin.
    pub left: i32,
    pub top: i32,
    /// `width * height` alpha coverage, row-major, `0..=255`.
    pub coverage: Vec<u8>,
}

/// Owns font bytes and a scale context, rasterizing chars on demand.
///
/// The `FontRef` borrows the byte buffer, so we store the bytes and rebuild the
/// `FontRef` per call rather than self-referencing.
pub struct GlyphRasterizer {
    data: Vec<u8>,
    index: usize,
    ppem: f32,
    context: ScaleContext,
}

impl GlyphRasterizer {
    /// Build a rasterizer from font-collection bytes. Returns `None` if the
    /// bytes are not a valid font at `index`.
    pub fn new(data: Vec<u8>, index: usize, ppem: f32) -> Option<Self> {
        FontRef::from_index(&data, index)?;
        Some(GlyphRasterizer {
            data,
            index,
            ppem,
            context: ScaleContext::new(),
        })
    }

    /// Map a character to its glyph id (0 == `.notdef`).
    pub fn glyph_id(&self, ch: char) -> u16 {
        FontRef::from_index(&self.data, self.index)
            .unwrap()
            .charmap()
            .map(ch)
    }

    /// Rasterize a single character. Returns `None` for `.notdef` or an empty
    /// (e.g. whitespace) outline.
    pub fn rasterize(&mut self, ch: char) -> Option<GlyphBitmap> {
        let glyph_id = self.glyph_id(ch);
        if glyph_id == 0 {
            return None;
        }
        self.rasterize_glyph(glyph_id)
    }

    /// Rasterize a specific glyph id (post-shaping). Returns `None` for an empty
    /// outline (e.g. a space glyph).
    pub fn rasterize_glyph(&mut self, glyph_id: u16) -> Option<GlyphBitmap> {
        // Safe to unwrap: validated in `new`.
        let font = FontRef::from_index(&self.data, self.index).unwrap();
        let mut scaler = self
            .context
            .builder(font)
            .size(self.ppem)
            .hint(true)
            .build();
        let image = Render::new(&[Source::Outline]).render(&mut scaler, glyph_id)?;
        if image.placement.width == 0 || image.placement.height == 0 {
            return None;
        }
        Some(GlyphBitmap {
            width: image.placement.width,
            height: image.placement.height,
            left: image.placement.left,
            top: image.placement.top,
            coverage: image.data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_font_bytes() {
        assert!(GlyphRasterizer::new(vec![0, 1, 2, 3], 0, 14.0).is_none());
    }

    #[test]
    #[ignore = "depends on a system font path; run with --ignored"]
    fn rasterizes_a_letter() {
        let bytes = std::fs::read("/System/Library/Fonts/Menlo.ttc")
            .or_else(|_| std::fs::read("/System/Library/Fonts/SFNSMono.ttf"))
            .expect("a monospace system font");
        let mut r = GlyphRasterizer::new(bytes, 0, 32.0).expect("valid font");
        let g = r.rasterize('A').expect("glyph A");
        assert!(g.width > 0 && g.height > 0);
        assert_eq!(g.coverage.len(), (g.width * g.height) as usize);
    }
}
