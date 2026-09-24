//! Dynamic glyph atlas (D1): a single R8Unorm coverage texture filled on demand.
//!
//! Rasterized glyphs (via `swash`, see [`crate::glyph`]) are shelf-packed into one
//! GPU texture; each `glyph_id` at the atlas's fixed pixel size caches its UV rect
//! plus placement metrics (D6). The atlas is bounded (D1/R1.3): when a shelf run
//! fills the texture it evicts everything and repacks, rather than growing
//! unboundedly or panicking.
//!
//! The atlas spans the whole [`FontSet`] fallback chain (H-D7): it holds a
//! [`FamilyRasterizers`] per chain entry — up to four [`GlyphRasterizer`]s, one per
//! style slot — and [`GlyphKey`] carries both the entry index and the slot.
//!
//! Both axes are *correctness* requirements, not cache refinements, and for the
//! same underlying reason: **glyph ids are a per-face namespace**. Without the face
//! axis, Menlo's gid 42 and the Hangul face's gid 42 share a cache slot (H-R3.1).
//! Without the slot axis, Menlo's *regular* gid 407 (`Ǟ`, which the italic face
//! lacks and which is therefore drawn sheared) and its *italic* gid 407 (`ǟ`) share
//! one — measured reachable, and the wrong character wins (D15/R3.3).
//!
//! Style never selects the chain entry: coverage is judged on the regular face and
//! the slot is chosen *within* the entry that wins (D7). That is what keeps H-R0.3
//! — unstyled text renders exactly as it did before the chain existed — true of the
//! style axis too.
//!
//! [`Atlas::resolve`] is the single character→glyph entry point for every draw
//! path (grid, tab bar, overlay, preedit — D5/D10), so fallback and the
//! degraded-mode rules are decided in exactly one place.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::font_set::{FontSet, StyleResolution, StyleSlot};
use crate::glyph::{GlyphBitmap, GlyphRasterizer};

/// Source of [`Atlas::epoch`] — the identity of a *texture*, handed out by
/// [`new_atlas_texture`] and never reused (atlas-swap-bind-invalidation D1/R1.2).
static NEXT_ATLAS_EPOCH: AtomicU64 = AtomicU64::new(1);

/// **The only place an atlas texture is born.** Returns the texture, its view and
/// sampler, and a fresh `epoch` identifying all three.
///
/// Routing every creation through here is what makes "new texture ⟹ new epoch"
/// true *by construction* rather than by comment. That matters because the layers
/// that sample this texture cache their bind groups keyed on `epoch`: a texture
/// swapped without a new epoch leaves them sampling the replaced texture with UVs
/// computed against the new one, which renders as wrong-glyph garbage that
/// persists for the whole session (D1/R1.1).
///
/// **Do not create an atlas texture anywhere else.** If a future `Atlas::resize`
/// or in-place ppem rebuild swaps the texture without coming through here, `epoch`
/// freezes and that bug returns — behind a field whose name promises it cannot
/// (R6.3; spec Known Gap G-b).
fn new_atlas_texture(
    device: &wgpu::Device,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler, u64) {
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
    // Relaxed is enough: we need uniqueness, not ordering against other memory.
    let epoch = NEXT_ATLAS_EPOCH.fetch_add(1, Ordering::Relaxed);
    (texture, view, sampler, epoch)
}

/// Cache key for a rasterized glyph.
///
/// The key names **the face the id came from**, never the style that was asked
/// for (D15). Those are not the same thing: a character the italic face does not
/// cover is drawn from the *regular* face with a synthetic shear (D7), so its id
/// lives in `Regular`'s namespace while the cell is still italic.
///
/// Keying on the requested style instead collides two different glyphs, and the
/// collision is reachable — on Menlo, regular gid 407 is `Ǟ` (which the italic
/// face lacks) and italic gid 407 is `ǟ`. Both would key as
/// `{face: 0, id: 407, italic: true}`, whichever cached first would win, and the
/// other would render as the wrong character.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    /// Index into the [`FontSet`] chain the atlas was built from; 0 == primary.
    /// Still a **chain entry**, not a face — that is what keeps `is_degraded()`
    /// and `resolve`'s enumerate index meaning what they always meant (D10).
    pub face: u16,
    /// Which face *within* that chain entry owns [`GlyphKey::id`].
    pub slot: StyleSlot,
    pub id: u16,
    /// Embolden applied on top of `slot`'s outline — a fallback for a file with
    /// no real bold face, never used when `slot` is already `Bold` (D9).
    pub synth_bold: bool,
    /// Shear applied on top of `slot`'s outline. Still the normal path for
    /// Hangul: AppleSDGothicNeo ships no italic face at all.
    pub synth_italic: bool,
}

impl GlyphKey {
    /// The regular (unstyled) key for a glyph id in the **primary** family (H-D7).
    pub fn plain(id: u16) -> Self {
        GlyphKey {
            face: 0,
            slot: StyleSlot::Regular,
            id,
            synth_bold: false,
            synth_italic: false,
        }
    }
}

/// Why a style slot ended up with no rasterizer (D2).
///
/// The two cases look identical downstream — both fall back to synthesis — but
/// they mean opposite things about the machine, so only one is worth a log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotAbsence {
    /// The font file ships no such face. **Normal.** AppleSDGothicNeo has no
    /// italic face at all — all 18 of its faces are `style=Normal` — so warning
    /// here would fire on every startup, for every user, about a face that was
    /// never supposed to exist (R2.1).
    NotOffered,
    /// The file offers the face and it would not parse. **Anomalous** — the font
    /// is damaged or swash rejected it — so say so once and degrade (R2.2).
    Unparseable,
}

impl SlotAbsence {
    /// Whether this absence is a signal or just the shape of the font file.
    ///
    /// Split out as a named predicate rather than an inline `if` so the rule
    /// "absent is silent, broken is loud" is a value a test can assert, instead
    /// of a branch whose only evidence is a comment beside it.
    fn is_worth_reporting(self) -> bool {
        matches!(self, SlotAbsence::Unparseable)
    }
}

/// Classify a missing slot, or `None` when the slot was built successfully.
fn slot_absence(offered: bool, built: bool) -> Option<SlotAbsence> {
    match (offered, built) {
        (_, true) => None,
        (true, false) => Some(SlotAbsence::Unparseable),
        (false, false) => Some(SlotAbsence::NotOffered),
    }
}

/// The rasterizers built for one chain entry — one per style slot the family
/// actually supplied *and* that parsed (D2).
///
/// Kept as a nested struct rather than flattened into the atlas's vector on
/// purpose: flattening would make `rasterizers.len()` count style faces, and
/// `is_degraded()` is `len() == 1`. A Menlo-only chain with four style faces would
/// then report "not degraded" while Hangul had no font at all.
struct FamilyRasterizers {
    regular: GlyphRasterizer,
    bold: Option<GlyphRasterizer>,
    italic: Option<GlyphRasterizer>,
    bold_italic: Option<GlyphRasterizer>,
}

impl FamilyRasterizers {
    fn get(&self, slot: StyleSlot) -> Option<&GlyphRasterizer> {
        match slot {
            StyleSlot::Regular => Some(&self.regular),
            StyleSlot::Bold => self.bold.as_ref(),
            StyleSlot::Italic => self.italic.as_ref(),
            StyleSlot::BoldItalic => self.bold_italic.as_ref(),
        }
    }

    fn get_mut(&mut self, slot: StyleSlot) -> Option<&mut GlyphRasterizer> {
        match slot {
            StyleSlot::Regular => Some(&mut self.regular),
            StyleSlot::Bold => self.bold.as_mut(),
            StyleSlot::Italic => self.italic.as_mut(),
            StyleSlot::BoldItalic => self.bold_italic.as_mut(),
        }
    }

    /// Which style this entry can really serve, judged by the rasterizers that
    /// were **built** — not by what the file claimed to have. A face that failed
    /// to parse is absent here, so its style degrades to synthesis (D2/R2.2).
    fn resolve_style(&self, bold: bool, italic: bool) -> StyleResolution {
        crate::font_set::resolve_style_over(bold, italic, |slot| self.get(slot).is_some())
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

/// Coverage values backing the shading blocks `░▒▓` (U+2591–2593), i.e. 25% / 50% /
/// 75% (D6).
///
/// These are *alpha*, not color. `glyph.wgsl` returns `vec4(color * a, a)` from the
/// sampled coverage, so a quad on a 64-coverage block is a correct premultiplied
/// 25%-alpha fill in the cell's foreground — no shader change, no per-instance alpha
/// channel, and it composites correctly over an inline image the way a CPU-side
/// blend toward the cell background would not.
const SHADE_LEVELS: [u8; 3] = [64, 128, 191];

/// The glyph atlas: a GPU texture + a shelf packer + a per-glyph cache.
pub struct Atlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    /// Identity of [`Atlas::texture`], stamped by [`new_atlas_texture`]. Distinct
    /// from [`Atlas::generation`] and orthogonal to it: `epoch` says *which
    /// texture*, `generation` says *how many times that texture was repacked*.
    ///
    /// Text layers key their cached bind groups on this so a live `apply_config`
    /// atlas swap cannot leave one sampling the replaced texture (D1/D2).
    epoch: u64,
    /// One entry per [`FontSet`] **chain step**, in priority order; index 0 is the
    /// primary. [`GlyphKey::face`] indexes this vector (H-D7).
    ///
    /// Each entry carries up to four rasterizers, one per style slot — but the
    /// vector's *length* is still the chain length, which `is_degraded()` and
    /// `resolve`'s enumerate both depend on (D10).
    rasterizers: Vec<FamilyRasterizers>,
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
    /// Partial-coverage twins of [`Atlas::solid`], one per shading level, for the
    /// `░▒▓` block elements (D6). Same lazy re-reservation rule.
    shades: [Option<(GlyphEntry, u64)>; SHADE_LEVELS.len()],
}

impl Atlas {
    /// Build an atlas covering every face of `fonts` at `ppem` (D7).
    ///
    /// Each face contributes one [`GlyphRasterizer`] that *shares* the face's
    /// mapped bytes — the `FontData` handle is cloned, never the 55 MB behind it
    /// (D7/R3.2). `None` if any face fails to parse at its recorded index.
    pub fn new(device: &wgpu::Device, fonts: &FontSet, ppem: f32) -> Option<Atlas> {
        // A styled slot that the file simply does not have is **normal** and logs
        // nothing (R2.1) — AppleSDGothicNeo has no italic face by design, and
        // warning about it would fire on every single startup. A slot the file
        // *does* have but that fails to parse is a real anomaly: warn once and
        // fall back to synthesis (R2.2). Only the regular face is fatal (R2.3).
        let build_slot = |family: &crate::font_set::FontFamily, slot: StyleSlot| {
            let offered = family.slot(slot);
            let built =
                offered.and_then(|face| GlyphRasterizer::new(face.data.clone(), face.index, ppem));
            if let Some(absence) = slot_absence(offered.is_some(), built.is_some()) {
                if absence.is_worth_reporting() {
                    tracing::warn!(
                        ?slot,
                        index = offered.map(|f| f.index),
                        "font file offers this face but it failed to parse; \
                         falling back to synthesis for that style (D2/R2.2)"
                    );
                }
            }
            built
        };

        let rasterizers = fonts
            .families()
            .iter()
            .map(|family| {
                let face = family.regular();
                // R2.3: the regular face is the only fatal one. `?` here makes the
                // whole atlas `None`, exactly as before styles existed.
                let regular = GlyphRasterizer::new(face.data.clone(), face.index, ppem)?;
                Some(FamilyRasterizers {
                    regular,
                    bold: build_slot(family, StyleSlot::Bold),
                    italic: build_slot(family, StyleSlot::Italic),
                    bold_italic: build_slot(family, StyleSlot::BoldItalic),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        if rasterizers.is_empty() {
            return None;
        }
        let (texture, view, sampler, epoch) = new_atlas_texture(device);
        Some(Atlas {
            texture,
            view,
            sampler,
            epoch,
            rasterizers,
            cache: HashMap::new(),
            shelf_x: 0,
            shelf_y: 0,
            shelf_h: 0,
            generation: 0,
            solid: None,
            shades: [None; SHADE_LEVELS.len()],
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
    /// Identity of the underlying texture — see [`Atlas::epoch`] and
    /// [`new_atlas_texture`]. Unique per texture, unchanged by eviction.
    pub fn epoch(&self) -> u64 {
        self.epoch
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
    pub fn resolve(&self, ch: char, bold: bool, italic: bool) -> Option<GlyphKey> {
        for (face, family) in self.rasterizers.iter().enumerate() {
            // D7: the chain walk asks the **regular** face, always. Style never
            // changes *which font* a character comes from, so H-R0.3 — "what the
            // primary covers renders exactly as it did before the chain existed"
            // — survives the style axis untouched.
            let regular_id = family.regular.glyph_id(ch);
            if regular_id == 0 {
                continue;
            }
            let face = face as u16;
            let want = family.resolve_style(bold, italic);

            // Prefer the real styled face, but only if it actually has this
            // glyph. Menlo Italic is missing 267 codepoints Menlo Regular has
            // (`☁ ★ ☎ ✔` …), and Menlo Bold is missing 128 — though those 128 are
            // exactly the box-drawing block, which never reaches shaping anyway.
            if want.slot != StyleSlot::Regular {
                if let Some(styled) = family.get(want.slot) {
                    let id = styled.glyph_id(ch);
                    if id != 0 {
                        return Some(GlyphKey {
                            face,
                            slot: want.slot,
                            id,
                            synth_bold: want.synth_bold,
                            synth_italic: want.synth_italic,
                        });
                    }
                }
            }

            // R3.2: a hole in the styled face falls back to *this* entry's
            // regular with full synthesis — not to the next chain entry, which
            // would silently swap the font, and not to nothing, which would make
            // a character vanish the moment an attribute was added to it.
            return Some(GlyphKey {
                face,
                slot: StyleSlot::Regular,
                id: regular_id,
                synth_bold: bold,
                synth_italic: italic,
            });
        }
        // H-D5: keep `.notdef` visible when there is no fallback face to consult.
        self.is_degraded().then(|| GlyphKey::plain(0))
    }

    /// Map a character to its glyph id in the **primary** face (0 == `.notdef`).
    /// Face-blind entry point kept for callers that predate the chain; new code
    /// should use [`resolve`](Atlas::resolve) so fallback applies (D10).
    pub fn glyph_id(&self, ch: char) -> u16 {
        self.rasterizers
            .first()
            .map_or(0, |family| family.regular.glyph_id(ch))
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
        // A key naming a face outside the chain, or a slot this family never
        // built, is malformed rather than fatal: draw nothing instead of panicking
        // on an out-of-range index (no `unwrap`, C4).
        let rasterizer = self
            .rasterizers
            .get_mut(key.face as usize)?
            .get_mut(key.slot)?;
        let bmp = rasterizer.rasterize_glyph_synth(key.id, key.synth_bold, key.synth_italic);
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

    /// A packed partial-coverage entry for shading level `level` (0=`░`, 1=`▒`,
    /// 2=`▓`), the alpha-carrying twin of [`solid_entry`](Atlas::solid_entry) (D6).
    ///
    /// A quad using it renders as a `SHADE_LEVELS[level]/255` fill in the instance
    /// color, because `glyph.wgsl` multiplies the sampled coverage into both the rgb
    /// and the alpha. That is why `░▒▓` need no shader change and no per-instance
    /// alpha channel.
    ///
    /// Returns `None` — rather than `solid_entry`'s `expect` — on an out-of-range
    /// level or a pack failure, so the caller drops the quad instead of panicking.
    /// The project forbids `unwrap`/`expect` outside tests, and `Option` is what
    /// [`get_or_insert_styled`](Atlas::get_or_insert_styled) already hands back.
    pub fn shade_entry(&mut self, queue: &wgpu::Queue, level: usize) -> Option<GlyphEntry> {
        let coverage = *SHADE_LEVELS.get(level)?;
        if let Some((entry, gen)) = self.shades[level] {
            if gen == self.generation {
                return Some(entry);
            }
        }
        let bmp = GlyphBitmap {
            width: 2,
            height: 2,
            left: 0,
            top: 0,
            coverage: vec![coverage; 4],
        };
        let entry = self.pack(queue, &bmp)?;
        self.shades[level] = Some((entry, self.generation));
        Some(entry)
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

    /// Test-only: force the bounded-eviction path — clear the cache, reset the shelf
    /// cursor, and bump the generation, exactly as [`pack`](Atlas::pack) does when
    /// the texture fills. Lets the re-reservation rule for `solid`/`shades` be
    /// exercised without packing 2048² px worth of glyphs to trigger it for real.
    #[cfg(test)]
    pub(crate) fn evict_for_test(&mut self) {
        self.cache.clear();
        self.shelf_x = 0;
        self.shelf_y = 0;
        self.shelf_h = 0;
        self.generation += 1;
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
        let k = GlyphKey::plain(42);
        assert_eq!(k.slot, StyleSlot::Regular);
        assert!(!k.synth_bold && !k.synth_italic);
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

    // ---- T3 / R2-R3: slot resolution and the key that cannot collide --------

    #[test]
    fn an_unoffered_slot_is_silent_and_a_broken_one_is_not() {
        // R2.1 vs R2.2 — the distinction D2 exists to draw. Both end in synthesis,
        // so behaviour alone cannot tell them apart; what differs is whether the
        // machine is trying to tell you something.
        assert_eq!(slot_absence(false, false), Some(SlotAbsence::NotOffered));
        assert_eq!(slot_absence(true, false), Some(SlotAbsence::Unparseable));
        assert_eq!(slot_absence(true, true), None, "built — nothing to report");

        // R2.1: AppleSDGothicNeo legitimately has no italic face. Logging that
        // would put a warning on every startup about a face that was never meant
        // to exist.
        assert!(!SlotAbsence::NotOffered.is_worth_reporting());
        // R2.2: a face the file *does* offer failing to parse is a real signal.
        assert!(SlotAbsence::Unparseable.is_worth_reporting());
    }

    /// A `FontFace` over bytes that are not a font, at `index`.
    fn broken_face(index: usize) -> crate::font_set::FontFace {
        let bytes = [0u8, 1, 2, 3];
        let mut map = memmap2::MmapOptions::new()
            .len(bytes.len())
            .map_anon()
            .expect("anon mapping");
        map.copy_from_slice(&bytes);
        crate::font_set::FontFace {
            data: std::sync::Arc::new(map.make_read_only().expect("freeze")),
            index,
        }
    }

    #[test]
    fn a_broken_style_face_degrades_that_style_but_still_builds_the_atlas() {
        // R2.2: going from 2 faces to 8 quadruples the chance of hitting an
        // unparseable one. A decorative defect must not become a black window —
        // the trade H-D4 already refused for the fallback font.
        let Some((device, _queue)) = crate::headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let Ok(real) = FontSet::load() else {
            eprintln!("skipping: no system font");
            return;
        };
        let good = real.families()[0].regular().clone();
        let sabotaged = crate::font_set::FontFamily::from_parts(
            good,
            Some(broken_face(0)), // offered, will not parse
            None,
            None,
        );
        let fonts = FontSet::from_families(vec![sabotaged]);

        let atlas = Atlas::new(&device, &fonts, 32.0)
            .expect("R2.2: a broken STYLE face must not fail atlas construction");
        // and that style quietly falls back to synthesis on the regular face
        let key = atlas.resolve('A', true, false).expect("A resolves");
        assert_eq!(key.slot, StyleSlot::Regular);
        assert!(key.synth_bold, "the weight is owed as synthesis now");
    }

    #[test]
    fn a_broken_regular_face_is_still_fatal() {
        // R2.3: the one case that keeps its old severity. Without a regular face
        // there is nothing to fall back *to*, so `Atlas::new` answers `None`
        // exactly as it did before style slots existed.
        let Some((device, _queue)) = crate::headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let fonts = FontSet::from_families(vec![crate::font_set::FontFamily::from_parts(
            broken_face(0),
            None,
            None,
            None,
        )]);
        assert!(
            Atlas::new(&device, &fonts, 32.0).is_none(),
            "R2.3: a broken REGULAR face must still be fatal"
        );
    }

    #[test]
    fn a_style_slot_is_part_of_the_key() {
        // R3.3 at the key level: same face, same id, different slot → different
        // cache entry. Without this the sheared-regular `Ǟ` and the real italic
        // `ǟ` (both gid 407 on Menlo) share one slot and one wins.
        let sheared_regular = GlyphKey {
            face: 0,
            slot: StyleSlot::Regular,
            id: 407,
            synth_bold: false,
            synth_italic: true,
        };
        let real_italic = GlyphKey {
            slot: StyleSlot::Italic,
            synth_italic: false,
            ..sheared_regular
        };
        assert_ne!(sheared_regular, real_italic);
        let distinct: HashSet<GlyphKey> = [sheared_regular, real_italic].into_iter().collect();
        assert_eq!(distinct.len(), 2, "slot must participate in the hash");
    }

    #[test]
    fn hangul_bold_resolves_to_the_real_bold_slot() {
        // R0.1: the point of the whole feature, at the resolver.
        let Some((_, atlas)) = atlas_for_test() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let key = atlas.resolve('한', true, false).expect("한 resolves");
        assert_eq!(key.face, 1, "Hangul comes from the fallback chain entry");
        assert_eq!(key.slot, StyleSlot::Bold, "a real w700 face, not synthesis");
        assert!(
            !key.synth_bold,
            "the weight is real, so embolden must never be applied — that is what \
             erodes 593 syllables"
        );
    }

    #[test]
    fn hangul_bold_italic_keeps_the_real_weight_and_shears() {
        // D9/R2.5: axes resolve independently. AppleSDGothicNeo has no italic
        // face, so the slant is synthetic — but the weight must stay real.
        let Some((_, atlas)) = atlas_for_test() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let key = atlas.resolve('한', true, true).expect("한 resolves");
        assert_eq!(key.slot, StyleSlot::Bold);
        assert!(!key.synth_bold, "real weight");
        assert!(key.synth_italic, "synthetic slant");
    }

    #[test]
    fn a_hole_in_the_styled_face_stays_on_the_same_chain_entry() {
        // R3.1/R3.2. `★` is one of 267 codepoints Menlo Regular has and Menlo
        // Italic does not. Adding an italic attribute must not move it to another
        // font, and must not make it vanish.
        let Some((_, atlas)) = atlas_for_test() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let plain = atlas.resolve('★', false, false).expect("★ resolves plain");
        let italic = atlas.resolve('★', false, true).expect("★ resolves italic");
        assert_eq!(
            italic.face, plain.face,
            "R3.1: style must not change WHICH font a character comes from"
        );
        assert_eq!(
            italic.slot,
            StyleSlot::Regular,
            "R3.2: the italic face has no ★, so fall back to this entry's regular"
        );
        assert_eq!(italic.id, plain.id, "same face ⇒ same glyph id");
        assert!(italic.synth_italic, "and owe the shear");
    }

    #[test]
    fn the_italic_hole_and_a_real_italic_glyph_get_different_keys() {
        // R3.4 — the collision, end to end on real fonts. On Menlo, regular gid
        // 407 is `Ǟ` (absent from the italic face) and italic gid 407 is `ǟ`.
        // Keyed on the requested style they would be indistinguishable.
        let Some((_, atlas)) = atlas_for_test() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let hole = atlas.resolve('Ǟ', false, true).expect("Ǟ resolves");
        let real = atlas.resolve('ǟ', false, true).expect("ǟ resolves");
        assert_eq!(hole.slot, StyleSlot::Regular, "Ǟ: sheared regular");
        assert_eq!(real.slot, StyleSlot::Italic, "ǟ: the real italic face");
        assert_ne!(
            hole, real,
            "these must never share a cache entry, whatever their ids"
        );
    }

    #[test]
    fn an_empty_style_slot_does_not_degrade_the_chain() {
        // R2.4/D11: `is_degraded` asks whether Hangul has a font at all. Gothic
        // ships no italic face, and that must not be mistaken for a missing
        // fallback — which would wrongly switch on the `.notdef` path (H-D5).
        let Some((_, atlas)) = atlas_for_test() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        assert!(
            !atlas.is_degraded(),
            "two chain entries loaded, so not degraded — regardless of style slots"
        );
        assert_eq!(
            atlas.rasterizers.len(),
            2,
            "R3.6: the vector counts CHAIN ENTRIES, not style faces"
        );
    }

    #[test]
    fn atlas_is_2048_square() {
        // D13/R3.3: capacity headroom for Hangul's glyph count.
        assert_eq!(ATLAS_SIZE, 2048);
    }

    #[test]
    fn shade_levels_are_the_quarter_steps() {
        // D6: 25% / 50% / 75% of 255, the alpha `░▒▓` render at.
        assert_eq!(SHADE_LEVELS, [64, 128, 191]);
    }

    /// The atlas needs a real device; adapterless CI skips (D10).
    fn atlas_for_test() -> Option<(wgpu::Queue, Atlas)> {
        let (device, queue) = crate::headless()?;
        let fonts = FontSet::load().ok()?;
        let atlas = Atlas::new(&device, &fonts, 32.0)?;
        Some((queue, atlas))
    }

    #[test]
    fn shade_entries_are_distinct_and_memoized() {
        // R7.1: three levels → three distinct entries, and a second request returns
        // the same one rather than packing again.
        let Some((queue, mut atlas)) = atlas_for_test() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let first: Vec<GlyphEntry> = (0..3)
            .map(|l| atlas.shade_entry(&queue, l).expect("a 2x2 block fits"))
            .collect();
        let distinct: HashSet<[u32; 2]> = first
            .iter()
            .map(|e| [e.uv0[0].to_bits(), e.uv0[1].to_bits()])
            .collect();
        assert_eq!(distinct.len(), 3, "each level packs its own block");

        for (l, want) in first.iter().enumerate() {
            let again = atlas.shade_entry(&queue, l).expect("memoized");
            assert_eq!(&again, want, "level {l} must not repack");
        }
    }

    #[test]
    fn shade_entry_refuses_an_out_of_range_level() {
        // R7.3: `None`, not a panic and not a silent wrong level.
        let Some((queue, mut atlas)) = atlas_for_test() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        assert!(atlas.shade_entry(&queue, 3).is_none());
        assert!(atlas.shade_entry(&queue, usize::MAX).is_none());
    }

    #[test]
    fn shade_entries_are_re_reserved_after_an_eviction() {
        // R7.2: a generation bump invalidates every UV already handed out, so the
        // memo must not survive it — the same rule `solid_entry` follows.
        let Some((queue, mut atlas)) = atlas_for_test() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        // Occupy the first shelf slot so the shade block does NOT land at the
        // origin — otherwise the post-eviction repack would coincidentally return
        // the same UV and this test would pass without proving anything.
        atlas.solid_entry(&queue);
        let before = atlas.shade_entry(&queue, 1).expect("a 2x2 block fits");
        assert_ne!(
            before.uv0[0], 0.0,
            "the shade block sits past the solid one"
        );
        let gen_before = atlas.generation;

        atlas.evict_for_test();
        assert_ne!(
            atlas.generation, gen_before,
            "eviction bumps the generation"
        );

        let after = atlas.shade_entry(&queue, 1).expect("re-reserved");
        assert_eq!(
            atlas.shades[1].map(|(_, g)| g),
            Some(atlas.generation),
            "the memo is stamped with the new generation"
        );
        // The re-packed block starts a fresh shelf run, so it lands at the origin
        // rather than wherever it sat before the clear.
        assert_ne!(
            (before.uv0[0].to_bits(), before.uv0[1].to_bits()),
            (after.uv0[0].to_bits(), after.uv0[1].to_bits()),
            "a stale pre-eviction UV must not be handed back"
        );
    }

    #[test]
    fn solid_entry_still_memoizes() {
        // R7.4: the underline path is untouched by the shade addition.
        let Some((queue, mut atlas)) = atlas_for_test() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let a = atlas.solid_entry(&queue);
        let b = atlas.solid_entry(&queue);
        assert_eq!(a, b);
    }

    #[test]
    fn two_atlases_never_share_an_epoch() {
        // R1.3: the whole fix rests on this. Two atlases built from identical
        // inputs are still two *textures*, so a consumer keyed on `epoch` must be
        // able to tell them apart. `generation` cannot do this job — both start at
        // 0, which is exactly the bug (a swap looked like "nothing changed").
        //
        // Only inequality is asserted: the counter is process-global and shared
        // with every other test running in this binary, so no specific value is
        // stable.
        let Some((device, _queue)) = crate::headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let Ok(fonts) = FontSet::load() else {
            eprintln!("skipping: no font");
            return;
        };
        let (Some(a), Some(b)) = (
            Atlas::new(&device, &fonts, 32.0),
            Atlas::new(&device, &fonts, 32.0),
        ) else {
            eprintln!("skipping: atlas build failed");
            return;
        };
        assert_ne!(
            a.epoch(),
            b.epoch(),
            "a fresh atlas is a fresh texture and must carry a fresh epoch"
        );
        assert_eq!(
            (a.generation(), b.generation()),
            (0, 0),
            "both generations start at 0 — which is precisely why generation \
             cannot stand in for texture identity"
        );
    }

    #[test]
    fn eviction_bumps_the_generation_but_not_the_epoch() {
        // R1.5: the two counters are orthogonal, and the bind-group key depends on
        // that. Eviction reuses the same texture (and so the same view/sampler), so
        // rebuilding a bind group for it would produce a byte-identical result —
        // `epoch` must stay put or every eviction would churn bind groups for
        // nothing (the G1 idle-CPU regression).
        let Some((_queue, mut atlas)) = atlas_for_test() else {
            eprintln!("skipping: no GPU/font");
            return;
        };
        let epoch_before = atlas.epoch();
        let gen_before = atlas.generation();
        atlas.evict_for_test();
        assert_eq!(
            atlas.epoch(),
            epoch_before,
            "eviction reuses the texture, so its identity is unchanged"
        );
        assert_eq!(
            atlas.generation(),
            gen_before + 1,
            "eviction still invalidates every UV handed out"
        );
    }
}
