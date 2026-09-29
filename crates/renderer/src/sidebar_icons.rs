//! The mock's sidebar icon set as const stroke-geometry tables in a 24×24 unit box,
//! emitted through the shape primitive via a caller-owned sink (D7, D8).
//!
//! Each icon is a short list of strokes (lines, filled dots, rings, a rounded-rect
//! outline) in 24×24 space; [`push_icon`] scales and translates them to `(x, y,
//! size)` and appends `SidebarShape`s to the caller's `Vec` — **never returning a
//! `Vec`**, so a per-frame redraw allocates nothing once the buffer has grown (D7 ×
//! SD4). The `✳`/`◇` status marks are drawn here as geometry, not font glyphs, so
//! they render regardless of the user's font (D8).

use crate::sidebar_shapes::{Fill, SidebarShape};
use crate::sidebar_theme::{linear, AgentSkin};

/// The mock's six stroke-only sidebar icons. `Search` is intentionally absent — its
/// only consumer (the header search field) is deferred (SD8/D7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// Header filter/slider glyph: three lines with knobs.
    Filter,
    /// Header new-tab glyph.
    Plus,
    /// Header hide-sidebar / panel glyph.
    Panel,
    /// Per-tab git-branch glyph (second card line).
    GitBranch,
    /// Agent status mark `✳` (drawn, not a font glyph — D8).
    Asterisk,
    /// Ready status mark `◇` (drawn, not a font glyph — D8).
    Diamond,
    /// Waiting-band signal `⏸` — an agent handed its turn back and is waiting for
    /// input (two vertical bars). Drawn geometry, not a font glyph (D6/D8).
    Pause,
    /// Waiting-band signal `✓` — a long foreground command completed cleanly (D6).
    Check,
    /// Waiting-band signal `✕` — a foreground command exited with an error code (D6).
    Cross,
    /// Per-card menu affordance — the hamburger (sticky-tab-title D4/R3.1). Three plain
    /// rules without [`Filter`](Icon::Filter)'s knobs, so the per-card control never
    /// reads as the header's filter control. Since sidebar-weather-cards it sits on a
    /// backplate beside the agent icon, at the mock's `5→19` rules and 2.2 stroke (D21).
    Hamburger,
    // ---- sidebar-weather-cards D21: the weather card's own icons. Separate variants
    // from the band's `Asterisk`/`Diamond`/`Pause`/`Cross` on purpose: the mock's
    // shapes differ (six spokes, filled bars) and the band is out of scope, so changing
    // those in place would repaint it.
    /// Card `Working` status — the mock's six-spoke asterisk (13px, stroke 2.2).
    CardAsterisk,
    /// Card `Ready` status — a diamond outline (12px, stroke 2.2).
    CardDiamond,
    /// Card `Waiting for input` status — two **filled** bars (12px).
    CardPause,
    /// Card `Exited N` status — a heavy X (12px, stroke 3).
    CardCross,
    /// Card `Running` / `zsh` status — the `>_` prompt (13px, stroke 2).
    Prompt,
    /// Card folder-line marker — a folder outline (13px, stroke 2).
    Folder,
    /// Card branch marker — the mock's branch glyph (12px, stroke 2).
    CardBranch,
}

/// A stroke primitive in the 24×24 unit box.
#[derive(Debug, Clone, Copy)]
enum Stroke {
    /// A filled rounded rect `(x,y,w,h)` with corner radius `r` — the Waiting bars
    /// (sidebar-weather-cards D21). `RectOutline` is border-only, so it cannot draw them.
    RectFill(f32, f32, f32, f32, f32),
    /// A line `(x1,y1)→(x2,y2)` at the icon stroke width (round caps).
    Line(f32, f32, f32, f32),
    /// A filled dot of radius `r` at `(cx,cy)`.
    Dot(f32, f32, f32),
    /// A stroked ring of radius `r` at `(cx,cy)`.
    Ring(f32, f32, f32),
    /// A stroked rounded-rect outline `(x,y,w,h)` with corner radius `r`.
    RectOutline(f32, f32, f32, f32, f32),
}

/// Icon stroke width in 24-unit space (the mock's ~1.7px on a 24 viewBox).
const STROKE_W: f32 = 1.7;

const FILTER: &[Stroke] = &[
    Stroke::Line(4.0, 7.0, 20.0, 7.0),
    Stroke::Line(4.0, 12.0, 20.0, 12.0),
    Stroke::Line(4.0, 17.0, 20.0, 17.0),
    Stroke::Dot(9.0, 7.0, 2.1),
    Stroke::Dot(15.0, 12.0, 2.1),
    Stroke::Dot(8.0, 17.0, 2.1),
];

const PLUS: &[Stroke] = &[
    Stroke::Line(5.0, 12.0, 19.0, 12.0),
    Stroke::Line(12.0, 5.0, 12.0, 19.0),
];

const PANEL: &[Stroke] = &[
    Stroke::RectOutline(3.5, 4.5, 17.0, 15.0, 2.5),
    Stroke::Line(9.5, 4.5, 9.5, 19.5),
];

const GIT_BRANCH: &[Stroke] = &[
    Stroke::Ring(6.0, 6.0, 2.4),
    Stroke::Ring(6.0, 18.0, 2.4),
    Stroke::Ring(18.0, 8.5, 2.4),
    Stroke::Line(6.0, 8.4, 6.0, 15.6),    // trunk
    Stroke::Line(18.0, 11.0, 18.0, 13.0), // down from the top-right node
    Stroke::Line(18.0, 13.0, 14.0, 16.5), // elbow (arc approximated by 2 segments)
    Stroke::Line(14.0, 16.5, 8.4, 16.5),  // into the trunk
];

const ASTERISK: &[Stroke] = &[
    Stroke::Line(12.0, 5.0, 12.0, 19.0),
    Stroke::Line(5.0, 12.0, 19.0, 12.0),
    Stroke::Line(7.0, 7.0, 17.0, 17.0),
    Stroke::Line(17.0, 7.0, 7.0, 17.0),
];

const DIAMOND: &[Stroke] = &[
    Stroke::Line(12.0, 4.0, 20.0, 12.0),
    Stroke::Line(20.0, 12.0, 12.0, 20.0),
    Stroke::Line(12.0, 20.0, 4.0, 12.0),
    Stroke::Line(4.0, 12.0, 12.0, 4.0),
];

// Two vertical bars — the pause / input-waiting mark (⏸).
const PAUSE: &[Stroke] = &[
    Stroke::Line(9.5, 6.0, 9.5, 18.0),
    Stroke::Line(14.5, 6.0, 14.5, 18.0),
];

// A checkmark — the clean-completion mark (✓).
const CHECK: &[Stroke] = &[
    Stroke::Line(5.0, 12.5, 10.0, 17.5),
    Stroke::Line(10.0, 17.5, 19.0, 6.5),
];

// An X — the error mark (✕).
const CROSS: &[Stroke] = &[
    Stroke::Line(6.5, 6.5, 17.5, 17.5),
    Stroke::Line(17.5, 6.5, 6.5, 17.5),
];

// Three plain rules — the per-card menu affordance (☰), at the weather-card mock's
// geometry (`5→19`, stroke 2.2) since it moved onto its own backplate (D11/D21).
const HAMBURGER: &[Stroke] = &[
    Stroke::Line(5.0, 7.0, 19.0, 7.0),
    Stroke::Line(5.0, 12.0, 19.0, 12.0),
    Stroke::Line(5.0, 17.0, 19.0, 17.0),
];

// ---- The weather card's icons (sidebar-weather-cards D21), transcribed from the
// mock's `viewBox="0 0 24 24"` SVGs. Arcs are approximated by short segments, which
// the round caps join; at 12–13px the difference is sub-pixel.

// Six spokes: one vertical and two at ±60° (`M12 3v18`, `4.2 7.5→19.8 16.5`, …).
const CARD_ASTERISK: &[Stroke] = &[
    Stroke::Line(12.0, 3.0, 12.0, 21.0),
    Stroke::Line(4.2, 7.5, 19.8, 16.5),
    Stroke::Line(4.2, 16.5, 19.8, 7.5),
];

// `M12 3l9 9-9 9-9-9z`.
const CARD_DIAMOND: &[Stroke] = &[
    Stroke::Line(12.0, 3.0, 21.0, 12.0),
    Stroke::Line(21.0, 12.0, 12.0, 21.0),
    Stroke::Line(12.0, 21.0, 3.0, 12.0),
    Stroke::Line(3.0, 12.0, 12.0, 3.0),
];

// Two filled bars: `rect x=6 y=4 w=4 h=16 rx=1` and the same at x=14.
const CARD_PAUSE: &[Stroke] = &[
    Stroke::RectFill(6.0, 4.0, 4.0, 16.0, 1.0),
    Stroke::RectFill(14.0, 4.0, 4.0, 16.0, 1.0),
];

// `6,6→18,18` and `18,6→6,18`.
const CARD_CROSS: &[Stroke] = &[
    Stroke::Line(6.0, 6.0, 18.0, 18.0),
    Stroke::Line(18.0, 6.0, 6.0, 18.0),
];

// `polyline 5 8 9 12 5 16` + `line 12,16→19,16`.
const PROMPT: &[Stroke] = &[
    Stroke::Line(5.0, 8.0, 9.0, 12.0),
    Stroke::Line(9.0, 12.0, 5.0, 16.0),
    Stroke::Line(12.0, 16.0, 19.0, 16.0),
];

// `M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z`,
// walked clockwise from the top-left corner; each 2-unit corner arc is one segment.
const FOLDER: &[Stroke] = &[
    Stroke::Line(3.0, 7.0, 5.0, 5.0),
    Stroke::Line(5.0, 5.0, 9.0, 5.0),
    Stroke::Line(9.0, 5.0, 11.0, 7.0),
    Stroke::Line(11.0, 7.0, 19.0, 7.0),
    Stroke::Line(19.0, 7.0, 21.0, 9.0),
    Stroke::Line(21.0, 9.0, 21.0, 17.0),
    Stroke::Line(21.0, 17.0, 19.0, 19.0),
    Stroke::Line(19.0, 19.0, 5.0, 19.0),
    Stroke::Line(5.0, 19.0, 3.0, 17.0),
    Stroke::Line(3.0, 17.0, 3.0, 7.0),
];

// Nodes at (6,5), (6,19), (18,8) r=2.2; trunk `M6 7.2v9.6`; and the merge curve
// `M18 10.2c0 4-6 3.5-11 7`, sampled at t = 0, ⅓, ⅔, 1.
const CARD_BRANCH: &[Stroke] = &[
    Stroke::Ring(6.0, 5.0, 2.2),
    Stroke::Ring(6.0, 19.0, 2.2),
    Stroke::Ring(18.0, 8.0, 2.2),
    Stroke::Line(6.0, 7.2, 6.0, 16.8),
    Stroke::Line(18.0, 10.2, 16.26, 13.01),
    Stroke::Line(16.26, 13.01, 12.07, 14.72),
    Stroke::Line(12.07, 14.72, 7.0, 17.2),
];

fn strokes(icon: Icon) -> &'static [Stroke] {
    match icon {
        Icon::Filter => FILTER,
        Icon::Plus => PLUS,
        Icon::Panel => PANEL,
        Icon::GitBranch => GIT_BRANCH,
        Icon::Asterisk => ASTERISK,
        Icon::Diamond => DIAMOND,
        Icon::Pause => PAUSE,
        Icon::Check => CHECK,
        Icon::Cross => CROSS,
        Icon::Hamburger => HAMBURGER,
        Icon::CardAsterisk => CARD_ASTERISK,
        Icon::CardDiamond => CARD_DIAMOND,
        Icon::CardPause => CARD_PAUSE,
        Icon::CardCross => CARD_CROSS,
        Icon::Prompt => PROMPT,
        Icon::Folder => FOLDER,
        Icon::CardBranch => CARD_BRANCH,
    }
}

/// The icon's stroke width in 24-unit space. The pre-weather-card icons share the
/// original mock's `1.7`; the weather card's use its SVGs' own widths, which differ on
/// purpose — the X is heavier so it reads as an error (sidebar-weather-cards D21).
pub fn stroke_w(icon: Icon) -> f32 {
    match icon {
        Icon::Filter
        | Icon::Plus
        | Icon::Panel
        | Icon::GitBranch
        | Icon::Asterisk
        | Icon::Diamond
        | Icon::Pause
        | Icon::Check
        | Icon::Cross => STROKE_W,
        Icon::Hamburger | Icon::CardAsterisk | Icon::CardDiamond => 2.2,
        Icon::CardCross => 3.0,
        // Filled bars have no stroke; the value only sizes the box-escape margin.
        Icon::CardPause | Icon::Prompt | Icon::Folder | Icon::CardBranch => 2.0,
    }
}

/// Append `icon` scaled to `size` (px, its 24×24 box mapped to `size`) at top-left
/// `(x, y)` in `color` (packed `0xRRGGBB`, opaque) to `out`. A sink, not a return value,
/// so nothing allocates per frame once `out` has grown (D7).
pub fn push_icon(out: &mut Vec<SidebarShape>, icon: Icon, x: f32, y: f32, size: f32, color: u32) {
    push_icon_rgba(out, icon, x, y, size, linear(color, 1.0));
}

/// [`push_icon`] with an already-linear RGBA colour, so an icon can carry its text's
/// alpha — the card's folder, branch and prompt icons are drawn at the secondary text's
/// `.78` (sidebar-weather-cards D21).
pub fn push_icon_rgba(
    out: &mut Vec<SidebarShape>,
    icon: Icon,
    x: f32,
    y: f32,
    size: f32,
    col: [f32; 4],
) {
    let s = size / 24.0;
    let sw = stroke_w(icon) * s;
    let fill = Fill::solid(col);
    let tx = |u: f32| x + u * s;
    let ty = |u: f32| y + u * s;
    for stroke in strokes(icon) {
        let shape = match *stroke {
            Stroke::Line(x1, y1, x2, y2) => {
                SidebarShape::capsule(tx(x1), ty(y1), tx(x2), ty(y2), sw, fill)
            }
            Stroke::Dot(cx, cy, r) => {
                // A filled dot is a zero-length capsule of diameter 2r.
                SidebarShape::capsule(tx(cx), ty(cy), tx(cx), ty(cy), 2.0 * r * s, fill)
            }
            Stroke::Ring(cx, cy, r) => SidebarShape::ring(tx(cx), ty(cy), r * s, sw, fill),
            Stroke::RectOutline(rx, ry, rw, rh, rr) => {
                // Outline only: transparent fill, the stroke is the border band.
                SidebarShape::rounded_box(
                    [tx(rx), ty(ry), rw * s, rh * s],
                    rr * s,
                    sw,
                    Fill::solid([0.0; 4]),
                    col,
                )
            }
            Stroke::RectFill(rx, ry, rw, rh, rr) => SidebarShape::rounded_box(
                [tx(rx), ty(ry), rw * s, rh * s],
                rr * s,
                0.0,
                fill,
                [0.0; 4],
            ),
        };
        if let Some(shape) = shape {
            out.push(shape);
        }
    }
}

// ---------------------------------------------------------------------------
// Pixel-art agent icons (sidebar-weather-cards D6)
// ---------------------------------------------------------------------------

/// The four agent icons the weather card draws top-right, each a grid of opaque
/// cells transcribed from the mock (`shape-rendering: crispEdges`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelIcon {
    /// Claude Code's crab, 17×10.
    Claude,
    /// Antigravity's multicolour "A", 12×10.
    Antigravity,
    /// Codex's `>_` bubble, 12×10.
    Codex,
    /// The default terminal window, 12×10 — every card with no agent skin.
    Terminal,
}

/// One filled cell run: `(x, y, w, h)` in grid units and a packed `0xRRGGBB` colour.
/// Cells are emitted in table order, so a later cell paints over an earlier one — the
/// Codex and terminal icons rely on that, exactly as the SVGs they came from do.
#[derive(Debug, Clone, Copy)]
struct PixelCell(u8, u8, u8, u8, u32);

// Generated from the design file's SVGs (`Sidebar Weather Cards.html`, identical on
// its After and Anatomy boards). Antigravity's 1.02-wide cells are 1 wide here: the
// overlap only hid seams that uniform whole-pixel cells cannot have.
// Claude Code: 10 cells
const CLAUDE_CELLS: &[PixelCell] = &[
    PixelCell(2, 0, 13, 2, 0x00E6_714E),
    PixelCell(2, 2, 2, 2, 0x00E6_714E),
    PixelCell(5, 2, 7, 2, 0x00E6_714E),
    PixelCell(13, 2, 2, 2, 0x00E6_714E),
    PixelCell(0, 4, 17, 2, 0x00E6_714E),
    PixelCell(2, 6, 13, 2, 0x00E6_714E),
    PixelCell(2, 8, 1, 2, 0x00E6_714E),
    PixelCell(4, 8, 1, 2, 0x00E6_714E),
    PixelCell(12, 8, 1, 2, 0x00E6_714E),
    PixelCell(14, 8, 1, 2, 0x00E6_714E),
];
// Antigravity: 50 cells
const ANTIGRAVITY_CELLS: &[PixelCell] = &[
    PixelCell(5, 0, 1, 1, 0x00FF_8C00),
    PixelCell(6, 0, 1, 1, 0x00FF_681A),
    PixelCell(4, 1, 1, 1, 0x00E3_AF00),
    PixelCell(5, 1, 1, 1, 0x00FF_8B00),
    PixelCell(6, 1, 1, 1, 0x00FF_691B),
    PixelCell(7, 1, 1, 1, 0x00FF_482A),
    PixelCell(3, 2, 1, 1, 0x0094_C422),
    PixelCell(4, 2, 1, 1, 0x00B5_B416),
    PixelCell(5, 2, 1, 1, 0x00EF_951B),
    PixelCell(6, 2, 1, 1, 0x00FF_710E),
    PixelCell(7, 2, 1, 1, 0x00FF_5D18),
    PixelCell(8, 2, 1, 1, 0x00FF_4335),
    PixelCell(3, 3, 1, 1, 0x0071_C834),
    PixelCell(4, 3, 1, 1, 0x005F_B652),
    PixelCell(5, 3, 1, 1, 0x00D6_923D),
    PixelCell(6, 3, 1, 1, 0x00FF_7036),
    PixelCell(7, 3, 1, 1, 0x00F3_5C4A),
    PixelCell(8, 3, 1, 1, 0x00F4_3F54),
    PixelCell(2, 4, 1, 1, 0x0063_C43B),
    PixelCell(3, 4, 1, 1, 0x0050_C44C),
    PixelCell(4, 4, 1, 1, 0x003A_AB8D),
    PixelCell(5, 4, 1, 1, 0x0049_93B6),
    PixelCell(6, 4, 1, 1, 0x0086_72B4),
    PixelCell(7, 4, 1, 1, 0x0075_6FC9),
    PixelCell(8, 4, 1, 1, 0x00A3_59AD),
    PixelCell(9, 4, 1, 1, 0x00A6_579A),
    PixelCell(2, 5, 1, 1, 0x0068_C83E),
    PixelCell(3, 5, 1, 1, 0x0002_BB7C),
    PixelCell(4, 5, 1, 1, 0x0000_99E4),
    PixelCell(7, 5, 1, 1, 0x0036_80EB),
    PixelCell(8, 5, 1, 1, 0x0070_6ED5),
    PixelCell(9, 5, 1, 1, 0x0097_62B9),
    PixelCell(2, 6, 1, 1, 0x002B_C676),
    PixelCell(3, 6, 1, 1, 0x0000_B1AC),
    PixelCell(8, 6, 1, 1, 0x0035_82F2),
    PixelCell(9, 6, 1, 1, 0x006A_73DF),
    PixelCell(1, 7, 1, 1, 0x0045_C990),
    PixelCell(2, 7, 1, 1, 0x0038_BDD8),
    PixelCell(3, 7, 1, 1, 0x0000_ABE1),
    PixelCell(8, 7, 1, 1, 0x0002_8BFF),
    PixelCell(9, 7, 1, 1, 0x0034_83F8),
    PixelCell(10, 7, 1, 1, 0x0060_7AE8),
    PixelCell(1, 8, 1, 1, 0x0040_CAA1),
    PixelCell(2, 8, 1, 1, 0x0040_B8FC),
    PixelCell(9, 8, 1, 1, 0x0000_88FF),
    PixelCell(10, 8, 1, 1, 0x0030_83FC),
    PixelCell(0, 9, 1, 1, 0x0044_BBFA),
    PixelCell(1, 9, 1, 1, 0x0040_B8FC),
    PixelCell(10, 9, 1, 1, 0x0000_85FF),
    PixelCell(11, 9, 1, 1, 0x000E_87FF),
];
// Codex: 9 cells
const CODEX_CELLS: &[PixelCell] = &[
    PixelCell(2, 0, 8, 1, 0x00F4_EFE6),
    PixelCell(1, 1, 10, 1, 0x00F4_EFE6),
    PixelCell(0, 2, 12, 6, 0x00F4_EFE6),
    PixelCell(1, 8, 10, 1, 0x00F4_EFE6),
    PixelCell(2, 9, 8, 1, 0x00F4_EFE6),
    PixelCell(3, 3, 1, 1, 0x001C_2A27),
    PixelCell(4, 4, 1, 1, 0x001C_2A27),
    PixelCell(3, 5, 1, 1, 0x001C_2A27),
    PixelCell(6, 5, 3, 1, 0x001C_2A27),
];
// Terminal: 11 cells
const TERMINAL_CELLS: &[PixelCell] = &[
    PixelCell(0, 0, 12, 2, 0x00C9_C2B2),
    PixelCell(1, 2, 10, 7, 0x0015_130F),
    PixelCell(0, 2, 1, 8, 0x00C9_C2B2),
    PixelCell(11, 2, 1, 8, 0x00C9_C2B2),
    PixelCell(0, 9, 12, 1, 0x00C9_C2B2),
    PixelCell(2, 3, 1, 1, 0x00E8_E2D2),
    PixelCell(3, 4, 1, 1, 0x00E8_E2D2),
    PixelCell(4, 5, 1, 1, 0x00E8_E2D2),
    PixelCell(3, 6, 1, 1, 0x00E8_E2D2),
    PixelCell(2, 7, 1, 1, 0x00E8_E2D2),
    PixelCell(6, 7, 4, 1, 0x0086_B0A1),
];

impl PixelIcon {
    /// Every icon, for a caller that needs the widest (the `≡`'s slot, D11).
    pub const ALL: [PixelIcon; 4] = [
        PixelIcon::Claude,
        PixelIcon::Antigravity,
        PixelIcon::Codex,
        PixelIcon::Terminal,
    ];

    /// The icon for a card skin (D6): each agent its own, and the plain card — which
    /// also covers Gemini and Aider — the terminal.
    pub fn for_skin(skin: AgentSkin) -> PixelIcon {
        match skin {
            AgentSkin::Claude => PixelIcon::Claude,
            AgentSkin::Antigravity => PixelIcon::Antigravity,
            AgentSkin::Codex => PixelIcon::Codex,
            AgentSkin::Generic => PixelIcon::Terminal,
        }
    }

    /// Grid size in cells, `(cols, rows)`.
    pub fn grid(self) -> (u32, u32) {
        match self {
            PixelIcon::Claude => (17, 10),
            PixelIcon::Antigravity | PixelIcon::Codex | PixelIcon::Terminal => (12, 10),
        }
    }

    fn cells(self) -> &'static [PixelCell] {
        match self {
            PixelIcon::Claude => CLAUDE_CELLS,
            PixelIcon::Antigravity => ANTIGRAVITY_CELLS,
            PixelIcon::Codex => CODEX_CELLS,
            PixelIcon::Terminal => TERMINAL_CELLS,
        }
    }

    /// One cell's side in **logical** px at the design size: the crab is drawn 43
    /// wide over 17 columns, the others 36 wide over 12.
    pub fn design_cell(self) -> f32 {
        match self {
            PixelIcon::Claude => 43.0 / 17.0,
            PixelIcon::Antigravity | PixelIcon::Codex | PixelIcon::Terminal => 3.0,
        }
    }

    /// One cell's side in **physical** px: `max(1, round(design_cell × scale))` (D6).
    /// Uniform for the whole icon, so every cell is the same size and they tile with
    /// no seam — at the cost of ≤ half a pixel per cell of size fidelity.
    pub fn cell_px(self, scale: f64) -> f32 {
        (self.design_cell() * scale as f32).round().max(1.0)
    }

    /// The drawn extent `[w, h]` in physical px at `scale`.
    pub fn extent(self, scale: f64) -> [f32; 2] {
        let (cols, rows) = self.grid();
        let c = self.cell_px(scale);
        [cols as f32 * c, rows as f32 * c]
    }
}

/// Append `icon`'s cells with its top-left at `(x, y)` — snapped to a whole physical
/// pixel — at `scale`, and return the drawn extent `[w, h]`. Every emitted rect has
/// integer coordinates and sides, so the SDF's antialiasing lands exactly on pixel
/// boundaries and the art stays crisp (D6).
pub fn push_pixel_icon(
    out: &mut Vec<SidebarShape>,
    icon: PixelIcon,
    x: f32,
    y: f32,
    scale: f64,
) -> [f32; 2] {
    let c = icon.cell_px(scale);
    let (ox, oy) = (x.round(), y.round());
    for &PixelCell(cx, cy, cw, ch, rgb) in icon.cells() {
        if let Some(shape) = SidebarShape::rounded_box(
            [
                ox + f32::from(cx) * c,
                oy + f32::from(cy) * c,
                f32::from(cw) * c,
                f32::from(ch) * c,
            ],
            0.0,
            0.0,
            Fill::solid(linear(rgb, 1.0)),
            [0.0; 4],
        ) {
            out.push(shape);
        }
    }
    icon.extent(scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Icon; 9] = [
        Icon::Filter,
        Icon::Plus,
        Icon::Panel,
        Icon::GitBranch,
        Icon::Asterisk,
        Icon::Diamond,
        Icon::Pause,
        Icon::Check,
        Icon::Cross,
    ];

    /// The card's icons (sidebar-weather-cards D21) — kept apart from [`ALL`] so the
    /// tests over the band's icons stay exactly as they were (R18.3).
    const CARD_ICONS: [Icon; 8] = [
        Icon::Hamburger,
        Icon::CardAsterisk,
        Icon::CardDiamond,
        Icon::CardPause,
        Icon::CardCross,
        Icon::Prompt,
        Icon::Folder,
        Icon::CardBranch,
    ];

    #[test]
    fn every_icon_is_non_empty() {
        // R7.1 — all six expand to at least one shape.
        for icon in ALL {
            let mut out = Vec::new();
            push_icon(&mut out, icon, 0.0, 0.0, 24.0, 0x00FF_FFFF);
            assert!(!out.is_empty(), "{icon:?} produced no shapes");
        }
    }

    #[test]
    fn every_icon_stays_in_its_box() {
        // R7.2 — at size 24 from origin, every shape's bbox stays within the 24×24
        // box plus a small stroke-overhang margin.
        let margin = STROKE_W; // stroke extends ~STROKE_W/2 beyond an endpoint
        for icon in ALL {
            let mut out = Vec::new();
            push_icon(&mut out, icon, 0.0, 0.0, 24.0, 0x00FF_FFFF);
            for s in &out {
                let [bx, by, bw, bh] = s.bbox;
                assert!(
                    bx >= -margin
                        && by >= -margin
                        && bx + bw <= 24.0 + margin
                        && by + bh <= 24.0 + margin,
                    "{icon:?} shape bbox {:?} escapes the 24×24 box",
                    s.bbox
                );
            }
        }
    }

    #[test]
    fn marks_are_font_independent() {
        // R7.4 — the ✳ / ◇ marks are geometry, so they emit shapes with no atlas /
        // font lookup (push_icon never touches the glyph atlas).
        for icon in [Icon::Asterisk, Icon::Diamond] {
            let mut out = Vec::new();
            push_icon(&mut out, icon, 4.0, 4.0, 13.0, 0x00E2_D9C8);
            assert!(out.len() >= 4, "{icon:?} should have 4 strokes");
        }
    }

    #[test]
    fn push_icon_appends_to_existing_buffer() {
        // R7.3 — the sink appends; it does not replace or return.
        let mut out = Vec::new();
        push_icon(&mut out, Icon::Plus, 0.0, 0.0, 24.0, 0x00FF_FFFF);
        let after_first = out.len();
        push_icon(&mut out, Icon::Diamond, 0.0, 0.0, 24.0, 0x00FF_FFFF);
        assert!(
            out.len() > after_first,
            "second icon must append, not replace"
        );
        assert_eq!(after_first, 2, "Plus is two strokes");
    }

    #[test]
    fn scales_and_translates() {
        // A larger size and an offset move the shapes accordingly.
        let mut small = Vec::new();
        push_icon(&mut small, Icon::Plus, 0.0, 0.0, 24.0, 0x00FF_FFFF);
        let mut big = Vec::new();
        push_icon(&mut big, Icon::Plus, 100.0, 50.0, 48.0, 0x00FF_FFFF);
        // The horizontal line of Plus spans x 5→19 in unit space. At size 48 (s=2)
        // offset x=100, that's 110→138 — wider and shifted vs the size-24 version.
        assert!(
            big[0].bbox[2] > small[0].bbox[2],
            "bigger size → wider shape"
        );
        assert!(big[0].bbox[0] >= 100.0, "offset shifts the shape right");
    }

    // -----------------------------------------------------------------------
    // sidebar-weather-cards — card icons (D21) and pixel agent icons (D6)
    // -----------------------------------------------------------------------

    #[test]
    fn every_card_icon_is_drawn_inside_its_box() {
        // R18.1 — the band's two checks above, over the card's icons: each emits shapes,
        // and at size 24 they stay in the 24×24 box plus the overhang of its own, thicker
        // stroke (D21: up to 3.0 where the band's is 1.7).
        for icon in CARD_ICONS {
            let margin = stroke_w(icon); // a stroke extends ~w/2 beyond an endpoint
            let mut out = Vec::new();
            push_icon(&mut out, icon, 0.0, 0.0, 24.0, 0x00FF_FFFF);
            assert!(!out.is_empty(), "{icon:?} produced no shapes");
            for s in &out {
                let [bx, by, bw, bh] = s.bbox;
                assert!(
                    bx >= -margin
                        && by >= -margin
                        && bx + bw <= 24.0 + margin
                        && by + bh <= 24.0 + margin,
                    "{icon:?} shape bbox {:?} escapes the 24×24 box",
                    s.bbox
                );
            }
        }
    }

    /// The capsules' full thickness (a capsule stores half of it in `geom[0]`).
    fn thicknesses(out: &[SidebarShape]) -> Vec<f32> {
        out.iter()
            .filter(|s| s.kind == crate::sidebar_shapes::KIND_CAPSULE)
            .map(|s| s.geom[0] * 2.0)
            .collect()
    }

    #[test]
    fn card_icons_stroke_at_the_mocks_widths_and_sizes() {
        // R18.1 — thickness is `w × S / 24` at the D21 sizes.
        for (icon, size, w) in [
            (Icon::CardAsterisk, 13.0, 2.2),
            (Icon::CardDiamond, 12.0, 2.2),
            (Icon::CardCross, 12.0, 3.0),
            (Icon::Prompt, 13.0, 2.0),
            (Icon::Folder, 13.0, 2.0),
            (Icon::CardBranch, 12.0, 2.0),
            (Icon::Hamburger, 14.0, 2.2),
        ] {
            assert_eq!(stroke_w(icon), w, "{icon:?} stroke width");
            let mut out = Vec::new();
            push_icon(&mut out, icon, 0.0, 0.0, size, 0x00FF_FFFF);
            let t = thicknesses(&out);
            assert!(!t.is_empty(), "{icon:?} draws strokes");
            for got in t {
                assert!((got - w * size / 24.0).abs() < 1e-5, "{icon:?}: {got}");
            }
        }
    }

    #[test]
    fn card_icons_have_the_mocks_stroke_counts() {
        // Six spokes are three lines; the diamond four sides; the X two; the prompt a
        // two-segment chevron plus its underscore.
        for (icon, lines) in [
            (Icon::CardAsterisk, 3),
            (Icon::CardDiamond, 4),
            (Icon::CardCross, 2),
            (Icon::Prompt, 3),
        ] {
            let mut out = Vec::new();
            push_icon(&mut out, icon, 0.0, 0.0, 24.0, 0x00FF_FFFF);
            assert_eq!(thicknesses(&out).len(), lines, "{icon:?}");
        }
    }

    #[test]
    fn the_card_pause_is_two_filled_bars() {
        // R18.2 — at size 12 (half scale) the mock's 4×16 bars at x=6/14, y=4, rx=1
        // become 2×8 at x=3/7, y=2, radius 0.5 — filled, with no border band.
        let mut out = Vec::new();
        push_icon(&mut out, Icon::CardPause, 0.0, 0.0, 12.0, 0x00F2_C186);
        assert_eq!(out.len(), 2);
        for (s, x) in out.iter().zip([3.0, 7.0]) {
            assert_eq!(s.kind, crate::sidebar_shapes::KIND_ROUNDED_BOX);
            assert_eq!(s.bbox, [x, 2.0, 2.0, 8.0]);
            assert_eq!(s.geom, [0.5, 0.0], "radius 0.5, no border");
            assert_eq!(s.fill0, linear(0x00F2_C186, 1.0), "filled in the colour");
        }
    }

    #[test]
    fn the_bands_icons_are_untouched() {
        // R18.3 — the waiting band's Pause / Check / Cross keep their pre-weather-card
        // geometry and 1.7 stroke: pinned as the capsule endpoints at size 24.
        let segs = |icon| {
            let mut out = Vec::new();
            push_icon(&mut out, icon, 0.0, 0.0, 24.0, 0x00FF_FFFF);
            assert!(thicknesses(&out).iter().all(|t| (t - 1.7).abs() < 1e-6));
            out.iter().map(|s| s.seg).collect::<Vec<_>>()
        };
        assert_eq!(
            segs(Icon::Pause),
            vec![[9.5, 6.0, 9.5, 18.0], [14.5, 6.0, 14.5, 18.0]]
        );
        assert_eq!(
            segs(Icon::Check),
            vec![[5.0, 12.5, 10.0, 17.5], [10.0, 17.5, 19.0, 6.5]]
        );
        assert_eq!(
            segs(Icon::Cross),
            vec![[6.5, 6.5, 17.5, 17.5], [17.5, 6.5, 6.5, 17.5]]
        );
    }

    #[test]
    fn push_icon_rgba_carries_alpha() {
        // D21 — the folder / branch / prompt icons take the secondary text's `.78`.
        let mut out = Vec::new();
        let col = linear(0x00F4_EFE6, 0.78);
        push_icon_rgba(&mut out, Icon::Folder, 0.0, 0.0, 13.0, col);
        assert!(out.iter().all(|s| s.fill0 == col));
    }

    const PIXEL_ICONS: [PixelIcon; 4] = [
        PixelIcon::Claude,
        PixelIcon::Antigravity,
        PixelIcon::Codex,
        PixelIcon::Terminal,
    ];

    #[test]
    fn pixel_icon_follows_the_skin() {
        assert_eq!(PixelIcon::for_skin(AgentSkin::Claude), PixelIcon::Claude);
        assert_eq!(
            PixelIcon::for_skin(AgentSkin::Antigravity),
            PixelIcon::Antigravity
        );
        assert_eq!(PixelIcon::for_skin(AgentSkin::Codex), PixelIcon::Codex);
        assert_eq!(PixelIcon::for_skin(AgentSkin::Generic), PixelIcon::Terminal);
    }

    #[test]
    fn pixel_cells_are_uniform_whole_pixels() {
        // R6.2 — at 1×, 1.5× and 2×, from a fractional origin: every rect is integral
        // and an exact multiple of the icon's one cell size.
        for icon in PIXEL_ICONS {
            for scale in [1.0, 1.5, 2.0] {
                let c = icon.cell_px(scale);
                assert_eq!(c, (icon.design_cell() * scale as f32).round().max(1.0));
                let mut out = Vec::new();
                push_pixel_icon(&mut out, icon, 10.4, 20.6, scale);
                assert!(!out.is_empty());
                for s in &out {
                    let [x, y, w, h] = s.bbox;
                    for v in [x, y, w, h] {
                        assert_eq!(v, v.round(), "{icon:?}@{scale}: {:?}", s.bbox);
                    }
                    assert_eq!((x - 10.0) % c, 0.0, "{icon:?}@{scale} x on the grid");
                    assert_eq!((y - 21.0) % c, 0.0, "{icon:?}@{scale} y on the grid");
                    assert_eq!(w % c, 0.0, "{icon:?}@{scale} w");
                    assert_eq!(h % c, 0.0, "{icon:?}@{scale} h");
                    assert_eq!(s.geom, [0.0, 0.0], "square, borderless cells");
                }
            }
        }
    }

    #[test]
    fn adjacent_pixel_cells_share_edges_exactly() {
        // R6.2 — Antigravity's cells are all 1×1: every horizontally adjacent pair
        // must meet with no gap and no overlap, at a scale that is not integral.
        let mut out = Vec::new();
        push_pixel_icon(&mut out, PixelIcon::Antigravity, 0.0, 0.0, 1.5);
        let c = PixelIcon::Antigravity.cell_px(1.5);
        let mut pairs = 0;
        for a in &out {
            for b in &out {
                if a.bbox[1] == b.bbox[1] && b.bbox[0] > a.bbox[0] && b.bbox[0] - a.bbox[0] <= c {
                    assert_eq!(a.bbox[0] + a.bbox[2], b.bbox[0], "seam or overlap");
                    pairs += 1;
                }
            }
        }
        assert!(pairs > 20, "the A has many horizontal neighbours ({pairs})");
    }

    #[test]
    fn the_crab_is_within_half_a_pixel_per_cell_of_its_design_size() {
        // R6.3 — 85×50 at 2× (design 86×50.6), 51×30 at 1× (design 43×25.3).
        assert_eq!(PixelIcon::Claude.extent(2.0), [85.0, 50.0]);
        assert_eq!(PixelIcon::Claude.extent(1.0), [51.0, 30.0]);
        for scale in [1.0, 2.0] {
            let [w, _] = PixelIcon::Claude.extent(scale);
            let design = 43.0 * scale as f32;
            assert!((w - design).abs() <= 0.5 * 17.0, "{scale}: {w} vs {design}");
        }
        // The 12-column icons are exact at integral scales.
        assert_eq!(PixelIcon::Terminal.extent(2.0), [72.0, 60.0]);
        assert_eq!(PixelIcon::Codex.extent(1.0), [36.0, 30.0]);
    }

    #[test]
    fn push_pixel_icon_returns_its_extent_and_places_cells_from_the_origin() {
        let mut out = Vec::new();
        let ext = push_pixel_icon(&mut out, PixelIcon::Terminal, 100.0, 50.0, 2.0);
        assert_eq!(ext, PixelIcon::Terminal.extent(2.0));
        // The frame's top bar is cell (0,0) 12×2 → 72×12 physical at the origin.
        assert_eq!(out[0].bbox, [100.0, 50.0, 72.0, 12.0]);
        assert_eq!(out[0].fill0, linear(0x00C9_C2B2, 1.0));
        // A cell off both axes pins the offset's direction: the screen, cell (1,2)
        // 10×7, is at +6/+12 and 60×42 — not mirrored above or left of the origin.
        assert_eq!(out[1].bbox, [106.0, 62.0, 60.0, 42.0]);
        assert_eq!(out[1].fill0, linear(0x0015_130F, 1.0));
    }

    #[test]
    fn pixel_cells_carry_their_table_colours() {
        // R6.4 — 50 cells, (5,0) is #ff8c00; and the counts of the other three.
        let count = |icon| {
            let mut out = Vec::new();
            push_pixel_icon(&mut out, icon, 0.0, 0.0, 1.0);
            out
        };
        let a = count(PixelIcon::Antigravity);
        assert_eq!(a.len(), 50);
        let c = PixelIcon::Antigravity.cell_px(1.0);
        let first = a
            .iter()
            .find(|s| s.bbox[0] == 5.0 * c && s.bbox[1] == 0.0)
            .expect("cell (5,0)");
        assert_eq!(first.fill0, linear(0x00FF_8C00, 1.0));
        assert_eq!(count(PixelIcon::Claude).len(), 10);
        assert_eq!(count(PixelIcon::Codex).len(), 9);
        assert_eq!(count(PixelIcon::Terminal).len(), 11);
        assert!(count(PixelIcon::Claude)
            .iter()
            .all(|s| s.fill0 == linear(0x00E6_714E, 1.0)));
    }
}
