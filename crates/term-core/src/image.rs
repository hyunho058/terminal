//! Inline-image side channel (image-paste D1/D2, SD5).
//!
//! Holds decoded RGBA image placements anchored to cell coordinates, kept
//! alongside the grid and exposed on the [`crate::GridSnapshot`] — images are a
//! *side channel*, never stored in cells (SD5). GPU-free (C1): this module only
//! decodes to RGBA bytes; the renderer uploads + draws them.
//!
//! Kitty graphics ride APC escapes (`ESC _ G <control> ; <base64> ESC \`). Note:
//! `vte` 0.15 silently discards APC strings, so `parser.rs` hand-scans APC out of
//! the byte stream and hands the payload here (correcting D1's stated
//! `hook/put/unhook` mechanism, which is DCS — not where Kitty lives).

use base64::Engine;

use crate::grid::Cell;

/// One placed image: RGBA pixels anchored to a top-left cell, spanning
/// `cols × rows` cells (D2).
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub id: u32,
    pub col: u16,
    pub row: u16,
    pub cols: u16,
    pub rows: u16,
    pub px_w: u32,
    pub px_h: u32,
    pub rgba: Vec<u8>,
}

/// A decoded image, before it is anchored to a cell.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub rgba: Vec<u8>,
    pub px_w: u32,
    pub px_h: u32,
    pub cols: u16,
    pub rows: u16,
}

/// The set of on-screen image placements (D2). Scrolls with the grid.
#[derive(Clone, Debug, Default)]
pub struct ImageLayer {
    placements: Vec<Placement>,
    next_id: u32,
}

impl ImageLayer {
    pub fn new() -> Self {
        ImageLayer::default()
    }

    /// Anchor a decoded image at `(col, row)`; returns its id.
    pub fn place(&mut self, decoded: Decoded, col: u16, row: u16) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.placements.push(Placement {
            id,
            col,
            row,
            cols: decoded.cols,
            rows: decoded.rows,
            px_w: decoded.px_w,
            px_h: decoded.px_h,
            rgba: decoded.rgba,
        });
        id
    }

    /// Move every placement up one row on a scroll; drop any that scroll off the
    /// top of the live view (R1.2).
    pub fn scroll_up_one(&mut self) {
        self.placements.retain(|p| p.row > 0);
        for p in &mut self.placements {
            p.row -= 1;
        }
    }

    /// Drop everything (e.g. on a full clear / alt-screen).
    pub fn clear(&mut self) {
        self.placements.clear();
    }

    pub fn placements(&self) -> &[Placement] {
        &self.placements
    }

    pub fn is_empty(&self) -> bool {
        self.placements.is_empty()
    }
}

/// Parse a Kitty graphics APC payload (the bytes between `ESC _` and `ESC \`,
/// starting with `G`) into a decoded image. Returns `None` for anything not a
/// decodable transmit-and-display (R1.3 — malformed is dropped, never panics).
pub fn parse_graphics_apc(content: &[u8]) -> Option<Decoded> {
    let rest = content.strip_prefix(b"G")?;
    // control ; base64-payload
    let sep = rest.iter().position(|&b| b == b';')?;
    let control = std::str::from_utf8(&rest[..sep]).ok()?;
    let payload_b64 = &rest[sep + 1..];

    let mut fmt = 32u32; // f: 24 | 32 | 100(png)
    let (mut s, mut v) = (0u32, 0u32); // pixel dims (for raw formats)
    let (mut c, mut r) = (0u16, 0u16); // cell span
    for kv in control.split(',') {
        let Some((k, val)) = kv.split_once('=') else {
            continue;
        };
        match k {
            "f" => fmt = val.parse().unwrap_or(32),
            "s" => s = val.parse().unwrap_or(0),
            "v" => v = val.parse().unwrap_or(0),
            "c" => c = val.parse().unwrap_or(0),
            "r" => r = val.parse().unwrap_or(0),
            _ => {}
        }
    }

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload_b64)
        .ok()?;

    let (rgba, px_w, px_h) = match fmt {
        100 => decode_png(&bytes)?,
        32 => {
            // Saturating u64 math: s/v are attacker-controlled, so `s * v * 4`
            // must not overflow (R1.3 — no panic on adversarial dims).
            let need = raw_len(s, v, 4)?;
            if s == 0 || v == 0 || bytes.len() < need {
                return None;
            }
            (bytes[..need].to_vec(), s, v)
        }
        24 => {
            let need = raw_len(s, v, 3)?;
            if s == 0 || v == 0 || bytes.len() < need {
                return None;
            }
            (rgb_to_rgba(&bytes[..need]), s, v)
        }
        _ => return None,
    };

    // Cell span: prefer the explicit c/r; else a rough first-cut estimate from px
    // (Known Gap: exact cell fit is refined once cell metrics reach term-core).
    let cols = if c > 0 {
        c
    } else {
        (px_w.div_ceil(8)).clamp(1, u16::MAX as u32) as u16
    };
    let rows = if r > 0 {
        r
    } else {
        (px_h.div_ceil(16)).clamp(1, u16::MAX as u32) as u16
    };

    Some(Decoded {
        rgba,
        px_w,
        px_h,
        cols,
        rows,
    })
}

/// Byte length of a `w x h` raw image at `bpp` bytes/pixel, or `None` if it can't
/// fit in `usize` (guards against overflow on attacker-controlled dims, R1.3).
fn raw_len(w: u32, h: u32, bpp: u32) -> Option<usize> {
    (w as u64)
        .checked_mul(h as u64)?
        .checked_mul(bpp as u64)?
        .try_into()
        .ok()
}

fn decode_png(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some((rgba.into_raw(), w, h))
}

fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for px in rgb.chunks_exact(3) {
        out.extend_from_slice(px);
        out.push(255);
    }
    out
}

/// Marker so the snapshot module can name the cell type without a separate import.
pub type ImageCell = Cell;

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 solid-red raw-RGBA (f=32) transmit, base64-encoded.
    fn red_2x2_apc() -> Vec<u8> {
        let rgba: Vec<u8> = std::iter::repeat_n([255u8, 0, 0, 255], 4)
            .flatten()
            .collect();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&rgba);
        format!("Gf=32,s=2,v=2,c=1,r=1;{b64}").into_bytes()
    }

    #[test]
    fn parses_raw_rgba_transmit() {
        let d = parse_graphics_apc(&red_2x2_apc()).expect("decodes");
        assert_eq!((d.px_w, d.px_h), (2, 2));
        assert_eq!((d.cols, d.rows), (1, 1));
        assert_eq!(d.rgba.len(), 2 * 2 * 4);
        assert_eq!(&d.rgba[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn malformed_payloads_are_dropped_not_panicking() {
        assert!(parse_graphics_apc(b"not-graphics").is_none()); // no 'G'
        assert!(parse_graphics_apc(b"Gf=32,s=2,v=2").is_none()); // no ';' payload
        assert!(parse_graphics_apc(b"Gf=100;@@notbase64@@").is_none()); // bad base64
        assert!(parse_graphics_apc(b"Gf=100;aGVsbG8=").is_none()); // valid b64, not a PNG
        assert!(parse_graphics_apc(b"Gf=32,s=9,v=9;AAAA").is_none()); // payload too short
                                                                      // Adversarial dims: s*v*4 overflows u32 — must be None, never panic (R1.3).
        assert!(parse_graphics_apc(b"Gf=32,s=66000,v=66000;AAAA").is_none());
        assert!(parse_graphics_apc(b"Gf=24,s=4000000000,v=4000000000;AAAA").is_none());
    }

    #[test]
    fn placements_scroll_up_and_drop() {
        let mut layer = ImageLayer::new();
        let d = parse_graphics_apc(&red_2x2_apc()).unwrap();
        layer.place(d.clone(), 0, 0); // row 0 → dropped on scroll
        layer.place(d, 3, 2); // row 2 → moves to 1
        layer.scroll_up_one();
        assert_eq!(layer.placements().len(), 1);
        assert_eq!(layer.placements()[0].row, 1);
    }
}
