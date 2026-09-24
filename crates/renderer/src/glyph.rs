//! Glyph rasterization via `swash` (D6).
//!
//! Produces alpha-coverage bitmaps ready to pack into a GPU glyph atlas. The
//! atlas + textured shader is the first *render* feature (see
//! `docs/architecture.md`); this module proves the swash path compiles and
//! rasterizes, and gives that feature a concrete starting point. Ligature
//! shaping (config `font.ligatures`) plugs in here via `swash::shape`.

use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::{Angle, Transform};
use swash::FontRef;

use crate::font_set::FontData;

/// Synthetic-italic slant applied via a shear transform when no italic face is
/// available (about 14°, the common oblique angle).
///
/// Still the normal path for Hangul: all 18 AppleSDGothicNeo faces are
/// `style=Normal`, so there is no italic face to prefer over this.
const ITALIC_SKEW_DEGREES: f32 = 14.0;

/// Synthetic-bold strength as a fraction of the em size, applied to the outline
/// before rasterization.
///
/// **This is a fallback, not the bold path (D1/D9).** When the font file ships a
/// real bold face — Menlo at face 1, AppleSDGothicNeo at face 6 — the atlas
/// rasterizes from *that face* and never reaches this constant. It is used only
/// where a file has no bold face at all, which on this machine means the
/// second-choice fallbacks (`AppleGothic.ttf`, `SFNSMono.ttf`; see G-A).
///
/// **Do not re-tune this value without citing D18's differentiability floor**
/// (R7.3). It is not a knob for "bold looks too light": emboldening this font is
/// what *erodes* 593 of 11,172 Hangul syllables instead of thickening them —
/// 안/간/건/난 lose 86–89% of their ink — because the contour winding makes the
/// offset outlines cancel. Raising it deepens that hole rather than filling it.
const BOLD_EMBOLDEN_FRAC: f32 = 0.04;

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

/// Shares one face's mapped font bytes and owns a scale context, rasterizing
/// glyphs of that face on demand.
///
/// The `FontRef` borrows the byte buffer, so we hold the [`FontData`] handle and
/// rebuild the `FontRef` per call rather than self-referencing. The handle is a
/// refcount over an mmap, never a copy: the atlas builds one rasterizer *per
/// face* (D7), and the Hangul face measures 55 MB — copying it per face, on every
/// `Atlas::new` (which `apply_config` re-runs whenever the config watcher changes
/// ppem), is the deep copy D7/R3.2 exists to delete.
pub struct GlyphRasterizer {
    data: FontData,
    index: usize,
    ppem: f32,
    context: ScaleContext,
}

impl GlyphRasterizer {
    /// Build a rasterizer over a face's mapped bytes. Returns `None` if the
    /// bytes are not a valid font at `index`.
    pub fn new(data: FontData, index: usize, ppem: f32) -> Option<Self> {
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

    /// Rasterize a specific glyph id (post-shaping) with **no synthesis**.
    /// Returns `None` for an empty outline (e.g. a space glyph).
    ///
    /// This is now the path a *real* styled face takes: the atlas points a
    /// rasterizer at Menlo Bold or Gothic Bold and asks for the outline as drawn.
    pub fn rasterize_glyph(&mut self, glyph_id: u16) -> Option<GlyphBitmap> {
        self.rasterize_glyph_synth(glyph_id, false, false)
    }

    /// Rasterize a glyph id, applying the synthesis the **caller** asks for.
    ///
    /// The flags say *what to fake*, not *what style the cell is* — that is the
    /// whole change (D9). Previously this method inferred "bold ⇒ embolden", so
    /// there was no way to express "this cell is bold and the weight is already
    /// real". Now the family decides (`FontFamily::resolve_style`) and passes down
    /// only the leftovers, so bold-italic Hangul arrives here as
    /// `(synth_bold: false, synth_italic: true)` — real w700 outlines, sheared.
    ///
    /// `glyph_id` must be an id in **this rasterizer's own face**; ids are a
    /// per-face namespace and do not survive a face change (D15).
    pub fn rasterize_glyph_synth(
        &mut self,
        glyph_id: u16,
        synth_bold: bool,
        synth_italic: bool,
    ) -> Option<GlyphBitmap> {
        // Safe to unwrap: validated in `new`.
        let font = FontRef::from_index(&self.data, self.index).unwrap();
        let mut scaler = self
            .context
            .builder(font)
            .size(self.ppem)
            .hint(true)
            .build();
        let mut render = Render::new(&[Source::Outline]);
        if synth_bold {
            render.embolden(self.ppem * BOLD_EMBOLDEN_FRAC);
        }
        if synth_italic {
            render.transform(Some(Transform::skew(
                Angle::from_degrees(ITALIC_SKEW_DEGREES),
                Angle::from_degrees(0.0),
            )));
        }
        let image = render.render(&mut scaler, glyph_id)?;
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

    use std::sync::Arc;

    use crate::font_set::FontSet;

    /// A [`FontData`] handle over arbitrary bytes, with no file involved: an
    /// anonymous mapping, written once and frozen. Lets the invalid-bytes case be
    /// tested now that the constructor takes a mapping rather than a `Vec`.
    fn mapped(bytes: &[u8]) -> FontData {
        let mut map = memmap2::MmapOptions::new()
            .len(bytes.len())
            .map_anon()
            .expect("an anonymous mapping");
        map.copy_from_slice(bytes);
        Arc::new(map.make_read_only().expect("freeze the mapping"))
    }

    #[test]
    fn rejects_invalid_font_bytes() {
        assert!(GlyphRasterizer::new(mapped(&[0, 1, 2, 3]), 0, 14.0).is_none());
    }

    #[test]
    #[ignore = "depends on a system font path; run with --ignored"]
    fn rasterizes_a_letter() {
        // Goes through `FontSet` so the primary face's bytes are shared, not read
        // into a `Vec` — the same zero-copy path production uses (D7/R3.2).
        let set = FontSet::load().expect("a monospace system font");
        let face = set
            .families()
            .first()
            .map(|f| f.regular().clone())
            .expect("a primary face");
        let mut r = GlyphRasterizer::new(face.data, face.index, 32.0).expect("valid font");
        let g = r.rasterize('A').expect("glyph A");
        assert!(g.width > 0 && g.height > 0);
        assert_eq!(g.coverage.len(), (g.width * g.height) as usize);
    }

    /// Coverage-weighted ink: the quantity "bold" is supposed to increase.
    fn ink(g: &GlyphBitmap) -> f64 {
        g.coverage.iter().map(|&a| a as f64 / 255.0).sum()
    }

    fn rasterizer_for(
        slot: crate::font_set::StyleSlot,
        family: usize,
        ppem: f32,
    ) -> Option<GlyphRasterizer> {
        let set = FontSet::load().ok()?;
        let face = set.families().get(family)?.slot(slot)?.clone();
        GlyphRasterizer::new(face.data, face.index, ppem)
    }

    #[test]
    #[ignore = "depends on a system font path; run with --ignored"]
    fn no_synthesis_is_the_plain_outline() {
        // The flags mean "fake this", so all-false must be byte-identical to the
        // unstyled call — otherwise a real styled face would silently get extra
        // weight applied on top of the weight it already has.
        let mut r =
            rasterizer_for(crate::font_set::StyleSlot::Regular, 0, 32.0).expect("a primary face");
        let gid = r.glyph_id('A');
        assert_eq!(
            r.rasterize_glyph(gid),
            r.rasterize_glyph_synth(gid, false, false)
        );
    }

    #[test]
    #[ignore = "depends on a system font path; run with --ignored"]
    fn the_real_bold_face_thickens_hangul_where_embolden_erodes_it() {
        // This is the defect, at the rasterizer level (R0.2/D1). `글` is one of
        // 593 syllables whose outline swash's `embolden` makes *thinner*, because
        // offsetting the contours makes the fill cancel rather than grow.
        use crate::font_set::StyleSlot;
        let (Some(mut regular), Some(mut bold)) = (
            rasterizer_for(StyleSlot::Regular, 1, 28.0),
            rasterizer_for(StyleSlot::Bold, 1, 28.0),
        ) else {
            eprintln!("skipping: no Hangul fallback family with a bold face");
            return;
        };

        let plain = regular.rasterize('글').expect("regular 글");
        let gid_reg = regular.glyph_id('글');
        let synthetic = regular
            .rasterize_glyph_synth(gid_reg, true, false)
            .expect("emboldened 글");
        let gid_bold = bold.glyph_id('글');
        let real = bold.rasterize_glyph(gid_bold).expect("real bold 글");

        assert!(
            ink(&synthetic) < ink(&plain),
            "the bug: embolden ERODES this glyph ({:.1} -> {:.1})",
            ink(&plain),
            ink(&synthetic)
        );
        assert!(
            ink(&real) > ink(&plain),
            "the fix: the real w700 face thickens it ({:.1} -> {:.1})",
            ink(&plain),
            ink(&real)
        );
    }
}
