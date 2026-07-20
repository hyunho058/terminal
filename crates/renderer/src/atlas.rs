//! Dynamic glyph atlas (D1): a single R8Unorm coverage texture filled on demand.
//!
//! Rasterized glyphs (via `swash`, see [`crate::glyph`]) are shelf-packed into one
//! GPU texture; each `glyph_id` at the atlas's fixed pixel size caches its UV rect
//! plus placement metrics (D6). The atlas is bounded (D1/R1.3): when a shelf run
//! fills the texture it evicts everything and repacks, rather than growing
//! unboundedly or panicking.
//!
//! The atlas spans the whole [`FontSet`] fallback chain (D7): it holds one
//! [`GlyphRasterizer`] per face, and [`GlyphKey`] carries the face index. That axis
//! is a *correctness* requirement, not a cache refinement — glyph ids are per-font
//! namespaces, so without it Menlo's gid 42 and the Hangul face's gid 42 share one
//! cache slot and the wrong glyph is drawn (R3.1).
//!
//! [`Atlas::resolve`] is the single character→glyph entry point for every draw
//! path (grid, tab bar, overlay, preedit — D5/D10), so fallback and the
//! degraded-mode rules are decided in exactly one place.

use std::collections::HashMap;

use crate::font_set::FontSet;
use crate::glyph::{GlyphBitmap, GlyphRasterizer};

/// Cache key for a rasterized glyph: the face it came from, the glyph id within
/// that face, and its synthesized style — so a bold/italic variant caches
/// separately from the regular one (D10) and two faces never collide (D7/R3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    /// Index into the [`FontSet`] chain the atlas was built from; 0 == primary.
    pub face: u16,
    pub id: u16,
    pub bold: bool,
    pub italic: bool,
}

impl GlyphKey {
    /// The regular (unstyled) key for a glyph id in the **primary** face (D7).
    pub fn plain(id: u16) -> Self {
        GlyphKey {
            face: 0,
            id,
            bold: false,
            italic: false,
        }
    }
}

/// A packed glyph: its atlas UV rect plus pixel size and pen bearings (D6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphEntry {
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
    /// Bitmap size in pixels.
    pub px_w: f32,
    pub px_h: f32,
    /// Left / top bearing relative to the pen origin (baseline).
    pub left: f32,
    pub top: f32,
}

/// Atlas edge in pixels — 2048² R8Unorm, i.e. 4 MB (D13/R3.3).
///
/// Sized up from 1024² for Hangul: it has two orders of magnitude more distinct
/// glyphs than Latin, which raises the chance that a shelf run fills mid-frame and
/// [`Atlas::pack`] evicts — invalidating the UVs of instances already emitted in
/// that same frame. 3 MB buys the probability back out of practical range. (The
/// underlying intra-frame invalidation is a Known Gap, not fixed here.)
const ATLAS_SIZE: u32 = 2048;
const PAD: u32 = 1;

/// The glyph atlas: a GPU texture + a shelf packer + a per-glyph cache.
pub struct Atlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    /// One rasterizer per [`FontSet`] face, in chain order; index 0 is the
    /// primary. [`GlyphKey::face`] indexes this vector (D7).
    rasterizers: Vec<GlyphRasterizer>,
    cache: HashMap<GlyphKey, Option<GlyphEntry>>,
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    /// Bumped whenever the atlas is cleared (bounded eviction).
    generation: u64,
    /// A packed 2×2 fully-opaque coverage block + the generation it was packed in,
    /// reused for underline rules (a solid fill in the glyph pass). Re-reserved
    /// lazily after an eviction bumps the generation.
    solid: Option<(GlyphEntry, u64)>,
}

impl Atlas {
    /// Build an atlas covering every face of `fonts` at `ppem` (D7).
    ///
    /// Each face contributes one [`GlyphRasterizer`] that *shares* the face's
    /// mapped bytes — the `FontData` handle is cloned, never the 55 MB behind it
    /// (D7/R3.2). `None` if any face fails to parse at its recorded index.
    pub fn new(device: &wgpu::Device, fonts: &FontSet, ppem: f32) -> Option<Atlas> {
        let rasterizers = fonts
            .faces()
            .iter()
            .map(|face| GlyphRasterizer::new(face.data.clone(), face.index, ppem))
            .collect::<Option<Vec<_>>>()?;
        if rasterizers.is_empty() {
            return None;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph-atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Some(Atlas {
            texture,
            view,
            sampler,
            rasterizers,
            cache: HashMap::new(),
            shelf_x: 0,
            shelf_y: 0,
            shelf_h: 0,
            generation: 0,
            solid: None,
        })
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
    pub fn sampler(&self) -> &wgpu::Sampler {
        &self.sampler
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// True when the chain is primary-only, i.e. no fallback face loaded (D4).
    ///
    /// The judgement lives here rather than on [`FontSet`] on purpose (D5): a
    /// sibling `DrawLayer` only ever holds `ctx.atlas`, so a `FontSet`-side
    /// predicate would be unreachable from the tab-bar/overlay text paths and the
    /// same silent failure would grow back there.
    pub fn is_degraded(&self) -> bool {
        self.rasterizers.len() == 1
    }

    /// Map a character to the face + glyph that should draw it — the single
    /// fallback entry point for every draw path (D5/D10).
    ///
    /// Walks the chain in order and takes the first face whose charmap covers
    /// `ch`, so anything the primary covers resolves to face 0 and renders exactly
    /// as it did before the chain existed (R0.3, a hard constraint).
    ///
    /// Uncovered characters split by mode (R2.4/R2.5):
    ///
    /// * **normal** — `None`. The caller emits no instance, so a character no face
    ///   can draw (an emoji, say) leaves a blank cell rather than a wall of tofu.
    /// * **degraded** ([`is_degraded`](Atlas::is_degraded)) — `Some(face 0, id 0)`,
    ///   i.e. the primary's `.notdef`. If Apple ever relocates *both* Hangul font
    ///   paths, "draw nothing" would make Hangul completely **invisible**, signalled
    ///   only by a `warn!` line the user never sees — strictly worse than today's
    ///   tofu boxes. Degraded mode keeps the breakage visible on purpose.
    pub fn resolve(&self, ch: char) -> Option<GlyphKey> {
        for (face, rasterizer) in self.rasterizers.iter().enumerate() {
            let id = rasterizer.glyph_id(ch);
            if id != 0 {
                return Some(GlyphKey {
                    face: face as u16,
                    id,
                    bold: false,
                    italic: false,
                });
            }
        }
        // D5: keep `.notdef` visible when there is no fallback face to consult.
        self.is_degraded().then(|| GlyphKey::plain(0))
    }

    /// Map a character to its glyph id in the **primary** face (0 == `.notdef`).
    /// Face-blind entry point kept for callers that predate the chain; new code
    /// should use [`resolve`](Atlas::resolve) so fallback applies (D10).
    pub fn glyph_id(&self, ch: char) -> u16 {
        self.rasterizers
            .first()
            .map_or(0, |rasterizer| rasterizer.glyph_id(ch))
    }

    /// Get a glyph's atlas entry in the regular style. Backward-compatible entry
    /// point kept for callers that don't carry style (e.g. sibling `DrawLayer`s).
    /// `None` = the glyph has no visible outline (e.g. a space).
    pub fn get_or_insert(&mut self, queue: &wgpu::Queue, glyph_id: u16) -> Option<GlyphEntry> {
        self.get_or_insert_styled(queue, GlyphKey::plain(glyph_id))
    }

    /// Get a glyph's atlas entry for a specific style (D10), rasterizing +
    /// uploading it on first request through `key.face`'s rasterizer (D7).
    /// Bold/italic variants cache separately from the regular glyph via the
    /// [`GlyphKey`].
    ///
    /// Glyph id 0 (`.notdef`) is refused in normal mode and packed in degraded
    /// mode (R2.6/R2.5) — the mirror of [`resolve`](Atlas::resolve)'s rule. The
    /// guard has to live here: `rasterize_glyph_styled` deliberately does not
    /// filter id 0 (only `rasterize()` does), and the shaping path calls the
    /// former, which is how the tofu boxes reached the screen in the first place.
    pub fn get_or_insert_styled(
        &mut self,
        queue: &wgpu::Queue,
        key: GlyphKey,
    ) -> Option<GlyphEntry> {
        if key.id == 0 && !self.is_degraded() {
            return None;
        }
        if let Some(cached) = self.cache.get(&key) {
            return *cached;
        }
        // A key naming a face outside the chain is malformed, not fatal: draw
        // nothing rather than panic on an out-of-range index (no `unwrap`, C4).
        let rasterizer = self.rasterizers.get_mut(key.face as usize)?;
        let bmp = rasterizer.rasterize_glyph_styled(key.id, key.bold, key.italic);
        let entry = bmp.and_then(|bmp| self.pack(queue, &bmp));
        self.cache.insert(key, entry);
        entry
    }

    /// A packed fully-opaque entry used to draw solid rules (underline) in the
    /// glyph pass. Its UV rect samples constant 255 coverage, so a quad using it
    /// renders as a solid fill in the instance color. Re-reserved after eviction.
    pub fn solid_entry(&mut self, queue: &wgpu::Queue) -> GlyphEntry {
        if let Some((entry, gen)) = self.solid {
            if gen == self.generation {
                return entry;
            }
        }
        let bmp = GlyphBitmap {
            width: 2,
            height: 2,
            left: 0,
            top: 0,
            coverage: vec![255u8; 4],
        };
        // A 2×2 block always fits (possibly after one eviction inside `pack`).
        let entry = self
            .pack(queue, &bmp)
            .expect("a 2x2 solid block must fit the atlas");
        self.solid = Some((entry, self.generation));
        entry
    }

    /// Number of distinct glyphs currently cached (incl. empty). Test/observability.
    pub fn cached_len(&self) -> usize {
        self.cache.len()
    }

    /// Test-only: drop every fallback rasterizer so the atlas reports
    /// [`is_degraded`](Atlas::is_degraded). Lets the degraded branch of D5 —
    /// `.notdef` stays visible (R2.5) — be exercised on a machine where the
    /// Hangul fallback font *is* present, which is every dev machine.
    #[cfg(test)]
    pub(crate) fn degrade_for_test(&mut self) {
        self.rasterizers.truncate(1);
        self.cache.clear();
    }

    /// Shelf-pack a rasterized bitmap into the texture, uploading its coverage.
    fn pack(&mut self, queue: &wgpu::Queue, bmp: &GlyphBitmap) -> Option<GlyphEntry> {
        let (w, h) = (bmp.width, bmp.height);
        if w == 0 || h == 0 || w > ATLAS_SIZE || h > ATLAS_SIZE {
            return None;
        }
        // Advance to a new shelf if this glyph doesn't fit the current one.
        if self.shelf_x + w + PAD > ATLAS_SIZE {
            self.shelf_y += self.shelf_h + PAD;
            self.shelf_x = 0;
            self.shelf_h = 0;
        }
        // Atlas full → bounded eviction: clear everything and repack from origin.
        if self.shelf_y + h + PAD > ATLAS_SIZE {
            self.cache.clear();
            self.shelf_x = 0;
            self.shelf_y = 0;
            self.shelf_h = 0;
            self.generation += 1;
        }
        let x = self.shelf_x;
        let y = self.shelf_y;
        self.shelf_x += w + PAD;
        self.shelf_h = self.shelf_h.max(h);

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &bmp.coverage,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );

        let s = ATLAS_SIZE as f32;
        Some(GlyphEntry {
            uv0: [x as f32 / s, y as f32 / s],
            uv1: [(x + w) as f32 / s, (y + h) as f32 / s],
            px_w: w as f32,
            px_h: h as f32,
            left: bmp.left as f32,
            top: bmp.top as f32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn plain_keys_name_the_primary_face() {
        // D7: the face-blind entry point is face 0, never "some face".
        assert_eq!(GlyphKey::plain(42).face, 0);
        assert_eq!(GlyphKey::plain(42).id, 42);
        assert!(!GlyphKey::plain(42).bold && !GlyphKey::plain(42).italic);
    }

    #[test]
    fn the_face_axis_separates_cache_slots() {
        // R3.1, at the key level: same glyph id, different face → different cache
        // entry. Glyph ids are per-font namespaces, so collapsing these would draw
        // Menlo's gid 42 for the Hangul face's gid 42.
        let primary = GlyphKey::plain(42);
        let fallback = GlyphKey { face: 1, ..primary };
        assert_ne!(primary, fallback);
        let distinct: HashSet<GlyphKey> = [primary, fallback].into_iter().collect();
        assert_eq!(distinct.len(), 2, "face must participate in the hash");
    }

    #[test]
    fn atlas_is_2048_square() {
        // D13/R3.3: capacity headroom for Hangul's glyph count.
        assert_eq!(ATLAS_SIZE, 2048);
    }
}
