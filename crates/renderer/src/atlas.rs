//! Dynamic glyph atlas (D1): a single R8Unorm coverage texture filled on demand.
//!
//! Rasterized glyphs (via `swash`, see [`crate::glyph`]) are shelf-packed into one
//! GPU texture; each `glyph_id` at the atlas's fixed pixel size caches its UV rect
//! plus placement metrics (D6). The atlas is bounded (D1/R1.3): when a shelf run
//! fills the texture it evicts everything and repacks, rather than growing
//! unboundedly or panicking.

use std::collections::HashMap;
use std::sync::Arc;

use crate::glyph::{GlyphBitmap, GlyphRasterizer};

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

const ATLAS_SIZE: u32 = 1024;
const PAD: u32 = 1;

/// The glyph atlas: a GPU texture + a shelf packer + a per-glyph cache.
pub struct Atlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    rasterizer: GlyphRasterizer,
    cache: HashMap<u16, Option<GlyphEntry>>,
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    /// Bumped whenever the atlas is cleared (bounded eviction).
    generation: u64,
}

impl Atlas {
    pub fn new(
        device: &wgpu::Device,
        font: Arc<Vec<u8>>,
        index: usize,
        ppem: f32,
    ) -> Option<Atlas> {
        let rasterizer = GlyphRasterizer::new((*font).clone(), index, ppem)?;
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
            rasterizer,
            cache: HashMap::new(),
            shelf_x: 0,
            shelf_y: 0,
            shelf_h: 0,
            generation: 0,
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

    /// Map a character to its glyph id via the atlas's font (0 == `.notdef`).
    pub fn glyph_id(&self, ch: char) -> u16 {
        self.rasterizer.glyph_id(ch)
    }

    /// Get a glyph's atlas entry, rasterizing + uploading it on first request.
    /// `None` = the glyph has no visible outline (e.g. a space).
    pub fn get_or_insert(&mut self, queue: &wgpu::Queue, glyph_id: u16) -> Option<GlyphEntry> {
        if let Some(cached) = self.cache.get(&glyph_id) {
            return *cached;
        }
        let bmp = self.rasterizer.rasterize_glyph(glyph_id);
        let entry = bmp.and_then(|bmp| self.pack(queue, &bmp));
        self.cache.insert(glyph_id, entry);
        entry
    }

    /// Number of distinct glyphs currently cached (incl. empty). Test/observability.
    pub fn cached_len(&self) -> usize {
        self.cache.len()
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
