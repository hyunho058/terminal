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
use crate::sidebar_theme::linear;

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
    /// rules, deliberately narrower than [`Filter`](Icon::Filter) and without its knobs,
    /// so the per-card control never reads as the header's filter control.
    Hamburger,
}

/// A stroke primitive in the 24×24 unit box.
#[derive(Debug, Clone, Copy)]
enum Stroke {
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

// Three plain rules — the per-card menu affordance (☰). Inset further than FILTER's
// 4→20 so it reads as a smaller, subordinate control on a card rather than header chrome.
const HAMBURGER: &[Stroke] = &[
    Stroke::Line(6.0, 7.5, 18.0, 7.5),
    Stroke::Line(6.0, 12.0, 18.0, 12.0),
    Stroke::Line(6.0, 16.5, 18.0, 16.5),
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
    }
}

/// Append `icon` scaled to `size` (px, its 24×24 box mapped to `size`) at top-left
/// `(x, y)` in `color` (packed `0xRRGGBB`) to `out`. A sink, not a return value, so
/// nothing allocates per frame once `out` has grown (D7).
pub fn push_icon(out: &mut Vec<SidebarShape>, icon: Icon, x: f32, y: f32, size: f32, color: u32) {
    let s = size / 24.0;
    let sw = STROKE_W * s;
    let col = linear(color, 1.0);
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
        };
        if let Some(shape) = shape {
            out.push(shape);
        }
    }
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
}
