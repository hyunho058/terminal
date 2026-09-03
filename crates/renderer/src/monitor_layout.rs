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

/// A bag of primitives — shapes and the runs drawn over them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Primitives {
    pub shapes: Vec<SidebarShape>,
    pub runs: Vec<TextRun>,
}

/// A half-open `[start, end)` slice of one of [`Primitives`]' vectors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Where one ladder rung's primitives live inside [`PanelDraw::content`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemSpans {
    pub shapes: Span,
    pub runs: Span,
}

/// Everything one frame of the panel draws — **stage one** of the two-stage shaping
/// contract (ai-monitor-panel-scroll D24/D38).
///
/// The split is not cosmetic. Stage two has to perform three *different* operations on
/// three *disjoint* subsets — translate-and-cull the document, copy one heading into the
/// sticky slot, and synthesise the footer line — and a flat `{shapes, runs}` pair cannot
/// address any of them. In particular a flat bag would make stage two translate the pinned
/// chrome along with the content, which is C2/D3 head-on.
///
/// Two coordinate spaces meet here, deliberately:
///
/// * [`chrome`](Self::chrome) is **panel-absolute** and final. Stage two neither moves nor
///   culls it, which is what makes "the chrome never flows" (R2.1) a property of the type
///   rather than of a reviewer's attention.
/// * [`content`](Self::content) is **document-local**: `y = 0` is the top of the first
///   heading, so the whole document is laid out once at its natural positions and stage two
///   is a pure `y += viewport.y - offset` plus a cull. Its `x` geometry is already
///   panel-relative, which is what lets a heading be copied into the slot by translation
///   alone (D36).
///
/// The footer *text* is deliberately absent — see [`PanelDraw::items`] and D38: stage two
/// always synthesises that one line, because it is the line the badge replaces (D34) and
/// hunting for it by `y` afterwards is the fragile implementation that structure exists to
/// prevent. The footer's hairline rule *is* here, in `chrome`.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelDraw {
    /// Pinned regions, panel-absolute: background, header band, session row, stat tiles,
    /// and the footer's hairline.
    pub chrome: Primitives,
    /// The scrollable document, laid out document-local from `y = 0`.
    pub content: Primitives,
    /// One entry per rung of [`item_ladder`], in the same order.
    pub items: Vec<ItemSpans>,
    /// The ladder itself, and each rung's height — carried here rather than recomputed by
    /// every caller so that `items[i]` and `ladder[i]` are the *same* `i` by construction
    /// (C11). Two independently derived enumerations that drift apart put the scroll a rung
    /// away from where the user asked, silently.
    pub ladder: Vec<Item>,
    pub heights: Vec<f32>,
    /// The band the sticky heading is copied into, panel-absolute. Zero-height when even
    /// one heading line does not fit (D31).
    pub slot: Rect,
    /// The band content may occupy, panel-absolute — **below** `slot`. Zero-height when the
    /// section area cannot take a rung.
    pub viewport: Rect,
    /// Where the footer's single line of text goes, panel-absolute. Stage two writes either
    /// the last-update string or the badge here (D34). Zero-height when the footer band did
    /// not fit, in which case stage two writes nothing.
    pub footer_text: Rect,
    /// How many columns that line has. Carried because the panel is drag-resizable and stage
    /// two has to choose between a long badge, a short one and nothing at all — a decision
    /// that needs the width in the same units the truncation uses.
    pub footer_cols: usize,
}

impl Default for PanelDraw {
    fn default() -> PanelDraw {
        PanelDraw {
            chrome: Primitives::default(),
            content: Primitives::default(),
            items: Vec::new(),
            ladder: Vec::new(),
            heights: Vec::new(),
            slot: NO_BAND,
            viewport: NO_BAND,
            footer_text: NO_BAND,
            footer_cols: 0,
        }
    }
}

/// The empty band. `Rect` is `sidebar_layout`'s and deliberately not `Default` (a zero rect
/// is a real geometric claim there), so the "no room at all" state is named here instead.
pub const NO_BAND: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 0.0,
    h: 0.0,
};

/// What one rung of the **item ladder** is (ai-monitor-panel-scroll D32/R1.3).
///
/// The ladder is the coordinate space the whole scroll feature counts in: D17 made the
/// scroll offset an *item index* rather than a pixel offset, so "what is item `i`" has to
/// have exactly one answer. C11 makes that answer live here — `app` calls this rather than
/// enumerating a second time, because two counts that disagree put the scroll somewhere the
/// user did not ask for, and they disagree *silently*.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    /// A section's `LABEL ──── marks` line. **A heading is a rung even for an empty
    /// section** — the empty-state wording rides the heading (`SectionBody::Empty`), so the
    /// section is still visible, still jumpable (D16), and still needs an index.
    Heading,
    /// One TASKS / ACTIVE AGENTS card. **One rung although it draws two lines** — a card
    /// reserves its detail line whether or not it has one, so it is a single indivisible
    /// step of the ladder.
    Card,
    /// One SKILLS / FILE ACTIVITY / RECENT CALLS row.
    Row,
    /// One TOOL USAGE bar.
    Bar,
    /// A section's fold control — `▸ N more` when closed, `▾ collapse` when open.
    ///
    /// A rung like any other: it can be scrolled to, landed on and clicked. It exists only
    /// for a [`Section::foldable`] section, and it exists in **both** fold states — a
    /// control that vanished once you opened it would leave no way back.
    ///
    /// This is not `+N more` returning. That mark was a *report* of rows the height
    /// allocator had already thrown away, with no way to reach them; this is a *control*,
    /// and the rows behind it are one click away.
    Fold,
}

/// One rung: which section it belongs to, what shape it is, and — for a body item — which
/// entry of that section's body it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item {
    /// Index into [`MonitorModel::sections`].
    pub section: usize,
    pub kind: ItemKind,
    /// Index into the section's body, or `None` for [`ItemKind::Heading`].
    pub entry: Option<usize>,
}

impl Item {
    pub fn is_heading(&self) -> bool {
        self.kind == ItemKind::Heading
    }

    /// A section's fold control rather than content.
    pub fn is_fold(&self) -> bool {
        self.kind == ItemKind::Fold
    }

    /// One entry of a section's body — the rungs that carry the data. Heading and fold
    /// rungs are chrome for the section, and both answer `false`.
    pub fn is_body(&self) -> bool {
        self.entry.is_some()
    }
}

/// Enumerate every rung of the document, in the model's section order (R1.2/R1.3).
///
/// Pure and GPU-free by construction — it reads the model and nothing else, which is what
/// lets both the scroll math and the app's anchor bookkeeping share one definition without
/// either of them needing a frame.
///
/// Deliberately **not** height-aware: heights need `scale`/`cell` and change on a DPI or
/// font change, while the identity of item `i` does not. Splitting them keeps the identity
/// stable across a resize — which is exactly what an anchor (D8/D19) needs.
pub fn item_ladder(model: &MonitorModel) -> Vec<Item> {
    let mut out = Vec::new();
    for (section, s) in model.sections.iter().enumerate() {
        out.push(Item {
            section,
            kind: ItemKind::Heading,
            entry: None,
        });
        let kind = match &s.body {
            // An empty section contributes its heading and nothing else. `len()` is 0 for
            // `Empty`, so the loop below would not run anyway — the match arm is here to
            // make that a stated fact rather than an accident of `len()`.
            SectionBody::Empty(_) => continue,
            SectionBody::Cards(_) => ItemKind::Card,
            SectionBody::Rows(_) => ItemKind::Row,
            SectionBody::Bars(_) => ItemKind::Bar,
        };
        // `shown()`, not `len()`: a closed fold makes the hidden entries not merely
        // undrawn but **absent from the coordinate space**. That is the whole reason the
        // fold state lives in the model — the ladder is the document, and a folded
        // document is a shorter one.
        for entry in 0..s.shown() {
            out.push(Item {
                section,
                kind,
                entry: Some(entry),
            });
        }
        if s.foldable() {
            out.push(Item {
                section,
                kind: ItemKind::Fold,
                entry: None,
            });
        }
    }
    out
}

/// Heights of every rung, parallel to [`item_ladder`] — what `monitor_scroll` measures with.
///
/// A separate pass rather than a field on [`Item`] because heights depend on `scale`/`cell`
/// and rung *identity* does not: an anchor (D8/D19) has to survive a DPI or font change, and
/// it does exactly because the thing it names carries no pixels.
pub fn item_heights(model: &MonitorModel, scale: f64, cell: [f32; 2]) -> Vec<f32> {
    let Some(m) = Metrics::new([0.0, 0.0, 1.0, 1.0], scale, cell) else {
        return vec![0.0; item_ladder(model).len()];
    };
    item_ladder(model)
        .iter()
        .map(|item| match model.sections.get(item.section) {
            None => 0.0,
            Some(section) => match item.kind {
                ItemKind::Heading => m.heading_h(),
                // One text line, like a row — never the body's own row height, because a
                // `Cards` body's rung is two lines tall and the control is not a card.
                ItemKind::Fold => m.fold_h(),
                _ => m.row_h(&section.body),
            },
        })
        .collect()
}

/// The height of the **sticky slot** — one heading line (D31).
///
/// The content viewport starts *below* this, which is what makes the slot un-overdrawable
/// without a scissor rect: shapes are composited under glyphs in `monitor_panel`, so a
/// heading background could never hide a row's glyphs passing beneath it. Reserving the
/// band instead means nothing ever passes beneath it.
///
/// Returns `0.0` for degenerate metrics rather than a NaN that would poison every later
/// comparison.
pub fn slot_h(scale: f64, cell: [f32; 2]) -> f32 {
    match Metrics::new([0.0, 0.0, 1.0, 1.0], scale, cell) {
        Some(m) => m.heading_h(),
        None => 0.0,
    }
}

/// The height of one rung, in physical px (R1.3).
///
/// `0.0` for a rung that names a section the model no longer has, and for degenerate
/// metrics — the scroll math treats a non-positive height as "cannot be a stop", so a stale
/// index degrades into an unreachable rung instead of a panic.
pub fn item_h(item: &Item, model: &MonitorModel, scale: f64, cell: [f32; 2]) -> f32 {
    let Some(m) = Metrics::new([0.0, 0.0, 1.0, 1.0], scale, cell) else {
        return 0.0;
    };
    let Some(section) = model.sections.get(item.section) else {
        return 0.0;
    };
    match item.kind {
        ItemKind::Heading => m.heading_h(),
        _ => m.row_h(&section.body),
    }
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
        to_content: false,
    };
    p.panel_bg();
    // D18 step 1: the pinned chrome is laid out before anything elastic, so the sections
    // divide what is genuinely left rather than pushing the footer off the bottom.
    //
    // The footer is pinned to the bottom, so its top edge is the ceiling for *everything
    // else the panel draws* — the header band and the session row as much as the stat grid
    // and the sections. Resolved before any of them (it is a pure function of the panel
    // rect, so nothing has to be drawn to know it): a region that only asks "am I inside
    // the panel?" stacks downward from the top and paints across the pinned footer on a
    // short panel instead of yielding to it. That is **one** defect with several sites, and
    // fixing it at the grid alone left the session row with the identical bug — its glyphs
    // ran through the footer's at every height in `[64.8, 87.2)` px at 430px wide, and from
    // `26.4px` up in the band where the header did not fit and the row took the panel's top.
    // "The footer survives any height" has to mean legible, not merely emitted.
    let footer_top = p.footer_top();
    // The session row rides the header: `None` is "the header band did not fit", and the
    // session must not float up to the panel's top edge to take the space. A bare identity
    // line under no header reads as a stray row rather than a panel — and it would
    // *vanish* again as the window grew and the header reclaimed the top, which is exactly
    // the take-a-region-away-on-a-drag that the height sweep forbids for the tiles.
    let y = match p.header(model, m.panel[1], footer_top) {
        Some(y) => p.session(model, y, footer_top),
        None => m.panel[1],
    };
    let y = p.tiles(model, y, footer_top);
    p.footer(model);

    // D31: the content viewport starts BELOW a one-heading-line slot. Reserving the band is
    // what makes the sticky heading drawable at all — shapes composite *under* glyphs
    // (`monitor_panel::record` runs the shape pass then the glyph pass), so a heading's
    // background could never hide a row's glyphs sliding beneath it, and C1 forbids the
    // scissor rect that would. Nothing passes beneath it because nothing is allowed there.
    let area_top = y + m.s(GAP);
    let area_bottom = footer_top - m.s(GAP);
    let slot_h = m.heading_h();
    let (slot, viewport) = if area_bottom - area_top + FIT_EPS >= slot_h {
        (
            Rect {
                x: m.panel[0],
                y: area_top,
                w: m.panel[2],
                h: slot_h,
            },
            Rect {
                x: m.panel[0],
                y: area_top + slot_h,
                w: m.panel[2],
                h: (area_bottom - area_top - slot_h).max(0.0),
            },
        )
    } else {
        // Shorter than one heading line. R3.7: draw no section area at all rather than a
        // partial slot — the pinned chrome still stands, which is what C2 protects.
        (NO_BAND, NO_BAND)
    };
    p.out.slot = slot;
    p.out.viewport = viewport;
    p.out.ladder = item_ladder(model);
    p.out.heights = item_heights(model, scale, cell);
    // Unconditional, even when the bands above came out empty. `items` must stay parallel
    // to `item_ladder(model)` at *every* panel size or the app and the renderer disagree
    // about what item `i` is — the silent mismatch C11 exists to prevent. Whether anything
    // is drawn is stage two's call, and it makes it from `viewport.h`.
    p.document(model);
    p.out
}

/// **Stage two** (D24): turn stage one's document into the primitives one frame draws, for a
/// given scroll position.
///
/// Three distinct operations on three disjoint subsets, which is precisely why
/// [`PanelDraw`] is structured rather than flat:
///
/// 1. **Chrome** is copied through untouched — not translated, not culled (C2/R2.1/R10.6).
/// 2. **Content** is translated by `viewport.y - document_y(top)` and culled to whole rungs
///    that fit the viewport. The unit of culling is the **rung**, walked in ladder order —
///    a rung is copied entire or not at all, so D17's snap holds by construction. (Where the
///    range *ends* is still a pixel test: the first rung whose bottom passes the viewport
///    stops the walk.)
/// 3. **The footer line** is synthesised here, because stage one deliberately does not emit
///    it (D38) — this is the line the badge replaces (D34).
///
/// Cheap on purpose: it moves `y` on already-shaped primitives and re-shapes nothing
/// (C3/R10.2). `TextRun` carries its own rect, and glyph UVs are independent of it.
///
/// `top` is a ladder index; out-of-range values clamp rather than panic, because the app can
/// legitimately hold a stale index for the frame between a model swap and its re-clamp.
pub fn compose(draw: &PanelDraw, model: &MonitorModel, top: usize, badge: u32) -> Primitives {
    let mut out = draw.chrome.clone();

    if draw.slot.h > 0.0 && !draw.items.is_empty() {
        let top = top.min(draw.items.len() - 1);

        // --- the sticky slot (D5/D18/D31/R4.1-R4.4) --------------------------------------
        //
        // The heading of whatever section `top` is inside, copied into the reserved band by
        // **translation alone** — never re-laid-out and never re-shaped (D36/R4.3). That
        // works because a heading's `x` geometry is entirely panel-relative: the label sits
        // at `inner_x`, the marks are right-aligned to `right()`, and the hairline fills
        // between them. Only `y` ever needed to change.
        //
        // Both the glyph runs and the rule *shape* are copied. Forgetting the shape is an
        // easy and invisible mistake — the heading would still read correctly, just without
        // its rule, on the one line the eye returns to most.
        let heading = heading_above(draw, top);
        if let Some(h) = heading {
            let spans = draw.items[h];
            let dy = draw.slot.y - item_top(draw, h);
            copy_rung(draw, &spans, dy, &mut out);
        }

        // --- the content, below the slot ------------------------------------------------
        //
        // D31's consumption rule: when `top` names a heading the slot has already drawn it,
        // so content starts at `top + 1`. Without this the same heading appears twice —
        // once in the slot and once flush beneath it — at index 0, at all six jump targets
        // and at `max_index`. That is the default state, not an edge case.
        let start = if is_heading_rung(draw, top) {
            top + 1
        } else {
            top
        };
        if draw.viewport.h > 0.0 && start < draw.items.len() {
            let dy = draw.viewport.y - item_top(draw, start);
            // `item_top` folds `min` over the rung's own primitives, so a rung that emitted
            // NONE — every column budget collapsed to zero on a sub-padding panel width —
            // answers `+inf` and makes `dy` `-inf`. Every later `rung_fits` would then be
            // trivially true and the whole document would be copied at `y = -inf`. Nothing
            // is visible at that width either way; the point is not to depend on that.
            if !dy.is_finite() {
                return out;
            }
            let bottom = draw.viewport.y + draw.viewport.h;
            for spans in &draw.items[start..] {
                // Whole rungs only: a rung whose bottom passes the viewport is not drawn at
                // all (C1 — there is no scissor rect, so a partial rung would paint outside
                // the band, and D17 exists so that never has to be represented).
                if !rung_fits(draw, spans, dy, bottom) {
                    break;
                }
                copy_rung(draw, spans, dy, &mut out);
            }
        }
    }

    // The footer line, synthesised here because stage one deliberately emits none (D38).
    //
    // When entries have arrived above the reader's anchor, this line becomes the badge
    // instead of the last-update time (D34). It is a REPLACEMENT, not a second run: the
    // footer is drawn with `text()`, which truncates silently, so two things sharing the line
    // would quietly lose the tail of whichever came last.
    //
    // The footer is the only pinned region that survives to the panel's minimum height
    // (`monitor_model`'s R5.1 note: below 64.8px the header and session row are gone and the
    // footer is not), which is exactly why the badge lives here — a warning that disappears
    // on a short panel is not a warning.
    if draw.footer_text.h > 0.0 {
        if let Some(text) = footer_line(draw, model, badge) {
            out.runs.push(TextRun {
                rect: draw.footer_text,
                text,
                color: if badge > 0 {
                    ColorRole::Amber.packed()
                } else {
                    ColorRole::TextFaint.packed()
                },
            });
        }
    }
    out
}

/// What the footer line says this frame, or `None` when nothing fits (D34/R7.5/R7.6).
///
/// The fallback order is the decision. The badge's first job is telling the reader they are
/// missing things — Inversion named "badge ignored" as the failure most likely to make this
/// whole feature a mistake — and its second is saying how to get back. So the key hint goes
/// first when the width runs out.
///
/// **The count itself is never truncated.** The footer is drawn with `text()`, which clips
/// silently, so stopping the fallback rule at "drop the hint" would leave `12 new · Cmd…`
/// becoming `12 ne…` — the same species of half-truth D20 refuses to draw on a heading. The
/// short form is the last honest thing that fits; below that, nothing.
fn footer_line(draw: &PanelDraw, model: &MonitorModel, badge: u32) -> Option<String> {
    let cols = draw.footer_cols;
    if badge == 0 {
        return (!model.footer.is_empty() && text_cols(&model.footer) <= cols)
            .then(|| model.footer.clone());
    }
    // Longest first: each fallback is a true statement, just a shorter one.
    [
        format!("{badge} new above · {RETURN_HINT}"),
        format!("{badge} new above"),
        format!("+{badge}"),
    ]
    .into_iter()
    .find(|c| text_cols(c) <= cols)
}

/// The chord that returns to the top (D30). TASKS is the document's first section, so D16's
/// first jump already IS "back to the top" — no new binding was invented for this.
const RETURN_HINT: &str = "⌘⌥1";

/// The heading whose section rung `top` belongs to — what the slot shows (R4.1).
///
/// Walks back to the section's own heading rather than reading `top`'s section index and
/// searching forward, so a rung naming a section the ladder no longer has degrades into
/// "no slot heading" instead of an out-of-bounds.
fn heading_above(draw: &PanelDraw, top: usize) -> Option<usize> {
    // Delegates rather than repeating the walk: `MonitorHandle::press` needs the same answer
    // from the ladder alone, and two walks that disagree would put the slot's heading and
    // the click's target on different sections (C11).
    crate::monitor_scroll::heading_above(&draw.ladder, top)
}

fn is_heading_rung(draw: &PanelDraw, i: usize) -> bool {
    draw.ladder.get(i).is_some_and(Item::is_heading)
}

/// Copy one rung's primitives, moved `dy` down. The single funnel for both the slot and the
/// content, so the two cannot drift apart about what "a rung" includes.
fn copy_rung(draw: &PanelDraw, spans: &ItemSpans, dy: f32, out: &mut Primitives) {
    out.shapes.extend(
        draw.content.shapes[spans.shapes.start..spans.shapes.end]
            .iter()
            .map(|s| s.translated_y(dy)),
    );
    out.runs.extend(
        draw.content.runs[spans.runs.start..spans.runs.end]
            .iter()
            .map(|r| TextRun {
                rect: Rect {
                    y: r.rect.y + dy,
                    ..r.rect
                },
                ..r.clone()
            }),
    );
}

/// Document-local `y` of rung `i`'s top edge, measured from the primitives it emitted.
fn item_top(draw: &PanelDraw, i: usize) -> f32 {
    let spans = draw.items[i];
    let runs = draw.content.runs[spans.runs.start..spans.runs.end]
        .iter()
        .map(|r| r.rect.y);
    let shapes = draw.content.shapes[spans.shapes.start..spans.shapes.end]
        .iter()
        .map(SidebarShape::top);
    runs.chain(shapes).fold(f32::INFINITY, f32::min)
}

fn rung_fits(draw: &PanelDraw, spans: &ItemSpans, dy: f32, bottom: f32) -> bool {
    let runs = draw.content.runs[spans.runs.start..spans.runs.end]
        .iter()
        .map(|r| r.rect.y + r.rect.h);
    let shapes = draw.content.shapes[spans.shapes.start..spans.shapes.end]
        .iter()
        .map(SidebarShape::bottom);
    let low = runs.chain(shapes).fold(f32::NEG_INFINITY, f32::max);
    // A rung that emitted nothing at all (an empty section's body draws none) never blocks
    // the ones after it.
    low == f32::NEG_INFINITY || low + dy <= bottom + FIT_EPS
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

    /// One row's height for a body kind.
    ///
    /// A card **reserves its detail line whether or not it has one**, so every card in a
    /// section is the same height. That used to matter because the allocator granted whole
    /// rows; under D4 it matters for a different reason — it is what makes a card exactly
    /// one rung of the ladder (R1.3) instead of a variable-height blob the scroll snap
    /// could land inside.
    fn row_h(&self, body: &SectionBody) -> f32 {
        match body {
            SectionBody::Cards(_) => 2.0 * self.line + self.s(2.0 * CARD_PAD + ROW_GAP),
            _ => self.line + self.s(ROW_GAP),
        }
    }

    fn heading_h(&self) -> f32 {
        self.line + self.s(ROW_GAP)
    }

    fn fold_h(&self) -> f32 {
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
    /// Where `shape`/`text` currently write. `false` = the pinned chrome (panel-absolute,
    /// clipped by `fits`); `true` = the document (document-local `y`, never clipped —
    /// stage two culls by ladder index instead, D38/R10.8).
    to_content: bool,
}

impl Painter {
    /// The single funnel for every shape: an `Option` from a constructor extends the
    /// vector with zero or one element, so rejected geometry (D9) is dropped here rather
    /// than unwrapped into a panic.
    fn shape(&mut self, s: Option<SidebarShape>) {
        self.buf().shapes.extend(s);
    }

    /// The buffer the current region writes into. One switch, read by every emit site, so
    /// a region cannot land in the wrong coordinate space by forgetting a parameter.
    fn buf(&mut self) -> &mut Primitives {
        if self.to_content {
            &mut self.out.content
        } else {
            &mut self.out.chrome
        }
    }

    /// Is a region `h` tall, starting at `y`, wholly inside the panel band? Nothing may
    /// paint outside it — there is no scissor rect (C1).
    ///
    /// **Always true while laying out the document.** Content is emitted document-local and
    /// most of it is legitimately far outside the panel band; stage two is what decides which
    /// rungs reach the GPU, and it decides a rung at a time in ladder order (D38/R10.8).
    /// Clipping here instead would drop rungs stage two still needs.
    fn fits(&self, y: f32, h: f32) -> bool {
        self.to_content || (y + FIT_EPS >= self.m.panel[1] && y + h <= self.m.bottom() + FIT_EPS)
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
        self.buf().runs.push(TextRun {
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
        self.buf().runs.push(TextRun {
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

    /// Header band: accent bar, title, status pill. Returns the `y` below the band, or
    /// `None` when the band was dropped — which is also the caller's signal that the
    /// session row has nothing to hang off (see [`layout`]).
    ///
    /// `ceiling` is the pinned footer's top edge. The band is a filled rect, not a text
    /// line, so a header that fits the panel but crosses the footer does not merely crowd
    /// it — it repaints the footer's background out from under it and drops the hairline
    /// rule inside the band.
    ///
    /// The design's `✕` close button is **not** drawn. D16 makes the whole band
    /// pointer-inert, so a close affordance would be a control that looks clickable and is
    /// not — the same trade D15 already rejected for the sidebar's toggle row. The panel
    /// closes with the chord that opened it (D1).
    fn header(&mut self, model: &MonitorModel, y: f32, ceiling: f32) -> Option<f32> {
        let m = self.m;
        let h = m.line + m.s(2.0 * HEADER_PAD_Y);
        if !self.fits(y, h) || y + h > ceiling + FIT_EPS || m.inner_w <= 0.0 {
            return None;
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
        Some(y + h)
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
        self.buf().runs.push(TextRun {
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
    ///
    /// `ceiling` is the pinned footer's top edge — **the same ceiling [`tiles`] reads, and
    /// for the same reason.** Fitting inside the panel is not enough: the footer is pinned
    /// to the bottom, so a row that only checked the panel's bottom edge drew its glyphs
    /// straight through the footer's on a short panel (measured at 430px wide, a 16px cell
    /// and scale 1: 7.2px of overlap at an 80px panel, and some overlap at every height
    /// below 87.2px). Yielding the row leaves the footer legible; the panel bottom on its
    /// own cannot tell the difference.
    ///
    /// [`tiles`]: Painter::tiles
    fn session(&mut self, model: &MonitorModel, y: f32, ceiling: f32) -> f32 {
        let m = self.m;
        if m.inner_w <= 0.0 {
            return y;
        }
        let s = &model.session;
        let mut y = y + m.s(GAP);
        if !self.fits(y, m.line) || y + m.line > ceiling + FIT_EPS {
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
            // The detached wording is a second line and yields on its own terms: it is the
            // one part of the session row that can be dropped while the identity line it
            // qualifies still reads.
            if self.fits(y, m.line) && y + m.line <= ceiling + FIT_EPS {
                self.text(m.inner_x, y, m.inner_w, note, ColorRole::TextMuted);
                y += m.line;
            }
        }
        y
    }

    /// The stat grid, three tiles per row, stopping at `ceiling` (the pinned footer's top).
    ///
    /// **This is where the panel's height degradation actually happens, and its order is a
    /// decision, not an accident.** The chrome D10 pins is not all equally pinnable: on a
    /// short window the header, session row and footer still fit when the 3x2 grid does
    /// not, so the grid is what yields — and it yields **whole rows from the bottom**,
    /// because `ty` is a function of the row index alone, so the first tile that does not
    /// fit ends the loop for its entire row and every row after it. That mirrors D18's
    /// headings-drop-from-the-bottom rule rather than inventing a second direction.
    ///
    /// Dropping a *partial* row would be the bad failure: UPTIME and IDLE present with
    /// ERRORS missing reads as "no errors recorded", which is a different claim from "this
    /// window is too short to show the tile".
    fn tiles(&mut self, model: &MonitorModel, y: f32, ceiling: f32) -> f32 {
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
            if !self.fits(ty, th) || ty + th > ceiling + FIT_EPS {
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

    /// The `y` the pinned footer's band starts at — the ceiling every elastic region must
    /// stay above — or the panel's bottom when there is no room for a footer at all.
    ///
    /// Split out from [`footer`](Painter::footer) because the stat grid needs this number
    /// *before* the footer is painted, and re-deriving it there would let the two drift:
    /// one copy would eventually round differently and the grid would creep back over the
    /// footer it is supposed to yield to.
    ///
    /// [`footer`]: Painter::footer
    fn footer_top(&self) -> f32 {
        let m = self.m;
        let h = m.line + m.s(GAP);
        let top = m.bottom() - h;
        if m.inner_w <= 0.0 || !self.fits(top, h) {
            m.bottom()
        } else {
            top
        }
    }

    /// The footer, pinned to the panel's bottom edge.
    ///
    /// The design's `q` keycap is deliberately absent (D1/R1.4): the panel consumes no
    /// keystroke but the chord, so drawing a keycap would promise a binding that does not
    /// exist and would cost a terminal user their `less` quit key if it did.
    ///
    /// **Both premises of that decision have since changed** (ai-monitor-panel-scroll D30),
    /// so stage two now writes a chord hint onto this line while the new-items badge is up:
    /// the panel *does* answer to `Cmd+Opt+1` now (D16), and that chord is not a key any
    /// shell is waiting for. The keycap **box** is still absent, and that part has not
    /// changed — a drawn control invites a click the panel has no answer for (D10 keeps it
    /// click-free), so the hint is text and may only ever be text.
    fn footer(&mut self, model: &MonitorModel) {
        let m = self.m;
        let top = self.footer_top();
        if top >= m.bottom() {
            return;
        }
        self.rule(
            m.panel[0],
            m.panel[0] + m.panel[2],
            top,
            ColorRole::Hairline,
        );
        // The footer *text* is not emitted here, and that absence is the decision (D38).
        // Stage two synthesises this one line every frame because it is the line the badge
        // replaces (D34) — and because the alternative, letting stage one draw it and then
        // having stage two hunt for it by `y` to suppress it, is exactly the fragile
        // addressing the structured output exists to abolish. What stage two needs is the
        // geometry, which is recorded here.
        let _ = model;
        self.out.footer_text = Rect {
            x: m.inner_x,
            y: top + m.s(GAP) * 0.5,
            w: m.inner_w,
            h: m.line,
        };
        self.out.footer_cols = m.cols(m.inner_w);
    }

    /// The whole document: every rung of every section, in order, laid out **document-local**
    /// from `y = 0` (D4/R1.1).
    ///
    /// This replaced D18's height allocator rather than wrapping it. The allocator existed to
    /// answer "which rows fit?", and D4 deleted that question — the panel now promises
    /// *reachability*, not simultaneous visibility, so there is nothing left to ration. What
    /// this owes instead is a **span per rung**, because that is the only handle stage two has
    /// on "draw items 12..19 and nothing else".
    ///
    /// The spans are recorded around each rung's emit, so they stay correct no matter how many
    /// primitives a rung happens to produce — a card with no detail line emits fewer runs than
    /// one with a detail line, and neither the ladder nor stage two should have to know that.
    fn document(&mut self, model: &MonitorModel) {
        self.to_content = true;
        let mut y = 0.0f32;
        for section in &model.sections {
            y = self.rung(y, |p, y| p.heading(section, y));
            // `take(shown())` is the single point the fold is applied to the drawing, and it
            // matches `item_ladder`'s own `shown()` by construction. C11's rule: two counts
            // of the same document that are derived separately drift apart silently.
            let shown = section.shown();
            match &section.body {
                SectionBody::Empty(_) => {}
                SectionBody::Cards(v) => {
                    for c in v.iter().take(shown) {
                        y = self.rung(y, |p, y| p.card(c, y));
                    }
                }
                SectionBody::Rows(v) => {
                    for r in v.iter().take(shown) {
                        y = self.rung(y, |p, y| p.row(r, y));
                    }
                }
                SectionBody::Bars(v) => {
                    for b in v.iter().take(shown) {
                        y = self.rung(y, |p, y| p.usage_bar(b, y));
                    }
                }
            }
            if section.foldable() {
                y = self.rung(y, |p, y| p.fold(section, y));
            }
        }
        self.to_content = false;
    }

    /// Emit one rung, bracketing it with the span bookkeeping. Returns the `y` the next rung
    /// starts at.
    fn rung(&mut self, y: f32, draw: impl FnOnce(&mut Painter, f32) -> f32) -> f32 {
        let s0 = self.out.content.shapes.len();
        let r0 = self.out.content.runs.len();
        let next = draw(self, y);
        self.out.items.push(ItemSpans {
            shapes: Span {
                start: s0,
                end: self.out.content.shapes.len(),
            },
            runs: Span {
                start: r0,
                end: self.out.content.runs.len(),
            },
        });
        next
    }

    /// `LABEL ──────── +N more  12` — label left, hairline filling, marks right.
    ///
    /// Both the `+N more` marker (R8.6) and the empty-state wording ride the heading
    /// rather than taking a row. A marker that occupied a row would compete with the data
    /// it is announcing for the single row D18 guarantees the section — which is the same
    /// reason the current dashboard already places it there.
    /// The fold control (`▸ 13 more` / `▾ collapse`), indented to the body's own column.
    ///
    /// Wording is derived from `folded()` rather than branched on the flag twice: a closed
    /// fold hides a positive number, an open one hides none, and there is no third state to
    /// get out of step.
    fn fold(&mut self, section: &Section, y: f32) -> f32 {
        let m = self.m;
        let advance = m.line + m.s(ROW_GAP);
        if !self.fits(y, m.line) {
            return y + advance;
        }
        let hidden = section.folded();
        let text = if hidden > 0 {
            format!("▸ {hidden} more")
        } else {
            "▾ collapse".to_string()
        };
        // Faint, not muted: the control is an affordance, and it must not compete with the
        // rows it sits under for the eye that is reading them.
        self.text(m.inner_x, y, m.inner_w, &text, ColorRole::TextFaint);
        y + advance
    }

    fn heading(&mut self, section: &Section, y: f32) -> f32 {
        let m = self.m;
        let mut edge = m.right();
        match &section.body {
            SectionBody::Empty(state) => {
                edge = self.text_right(edge, y, state, ColorRole::TextFaint);
            }
            _ => {
                // D20: when the ring has dropped entries, the count mark says so — and it
                // REPLACES the bare total rather than sitting beside it. Two marks would
                // print the same session number twice and eat the width the label needs.
                //
                // `mark_right` draws a mark whole or not at all (a truncated `32 of 400`
                // would state a falsehood), so a refusal is detectable by the edge not
                // moving. On a panel too narrow for the long form we fall back to the bare
                // total — today's behaviour — never to a half-truth.
                let mark = if section.lost > 0 {
                    format!("{} of {}", section.total, section.total + section.lost)
                } else {
                    section.total.to_string()
                };
                let after = self.mark_right(edge, y, &mark, ColorRole::TextMuted);
                edge = if after == edge && section.lost > 0 {
                    self.mark_right(edge, y, &section.total.to_string(), ColorRole::TextMuted)
                } else {
                    after
                };
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
        y + m.heading_h()
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

    /// What the frame actually draws: stage one, then stage two at the top of the document.
    ///
    /// Almost every assertion below is about *drawn output*, and after D24 that is stage
    /// two's job — stage one alone emits the document in its own coordinate space and no
    /// footer text at all. Routing the old assertions through both stages keeps them
    /// testing what they always tested.
    fn frame(m: &MonitorModel, panel: [f32; 4], scale: f64, cell: [f32; 2]) -> Primitives {
        let draw = layout(m, panel, scale, cell);
        compose(&draw, m, 0, 0)
    }

    fn rows_section(label: &str, n: usize, total: usize) -> Section {
        Section {
            collapsed: false,
            label: label.into(),
            total,
            lost: 0,
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

    fn cards_section(label: &str, n: usize) -> Section {
        Section {
            collapsed: false,
            label: label.into(),
            total: n,
            lost: 0,
            body: SectionBody::Cards(
                (0..n)
                    .map(|i| Card {
                        title: format!("card{i}"),
                        ..Card::default()
                    })
                    .collect(),
            ),
        }
    }

    fn empty_section(label: &str) -> Section {
        Section {
            collapsed: false,
            label: label.into(),
            total: 0,
            lost: 0,
            body: SectionBody::Empty("(none)".into()),
        }
    }

    // ---- ai-monitor-panel-scroll T1 / R1.2 / R1.3: the item ladder ----

    #[test]
    fn r1_3_a_heading_is_a_rung_even_when_its_section_is_empty() {
        // The empty-state wording rides the heading, so an empty section is still on
        // screen, still jumpable (D16), and therefore still needs an index. If it were
        // skipped, `Cmd+Shift+N` for that section would have no target and the six jumps
        // would silently renumber themselves as sections filled up.
        let m = model(vec![empty_section("TASKS"), rows_section("SKILLS", 2, 2)]);
        let ladder = item_ladder(&m);
        assert_eq!(
            ladder,
            vec![
                Item {
                    section: 0,
                    kind: ItemKind::Heading,
                    entry: None
                },
                Item {
                    section: 1,
                    kind: ItemKind::Heading,
                    entry: None
                },
                Item {
                    section: 1,
                    kind: ItemKind::Row,
                    entry: Some(0)
                },
                Item {
                    section: 1,
                    kind: ItemKind::Row,
                    entry: Some(1)
                },
            ]
        );
    }

    #[test]
    fn r1_3_a_card_is_one_rung_although_it_draws_two_lines() {
        // A card reserves its detail line whether or not it has one. Counting it as two
        // rungs would make the ladder's index space disagree with what the anchor sees,
        // and D17's snap would land between a card's two lines — which C1 forbids drawing.
        let m = model(vec![cards_section("TASKS", 3)]);
        let ladder = item_ladder(&m);
        assert_eq!(ladder.len(), 4, "one heading + three cards");
        assert!(ladder[1..].iter().all(|i| i.kind == ItemKind::Card));

        // And its height is the card height, not the row height — one rung, two lines.
        let card_h = item_h(&ladder[1], &m, 1.0, CELL);
        let heading_h = item_h(&ladder[0], &m, 1.0, CELL);
        assert!(
            card_h > heading_h,
            "a card rung ({card_h}) must be taller than a heading rung ({heading_h})"
        );
    }

    #[test]
    fn r1_2_the_ladder_follows_the_models_section_order() {
        // The order is the app's (the design's six, fixed). The ladder must not sort,
        // filter or reorder — D4 replaced "everything visible at once" with "everything
        // reachable in a known order", and the order is half of that promise.
        let m = model(vec![
            cards_section("TASKS", 1),
            cards_section("ACTIVE AGENTS", 1),
            rows_section("SKILLS", 1, 1),
            rows_section("FILE ACTIVITY", 1, 1),
            rows_section("RECENT CALLS", 1, 1),
            empty_section("TOOL USAGE"),
        ]);
        let ladder = item_ladder(&m);
        let headings: Vec<usize> = ladder
            .iter()
            .filter(|i| i.is_heading())
            .map(|i| i.section)
            .collect();
        assert_eq!(headings, vec![0, 1, 2, 3, 4, 5]);
        // 6 headings + 5 bodies of one entry each.
        assert_eq!(ladder.len(), 11);
    }

    #[test]
    fn r13_2_the_fold_control_says_what_it_will_do_in_both_states() {
        // The wording is derived from `folded()`, so the two states cannot get out of step:
        // shut names the number it is hiding, open offers the way back.
        let mut m = model(vec![rows_section("RECENT CALLS", 32, 32)]);
        let text_of = |m: &MonitorModel| {
            let draw = layout(m, [0.0, 0.0, 430.0, 2000.0], 1.0, CELL);
            draw.content
                .runs
                .iter()
                .map(|r| r.text.clone())
                .find(|t| t.starts_with('▸') || t.starts_with('▾'))
        };
        m.sections[0].collapsed = true;
        assert_eq!(text_of(&m).as_deref(), Some("▸ 27 more"));
        m.sections[0].collapsed = false;
        assert_eq!(text_of(&m).as_deref(), Some("▾ collapse"));

        // A section too short to fold offers no control at all — one that hid nothing would
        // cost a rung to answer a question nobody asked.
        let short = model(vec![rows_section(
            "SKILLS",
            crate::monitor_model::FOLD_THRESHOLD,
            5,
        )]);
        assert_eq!(text_of(&short), None);
        assert!(!short.sections[0].foldable());
    }

    #[test]
    fn r13_3_a_shut_fold_draws_the_threshold_and_the_control_and_nothing_else() {
        // The drawn set and the ladder must agree about what a fold hid, or the scroll math
        // is measuring a document the eye is not looking at (C11).
        let mut m = model(vec![rows_section("RECENT CALLS", 32, 32)]);
        m.sections[0].collapsed = true;
        let draw = layout(&m, [0.0, 0.0, 430.0, 2000.0], 1.0, CELL);
        let shown = crate::monitor_model::FOLD_THRESHOLD;
        assert_eq!(draw.ladder.iter().filter(|i| i.is_body()).count(), shown);
        assert_eq!(draw.ladder.iter().filter(|i| i.is_fold()).count(), 1);
        assert_eq!(
            draw.items.len(),
            draw.ladder.len(),
            "spans stay parallel (C11)"
        );
        // Row 6 exists in the model and must NOT have been drawn.
        assert!(
            !draw.content.runs.iter().any(|r| r.text.contains("/5")),
            "a shut fold drew an entry it claims to be hiding"
        );
    }

    #[test]
    fn r1_3_every_body_entry_gets_exactly_one_rung() {
        // The whole point of D4: nothing is dropped for height. The ladder is the record
        // of that, so it must equal heading-count + total body entries for any model.
        let m = model(vec![
            cards_section("TASKS", 7),
            rows_section("RECENT CALLS", 32, 400),
            empty_section("TOOL USAGE"),
        ]);
        let ladder = item_ladder(&m);
        let entries: usize = m.sections.iter().map(|s| s.body.len()).sum();
        // Every entry is a rung, and no entry is two rungs. The fold controls are counted
        // separately and asserted separately, so this stays a statement about *entries*.
        assert_eq!(ladder.iter().filter(|i| i.is_body()).count(), entries);
        let foldable = m.sections.iter().filter(|s| s.foldable()).count();
        assert_eq!(ladder.len(), m.sections.len() + entries + foldable);
        assert_eq!(
            ladder.iter().filter(|i| i.is_fold()).count(),
            foldable,
            "one control per foldable section, and none for the others"
        );
        assert_eq!(entries, 39, "7 cards + 32 rows + 0 for the empty section");
    }

    #[test]
    fn r1_3_a_stale_or_degenerate_rung_measures_zero_rather_than_panicking() {
        // `item_h` is called from the app across model swaps, so a rung naming a section
        // that has since gone away is a normal transient, not a bug to unwrap on. The
        // scroll math reads a non-positive height as "not a stop".
        let m = model(vec![rows_section("SKILLS", 1, 1)]);
        let stale = Item {
            section: 9,
            kind: ItemKind::Row,
            entry: Some(0),
        };
        assert_eq!(item_h(&stale, &m, 1.0, CELL), 0.0);

        let real = item_ladder(&m)[0];
        assert_eq!(item_h(&real, &m, 1.0, [0.0, 0.0]), 0.0, "degenerate cell");
        assert_eq!(slot_h(1.0, [0.0, 0.0]), 0.0);
        assert!(slot_h(1.0, CELL) > 0.0);
    }

    #[test]
    fn r1_3_the_slot_is_exactly_one_heading_line() {
        // D31 reserves the slot out of the content viewport. If the slot were taller or
        // shorter than the heading it hosts, the copied heading (D36) would not sit flush
        // and the first content rung would either overlap it or leave a gap.
        let m = model(vec![rows_section("SKILLS", 1, 1)]);
        let heading = item_ladder(&m)[0];
        assert_eq!(slot_h(1.0, CELL), item_h(&heading, &m, 1.0, CELL));
        // And it scales with DPI, or a hidpi panel would reserve a band sized for 1x.
        assert!(slot_h(2.0, CELL) > slot_h(1.0, CELL));
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

    // ---- ai-monitor-panel-scroll T2 / R1.1 / R9.1 / R10.5-R10.7: the structured document ----

    #[test]
    fn r1_1_every_entry_is_laid_out_however_short_the_panel_is() {
        // The heart of D4. The old allocator handed out rows until the height ran out and
        // announced the rest as `+N more`; the document now contains all of them at every
        // height, and it is stage two that decides how many reach the screen. A regression
        // here is invisible on screen — the panel looks fine, it just cannot be scrolled to
        // content that was never laid out.
        let m = six_sections([9, 7, 5, 11, 32, 4]);
        let entries: usize = m.sections.iter().map(|s| s.body.len()).sum();
        // Including heights too short to show a single rung: the document is laid out
        // anyway, because `items` must stay parallel to the ladder at every size (C11).
        // What a short panel changes is `viewport.h`, which is stage two's business.
        for h in [40.0f32, 120.0, 240.0, 400.0, 4000.0] {
            let draw = layout(&m, [0.0, 0.0, 430.0, h], 1.0, CELL);
            let foldable = m.sections.iter().filter(|s| s.foldable()).count();
            assert_eq!(
                draw.items.len(),
                6 + entries + foldable,
                "panel height {h}: the document must hold six headings, all {entries} entries \
                 and {foldable} fold controls"
            );
            assert_eq!(
                draw.items.len(),
                item_ladder(&m).len(),
                "panel height {h}: spans and ladder must stay parallel (C11)"
            );
        }
    }

    #[test]
    fn r9_1_the_more_marker_is_gone() {
        // R8.6 used to pin `+N more` onto the heading. D4 deleted the condition it reported
        // (height-starved rows), so the string must not survive anywhere — including in the
        // one section where a difference between `total` and `body.len()` still exists,
        // which is the ring-buffer loss T6 reports differently.
        let m = six_sections([9, 7, 5, 11, 32, 4]);
        let draw = layout(&m, [0.0, 0.0, 430.0, 200.0], 1.0, CELL);
        let all: Vec<&str> = draw
            .chrome
            .runs
            .iter()
            .chain(draw.content.runs.iter())
            .map(|r| r.text.as_str())
            .collect();
        assert!(
            !all.iter().any(|t| t.contains("more")),
            "no `+N more` anywhere: {all:?}"
        );
    }

    #[test]
    fn r10_6_the_pinned_chrome_is_not_in_the_scrollable_document() {
        // The type is what enforces C2: stage two translates `content` and never `chrome`,
        // so a region that ended up in the wrong bag would scroll away and no amount of
        // care in `compose` would save it. Assert the separation at the source.
        let m = six_sections([2, 2, 2, 2, 2, 2]);
        let draw = layout(&m, [0.0, 0.0, 430.0, 400.0], 1.0, CELL);
        let chrome: Vec<&str> = draw.chrome.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(
            chrome.contains(&"ai-monitor"),
            "the header title is chrome: {chrome:?}"
        );
        let content: Vec<&str> = draw.content.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(
            content.contains(&"TASKS"),
            "section headings are content: {content:?}"
        );
        assert!(
            !chrome.contains(&"TASKS"),
            "a section heading must never land in the chrome bag"
        );
    }

    #[test]
    fn r10_7_stage_one_emits_no_footer_text_but_names_where_it_goes() {
        // D38: stage two owns that line because the badge replaces it (D34). If stage one
        // drew it, stage two would have to find and suppress it by `y` — the fragile
        // addressing the structured output exists to abolish.
        let m = six_sections([2, 2, 2, 2, 2, 2]);
        let draw = layout(&m, [0.0, 0.0, 430.0, 400.0], 1.0, CELL);
        assert!(
            !draw
                .chrome
                .runs
                .iter()
                .chain(draw.content.runs.iter())
                .any(|r| r.text == m.footer),
            "stage one must not emit the footer string"
        );
        assert!(draw.footer_text.h > 0.0, "but it must name the band");
        // And stage two puts it back, in that exact band.
        let out = compose(&draw, &m, 0, 0);
        let footer = out
            .runs
            .iter()
            .find(|r| r.text == m.footer)
            .expect("stage two synthesises the footer line");
        assert_eq!(footer.rect.y, draw.footer_text.y);
    }

    #[test]
    fn r10_5_every_rung_is_addressable_without_hunting_by_pixel() {
        // The span per rung is the handle stage two needs for "draw items 12..19". Ranges
        // must be non-overlapping and in document order, or a cull would drop or duplicate
        // a neighbour's primitives.
        let m = six_sections([3, 3, 3, 3, 3, 3]);
        let draw = layout(&m, [0.0, 0.0, 430.0, 400.0], 1.0, CELL);
        let mut runs_end = 0usize;
        let mut shapes_end = 0usize;
        for (i, spans) in draw.items.iter().enumerate() {
            assert!(spans.runs.start >= runs_end, "rung {i} runs overlap");
            assert!(spans.shapes.start >= shapes_end, "rung {i} shapes overlap");
            runs_end = spans.runs.end;
            shapes_end = spans.shapes.end;
        }
        assert_eq!(runs_end, draw.content.runs.len(), "no run is unclaimed");
        assert_eq!(
            shapes_end,
            draw.content.shapes.len(),
            "no shape is unclaimed"
        );
    }

    // ---- ai-monitor-panel-scroll T9 / R7.2-R7.6 / R2.2: the badge on the footer ----

    /// The one text run in the footer band, if any.
    fn footer_run(m: &MonitorModel, panel: [f32; 4], badge: u32) -> Option<String> {
        let draw = layout(m, panel, 1.0, CELL);
        let f = compose(&draw, m, 0, badge);
        let band = draw.footer_text;
        let runs: Vec<String> = f
            .runs
            .iter()
            .filter(|r| r.rect.y >= band.y - FIT_EPS && r.rect.y < band.y + band.h + FIT_EPS)
            .map(|r| r.text.clone())
            .collect();
        assert!(
            runs.len() <= 1,
            "the footer band must hold one line: {runs:?}"
        );
        runs.into_iter().next()
    }

    #[test]
    fn r7_2_the_badge_replaces_the_last_update_line_rather_than_joining_it() {
        // D34. The footer is drawn with `text()`, which truncates silently — two things
        // sharing the line would quietly lose the tail of whichever came second, and on a
        // drag-narrowed panel that is the common case, not the corner.
        let m = model(vec![rows_section("TASKS", 2, 2)]);
        let wide = [0.0, 0.0, 430.0, 900.0];
        assert_eq!(
            footer_run(&m, wide, 0).as_deref(),
            Some("Last update 16:21:18")
        );
        let badged = footer_run(&m, wide, 12).expect("a badge line");
        assert!(badged.contains("12"), "the count is there: {badged}");
        assert!(
            !badged.contains("Last update"),
            "and the time is gone, not squeezed: {badged}"
        );
    }

    #[test]
    fn r7_4_the_badge_names_the_chord_that_gets_back_to_the_top() {
        // D30 has no new binding — TASKS is the first section, so D16's first jump already
        // IS "back to the top". But a return path nobody can discover is not a return path,
        // which is why the badge has to say it. This is the second decision this feature
        // deliberately reverses: the footer was specified to draw NO keycap, precisely
        // because the panel had no binding to promise. It has one now.
        let m = model(vec![rows_section("TASKS", 2, 2)]);
        let line = footer_run(&m, [0.0, 0.0, 430.0, 900.0], 3).expect("a badge line");
        assert!(
            line.contains(RETURN_HINT),
            "no way back is advertised: {line}"
        );
    }

    #[test]
    fn r7_5_and_r7_6_narrowing_drops_the_hint_first_and_never_truncates_the_count() {
        // The fallback ORDER is the decision. The badge's first job is telling the reader
        // they are missing things — Inversion named "badge ignored" as the failure most
        // likely to make this whole feature a mistake — and its second is saying how to get
        // back. So the hint goes first.
        //
        // And the count itself is never clipped: `text()` truncates silently, so a rule that
        // stopped at "drop the hint" would render `12 ne…`, the same half-truth D20 refuses
        // to draw on a heading.
        let m = model(vec![rows_section("TASKS", 2, 2)]);
        let mut saw_full = false;
        let mut saw_short = false;
        let mut saw_tiny = false;
        let mut saw_none = false;
        for w in (40..440).step_by(4) {
            let line = footer_run(&m, [0.0, 0.0, w as f32, 900.0], 12);
            match line.as_deref() {
                Some(t) if t.contains(RETURN_HINT) => saw_full = true,
                Some("12 new above") => saw_short = true,
                Some("+12") => saw_tiny = true,
                None => saw_none = true,
                Some(other) => panic!("width {w}: a truncated badge reached the footer: {other:?}"),
            }
        }
        assert!(saw_full, "some width shows the whole thing");
        assert!(saw_short, "some width drops the hint and keeps the count");
        assert!(saw_tiny, "some width falls back to the short count");
        assert!(saw_none, "and some width draws nothing rather than a lie");
    }

    #[test]
    fn r7_3_and_r2_2_the_badge_holds_its_place_at_every_height_and_scroll_position() {
        // The badge is the only defence against the failure Inversion ranked first — the
        // reader who scrolls down, gets distracted, and silently stops seeing live activity.
        // A defence that vanishes on a short panel or once you scroll is not a defence, and
        // both are real: the pinned chrome yields from the bottom of its order as the panel
        // shrinks, and the footer is the ONLY region that survives to the floor.
        let m = six_sections([9, 7, 5, 11, 32, 4]);
        for h in [100.0f32, 200.0, 400.0, 900.0] {
            let panel = [0.0, 0.0, 430.0, h];
            let draw = layout(&m, panel, 1.0, CELL);
            if draw.footer_text.h <= 0.0 {
                continue; // below the floor there is no panel to speak of
            }
            let max =
                crate::monitor_scroll::max_index(&draw.ladder, &draw.heights, draw.viewport.h);
            for top in [0, max / 2, max] {
                let f = compose(&draw, &m, top, 7);
                let band = draw.footer_text;
                let line = f
                    .runs
                    .iter()
                    .find(|r| r.rect.y >= band.y - FIT_EPS && r.rect.y < band.y + band.h + FIT_EPS)
                    .map(|r| r.text.clone());
                assert!(
                    line.as_deref().is_some_and(|t| t.contains('7')),
                    "h={h} top={top}: the badge went missing ({line:?})"
                );
            }
        }
    }

    // ---- ai-monitor-panel-scroll T6 / R9.2-R9.5: the ring-buffer loss mark ----

    fn lossy_section(label: &str, retained: usize, lost: usize) -> Section {
        Section {
            collapsed: false,
            lost,
            ..rows_section(label, retained, retained)
        }
    }

    /// The marks drawn on `label`'s heading, right to left.
    fn heading_marks(m: &MonitorModel, panel: [f32; 4], label: &str) -> Vec<String> {
        let draw = layout(m, panel, 1.0, CELL);
        let h = draw
            .ladder
            .iter()
            .position(|i| i.is_heading() && m.sections[i.section].label == label)
            .expect("the section exists");
        let spans = draw.items[h];
        let runs = &draw.content.runs[spans.runs.start..spans.runs.end];
        // The label is the leftmost run; the marks are right-aligned after it. Comparing
        // text would miss a label the panel truncated, which is exactly the width band this
        // helper is used to probe.
        let label_x = runs.iter().map(|r| r.rect.x).fold(f32::INFINITY, f32::min);
        let _ = label;
        runs.iter()
            .filter(|r| r.rect.x > label_x)
            .map(|r| r.text.clone())
            .collect()
    }

    #[test]
    fn r9_2_a_ring_that_dropped_entries_says_so_instead_of_showing_a_bare_count() {
        // D20. `+N more` summed two unrelated differences; this reports exactly one — the
        // entries that are GONE, which D1 puts permanently out of reach. Saying "32" alone
        // would let "everything is reachable now" quietly become a lie on any long session.
        let m = model(vec![lossy_section("RECENT CALLS", 32, 368)]);
        let marks = heading_marks(&m, [0.0, 0.0, 430.0, 900.0], "RECENT CALLS");
        assert_eq!(marks, vec!["32 of 400"]);
        assert!(
            !marks.iter().any(|t| t.contains("more")),
            "and it is not the old marker wearing a new number"
        );
    }

    #[test]
    fn r9_3_a_section_that_lost_nothing_looks_exactly_as_it_did() {
        // The common case must not grow a new phrase. `lost == 0` reads as "nothing known to
        // be lost", which is the honest answer for every ring the crate gives no cumulative
        // counter for — inventing a number for those would be `+N more` all over again.
        let m = model(vec![lossy_section("SKILLS", 5, 0)]);
        assert_eq!(heading_marks(&m, [0.0, 0.0, 430.0, 900.0], "SKILLS"), ["5"]);
    }

    #[test]
    fn r9_4_a_panel_too_narrow_for_the_long_mark_falls_back_to_the_bare_total() {
        // `mark_right` draws a mark whole or not at all — a truncated `32 of 400` states a
        // falsehood. So the narrow case must degrade to the short true thing, never to a
        // clipped one, and never to nothing while a shorter true mark would have fit.
        let m = model(vec![lossy_section("RECENT CALLS", 32, 368)]);
        let long = heading_marks(&m, [0.0, 0.0, 430.0, 900.0], "RECENT CALLS");
        assert_eq!(long, ["32 of 400"], "premise: it fits at full width");

        let mut widths_with_short = 0;
        for w in (60..430).step_by(10) {
            let marks = heading_marks(&m, [0.0, 0.0, w as f32, 900.0], "RECENT CALLS");
            for mark in &marks {
                assert!(
                    mark == "32 of 400" || mark == "32",
                    "width {w}: a half-truth reached the heading: {mark:?}"
                );
                if mark == "32" {
                    widths_with_short += 1;
                }
            }
        }
        assert!(
            widths_with_short > 0,
            "premise: some width must actually exercise the fallback"
        );
    }

    #[test]
    fn r9_5_an_app_display_cap_is_not_reported_as_a_loss() {
        // C10, and the trap D20 walked into on its first draft. TOOL USAGE's `total` is the
        // tally's 11 names while its body holds 9 bars — that gap is `TOOL_BARS`, a
        // readability cap D21 keeps. The data is not gone; the app chose not to send it.
        // Reporting it as loss would re-create the exact conflation `+N more` was retired for.
        let m = model(vec![Section {
            collapsed: false,
            label: "TOOL USAGE".into(),
            total: 11,
            lost: 0,
            body: SectionBody::Bars((0..9).map(|i| Bar::new(format!("t{i}"), 1, 1.0)).collect()),
        }]);
        let marks = heading_marks(&m, [0.0, 0.0, 430.0, 900.0], "TOOL USAGE");
        assert_eq!(marks, ["11"], "the cap must not produce an `of` phrase");
    }

    // ---- ai-monitor-panel-scroll T5 / R4.1-R4.4 / R0.1-R0.2: the sticky slot ----

    /// Every stop of a document, as `(what the slot shows, what the body shows)`.
    fn walk(m: &MonitorModel, panel: [f32; 4]) -> Vec<(String, Vec<String>)> {
        let draw = layout(m, panel, 1.0, CELL);
        let max = crate::monitor_scroll::max_index(&draw.ladder, &draw.heights, draw.viewport.h);
        let labels: Vec<&str> = m.sections.iter().map(|s| s.label.as_str()).collect();
        (0..=max)
            .filter(|&i| crate::monitor_scroll::is_stop(&draw.ladder, i))
            .map(|top| {
                let f = compose(&draw, m, top, 0);
                let in_slot: Vec<String> = f
                    .runs
                    .iter()
                    .filter(|r| r.rect.y >= draw.slot.y && r.rect.y < draw.slot.y + draw.slot.h)
                    .filter(|r| labels.contains(&r.text.as_str()))
                    .map(|r| r.text.clone())
                    .collect();
                let body: Vec<String> = f
                    .runs
                    .iter()
                    .filter(|r| r.rect.y >= draw.viewport.y)
                    .filter(|r| r.rect.y < draw.viewport.y + draw.viewport.h)
                    .map(|r| r.text.clone())
                    .collect();
                (in_slot.join("+"), body)
            })
            .collect()
    }

    #[test]
    fn r3_1_and_r10_8_content_stays_inside_the_viewport_as_whole_rungs_at_every_stop() {
        // C1's real consequence, asserted where it can actually fail. There is no scissor
        // rect, so a rung that half-fits would paint across the footer — D17's snap exists so
        // that state cannot arise, and this is the check that it does not.
        //
        // R10.8 rides along: the drawn set must be a CONTIGUOUS ladder range starting at
        // `content_start`. A cull that skipped a too-tall rung and kept going would leave the
        // document with a hole the scroll can never fill, and every "is it inside the band"
        // assertion would still pass.
        let m = six_sections([9, 7, 5, 11, 32, 4]);
        for h in [300.0f32, 420.0, 700.0] {
            let panel = [0.0, 0.0, 430.0, h];
            let draw = layout(&m, panel, 1.0, CELL);
            if draw.viewport.h <= 0.0 {
                continue;
            }
            let max =
                crate::monitor_scroll::max_index(&draw.ladder, &draw.heights, draw.viewport.h);
            let bottom = draw.viewport.y + draw.viewport.h;
            for top in 0..=max {
                let f = compose(&draw, &m, top, 0);
                // Nothing the DOCUMENT drew may cross the viewport's bottom edge.
                //
                // "Begins inside the band", not "begins at or after its top": the pinned
                // chrome below the band includes the footer's own hairline rule, which sits
                // between the band's bottom and the footer text. Anything starting below the
                // band is chrome by construction. What C1 makes dangerous — and what this
                // asserts — is a rung that begins inside the band and does not end there.
                let is_content = |y: f32| y >= draw.viewport.y - FIT_EPS && y < bottom - FIT_EPS;
                for r in &f.runs {
                    if is_content(r.rect.y) {
                        assert!(
                            r.rect.y + r.rect.h <= bottom + FIT_EPS,
                            "h={h} top={top}: run {:?} crosses the viewport bottom",
                            r.text
                        );
                    }
                }
                for sh in &f.shapes {
                    if is_content(sh.top()) {
                        assert!(
                            sh.bottom() <= bottom + FIT_EPS,
                            "h={h} top={top}: a shape spanning {}..{} crosses the viewport \
                             bottom {bottom}",
                            sh.top(),
                            sh.bottom()
                        );
                    }
                }
                // Contiguity: the body texts drawn are a prefix of the ladder from
                // `content_start`, with no gap.
                let start = crate::monitor_scroll::content_start(&draw.ladder, top);
                let drawn: Vec<String> = f
                    .runs
                    .iter()
                    .filter(|r| is_content(r.rect.y))
                    .filter(|r| r.text.contains('/'))
                    .map(|r| r.text.clone())
                    .collect();
                let expected_order: Vec<String> = draw.ladder[start..]
                    .iter()
                    // Body rungs only: a heading draws its label and a fold control draws
                    // its own wording, and neither is in the `label/entry` form this walk
                    // matches on.
                    .filter(|i| i.is_body())
                    .map(|i| {
                        format!(
                            "{}/{}",
                            m.sections[i.section].label,
                            i.entry.expect("is_body")
                        )
                    })
                    .collect();
                for (k, got) in drawn.iter().enumerate() {
                    assert_eq!(
                        Some(got),
                        expected_order.get(k),
                        "h={h} top={top}: the drawn rungs are not a contiguous run from {start}"
                    );
                }
            }
        }
    }

    #[test]
    fn r6_7_the_layout_is_pure_so_it_can_hold_no_frame_to_frame_state() {
        // Why the anchor and the badge live in the app (D27): `layout` must stay a function
        // of its arguments, or the GPU-free testability every assertion in this file depends
        // on goes with it. Stated executably — same inputs, byte-identical output, no matter
        // how many times or in what order.
        let m = six_sections([4, 0, 3, 5, 9, 2]);
        let panel = [0.0, 0.0, 430.0, 500.0];
        let a = layout(&m, panel, 1.0, CELL);
        let other = layout(&six_sections([1, 1, 1, 1, 1, 1]), panel, 1.0, CELL);
        let b = layout(&m, panel, 1.0, CELL);
        assert_eq!(a, b, "layout kept state between calls");
        assert_ne!(a, other, "premise: the two models really do differ");
        // And composing does not mutate what it composed from.
        let before = a.clone();
        let _ = compose(&a, &m, 3, 7);
        assert_eq!(a, before, "compose mutated stage one's output");
    }

    #[test]
    fn r3_2_the_top_rung_sits_flush_on_the_viewports_upper_edge() {
        // R3.2 is a *geometry* claim — "항목 `i`가 슬롯의 아래 모서리에 놓인다" — and it was
        // for a while carried only by an id on a test that asserts the consumption RULE
        // (`content_start`) instead. They are different facts: the rule could be right while
        // the translation put the rung a gap-height too low, and every "inside the band"
        // assertion would still pass while a sliver of the previous rung showed above it.
        let m = six_sections([4, 4, 4, 4, 4, 4]);
        let draw = layout(&m, [0.0, 0.0, 430.0, 500.0], 1.0, CELL);
        let tallest = draw.heights.iter().copied().fold(0.0f32, f32::max);
        assert!(
            draw.viewport.h >= tallest,
            "premise: the band must hold the tallest rung, or this fails confusingly \
             instead of skipping"
        );
        let max = crate::monitor_scroll::max_index(&draw.ladder, &draw.heights, draw.viewport.h);
        for top in (0..=max).filter(|&i| crate::monitor_scroll::is_stop(&draw.ladder, i)) {
            let start = crate::monitor_scroll::content_start(&draw.ladder, top);
            if start >= draw.items.len() {
                continue;
            }
            let f = compose(&draw, &m, top, 0);
            // The highest thing the document drew inside the band is the top rung's own edge,
            // and it must land flush on the band's top rather than a gap below it. (The
            // symmetric "never ABOVE" half is not visible to this filter; it is carried by
            // `r3_1_and_r10_8`'s contiguity check, which would see a wrong first rung.)
            let highest = f
                .runs
                .iter()
                .map(|r| r.rect.y)
                .chain(f.shapes.iter().map(|s| s.top()))
                .filter(|y| *y >= draw.viewport.y - FIT_EPS && *y < draw.footer_text.y)
                .fold(f32::INFINITY, f32::min);
            assert!(
                (highest - draw.viewport.y).abs() <= FIT_EPS,
                "top {top}: the first rung starts at {highest}, not flush on {}",
                draw.viewport.y
            );
        }
    }

    #[test]
    fn r4_1_the_current_sections_heading_is_in_the_slot_even_when_it_has_scrolled_off() {
        // The reason D5 exists: the six sections' rows look alike, so without the heading a
        // reader deep in RECENT CALLS cannot tell which list they are in.
        let m = six_sections([1, 1, 1, 1, 20, 1]);
        let stops = walk(&m, [0.0, 0.0, 430.0, 300.0]);
        assert!(stops.len() > 6, "premise: this document scrolls");
        for (slot, body) in &stops {
            assert!(
                !slot.is_empty(),
                "a stop with no heading in the slot: {body:?}"
            );
        }
        // And deep in the long section it is still RECENT CALLS — the heading rung itself is
        // long gone from the viewport by then.
        let deep = stops.last().expect("a last stop");
        assert_eq!(deep.0, "TOOL USAGE");
    }

    #[test]
    fn r4_2_exactly_one_heading_occupies_the_slot_at_every_stop() {
        // D18 chose push-off over stacking: six stacked headings would eat the viewport D4
        // just fought to fill. Under D17's snap "push-off" is realised as replacement at the
        // boundary, so what must hold at every observable frame is simply "exactly one".
        let m = six_sections([2, 0, 3, 1, 4, 2]);
        for h in [200.0f32, 300.0, 900.0] {
            for (slot, body) in walk(&m, [0.0, 0.0, 430.0, h]) {
                assert!(
                    !slot.contains('+'),
                    "h={h}: two headings shared the slot ({slot}) over {body:?}"
                );
            }
        }
    }

    #[test]
    fn r4_2_the_slot_heading_never_repeats_itself_in_the_body() {
        // D31's consumption rule. Without it the same heading is drawn twice — in the slot
        // and flush beneath it — and that is the DEFAULT state, not an edge case: it happens
        // at index 0, at all six jump targets, and at `max_index`.
        let m = six_sections([3, 0, 3, 3, 3, 3]);
        for h in [200.0f32, 300.0, 900.0] {
            for (slot, body) in walk(&m, [0.0, 0.0, 430.0, h]) {
                assert!(
                    !body.contains(&slot),
                    "h={h}: {slot:?} appears in the slot AND in the body {body:?}"
                );
            }
        }
    }

    #[test]
    fn r4_4_nothing_but_the_heading_is_drawn_in_the_slot_band() {
        // C1: shapes composite UNDER glyphs (`monitor_panel::record` runs the shape pass
        // then the glyph pass), so a heading background could never hide a row's glyphs
        // sliding beneath it, and there is no scissor rect to clip them. D31 answers by
        // reserving the band — so the invariant to hold is that nothing else is ever in it.
        let m = six_sections([4, 4, 4, 4, 4, 4]);
        let panel = [0.0, 0.0, 430.0, 300.0];
        let draw = layout(&m, panel, 1.0, CELL);
        let max = crate::monitor_scroll::max_index(&draw.ladder, &draw.heights, draw.viewport.h);
        let slot_lo = draw.slot.y;
        let slot_hi = draw.slot.y + draw.slot.h;
        let overlaps =
            |top: f32, bottom: f32| bottom > slot_lo + FIT_EPS && top < slot_hi - FIT_EPS;
        for top in 0..=max {
            let f = compose(&draw, &m, top, 0);

            // The slot legitimately holds the WHOLE heading — its label, its right-aligned
            // marks and its rule. What must never be in the band is a *body* rung.
            let intruding_runs: Vec<&str> = f
                .runs
                .iter()
                .filter(|r| overlaps(r.rect.y, r.rect.y + r.rect.h))
                .map(|r| r.text.as_str())
                .filter(|t| t.contains('/'))
                .collect();
            assert!(
                intruding_runs.is_empty(),
                "top {top}: body text in the slot band: {intruding_runs:?}"
            );

            // And its SHAPES. Checking only the glyphs was the weaker half: a card's box or
            // a bar's track sliding under the heading is precisely the overdraw C1 cannot
            // clip, and it carries no text to be caught by the filter above. Counted against
            // what the slot's own heading emits, so the heading's rule is not an intruder.
            let slot_shapes = compose(&draw, &m, 0, 0)
                .shapes
                .iter()
                .filter(|sh| overlaps(sh.top(), sh.bottom()))
                .count();
            let in_band = f
                .shapes
                .iter()
                .filter(|sh| overlaps(sh.top(), sh.bottom()))
                .count();
            assert_eq!(
                in_band, slot_shapes,
                "top {top}: {} shape(s) in the slot band, but a heading alone emits \
                 {slot_shapes} — a body rung is drawing under the sticky heading",
                in_band
            );
        }
    }

    #[test]
    fn r4_3_the_slot_heading_is_a_translated_copy_not_a_reshape() {
        // D36: the copy carries the glyph runs AND the hairline rule shape. Dropping the
        // shape is easy and invisible in a diff — the heading still reads, it just loses its
        // rule on the one line the eye returns to most.
        let m = six_sections([6, 1, 1, 1, 1, 1]);
        let panel = [0.0, 0.0, 430.0, 300.0];
        let draw = layout(&m, panel, 1.0, CELL);

        let at_heading = compose(&draw, &m, 0, 0);
        let scrolled = compose(&draw, &m, 3, 0); // inside TASKS, heading off-screen
        let label_run = |f: &Primitives| {
            f.runs
                .iter()
                .find(|r| r.text == "TASKS")
                .expect("TASKS is drawn")
                .clone()
        };
        let a = label_run(&at_heading);
        let b = label_run(&scrolled);
        assert_eq!(a.rect.y, b.rect.y, "the slot holds it at the same y");
        assert_eq!(
            a.rect.x, b.rect.x,
            "x geometry is panel-relative, untouched"
        );
        assert_eq!(a.rect.w, b.rect.w);
        assert_eq!(a.color, b.color);

        // The rule travels with it: the slot band carries a shape in both frames.
        let in_slot = |f: &Primitives| {
            f.shapes
                .iter()
                .filter(|s| {
                    s.top() >= draw.slot.y - FIT_EPS
                        && s.bottom() <= draw.slot.y + draw.slot.h + FIT_EPS
                })
                .count()
        };
        assert!(in_slot(&at_heading) > 0, "premise: a heading has a rule");
        assert_eq!(
            in_slot(&scrolled),
            in_slot(&at_heading),
            "the copied heading kept its rule"
        );
    }

    /// The one precondition R0 carries, stated so it cannot be discovered by accident.
    ///
    /// A rung taller than the content band is unreachable: whole-rung culling is forced by
    /// C1 (there is no scissor rect, so a partial rung would paint over the footer), and a
    /// card is 51.8px against a band that is 25.8px on a 280px-tall panel. **This is not a
    /// regression** — the allocator this replaced had the identical property, from the
    /// identical cause: `Painter::card` skipped a card that did not fit, and `allocate`
    /// granted a row only when `remaining >= row_h`. What changed is that the panel now says
    /// so by simply not drawing, where before it said `+N more`.
    ///
    /// The window is narrow (roughly 276-328px of panel height at this width) and inside it
    /// the panel shows one row at a time, so it is a degenerate regime either way. Recorded
    /// rather than fixed because every fix costs something the spec bought deliberately:
    /// clipping breaks C1, dropping the slot breaks D5, and overflowing the band breaks C2.
    #[test]
    fn r3_7_the_real_height_sweep_never_offers_a_scroll_that_changes_nothing() {
        // D22's degenerate clause, driven through the REAL `layout` rather than through
        // hand-passed viewport heights. The distinction matters: passing `0.0`/NaN exercises
        // only the "no band at all" branch, and the branch that was actually wrong is the
        // other one — a band that exists and cannot hold a single rung whole. At 430px wide
        // with a 16px cell there is a window of panel heights that produces exactly that.
        let m = six_sections([9, 7, 5, 11, 32, 4]);
        let mut saw_degenerate_band = false;
        for h in 2..500 {
            let draw = layout(&m, [0.0, 0.0, 430.0, h as f32], 1.0, CELL);
            let max =
                crate::monitor_scroll::max_index(&draw.ladder, &draw.heights, draw.viewport.h);
            let smallest = draw
                .heights
                .iter()
                .copied()
                .filter(|x| *x > 0.0)
                .fold(f32::INFINITY, f32::min);
            if draw.viewport.h > 0.0 && draw.viewport.h < smallest {
                saw_degenerate_band = true;
                assert_eq!(
                    max, 0,
                    "h={h}: a band too small for any rung must offer no scroll, got max={max}"
                );
            }
            // And where a scroll range is claimed, its two ENDS must actually differ. The
            // first version of this asserted `compose(top=0)` was non-empty — which
            // `panel_bg` alone guarantees at every height, so it could not fail, and it hid a
            // real regression in `max_index` for a whole round.
            if max > 0 {
                assert_ne!(
                    compose(&draw, &m, 0, 0),
                    compose(&draw, &m, max, 0),
                    "h={h}: claims a scroll range 0..={max} whose ends render identically"
                );
            }
        }
        assert!(
            saw_degenerate_band,
            "premise: the sweep must actually visit a band-too-small-for-a-rung height"
        );
    }

    #[test]
    fn a_rung_taller_than_the_viewport_is_unreachable_and_always_was() {
        let m = six_sections([9, 7, 5, 11, 32, 4]);
        let draw = layout(&m, [0.0, 0.0, 430.0, 280.0], 1.0, CELL);
        let card_h = draw.heights[1];
        assert!(
            draw.viewport.h > 0.0 && draw.viewport.h < card_h,
            "premise: a band that exists but cannot hold a card ({} < {card_h})",
            draw.viewport.h
        );
        let shown: Vec<String> = (0..draw.items.len())
            .flat_map(|top| compose(&draw, &m, top, 0).runs)
            .map(|r| r.text)
            .collect();
        assert!(
            !shown.iter().any(|t| t == "TASKS/0"),
            "a card that cannot fit is not drawn — the honest outcome, not a silent clip"
        );
        // The rows, which do fit, are still reachable — the limit is per-rung, not global.
        assert!(shown.iter().any(|t| t == "SKILLS/0"));
    }

    #[test]
    fn r0_1_and_r0_2_every_retained_row_is_reachable() {
        // **The completion criterion** (D6): RECENT CALLS' retained rows — the ring's cap is
        // 32 — must all be readable by scrolling, and no rung of any section may be
        // unreachable. Asserted on drawn output, not on the ladder, so it covers the whole
        // path: full expansion, the stop rule, `max_index`, the consumption rule and the
        // whole-rung cull together.
        let m = six_sections([9, 7, 5, 11, 32, 4]);
        // Every height at which the content band can hold the tallest rung. Below that the
        // panel cannot show a card at all — see
        // `a_rung_taller_than_the_viewport_is_unreachable_and_always_was`.
        for h in [340.0f32, 400.0, 700.0] {
            let panel = [0.0, 0.0, 430.0, h];
            let draw = layout(&m, panel, 1.0, CELL);
            let tallest = draw.heights.iter().copied().fold(0.0f32, f32::max);
            assert!(
                draw.viewport.h >= tallest,
                "premise: h={h} must hold the tallest rung ({tallest}), got {}",
                draw.viewport.h
            );
            let max =
                crate::monitor_scroll::max_index(&draw.ladder, &draw.heights, draw.viewport.h);
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            for top in 0..=max {
                if !crate::monitor_scroll::is_stop(&draw.ladder, top) {
                    continue;
                }
                for r in compose(&draw, &m, top, 0).runs {
                    seen.insert(r.text);
                }
            }
            // Every RECENT CALLS row, by name.
            let missing: Vec<String> = (0..32)
                .map(|i| format!("RECENT CALLS/{i}"))
                .filter(|t| !seen.contains(t))
                .collect();
            assert!(
                missing.is_empty(),
                "h={h}: unreachable RECENT CALLS rows: {missing:?}"
            );
            // …and every other section's, plus all six headings.
            for (s, n) in m.sections.iter().zip([9, 7, 5, 11, 32, 4]) {
                assert!(seen.contains(&s.label), "h={h}: {} never shown", s.label);
                for i in 0..n {
                    let t = format!("{}/{i}", s.label);
                    assert!(seen.contains(&t), "h={h}: {t} unreachable");
                }
            }
        }
    }

    fn run_texts(draw: &Primitives) -> Vec<String> {
        draw.runs.iter().map(|r| r.text.clone()).collect()
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
                    collapsed: false,
                    label: (*label).into(),
                    total: n + 3,
                    lost: 0,
                    body,
                }
            })
            .collect();
        let mut m = model(sections);
        m.tiles = vec![StatTile::new("UPTIME", "1h 12m", ColorRole::TextPrimary); 6];
        m
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
    fn r1_4_the_footer_band_carries_one_line_and_no_control_that_lies() {
        // **This test was rewritten, and the rewrite is the record of a deliberate reversal.**
        //
        // R1.4 said the footer draws the last-update time ALONE, and gave a reason: the
        // design's `q` keycap is a *claim* — it says pressing `q` closes the panel — and the
        // panel did not answer to `q`, which went to the shell and quit `less`. A control
        // that looks like a control and is not is worse than no control.
        //
        // Both halves of that reason have changed under ai-monitor-panel-scroll:
        //
        // * The panel now HAS a binding to promise. D16 gave it `Cmd+Opt+1`..`6`, and D30
        //   makes the first of those the way back to the top — the only way, since D14
        //   forbids returning on its own.
        // * The hint is a modifier chord the panel consumes only while it is on screen, not
        //   a bare letter the shell is waiting for. `less` never sees `Cmd+Opt+1`.
        //
        // What survives untouched is the part that was really being protected: the band
        // carries **one line**, never a second run competing with it, and never a keycap box
        // — a drawn control the user could click, which the panel still has no answer for.
        let m = model(vec![rows_section("TASKS", 3, 3)]);
        let panel = [0.0, 0.0, 430.0, 400.0];
        let draw = layout(&m, panel, 1.0, CELL);
        let band_top = draw.footer_text.y;

        for badge in [0u32, 12] {
            let f = compose(&draw, &m, 0, badge);
            let in_band: Vec<&str> = f
                .runs
                .iter()
                .filter(|r| r.rect.y >= band_top)
                .map(|r| r.text.as_str())
                .collect();
            assert_eq!(
                in_band.len(),
                1,
                "badge={badge}: the band must carry exactly one line, got {in_band:?}"
            );
            if badge == 0 {
                assert_eq!(in_band[0], m.footer, "with no badge it is still the time");
            } else {
                assert!(
                    in_band[0].contains("12"),
                    "with a badge it is the badge, replacing the time: {in_band:?}"
                );
            }

            // And no keycap *box* in either state: a `q` in a rounded rect is a shape before
            // it is a glyph, so counting runs alone would miss an empty keycap. The chord
            // hint is text, and text is all it may ever be — a drawn control would invite a
            // click the panel has no answer for (D10 keeps it click-free).
            let shapes_in_band: Vec<[f32; 4]> = f
                .shapes
                .iter()
                .filter(|s| s.top() >= band_top)
                .map(|s| s.bbox)
                .collect();
            assert!(
                shapes_in_band.is_empty(),
                "badge={badge}: a shape begins inside the footer's line box: {shapes_in_band:?}"
            );
        }
    }

    #[test]
    fn an_empty_section_states_itself_on_the_heading() {
        // Same placement rule: an empty state must not spend the row a populated
        // neighbour was guaranteed.
        let m = model(vec![Section {
            collapsed: false,
            label: "SKILLS".into(),
            total: 0,
            lost: 0,
            body: SectionBody::Empty("none yet".into()),
        }]);
        let draw = frame(&m, [0.0, 0.0, 430.0, 400.0], 1.0, CELL);
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
            let draw = frame(&m, panel, 1.0, CELL);
            assert!(
                draw.shapes.is_empty() && draw.runs.is_empty(),
                "panel {panel:?} drew something"
            );
        }
        // A zero cell width would divide by zero in every column budget.
        assert_eq!(
            frame(&m, [0.0, 0.0, 430.0, 400.0], 1.0, [0.0, 16.0]),
            Primitives::default()
        );
        // An invisible model draws nothing at all — not even the background.
        let hidden = MonitorModel {
            visible: false,
            ..m.clone()
        };
        assert_eq!(
            frame(&hidden, [0.0, 0.0, 430.0, 400.0], 1.0, CELL),
            Primitives::default()
        );
    }

    #[test]
    fn a_panel_too_narrow_for_content_still_paints_its_band() {
        // inner_w ≤ 0: the chrome has nowhere to go, but the band must still be filled or
        // the terminal shows through the reserved columns.
        let m = model(vec![rows_section("TASKS", 3, 3)]);
        let draw = frame(&m, [0.0, 0.0, 12.0, 400.0], 1.0, CELL);
        assert!(!draw.shapes.is_empty(), "background and border still drawn");
        assert!(draw.runs.is_empty(), "no text fits");
    }

    // ---- R5.1 / D10 / D18: the floor, and what yields ----

    /// R5.1 as amended, swept across every panel height a window can produce — **starting
    /// below the floor**, because that is where the last two defects in this file lived.
    ///
    /// **R5.1's Given used to read "Any panel height" and the code did not honour it** —
    /// found by the Gate-3 checker, which measured the stat grid vanishing entirely on a
    /// short panel. D10's intent is unchanged (the panel does not drop regions on a
    /// whim); what is recorded here is the shape of the one place it cannot hold, so the
    /// yielding is a rule with a direction instead of whatever the arithmetic happened to
    /// do.
    ///
    /// **The sweep used to start at 80px, and that was the hole the second defect fell
    /// through** (Gate-3 checker, iteration 2). 80px is *inside* the band where the floor
    /// does not stack, and the floor check was presence-only — `any(|t| t == pinned)` —
    /// so a session row drawn straight through the footer's glyphs satisfied it. The sweep
    /// now starts at 2px and asserts the regions against **each other**, which is the only
    /// form of the assertion that can tell "drawn" from "readable".
    ///
    /// The properties, at every height in the sweep:
    ///
    /// * **No two floor regions' line boxes overlap.** This is the probe; everything else
    ///   is presence, and presence cannot see a collision.
    /// * **The floor yields in a fixed order, at measured heights**, asserted as an *iff*
    ///   so a region that reappears 2px early — back across the footer — fails as loudly
    ///   as one that goes missing. The order is the constants' own order: the footer
    ///   outlasts the header band, which outlasts the session row. Every region above the
    ///   footer reads `Painter::footer_top` as its ceiling, so the pinned footer is the
    ///   last thing standing, which is what "the footer survives any height" has to mean.
    /// * The tiles drawn are a **prefix** of the model's, in whole rows of `TILE_COLS`,
    ///   and never before the whole floor is up. A partial row is the failure that
    ///   matters: UPTIME and IDLE without ERRORS reads as "no errors", which is a claim,
    ///   not a shortfall.
    /// * The count is **monotonic** in the height and no tile crosses the pinned footer.
    ///   Monotonic is what makes a window drag feel like a window drag: dragging taller
    ///   may add a row and may never take one away. The floor's own iffs give it the same
    ///   property, which is why the session row is not allowed to squat at the panel's top
    ///   when the header is dropped (see `layout`) — that state would appear at 52.8px and
    ///   disappear again at 64.8px.
    #[test]
    fn r5_1_the_floor_survives_every_height_and_the_stat_grid_yields_whole_rows() {
        const TITLE: &str = "MONITOR-TITLE";
        const SESSION: &str = "SESSION-ROW";
        const FOOTER: &str = "FOOTER-STAMP";
        // Measured off this panel — 430px wide, a 16px cell, scale 1 — by sweeping the
        // real `layout` in 0.1px steps, not derived by hand: `line` = 16 × 1.15 = 18.4, the
        // header band = line + 2·10 = 38.4, the footer band = line + 8 = 26.4.
        //
        //   26.4px  the footer band first clears the panel's top edge
        //   64.8px  the header band first clears the footer's rule (38.4 ≤ h − 26.4)
        //   91.2px  the session line box first clears it (46.4 + 18.4 ≤ h − 26.4) — the floor
        //
        // A token change (`LINE_FACTOR`, `GAP`, `HEADER_PAD_Y`) moves these, and it should:
        // the numbers are a measurement of the design's own spacing, and the spec's Known
        // Gap quotes them.
        const FOOTER_MIN: f32 = 26.4;
        const HEADER_MIN: f32 = 64.8;
        const FLOOR: f32 = 91.2;
        let tile_label = |i: usize| format!("TILE{i}");

        let mut m = model(
            (0..6)
                .map(|i| rows_section(&format!("S{i}"), 4, 4))
                .collect(),
        );
        m.title = TITLE.into();
        m.session.name = SESSION.into();
        m.footer = FOOTER.into();
        m.tiles = (0..6)
            .map(|i| StatTile::new(tile_label(i), "1", ColorRole::Teal))
            .collect();

        let mut prev = 0usize;
        let mut saw_partial_grid = false;
        let mut overlaps: Vec<String> = Vec::new();
        let mut bands = [false; 4];
        // 2px steps from 2px: no height in the sweep lands within `FIT_EPS` of a threshold,
        // so each `h >= …` below is unambiguous.
        let mut h = 2.0f32;
        while h <= 900.0 {
            let draw = frame(&m, [0.0, 0.0, 430.0, h], 1.0, CELL);
            let texts = run_texts(&draw);
            for (pinned, min) in [(TITLE, HEADER_MIN), (SESSION, FLOOR), (FOOTER, FOOTER_MIN)] {
                assert_eq!(
                    texts.iter().any(|t| t == pinned),
                    h >= min,
                    "at height {h}, {pinned} drawn should be {}",
                    h >= min
                );
            }
            bands[[h >= FOOTER_MIN, h >= HEADER_MIN, h >= FLOOR]
                .iter()
                .filter(|b| **b)
                .count()] = true;

            // C2/R2.2 — the same sweep with the badge up. This is the whole reason C2 named
            // this test: the badge is the only defence against the failure Inversion ranked
            // first, the footer is the ONLY pinned region that survives to the floor, and the
            // band between FOOTER_MIN and HEADER_MIN is where every other region has already
            // yielded. A badge that vanished exactly there would be a warning that disappears
            // when the panel is at its least readable.
            let d = layout(&m, [0.0, 0.0, 430.0, h], 1.0, CELL);
            let badged = compose(&d, &m, 0, 9);
            let footer_line = badged
                .runs
                .iter()
                .find(|r| d.footer_text.h > 0.0 && r.rect.y >= d.footer_text.y - FIT_EPS);
            assert_eq!(
                footer_line.is_some(),
                h >= FOOTER_MIN,
                "at height {h}, a badged footer line should be drawn: {}",
                h >= FOOTER_MIN
            );
            if let Some(line) = footer_line {
                assert!(
                    line.text.contains('9') && line.text != FOOTER,
                    "at height {h}, the badge must REPLACE the stamp, not sit beside it: {:?}",
                    line.text
                );
            }

            // Presence is not legibility. Three runs can each be emitted and still land on
            // top of one another, which is what the `any(...)` checks above cannot see —
            // so every pair of floor line boxes is measured against every other.
            let boxes: Vec<(&str, f32, f32)> = [TITLE, SESSION, FOOTER]
                .iter()
                .filter_map(|name| {
                    draw.runs
                        .iter()
                        .find(|r| r.text == *name)
                        .map(|r| (*name, r.rect.y, r.rect.y + r.rect.h))
                })
                .collect();
            for (i, (an, a0, a1)) in boxes.iter().enumerate() {
                for (bn, b0, b1) in &boxes[i + 1..] {
                    let over = a1.min(*b1) - a0.max(*b0);
                    if over > FIT_EPS {
                        overlaps.push(format!(
                            "h={h}: {an} @ y={a0}..{a1} overlaps {bn} @ y={b0}..{b1} by {over}px"
                        ));
                    }
                }
            }

            let drawn: Vec<bool> = (0..6)
                .map(|i| texts.iter().any(|t| *t == tile_label(i)))
                .collect();
            let n = drawn.iter().filter(|d| **d).count();
            assert_eq!(
                &drawn[..n],
                &vec![true; n][..],
                "height {h} drew {n} tiles but not the first {n}: {drawn:?}"
            );
            assert_eq!(
                n % TILE_COLS,
                0,
                "height {h} drew a partial row of {n} tiles"
            );
            assert!(
                n >= prev,
                "height {h} dropped a tile that {} px drew ({prev} → {n})",
                h - 2.0
            );
            // The grid is the accessory (R5.1/D4): it may not outlive the identity it
            // decorates, so no tile may be drawn at a height the floor itself does not
            // survive.
            assert!(
                n == 0 || h >= FLOOR,
                "height {h} drew {n} tiles below the floor"
            );
            if n > 0 && n < m.tiles.len() {
                saw_partial_grid = true;
            }
            prev = n;

            // Nothing the grid draws may cross the footer. The footer's text sits half a
            // GAP below its rule (see `footer`), and `scale` is 1 here, so its run's `y`
            // less that half-gap is the pinned ceiling. Below `FOOTER_MIN` there is no
            // footer to cross — and, per the assertion above, no tile either.
            if let Some(footer_y) = draw
                .runs
                .iter()
                .find(|r| r.text == FOOTER)
                .map(|r| r.rect.y - GAP * 0.5)
            {
                for s in draw.shapes.iter().filter(|s| {
                    s.kind == crate::sidebar_shapes::KIND_ROUNDED_BOX && s.geom[1] > 0.0
                }) {
                    assert!(
                        s.bbox[1] + s.bbox[3] <= footer_y + FIT_EPS,
                        "a stat tile at height {h} runs to {}, past the footer at {footer_y}",
                        s.bbox[1] + s.bbox[3]
                    );
                }
            }
            h += 2.0;
        }
        assert!(
            overlaps.is_empty(),
            "floor regions overlapped at {} heights:\n{}",
            overlaps.len(),
            overlaps.join("\n")
        );
        // The sweep is only meaningful if it actually crossed every boundary: all four
        // rungs of the ladder (nothing, footer, +header, +session) and both grid edges.
        assert_eq!(
            bands, [true; 4],
            "the sweep skipped a rung of the yield ladder: {bands:?}"
        );
        assert!(
            saw_partial_grid,
            "the sweep never caught the grid mid-yield"
        );
        assert_eq!(
            prev, 6,
            "the sweep never reached a height drawing the whole grid"
        );
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
            frame(&m, [0.0, 0.0, 430.0, 400.0], 1.0, CELL).shapes.len()
        };
        let none = tile(None);
        let zero = tile(Some(0.0));
        let some = tile(Some(0.45));
        assert_eq!(zero, none + 1, "Some(0.0) draws the track and no fill");
        assert_eq!(some, none + 2, "Some(0.45) draws the track and its fill");
    }

    /// R5.4: the fill really is `tile.gauge` **of the track**, at several fractions.
    ///
    /// `GAUGE_TOL` is 0.02 and it is not slack. A capsule's `seg` endpoints are inset by
    /// half its thickness at each end — the round caps paint the rest — so the measured
    /// segment ratio deviates from the requested fraction by `r·|2f−1| / (w − 2r)`, at
    /// most 1.3% of the track at this size. Everything above that is the fraction.
    ///
    /// The window this assertion used to carry (`0.35..0.55` for a single `f = 0.45`) was
    /// wide enough that a bar ignoring `StatTile::gauge` and painting a constant half
    /// passed it — and passed the whole workspace with it. A gauge that is not
    /// proportional is not a gauge, so the tolerance has to be tight enough to say so.
    #[test]
    fn r5_4_the_gauge_fill_is_proportional() {
        const GAUGE_TOL: f32 = 0.02;

        // `(track span, fill span)` for a CONTEXT tile drawn at `f`.
        let gauge = |f: f32| -> (f32, f32) {
            let mut m = model(Vec::new());
            m.tiles = vec![StatTile::new("CONTEXT", "45%", ColorRole::Teal).with_gauge(f)];
            let draw = frame(&m, [0.0, 0.0, 430.0, 400.0], 1.0, CELL);
            // The gauge is the only pair of horizontal capsules at half the gauge
            // thickness (the header accent shares the thickness but runs vertically; the
            // rules are thinner).
            let bars: Vec<&SidebarShape> = draw
                .shapes
                .iter()
                .filter(|s| {
                    s.kind == crate::sidebar_shapes::KIND_CAPSULE
                        && (s.geom[0] - GAUGE_H * 0.5).abs() < FIT_EPS
                        && (s.seg[1] - s.seg[3]).abs() < FIT_EPS
                })
                .collect();
            assert_eq!(bars.len(), 2, "a track and its fill at gauge {f}");
            let span = |s: &SidebarShape| s.seg[2] - s.seg[0];
            (span(bars[0]), span(bars[1]))
        };

        for f in [0.2f32, 0.45, 0.75, 0.9] {
            let (track, fill) = gauge(f);
            assert!(
                track > 0.0,
                "a track with no width is not a fraction of anything"
            );
            let ratio = fill / track;
            assert!(
                (ratio - f).abs() < GAUGE_TOL,
                "gauge {f} drew fill/track = {ratio} ({fill}px of {track}px)"
            );
        }

        // Two fractions must be a proportional distance apart, not merely both plausible:
        // this is what pins the *slope*, so a bar keyed on anything but `gauge` — a
        // constant, the tile index, the value string's length — moves the wrong amount.
        let (track, quarter) = gauge(0.25);
        let (_, three_quarters) = gauge(0.75);
        let delta = (three_quarters - quarter) / track;
        assert!(
            (delta - 0.5).abs() < GAUGE_TOL,
            "0.25 drew {quarter}px and 0.75 drew {three_quarters}px of a {track}px track: \
             {delta} of a track apart, expected half"
        );
    }

    // ---- column-based measurement ----

    #[test]
    fn hangul_is_measured_in_columns_not_chars() {
        // `한` is 2 columns: a 6-char title reserves 12 cells, and a char-counting rule
        // would reserve 6 and overlap every glyph with the value beside it.
        let title = "한글한글한글";
        let m = model(vec![Section {
            collapsed: false,
            label: "TASKS".into(),
            total: 1,
            lost: 0,
            body: SectionBody::Rows(vec![Row {
                text: title.into(),
                ..Row::default()
            }]),
        }]);
        let draw = frame(&m, [0.0, 0.0, 430.0, 400.0], 1.0, CELL);
        let run = draw
            .runs
            .iter()
            .find(|r| r.text == title)
            .expect("the whole title fits a 430px panel");
        assert_eq!(run.rect.w, 12.0 * CELL[0]);

        // Narrow it until the row's budget clips: the prefix is cut on a column budget,
        // so it holds half as many chars as columns and never splits a codepoint.
        let narrow = frame(&m, [0.0, 0.0, 90.0, 400.0], 1.0, CELL);
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
                collapsed: false,
                label: "AGENTS".into(),
                total: 2,
                lost: 0,
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
                collapsed: false,
                label: "TOOL USAGE".into(),
                total: 3,
                lost: 0,
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
        let draw = frame(&m, panel, 2.0, [17.0, 34.0]);
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
            collapsed: false,
            label: "FILE ACTIVITY".into(),
            total: 2,
            lost: 0,
            body: SectionBody::Rows(rows),
        }]);
        let draw = frame(&m, [0.0, 0.0, 430.0, 400.0], 1.0, CELL);
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
            let draw = frame(&m, [0.0, 0.0, 430.0, 400.0], 1.0, CELL);
            let run = draw
                .runs
                .iter()
                .find(|r| r.text == label)
                .unwrap_or_else(|| panic!("{label} pill drawn"));
            assert_eq!(run.color, role.packed());
        }
    }
}
