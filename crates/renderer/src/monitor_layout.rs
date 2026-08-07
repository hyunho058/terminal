//! Pure, state-free, GPU-free pixel layout for the ai-monitor panel (R3.4).
//!
//! `(model, panel rect, scale, cell) → shapes + positioned text runs`. Every function is a
//! `pub fn` of its arguments: nothing here holds state, touches a device, or reads a
//! clock, so the whole panel — including the height allocator D18 hinges on — is testable
//! headlessly. The layer ([`crate::monitor_panel`], T4) does nothing but hand the shapes
//! to [`SidebarShapePipeline`](crate::sidebar_shapes::SidebarShapePipeline) and the runs to
//! `tab_bar::layout_line`.
//!
//! **Three rules this module is built around:**
//!
//! * **Text is measured in columns, through the shared helpers.** `text_w` reads the pen
//!   off [`pen_layout`] and budgets are counted with [`char_cols`], so a Hangul task title
//!   spends 2 columns per char here exactly as it will when the layer draws it. A second
//!   width rule in this file would put the run rectangles and the glyphs it reserves them
//!   for permanently out of step.
//! * **Nothing is drawn outside the panel band.** There is no scissor rect — the terminal
//!   owns every pixel to the left of `panel.x`, so a region that does not fit vertically is
//!   dropped rather than allowed to paint over the grid.
//! * **Degenerate geometry never reaches the GPU.** The shape constructors return `Option`
//!   and reject non-finite/non-positive extents (`sidebar_shapes` D9); every call here
//!   funnels through [`Painter::shape`], which extends the vector with the `Option` and so
//!   silently drops a bad shape instead of panicking on a zero-width panel.

use crate::monitor_model::{
    Bar, Card, ColorRole, MonitorModel, PanelStatus, Row, Section, SectionBody, StatTile,
};
use crate::sidebar_layout::Rect;
use crate::sidebar_shapes::{Fill, SidebarShape};
use crate::sidebar_theme::linear;
use crate::tab_bar::{advance_px, char_cols, pen_layout, truncate_cols};

/// Layout tokens in **logical** px, multiplied by `scale` at the single point of use —
/// the same discipline as `sidebar_layout::tokens`. Public so the layer and any future
/// hit test measure the panel the same way this module drew it.
pub mod metrics {
    /// Horizontal padding between the panel edge and its content.
    pub const PAD: f32 = 14.0;
    /// Vertical gap between two regions.
    pub const GAP: f32 = 8.0;
    /// Padding above and below the header's text line.
    pub const HEADER_PAD_Y: f32 = 10.0;
    /// Width of the header's accent bar and a card's left accent.
    pub const ACCENT_W: f32 = 3.0;
    /// Rule/border thickness.
    pub const HAIRLINE: f32 = 1.0;
    /// Card and tile corner radius.
    pub const RADIUS: f32 = 5.0;
    /// Horizontal padding inside the status pill.
    pub const PILL_PAD_X: f32 = 8.0;
    /// Padding above and below the pill's text line.
    pub const PILL_PAD_Y: f32 = 3.0;
    /// Diameter of the pill's status dot.
    pub const DOT_D: f32 = 5.0;
    /// Gap between the dot and the pill's label.
    pub const DOT_GAP: f32 = 5.0;
    /// Stat-grid column count (the design's 3x2).
    pub const TILE_COLS: usize = 3;
    /// Gap between stat tiles, both axes.
    pub const TILE_GAP: f32 = 6.0;
    /// Padding inside a stat tile.
    pub const TILE_PAD: f32 = 6.0;
    /// Context-gauge track thickness.
    pub const GAUGE_H: f32 = 3.0;
    /// Gap between a tile's value line and its gauge.
    pub const GAUGE_GAP: f32 = 4.0;
    /// Padding inside a card.
    pub const CARD_PAD: f32 = 6.0;
    /// Vertical gap below one section row.
    pub const ROW_GAP: f32 = 3.0;
    /// Width of the verb gutter, in **columns** — a fixed gutter (not each label's own
    /// width) is what makes `READ`/`WRITE`/`EDIT` scan as a column.
    pub const LABEL_COLS: usize = 6;
    /// Tool-usage bar track width.
    pub const BAR_W: f32 = 90.0;
    /// Tool-usage bar thickness.
    pub const BAR_H: f32 = 4.0;
    /// Line box height as a multiple of the cell height — matches the retiring overlay's
    /// spacing so the panel's leading does not visibly change with this feature.
    pub const LINE_FACTOR: f32 = 1.15;
}

use metrics::*;

/// Float slack for "does this fit" comparisons. Region heights are sums of scaled
/// constants, so an exactly-sized panel (the R8.1/R8.5 cases) misses by an ulp without
/// it — and a dropped heading is a visible defect, not a rounding one.
const FIT_EPS: f32 = 1e-3;

/// One positioned string. `rect` is the **line box** (not the glyph ink): the layer draws
/// the run starting at `rect.x` with the baseline at `rect.y + ascent`, which is what
/// `tab_bar::layout_line` advances through. `color` is packed `0xRRGGBB`.
#[derive(Clone, Debug, PartialEq)]
pub struct TextRun {
    pub rect: Rect,
    pub text: String,
    pub color: u32,
}

/// Everything one frame of the panel draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelDraw {
    pub shapes: Vec<SidebarShape>,
    pub runs: Vec<TextRun>,
}

/// What the height allocator needs to know about one section: how many entries it could
/// draw, and how tall one of its rows is.
///
/// `row_h` is per section because a card (title + detail line) is taller than a list row.
/// A card **reserves its detail line whether or not it has one**: the allocator grants
/// *rows*, so a section's rows must all be the same height, or the height it was granted
/// and the height it draws disagree and the footer moves under the content.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SectionMetrics {
    pub entries: usize,
    pub row_h: f32,
}

/// One section's share of the panel's leftover height.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Grant {
    /// Whether the section's heading is drawn at all. `false` only in D18's last resort,
    /// where not even the headings fit.
    pub heading: bool,
    /// How many entry rows the section draws. May be 0 alongside `heading: true` — D10
    /// treats a heading-only section as a valid state, not an omission (R8.4).
    pub rows: usize,
}

/// D18's height allocator: divide `available` px between the sections, in the design's
/// order, and report what each one may draw.
///
/// The order of operations is the decision, and it is not the obvious one:
///
/// 1. **Headings are reserved first, for as many sections as fit, from the top.** A
///    section that renders only its heading still tells the user the section exists (D10),
///    which is worth more than a row on a neighbour — so R8.5's four-heading height yields
///    four headings and no rows, not two headings with a row each. When they do not all
///    fit, the ones that go are the ones at the **bottom** of the design order.
/// 2. **One row each, in order** (R8.1), skipping empty sections. The grant is splittable:
///    a section whose heading fit but whose row does not gets the heading alone (R8.4).
/// 3. **Round-robin, one row per section per pass** (R8.2), each section capped by its own
///    entry count (R8.3). Greedy would hand a 40-entry TASKS every spare row and leave the
///    other five at one apiece.
///
/// A section with a non-positive or non-finite `row_h` is granted no rows — that also
/// keeps the round-robin loop from spinning forever on a zero-height row.
pub fn allocate(sections: &[SectionMetrics], available: f32, heading_h: f32) -> Vec<Grant> {
    let mut grants = vec![Grant::default(); sections.len()];
    if !available.is_finite() || !heading_h.is_finite() || available <= 0.0 || heading_h < 0.0 {
        return grants;
    }

    // 1. Headings, dropping from the bottom of the order when they do not all fit.
    let mut used = 0.0f32;
    let mut headed = 0usize;
    while headed < sections.len() && used + heading_h <= available + FIT_EPS {
        used += heading_h;
        headed += 1;
    }
    for g in grants.iter_mut().take(headed) {
        g.heading = true;
    }
    let mut remaining = available - used;

    // 2. The first row of each headed section. `continue`, not `break`: a tall card row
    //    that does not fit must not veto a later section's shorter one.
    for (g, sm) in grants.iter_mut().zip(sections).take(headed) {
        if sm.entries == 0 || !usable_row(sm.row_h) || remaining + FIT_EPS < sm.row_h {
            continue;
        }
        remaining -= sm.row_h;
        g.rows = 1;
    }

    // 3. Round-robin passes until one pass grants nothing. Terminates: every grant either
    //    spends `row_h > 0` of a finite budget or raises `rows` toward a finite entry cap.
    loop {
        let mut granted = false;
        for (g, sm) in grants.iter_mut().zip(sections).take(headed) {
            if g.rows >= sm.entries || !usable_row(sm.row_h) || remaining + FIT_EPS < sm.row_h {
                continue;
            }
            remaining -= sm.row_h;
            g.rows += 1;
            granted = true;
        }
        if !granted {
            break;
        }
    }
    grants
}

fn usable_row(row_h: f32) -> bool {
    row_h.is_finite() && row_h > 0.0
}

/// Lay the panel out inside `panel` (`[x, y, w, h]`, physical px, top-left origin).
///
/// Returns an empty draw — never a panic — for an invisible model or a degenerate panel:
/// the caller derives the rect from a live `right_inset` that D4 can clamp to nothing, so
/// a zero-width panel is a normal transient state, not a bug to assert on.
pub fn layout(model: &MonitorModel, panel: [f32; 4], scale: f64, cell: [f32; 2]) -> PanelDraw {
    if !model.visible {
        return PanelDraw::default();
    }
    let Some(m) = Metrics::new(panel, scale, cell) else {
        return PanelDraw::default();
    };
    let mut p = Painter {
        m,
        out: PanelDraw::default(),
    };
    p.panel_bg();
    // D18 step 1: the pinned chrome is laid out before anything elastic, so the sections
    // divide what is genuinely left rather than pushing the footer off the bottom.
    let y = p.header(model, m.panel[1]);
    let y = p.session(model, y);
    let y = p.tiles(model, y);
    let footer_top = p.footer(model);
    p.sections(model, y, footer_top);
    p.out
}

/// Resolved per-frame geometry. All physical px except `scale`.
#[derive(Clone, Copy, Debug)]
struct Metrics {
    scale: f32,
    cell_w: f32,
    /// One text line's box height.
    line: f32,
    panel: [f32; 4],
    /// Left edge of the content column (panel + padding).
    inner_x: f32,
    /// Width of the content column; may be ≤ 0 on a very narrow panel.
    inner_w: f32,
}

impl Metrics {
    fn new(panel: [f32; 4], scale: f64, cell: [f32; 2]) -> Option<Metrics> {
        let [x, y, w, h] = panel;
        if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite())
            || w <= 0.0
            || h <= 0.0
        {
            return None;
        }
        let [cell_w, cell_h] = cell;
        if !(cell_w.is_finite() && cell_h.is_finite()) || cell_w <= 0.0 || cell_h <= 0.0 {
            return None;
        }
        let scale = scale as f32;
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        Some(Metrics {
            scale,
            cell_w,
            line: cell_h * LINE_FACTOR,
            panel,
            inner_x: x + PAD * scale,
            inner_w: w - 2.0 * PAD * scale,
        })
    }

    fn s(&self, logical: f32) -> f32 {
        logical * self.scale
    }

    fn right(&self) -> f32 {
        self.inner_x + self.inner_w
    }

    fn bottom(&self) -> f32 {
        self.panel[1] + self.panel[3]
    }

    /// Column budget for a pixel width — the same floor the retiring overlay used, so a
    /// wide glyph is never half-drawn past the edge.
    fn cols(&self, px: f32) -> usize {
        if !px.is_finite() || px <= 0.0 {
            return 0;
        }
        (px / self.cell_w).floor().max(0.0) as usize
    }

    /// One row's height for a body kind. See [`SectionMetrics::row_h`] for why a card
    /// reserves its detail line unconditionally.
    fn row_h(&self, body: &SectionBody) -> f32 {
        match body {
            SectionBody::Cards(_) => 2.0 * self.line + self.s(2.0 * CARD_PAD + ROW_GAP),
            _ => self.line + self.s(ROW_GAP),
        }
    }

    fn heading_h(&self) -> f32 {
        self.line + self.s(ROW_GAP)
    }
}

/// Advance width of `s` in px: the pen position after its last char, read off
/// [`pen_layout`] itself so a run's reserved width and the glyphs the layer will draw into
/// it cannot disagree about a wide char.
fn text_w(s: &str, cell_w: f32) -> f32 {
    pen_layout(s, 0.0, cell_w)
        .map(|(c, _)| advance_px(c, cell_w))
        .sum()
}

/// Display width of `s` in columns, on the one width rule the whole codebase shares.
fn text_cols(s: &str) -> usize {
    s.chars().map(char_cols).sum()
}

fn solid(role: ColorRole) -> Fill {
    Fill::solid(linear(role.packed(), 1.0))
}

/// Accumulates one frame's primitives. Methods return the `y` they finished at, so each
/// region stacks on the previous one without a shared cursor to get out of step.
struct Painter {
    m: Metrics,
    out: PanelDraw,
}

impl Painter {
    /// The single funnel for every shape: an `Option` from a constructor extends the
    /// vector with zero or one element, so rejected geometry (D9) is dropped here rather
    /// than unwrapped into a panic.
    fn shape(&mut self, s: Option<SidebarShape>) {
        self.out.shapes.extend(s);
    }

    /// Is a region `h` tall, starting at `y`, wholly inside the panel band? Nothing may
    /// paint outside it — there is no scissor rect.
    fn fits(&self, y: f32, h: f32) -> bool {
        y + FIT_EPS >= self.m.panel[1] && y + h <= self.m.bottom() + FIT_EPS
    }

    fn fill_rect(&mut self, r: Rect, role: ColorRole) {
        self.shape(SidebarShape::rounded_box(
            [r.x, r.y, r.w, r.h],
            0.0,
            0.0,
            solid(role),
            [0.0; 4],
        ));
    }

    fn boxed(&mut self, r: Rect, fill: ColorRole, border: ColorRole) {
        let m = self.m;
        self.shape(SidebarShape::rounded_box(
            [r.x, r.y, r.w, r.h],
            m.s(RADIUS),
            m.s(HAIRLINE),
            solid(fill),
            linear(border.packed(), 1.0),
        ));
    }

    /// A horizontal rule. A zero-length capsule is a legal *dot* to the constructor, which
    /// on a rule would be a stray pixel — so an empty span draws nothing.
    fn rule(&mut self, x0: f32, x1: f32, y: f32, role: ColorRole) {
        if !x0.is_finite() || !x1.is_finite() || x1 <= x0 {
            return;
        }
        let t = self.m.s(HAIRLINE);
        self.shape(SidebarShape::capsule(x0, y, x1, y, t, solid(role)));
    }

    /// Draw `text` at `x`, truncated to `max_w`. Returns the width actually used.
    fn text(&mut self, x: f32, y: f32, max_w: f32, text: &str, role: ColorRole) -> f32 {
        let m = self.m;
        let fitted = truncate_cols(text, m.cols(max_w));
        if fitted.is_empty() {
            return 0.0;
        }
        let w = text_w(&fitted, m.cell_w);
        self.out.runs.push(TextRun {
            rect: Rect { x, y, w, h: m.line },
            text: fitted,
            color: role.packed(),
        });
        w
    }

    /// Draw `text` ending at `right`. Returns the left edge of what was drawn — `right`
    /// itself when nothing was, so callers can chain marks inward without a special case.
    fn text_right(&mut self, right: f32, y: f32, text: &str, role: ColorRole) -> f32 {
        let m = self.m;
        let fitted = truncate_cols(text, m.cols(right - m.inner_x));
        if fitted.is_empty() {
            return right;
        }
        let w = text_w(&fitted, m.cell_w);
        let x = right - w;
        self.out.runs.push(TextRun {
            rect: Rect { x, y, w, h: m.line },
            text: fitted,
            color: role.packed(),
        });
        x
    }

    /// Draw a trailing **mark** — a count, a duration, `+N more` — ending at `right`, or
    /// draw nothing at all if it does not fit whole.
    ///
    /// Unlike prose, a clipped number is not a shortened value but a *wrong* one: `+12
    /// more` truncated to `+1 more` states a falsehood, and `32.8s` truncated to `32.` is
    /// not a duration. Dropping the mark loses information; keeping a clipped one invents
    /// it.
    fn mark_right(&mut self, right: f32, y: f32, text: &str, role: ColorRole) -> f32 {
        if text.is_empty() || text_cols(text) > self.m.cols(right - self.m.inner_x) {
            return right;
        }
        self.text_right(right, y, text, role)
    }

    /// Lay `parts` out left to right with a one-column gap, stopping at `right`.
    fn run_seq(&mut self, x: f32, y: f32, right: f32, parts: &[(&str, ColorRole)]) {
        let mut pen = x;
        for (text, role) in parts {
            if text.is_empty() {
                continue;
            }
            if pen >= right {
                break;
            }
            let w = self.text(pen, y, right - pen, text, *role);
            if w <= 0.0 {
                break;
            }
            pen += w + self.m.cell_w;
        }
    }

    /// A track plus its filled part. Only the *fill* is conditional — a caller that has no
    /// fraction must not call this at all (R5.5).
    fn track(&mut self, x: f32, y: f32, w: f32, thickness: f32, fraction: f32, role: ColorRole) {
        if w <= thickness {
            return;
        }
        let r = thickness * 0.5;
        let cy = y + r;
        self.shape(SidebarShape::capsule(
            x + r,
            cy,
            x + w - r,
            cy,
            thickness,
            solid(ColorRole::Hairline),
        ));
        let f = if fraction.is_finite() {
            fraction.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if f <= 0.0 {
            return;
        }
        let end = (x + w * f).max(x + r).min(x + w - r);
        self.shape(SidebarShape::capsule(
            x + r,
            cy,
            end,
            cy,
            thickness,
            solid(role),
        ));
    }

    fn panel_bg(&mut self) {
        let m = self.m;
        self.fill_rect(
            Rect {
                x: m.panel[0],
                y: m.panel[1],
                w: m.panel[2],
                h: m.panel[3],
            },
            ColorRole::PanelBg,
        );
        // The 1px left border is the only thing separating the panel from the terminal's
        // last column; without it two dark surfaces merge into one.
        let t = m.s(HAIRLINE);
        let x = m.panel[0] + t * 0.5;
        self.shape(SidebarShape::capsule(
            x,
            m.panel[1],
            x,
            m.bottom(),
            t,
            solid(ColorRole::PanelBorder),
        ));
    }

    /// Header band: accent bar, title, status pill.
    ///
    /// The design's `✕` close button is **not** drawn. D16 makes the whole band
    /// pointer-inert, so a close affordance would be a control that looks clickable and is
    /// not — the same trade D15 already rejected for the sidebar's toggle row. The panel
    /// closes with the chord that opened it (D1).
    fn header(&mut self, model: &MonitorModel, y: f32) -> f32 {
        let m = self.m;
        let h = m.line + m.s(2.0 * HEADER_PAD_Y);
        if !self.fits(y, h) || m.inner_w <= 0.0 {
            return y;
        }
        self.fill_rect(
            Rect {
                x: m.panel[0],
                y,
                w: m.panel[2],
                h,
            },
            ColorRole::HeaderBg,
        );
        let text_y = y + m.s(HEADER_PAD_Y);
        let aw = m.s(ACCENT_W);
        let ax = m.inner_x + aw * 0.5;
        self.shape(SidebarShape::capsule(
            ax,
            text_y,
            ax,
            text_y + m.line,
            aw,
            solid(ColorRole::AccentGreen),
        ));
        let pill_left = self.pill(model.status, y + h * 0.5);
        let tx = m.inner_x + aw + m.s(GAP);
        self.text(
            tx,
            text_y,
            pill_left - m.s(GAP) - tx,
            &model.title,
            ColorRole::TextPrimary,
        );
        self.rule(
            m.panel[0],
            m.panel[0] + m.panel[2],
            y + h,
            ColorRole::Hairline,
        );
        y + h
    }

    /// The status pill, right-aligned on the header line. Returns its left edge (the
    /// title's budget) — or the right margin when it does not fit, so a narrow panel
    /// spends its width on the title rather than on a badge.
    fn pill(&mut self, status: PanelStatus, cy: f32) -> f32 {
        let m = self.m;
        let right = m.right();
        let label = status.label();
        let role = status.role();
        let lw = text_w(label, m.cell_w);
        let dot = m.s(DOT_D);
        let ph = m.line + m.s(2.0 * PILL_PAD_Y);
        let pw = lw + dot + m.s(DOT_GAP + 2.0 * PILL_PAD_X);
        let left = right - pw;
        if left <= m.inner_x {
            return right;
        }
        let r = ph * 0.5;
        self.shape(SidebarShape::capsule(
            left + r,
            cy,
            right - r,
            cy,
            ph,
            solid(ColorRole::CardBg),
        ));
        let dot_x = left + m.s(PILL_PAD_X) + dot * 0.5;
        // A zero-length capsule is the constructor's filled dot — the design's status
        // light, which D2 renders as a colour rather than a pulse.
        self.shape(SidebarShape::capsule(
            dot_x,
            cy,
            dot_x,
            cy,
            dot,
            solid(role),
        ));
        self.out.runs.push(TextRun {
            rect: Rect {
                x: dot_x + dot * 0.5 + m.s(DOT_GAP),
                y: cy - m.line * 0.5,
                w: lw,
                h: m.line,
            },
            text: label.to_string(),
            color: role.packed(),
        });
        left
    }

    /// The session identity line, plus the detached wording when there is one (R5.3).
    fn session(&mut self, model: &MonitorModel, y: f32) -> f32 {
        let m = self.m;
        if m.inner_w <= 0.0 {
            return y;
        }
        let s = &model.session;
        let mut y = y + m.s(GAP);
        if !self.fits(y, m.line) {
            return y;
        }
        self.run_seq(
            m.inner_x,
            y,
            m.right(),
            &[
                (s.name.as_str(), ColorRole::TextPrimary),
                (s.short_id.as_str(), ColorRole::TextFaint),
                (s.provider.as_str(), ColorRole::Teal),
                (s.model.as_str(), ColorRole::TextMuted),
            ],
        );
        y += m.line;
        if let Some(note) = &s.note {
            if self.fits(y, m.line) {
                self.text(m.inner_x, y, m.inner_w, note, ColorRole::TextMuted);
                y += m.line;
            }
        }
        y
    }

    /// The stat grid, three tiles per row.
    fn tiles(&mut self, model: &MonitorModel, y: f32) -> f32 {
        let m = self.m;
        if model.tiles.is_empty() || m.inner_w <= 0.0 {
            return y;
        }
        let top = y + m.s(GAP);
        let gap = m.s(TILE_GAP);
        let cols = TILE_COLS as f32;
        let tw = (m.inner_w - gap * (cols - 1.0)) / cols;
        // Every tile is the same height whether or not it has a gauge, so the grid stays
        // rectangular — only CONTEXT fills the reserved gauge slot.
        let th = 2.0 * m.line + m.s(2.0 * TILE_PAD + GAUGE_GAP + GAUGE_H);
        if tw <= 0.0 {
            return y;
        }
        let mut bottom = y;
        for (i, tile) in model.tiles.iter().enumerate() {
            let row = (i / TILE_COLS) as f32;
            let col = (i % TILE_COLS) as f32;
            let ty = top + row * (th + gap);
            if !self.fits(ty, th) {
                break;
            }
            let rect = Rect {
                x: m.inner_x + col * (tw + gap),
                y: ty,
                w: tw,
                h: th,
            };
            self.tile(tile, rect);
            bottom = bottom.max(ty + th);
        }
        bottom
    }

    fn tile(&mut self, tile: &StatTile, r: Rect) {
        let m = self.m;
        self.boxed(r, ColorRole::CardBg, ColorRole::Hairline);
        let pad = m.s(TILE_PAD);
        let ix = r.x + pad;
        let iw = r.w - 2.0 * pad;
        self.text(ix, r.y + pad, iw, &tile.label, ColorRole::TextFaint);
        self.text(ix, r.y + pad + m.line, iw, &tile.value, tile.color);
        // R5.5: `None` emits no track at all. An empty track would assert "0% used",
        // which an unknown context window does not entitle the panel to say.
        if let Some(f) = tile.gauge {
            let gy = r.y + pad + 2.0 * m.line + m.s(GAUGE_GAP);
            self.track(ix, gy, iw, m.s(GAUGE_H), f, tile.color);
        }
    }

    /// The footer, pinned to the panel's bottom edge. Returns its top `y` — the ceiling
    /// the sections must stay above.
    ///
    /// The design's `q` keycap is deliberately absent (D1/R1.4): the panel consumes no
    /// keystroke but the chord, so drawing a keycap would promise a binding that does not
    /// exist and would cost a terminal user their `less` quit key if it did.
    fn footer(&mut self, model: &MonitorModel) -> f32 {
        let m = self.m;
        let h = m.line + m.s(GAP);
        let top = m.bottom() - h;
        if m.inner_w <= 0.0 || !self.fits(top, h) {
            return m.bottom();
        }
        self.rule(
            m.panel[0],
            m.panel[0] + m.panel[2],
            top,
            ColorRole::Hairline,
        );
        self.text(
            m.inner_x,
            top + m.s(GAP) * 0.5,
            m.inner_w,
            &model.footer,
            ColorRole::TextFaint,
        );
        top
    }

    /// The six content sections, sharing everything the pinned chrome left.
    fn sections(&mut self, model: &MonitorModel, y_top: f32, footer_top: f32) {
        let m = self.m;
        if m.inner_w <= 0.0 || model.sections.is_empty() {
            return;
        }
        let mut y = y_top + m.s(GAP);
        let available = footer_top - m.s(GAP) - y;
        let sm: Vec<SectionMetrics> = model
            .sections
            .iter()
            .map(|s| SectionMetrics {
                entries: s.body.len(),
                row_h: m.row_h(&s.body),
            })
            .collect();
        let grants = allocate(&sm, available, m.heading_h());
        for (section, grant) in model.sections.iter().zip(grants) {
            if !grant.heading {
                continue;
            }
            self.heading(section, y, grant.rows);
            y += m.heading_h();
            y = self.body(section, y, grant.rows);
        }
    }

    /// `LABEL ──────── +N more  12` — label left, hairline filling, marks right.
    ///
    /// Both the `+N more` marker (R8.6) and the empty-state wording ride the heading
    /// rather than taking a row. A marker that occupied a row would compete with the data
    /// it is announcing for the single row D18 guarantees the section — which is the same
    /// reason the current dashboard already places it there.
    fn heading(&mut self, section: &Section, y: f32, shown: usize) {
        let m = self.m;
        let mut edge = m.right();
        match &section.body {
            SectionBody::Empty(state) => {
                edge = self.text_right(edge, y, state, ColorRole::TextFaint);
            }
            _ => {
                edge = self.mark_right(edge, y, &section.total.to_string(), ColorRole::TextMuted);
                let hidden = section.total.saturating_sub(shown.min(section.body.len()));
                if hidden > 0 {
                    edge = self.mark_right(
                        edge - m.cell_w,
                        y,
                        &format!("+{hidden} more"),
                        ColorRole::TextFaint,
                    );
                }
            }
        }
        let label_w = self.text(
            m.inner_x,
            y,
            m.inner_w,
            &section.label,
            ColorRole::TextSecondary,
        );
        self.rule(
            m.inner_x + label_w + m.cell_w,
            edge - m.cell_w,
            y + m.line * 0.5,
            ColorRole::Hairline,
        );
    }

    fn body(&mut self, section: &Section, y: f32, rows: usize) -> f32 {
        let mut y = y;
        match &section.body {
            SectionBody::Empty(_) => y,
            SectionBody::Cards(v) => {
                for c in v.iter().take(rows) {
                    y = self.card(c, y);
                }
                y
            }
            SectionBody::Rows(v) => {
                for r in v.iter().take(rows) {
                    y = self.row(r, y);
                }
                y
            }
            SectionBody::Bars(v) => {
                for b in v.iter().take(rows) {
                    y = self.usage_bar(b, y);
                }
                y
            }
        }
    }

    fn card(&mut self, card: &Card, y: f32) -> f32 {
        let m = self.m;
        let pad = m.s(CARD_PAD);
        let h = 2.0 * m.line + 2.0 * pad;
        let advance = h + m.s(ROW_GAP);
        if !self.fits(y, h) {
            return y + advance;
        }
        self.boxed(
            Rect {
                x: m.inner_x,
                y,
                w: m.inner_w,
                h,
            },
            ColorRole::CardBg,
            ColorRole::Hairline,
        );
        let mut x = m.inner_x + pad;
        if let Some(accent) = card.accent {
            let aw = m.s(ACCENT_W);
            let ax = m.inner_x + aw * 0.5;
            self.shape(SidebarShape::capsule(
                ax,
                y + pad,
                ax,
                y + h - pad,
                aw,
                solid(accent),
            ));
            x += aw;
        }
        let right = m.right() - pad;
        let edge = self.mark_right(right, y + pad, &card.value, ColorRole::TextMuted);
        self.text(
            x,
            y + pad,
            edge - m.cell_w - x,
            &card.title,
            ColorRole::TextPrimary,
        );
        if let Some(detail) = &card.detail {
            self.text(x, y + pad + m.line, right - x, detail, ColorRole::TextFaint);
        }
        y + advance
    }

    fn row(&mut self, row: &Row, y: f32) -> f32 {
        let m = self.m;
        let advance = m.line + m.s(ROW_GAP);
        if !self.fits(y, m.line) {
            return y + advance;
        }
        let mut edge = m.right();
        if !row.value.is_empty() {
            edge = self.mark_right(edge, y, &row.value, row.value_color);
        }
        let mut x = m.inner_x;
        if let Some(label) = &row.label {
            // A fixed gutter, not the label's own width — otherwise `READ` and `WRITE`
            // start the following text in different columns and the list stops scanning.
            let gutter = LABEL_COLS as f32 * m.cell_w;
            self.text(x, y, gutter, &label.text, label.color);
            x += gutter;
        }
        if let Some(glyph) = row.glyph {
            let mut buf = [0u8; 4];
            let w = self.text(
                x,
                y,
                m.right() - x,
                glyph.encode_utf8(&mut buf),
                row.value_color,
            );
            if w > 0.0 {
                x += w + m.cell_w;
            }
        }
        self.text(
            x,
            y,
            edge - m.cell_w - x,
            &row.text,
            ColorRole::TextSecondary,
        );
        y + advance
    }

    fn usage_bar(&mut self, bar: &Bar, y: f32) -> f32 {
        let m = self.m;
        let advance = m.line + m.s(ROW_GAP);
        if !self.fits(y, m.line) {
            return y + advance;
        }
        let edge = self.mark_right(m.right(), y, &bar.count.to_string(), ColorRole::TextMuted);
        let track_w = m.s(BAR_W).min((edge - m.inner_x - 2.0 * m.cell_w).max(0.0));
        let track_x = edge - m.cell_w - track_w;
        self.text(
            m.inner_x,
            y,
            track_x - m.cell_w - m.inner_x,
            &bar.name,
            ColorRole::TextSecondary,
        );
        let th = m.s(BAR_H);
        self.track(
            track_x,
            y + (m.line - th) * 0.5,
            track_w,
            th,
            bar.fraction,
            ColorRole::Teal,
        );
        y + advance
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor_model::{RowLabel, SessionRow};

    const CELL: [f32; 2] = [8.0, 16.0];

    fn uniform(entries: &[usize], row_h: f32) -> Vec<SectionMetrics> {
        entries
            .iter()
            .map(|&entries| SectionMetrics { entries, row_h })
            .collect()
    }

    fn rows_section(label: &str, n: usize, total: usize) -> Section {
        Section {
            label: label.into(),
            total,
            body: SectionBody::Rows(
                (0..n)
                    .map(|i| Row {
                        text: format!("row{i}"),
                        ..Row::default()
                    })
                    .collect(),
            ),
        }
    }

    fn model(sections: Vec<Section>) -> MonitorModel {
        MonitorModel {
            visible: true,
            title: "ai-monitor".into(),
            status: PanelStatus::Connected,
            session: SessionRow {
                name: "ai-monitor".into(),
                short_id: "1a2b3c4d".into(),
                provider: "[Claude]".into(),
                model: "opus".into(),
                note: None,
            },
            tiles: Vec::new(),
            sections,
            footer: "Last update 16:21:18".into(),
        }
    }

    fn run_texts(draw: &PanelDraw) -> Vec<String> {
        draw.runs.iter().map(|r| r.text.clone()).collect()
    }

    // ---- R8: the height allocator ----

    #[test]
    fn r8_1_every_section_gets_a_heading_and_a_row() {
        // Chrome plus exactly 12 section lines, six populated sections: six headings and
        // six first rows, with nothing left over.
        let sm = uniform(&[4, 4, 4, 4, 4, 4], 10.0);
        let grants = allocate(&sm, 12.0 * 10.0, 10.0);
        assert!(grants.iter().all(|g| g.heading && g.rows >= 1));
        assert_eq!(grants.iter().map(|g| g.rows).sum::<usize>(), 6);
    }

    #[test]
    fn r8_2_leftover_rows_go_round_robin_not_to_the_first_section() {
        // Six leftover rows, TASKS holding 40 and TOOL USAGE 11: one row per section per
        // pass, so every section reaches 2 — greedy would have given TASKS all six.
        let sm = uniform(&[40, 4, 4, 4, 4, 11], 10.0);
        let grants = allocate(&sm, (6 + 6 + 6) as f32 * 10.0, 10.0);
        assert_eq!(
            grants.iter().map(|g| g.rows).collect::<Vec<_>>(),
            vec![2, 2, 2, 2, 2, 2]
        );
    }

    #[test]
    fn r8_3_a_section_is_capped_by_its_own_entries() {
        // Ten leftover rows and a 2-entry SKILLS (index 2): it stops at 2 and the surplus
        // reaches the sections after it.
        let sm = uniform(&[40, 40, 2, 40, 40, 40], 10.0);
        let grants = allocate(&sm, (6 + 6 + 10) as f32 * 10.0, 10.0);
        assert_eq!(grants[2].rows, 2, "capped by its entry count");
        // Nothing is wasted: all 16 rows the height affords are handed out even though
        // one section stopped taking them.
        assert_eq!(grants.iter().map(|g| g.rows).sum::<usize>(), 16);
        // The surplus reached the sections *after* the capped one — every later section
        // is past the single row phase 2 guaranteed it.
        assert!(
            grants[3..].iter().all(|g| g.rows > 1),
            "surplus stopped short: {grants:?}"
        );
    }

    #[test]
    fn r8_4_a_heading_may_be_granted_without_a_row() {
        // Six headings but only five first rows fit: the sixth renders its heading alone.
        let sm = uniform(&[4, 4, 4, 4, 4, 4], 10.0);
        let grants = allocate(&sm, (6 + 5) as f32 * 10.0, 10.0);
        assert!(grants.iter().all(|g| g.heading));
        assert_eq!(
            grants.iter().map(|g| g.rows).collect::<Vec<_>>(),
            vec![1, 1, 1, 1, 1, 0]
        );
    }

    #[test]
    fn r8_5_headings_drop_from_the_bottom_only_when_even_they_do_not_fit() {
        // Height for four headings: the first four in design order keep theirs, the last
        // two are dropped, and nobody gets a row. Reserving headings before rows is what
        // produces this — spending the height on "heading + row" in order would leave two
        // sections announced and four invisible.
        let sm = uniform(&[4, 4, 4, 4, 4, 4], 10.0);
        let grants = allocate(&sm, 4.0 * 10.0, 10.0);
        assert_eq!(
            grants.iter().map(|g| g.heading).collect::<Vec<_>>(),
            vec![true, true, true, true, false, false]
        );
        assert!(grants.iter().all(|g| g.rows == 0));
    }

    #[test]
    fn allocator_survives_degenerate_metrics() {
        // A zero-height row would spin the round-robin loop forever; a non-finite budget
        // would hand every section an unbounded grant.
        let sm = uniform(&[10, 10], 0.0);
        assert!(allocate(&sm, 100.0, 10.0).iter().all(|g| g.rows == 0));
        let sm = uniform(&[10, 10], f32::NAN);
        assert!(allocate(&sm, 100.0, 10.0).iter().all(|g| g.rows == 0));
        assert!(allocate(&uniform(&[10], 10.0), f32::INFINITY, 10.0)[0].rows == 0);
        assert!(allocate(&uniform(&[10], 10.0), -5.0, 10.0)[0] == Grant::default());
        assert!(allocate(&[], 100.0, 10.0).is_empty());
    }

    #[test]
    fn allocator_mixes_row_heights_without_starving_the_short_rows() {
        // A tall card row that does not fit must not veto the shorter rows after it.
        let sm = vec![
            SectionMetrics {
                entries: 4,
                row_h: 40.0,
            },
            SectionMetrics {
                entries: 4,
                row_h: 10.0,
            },
        ];
        let grants = allocate(&sm, 2.0 * 10.0 + 20.0, 10.0);
        assert!(grants.iter().all(|g| g.heading));
        assert_eq!(grants[0].rows, 0, "the 40px card row never fit");
        assert_eq!(grants[1].rows, 2, "the short rows still got the space");
    }

    /// A six-section model shaped like the one the app actually sends: the design's order,
    /// both card sections first, then rows, then bars. Entry texts are short and unique
    /// (`TASKS/0`, `SKILLS/1`, …) so a drawn row can be attributed to its section by name
    /// alone — real telemetry strings truncate on a narrow panel, which would make a
    /// missing row and a clipped one indistinguishable.
    fn six_sections(entries: [usize; 6]) -> MonitorModel {
        let labels = [
            "TASKS",
            "ACTIVE AGENTS",
            "SKILLS",
            "FILE ACTIVITY",
            "RECENT CALLS",
            "TOOL USAGE",
        ];
        let sections = labels
            .iter()
            .zip(entries)
            .enumerate()
            .map(|(i, (label, n))| {
                let body = match i {
                    0 | 1 => SectionBody::Cards(
                        (0..n)
                            .map(|k| Card {
                                accent: Some(ColorRole::Amber),
                                title: format!("{label}/{k}"),
                                value: "1s".into(),
                                detail: Some("d".into()),
                            })
                            .collect(),
                    ),
                    5 => SectionBody::Bars(
                        (0..n)
                            .map(|k| Bar::new(format!("{label}/{k}"), 1, 1.0))
                            .collect(),
                    ),
                    _ => SectionBody::Rows(
                        (0..n)
                            .map(|k| Row {
                                text: format!("{label}/{k}"),
                                ..Row::default()
                            })
                            .collect(),
                    ),
                };
                // `total` deliberately exceeds what is sent, so every populated section is
                // a `+N more` candidate at every height.
                Section {
                    label: (*label).into(),
                    total: n + 3,
                    body,
                }
            })
            .collect();
        let mut m = model(sections);
        m.tiles = vec![StatTile::new("UPTIME", "1h 12m", ColorRole::TextPrimary); 6];
        m
    }

    #[test]
    fn r8_1_to_r8_6_hold_at_every_height_a_real_panel_can_take() {
        // The allocator's own unit tests above feed it synthetic metrics. This one runs
        // the whole path — model → `SectionMetrics` → `allocate` → drawn primitives — at
        // every height in a wide sweep, and asserts D18's invariants on what actually
        // reached the glass. A rule that held for uniform 10px rows and broke for a
        // 2-line card beside a 1-line bar would pass there and fail here.
        let labels = [
            "TASKS",
            "ACTIVE AGENTS",
            "SKILLS",
            "FILE ACTIVITY",
            "RECENT CALLS",
            "TOOL USAGE",
        ];
        let entries = [40, 4, 2, 6, 12, 9];
        let m = six_sections(entries);
        let mut saw_heading_without_a_row = false;
        let mut saw_a_dropped_heading = false;
        let mut saw_every_section_fed = false;

        for h in (120..1800).step_by(7) {
            let draw = layout(&m, [0.0, 0.0, 430.0, h as f32], 1.0, CELL);
            let texts = run_texts(&draw);
            let headed: Vec<bool> = labels
                .iter()
                .map(|l| texts.iter().any(|t| t == l))
                .collect();
            let rows: Vec<usize> = labels
                .iter()
                .map(|l| {
                    texts
                        .iter()
                        .filter(|t| t.starts_with(&format!("{l}/")))
                        .count()
                })
                .collect();

            // R8.5 — the headings that survive are a prefix of the design order. A hole in
            // the middle would mean a section vanished while a lower-priority one stayed.
            let kept = headed.iter().filter(|h| **h).count();
            assert_eq!(
                headed,
                (0..6).map(|i| i < kept).collect::<Vec<_>>(),
                "h={h}: headings must drop from the bottom, got {headed:?}"
            );
            saw_a_dropped_heading |= kept < 6;
            saw_every_section_fed |= kept == 6 && rows.iter().all(|r| *r >= 1);

            for i in 0..6 {
                // Nothing may draw a row it was not given a heading for...
                assert!(
                    headed[i] || rows[i] == 0,
                    "h={h}: {} drew a headless row",
                    labels[i]
                );
                // ... and R8.3: no section exceeds what it actually holds.
                assert!(
                    rows[i] <= entries[i],
                    "h={h}: {} drew {} of {} entries",
                    labels[i],
                    rows[i],
                    entries[i]
                );
                // R8.4 — a heading with no row is a valid state, and it happens.
                saw_heading_without_a_row |= headed[i] && rows[i] == 0;

                // R8.6 — a section showing fewer than its total says so ON the heading.
                if headed[i] && rows[i] < entries[i] + 3 {
                    let marker = format!("+{} more", entries[i] + 3 - rows[i]);
                    let heading_y = draw
                        .runs
                        .iter()
                        .find(|r| r.text == labels[i])
                        .map(|r| r.rect.y)
                        .unwrap_or_else(|| panic!("h={h}: {} heading not drawn", labels[i]));
                    // Matched on the heading's own line, never merely somewhere in the
                    // draw: two sections hiding the same number emit the same marker
                    // text, so a global search would let one section's marker vouch for
                    // another's. The marker is dropped rather than clipped on a panel too
                    // narrow for it — but this one is 430px, so it is always drawn.
                    assert!(
                        draw.runs
                            .iter()
                            .any(|r| r.text == marker && (r.rect.y - heading_y).abs() < FIT_EPS),
                        "h={h}: {} lost `{marker}` off its heading",
                        labels[i]
                    );
                }
            }

            // R8.2 — one row per section per pass, so among sections still taking rows the
            // counts differ by at most one. Greedy would put every spare row in the
            // 40-entry TASKS and leave the other five at one apiece.
            //
            // Grouped by row height, because the comparison is only meaningful within a
            // group: a card is two lines and a list row is one, and the allocator
            // deliberately lets a short row through in a pass where the tall one no
            // longer fits (`continue`, not `break`). Comparing across the two would read
            // that as starvation when it is the tall section running out of room.
            for tall in [true, false] {
                let live: Vec<usize> = (0..6)
                    .filter(|&i| (i < 2) == tall && headed[i] && rows[i] < entries[i])
                    .map(|i| rows[i])
                    .collect();
                if let (Some(lo), Some(hi)) = (live.iter().min(), live.iter().max()) {
                    assert!(
                        hi - lo <= 1,
                        "h={h}: rows {rows:?} are not round-robin among the unexhausted"
                    );
                }
            }
        }

        assert!(saw_heading_without_a_row, "R8.4 was never exercised");
        assert!(saw_a_dropped_heading, "R8.5 was never exercised");
        assert!(saw_every_section_fed, "R8.1 was never exercised");
    }

    // ---- R0.5 / D15: the left sidebar gained no ai-monitor affordance ----

    /// R0.5, as a **compile-time** assertion rather than a runtime one.
    ///
    /// The design puts a `⌘ M  ai-monitor` row at the bottom of the left sidebar. D15 put
    /// it out of scope, and named exactly what building it would cost: "a new
    /// `ChromeRegion`, a `hit_test` branch and layout changes in `sidebar_layout.rs` — a
    /// second feature wearing this one's clothes". So the check is on that cost, not on
    /// the pixels: a sidebar affordance is a *pressable target*, and a pressable target in
    /// this codebase is a region variant. There is no way to add one and leave both
    /// matches below exhaustive.
    ///
    /// It lives in the panel's own module deliberately. Putting it in
    /// `sidebar_layout.rs` would mean editing the file whose untouchedness is the claim —
    /// and the claim is checkable in a diff (`git diff -- crates/renderer/src/sidebar_*.rs`
    /// shows only a z-order constant repointed from the deleted `overlay_layer.rs` and one
    /// `right_inset: 0.0` added to a test's `FrameCtx`), which is the form R0.5 asks for.
    /// This is the half a diff cannot carry forward: it keeps failing after the diff is
    /// merged and forgotten.
    #[test]
    fn r0_5_the_sidebar_has_no_ai_monitor_region() {
        use crate::sidebar_layout::{ChromeRegion, SidebarRegion};
        // Exhaustive, with no `_` arm: a fourth chrome target — which is what an
        // `ai-monitor` row would be — stops this compiling.
        for region in [
            ChromeRegion::NewTab,
            ChromeRegion::HideSidebar,
            ChromeRegion::Restore,
        ] {
            match region {
                ChromeRegion::NewTab | ChromeRegion::HideSidebar | ChromeRegion::Restore => {}
            }
        }
        // And the outer region enum, because an affordance could be added beside
        // `Chrome` rather than inside it.
        let sample = SidebarRegion::Chrome(ChromeRegion::NewTab);
        match sample {
            SidebarRegion::Chrome(_)
            | SidebarRegion::WaitingRow(_)
            | SidebarRegion::MenuButton(_)
            | SidebarRegion::MenuItem(_)
            | SidebarRegion::Card(_)
            | SidebarRegion::GrabEdge
            | SidebarRegion::Panel => {}
        }
        // The panel is reachable by Cmd+Shift+M alone (D1), which is what makes dropping
        // the row a scope decision rather than a lost feature.
    }

    // ---- R1.4: the design's `q` keycap is not drawn ----

    #[test]
    fn r1_4_the_footer_band_draws_the_footer_string_and_nothing_else() {
        // D1 dropped the design's `q` keycap. The keycap is a *claim* — it says pressing
        // `q` closes the panel — and the panel does not answer to `q`: it goes to the
        // shell, where it quits `less` (R1.3). A control that looks like a control and is
        // not is worse than no control.
        //
        // `dashboard.rs` pins the model's footer string as free of any close affordance.
        // This is the other half: that the *layout* adds none of its own. It is the half
        // that can go wrong independently, because a keycap is a shape plus a glyph and
        // the layout is the only place either can be emitted.
        let m = model(vec![rows_section("TASKS", 3, 3)]);
        let panel = [0.0, 0.0, 430.0, 400.0];
        let draw = layout(&m, panel, 1.0, CELL);

        // The footer band: the bottom line box of the panel. Everything drawn at or below
        // its top belongs to the footer.
        let footer_run = draw
            .runs
            .iter()
            .find(|r| r.text == m.footer)
            .expect("premise: the footer is drawn at all");
        let band_top = footer_run.rect.y;
        let in_band: Vec<&str> = draw
            .runs
            .iter()
            .filter(|r| r.rect.y >= band_top)
            .map(|r| r.text.as_str())
            .collect();
        assert_eq!(
            in_band,
            vec![m.footer.as_str()],
            "the footer band must carry the last-update time alone — a keycap glyph, an \
             `Esc` hint or a `✕` would show up here as a second run"
        );
        // And no keycap *box* either: a `q` in a rounded rect is a shape before it is a
        // glyph, so counting runs alone would miss an empty keycap. Nothing starts inside
        // the footer's line box — the hairline rule sits above it and the panel background
        // starts at the panel's top.
        let shapes_in_band: Vec<[f32; 4]> = draw
            .shapes
            .iter()
            .filter(|s| s.bbox[1] >= band_top)
            .map(|s| s.bbox)
            .collect();
        assert!(
            shapes_in_band.is_empty(),
            "a shape begins inside the footer's line box: {shapes_in_band:?} — the only \
             thing the design puts there is the `q` keycap D1 dropped"
        );
    }

    // ---- R8.6: `+N more` rides the heading ----

    #[test]
    fn r8_6_more_marker_is_drawn_on_the_heading_not_as_a_row() {
        // A short panel so the section cannot draw all five entries.
        let m = model(vec![rows_section("TASKS", 5, 5)]);
        let draw = layout(&m, [0.0, 0.0, 430.0, 190.0], 1.0, CELL);
        let texts = run_texts(&draw);
        let drawn = (0..5)
            .filter(|i| texts.iter().any(|t| t == &format!("row{i}")))
            .count();
        assert!(
            (1..5).contains(&drawn),
            "the test panel must clip some rows; drew {drawn}"
        );
        let marker = format!("+{} more", 5 - drawn);
        let marker_run = draw
            .runs
            .iter()
            .find(|r| r.text == marker)
            .unwrap_or_else(|| panic!("expected `{marker}` among {texts:?}"));
        let heading = draw
            .runs
            .iter()
            .find(|r| r.text == "TASKS")
            .expect("heading label drawn");
        assert_eq!(
            marker_run.rect.y, heading.rect.y,
            "the marker shares the heading's line, never a data row's"
        );
        // And it is right of the label, filling toward the count.
        assert!(marker_run.rect.x > heading.rect.x + heading.rect.w);
    }

    #[test]
    fn an_empty_section_states_itself_on_the_heading() {
        // Same placement rule: an empty state must not spend the row a populated
        // neighbour was guaranteed.
        let m = model(vec![Section {
            label: "SKILLS".into(),
            total: 0,
            body: SectionBody::Empty("none yet".into()),
        }]);
        let draw = layout(&m, [0.0, 0.0, 430.0, 400.0], 1.0, CELL);
        let state = draw
            .runs
            .iter()
            .find(|r| r.text == "none yet")
            .expect("empty state drawn");
        let heading = draw
            .runs
            .iter()
            .find(|r| r.text == "SKILLS")
            .expect("heading drawn");
        assert_eq!(state.rect.y, heading.rect.y);
    }

    // ---- degenerate panels ----

    #[test]
    fn a_degenerate_panel_emits_nothing_instead_of_panicking() {
        let m = model(vec![rows_section("TASKS", 3, 3)]);
        for panel in [
            [0.0, 0.0, 0.0, 400.0],
            [0.0, 0.0, 430.0, 0.0],
            [0.0, 0.0, -430.0, 400.0],
            [0.0, 0.0, f32::NAN, 400.0],
        ] {
            let draw = layout(&m, panel, 1.0, CELL);
            assert!(
                draw.shapes.is_empty() && draw.runs.is_empty(),
                "panel {panel:?} drew something"
            );
        }
        // A zero cell width would divide by zero in every column budget.
        assert_eq!(
            layout(&m, [0.0, 0.0, 430.0, 400.0], 1.0, [0.0, 16.0]),
            PanelDraw::default()
        );
        // An invisible model draws nothing at all — not even the background.
        let hidden = MonitorModel {
            visible: false,
            ..m.clone()
        };
        assert_eq!(
            layout(&hidden, [0.0, 0.0, 430.0, 400.0], 1.0, CELL),
            PanelDraw::default()
        );
    }

    #[test]
    fn a_panel_too_narrow_for_content_still_paints_its_band() {
        // inner_w ≤ 0: the chrome has nowhere to go, but the band must still be filled or
        // the terminal shows through the reserved columns.
        let m = model(vec![rows_section("TASKS", 3, 3)]);
        let draw = layout(&m, [0.0, 0.0, 12.0, 400.0], 1.0, CELL);
        assert!(!draw.shapes.is_empty(), "background and border still drawn");
        assert!(draw.runs.is_empty(), "no text fits");
    }

    // ---- R5.4 / R5.5: the context gauge ----

    #[test]
    fn r5_5_an_absent_gauge_emits_no_track_at_all() {
        let tile = |gauge: Option<f32>| {
            let mut m = model(Vec::new());
            m.tiles = vec![StatTile {
                label: "CONTEXT".into(),
                value: "45%".into(),
                color: ColorRole::Teal,
                gauge,
            }];
            layout(&m, [0.0, 0.0, 430.0, 400.0], 1.0, CELL).shapes.len()
        };
        let none = tile(None);
        let zero = tile(Some(0.0));
        let some = tile(Some(0.45));
        assert_eq!(zero, none + 1, "Some(0.0) draws the track and no fill");
        assert_eq!(some, none + 2, "Some(0.45) draws the track and its fill");
    }

    #[test]
    fn r5_4_the_gauge_fill_is_proportional() {
        let mut m = model(Vec::new());
        m.tiles = vec![StatTile::new("CONTEXT", "45%", ColorRole::Teal).with_gauge(0.45)];
        let draw = layout(&m, [0.0, 0.0, 430.0, 400.0], 1.0, CELL);
        // The gauge is the only pair of horizontal capsules at half the gauge thickness
        // (the header accent shares the thickness but runs vertically; the rules are
        // thinner).
        let gauge: Vec<&SidebarShape> = draw
            .shapes
            .iter()
            .filter(|s| {
                s.kind == crate::sidebar_shapes::KIND_CAPSULE
                    && (s.geom[0] - GAUGE_H * 0.5).abs() < FIT_EPS
                    && (s.seg[1] - s.seg[3]).abs() < FIT_EPS
            })
            .collect();
        assert_eq!(gauge.len(), 2, "a track and its fill");
        let span = |s: &SidebarShape| s.seg[2] - s.seg[0];
        let ratio = span(gauge[1]) / span(gauge[0]);
        assert!(
            (0.35..0.55).contains(&ratio),
            "fill/track = {ratio}, expected ~0.45"
        );
    }

    // ---- column-based measurement ----

    #[test]
    fn hangul_is_measured_in_columns_not_chars() {
        // `한` is 2 columns: a 6-char title reserves 12 cells, and a char-counting rule
        // would reserve 6 and overlap every glyph with the value beside it.
        let title = "한글한글한글";
        let m = model(vec![Section {
            label: "TASKS".into(),
            total: 1,
            body: SectionBody::Rows(vec![Row {
                text: title.into(),
                ..Row::default()
            }]),
        }]);
        let draw = layout(&m, [0.0, 0.0, 430.0, 400.0], 1.0, CELL);
        let run = draw
            .runs
            .iter()
            .find(|r| r.text == title)
            .expect("the whole title fits a 430px panel");
        assert_eq!(run.rect.w, 12.0 * CELL[0]);

        // Narrow it until the row's budget clips: the prefix is cut on a column budget,
        // so it holds half as many chars as columns and never splits a codepoint.
        let narrow = layout(&m, [0.0, 0.0, 90.0, 400.0], 1.0, CELL);
        let clipped = narrow
            .runs
            .iter()
            .find(|r| !r.text.is_empty() && title.starts_with(r.text.as_str()))
            .expect("a clipped prefix of the title");
        assert!(clipped.text.chars().count() < 6);
        assert_eq!(
            clipped.rect.w,
            text_cols(&clipped.text) as f32 * CELL[0],
            "width follows columns, not chars"
        );
        assert_eq!(clipped.text.chars().count() * 2, text_cols(&clipped.text));
    }

    #[test]
    fn every_run_and_shape_stays_inside_the_panel_band() {
        // There is no scissor rect: a primitive escaping the band paints over the
        // terminal's own columns.
        let mut m = model(vec![
            rows_section("TASKS", 6, 20),
            Section {
                label: "AGENTS".into(),
                total: 2,
                body: SectionBody::Cards(vec![
                    Card {
                        accent: Some(ColorRole::Amber),
                        title: "explore".into(),
                        value: "12s".into(),
                        detail: Some("reading crates/renderer".into()),
                    };
                    2
                ]),
            },
            Section {
                label: "TOOL USAGE".into(),
                total: 3,
                body: SectionBody::Bars(vec![
                    Bar::new("bash", 76, 1.0),
                    Bar::new("read", 27, 27.0 / 76.0),
                    Bar::new("edit", 3, 3.0 / 76.0),
                ]),
            },
        ]);
        m.session.note = Some("no Claude session in this tab".into());
        m.tiles = vec![
            StatTile::new("UPTIME", "1h 12m", ColorRole::TextPrimary),
            StatTile::new("IDLE", "6s", ColorRole::TextPrimary),
            StatTile::new("ERRORS", "3", ColorRole::Rose),
            StatTile::new("IN·OUT", "12k·4k", ColorRole::TextPrimary),
            StatTile::new("CACHE", "88k", ColorRole::Teal),
            StatTile::new("CONTEXT", "45%", ColorRole::Teal).with_gauge(0.45),
        ];
        let panel = [1370.0, 0.0, 430.0, 900.0];
        let draw = layout(&m, panel, 2.0, [17.0, 34.0]);
        let (l, t, r, b) = (panel[0], panel[1], panel[0] + panel[2], panel[1] + panel[3]);
        for run in &draw.runs {
            assert!(
                run.rect.x >= l - FIT_EPS && run.rect.x + run.rect.w <= r + FIT_EPS,
                "run {:?} escapes horizontally",
                run.text
            );
            assert!(
                run.rect.y >= t - FIT_EPS && run.rect.y + run.rect.h <= b + FIT_EPS,
                "run {:?} escapes vertically",
                run.text
            );
        }
        for s in &draw.shapes {
            assert!(
                s.bbox[0] >= l - 1.0 && s.bbox[0] + s.bbox[2] <= r + 1.0,
                "shape {:?} escapes horizontally",
                s.bbox
            );
            assert!(
                s.bbox[1] >= t - 1.0 && s.bbox[1] + s.bbox[3] <= b + 1.0,
                "shape {:?} escapes vertically",
                s.bbox
            );
        }
        // R5.1: every pinned region is present at this height. The tile label is matched
        // by prefix — a 116px tile column clips `CONTEXT`, which is the layout working,
        // not failing.
        let texts = run_texts(&draw);
        for expected in ["ai-monitor", "CONNECTED", "Last update 16:21:18"] {
            assert!(texts.iter().any(|t| t == expected), "missing {expected}");
        }
        assert!(
            texts
                .iter()
                .any(|t| "CONTEXT".starts_with(t.as_str()) && !t.is_empty()),
            "the context tile's label is missing from {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| !t.is_empty() && "no Claude session in this tab".starts_with(t.as_str())),
            "R5.3: the detached wording reaches the session row"
        );
    }

    #[test]
    fn the_verb_gutter_is_fixed_width_so_the_column_lines_up() {
        // R5.6's verbs differ in length; the text after them must still start in the same
        // column, or the list stops scanning as a column.
        let rows = vec![
            Row {
                label: Some(RowLabel {
                    text: "WRITE".into(),
                    color: ColorRole::Magenta,
                }),
                text: "src/main.rs".into(),
                ..Row::default()
            },
            Row {
                label: Some(RowLabel {
                    text: "READ".into(),
                    color: ColorRole::Teal,
                }),
                text: "src/lib.rs".into(),
                ..Row::default()
            },
        ];
        let m = model(vec![Section {
            label: "FILE ACTIVITY".into(),
            total: 2,
            body: SectionBody::Rows(rows),
        }]);
        let draw = layout(&m, [0.0, 0.0, 430.0, 400.0], 1.0, CELL);
        let x_of = |t: &str| {
            draw.runs
                .iter()
                .find(|r| r.text == t)
                .map(|r| r.rect.x)
                .expect("run drawn")
        };
        assert_eq!(x_of("src/main.rs"), x_of("src/lib.rs"));
        assert_eq!(x_of("WRITE"), x_of("READ"));
        // The verbs keep their own colours (R5.6).
        let color_of = |t: &str| {
            draw.runs
                .iter()
                .find(|r| r.text == t)
                .map(|r| r.color)
                .expect("run drawn")
        };
        assert_eq!(color_of("WRITE"), ColorRole::Magenta.packed());
        assert_eq!(color_of("READ"), ColorRole::Teal.packed());
    }

    #[test]
    fn the_status_pill_carries_the_state_colour() {
        // R5.2 / D2 — three states, distinguished by colour rather than motion.
        for (status, label, role) in [
            (PanelStatus::Connected, "CONNECTED", ColorRole::LiveLime),
            (PanelStatus::Idle, "IDLE", ColorRole::Amber),
            (PanelStatus::NoSession, "NO SESSION", ColorRole::TextFaint),
        ] {
            let mut m = model(Vec::new());
            m.status = status;
            let draw = layout(&m, [0.0, 0.0, 430.0, 400.0], 1.0, CELL);
            let run = draw
                .runs
                .iter()
                .find(|r| r.text == label)
                .unwrap_or_else(|| panic!("{label} pill drawn"));
            assert_eq!(run.color, role.packed());
        }
    }
}
