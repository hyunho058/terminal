//! Procedural box-drawing and block-element geometry (D2/D4/D7).
//!
//! Characters in `U+2500..=U+259F` — except the three diagonals `╱╲╳` — are not
//! shaped through the font. They are described here as axis-aligned rectangles
//! snapped to the cell, and emitted as solid quads by the grid's glyph pass.
//!
//! # Why
//!
//! A shaped glyph is placed at `baseline - entry.top` with every term an unrounded
//! float from the rasterizer. Two horizontally adjacent `─` cells therefore need not
//! land on the same pixel row, which is what makes box art show hairline gaps and
//! double-thick seams. Rectangles derived from the cell rect cannot: [`band`] and
//! [`snap`] are pure functions of `(extent, thickness | fraction)`, identical for
//! every cell, so a shared edge is computed from the same inputs on both sides.
//!
//! # The two helpers everything goes through (D7)
//!
//! Nothing in this module rounds on its own. All geometry comes from [`band`] (a
//! centered stem of a given thickness) or [`snap`] (a fractional split of the cell).
//! That is what makes "neighbouring cells share integer edges" a property of the
//! arithmetic rather than of the table being written carefully.
//!
//! # Invariants
//!
//! * **light ⊆ heavy** — a light band always sits fully inside the heavy band on the
//!   same axis, so a light arm meeting a heavy one widens into a clean T. They are
//!   *not* concentric and cannot be: an odd-width and an even-width band have centers
//!   half a pixel apart on an integer lattice, and no rounding rule fixes that.
//! * **light ∥ double** — the light band and the `3 * light` double span *are*
//!   concentric (exactly), because `L` and `3L` share parity. This is what keeps the
//!   `╪ ╫ ╞ ╢` crossings symmetric, and it is a different property from containment.
//!
//! Both are enforced by tests over a swept range, not by comment.

/// An axis-aligned rectangle in physical pixels, top-left origin — the unit this
/// module speaks in. Converted to a quad by the caller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Rect { x0, y0, x1, y1 }
    }

    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    /// Whether the rect encloses at least one pixel. The emit path drops anything
    /// that doesn't (R3.5) rather than sending a degenerate quad to the GPU.
    pub fn is_visible(&self) -> bool {
        self.width() >= 1.0 && self.height() >= 1.0
    }
}

/// Fraction of cell height that makes one light stem (D1).
const LIGHT_RATIO: f32 = 0.075;

/// The light stem thickness for a cell of height `ch` (D1).
///
/// Derived from **height** and then used on both axes, so the two arms of `┼` come
/// out the same weight. Tying it to width instead would make the weight jump on a
/// font whose aspect ratio differs from Menlo's. The floor is what keeps a stem
/// visible at tiny cell sizes (R2.3) — `round` alone reaches 0 below `ch ≈ 6.7`.
pub fn light_thickness(ch: f32) -> f32 {
    (ch * LIGHT_RATIO).round().max(1.0)
}

/// The heavy stem thickness: twice light (D1).
pub fn heavy_thickness(ch: f32) -> f32 {
    2.0 * light_thickness(ch)
}

/// A centered band of thickness `t` within `[origin, origin + extent]` (D7).
///
/// Returns `(start, end)`. The offset from `origin` depends only on `(extent, t)`,
/// so every cell in a column — or row — gets the same band, and two adjacent cells'
/// spans abut exactly. This is the whole seamlessness argument in one function.
pub fn band(origin: f32, extent: f32, t: f32) -> (f32, f32) {
    // A stem can never be thicker than the cell it lives in. Without this clamp a
    // heavy stem in a 1px-tall cell — `band(0, 1, 2)` — starts at
    // `round((1-2)*0.5) == -1` and paints a row into the cell ABOVE. Reachable via
    // `font.size_pt = 1.0`, and it corrupts the neighbouring row rather than merely
    // looking wrong. The clamp is inert at every realistic cell size: it only bites
    // when `t > extent`, which the containment (`2L ≤ e`) and concentricity
    // (`3L ≤ e`) sweeps never reach.
    let t = t.min(extent).max(1.0);
    let start = origin + ((extent - t) * 0.5).round().max(0.0);
    (start, start + t)
}

/// The `3 * light` span a double stem occupies, centered like a single band (D7).
///
/// The two rules `╔═╗` draws are the outer thirds of this span; the middle third is
/// the gap. Sharing [`band`]'s centering is what makes the double span concentric
/// with a light band, which the `╪` crossings depend on.
pub fn double_span(origin: f32, extent: f32, light: f32) -> (f32, f32) {
    band(origin, extent, 3.0 * light)
}

/// The two rules of a double stem: outer thirds of [`double_span`], light gap between.
pub fn double_bands(origin: f32, extent: f32, light: f32) -> ((f32, f32), (f32, f32)) {
    let (start, end) = double_span(origin, extent, light);
    ((start, start + light), (end - light, end))
}

/// A fractional position within `[origin, origin + extent]`, snapped (D7).
///
/// Complementary fractions are exact complements *because* both sides call this with
/// the same `f`: `▀` ends where `▄` starts, at one shared value. Quadrants split on
/// the same lines the halves use, so the two tile with each other.
pub fn snap(origin: f32, extent: f32, f: f32) -> f32 {
    origin + (extent * f).round()
}

/// The weight of one arm of a box-drawing character (D4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Weight {
    /// No arm in this direction.
    #[default]
    None,
    Light,
    Heavy,
    /// A double rule — two light lines with a light gap.
    Double,
}

impl Weight {
    fn is_some(self) -> bool {
        self != Weight::None
    }

    /// The band thickness this weight draws at, for a cell of height `ch`.
    /// `Double` reports its *whole* span; its two rules come from [`double_bands`].
    fn thickness(self, ch: f32) -> f32 {
        let light = light_thickness(ch);
        match self {
            Weight::None => 0.0,
            Weight::Light => light,
            Weight::Heavy => 2.0 * light,
            Weight::Double => 3.0 * light,
        }
    }
}

/// A box-drawing character as four arms reaching out from the cell center (D4).
///
/// This is the whole model for `U+2500..=U+2570` and `U+2574..=U+257F`. Junctions are
/// *generated* from it — every arm's band comes from [`band`] with that arm's own
/// weight — so a corner cannot notch because of a mistyped table row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Arms {
    pub up: Weight,
    pub down: Weight,
    pub left: Weight,
    pub right: Weight,
    /// Dash count for the dashed variants; `None` for solid arms (D2). Filled in by
    /// the dashed-character work; solid characters leave it unset.
    pub dashes: Option<u8>,
}

impl Arms {
    const fn h(left: Weight, right: Weight) -> Self {
        Arms {
            up: Weight::None,
            down: Weight::None,
            left,
            right,
            dashes: None,
        }
    }

    const fn v(up: Weight, down: Weight) -> Self {
        Arms {
            up,
            down,
            left: Weight::None,
            right: Weight::None,
            dashes: None,
        }
    }

    const fn new(up: Weight, down: Weight, left: Weight, right: Weight) -> Self {
        Arms {
            up,
            down,
            left,
            right,
            dashes: None,
        }
    }

    /// Whether every present arm is [`Weight::Light`] — the guard on BOLD promotion.
    ///
    /// A character that already encodes a heavy or double arm (`╀`, `╂`, `╾`, …)
    /// must be left alone: promoting its light arms would turn `╀` into `╋` and
    /// destroy the distinction the codepoint exists to make (D3).
    fn is_uniformly_light(&self) -> bool {
        [self.up, self.down, self.left, self.right]
            .iter()
            .all(|w| matches!(w, Weight::None | Weight::Light))
    }

    /// The BOLD-adjusted arms (D3): light → heavy, but only for a uniformly-light
    /// character. Anything else, and any non-BOLD cell, is returned unchanged.
    pub fn with_bold(self, bold: bool) -> Self {
        if !bold || !self.is_uniformly_light() {
            return self;
        }
        let promote = |w: Weight| match w {
            Weight::Light => Weight::Heavy,
            other => other,
        };
        Arms {
            up: promote(self.up),
            down: promote(self.down),
            left: promote(self.left),
            right: promote(self.right),
            dashes: self.dashes,
        }
    }
}

use Weight::{Double as D, Heavy as H, Light as L, None as N};

/// What a character in the intercepted range draws (D2/D4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Glyph {
    /// Stems and junctions — the arm model.
    Box(Arms),
    /// A filled fraction of the cell: `(from, to)` along each axis as cell
    /// fractions. Covers halves, eighths, quadrants and the full block.
    Fill(Fraction),
    /// A full-cell wash at a shading level (0=`░`, 1=`▒`, 2=`▓`).
    Shade(usize),
    /// One to three of the cell's four quadrants, in the order upper-left,
    /// upper-right, lower-left, lower-right. Kept distinct from [`Glyph::Fill`]
    /// because `▚`/`▞` are two *disjoint* rects, not one — but every edge still
    /// resolves through the half-cell split lines, so they tile with `▀▄▌▐`.
    Quadrants([bool; 4]),
}

/// A rectangular fraction of the cell, in 0..1 units on each axis (D7).
///
/// Boundaries are resolved through [`snap`], which is what makes `▀` and `▄` exact
/// complements and lets quadrants share the halves' split lines.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fraction {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Fraction {
    const fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Fraction { x0, y0, x1, y1 }
    }
}

/// Classify a character into its procedural description, or `None` to let the font
/// draw it (D2).
///
/// `None` means exactly one thing: the three diagonals `╱╲╳` (`U+2571..=U+2573`),
/// which no axis-aligned rect can express and which never meet a stem mid-edge, so
/// they cannot show the anchor mismatch that motivated intercepting everything else.
/// Every other codepoint in `U+2500..=U+259F` is procedural.
pub fn classify(ch: char) -> Option<Glyph> {
    let arms = |a: Arms| Some(Glyph::Box(a));
    let fill = |f: Fraction| Some(Glyph::Fill(f));
    match ch {
        // ── Stems, solid and dashed ──────────────────────────────────────────
        '\u{2500}' => arms(Arms::h(L, L)),
        '\u{2501}' => arms(Arms::h(H, H)),
        '\u{2502}' => arms(Arms::v(L, L)),
        '\u{2503}' => arms(Arms::v(H, H)),
        // Dashed: same arms, plus a dash count. 3 for 2504-2507, 4 for 2508-250B.
        '\u{2504}' => arms(Arms::h(L, L).dashed(3)),
        '\u{2505}' => arms(Arms::h(H, H).dashed(3)),
        '\u{2506}' => arms(Arms::v(L, L).dashed(3)),
        '\u{2507}' => arms(Arms::v(H, H).dashed(3)),
        '\u{2508}' => arms(Arms::h(L, L).dashed(4)),
        '\u{2509}' => arms(Arms::h(H, H).dashed(4)),
        '\u{250A}' => arms(Arms::v(L, L).dashed(4)),
        '\u{250B}' => arms(Arms::v(H, H).dashed(4)),

        // ── Corners: down+right, down+left, up+right, up+left ────────────────
        '\u{250C}' => arms(Arms::new(N, L, N, L)),
        '\u{250D}' => arms(Arms::new(N, L, N, H)),
        '\u{250E}' => arms(Arms::new(N, H, N, L)),
        '\u{250F}' => arms(Arms::new(N, H, N, H)),
        '\u{2510}' => arms(Arms::new(N, L, L, N)),
        '\u{2511}' => arms(Arms::new(N, L, H, N)),
        '\u{2512}' => arms(Arms::new(N, H, L, N)),
        '\u{2513}' => arms(Arms::new(N, H, H, N)),
        '\u{2514}' => arms(Arms::new(L, N, N, L)),
        '\u{2515}' => arms(Arms::new(L, N, N, H)),
        '\u{2516}' => arms(Arms::new(H, N, N, L)),
        '\u{2517}' => arms(Arms::new(H, N, N, H)),
        '\u{2518}' => arms(Arms::new(L, N, L, N)),
        '\u{2519}' => arms(Arms::new(L, N, H, N)),
        '\u{251A}' => arms(Arms::new(H, N, L, N)),
        '\u{251B}' => arms(Arms::new(H, N, H, N)),

        // ── Vertical tees (├ family): up, down, right ────────────────────────
        '\u{251C}' => arms(Arms::new(L, L, N, L)),
        '\u{251D}' => arms(Arms::new(L, L, N, H)),
        '\u{251E}' => arms(Arms::new(H, L, N, L)),
        '\u{251F}' => arms(Arms::new(L, H, N, L)),
        '\u{2520}' => arms(Arms::new(H, H, N, L)),
        '\u{2521}' => arms(Arms::new(H, L, N, H)),
        '\u{2522}' => arms(Arms::new(L, H, N, H)),
        '\u{2523}' => arms(Arms::new(H, H, N, H)),
        // ── Vertical tees (┤ family): up, down, left ─────────────────────────
        '\u{2524}' => arms(Arms::new(L, L, L, N)),
        '\u{2525}' => arms(Arms::new(L, L, H, N)),
        '\u{2526}' => arms(Arms::new(H, L, L, N)),
        '\u{2527}' => arms(Arms::new(L, H, L, N)),
        '\u{2528}' => arms(Arms::new(H, H, L, N)),
        '\u{2529}' => arms(Arms::new(H, L, H, N)),
        '\u{252A}' => arms(Arms::new(L, H, H, N)),
        '\u{252B}' => arms(Arms::new(H, H, H, N)),

        // ── Horizontal tees (┬ family): down, left, right ────────────────────
        '\u{252C}' => arms(Arms::new(N, L, L, L)),
        '\u{252D}' => arms(Arms::new(N, L, H, L)),
        '\u{252E}' => arms(Arms::new(N, L, L, H)),
        '\u{252F}' => arms(Arms::new(N, L, H, H)),
        '\u{2530}' => arms(Arms::new(N, H, L, L)),
        '\u{2531}' => arms(Arms::new(N, H, H, L)),
        '\u{2532}' => arms(Arms::new(N, H, L, H)),
        '\u{2533}' => arms(Arms::new(N, H, H, H)),
        // ── Horizontal tees (┴ family): up, left, right ──────────────────────
        '\u{2534}' => arms(Arms::new(L, N, L, L)),
        '\u{2535}' => arms(Arms::new(L, N, H, L)),
        '\u{2536}' => arms(Arms::new(L, N, L, H)),
        '\u{2537}' => arms(Arms::new(L, N, H, H)),
        '\u{2538}' => arms(Arms::new(H, N, L, L)),
        '\u{2539}' => arms(Arms::new(H, N, H, L)),
        '\u{253A}' => arms(Arms::new(H, N, L, H)),
        '\u{253B}' => arms(Arms::new(H, N, H, H)),

        // ── Crosses (┼ family): all four arms ────────────────────────────────
        '\u{253C}' => arms(Arms::new(L, L, L, L)),
        '\u{253D}' => arms(Arms::new(L, L, H, L)),
        '\u{253E}' => arms(Arms::new(L, L, L, H)),
        '\u{253F}' => arms(Arms::new(L, L, H, H)),
        '\u{2540}' => arms(Arms::new(H, L, L, L)),
        '\u{2541}' => arms(Arms::new(L, H, L, L)),
        '\u{2542}' => arms(Arms::new(H, H, L, L)),
        '\u{2543}' => arms(Arms::new(H, L, H, L)),
        '\u{2544}' => arms(Arms::new(H, L, L, H)),
        '\u{2545}' => arms(Arms::new(L, H, H, L)),
        '\u{2546}' => arms(Arms::new(L, H, L, H)),
        '\u{2547}' => arms(Arms::new(H, L, H, H)),
        '\u{2548}' => arms(Arms::new(L, H, H, H)),
        '\u{2549}' => arms(Arms::new(H, H, H, L)),
        '\u{254A}' => arms(Arms::new(H, H, L, H)),
        '\u{254B}' => arms(Arms::new(H, H, H, H)),

        // ── Double-dash stems (2 dashes) ─────────────────────────────────────
        '\u{254C}' => arms(Arms::h(L, L).dashed(2)),
        '\u{254D}' => arms(Arms::h(H, H).dashed(2)),
        '\u{254E}' => arms(Arms::v(L, L).dashed(2)),
        '\u{254F}' => arms(Arms::v(H, H).dashed(2)),

        // ── Double lines ─────────────────────────────────────────────────────
        '\u{2550}' => arms(Arms::h(D, D)),
        '\u{2551}' => arms(Arms::v(D, D)),
        '\u{2552}' => arms(Arms::new(N, L, N, D)),
        '\u{2553}' => arms(Arms::new(N, D, N, L)),
        '\u{2554}' => arms(Arms::new(N, D, N, D)),
        '\u{2555}' => arms(Arms::new(N, L, D, N)),
        '\u{2556}' => arms(Arms::new(N, D, L, N)),
        '\u{2557}' => arms(Arms::new(N, D, D, N)),
        '\u{2558}' => arms(Arms::new(L, N, N, D)),
        '\u{2559}' => arms(Arms::new(D, N, N, L)),
        '\u{255A}' => arms(Arms::new(D, N, N, D)),
        '\u{255B}' => arms(Arms::new(L, N, D, N)),
        '\u{255C}' => arms(Arms::new(D, N, L, N)),
        '\u{255D}' => arms(Arms::new(D, N, D, N)),
        '\u{255E}' => arms(Arms::new(L, L, N, D)),
        '\u{255F}' => arms(Arms::new(D, D, N, L)),
        '\u{2560}' => arms(Arms::new(D, D, N, D)),
        '\u{2561}' => arms(Arms::new(L, L, D, N)),
        '\u{2562}' => arms(Arms::new(D, D, L, N)),
        '\u{2563}' => arms(Arms::new(D, D, D, N)),
        '\u{2564}' => arms(Arms::new(N, L, D, D)),
        '\u{2565}' => arms(Arms::new(N, D, L, L)),
        '\u{2566}' => arms(Arms::new(N, D, D, D)),
        '\u{2567}' => arms(Arms::new(L, N, D, D)),
        '\u{2568}' => arms(Arms::new(D, N, L, L)),
        '\u{2569}' => arms(Arms::new(D, N, D, D)),
        '\u{256A}' => arms(Arms::new(L, L, D, D)),
        '\u{256B}' => arms(Arms::new(D, D, L, L)),
        '\u{256C}' => arms(Arms::new(D, D, D, D)),

        // ── Rounded corners — drawn SQUARE (D2) ──────────────────────────────
        // Identical arms to their square counterparts 250C/2510/2518/2514. They
        // lose their roundness and gain alignment with the stems they join.
        '\u{256D}' => arms(Arms::new(N, L, N, L)),
        '\u{256E}' => arms(Arms::new(N, L, L, N)),
        '\u{256F}' => arms(Arms::new(L, N, L, N)),
        '\u{2570}' => arms(Arms::new(L, N, N, L)),

        // ── Diagonals: the ONLY fallthrough to the font (D2) ─────────────────
        '\u{2571}'..='\u{2573}' => None,

        // ── Single-arm stubs and mixed light/heavy stems ─────────────────────
        '\u{2574}' => arms(Arms::h(L, N)),
        '\u{2575}' => arms(Arms::v(L, N)),
        '\u{2576}' => arms(Arms::h(N, L)),
        '\u{2577}' => arms(Arms::v(N, L)),
        '\u{2578}' => arms(Arms::h(H, N)),
        '\u{2579}' => arms(Arms::v(H, N)),
        '\u{257A}' => arms(Arms::h(N, H)),
        '\u{257B}' => arms(Arms::v(N, H)),
        '\u{257C}' => arms(Arms::h(L, H)),
        '\u{257D}' => arms(Arms::v(L, H)),
        '\u{257E}' => arms(Arms::h(H, L)),
        '\u{257F}' => arms(Arms::v(H, L)),

        // ── Block elements ───────────────────────────────────────────────────
        '\u{2580}' => fill(Fraction::new(0.0, 0.0, 1.0, 0.5)), // ▀ upper half
        // ▁▂▃▄▅▆▇█ — lower n/8, bottom-anchored.
        '\u{2581}'..='\u{2588}' => {
            let n = (ch as u32 - 0x2580) as f32;
            fill(Fraction::new(0.0, 1.0 - n / 8.0, 1.0, 1.0))
        }
        // ▉▊▋▌▍▎▏ — left n/8, from 7/8 down to 1/8.
        '\u{2589}'..='\u{258F}' => {
            let n = (0x2590 - ch as u32) as f32;
            fill(Fraction::new(0.0, 0.0, n / 8.0, 1.0))
        }
        '\u{2590}' => fill(Fraction::new(0.5, 0.0, 1.0, 1.0)), // ▐ right half
        '\u{2591}' => Some(Glyph::Shade(0)),                   // ░
        '\u{2592}' => Some(Glyph::Shade(1)),                   // ▒
        '\u{2593}' => Some(Glyph::Shade(2)),                   // ▓
        '\u{2594}' => fill(Fraction::new(0.0, 0.0, 1.0, 0.125)), // ▔ upper eighth
        '\u{2595}' => fill(Fraction::new(0.875, 0.0, 1.0, 1.0)), // ▕ right eighth

        // ── Quadrants ────────────────────────────────────────────────────────
        // Every edge is a half-cell line, so these tile with ▀▄▌▐ exactly.
        '\u{2596}'..='\u{259F}' => quadrants(ch).map(Glyph::Quadrants),

        _ => None,
    }
}

impl Arms {
    /// This character's arms, dashed into `n` segments (D2).
    const fn dashed(mut self, n: u8) -> Self {
        self.dashes = Some(n);
        self
    }
}

/// Which of the four quadrants a character in `U+2596..=U+259F` covers, in the order
/// upper-left, upper-right, lower-left, lower-right.
///
/// Kept as flags rather than rects so every quadrant edge resolves through the *same*
/// half-cell split lines the half blocks use — which is what makes `▙` tile with `▌`
/// and `▄` (R6.2).
pub fn quadrants(ch: char) -> Option<[bool; 4]> {
    let q = match ch {
        '\u{2596}' => [false, false, true, false], // ▖
        '\u{2597}' => [false, false, false, true], // ▗
        '\u{2598}' => [true, false, false, false], // ▘
        '\u{2599}' => [true, false, true, true],   // ▙
        '\u{259A}' => [true, false, false, true],  // ▚
        '\u{259B}' => [true, true, true, false],   // ▛
        '\u{259C}' => [true, true, false, true],   // ▜
        '\u{259D}' => [false, true, false, false], // ▝
        '\u{259E}' => [false, true, true, false],  // ▞
        '\u{259F}' => [false, true, true, true],   // ▟
        _ => return None,
    };
    Some(q)
}

/// Every rect a character draws in the cell at `(x, y)` of size `(cw, ch)` (D4/D7).
///
/// `bold` applies D3's promotion rule. Returns an empty vec for a character the font
/// draws (the diagonals) or one that resolves to nothing visible.
///
/// Shading characters return no rects — they are a whole-cell quad at a coverage
/// level, which the caller emits from [`Glyph::Shade`] because it needs a different
/// atlas entry. Callers should match on [`classify`] rather than assuming rects.
pub fn rects(ch: char, x: f32, y: f32, cw: f32, chh: f32, bold: bool) -> Vec<Rect> {
    match classify(ch) {
        Some(Glyph::Box(arms)) => box_rects(arms.with_bold(bold), x, y, cw, chh),
        Some(Glyph::Fill(f)) => {
            let r = floor_to_pixel(
                Rect::new(
                    snap(x, cw, f.x0),
                    snap(y, chh, f.y0),
                    snap(x, cw, f.x1),
                    snap(y, chh, f.y1),
                ),
                x,
                y,
                cw,
                chh,
            );
            if r.is_visible() {
                vec![r]
            } else {
                Vec::new()
            }
        }
        Some(Glyph::Quadrants(q)) => {
            let mx = snap(x, cw, 0.5);
            let my = snap(y, chh, 0.5);
            let (x1, y1) = (x + cw, y + chh);
            // The four corner rects, built from the two shared split lines — so a
            // quadrant edge is bit-identical to the half-block edge beside it.
            let cells = [
                Rect::new(x, y, mx, my),
                Rect::new(mx, y, x1, my),
                Rect::new(x, my, mx, y1),
                Rect::new(mx, my, x1, y1),
            ];
            q.iter()
                .zip(cells)
                .filter(|(on, _)| **on)
                .map(|(_, r)| floor_to_pixel(r, x, y, cw, chh))
                .filter(Rect::is_visible)
                .collect()
        }
        Some(Glyph::Shade(_)) | None => Vec::new(),
    }
}

/// Grow a rect that rounded away to nothing back to one pixel, kept inside its cell.
///
/// At a 1px cell an eighth-block's fraction (`▁` is `0.875..1.0`) rounds to a
/// zero-height rect and the character silently disappears. Every other degenerate
/// case in this module floors to one pixel — stem thickness, the dash pattern — and
/// a fill should too: rendering `▁` as the single available pixel is coarse, but a
/// vanishing character is a defect. Inert whenever the rect is already ≥1px.
fn floor_to_pixel(r: Rect, x: f32, y: f32, cw: f32, chh: f32) -> Rect {
    let (mut x0, mut x1, mut y0, mut y1) = (r.x0, r.x1, r.y0, r.y1);
    if x1 - x0 < 1.0 {
        x0 = x0.min(x + cw - 1.0).max(x);
        x1 = x0 + 1.0;
    }
    if y1 - y0 < 1.0 {
        y0 = y0.min(y + chh - 1.0).max(y);
        y1 = y0 + 1.0;
    }
    Rect::new(x0, y0, x1, y1)
}

/// Turn arms into rects, one band per arm (D4).
///
/// Each arm's band comes from [`band`] with *that arm's own* thickness — not one
/// band per axis — because `╀ ╂ ╼ ╾` carry two weights on a single axis. An arm
/// runs from its cell edge to the far side of the *crossing* band, so the stroke is
/// continuous through the junction and no row is left unpainted (R4.1).
fn box_rects(arms: Arms, x: f32, y: f32, cw: f32, chh: f32) -> Vec<Rect> {
    let mut out = Vec::new();

    // A dashed character is a plain stem cut into segments along its long axis.
    // Every dashed codepoint in the range is a bare stem — never a junction — so
    // this is a whole-cell run, not a per-arm one.
    if let Some(n) = arms.dashes {
        let horiz = arms.left.is_some() || arms.right.is_some();
        let weight = pick(pick(arms.left, arms.right), pick(arms.up, arms.down));
        let t = weight.thickness(chh);
        let (b0, b1) = if horiz {
            band(y, chh, t)
        } else {
            band(x, cw, t)
        };
        let (origin, extent) = if horiz { (x, cw) } else { (y, chh) };
        return dash_segments(origin, extent, n)
            .into_iter()
            .map(|(s, e)| {
                if horiz {
                    Rect::new(s, b0, e, b1)
                } else {
                    Rect::new(b0, s, b1, e)
                }
            })
            .filter(Rect::is_visible)
            .collect();
    }

    // The crossing extent each arm must reach to. A horizontal arm stops at the far
    // side of the vertical stem's band, and vice versa. With no crossing arm it
    // stops at the cell center line, which is what makes the stubs `╴╵╶╷` half-length.
    let vertical = pick(arms.up, arms.down);
    let horizontal = pick(arms.left, arms.right);
    let (vx0, vx1) = span(x, cw, vertical, chh);
    let (hy0, hy1) = span(y, chh, horizontal, chh);

    // Opposing arms of equal weight are one rect, not two. `─` and `│` are the most
    // common characters in box art by a wide margin, so emitting a single quad for
    // them halves the instance count — and the two halves abut exactly anyway, so
    // nothing about the result changes. Mixed weights (`╾`, `╿`) can't merge: each
    // side has its own band.
    let merge_h = arms.left == arms.right && arms.left.is_some();
    let merge_v = arms.up == arms.down && arms.up.is_some();

    let mut plan: Vec<(Weight, Dir)> = Vec::new();
    if merge_h {
        plan.push((arms.left, Dir::Horizontal));
    } else {
        plan.push((arms.left, Dir::Left));
        plan.push((arms.right, Dir::Right));
    }
    if merge_v {
        plan.push((arms.up, Dir::Vertical));
    } else {
        plan.push((arms.up, Dir::Up));
        plan.push((arms.down, Dir::Down));
    }

    for (weight, dir) in plan {
        if !weight.is_some() {
            continue;
        }
        let horiz = matches!(dir, Dir::Left | Dir::Right | Dir::Horizontal);
        // The arm's own band, on the axis it is *thick* across.
        let bands: Vec<(f32, f32)> = if weight == Weight::Double {
            let light = light_thickness(chh);
            let ((a0, a1), (b0, b1)) = if horiz {
                double_bands(y, chh, light)
            } else {
                double_bands(x, cw, light)
            };
            vec![(a0, a1), (b0, b1)]
        } else {
            let t = weight.thickness(chh);
            vec![if horiz {
                band(y, chh, t)
            } else {
                band(x, cw, t)
            }]
        };

        for (b0, b1) in bands {
            let r = match dir {
                Dir::Left => Rect::new(x, b0, vx1, b1),
                Dir::Right => Rect::new(vx0, b0, x + cw, b1),
                Dir::Up => Rect::new(b0, y, b1, hy1),
                Dir::Down => Rect::new(b0, hy0, b1, y + chh),
                Dir::Horizontal => Rect::new(x, b0, x + cw, b1),
                Dir::Vertical => Rect::new(b0, y, b1, y + chh),
            };
            if r.is_visible() {
                out.push(r);
            }
        }
    }
    out
}

enum Dir {
    Up,
    Down,
    Left,
    Right,
    /// Both horizontal arms at once — the merged form when they share a weight.
    Horizontal,
    /// Both vertical arms at once.
    Vertical,
}

/// The dash segments across `[origin, origin + extent]` for a dashed stem (D2).
///
/// Returns `(start, end)` pairs along the stem's *long* axis. Two guards make this
/// hold up at real cell sizes, and both are load-bearing:
///
/// 1. **Count bound.** `n_eff = min(n, ⌊extent/2⌋)`, because each dash needs its own
///    pixel *and* a trailing gap pixel. The trailing gap is not padding — without it
///    the last dash of one cell butts against the first dash of the next and they
///    merge across the boundary.
/// 2. **End clamp.** `start` and `end` round independently, so `end_i` can round up
///    *onto* `start_{i+1}` and silently merge two dashes — which the count bound
///    cannot prevent. At `extent = 7, n = 4` the unclamped ends are `2,4,6` against
///    starts `0,2,5`, so dashes 0 and 1 touch at x=2.
///
/// At `n_eff == 1` (extent ≤ 3) the pattern degrades to one **full-extent** stem.
/// That is deliberate: feeding `n_eff = 1` through the formula would give
/// `[0, ⌊2·extent/3⌋]`, a stub that stops short of the cell edge and leaves an
/// inter-cell gap — the opposite of the intended degradation.
fn dash_segments(origin: f32, extent: f32, n: u8) -> Vec<(f32, f32)> {
    let n_eff = (n as u32)
        .min((extent / 2.0).floor().max(0.0) as u32)
        .max(1);
    if n_eff == 1 {
        return vec![(origin, origin + extent)];
    }
    let n_f = n_eff as f32;
    let starts: Vec<f32> = (0..n_eff)
        .map(|i| snap(origin, extent, i as f32 / n_f))
        .collect();
    (0..n_eff as usize)
        .map(|i| {
            let raw = snap(origin, extent, i as f32 / n_f + 2.0 / (3.0 * n_f));
            // Clamp against the next dash's start (or the cell edge for the last),
            // leaving at least one pixel of gap.
            let limit = starts.get(i + 1).copied().unwrap_or(origin + extent) - 1.0;
            (starts[i], raw.min(limit))
        })
        .filter(|(s, e)| e - s >= 1.0)
        .collect()
}

/// The heavier of two opposing arms — the one whose band a crossing arm must reach
/// across so the junction has no unpainted row.
fn pick(a: Weight, b: Weight) -> Weight {
    let rank = |w: Weight| match w {
        Weight::None => 0,
        Weight::Light => 1,
        Weight::Heavy => 2,
        Weight::Double => 3,
    };
    if rank(a) >= rank(b) {
        a
    } else {
        b
    }
}

/// The `(near, far)` edges of the crossing stem's band on one axis.
///
/// With no crossing arm the "band" collapses to the cell's center line, so a stub
/// like `╴` reaches the middle and stops (R4.4) instead of crossing the whole cell.
fn span(origin: f32, extent: f32, crossing: Weight, chh: f32) -> (f32, f32) {
    if !crossing.is_some() {
        // The near/far edges are pulled apart just enough that a stub keeps a
        // pixel from EITHER side. At a 1px cell `snap(.., 0.5)` rounds to 1, so a
        // right-facing stub `╶` would start at the cell's far edge and vanish; a
        // left-facing `╴` has the mirror problem at the near edge. Clamping each
        // edge independently keeps both visible and is inert once the cell is ≥2px,
        // where the midpoint already sits strictly inside.
        let mid = snap(origin, extent, 0.5);
        let near = mid.min(origin + extent - 1.0).max(origin);
        let far = mid.max(origin + 1.0).min(origin + extent);
        return (near, far);
    }
    band(origin, extent, crossing.thickness(chh))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every extent the spec's invariants are stated over (`1..=64`), plus 128 for
    /// headroom. Swept exhaustively rather than sampled: R3.2/R3.3 claim to hold for
    /// *every* cell extent, and sampling would leave the parity cases untested.
    fn extents() -> Vec<f32> {
        (1..=64).map(|e| e as f32).chain([128.0]).collect()
    }

    #[test]
    fn light_thickness_at_the_reference_cells() {
        // R2.1: 14pt @2x -> cell (17, 34).
        assert_eq!(light_thickness(34.0), 3.0);
        assert_eq!(heavy_thickness(34.0), 6.0);
        // R2.2: 14pt @1x -> cell (8, 17). round(17 * 0.075) == round(1.275) == 1.
        assert_eq!(light_thickness(17.0), 1.0);
        assert_eq!(heavy_thickness(17.0), 2.0);
    }

    #[test]
    fn thickness_never_reaches_zero() {
        // R2.3: round(4 * 0.075) == 0 without the floor, which would emit a
        // zero-area quad — i.e. an invisible stem — at small font sizes.
        assert_eq!(light_thickness(4.0), 1.0);
        assert_eq!(light_thickness(1.0), 1.0);
        assert_eq!(light_thickness(0.0), 1.0);
        for e in extents() {
            assert!(light_thickness(e) >= 1.0, "extent {e}");
        }
    }

    #[test]
    fn band_is_cell_independent() {
        // R3.1: the offset from origin depends only on (extent, t), so two adjacent
        // cells produce bands that abut exactly — no gap, no overlap.
        for e in extents() {
            for t in 1..=(e as u32).max(1) {
                let t = t as f32;
                if t > e {
                    continue;
                }
                let (s0, e0) = band(0.0, e, t);
                let (s1, e1) = band(e, e, t);
                assert_eq!(s1 - s0, e, "extent {e} t {t}: offset must be constant");
                assert_eq!(e1 - e0, e, "extent {e} t {t}");
            }
        }
    }

    #[test]
    fn adjacent_cells_share_an_exact_edge() {
        // R0.1/R0.3: cell (17, 34) — three horizontally adjacent stems tile as
        // [51,68] [68,85] [85,102] with the identical y-band.
        let (cw, ch) = (17.0f32, 34.0f32);
        let t = light_thickness(ch);
        // The painted x-extent of `─` in each cell — from the real rect generator,
        // not from `band(o, cw, cw)`, which is trivially `(o, o+cw)` and would pass
        // no matter what the geometry did.
        let spans: Vec<(f32, f32)> = (3..6)
            .map(|col| {
                let r = rects('\u{2500}', col as f32 * cw, 0.0, cw, ch, false);
                assert_eq!(r.len(), 1, "a plain stem is one rect");
                (r[0].x0, r[0].x1)
            })
            .collect();
        assert_eq!(spans[0], (51.0, 68.0));
        assert_eq!(spans[1], (68.0, 85.0));
        assert_eq!(spans[2], (85.0, 102.0));
        // Abutting exactly: each cell starts where the previous ended.
        assert_eq!(spans[0].1, spans[1].0);
        assert_eq!(spans[1].1, spans[2].0);
        // And every cell shares one y band — the same (extent, t) in each.
        let ys: Vec<(f32, f32)> = (3..6)
            .map(|col| {
                let r = rects('\u{2500}', col as f32 * cw, 0.0, cw, ch, false);
                (r[0].y0, r[0].y1)
            })
            .collect();
        assert!(ys.windows(2).all(|w| w[0] == w[1]), "{ys:?}");
        assert_eq!(ys[0], band(0.0, ch, t), "on the light stem band");
    }

    #[test]
    fn containment_light_inside_heavy() {
        // R3.2: the invariant that makes `╾` widen into a clean T. Light and heavy
        // are NOT concentric (odd vs even width on an integer lattice), so
        // containment — not concentricity — is what has to hold.
        // Swept to `L ≤ e`, not `L ≤ e/2`: the spec states containment
        // **unrestricted**, including the clamped region where `2L > extent` and
        // `band` truncates the heavy stem to the cell. (Concentricity is the one
        // that genuinely cannot survive that clamp — see its own test's domain.)
        for e in extents() {
            for l in 1..=(e as u32).max(1) {
                let l = l as f32;
                if l > e {
                    continue;
                }
                let (sl, el) = band(0.0, e, l);
                let (sh, eh) = band(0.0, e, 2.0 * l);
                assert!(sh <= sl, "extent {e} light {l}: heavy must start no later");
                assert!(el <= eh, "extent {e} light {l}: heavy must end no earlier");
            }
        }
    }

    #[test]
    fn the_band_clamp_only_bites_at_degenerate_cell_sizes() {
        // The clamp inside `band` exists to stop a heavy stem starting at a negative
        // offset and painting into the neighbouring cell. It changes geometry only
        // where `t > extent`. Pin that it is inert at every cell size a real font
        // setting can produce, so it can never silently reshape normal box art.
        // The bound that matters is 3L, NOT 2L: `band` is reached with `t = 3L`
        // through `double_span` (a double stem's whole span) and
        // `Weight::Double::thickness`, so a test that only checked `2L ≤ cell`
        // would assert something weaker than its own name.
        for pt10 in 80..=320u32 {
            let pt = pt10 as f32 / 10.0;
            for sf in [1.0f32, 2.0] {
                let px = pt * sf;
                let (cw, chh) = ((px * 0.6).round().max(1.0), (px * 1.2).round().max(1.0));
                let widest = Weight::Double.thickness(chh);
                assert!(
                    widest <= cw.min(chh),
                    "{pt}pt @{sf}x -> cell ({cw},{chh}): widest stem {widest} exceeds \
                     the cell, so the clamp would alter real geometry"
                );
            }
        }
    }

    #[test]
    fn concentricity_light_and_double() {
        // R3.3: exact, unlike containment — `L` and `3L` share parity, so the light
        // band and the double span have the same center. `╪` crossings depend on it.
        for e in extents() {
            let max_light = (e / 3.0).floor() as u32;
            for l in 1..=max_light {
                let l = l as f32;
                let (sl, el) = band(0.0, e, l);
                let (sd, ed) = double_span(0.0, e, l);
                assert_eq!(
                    sl + el,
                    sd + ed,
                    "extent {e} light {l}: centers must coincide exactly"
                );
            }
        }
    }

    #[test]
    fn double_bands_are_two_rules_with_a_light_gap() {
        // R4.5: `═` at cell (17, 34) with light 3 -> two 3px rules, 3px apart,
        // inside a 9px span.
        let ((a0, a1), (b0, b1)) = double_bands(0.0, 34.0, 3.0);
        assert_eq!(a1 - a0, 3.0);
        assert_eq!(b1 - b0, 3.0);
        assert_eq!(b0 - a1, 3.0, "the gap is one light unit");
        assert_eq!(b1 - a0, 9.0, "the whole span is 3 * light");
    }

    #[test]
    fn snap_produces_exact_complements() {
        // R3.4: `▀` ends where `▄` starts because both read the same snap value —
        // checked at an odd extent, where an independent floor/ceil would disagree.
        for e in extents() {
            let mid = snap(0.0, e, 0.5);
            assert!(mid >= 0.0 && mid <= e, "extent {e}: split inside the cell");
            // The real content: the split line is the SAME value both halves read,
            // and it is a whole pixel. (Asserting `mid + (e - mid) == e` would be
            // algebraically `e == e` and could never fail.)
            assert_eq!(mid, mid.floor(), "extent {e}: the split is integral");
        }
        // The odd case explicitly: 17 splits 8/9, not 8.5/8.5.
        assert_eq!(snap(0.0, 17.0, 0.5), 9.0);
        assert_eq!(snap(0.0, 34.0, 0.5), 17.0);
    }

    #[test]
    fn snap_eighths_are_monotonic_and_reach_the_full_cell() {
        // R6.3: `▁▂▃▄▅▆▇█` must never go backwards, and `█` must fill the cell.
        for e in extents() {
            let mut prev = 0.0f32;
            for n in 1..=8u32 {
                let v = snap(0.0, e, n as f32 / 8.0);
                assert!(v >= prev, "extent {e} eighth {n}: monotonic");
                prev = v;
            }
            assert_eq!(
                snap(0.0, e, 1.0),
                e,
                "extent {e}: full block spans the cell"
            );
        }
    }

    /// The reference cell: 14pt @2x.
    const CW: f32 = 17.0;
    const CH: f32 = 34.0;

    #[test]
    fn classification_is_total_over_the_range() {
        // R1.1: every codepoint in 2500..=259F is decided, and EXACTLY the three
        // diagonals fall through to the font.
        for cp in 0x2500u32..=0x259Fu32 {
            let ch = char::from_u32(cp).expect("valid scalar");
            let got = classify(ch);
            if (0x2571..=0x2573).contains(&cp) {
                assert!(got.is_none(), "U+{cp:04X} must reach the font");
            } else {
                assert!(got.is_some(), "U+{cp:04X} must be procedural");
            }
        }
    }

    #[test]
    fn exactly_157_of_160_codepoints_are_procedural() {
        // The totality test above proves each codepoint is decided correctly; this
        // pins the resulting *counts*, so silently dropping a character out of the
        // intercepted set — or quietly widening it — fails loudly rather than
        // shifting the boundary D2 fixed.
        let (mut procedural, mut font) = (0, 0);
        for cp in 0x2500u32..=0x259Fu32 {
            let ch = char::from_u32(cp).expect("valid scalar");
            match classify(ch) {
                Some(_) => procedural += 1,
                None => font += 1,
            }
        }
        assert_eq!(procedural + font, 160, "the range is 160 codepoints");
        assert_eq!(font, 3, "only ╱╲╳ may reach the font");
        assert_eq!(procedural, 157);
    }

    #[test]
    fn a_plain_stem_is_a_single_rect() {
        // Opposing arms of equal weight merge, so `─` and `│` — by far the most
        // common characters in box art — cost one quad, not two. Pinning this keeps
        // the merge from silently regressing into per-arm emission.
        assert_eq!(rects('\u{2500}', 0.0, 0.0, CW, CH, false).len(), 1, "─");
        assert_eq!(rects('\u{2502}', 0.0, 0.0, CW, CH, false).len(), 1, "│");
        assert_eq!(rects('\u{2501}', 0.0, 0.0, CW, CH, false).len(), 1, "━");
        assert_eq!(
            rects('\u{253C}', 0.0, 0.0, CW, CH, false).len(),
            2,
            "┼ is 2"
        );
        // A double stem is two rules by definition, per axis.
        assert_eq!(rects('\u{2550}', 0.0, 0.0, CW, CH, false).len(), 2, "═");
        // Mixed weights cannot merge — each side keeps its own band.
        assert_eq!(rects('\u{257E}', 0.0, 0.0, CW, CH, false).len(), 2, "╾");
    }

    #[test]
    fn characters_outside_the_range_are_not_intercepted() {
        for ch in ['a', ' ', '한', '\u{24FF}', '\u{25A0}', '\u{1FB00}'] {
            assert!(classify(ch).is_none(), "{ch:?} is out of range");
        }
    }

    #[test]
    fn rounded_corners_reuse_their_square_arms() {
        // R1.4: ╭╮╯╰ are the same Arms as ┌┐┘└ — square, by decision (D2). Equal
        // values, not merely "similar", so they cannot drift apart later.
        for (round, square) in [
            ('\u{256D}', '\u{250C}'),
            ('\u{256E}', '\u{2510}'),
            ('\u{256F}', '\u{2518}'),
            ('\u{2570}', '\u{2514}'),
        ] {
            assert_eq!(
                classify(round),
                classify(square),
                "{round:?} must render as {square:?}"
            );
        }
    }

    #[test]
    fn a_plain_stem_spans_the_whole_cell() {
        // R4.3: `─` reaches both cell edges, which is what lets neighbours abut.
        let r = rects('\u{2500}', 0.0, 0.0, CW, CH, false);
        let x0 = r.iter().map(|r| r.x0).fold(f32::MAX, f32::min);
        let x1 = r.iter().map(|r| r.x1).fold(f32::MIN, f32::max);
        assert_eq!((x0, x1), (0.0, CW));
        // `│` likewise on the other axis.
        let v = rects('\u{2502}', 0.0, 0.0, CW, CH, false);
        let y0 = v.iter().map(|r| r.y0).fold(f32::MAX, f32::min);
        let y1 = v.iter().map(|r| r.y1).fold(f32::MIN, f32::max);
        assert_eq!((y0, y1), (0.0, CH));
    }

    #[test]
    fn a_stub_stops_at_the_middle() {
        // R4.4: `╴` is a LEFT arm only — it must not cross to the right edge.
        let r = rects('\u{2574}', 0.0, 0.0, CW, CH, false);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].x0, 0.0);
        assert_eq!(r[0].x1, snap(0.0, CW, 0.5));
        assert!(r[0].x1 < CW, "a stub does not span the cell");
    }

    #[test]
    fn a_t_junction_leaves_no_unpainted_row() {
        // R4.1: `┴` — the up arm must reach the FAR side of the crossbar's band, so
        // the union has no gap between stem and bar.
        let r = rects('\u{2534}', 0.0, 0.0, CW, CH, false);
        let t = light_thickness(CH);
        let (by0, by1) = band(0.0, CH, t);
        let up = r
            .iter()
            .find(|r| r.height() > r.width())
            .expect("a vertical arm");
        assert_eq!(up.y0, 0.0, "the stem starts at the cell top");
        assert_eq!(up.y1, by1, "and reaches the crossbar's far edge");
        // The crossbar spans the full width on the same band.
        let bar_x0 = r.iter().map(|r| r.x0).fold(f32::MAX, f32::min);
        let bar_x1 = r.iter().map(|r| r.x1).fold(f32::MIN, f32::max);
        assert_eq!((bar_x0, bar_x1), (0.0, CW));
        assert!(by0 < by1);
    }

    #[test]
    fn mixed_weight_stems_keep_the_light_arm_inside_the_heavy_one() {
        // `╼╽╾╿` (U+257C..=U+257F) carry two weights on ONE axis, so they are the
        // characters the connectivity sweep structurally cannot reach. Their
        // correctness property is containment: the light arm's band must sit inside
        // the heavy arm's, or the two meet with a step instead of a clean widening.
        // Swept across cell sizes because the parity of `light` changes with them.
        for (cw, chh) in [(17.0f32, 34.0f32), (8.0, 17.0), (7.0, 33.0), (29.0, 58.0)] {
            for cp in 0x257Cu32..=0x257Fu32 {
                let ch = char::from_u32(cp).expect("valid scalar");
                let r = rects(ch, 0.0, 0.0, cw, chh, false);
                assert_eq!(r.len(), 2, "U+{cp:04X} at ({cw},{chh}): one rect per arm");
                let horizontal = cp == 0x257C || cp == 0x257E;
                // Compare the two arms across the axis they are thick on.
                let (a, b) = (r[0], r[1]);
                let (thin, thick) = if horizontal {
                    if a.height() <= b.height() {
                        (a, b)
                    } else {
                        (b, a)
                    }
                } else if a.width() <= b.width() {
                    (a, b)
                } else {
                    (b, a)
                };
                assert!(
                    thick.height().max(thick.width()) > 0.0,
                    "U+{cp:04X}: degenerate"
                );
                if horizontal {
                    assert!(
                        thick.y0 <= thin.y0 && thin.y1 <= thick.y1,
                        "U+{cp:04X} at ({cw},{chh}): light {thin:?} not inside heavy {thick:?}"
                    );
                } else {
                    assert!(
                        thick.x0 <= thin.x0 && thin.x1 <= thick.x1,
                        "U+{cp:04X} at ({cw},{chh}): light {thin:?} not inside heavy {thick:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn mixed_weight_arms_get_their_own_bands() {
        // R4.2: `╾` (U+257E) is HEAVY left + LIGHT right. Two bands, and the light
        // one must sit inside the heavy one so the junction widens cleanly (R3.2).
        let r = rects('\u{257E}', 0.0, 0.0, CW, CH, false);
        assert_eq!(r.len(), 2, "one rect per arm, not one merged band");
        let light = r.iter().find(|r| r.height() == 3.0).expect("light arm");
        let heavy = r.iter().find(|r| r.height() == 6.0).expect("heavy arm");
        assert!(
            heavy.y0 <= light.y0 && light.y1 <= heavy.y1,
            "light {light:?} must be contained in heavy {heavy:?}"
        );
    }

    #[test]
    fn bold_promotes_a_uniformly_light_character() {
        // R2.4: `┼` is all-light, so BOLD draws every arm heavy.
        let plain = rects('\u{253C}', 0.0, 0.0, CW, CH, false);
        let bold = rects('\u{253C}', 0.0, 0.0, CW, CH, true);
        let thin = plain
            .iter()
            .map(|r| r.height().min(r.width()))
            .fold(0.0, f32::max);
        let thick = bold
            .iter()
            .map(|r| r.height().min(r.width()))
            .fold(0.0, f32::max);
        assert_eq!(thin, 3.0);
        assert_eq!(thick, 6.0, "BOLD promotes light to heavy");
    }

    #[test]
    fn bold_leaves_a_mixed_weight_character_alone() {
        // R2.5: `╀` (U+2540) is light horizontal + heavy up. Promoting its light
        // arms would render it as `╋` and destroy the distinction it encodes.
        let arms = match classify('\u{2540}') {
            Some(Glyph::Box(a)) => a,
            other => panic!("expected box arms, got {other:?}"),
        };
        assert_eq!(arms.with_bold(true), arms, "mixed weights are not promoted");
        assert_eq!(
            rects('\u{2540}', 0.0, 0.0, CW, CH, true),
            rects('\u{2540}', 0.0, 0.0, CW, CH, false),
        );
        // And it is genuinely mixed, so the test is not vacuous.
        assert_eq!(arms.up, Weight::Heavy);
        assert_eq!(arms.left, Weight::Light);
    }

    #[test]
    fn bold_is_a_noop_on_blocks() {
        // R2.6: a block has no stems to thicken.
        for ch in ['\u{2588}', '\u{2580}', '\u{2596}'] {
            assert_eq!(
                rects(ch, 0.0, 0.0, CW, CH, true),
                rects(ch, 0.0, 0.0, CW, CH, false),
                "{ch:?}"
            );
        }
    }

    /// Rasterize a character's rects into a coverage grid — the ground truth for
    /// "what pixels does this actually paint", independent of how many rects it
    /// took to say it.
    fn painted(ch: char, cw: f32, chh: f32, bold: bool) -> Vec<bool> {
        let (w, h) = (cw as usize, chh as usize);
        let mut grid = vec![false; w * h];
        for r in rects(ch, 0.0, 0.0, cw, chh, bold) {
            for py in (r.y0 as usize)..(r.y1 as usize).min(h) {
                for px in (r.x0 as usize)..(r.x1 as usize).min(w) {
                    grid[py * w + px] = true;
                }
            }
        }
        grid
    }

    #[test]
    fn merging_opposing_arms_paints_the_same_pixels() {
        // The arm-merge is an instance-count optimization, not a geometry change.
        // Rect COUNTS are pinned elsewhere; this pins the thing that actually
        // matters — that merging paints pixel-for-pixel what the per-arm form did.
        //
        // The per-arm form is reconstructed here by drawing each opposing arm as its
        // own character where one exists: `╴`+`╶` is `─` split in two, `╵`+`╷` is
        // `│`, and the heavy stubs likewise. If merging ever shrank coverage, the
        // union of the two halves would differ from the merged whole.
        for (whole, first, second) in [
            ('\u{2500}', '\u{2574}', '\u{2576}'), // ─  =  ╴ + ╶
            ('\u{2502}', '\u{2575}', '\u{2577}'), // │  =  ╵ + ╷
            ('\u{2501}', '\u{2578}', '\u{257A}'), // ━  =  ╸ + ╺
            ('\u{2503}', '\u{2579}', '\u{257B}'), // ┃  =  ╹ + ╻
        ] {
            for (cw, chh) in [(17.0f32, 34.0f32), (8.0, 17.0), (7.0, 33.0), (4.0, 9.0)] {
                let merged = painted(whole, cw, chh, false);
                let a = painted(first, cw, chh, false);
                let b = painted(second, cw, chh, false);
                let union: Vec<bool> = a.iter().zip(&b).map(|(x, y)| *x || *y).collect();
                assert_eq!(
                    merged, union,
                    "{whole:?} at ({cw},{chh}): merged coverage != {first:?} ∪ {second:?}"
                );
                assert!(merged.iter().any(|p| *p), "{whole:?} painted nothing");
            }
        }
    }

    #[test]
    fn a_junction_is_pixel_connected() {
        // R4.1 as coverage rather than as edge arithmetic: every junction character
        // must paint ONE connected region. A notch or an unpainted row between stem
        // and crossbar would split it in two, and a rect-count check would miss that.
        let (cw, chh) = (17.0f32, 34.0f32);
        let (w, h) = (cw as usize, chh as usize);
        // Bounded at 256C on purpose. `2574..257F` are single-axis characters —
        // stubs and mixed light/heavy stems — so `has_h && has_v` is false for every
        // one of them and extending the range here would add no coverage at all.
        // Their real property is one-axis containment, covered by
        // `mixed_weight_stems_keep_the_light_arm_inside_the_heavy_one`.
        for cp in 0x2500u32..=0x256Cu32 {
            let ch = char::from_u32(cp).expect("valid scalar");
            let Some(Glyph::Box(arms)) = classify(ch) else {
                continue;
            };
            // Two shapes are disconnected BY DESIGN and are not junctions:
            //   * dashed stems — the gaps are the whole point;
            //   * a single-axis double stem like `═`, which is two parallel rules
            //     with a light gap between them and nothing crossing to join them.
            // A junction is a character with at least one arm on EACH axis; those
            // must paint one connected region, because a crossing stem always
            // spans the far side of the band it crosses.
            let has_h = arms.left.is_some() || arms.right.is_some();
            let has_v = arms.up.is_some() || arms.down.is_some();
            if arms.dashes.is_some() || !(has_h && has_v) {
                continue;
            }
            let grid = painted(ch, cw, chh, false);
            let start = match grid.iter().position(|p| *p) {
                Some(s) => s,
                None => panic!("U+{cp:04X} painted nothing"),
            };
            // Flood fill from the first painted pixel.
            let mut seen = vec![false; grid.len()];
            let mut stack = vec![start];
            seen[start] = true;
            while let Some(i) = stack.pop() {
                let (x, y) = (i % w, i / w);
                let push = |nx: usize, ny: usize, stack: &mut Vec<usize>, seen: &mut Vec<bool>| {
                    let j = ny * w + nx;
                    if grid[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                };
                if x > 0 {
                    push(x - 1, y, &mut stack, &mut seen);
                }
                if x + 1 < w {
                    push(x + 1, y, &mut stack, &mut seen);
                }
                if y > 0 {
                    push(x, y - 1, &mut stack, &mut seen);
                }
                if y + 1 < h {
                    push(x, y + 1, &mut stack, &mut seen);
                }
            }
            let total = grid.iter().filter(|p| **p).count();
            let reached = seen.iter().filter(|p| **p).count();
            assert_eq!(
                reached, total,
                "U+{cp:04X} paints {total} px but only {reached} are connected — \
                 there is a notch or an unpainted row at the junction"
            );
        }
    }

    #[test]
    fn every_procedural_character_emits_something_visible() {
        // R3.5: a classification that produces no rect would be a silently blank
        // cell — the failure mode this whole feature exists to fix. Swept across the
        // reachable cell-size range, not just the reference cell: the degenerate
        // small sizes are exactly where a rect can round away to nothing.
        for (cw, chh) in [
            (1.0f32, 1.0f32),
            (2.0, 3.0),
            (4.0, 9.0),
            (7.0, 15.0),
            (8.0, 17.0),
            (17.0, 34.0),
            (29.0, 58.0),
            (64.0, 128.0),
        ] {
            for cp in 0x2500u32..=0x259Fu32 {
                let ch = char::from_u32(cp).expect("valid scalar");
                match classify(ch) {
                    None => continue,                  // the diagonals
                    Some(Glyph::Shade(_)) => continue, // a whole-cell quad, not rects
                    Some(_) => {
                        let r = rects(ch, 0.0, 0.0, cw, chh, false);
                        assert!(
                            !r.is_empty(),
                            "U+{cp:04X} produced no rects at ({cw},{chh})"
                        );
                        assert!(
                            r.iter().all(Rect::is_visible),
                            "U+{cp:04X} degenerate at ({cw},{chh}): {r:?}"
                        );
                        // Nothing may escape its own cell.
                        for rect in &r {
                            assert!(
                                rect.x0 >= 0.0 && rect.y0 >= 0.0 && rect.x1 <= cw && rect.y1 <= chh,
                                "U+{cp:04X} at ({cw},{chh}) escapes its cell: {rect:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_closed_frame_has_an_unbroken_outline() {
        // R0.2, the goal stated as a whole picture rather than per-cell edges: draw
        //   ┌─┐
        //   │ │
        //   └─┘
        // across a 3×3 cell grid and require the painted outline to be ONE connected
        // region. A gap at any cell join — the defect this feature exists to remove —
        // splits it into two or more.
        let (cw, chh) = (17.0f32, 34.0f32);
        let (gw, gh) = (3usize * cw as usize, 3usize * chh as usize);
        let frame = [
            ['\u{250C}', '\u{2500}', '\u{2510}'],
            ['\u{2502}', ' ', '\u{2502}'],
            ['\u{2514}', '\u{2500}', '\u{2518}'],
        ];
        let mut grid = vec![false; gw * gh];
        for (row, line) in frame.iter().enumerate() {
            for (col, &ch) in line.iter().enumerate() {
                let (ox, oy) = (col as f32 * cw, row as f32 * chh);
                for r in rects(ch, ox, oy, cw, chh, false) {
                    for py in (r.y0 as usize)..(r.y1 as usize).min(gh) {
                        for px in (r.x0 as usize)..(r.x1 as usize).min(gw) {
                            grid[py * gw + px] = true;
                        }
                    }
                }
            }
        }
        let total = grid.iter().filter(|p| **p).count();
        assert!(total > 0, "the frame painted nothing");

        let start = grid.iter().position(|p| *p).expect("a painted pixel");
        let mut seen = vec![false; grid.len()];
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(i) = stack.pop() {
            let (x, y) = (i % gw, i / gw);
            let push = |nx: usize, ny: usize, st: &mut Vec<usize>, sn: &mut Vec<bool>| {
                let j = ny * gw + nx;
                if grid[j] && !sn[j] {
                    sn[j] = true;
                    st.push(j);
                }
            };
            if x > 0 {
                push(x - 1, y, &mut stack, &mut seen);
            }
            if x + 1 < gw {
                push(x + 1, y, &mut stack, &mut seen);
            }
            if y > 0 {
                push(x, y - 1, &mut stack, &mut seen);
            }
            if y + 1 < gh {
                push(x, y + 1, &mut stack, &mut seen);
            }
        }
        let reached = seen.iter().filter(|p| **p).count();
        assert_eq!(
            reached, total,
            "the frame outline is broken: {total} px painted, only {reached} connected"
        );
        // And the interior really is hollow — otherwise "connected" would be trivial.
        let centre = (chh as usize + chh as usize / 2) * gw + (cw as usize + cw as usize / 2);
        assert!(!grid[centre], "the frame interior must be empty");
    }

    #[test]
    fn dash_counts_come_from_the_codepoint() {
        // R5.1: 2 for 254C-254F, 3 for 2504-2507, 4 for 2508-250B — and the
        // vertical members of each range carry the same count as the horizontal.
        let dashes = |ch: char| match classify(ch) {
            Some(Glyph::Box(a)) => a.dashes,
            _ => None,
        };
        assert_eq!(dashes('\u{254C}'), Some(2)); // ╌
        assert_eq!(dashes('\u{254E}'), Some(2)); // ╎ vertical
        assert_eq!(dashes('\u{2504}'), Some(3)); // ┄
        assert_eq!(dashes('\u{2506}'), Some(3)); // ┆ vertical
        assert_eq!(dashes('\u{2508}'), Some(4)); // ┈
        assert_eq!(dashes('\u{250A}'), Some(4)); // ┊ vertical
        assert_eq!(dashes('\u{2500}'), None, "a solid stem is not dashed");
    }

    /// Every internal gap in a dash run, as `(start, end)` pairs.
    fn gaps(segs: &[(f32, f32)]) -> Vec<f32> {
        segs.windows(2).map(|w| w[1].0 - w[0].1).collect()
    }

    #[test]
    fn dashes_survive_a_small_cell_width() {
        // R5.2: `┈` (n=4) at cw=8 — 14pt @1x. Must not collapse into `─`.
        let segs = dash_segments(0.0, 8.0, 4);
        assert_eq!(segs.len(), 4);
        assert!(
            gaps(&segs).iter().all(|g| *g >= 1.0),
            "every internal gap is at least a pixel: {segs:?}"
        );
    }

    #[test]
    fn the_clamp_stops_dashes_merging() {
        // R5.3: the case independent rounding gets wrong. At extent 7, n=4 the
        // count bound gives n_eff=3 with starts 0,2,5 and UNCLAMPED ends 2,4,6 —
        // so dashes 0 and 1 would touch at x=2 and read as one long dash.
        // cw=7 is reachable: round(11 * 1.0 * 0.6) == 7.
        let segs = dash_segments(0.0, 7.0, 4);
        assert_eq!(segs, vec![(0.0, 1.0), (2.0, 4.0), (5.0, 6.0)]);
        assert!(gaps(&segs).iter().all(|g| *g >= 1.0), "{segs:?}");

        // R5.3, second case: extent 9 (round(15 * 0.6) == 9), n=4 -> n_eff=4.
        // Unclamped this gives [0,2][2,4][5,6][7,8] — 2 gaps where 3 are promised.
        let segs = dash_segments(0.0, 9.0, 4);
        assert_eq!(segs.len(), 4);
        assert_eq!(gaps(&segs).len(), 3);
        assert!(gaps(&segs).iter().all(|g| *g >= 1.0), "{segs:?}");
    }

    #[test]
    fn every_reachable_extent_keeps_its_gaps() {
        // R5.2/R5.3 as a property rather than spot checks: no dashed stem at any
        // reachable cell size may silently render as its solid counterpart.
        for extent in 2..=128u32 {
            let extent = extent as f32;
            for n in [2u8, 3, 4] {
                let segs = dash_segments(0.0, extent, n);
                assert!(!segs.is_empty(), "extent {extent} n {n}: nothing drawn");
                let n_eff = (n as u32).min((extent / 2.0).floor() as u32).max(1);
                if n_eff == 1 {
                    // Degenerate: one full-extent stem (R5.5).
                    assert_eq!(segs, vec![(0.0, extent)], "extent {extent} n {n}");
                    continue;
                }
                assert!(
                    gaps(&segs).iter().all(|g| *g >= 1.0),
                    "extent {extent} n {n}: dashes merged -> {segs:?}"
                );
                assert!(
                    segs.len() >= 2,
                    "extent {extent} n {n}: a dashed stem needs >= 2 dashes"
                );
            }
        }
    }

    #[test]
    fn the_clamp_is_a_noop_where_the_pattern_was_already_right() {
        // R5.4: the clamp must only fire where rounding actually collided —
        // otherwise it would be silently reshaping correct output.
        for extent in [4.0f32, 8.0, 17.0, 34.0] {
            let n = 4u8;
            let n_eff = (n as u32).min((extent / 2.0).floor() as u32).max(1);
            let n_f = n_eff as f32;
            let unclamped: Vec<(f32, f32)> = (0..n_eff)
                .map(|i| {
                    (
                        snap(0.0, extent, i as f32 / n_f),
                        snap(0.0, extent, i as f32 / n_f + 2.0 / (3.0 * n_f)),
                    )
                })
                .collect();
            // Only compare where the unclamped run was already gap-correct.
            if gaps(&unclamped).iter().all(|g| *g >= 1.0) {
                assert_eq!(
                    dash_segments(0.0, extent, n),
                    unclamped,
                    "extent {extent}: the clamp must not move a correct pattern"
                );
            }
        }
    }

    #[test]
    fn a_degenerate_extent_draws_a_full_solid_stem() {
        // R5.5: at extent <= 3 the pattern collapses. It must collapse to the FULL
        // extent — a [0, round(2*e/3)] stub would leave an inter-cell gap.
        for extent in [1.0f32, 2.0, 3.0] {
            assert_eq!(
                dash_segments(0.0, extent, 4),
                vec![(0.0, extent)],
                "extent {extent} must be a full stem, not a stub"
            );
        }
    }

    #[test]
    fn dashes_do_not_merge_across_a_cell_boundary() {
        // R5.6: the trailing gap is what keeps the last dash of one cell off the
        // first dash of the next — the reason the count bound is 2n, not 2n-1.
        for extent in [8.0f32, 17.0, 34.0] {
            let left = dash_segments(0.0, extent, 4);
            let right = dash_segments(extent, extent, 4);
            let last_end = left.last().expect("dashes").1;
            let first_start = right.first().expect("dashes").0;
            assert!(
                first_start - last_end >= 1.0,
                "extent {extent}: {last_end} -> {first_start} merges across cells"
            );
        }
    }

    #[test]
    fn a_dashed_stem_keeps_its_weight_and_axis() {
        // The dashed run must use the same band a solid stem would, so `┄` sits on
        // the same line as the `─` it continues.
        let solid = rects('\u{2500}', 0.0, 0.0, CW, CH, false);
        let dashed = rects('\u{2504}', 0.0, 0.0, CW, CH, false);
        assert!(dashed.len() > 1, "a dashed stem is segmented");
        assert_eq!(dashed[0].y0, solid[0].y0, "same band as the solid stem");
        assert_eq!(dashed[0].y1, solid[0].y1);
        // Heavy dashes are twice as thick, like their solid twin.
        let heavy = rects('\u{2505}', 0.0, 0.0, CW, CH, false);
        assert_eq!(heavy[0].height(), 6.0);
        // A vertical dashed stem segments on the other axis.
        let vert = rects('\u{2506}', 0.0, 0.0, CW, CH, false);
        assert!(vert.len() > 1);
        // Segmented along y: every dash is strictly shorter than the cell, and the
        // painted total leaves gaps. (`h < y1.max(CH)` was a no-op — `y1 <= CH`
        // always, so it reduced to `height < 34`.)
        assert!(
            vert.iter().all(|r| r.height() < CH),
            "each dash is shorter than the cell: {vert:?}"
        );
        let painted: f32 = vert.iter().map(Rect::height).sum();
        assert!(
            painted < CH,
            "gaps must leave part of the cell unpainted, got {painted} of {CH}"
        );
        assert_eq!(vert[0].width(), 3.0, "light weight across the short axis");
    }

    /// The single rect a fill/quadrant character draws, for tests that expect one.
    fn only_rect(ch: char, cw: f32, chh: f32) -> Rect {
        let r = rects(ch, 0.0, 0.0, cw, chh, false);
        assert_eq!(r.len(), 1, "{ch:?} should be a single rect, got {r:?}");
        r[0]
    }

    #[test]
    fn half_blocks_are_exact_complements() {
        // R6.1: `▀` must end exactly where `▄` starts — checked at an ODD height
        // too, where independently-rounded boundaries would overlap or gap.
        for chh in [17.0f32, 34.0, 33.0, 8.0] {
            let upper = only_rect('\u{2580}', CW, chh);
            let lower = only_rect('\u{2584}', CW, chh);
            assert_eq!(upper.y0, 0.0, "height {chh}");
            assert_eq!(lower.y1, chh, "height {chh}");
            assert_eq!(
                upper.y1, lower.y0,
                "height {chh}: halves must meet on one shared line"
            );
            assert_eq!(
                upper.height() + lower.height(),
                chh,
                "height {chh}: together they cover the cell exactly once"
            );
        }
    }

    #[test]
    fn left_and_right_blocks_are_complementary() {
        // R6.4: the same property on the x axis, including an odd width.
        for cw in [17.0f32, 16.0, 8.0, 7.0] {
            let left = only_rect('\u{258C}', cw, CH); // ▌ left half
            let right = only_rect('\u{2590}', cw, CH); // ▐ right half
            assert_eq!(left.x1, right.x0, "width {cw}");
            assert_eq!(left.width() + right.width(), cw, "width {cw}");
        }
    }

    #[test]
    fn quadrants_split_on_the_half_block_lines() {
        // R6.2: every quadrant edge must coincide with a half-block edge, or
        // `▙` would not tile with `▌` and `▄`.
        for (cw, chh) in [(17.0f32, 34.0f32), (8.0, 17.0), (7.0, 33.0)] {
            let mx = only_rect('\u{258C}', cw, chh).x1; // ▌ right edge
            let my = only_rect('\u{2580}', cw, chh).y1; // ▀ bottom edge
            for cp in 0x2596u32..=0x259Fu32 {
                let ch = char::from_u32(cp).expect("valid scalar");
                for r in rects(ch, 0.0, 0.0, cw, chh, false) {
                    for v in [r.x0, r.x1] {
                        assert!(
                            v == 0.0 || v == mx || v == cw,
                            "U+{cp:04X} x edge {v} is not a half line ({mx})"
                        );
                    }
                    for v in [r.y0, r.y1] {
                        assert!(
                            v == 0.0 || v == my || v == chh,
                            "U+{cp:04X} y edge {v} is not a half line ({my})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_four_quadrants_tile_the_cell() {
        // ▘▝▖▗ together are exactly the cell, with no overlap — the tiling claim
        // stated as area rather than as edges.
        let area: f32 = ['\u{2598}', '\u{259D}', '\u{2596}', '\u{2597}']
            .iter()
            .map(|&ch| {
                let r = only_rect(ch, CW, CH);
                r.width() * r.height()
            })
            .sum();
        assert_eq!(area, CW * CH);
    }

    #[test]
    fn two_quadrant_characters_are_two_disjoint_rects() {
        // `▚` is upper-left + lower-right — genuinely two rects, which is why
        // Quadrants is its own variant rather than a single Fraction.
        let r = rects('\u{259A}', 0.0, 0.0, CW, CH, false);
        assert_eq!(r.len(), 2);
        assert_eq!(
            classify('\u{259A}'),
            Some(Glyph::Quadrants([true, false, false, true]))
        );
        // `▛` is three quadrants.
        assert_eq!(rects('\u{259B}', 0.0, 0.0, CW, CH, false).len(), 3);
    }

    #[test]
    fn eighth_blocks_are_monotonic_and_bottom_anchored() {
        // R6.3: `▁▂▃▄▅▆▇█` grow upward from the bottom edge without going backwards.
        let mut prev = 0.0f32;
        for cp in 0x2581u32..=0x2588u32 {
            let ch = char::from_u32(cp).expect("valid scalar");
            let r = only_rect(ch, CW, CH);
            assert_eq!(r.y1, CH, "U+{cp:04X} is anchored to the bottom edge");
            assert!(r.height() >= prev, "U+{cp:04X} must not shrink");
            prev = r.height();
        }
        // The full block fills the cell exactly.
        let full = only_rect('\u{2588}', CW, CH);
        assert_eq!((full.x0, full.y0, full.x1, full.y1), (0.0, 0.0, CW, CH));
    }

    #[test]
    fn left_eighth_blocks_are_monotonic_and_left_anchored() {
        // ▉▊▋▌▍▎▏ shrink from 7/8 to 1/8, all anchored to the left edge.
        let mut prev = f32::MAX;
        for cp in 0x2589u32..=0x258Fu32 {
            let ch = char::from_u32(cp).expect("valid scalar");
            let r = only_rect(ch, CW, CH);
            assert_eq!(r.x0, 0.0, "U+{cp:04X} is anchored to the left edge");
            assert!(r.width() <= prev, "U+{cp:04X} must not grow");
            prev = r.width();
        }
    }

    #[test]
    fn shading_characters_ask_for_a_level_not_a_fill() {
        // R6.5: `░▒▓` must resolve to a shade LEVEL — the caller draws them with a
        // partial-coverage atlas entry. Emitting a solid rect would render them as
        // an opaque block.
        assert_eq!(classify('\u{2591}'), Some(Glyph::Shade(0)));
        assert_eq!(classify('\u{2592}'), Some(Glyph::Shade(1)));
        assert_eq!(classify('\u{2593}'), Some(Glyph::Shade(2)));
        for ch in ['\u{2591}', '\u{2592}', '\u{2593}'] {
            assert!(
                rects(ch, 0.0, 0.0, CW, CH, false).is_empty(),
                "{ch:?} must not emit a solid rect"
            );
        }
        // The full block, by contrast, IS solid.
        assert_eq!(rects('\u{2588}', 0.0, 0.0, CW, CH, false).len(), 1);
    }

    #[test]
    fn rect_visibility_rejects_degenerate_quads() {
        // R3.5: sub-pixel rects are dropped rather than sent to the GPU.
        assert!(Rect::new(0.0, 0.0, 3.0, 1.0).is_visible());
        assert!(!Rect::new(0.0, 0.0, 3.0, 0.0).is_visible());
        assert!(!Rect::new(0.0, 0.0, 0.5, 4.0).is_visible());
    }
}
