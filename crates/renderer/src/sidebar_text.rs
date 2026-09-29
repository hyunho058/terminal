//! The weather card's own glyph atlases (sidebar-weather-cards D2/D18).
//!
//! Card text is drawn at the design's sizes — a 17px bold title, 12.5px secondary text.
//! The grid's single atlas is rasterized at `font.size_pt × scale` and can only reach
//! those sizes by stretching a bitmap, so the sidebar owns two atlases of its own,
//! built with the existing [`Atlas::new`] at `17 × scale` and `12.5 × scale` over the
//! **same** [`FontSet`] chain the grid uses — same fonts, same real bold faces
//! (`specs/real-bold-faces`). They follow the scale factor and the font set, and never
//! `font.size_pt`: the sidebar is chrome, like the header (D2).
//!
//! This module owns only their lifecycle — build, rebuild on a scale or font-set
//! change, never retry a failed build every frame — and exposes what a text pipeline
//! needs around them: [`epoch`](CardAtlases::epoch) to key its bind groups on (a
//! rebuilt atlas is a new texture) and [`generation`](CardAtlases::generation) to
//! notice an eviction that happened mid-frame (D18).

use std::sync::Arc;

use crate::atlas::Atlas;
use crate::font_set::FontSet;
use crate::sidebar_theme::CardMetrics;

/// Which of the two card atlases a string is drawn from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardFace {
    /// The 17px bold title (and the rename field that replaces it, D15).
    Title,
    /// The 12.5px folder, status and branch text.
    Meta,
}

impl CardFace {
    pub const ALL: [CardFace; 2] = [CardFace::Title, CardFace::Meta];

    /// The atlas's ppem at `scale`: D2's design size in physical px, through the one
    /// logical → physical conversion ([`CardMetrics::scaled`], D1).
    pub fn ppem(self, scale: f64) -> f32 {
        let m = CardMetrics::DEFAULT.scaled(scale);
        match self {
            CardFace::Title => m.title_px,
            CardFace::Meta => m.meta_px,
        }
    }

    /// One display column's width in physical px at `scale` (D3/R3.3).
    pub fn column_px(self, scale: f64) -> f32 {
        column_px(self.ppem(scale))
    }

    /// One text line's height in physical px at `scale`.
    pub fn line_px(self, scale: f64) -> f32 {
        line_px(self.ppem(scale))
    }

    fn index(self) -> usize {
        match self {
            CardFace::Title => 0,
            CardFace::Meta => 1,
        }
    }
}

/// A display column's width for text rasterized at `ppem`: the grid's own
/// `max(1, round(0.6 × px))` (`cell_size_px`), so a card lays out monospace text by
/// exactly the rule the terminal does (D3).
pub fn column_px(ppem: f32) -> f32 {
    (ppem * 0.6).round().max(1.0)
}

/// A line's height for text rasterized at `ppem`: the grid's `max(1, round(1.2 × px))`.
pub fn line_px(ppem: f32) -> f32 {
    (ppem * 1.2).round().max(1.0)
}

/// The identity of a font set: each family's regular face, by the address of its
/// mapping and its face index. A clone shares its `Arc`s and so has the same identity —
/// the same fonts, so no rebuild — while a fresh [`FontSet::load`] maps anew.
fn font_identity(fonts: &FontSet) -> impl Iterator<Item = (usize, usize)> + '_ {
    fonts.families().iter().map(|family| {
        let face = family.regular();
        (Arc::as_ptr(&face.data) as usize, face.index)
    })
}

/// The two card atlases and what they were built for.
#[derive(Default)]
pub struct CardAtlases {
    atlases: [Option<Atlas>; 2],
    /// The scale the current atlases were built — or last *attempted* — at. `None`
    /// until the first [`ensure`](CardAtlases::ensure).
    built_scale: Option<f64>,
    /// The font identity they were built — or last attempted — from.
    built_fonts: Vec<(usize, usize)>,
    /// How many times [`ensure`](CardAtlases::ensure) (re)built — a test hook.
    builds: u64,
}

impl CardAtlases {
    pub fn new() -> CardAtlases {
        CardAtlases::default()
    }

    /// Make the atlases current for `fonts` at `scale`, rebuilding both when either
    /// changed. Returns `true` when it (re)built.
    ///
    /// A build that fails — `Atlas::new` answering `None` — is **remembered as
    /// attempted**: it is not retried on the next frame, only when the scale or the
    /// font set changes again. That is what keeps its `tracing::warn!` to one per
    /// failed build rather than one per frame, and keeps a failing GPU allocation off
    /// the 60 fps path (R2.6). The caller then draws that face's text from the grid
    /// atlas instead.
    pub fn ensure(&mut self, device: &wgpu::Device, fonts: &FontSet, scale: f64) -> bool {
        let same_fonts = font_identity(fonts).eq(self.built_fonts.iter().copied());
        if self.built_scale == Some(scale) && same_fonts {
            return false;
        }
        for face in CardFace::ALL {
            let ppem = face.ppem(scale);
            let atlas = Atlas::new(device, fonts, ppem);
            if atlas.is_none() {
                tracing::warn!(
                    ?face,
                    ppem,
                    "sidebar card atlas could not be built; drawing that text from the \
                     grid atlas at cell size instead (sidebar-weather-cards R2.6)"
                );
            }
            self.atlases[face.index()] = atlas;
        }
        self.built_scale = Some(scale);
        self.built_fonts.clear();
        self.built_fonts.extend(font_identity(fonts));
        self.builds += 1;
        true
    }

    /// The atlas for `face`, or `None` before the first build or after a failed one.
    pub fn get(&self, face: CardFace) -> Option<&Atlas> {
        self.atlases[face.index()].as_ref()
    }

    /// Mutable access, for packing glyphs.
    pub fn get_mut(&mut self, face: CardFace) -> Option<&mut Atlas> {
        self.atlases[face.index()].as_mut()
    }

    /// The texture identity of `face`'s atlas — what a bind group must be keyed on.
    /// A rebuilt atlas is a new texture with a new epoch (D18).
    pub fn epoch(&self, face: CardFace) -> Option<u64> {
        self.get(face).map(Atlas::epoch)
    }

    /// `face`'s eviction counter. Compared before and after a frame's glyph packing,
    /// a change means UVs already resolved that frame point into a repacked texture
    /// (D18/R2.5).
    pub fn generation(&self, face: CardFace) -> Option<u64> {
        self.get(face).map(Atlas::generation)
    }

    /// How many (re)builds have happened — a test hook.
    pub fn builds(&self) -> u64 {
        self.builds
    }
}

/// A font set with no usable face: four bytes that are not a font, so `Atlas::new`
/// answers `None` for it at any size (R2.6). Hoisted out of `tests` so the row layer's
/// tests can drive a failed card atlas through `TabBar::record`.
#[cfg(test)]
pub(crate) fn broken_fonts() -> FontSet {
    let bytes = [0u8, 1, 2, 3];
    let mut map = memmap2::MmapOptions::new()
        .len(bytes.len())
        .map_anon()
        .expect("anon mapping");
    map.copy_from_slice(&bytes);
    let face = crate::font_set::FontFace {
        data: Arc::new(map.make_read_only().expect("freeze")),
        index: 0,
    };
    FontSet::from_families(vec![crate::font_set::FontFamily::from_parts(
        face, None, None, None,
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fonts() -> Option<FontSet> {
        FontSet::load().ok()
    }

    #[test]
    fn faces_are_the_design_sizes() {
        // D2 — 17 and 12.5 logical px, scaled, and nothing else.
        assert_eq!(CardFace::Title.ppem(2.0), 34.0);
        assert_eq!(CardFace::Meta.ppem(2.0), 25.0);
        assert_eq!(CardFace::Title.ppem(1.0), 17.0);
        assert_eq!(CardFace::Meta.ppem(1.0), 12.5);
    }

    #[test]
    fn column_width_is_the_grids_rule() {
        // R3.3 — `max(1, round(0.6 × ppem))`, the same rule `cell_size_px` applies to
        // the grid; checked against that function directly so the two cannot drift.
        for ppem in [12.5_f32, 17.0, 25.0, 34.0, 0.5] {
            let mut cfg = common::config::Config::default();
            cfg.font.size_pt = ppem;
            let (cw, ch) = crate::cell_size_px(&cfg, 1.0);
            assert_eq!(column_px(ppem), cw, "column at {ppem}");
            assert_eq!(line_px(ppem), ch, "line at {ppem}");
        }
        assert_eq!(CardFace::Title.column_px(2.0), 20.0); // round(20.4)
        assert_eq!(CardFace::Meta.column_px(2.0), 15.0);
        assert_eq!(CardFace::Title.line_px(2.0), 41.0); // round(40.8)
    }

    #[test]
    fn the_atlases_are_built_at_the_design_sizes() {
        // R2.1 — the title atlas rasterizes at 17×scale and the meta atlas at
        // 12.5×scale: their glyphs match a plain `Atlas::new` at those ppems exactly,
        // with no input from the terminal's `font.size_pt` anywhere in the path.
        let Some((device, queue)) = crate::headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let Some(fonts) = fonts() else {
            eprintln!("skipping: no system font");
            return;
        };
        let mut cards = CardAtlases::new();
        assert!(
            cards.ensure(&device, &fonts, 2.0),
            "the first ensure builds"
        );
        for (face, ch, bold) in [(CardFace::Title, 'A', true), (CardFace::Meta, 'a', false)] {
            let mut reference = Atlas::new(&device, &fonts, face.ppem(2.0)).expect("atlas");
            let want = reference
                .resolve(ch, bold, false)
                .and_then(|k| reference.get_or_insert_styled(&queue, k))
                .expect("reference glyph");
            let atlas = cards.get_mut(face).expect("built");
            let got = atlas
                .resolve(ch, bold, false)
                .and_then(|k| atlas.get_or_insert_styled(&queue, k))
                .expect("card glyph");
            assert_eq!((got.px_w, got.px_h), (want.px_w, want.px_h), "{face:?}");
            assert_eq!((got.left, got.top), (want.left, want.top), "{face:?}");
        }
        // And the title glyph is bigger than the meta one.
        let t = cards.get_mut(CardFace::Title).expect("title");
        let tk = t.resolve('A', true, false).expect("A");
        let th = t.get_or_insert_styled(&queue, tk).expect("A").px_h;
        let m = cards.get_mut(CardFace::Meta).expect("meta");
        let mk = m.resolve('A', true, false).expect("A");
        let mh = m.get_or_insert_styled(&queue, mk).expect("A").px_h;
        assert!(
            th > mh,
            "17px title glyph ({th}) is taller than 12.5px ({mh})"
        );
    }

    #[test]
    fn an_unchanged_scale_and_font_set_does_not_rebuild() {
        let Some((device, _queue)) = crate::headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let Some(fonts) = fonts() else {
            eprintln!("skipping: no system font");
            return;
        };
        let mut cards = CardAtlases::new();
        cards.ensure(&device, &fonts, 2.0);
        let epochs = CardFace::ALL.map(|f| cards.epoch(f));
        assert!(
            !cards.ensure(&device, &fonts, 2.0),
            "same inputs → no rebuild"
        );
        // A clone shares the mappings — the same fonts — so it is not a change either.
        assert!(
            !cards.ensure(&device, &fonts.clone(), 2.0),
            "a clone is the same set"
        );
        assert_eq!(cards.builds(), 1);
        assert_eq!(CardFace::ALL.map(|f| cards.epoch(f)), epochs);
    }

    #[test]
    fn a_scale_change_rebuilds_both_at_the_new_ppem() {
        // R2.3 — new textures (new epochs) at the new size.
        let Some((device, queue)) = crate::headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let Some(fonts) = fonts() else {
            eprintln!("skipping: no system font");
            return;
        };
        let mut cards = CardAtlases::new();
        cards.ensure(&device, &fonts, 2.0);
        let before = CardFace::ALL.map(|f| cards.epoch(f).expect("built"));
        let h2 = {
            let a = cards.get_mut(CardFace::Title).expect("title");
            let k = a.resolve('H', true, false).expect("H");
            a.get_or_insert_styled(&queue, k).expect("H").px_h
        };
        assert!(cards.ensure(&device, &fonts, 1.0), "a new scale rebuilds");
        let after = CardFace::ALL.map(|f| cards.epoch(f).expect("rebuilt"));
        for i in 0..2 {
            assert_ne!(before[i], after[i], "face {i} is a new texture");
        }
        let h1 = {
            let a = cards.get_mut(CardFace::Title).expect("title");
            let k = a.resolve('H', true, false).expect("H");
            a.get_or_insert_styled(&queue, k).expect("H").px_h
        };
        assert!(h1 < h2, "at 1× the glyph is smaller ({h1} vs {h2})");
    }

    #[test]
    fn a_new_font_set_rebuilds() {
        // R2.4 — a font set loaded anew (as a family change would) is a different
        // identity, so both atlases rebuild from it.
        let Some((device, _queue)) = crate::headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let (Some(a), Some(b)) = (fonts(), fonts()) else {
            eprintln!("skipping: no system font");
            return;
        };
        let mut cards = CardAtlases::new();
        cards.ensure(&device, &a, 2.0);
        let before = cards.epoch(CardFace::Title);
        assert!(cards.ensure(&device, &b, 2.0), "a reloaded set is a change");
        assert_ne!(cards.epoch(CardFace::Title), before);
        assert_eq!(cards.builds(), 2);
    }

    #[test]
    fn an_eviction_moves_the_generation_but_not_the_epoch() {
        // D18/R2.5 — the two counters answer different questions: a repack of the
        // same texture moves `generation` (UVs resolved earlier this frame are stale)
        // and leaves `epoch` (the bind group is still valid).
        let Some((device, _queue)) = crate::headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let Some(fonts) = fonts() else {
            eprintln!("skipping: no system font");
            return;
        };
        let mut cards = CardAtlases::new();
        cards.ensure(&device, &fonts, 2.0);
        let (g, e, title) = (
            cards.generation(CardFace::Meta),
            cards.epoch(CardFace::Meta),
            (
                cards.generation(CardFace::Title),
                cards.epoch(CardFace::Title),
            ),
        );
        cards
            .get_mut(CardFace::Meta)
            .expect("meta")
            .evict_for_test();
        assert_ne!(
            cards.generation(CardFace::Meta),
            g,
            "eviction → new generation"
        );
        assert_eq!(cards.epoch(CardFace::Meta), e, "same texture → same epoch");
        assert_eq!(
            (
                cards.generation(CardFace::Title),
                cards.epoch(CardFace::Title)
            ),
            title,
            "the other face is untouched"
        );
    }

    #[test]
    fn a_failed_build_is_not_retried_every_frame() {
        // R2.6 — `Atlas::new` answers `None` for a set with no usable regular face.
        // The failure is remembered: `get` is `None`, and the next frames with the same
        // inputs do not attempt (or warn about) the build again.
        let Some((device, _queue)) = crate::headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let fonts = broken_fonts();
        let mut cards = CardAtlases::new();
        assert!(cards.ensure(&device, &fonts, 2.0), "attempted once");
        assert!(cards.get(CardFace::Title).is_none());
        assert!(cards.get(CardFace::Meta).is_none());
        assert_eq!(cards.epoch(CardFace::Title), None);
        for _ in 0..3 {
            assert!(!cards.ensure(&device, &fonts, 2.0), "not retried per frame");
        }
        assert_eq!(cards.builds(), 1);
        // A real change (here, the scale) is worth another attempt.
        assert!(cards.ensure(&device, &fonts, 1.0));
        assert_eq!(cards.builds(), 2);
    }
}
