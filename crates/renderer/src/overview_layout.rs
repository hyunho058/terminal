//! Layout of the Agent Monitor overview and session detail (session-overview-ui D15, D20,
//! D21). Pure: model + viewport + scale + scroll in, rectangles, text runs and hits out, so
//! every size, threshold and hit is testable without a GPU.
//!
//! Every length is **physical px** (logical design px × `scale`), and every rect is
//! `[x, y, w, h]`, window-absolute. The constants below are the design's logical px (D15).
//!
//! **Text is measured by column**, the way the sidebar's cards are: one display column of a
//! face is [`column_px`] of its ppem, a line is [`line_px`]. Every run is ellipsised to the
//! cell it is given before it is placed, so two runs can never overlap (R9.2).
//!
//! **No scissor rect** (the carried rule): the table and the detail's lanes scroll in whole
//! lines. A line is drawn only when it lies wholly inside its box, and the box itself is
//! the same at every offset, so the header, tiles, banner and filter never move (R9.4).
//!
//! **Colours.** [`LaidText::color`] and [`LaidSquare::color`] are straight **sRGB** RGBA,
//! comparable with the design's hex values; [`OverviewLayout::shapes`] are already
//! **linear**, ready for the shape pipeline (see `overview_theme`).

use crate::overview_model::{
    Bucket, DetailData, EventTone, OverviewHit, OverviewModel, SessionRowData, SubagentCell,
    SubagentStatus,
};
use crate::overview_theme as th;
use crate::sidebar_icons::{push_icon, Icon};
use crate::sidebar_shapes::{Fill, SidebarShape};
use crate::sidebar_text::{column_px, line_px, CardFace};
use crate::tab_bar::{char_cols, truncate_cols};

/// The three text faces the overview draws with (D21). Not [`CardFace`] variants: the
/// overview panel owns its own atlases, so the sidebar builds no extra one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OverviewFace {
    /// 17 px bold: the view title, project names, the selected subagent's title.
    Title,
    /// 12.5 px: everything else.
    Meta,
    /// 30 px: the four tile numbers only.
    Display,
}

impl OverviewFace {
    pub const ALL: [OverviewFace; 3] = [
        OverviewFace::Title,
        OverviewFace::Meta,
        OverviewFace::Display,
    ];

    /// Design size of [`OverviewFace::Display`], logical px (D21).
    pub const DISPLAY_LOGICAL_PX: f32 = 30.0;

    /// The atlas ppem at `scale`, physical px.
    pub fn ppem(self, scale: f64) -> f32 {
        match self {
            OverviewFace::Title => CardFace::Title.ppem(scale),
            OverviewFace::Meta => CardFace::Meta.ppem(scale),
            OverviewFace::Display => (Self::DISPLAY_LOGICAL_PX as f64 * scale) as f32,
        }
    }

    /// One display column's width at `scale`, physical px — the measure every run of this
    /// face is laid out and ellipsised by.
    pub fn column_px(self, scale: f64) -> f32 {
        column_px(self.ppem(scale))
    }

    /// One line's height at `scale`, physical px.
    pub fn line_px(self, scale: f64) -> f32 {
        line_px(self.ppem(scale))
    }

    /// Index into a per-face array, in [`OverviewFace::ALL`] order.
    pub fn index(self) -> usize {
        match self {
            OverviewFace::Title => 0,
            OverviewFace::Meta => 1,
            OverviewFace::Display => 2,
        }
    }
}

/// One laid-out text run: what to draw, where, in which face and colour (straight RGBA,
/// 0..=1). `rect` is the run's box after ellipsis.
#[derive(Clone, Debug, PartialEq)]
pub struct LaidText {
    pub text: String,
    pub rect: [f32; 4],
    pub face: OverviewFace,
    pub color: [f32; 4],
    /// Drawn from the bold face. Always true for [`OverviewFace::Title`] and
    /// [`OverviewFace::Display`]; a `Meta` run is bold where the design sets 600/700.
    pub bold: bool,
}

/// One drawn subagent square (or `+k` slot), D8.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaidSquare {
    pub cell: SubagentCell,
    pub rect: [f32; 4],
    pub color: [f32; 4],
}

/// The table's column widths, physical px, after D15's narrow-window rules. A dropped
/// column is `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Columns {
    pub status: f32,
    pub session: f32,
    pub now_doing: f32,
    pub subagents: Option<f32>,
    pub elapsed: Option<f32>,
    pub buttons: f32,
    /// Whether the Subagents column shows its summary text (false at the 88 px width).
    pub subagent_summary: bool,
}

/// A laid-out overview or detail screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverviewLayout {
    pub viewport: [f32; 2],
    pub scale: f64,
    /// The centred content box, `min(viewport − padding, 1376 logical)` wide.
    pub content: [f32; 4],
    /// Overview only: the four tiles in order Needs you, Working, Idle, Subagents.
    pub tiles: Vec<[f32; 4]>,
    /// Whether the tiles wrapped to 2 × 2.
    pub tiles_wrapped: bool,
    /// Overview only: the table box (header row excluded).
    pub table: [f32; 4],
    pub columns: Columns,
    /// Detail only: the timeline box and the side panel box.
    pub timeline: Option<[f32; 4]>,
    pub side_panel: Option<[f32; 4]>,
    /// Whether the side panel sits beside the timeline (true) or below it (false).
    pub side_by_side: bool,
    /// Scrollable lines in the list (table rows + tree lines, or detail lanes), and how
    /// many fit.
    pub lines_total: usize,
    pub lines_visible: usize,
    /// The `Window too narrow …` state (D15).
    pub too_narrow: bool,
    pub texts: Vec<LaidText>,
    pub squares: Vec<LaidSquare>,
    pub hits: Vec<(OverviewHit, [f32; 4])>,
    /// Every filled shape, in draw order (the full-window backdrop first), linear RGBA.
    /// Text draws over all of them.
    pub shapes: Vec<SidebarShape>,
}

impl OverviewLayout {
    /// The largest scroll offset, in whole lines: `lines_total − lines_visible`, floored at 0.
    pub fn max_scroll(&self) -> usize {
        self.lines_total.saturating_sub(self.lines_visible)
    }

    /// The hit under `point`, if any (rects half-open on the far edges).
    pub fn hit_at(&self, point: [f32; 2]) -> Option<OverviewHit> {
        self.hit_rect_at(point).map(|(hit, _)| hit)
    }

    /// [`hit_at`](Self::hit_at) with the hit's rect — the hover highlight's box.
    pub fn hit_rect_at(&self, point: [f32; 2]) -> Option<(OverviewHit, [f32; 4])> {
        let [px, py] = point;
        self.hits
            .iter()
            .rev()
            .find(|(_, [x, y, w, h])| px >= *x && px < x + w && py >= *y && py < y + h)
            .copied()
    }
}

// ---- design constants, logical px (D15) --------------------------------------------------

const SIDE_PAD: f32 = 32.0;
const TOP_PAD: f32 = 28.0;
const DETAIL_TOP_PAD: f32 = 24.0;
const BOTTOM_PAD: f32 = 48.0;
const MAX_CONTENT: f32 = 1376.0;
const SECTION_GAP: f32 = 20.0;
const HEADER_H: f32 = 44.0;
const TILE_H: f32 = 104.0;
const TILE_GAP: f32 = 12.0;
const BANNER_H: f32 = 48.0;
const TABLE_HEADER_H: f32 = 40.0;
const ROW_H: f32 = 72.0;
const TREE_H: f32 = 32.0;
const LEGEND_H: f32 = 28.0;
const COL_GAP: f32 = 16.0;
const ROW_PAD: f32 = 20.0;
const STATUS_W: f32 = 120.0;
const SESSION_MIN: f32 = 240.0;
const NOW_MIN: f32 = 280.0;
const NOW_MIN_NARROW: f32 = 160.0;
const SUBAGENTS_W: f32 = 220.0;
const SUBAGENTS_NARROW_W: f32 = 88.0;
const ELAPSED_W: f32 = 88.0;
const BUTTONS_W: f32 = 104.0;
const BUTTON: f32 = 44.0;
const SQUARE: f32 = 12.0;
const SQUARE_GAP: f32 = 3.0;
/// The empty table's height around its one line (`padding: 40px 20px`).
const EMPTY_PAD: f32 = 40.0;

/// D15's thresholds against the content width W.
const W_FULL: f32 = 1172.0;
const W_NOW_NARROW: f32 = 1052.0;
const W_ELAPSED: f32 = 920.0;
const W_SUBAGENTS: f32 = 816.0;
const W_MIN: f32 = 712.0;

/// Detail.
const SIDE_PANEL_W: f32 = 340.0;
const TWO_COLUMN_MIN: f32 = 1000.0;
const LANE_H: f32 = 40.0;
const LANE_LABEL_W: f32 = 240.0;
const TICK_H: f32 = 22.0;

const TOO_NARROW: &str = "Window too narrow — widen it to see sessions.";
const EMPTY_FILTER: &str = "No sessions in this filter.";
const NO_SESSIONS: &str = "No agent sessions in this window.";
const NO_TIMELINE: &str = "No timeline — only Claude Code sessions record subagents.";
const ELLIPSIS: char = '…';

// ---- text measurement ------------------------------------------------------------------

/// Display columns of `s` (Hangul and CJK are two).
pub fn text_cols(s: &str) -> usize {
    s.chars().map(char_cols).sum()
}

/// `text` cut to fit `max_w` px at `col` px per column, with a trailing `…` when cut.
/// `None` when nothing fits (or `text` is empty) — the run is then not drawn at all.
pub fn ellipsize(text: &str, max_w: f32, col: f32) -> Option<String> {
    if text.is_empty() || max_w.is_nan() || col.is_nan() || max_w <= 0.0 || col <= 0.0 {
        return None;
    }
    let budget = (max_w / col + 1e-3).floor() as usize;
    if text_cols(text) <= budget {
        return Some(text.to_string());
    }
    if budget < 2 {
        return None;
    }
    let mut head = truncate_cols(text, budget - 1);
    if head.is_empty() {
        return None;
    }
    head.push(ELLIPSIS);
    Some(head)
}

/// The square slots a row draws in a Subagents column holding `cap` slots (D8): the cells
/// as given when they fit, else the first `cap − 1` squares and a `+{k}` counting every
/// subagent not drawn (an incoming `More` included).
pub fn square_slots(cells: &[SubagentCell], cap: usize) -> Vec<SubagentCell> {
    if cells.len() <= cap {
        return cells.to_vec();
    }
    if cap == 0 {
        return Vec::new();
    }
    let total: usize = cells
        .iter()
        .map(|c| match c {
            SubagentCell::Square(_) => 1,
            SubagentCell::More(k) => *k,
        })
        .sum();
    let mut out: Vec<SubagentCell> = cells
        .iter()
        .copied()
        .filter(|c| matches!(c, SubagentCell::Square(_)))
        .take(cap - 1)
        .collect();
    let shown = out.len();
    out.push(SubagentCell::More(total.saturating_sub(shown)));
    out
}

/// D15's columns at content width `w` (logical px), or `None` below 712.
fn columns_logical(w: f32) -> Option<Columns> {
    if w < W_MIN {
        return None;
    }
    let subagents = if w >= W_NOW_NARROW {
        Some(SUBAGENTS_W)
    } else if w >= W_SUBAGENTS {
        Some(SUBAGENTS_NARROW_W)
    } else {
        None
    };
    let elapsed = (w >= W_ELAPSED).then_some(ELAPSED_W);
    let now_min = if w >= W_FULL { NOW_MIN } else { NOW_MIN_NARROW };
    let count = 4 + usize::from(subagents.is_some()) + usize::from(elapsed.is_some());
    let fixed = STATUS_W + BUTTONS_W + subagents.unwrap_or(0.0) + elapsed.unwrap_or(0.0);
    let rest = (w - 2.0 * ROW_PAD - fixed - COL_GAP * (count - 1) as f32).max(0.0);
    // CSS `minmax(240, 1.2fr)` / `minmax(min, 2fr)`: the fr share, each floored at its
    // minimum, the other column taking what is left.
    let mut session = rest * 1.2 / 3.2;
    let mut now = rest - session;
    if session < SESSION_MIN {
        session = SESSION_MIN;
        now = rest - session;
    }
    if now < now_min {
        now = now_min;
        session = (rest - now).max(SESSION_MIN);
    }
    Some(Columns {
        status: STATUS_W,
        session,
        now_doing: now,
        subagents,
        elapsed,
        buttons: BUTTONS_W,
        subagent_summary: subagents == Some(SUBAGENTS_W),
    })
}

fn scale_columns(c: Columns, s: f32) -> Columns {
    Columns {
        status: c.status * s,
        session: c.session * s,
        now_doing: c.now_doing * s,
        subagents: c.subagents.map(|v| v * s),
        elapsed: c.elapsed.map(|v| v * s),
        buttons: c.buttons * s,
        subagent_summary: c.subagent_summary,
    }
}

// ---- the sink every layout writes into ---------------------------------------------------

struct Ink {
    scale: f64,
    s: f32,
    texts: Vec<LaidText>,
    shapes: Vec<SidebarShape>,
    squares: Vec<LaidSquare>,
    hits: Vec<(OverviewHit, [f32; 4])>,
}

impl Ink {
    fn new(scale: f64) -> Ink {
        Ink {
            scale,
            s: scale as f32,
            texts: Vec::new(),
            shapes: Vec::new(),
            squares: Vec::new(),
            hits: Vec::new(),
        }
    }

    fn col(&self, face: OverviewFace) -> f32 {
        face.column_px(self.scale)
    }

    fn line(&self, face: OverviewFace) -> f32 {
        face.line_px(self.scale)
    }

    fn width(&self, text: &str, face: OverviewFace) -> f32 {
        text_cols(text) as f32 * self.col(face)
    }

    /// Place `text` with its line box's top-left at `(x, y)`, ellipsised to `max_w`.
    /// Returns the drawn width (0 when nothing fit).
    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        face: OverviewFace,
        color: u32,
        bold: bool,
        max_w: f32,
    ) -> f32 {
        let Some(t) = ellipsize(text, max_w, self.col(face)) else {
            return 0.0;
        };
        let w = self.width(&t, face);
        self.texts.push(LaidText {
            text: t,
            rect: [x, y, w, self.line(face)],
            face,
            color: th::srgb(color),
            bold: bold || face != OverviewFace::Meta,
        });
        w
    }

    /// [`text`](Self::text) centred vertically in `[top, top + h)`.
    #[allow(clippy::too_many_arguments)]
    fn text_mid(
        &mut self,
        text: &str,
        x: f32,
        top: f32,
        h: f32,
        face: OverviewFace,
        color: u32,
        bold: bool,
        max_w: f32,
    ) -> f32 {
        let y = top + (h - self.line(face)) / 2.0;
        self.text(text, x, y, face, color, bold, max_w)
    }

    /// [`text`](Self::text) ending at `right`.
    #[allow(clippy::too_many_arguments)]
    fn text_right(
        &mut self,
        text: &str,
        right: f32,
        y: f32,
        face: OverviewFace,
        color: u32,
        bold: bool,
        max_w: f32,
    ) -> f32 {
        let Some(t) = ellipsize(text, max_w, self.col(face)) else {
            return 0.0;
        };
        let w = self.width(&t, face);
        self.text(&t, right - w, y, face, color, bold, w)
    }

    fn fill(&mut self, rect: [f32; 4], radius: f32, color: u32) {
        if let Some(sh) = SidebarShape::rounded_box(
            rect,
            radius,
            0.0,
            Fill::solid(th::linear(color, 1.0)),
            [0.0; 4],
        ) {
            self.shapes.push(sh);
        }
    }

    /// A rounded box with a 1 px (logical) border; `fill: None` leaves it hollow.
    fn boxed(&mut self, rect: [f32; 4], radius: f32, fill: Option<u32>, border: u32) {
        let fill = fill.map_or([0.0; 4], |c| th::linear(c, 1.0));
        if let Some(sh) = SidebarShape::rounded_box(
            rect,
            radius,
            self.s,
            Fill::solid(fill),
            th::linear(border, 1.0),
        ) {
            self.shapes.push(sh);
        }
    }

    fn dot(&mut self, cx: f32, cy: f32, d: f32, color: u32) {
        if let Some(sh) =
            SidebarShape::capsule(cx, cy, cx, cy, d, Fill::solid(th::linear(color, 1.0)))
        {
            self.shapes.push(sh);
        }
    }

    fn ring(&mut self, cx: f32, cy: f32, d: f32, stroke: f32, color: u32) {
        let r = (d - stroke) / 2.0;
        if let Some(sh) = SidebarShape::ring(cx, cy, r, stroke, Fill::solid(th::linear(color, 1.0)))
        {
            self.shapes.push(sh);
        }
    }

    fn stroke(&mut self, a: [f32; 2], b: [f32; 2], w: f32, color: u32) {
        if let Some(sh) = SidebarShape::capsule(
            a[0],
            a[1],
            b[0],
            b[1],
            w,
            Fill::solid(th::linear(color, 1.0)),
        ) {
            self.shapes.push(sh);
        }
    }

    fn hit(&mut self, hit: OverviewHit, rect: [f32; 4]) {
        self.hits.push((hit, rect));
    }

    fn finish(self, mut out: OverviewLayout) -> OverviewLayout {
        out.texts = self.texts;
        out.shapes = self.shapes;
        out.squares = self.squares;
        out.hits = self.hits;
        out
    }
}

/// The content box's `(x, width)` for a viewport width (D15).
fn content_x(vw: f32, s: f32) -> (f32, f32) {
    let cw = (vw - 2.0 * SIDE_PAD * s).min(MAX_CONTENT * s).max(0.0);
    (((vw - cw) / 2.0).max(0.0), cw)
}

/// `“{text}”`, the text ellipsised so the quotes always survive.
fn quoted(text: &str, max_w: f32, col: f32) -> Option<String> {
    let inner = ellipsize(text, max_w - 2.0 * col, col)?;
    Some(format!("“{inner}”"))
}

// ---- the overview ------------------------------------------------------------------------

/// Lay out the overview (D15). `viewport` is `[w, h]` physical px; `scroll` is the table's
/// whole-line offset, clamped here.
pub fn layout_overview(
    model: &OverviewModel,
    viewport: [f32; 2],
    scale: f64,
    scroll: usize,
) -> OverviewLayout {
    let mut ink = Ink::new(scale);
    let s = ink.s;
    let [vw, vh] = viewport;
    ink.fill([0.0, 0.0, vw, vh], 0.0, th::BG);
    let (cx, cw) = content_x(vw, s);
    let top = TOP_PAD * s;
    let mut out = OverviewLayout {
        viewport,
        scale,
        content: [cx, top, cw, (vh - top - BOTTOM_PAD * s).max(0.0)],
        ..OverviewLayout::default()
    };
    let header_bottom = overview_header(&mut ink, model, cx, top, cw);

    let Some(logical) = columns_logical(cw / s) else {
        out.too_narrow = true;
        ink.text(
            TOO_NARROW,
            cx,
            header_bottom + SECTION_GAP * s,
            OverviewFace::Meta,
            th::MUTED,
            false,
            cw,
        );
        return ink.finish(out);
    };
    let cols = scale_columns(logical, s);
    out.columns = cols;

    // ---- tiles ----
    let wrapped = cw / s < W_NOW_NARROW;
    let per_row = if wrapped { 2 } else { 4 };
    let gap = TILE_GAP * s;
    let tw = (cw - gap * (per_row - 1) as f32) / per_row as f32;
    let th_ = TILE_H * s;
    let ty0 = header_bottom + SECTION_GAP * s;
    let tiles: Vec<[f32; 4]> = (0..4)
        .map(|i| {
            let (r, c) = (i / per_row, i % per_row);
            [
                cx + c as f32 * (tw + gap),
                ty0 + r as f32 * (th_ + gap),
                tw,
                th_,
            ]
        })
        .collect();
    for (i, t) in tiles.iter().enumerate() {
        tile(&mut ink, model, i, *t);
    }
    let tile_rows = 4 / per_row;
    let mut y = ty0 + tile_rows as f32 * th_ + (tile_rows - 1) as f32 * gap;
    out.tiles = tiles;
    out.tiles_wrapped = wrapped;

    // ---- banner ----
    if let Some(b) = &model.banner {
        y += SECTION_GAP * s;
        banner(&mut ink, b, [cx, y, cw, BANNER_H * s]);
        y += BANNER_H * s;
    }

    // ---- the table ----
    let sec_y = y + SECTION_GAP * s;
    let table_y = sec_y + TABLE_HEADER_H * s;
    let bottom = vh - (BOTTOM_PAD + SECTION_GAP + LEGEND_H) * s;
    let avail = (bottom - table_y).max(0.0);

    let mut lines: Vec<(usize, Option<usize>)> = Vec::new();
    for (i, r) in model.rows.iter().enumerate() {
        lines.push((i, None));
        if r.expanded {
            lines.extend((0..r.tree.len()).map(|j| (i, Some(j))));
        }
    }
    let height = |l: &(usize, Option<usize>)| if l.1.is_some() { TREE_H } else { ROW_H } * s;
    let empty_h = 2.0 * EMPTY_PAD * s + ink.line(OverviewFace::Meta);
    let content_h: f32 = lines.iter().map(height).sum();
    let table_h = if lines.is_empty() {
        empty_h.min(avail.max(empty_h))
    } else {
        content_h.min(avail)
    };
    // Visible lines: as many as fit counted from the end, so the last line is reachable
    // at `max_scroll` and the table never scrolls into blank space.
    let mut visible = 0;
    let mut acc = 0.0;
    for l in lines.iter().rev() {
        acc += height(l);
        if acc > table_h + 0.01 {
            break;
        }
        visible += 1;
    }
    out.lines_total = lines.len();
    out.lines_visible = visible;
    let eff = scroll.min(out.max_scroll());
    let table = [cx, table_y, cw, table_h];
    out.table = table;

    ink.boxed(
        [cx, sec_y, cw, TABLE_HEADER_H * s + table_h],
        14.0 * s,
        Some(th::PANEL),
        th::BORDER,
    );
    let x = ColX::new(cx, cw, cols, s);
    table_header(&mut ink, &x, sec_y);

    if lines.is_empty() {
        let msg = if model.resolving {
            None
        } else if model.counts.all == 0 {
            Some(NO_SESSIONS)
        } else {
            Some(EMPTY_FILTER)
        };
        if let Some(msg) = msg {
            let w = ink.width(msg, OverviewFace::Meta).min(cw);
            ink.text_mid(
                msg,
                cx + (cw - w) / 2.0,
                table_y,
                table_h,
                OverviewFace::Meta,
                th::MUTED,
                false,
                cw,
            );
        }
    } else {
        let mut ly = table_y;
        for l in &lines[eff..] {
            let h = height(l);
            if ly + h > table_y + table_h + 0.01 {
                break;
            }
            let row = &model.rows[l.0];
            match l.1 {
                None => draw_row(&mut ink, row, &x, ly),
                Some(j) => draw_tree(&mut ink, row, j, &x, ly),
            }
            ly += h;
        }
    }

    legend(
        &mut ink,
        cx,
        sec_y + TABLE_HEADER_H * s + table_h + SECTION_GAP * s,
        cw,
    );
    ink.finish(out)
}

/// The header row: title, counts and the filter tabs. Returns its bottom edge.
fn overview_header(ink: &mut Ink, model: &OverviewModel, cx: f32, top: f32, cw: f32) -> f32 {
    use crate::overview_model::Filter;
    let s = ink.s;
    let h = HEADER_H * s;
    let title_w = ink.text_mid(
        "Agent Monitor",
        cx,
        top,
        h,
        OverviewFace::Title,
        th::TEXT,
        true,
        cw,
    );

    let c = model.counts;
    let filters = [
        (Filter::All, format!("All {}", c.all)),
        (Filter::NeedsYou, format!("Needs you {}", c.needs_you)),
        (Filter::Working, format!("Working {}", c.working)),
        (Filter::Idle, format!("Idle {}", c.idle)),
    ];
    let pad = 14.0 * s;
    let widths: Vec<f32> = filters
        .iter()
        .map(|(_, l)| 2.0 * pad + ink.width(l, OverviewFace::Meta))
        .collect();
    let group_w = widths.iter().sum::<f32>() + 4.0 * s * (filters.len() - 1) as f32 + 8.0 * s;

    // Beside the title when it fits, else on a line of its own, else not at all.
    let beside = title_w + 16.0 * s + group_w <= cw;
    let (group, bottom) = if beside {
        (Some([cx + cw - group_w, top]), top + h)
    } else if group_w <= cw {
        (Some([cx, top + h + 8.0 * s]), top + 2.0 * h + 8.0 * s)
    } else {
        (None, top + h)
    };

    let sub_x = cx + title_w + 12.0 * s;
    let sub_right = if beside {
        cx + cw - group_w - 16.0 * s
    } else {
        cx + cw
    };
    let total = model.total_subagents;
    let sub = if model.resolving {
        "resolving…".to_string()
    } else {
        let n = c.all;
        let m = total.running + total.done + total.failed;
        format!(
            "{n} session{} · {m} subagent{}",
            if n == 1 { "" } else { "s" },
            if m == 1 { "" } else { "s" }
        )
    };
    ink.text_mid(
        &sub,
        sub_x,
        top,
        h,
        OverviewFace::Meta,
        th::MUTED,
        false,
        sub_right - sub_x,
    );

    if let Some([gx, gy]) = group {
        ink.boxed([gx, gy, group_w, h], 10.0 * s, Some(th::CARD), th::BORDER);
        let mut bx = gx + 4.0 * s;
        for ((f, label), w) in filters.iter().zip(widths) {
            let rect = [bx, gy + 4.0 * s, w, h - 8.0 * s];
            let on = model.filter == *f;
            if on {
                ink.fill(rect, 7.0 * s, th::FILTER_SELECTED);
            }
            let color = if on { th::BRIGHT } else { th::MUTED };
            ink.text_mid(
                label,
                bx + pad,
                rect[1],
                rect[3],
                OverviewFace::Meta,
                color,
                true,
                w - 2.0 * pad,
            );
            ink.hit(OverviewHit::Filter(*f), rect);
            bx += w + 4.0 * s;
        }
    }
    bottom
}

/// Tile `i` (Needs you, Working, Idle, Subagents) in `rect`.
fn tile(ink: &mut Ink, model: &OverviewModel, i: usize, rect: [f32; 4]) {
    let s = ink.s;
    let [x, y, w, _] = rect;
    let (fill, border) = if i == 0 {
        (th::NEEDS_YOU_FILL, th::NEEDS_YOU_BORDER)
    } else {
        (th::CARD, th::BORDER)
    };
    ink.boxed(rect, 12.0 * s, Some(fill), border);
    let px = x + 18.0 * s;
    let inner = w - 36.0 * s;
    let mline = ink.line(OverviewFace::Meta);
    let label_y = y + 14.0 * s;
    let number_y = label_y + mline + 5.0 * s;
    let dline = ink.line(OverviewFace::Display);
    let caption_y = number_y + dline + 5.0 * s;

    let c = model.counts;
    let (label, label_color, number, number_color, caption, caption_color) = match i {
        0 => (
            "NEEDS YOU",
            th::NEEDS_YOU_LABEL,
            c.needs_you,
            th::NEEDS_YOU_NUMBER,
            "Approval / input waiting",
            th::NEEDS_YOU_CAPTION,
        ),
        1 => (
            "WORKING",
            th::WORKING_LABEL,
            c.working,
            th::TEXT,
            "Sessions running a turn",
            th::MUTED,
        ),
        2 => (
            "IDLE",
            th::IDLE_LABEL,
            c.idle,
            th::TEXT,
            "Turn finished, ready",
            th::MUTED,
        ),
        _ => {
            subagent_tile(ink, model, rect);
            return;
        }
    };
    // The status dot before the label.
    let d = 8.0 * s;
    let dy = label_y + mline / 2.0;
    match i {
        0 => ink.dot(px + d / 2.0, dy, d, th::WAITING),
        1 => ink.dot(px + d / 2.0, dy, d, th::ACCENT),
        _ => ink.ring(px + d / 2.0, dy, d, 1.5 * s, th::IDLE_DOT),
    }
    let lx = px + d + 8.0 * s;
    ink.text(
        label,
        lx,
        label_y,
        OverviewFace::Meta,
        label_color,
        true,
        x + w - 18.0 * s - lx,
    );
    ink.text(
        &number.to_string(),
        px,
        number_y,
        OverviewFace::Display,
        number_color,
        true,
        inner,
    );
    ink.text(
        caption,
        px,
        caption_y,
        OverviewFace::Meta,
        caption_color,
        false,
        inner,
    );
}

/// The Subagents tile: running (Display) and done (failed counted in, D8), and a bar of
/// each part's own fraction (R4.6).
fn subagent_tile(ink: &mut Ink, model: &OverviewModel, rect: [f32; 4]) {
    let s = ink.s;
    let [x, y, w, h] = rect;
    let px = x + 18.0 * s;
    let right = x + w - 18.0 * s;
    let mline = ink.line(OverviewFace::Meta);
    let label_y = y + 14.0 * s;
    ink.text(
        "SUBAGENTS",
        px,
        label_y,
        OverviewFace::Meta,
        th::IDLE_LABEL,
        true,
        right - px,
    );
    let t = model.total_subagents;
    let number_y = label_y + mline + 5.0 * s;
    let dline = ink.line(OverviewFace::Display);
    let base = number_y + dline - 4.0 * s; // a shared baseline-ish bottom for the smaller runs
    let mut pen = px;
    pen += ink.text(
        &t.running.to_string(),
        pen,
        number_y,
        OverviewFace::Display,
        th::WORKING_LABEL,
        true,
        right - pen,
    );
    pen += 8.0 * s;
    pen += ink.text(
        "running",
        pen,
        base - mline,
        OverviewFace::Meta,
        th::MUTED,
        false,
        right - pen,
    );
    pen += 10.0 * s;
    let tline = ink.line(OverviewFace::Title);
    pen += ink.text(
        &(t.done + t.failed).to_string(),
        pen,
        base - tline,
        OverviewFace::Title,
        th::DONE_LIGHT,
        true,
        right - pen,
    );
    pen += 8.0 * s;
    ink.text(
        "done",
        pen,
        base - mline,
        OverviewFace::Meta,
        th::MUTED,
        false,
        right - pen,
    );
    // The bar.
    let bar = [px, y + h - 20.0 * s, right - px, 6.0 * s];
    ink.fill(bar, 3.0 * s, th::BORDER);
    let all = t.running + t.done + t.failed;
    if all > 0 {
        let mut bx = bar[0];
        for (n, color) in [
            (t.running, th::ACCENT),
            (t.done, th::DONE),
            (t.failed, th::FAILED),
        ] {
            let bw = bar[2] * n as f32 / all as f32;
            if bw > 0.0 {
                ink.fill([bx, bar[1], bw, bar[3]], 0.0, color);
            }
            bx += bw;
        }
    }
}

/// `Waiting on you` — text — `Open session →` (D5).
fn banner(ink: &mut Ink, b: &crate::overview_model::Banner, rect: [f32; 4]) {
    let s = ink.s;
    let [x, y, w, h] = rect;
    ink.boxed(rect, 10.0 * s, Some(th::BANNER_BG), th::BANNER_BORDER);
    let face = OverviewFace::Meta;
    let col = ink.col(face);
    let button = "Open session →";
    let bw = 28.0 * s + ink.width(button, face);
    let bh = 32.0 * s;
    let brect = [x + w - 16.0 * s - bw, y + (h - bh) / 2.0, bw, bh];
    ink.fill(brect, 8.0 * s, th::WAITING);
    ink.text_mid(
        button,
        brect[0] + 14.0 * s,
        brect[1],
        bh,
        face,
        th::ON_WAITING,
        true,
        bw - 28.0 * s + 0.5,
    );
    ink.hit(OverviewHit::Banner(b.tab, b.pane), brect);

    let mut pen = x + 16.0 * s;
    let limit = brect[0] - 12.0 * s;
    pen += ink.text_mid(
        "Waiting on you",
        pen,
        y,
        h,
        face,
        th::WAITING_TEXT,
        true,
        limit - pen,
    );
    pen += 12.0 * s;
    // The suffix is never clipped; the now-doing part is ellipsised first, then the head.
    let suffix = if b.more > 0 {
        format!(" (+{} more)", b.more)
    } else {
        String::new()
    };
    let suffix_w = ink.width(&suffix, face);
    let room = limit - pen - suffix_w;
    let full_w = ink.width(&b.text, face);
    let text = if full_w <= room {
        Some(b.text.clone())
    } else if let Some((head, tail)) = b.text.split_once(" — ") {
        let head = format!("{head} — ");
        let head_w = ink.width(&head, face);
        match ellipsize(tail, room - head_w, col) {
            Some(t) if head_w < room => Some(format!("{head}{t}")),
            _ => ellipsize(&b.text, room, col),
        }
    } else {
        ellipsize(&b.text, room, col)
    };
    if let Some(t) = text {
        pen += ink.text_mid(&t, pen, y, h, face, th::TEXT, false, room.max(0.0));
    }
    if !suffix.is_empty() {
        ink.text_mid(&suffix, pen, y, h, face, th::MUTED, false, suffix_w + 0.5);
    }
}

/// The table's column x positions.
struct ColX {
    cx: f32,
    cw: f32,
    c: Columns,
    status_x: f32,
    session_x: f32,
    now_x: f32,
    sub_x: Option<f32>,
    el_x: Option<f32>,
    buttons_x: f32,
}

impl ColX {
    fn new(cx: f32, cw: f32, c: Columns, s: f32) -> ColX {
        let g = COL_GAP * s;
        let status_x = cx + ROW_PAD * s;
        let session_x = status_x + c.status + g;
        let now_x = session_x + c.session + g;
        let mut pen = now_x + c.now_doing + g;
        let sub_x = c.subagents.map(|w| {
            let at = pen;
            pen += w + g;
            at
        });
        let el_x = c.elapsed.map(|_| pen);
        ColX {
            cx,
            cw,
            c,
            status_x,
            session_x,
            now_x,
            sub_x,
            el_x,
            buttons_x: cx + cw - ROW_PAD * s - c.buttons,
        }
    }
}

fn table_header(ink: &mut Ink, x: &ColX, sec_y: f32) {
    let s = ink.s;
    let h = TABLE_HEADER_H * s;
    let face = OverviewFace::Meta;
    let c = th::LABEL;
    ink.text_mid("STATUS", x.status_x, sec_y, h, face, c, true, x.c.status);
    ink.text_mid("SESSION", x.session_x, sec_y, h, face, c, true, x.c.session);
    ink.text_mid("NOW DOING", x.now_x, sec_y, h, face, c, true, x.c.now_doing);
    if let (Some(sx), Some(sw)) = (x.sub_x, x.c.subagents) {
        ink.text_mid("SUBAGENTS", sx, sec_y, h, face, c, true, sw);
    }
    if let (Some(ex), Some(ew)) = (x.el_x, x.c.elapsed) {
        let y = sec_y + (h - ink.line(face)) / 2.0;
        ink.text_right("ELAPSED", ex + ew, y, face, c, true, ew);
    }
    ink.fill([x.cx, sec_y + h - s, x.cw, s], 0.0, th::BORDER);
}

/// One session row at `y` (D3, D4, D7, D8, D10).
fn draw_row(ink: &mut Ink, r: &SessionRowData, x: &ColX, y: f32) {
    let s = ink.s;
    let h = ROW_H * s;
    let meta = OverviewFace::Meta;
    let mline = ink.line(meta);
    let bg = if r.selected {
        Some(th::ROW_SELECTED)
    } else if r.bucket == Bucket::NeedsYou {
        Some(th::ROW_NEEDS_YOU)
    } else {
        None
    };
    if let Some(bg) = bg {
        ink.fill([x.cx + s, y, x.cw - 2.0 * s, h], 0.0, bg);
    }
    if !r.expanded || r.tree.is_empty() {
        ink.fill(
            [x.cx + s, y + h - s, x.cw - 2.0 * s, s],
            0.0,
            th::ROW_SEPARATOR,
        );
    }
    ink.hit(OverviewHit::Row(r.tab, r.pane), [x.cx, y, x.cw, h]);

    // ---- status pill ----
    let label = th::bucket_label(r.bucket);
    let (fg, fill, dot) = th::pill(r.bucket);
    let ph = 24.0 * s;
    let pw = (34.0 * s + ink.width(label, meta)).min(x.c.status);
    let py = y + (h - ph) / 2.0;
    ink.fill([x.status_x, py, pw, ph], ph / 2.0, fill);
    ink.dot(x.status_x + 13.5 * s, py + ph / 2.0, 7.0 * s, dot);
    ink.text_mid(
        label,
        x.status_x + 24.0 * s,
        py,
        ph,
        meta,
        fg,
        true,
        pw - 34.0 * s + 0.5,
    );

    // ---- session: project + agent tag, then the branch line ----
    let title = OverviewFace::Title;
    let tline = ink.line(title);
    let block = tline + 4.0 * s + mline;
    let by = y + (h - block) / 2.0;
    let tag_w = ink.width(&r.agent_label, meta) + 14.0 * s;
    let room = x.c.session - tag_w - 8.0 * s;
    let show_tag = !r.agent_label.is_empty() && room >= 4.0 * ink.col(title);
    let pmax = if show_tag { room } else { x.c.session };
    let pw = ink.text(&r.project, x.session_x, by, title, th::BRIGHT, true, pmax);
    if show_tag {
        let tx = x.session_x + pw + if pw > 0.0 { 8.0 * s } else { 0.0 };
        let tag_h = mline + 4.0 * s;
        let ty = by + (tline - tag_h) / 2.0;
        ink.boxed([tx, ty, tag_w, tag_h], 5.0 * s, None, th::TAG_BORDER);
        ink.text(
            &r.agent_label,
            tx + 7.0 * s,
            ty + 2.0 * s,
            meta,
            th::agent(r.agent),
            false,
            tag_w - 14.0 * s + 0.5,
        );
    }
    let y2 = by + tline + 4.0 * s;
    if r.has_branch {
        let icon = 12.0 * s;
        push_icon(
            &mut ink.shapes,
            Icon::GitBranch,
            x.session_x,
            y2 + (mline - icon) / 2.0,
            icon,
            th::MUTED,
        );
        ink.text(
            &r.branch_line,
            x.session_x + 18.0 * s,
            y2,
            meta,
            th::MUTED,
            false,
            x.c.session - 18.0 * s,
        );
    } else {
        ink.text(
            &r.branch_line,
            x.session_x,
            y2,
            meta,
            th::MUTED,
            false,
            x.c.session,
        );
    }

    // ---- now doing + the prompt ----
    let col = ink.col(meta);
    let prompt = r
        .prompt
        .as_deref()
        .and_then(|p| quoted(p, x.c.now_doing, col));
    let block = if prompt.is_some() {
        2.0 * mline + 4.0 * s
    } else {
        mline
    };
    let ny = y + (h - block) / 2.0;
    ink.text(
        &r.now_doing,
        x.now_x,
        ny,
        meta,
        th::activity(r.bucket),
        false,
        x.c.now_doing,
    );
    if let Some(p) = prompt {
        ink.text(
            &p,
            x.now_x,
            ny + mline + 4.0 * s,
            meta,
            th::PROMPT,
            false,
            x.c.now_doing,
        );
    }

    // ---- subagents ----
    if let (Some(sx), Some(sw)) = (x.sub_x, x.c.subagents) {
        if r.subagents.is_empty() {
            ink.text_mid("—", sx, y, h, meta, th::FAINT, false, sw);
        } else {
            let sq = SQUARE * s;
            let g = SQUARE_GAP * s;
            let per_line = (((sw + g) / (sq + g) + 1e-3).floor() as usize).max(1);
            let slots = square_slots(&r.subagents, 2 * per_line);
            let nlines = slots.len().div_ceil(per_line).max(1);
            let sq_h = nlines as f32 * sq + (nlines - 1) as f32 * g;
            let summary = x.c.subagent_summary && !r.subagent_summary.is_empty();
            let block = sq_h + if summary { 6.0 * s + mline } else { 0.0 };
            let top = y + (h - block) / 2.0;
            for (i, cell) in slots.iter().enumerate() {
                let (li, ci) = (i / per_line, i % per_line);
                let rect = [
                    sx + ci as f32 * (sq + g),
                    top + li as f32 * (sq + g),
                    sq,
                    sq,
                ];
                match *cell {
                    SubagentCell::Square(st) => {
                        let color = th::subagent(st);
                        ink.fill(rect, 3.0 * s, color);
                        ink.squares.push(LaidSquare {
                            cell: *cell,
                            rect,
                            color: th::srgb(color),
                        });
                    }
                    SubagentCell::More(k) => {
                        ink.squares.push(LaidSquare {
                            cell: *cell,
                            rect,
                            color: th::srgb(th::MUTED),
                        });
                        ink.text(
                            &format!("+{k}"),
                            rect[0],
                            rect[1] + (sq - mline) / 2.0,
                            meta,
                            th::MUTED,
                            false,
                            sx + sw - rect[0],
                        );
                    }
                }
            }
            if summary {
                ink.text(
                    &r.subagent_summary,
                    sx,
                    top + sq_h + 6.0 * s,
                    meta,
                    th::IDLE_LABEL,
                    false,
                    sw,
                );
            }
        }
    }

    // ---- elapsed ----
    if let (Some(ex), Some(ew)) = (x.el_x, x.c.elapsed) {
        ink.text_right(
            &r.elapsed,
            ex + ew,
            y + (h - mline) / 2.0,
            meta,
            th::SOFT,
            false,
            ew,
        );
    }

    // ---- the chevron and open buttons (D8, D10) ----
    let b = BUTTON * s;
    let bty = y + (h - b) / 2.0;
    if !r.tree.is_empty() {
        let rect = [x.buttons_x, bty, b, b];
        ink.boxed(rect, 8.0 * s, Some(th::BUTTON), th::BUTTON_BORDER);
        chevron(ink, rect, r.expanded);
        ink.hit(OverviewHit::Expand(r.tab, r.pane), rect);
    }
    let rect = [x.buttons_x + x.c.buttons - b, bty, b, b];
    ink.boxed(rect, 8.0 * s, Some(th::BUTTON), th::BUTTON_BORDER);
    arrow(ink, rect);
    ink.hit(OverviewHit::Open(r.tab, r.pane), rect);
}

/// A 16 px chevron centred in `rect`: down when collapsed, up when expanded.
fn chevron(ink: &mut Ink, rect: [f32; 4], up: bool) {
    let s = ink.s;
    let (mx, my) = (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0);
    let d = if up { -1.0 } else { 1.0 };
    let w = 2.0 * s * 16.0 / 24.0;
    let a = [mx - 4.0 * s, my - 2.0 * s * d];
    let m = [mx, my + 2.0 * s * d];
    let b = [mx + 4.0 * s, my - 2.0 * s * d];
    ink.stroke(a, m, w, th::SOFT);
    ink.stroke(m, b, w, th::SOFT);
}

/// A `→` centred in `rect`.
fn arrow(ink: &mut Ink, rect: [f32; 4]) {
    let s = ink.s;
    let (mx, my) = (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0);
    let w = 2.0 * s * 16.0 / 24.0;
    let tip = [mx + 5.0 * s, my];
    ink.stroke([mx - 5.0 * s, my], tip, w, th::SOFT);
    ink.stroke([mx + 1.0 * s, my - 4.0 * s], tip, w, th::SOFT);
    ink.stroke([mx + 1.0 * s, my + 4.0 * s], tip, w, th::SOFT);
}

/// Tree line `j` of an expanded row (D8).
fn draw_tree(ink: &mut Ink, r: &SessionRowData, j: usize, x: &ColX, y: f32) {
    let s = ink.s;
    let h = TREE_H * s;
    let t = &r.tree[j];
    let meta = OverviewFace::Meta;
    let x0 = x.cx + (ROW_PAD + STATUS_W + COL_GAP) * s;
    let right = x.cx + x.cw - ROW_PAD * s;
    if r.bucket == Bucket::NeedsYou || r.selected {
        let bg = if r.selected {
            th::ROW_SELECTED
        } else {
            th::ROW_NEEDS_YOU
        };
        ink.fill([x.cx + s, y, x.cw - 2.0 * s, h], 0.0, bg);
    }
    ink.hit(OverviewHit::Row(r.tab, r.pane), [x.cx, y, x.cw, h]);
    ink.fill([x0, y, s, h], 0.0, th::TAG_BORDER);
    ink.fill([x0, y + h / 2.0, 18.0 * s, s], 0.0, th::TAG_BORDER);
    let g = 12.0 * s;
    let dot_x = x0 + 18.0 * s + g;
    let color = th::subagent(t.status);
    ink.dot(dot_x + 5.0 * s, y + h / 2.0, 8.0 * s, color);
    let type_x = dot_x + 10.0 * s + g;
    let fixed = (18.0 + 10.0 + 140.0 + 90.0 + 72.0) * s + 5.0 * g;
    let mut type_w = 140.0 * s;
    let mut desc_w = right - x0 - fixed;
    if desc_w < 0.0 {
        type_w = (type_w + desc_w).max(0.0);
        desc_w = 0.0;
    }
    ink.text_mid(
        &t.subagent_type,
        type_x,
        y,
        h,
        meta,
        th::LABEL,
        false,
        type_w,
    );
    let desc_x = type_x + type_w + g;
    let desc_color = if t.status == SubagentStatus::Done {
        th::IDLE_LABEL
    } else {
        th::TEXT
    };
    ink.text_mid(
        &t.description,
        desc_x,
        y,
        h,
        meta,
        desc_color,
        false,
        desc_w,
    );
    let st_x = desc_x + desc_w + g;
    ink.text_mid(&t.status_word, st_x, y, h, meta, color, false, 90.0 * s);
    let mline = ink.line(meta);
    ink.text_right(
        &t.duration,
        right,
        y + (h - mline) / 2.0,
        meta,
        th::MUTED,
        false,
        72.0 * s,
    );
    if j + 1 == r.tree.len() {
        ink.fill(
            [x.cx + s, y + h - s, x.cw - 2.0 * s, s],
            0.0,
            th::ROW_SEPARATOR,
        );
    }
}

/// The colour key under the table.
fn legend(ink: &mut Ink, cx: f32, y: f32, cw: f32) {
    let s = ink.s;
    let h = LEGEND_H * s;
    let meta = OverviewFace::Meta;
    let items: [(Option<u32>, &str); 5] = [
        (Some(th::WAITING), "needs you"),
        (Some(th::ACCENT), "running"),
        (Some(th::DONE), "done"),
        (Some(th::FAILED), "failed"),
        (None, "Sorted: needs you → working → idle"),
    ];
    let mut pen = cx;
    let right = cx + cw;
    for (swatch, label) in items {
        let sw = if swatch.is_some() { 16.0 * s } else { 0.0 };
        let need = sw + ink.width(label, meta);
        if pen + need > right {
            break;
        }
        if let Some(c) = swatch {
            let d = 10.0 * s;
            ink.fill([pen, y + (h - d) / 2.0, d, d], 3.0 * s, c);
        }
        ink.text_mid(
            label,
            pen + sw,
            y,
            h,
            meta,
            th::LABEL,
            false,
            need - sw + 0.5,
        );
        pen += need + 18.0 * s;
    }
}

// ---- the detail --------------------------------------------------------------------------

/// Lay out the session detail (D12, D13, D15). `scroll` is the lanes' whole-line offset.
pub fn layout_detail(
    detail: &DetailData,
    viewport: [f32; 2],
    scale: f64,
    scroll: usize,
) -> OverviewLayout {
    let mut ink = Ink::new(scale);
    let s = ink.s;
    let [vw, vh] = viewport;
    ink.fill([0.0, 0.0, vw, vh], 0.0, th::BG);
    let (cx, cw) = content_x(vw, s);
    let top = DETAIL_TOP_PAD * s;
    let mut out = OverviewLayout {
        viewport,
        scale,
        content: [cx, top, cw, (vh - top - BOTTOM_PAD * s).max(0.0)],
        ..OverviewLayout::default()
    };
    let meta = OverviewFace::Meta;
    let mline = ink.line(meta);

    // ---- breadcrumb ----
    let bh = 32.0 * s;
    let back = "‹ All sessions";
    let back_w = ink.text_mid(back, cx, top, bh, meta, th::LINK, false, cw);
    ink.hit(OverviewHit::Back, [cx, top, back_w, bh]);
    let mut pen = cx + back_w + 8.0 * s;
    pen += ink.text_mid("/", pen, top, bh, meta, th::MUTED, false, cx + cw - pen);
    pen += 8.0 * s;
    ink.text_mid(
        &detail.project,
        pen,
        top,
        bh,
        meta,
        th::TEXT,
        false,
        cx + cw - pen,
    );
    let mut y = top + bh + SECTION_GAP * s;

    if cw / s < W_MIN {
        out.too_narrow = true;
        ink.text(TOO_NARROW, cx, y, meta, th::MUTED, false, cw);
        return ink.finish(out);
    }

    y = detail_header(&mut ink, detail, cx, y, cw) + SECTION_GAP * s;

    if !detail.has_timeline {
        let h = 2.0 * EMPTY_PAD * s + mline;
        let rect = [cx, y, cw, h];
        ink.boxed(rect, 14.0 * s, Some(th::PANEL), th::BORDER);
        let w = ink.width(NO_TIMELINE, meta).min(cw - 40.0 * s);
        ink.text_mid(
            NO_TIMELINE,
            cx + (cw - w) / 2.0,
            y,
            h,
            meta,
            th::MUTED,
            false,
            cw - 40.0 * s,
        );
        return ink.finish(out);
    }

    let side_by_side = cw / s >= TWO_COLUMN_MIN;
    let gap = SECTION_GAP * s;
    let tl_w = if side_by_side {
        cw - SIDE_PANEL_W * s - gap
    } else {
        cw
    };
    let side_w = if side_by_side { SIDE_PANEL_W * s } else { cw };
    let side_h = side_panel_height(&ink, detail);

    let fixed_h = (18.0 + 20.0 + 14.0 + TICK_H + 2.0 + LANE_H + 2.0 + 20.0) * s;
    let bottom = vh - BOTTOM_PAD * s;
    let avail = if side_by_side {
        bottom - y
    } else {
        bottom - y - gap - side_h
    };
    let fit = (((avail - fixed_h) / (LANE_H * s)) + 1e-3).floor().max(0.0) as usize;
    let visible = fit.min(detail.lanes.len());
    out.lines_total = detail.lanes.len();
    out.lines_visible = visible;
    let eff = scroll.min(out.max_scroll());
    let tl = [cx, y, tl_w, fixed_h + visible as f32 * LANE_H * s];
    let sp = if side_by_side {
        [cx + tl_w + gap, y, side_w, side_h]
    } else {
        [cx, y + tl[3] + gap, side_w, side_h]
    };
    out.timeline = Some(tl);
    out.side_panel = Some(sp);
    out.side_by_side = side_by_side;

    timeline(&mut ink, detail, tl, eff, visible);
    side_panel(&mut ink, detail, sp);
    ink.finish(out)
}

/// The header card (D3, D22). Returns its bottom edge.
fn detail_header(ink: &mut Ink, d: &DetailData, cx: f32, y: f32, cw: f32) -> f32 {
    let s = ink.s;
    let meta = OverviewFace::Meta;
    let title = OverviewFace::Title;
    let mline = ink.line(meta);
    let tline = ink.line(title);
    let pad_x = 22.0 * s;
    let pad_y = 20.0 * s;
    let line1 = 24.0 * s;
    let mut h = pad_y + line1 + 8.0 * s + mline + 8.0 * s + mline + pad_y;
    if d.prompt.is_some() {
        h += 4.0 * s + mline;
    }
    let rect = [cx, y, cw, h];
    ink.boxed(rect, 14.0 * s, Some(th::CARD), th::BORDER);

    // Buttons, right-aligned at the top.
    let bh = 40.0 * s;
    let by = y + pad_y;
    let mut bx = cx + cw - pad_x;
    let focus = "Focus terminal";
    let fw = 32.0 * s + ink.width(focus, meta);
    bx -= fw;
    let frect = [bx, by, fw, bh];
    ink.fill(frect, 8.0 * s, th::ACCENT);
    ink.text_mid(
        focus,
        bx + 16.0 * s,
        by,
        bh,
        meta,
        th::ON_ACCENT,
        true,
        fw - 32.0 * s + 0.5,
    );
    ink.hit(OverviewHit::Focus(d.tab, d.pane), frect);
    if d.resume_cmd.is_some() {
        let copy = "Copy resume cmd";
        let w = 32.0 * s + ink.width(copy, meta);
        bx -= 8.0 * s + w;
        let crect = [bx, by, w, bh];
        ink.boxed(crect, 8.0 * s, Some(th::BUTTON), th::TAG_BORDER);
        ink.text_mid(
            copy,
            bx + 16.0 * s,
            by,
            bh,
            meta,
            th::TEXT,
            true,
            w - 32.0 * s + 0.5,
        );
        ink.hit(OverviewHit::CopyResume, crect);
    }
    let lx = cx + pad_x;
    let lright = bx - 16.0 * s;
    let lw = lright - lx;

    // Line 1: project, agent tag, status pill.
    let ly = y + pad_y;
    let tag_w = ink.width(&d.agent_label, meta) + 14.0 * s;
    let label = th::bucket_label(d.bucket);
    let pill_w = 34.0 * s + ink.width(label, meta);
    let mut room = lw;
    let show_pill = room - pill_w - 10.0 * s >= 4.0 * ink.col(title);
    if show_pill {
        room -= pill_w + 10.0 * s;
    }
    let show_tag = !d.agent_label.is_empty() && room - tag_w - 10.0 * s >= 4.0 * ink.col(title);
    if show_tag {
        room -= tag_w + 10.0 * s;
    }
    let mut pen = lx;
    let pw = ink.text(
        &d.project,
        pen,
        ly + (line1 - tline) / 2.0,
        title,
        th::BRIGHT,
        true,
        room,
    );
    pen += pw + 10.0 * s;
    if show_tag {
        let tag_h = mline + 4.0 * s;
        let ty = ly + (line1 - tag_h) / 2.0;
        ink.boxed([pen, ty, tag_w, tag_h], 5.0 * s, None, th::TAG_BORDER);
        ink.text(
            &d.agent_label,
            pen + 7.0 * s,
            ty + 2.0 * s,
            meta,
            th::agent(d.agent),
            false,
            tag_w - 14.0 * s + 0.5,
        );
        pen += tag_w + 10.0 * s;
    }
    if show_pill {
        let (fg, fill, dot) = th::pill(d.bucket);
        ink.fill([pen, ly, pill_w, line1], line1 / 2.0, fill);
        ink.dot(pen + 13.5 * s, ly + line1 / 2.0, 7.0 * s, dot);
        ink.text_mid(
            label,
            pen + 24.0 * s,
            ly,
            line1,
            meta,
            fg,
            true,
            pill_w - 34.0 * s + 0.5,
        );
    }

    // Line 2: branch, elapsed.
    let y2 = ly + line1 + 8.0 * s;
    let mut pen = lx;
    let elapsed = format!("elapsed {}", d.elapsed);
    let el_w = ink.width(&elapsed, meta);
    let icon_w = if d.branch_line.is_empty() {
        0.0
    } else {
        18.0 * s
    };
    let branch_room = (lright - pen - icon_w - 18.0 * s - el_w).max(0.0);
    if !d.branch_line.is_empty() {
        let icon = 12.0 * s;
        push_icon(
            &mut ink.shapes,
            Icon::GitBranch,
            pen,
            y2 + (mline - icon) / 2.0,
            icon,
            th::MUTED,
        );
        pen += icon_w;
        pen += ink.text(&d.branch_line, pen, y2, meta, th::MUTED, false, branch_room);
        pen += 18.0 * s;
    }
    ink.text(&elapsed, pen, y2, meta, th::MUTED, false, lright - pen);

    // Line 3: now doing; line 4: the prompt.
    let y3 = y2 + mline + 8.0 * s;
    ink.text(
        &d.now_doing,
        lx,
        y3,
        meta,
        th::activity(d.bucket),
        false,
        lw,
    );
    if let Some(p) = d
        .prompt
        .as_deref()
        .and_then(|p| quoted(p, lw, ink.col(meta)))
    {
        ink.text(&p, lx, y3 + mline + 4.0 * s, meta, th::PROMPT, false, lw);
    }
    y + h
}

/// The timeline section: legend, ticks, the main lane and the visible subagent lanes.
fn timeline(ink: &mut Ink, d: &DetailData, rect: [f32; 4], eff: usize, visible: usize) {
    let s = ink.s;
    let meta = OverviewFace::Meta;
    let mline = ink.line(meta);
    let [x, y, w, _] = rect;
    ink.boxed(rect, 14.0 * s, Some(th::PANEL), th::BORDER);
    let ix = x + 20.0 * s;
    let iw = w - 40.0 * s;
    let right = ix + iw;

    // Header: `Timeline` and the colour key, right-aligned, dropping keys that do not fit.
    let hy = y + 18.0 * s;
    let hh = 20.0 * s;
    let tw = ink.text_mid("Timeline", ix, hy, hh, meta, th::TEXT, true, iw);
    let keys: [(u32, &str); 4] = [
        (th::ACCENT, "main working"),
        (th::WAITING_ON_SUBAGENT, "main waiting on subagent"),
        (th::DONE, "subagent done"),
        (th::IDLE_SEGMENT, "idle"),
    ];
    let key_w = |ink: &Ink, l: &str| 20.0 * s + ink.width(l, meta);
    let mut n = keys.len();
    while n > 0 {
        let total: f32 =
            keys[..n].iter().map(|(_, l)| key_w(ink, l)).sum::<f32>() + 14.0 * s * (n - 1) as f32;
        if total <= iw - tw - 12.0 * s {
            break;
        }
        n -= 1;
    }
    let total: f32 = keys[..n].iter().map(|(_, l)| key_w(ink, l)).sum::<f32>()
        + 14.0 * s * n.saturating_sub(1) as f32;
    let mut pen = right - total;
    for (c, l) in &keys[..n] {
        ink.fill(
            [pen, hy + (hh - 8.0 * s) / 2.0, 14.0 * s, 8.0 * s],
            2.0 * s,
            *c,
        );
        let lw = key_w(ink, l);
        ink.text_mid(
            l,
            pen + 20.0 * s,
            hy,
            hh,
            meta,
            th::LABEL,
            false,
            lw - 20.0 * s + 0.5,
        );
        pen += lw + 14.0 * s;
    }

    // Ticks.
    let track_x = ix + (LANE_LABEL_W + 16.0) * s;
    let track_w = (right - track_x).max(0.0);
    let ty = hy + hh + 14.0 * s;
    let tick_h = TICK_H * s;
    let mut prev_right = f32::NEG_INFINITY;
    for t in &d.ticks {
        let lw = ink.width(&t.label, meta);
        let at = track_x + t.at.clamp(0.0, 1.0) * track_w;
        let lx = (at - lw / 2.0).clamp(track_x, (right - lw).max(track_x));
        if lx < prev_right + 4.0 * s {
            continue;
        }
        let drawn = ink.text(
            &t.label,
            lx,
            ty + tick_h - mline,
            meta,
            th::PROMPT,
            false,
            right - lx,
        );
        if drawn > 0.0 {
            prev_right = lx + drawn;
        }
    }

    // The main lane.
    let lane_h = LANE_H * s;
    let my = ty + tick_h + 2.0 * s;
    ink.text_mid(
        "main session",
        ix,
        my,
        lane_h,
        meta,
        th::BRIGHT,
        true,
        LANE_LABEL_W * s,
    );
    let bar_h = 18.0 * s;
    let bar_y = my + (lane_h - bar_h) / 2.0;
    ink.fill([track_x, bar_y, track_w, bar_h], 4.0 * s, th::TRACK);
    for seg in &d.main_lane {
        let Some(c) = th::segment(seg.kind) else {
            continue;
        };
        let x0 = track_x + seg.start.clamp(0.0, 1.0) * track_w;
        let x1 = track_x + seg.end.clamp(0.0, 1.0) * track_w;
        if x1 > x0 {
            ink.fill([x0, bar_y, x1 - x0, bar_h], 0.0, c);
        }
    }
    ink.fill([ix, my + lane_h - s, iw, s], 0.0, th::BORDER);

    // Subagent lanes, whole lines from `eff`.
    let ly0 = my + lane_h + 2.0 * s;
    for (k, i) in (eff..eff + visible).enumerate() {
        let Some(lane) = d.lanes.get(i) else {
            break;
        };
        let ly = ly0 + k as f32 * lane_h;
        let selected = lane.selected || d.selected == Some(i);
        if selected {
            ink.fill([ix, ly, iw, lane_h], 6.0 * s, th::ROW_SELECTED);
        }
        ink.hit(OverviewHit::Lane(i), [ix, ly, iw, lane_h]);
        for t in &d.ticks {
            let gx = track_x + t.at.clamp(0.0, 1.0) * track_w;
            ink.fill([gx, ly, s, lane_h], 0.0, th::GRID_LINE);
        }
        let label_color = if selected { th::BRIGHT } else { th::IDLE_LABEL };
        ink.text_mid(
            &lane.label,
            ix,
            ly,
            lane_h,
            meta,
            label_color,
            false,
            LANE_LABEL_W * s,
        );
        let x0 = track_x + lane.start.clamp(0.0, 1.0) * track_w;
        let x1 = (track_x + lane.end.clamp(0.0, 1.0) * track_w).max(x0 + 2.0 * s);
        let color = if selected {
            th::DONE_LIGHT
        } else {
            th::subagent(lane.status)
        };
        let bh = 14.0 * s;
        ink.fill([x0, ly + (lane_h - bh) / 2.0, x1 - x0, bh], 3.0 * s, color);
        let dw = ink.width(&lane.duration, meta);
        let mut dx = x1 + 8.0 * s;
        if dx + dw > right {
            dx = (x0 - 8.0 * s - dw).max(track_x);
        }
        ink.text_mid(
            &lane.duration,
            dx,
            ly,
            lane_h,
            meta,
            th::MUTED,
            false,
            right - dx,
        );
    }
}

/// The selected-subagent card's and the events card's heights, plus the gap between.
fn side_panel_height(ink: &Ink, d: &DetailData) -> f32 {
    let (a, b) = side_card_heights(ink, d);
    a + 16.0 * ink.s + b
}

fn side_card_heights(ink: &Ink, d: &DetailData) -> (f32, f32) {
    let s = ink.s;
    let mline = ink.line(OverviewFace::Meta);
    let tline = ink.line(OverviewFace::Title);
    let pad = 36.0 * s;
    let selected = match &d.panel {
        Some(p) => {
            let report = p.report.len().clamp(1, 4) as f32;
            pad + mline
                + 12.0 * s
                + tline
                + 12.0 * s
                + 4.0 * mline
                + 3.0 * 8.0 * s
                + 12.0 * s
                + 24.0 * s
                + report * mline
                + (report - 1.0) * 4.0 * s
        }
        None => pad + mline + 12.0 * s + tline,
    };
    let n = d.events.len().max(1) as f32;
    let events = pad + mline + 10.0 * s + n * mline + (n - 1.0) * 10.0 * s;
    (selected, events)
}

/// The selected-subagent card and the recent-events card (D13).
fn side_panel(ink: &mut Ink, d: &DetailData, rect: [f32; 4]) {
    let s = ink.s;
    let meta = OverviewFace::Meta;
    let title = OverviewFace::Title;
    let mline = ink.line(meta);
    let tline = ink.line(title);
    let (h1, h2) = side_card_heights(ink, d);
    let [x, y, w, _] = rect;
    let ix = x + 20.0 * s;
    let iw = w - 40.0 * s;

    ink.boxed([x, y, w, h1], 14.0 * s, Some(th::CARD), th::BORDER);
    let mut py = y + 18.0 * s;
    ink.text("SELECTED SUBAGENT", ix, py, meta, th::LABEL, true, iw);
    py += mline + 12.0 * s;
    match &d.panel {
        Some(p) => {
            ink.text(&p.title, ix, py, title, th::BRIGHT, true, iw);
            py += tline + 12.0 * s;
            let vx = ix + 90.0 * s;
            let vw = iw - 90.0 * s;
            for (k, v) in [
                ("Type", p.subagent_type.as_str()),
                ("Status", ""),
                ("Started", p.started.as_str()),
                ("Duration", p.duration.as_str()),
            ] {
                ink.text(k, ix, py, meta, th::LABEL, false, 90.0 * s - 8.0 * s);
                if k == "Status" {
                    let c = th::subagent(p.status);
                    ink.dot(vx + 4.0 * s, py + mline / 2.0, 8.0 * s, c);
                    ink.text(
                        &p.status_word,
                        vx + 14.0 * s,
                        py,
                        meta,
                        c,
                        false,
                        vw - 14.0 * s,
                    );
                } else {
                    ink.text(v, vx, py, meta, th::TEXT, false, vw);
                }
                py += mline + 8.0 * s;
            }
            py += 4.0 * s;
            let n = p.report.len().clamp(1, 4);
            let bh = 24.0 * s + n as f32 * mline + (n - 1) as f32 * 4.0 * s;
            ink.boxed([ix, py, iw, bh], 8.0 * s, Some(th::REPORT_BG), th::BORDER);
            let mut ry = py + 12.0 * s;
            let lines: Vec<&str> = if p.report.is_empty() {
                vec!["—"]
            } else {
                p.report.iter().take(4).map(String::as_str).collect()
            };
            for l in lines {
                ink.text(l, ix + 14.0 * s, ry, meta, th::SOFT, false, iw - 28.0 * s);
                ry += mline + 4.0 * s;
            }
        }
        None => {
            ink.text("—", ix, py, title, th::FAINT, true, iw);
        }
    }

    let ey = y + h1 + 16.0 * s;
    ink.boxed([x, ey, w, h2], 14.0 * s, Some(th::CARD), th::BORDER);
    let mut py = ey + 18.0 * s;
    ink.text("RECENT EVENTS", ix, py, meta, th::LABEL, true, iw);
    py += mline + 10.0 * s;
    if d.events.is_empty() {
        ink.text("—", ix, py, meta, th::FAINT, false, iw);
    }
    for e in &d.events {
        ink.text(&e.time, ix, py, meta, th::PROMPT, false, 52.0 * s);
        let c = match e.tone {
            EventTone::Plain => th::SOFT,
            EventTone::Done => th::DONE_LIGHT,
            EventTone::Failed => th::FAILED,
        };
        ink.text(&e.text, ix + 62.0 * s, py, meta, c, false, iw - 62.0 * s);
        py += mline + 10.0 * s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overview_model::{Counts, Filter};

    #[test]
    fn ellipsize_cuts_by_column_and_keeps_what_fits() {
        assert_eq!(ellipsize("hello", 40.0, 8.0).as_deref(), Some("hello"));
        assert_eq!(ellipsize("hello", 32.0, 8.0).as_deref(), Some("hel…"));
        assert_eq!(ellipsize("hello", 8.0, 8.0), None);
        assert_eq!(ellipsize("h", 8.0, 8.0).as_deref(), Some("h"));
        assert_eq!(ellipsize("", 80.0, 8.0), None);
        assert_eq!(ellipsize("한글abc", 40.0, 8.0).as_deref(), Some("한글…"));
        assert_eq!(ellipsize("abc", -1.0, 8.0), None);
    }

    #[test]
    fn square_slots_cap_with_a_trailing_more() {
        use SubagentCell::*;
        use SubagentStatus::*;
        let cells = vec![Square(Done); 5];
        assert_eq!(square_slots(&cells, 5), cells);
        assert_eq!(
            square_slots(&cells, 4),
            vec![Square(Done), Square(Done), Square(Done), More(2)]
        );
        let with_more = vec![Square(Running), Square(Done), More(10)];
        assert_eq!(square_slots(&with_more, 2), vec![Square(Running), More(11)]);
        assert_eq!(square_slots(&cells, 0), vec![]);
    }

    #[test]
    fn the_thresholds_land_on_d15s_sums() {
        for (w, sub, el, now_min) in [
            (1172.0, Some(220.0), Some(88.0), 280.0),
            (1171.0, Some(220.0), Some(88.0), 160.0),
            (1052.0, Some(220.0), Some(88.0), 160.0),
            (1051.0, Some(88.0), Some(88.0), 160.0),
            (920.0, Some(88.0), Some(88.0), 160.0),
            (919.0, Some(88.0), None, 160.0),
            (816.0, Some(88.0), None, 160.0),
            (815.0, None, None, 160.0),
            (712.0, None, None, 160.0),
        ] {
            let c = columns_logical(w).expect("drawn");
            assert_eq!((c.subagents, c.elapsed), (sub, el), "W = {w}");
            assert!(c.session >= SESSION_MIN - 0.01, "W = {w}");
            assert!(c.now_doing >= now_min - 0.01, "W = {w}");
            let n = 4 + usize::from(sub.is_some()) + usize::from(el.is_some());
            let sum = c.status
                + c.session
                + c.now_doing
                + c.subagents.unwrap_or(0.0)
                + c.elapsed.unwrap_or(0.0)
                + c.buttons
                + COL_GAP * (n - 1) as f32
                + 2.0 * ROW_PAD;
            assert!((sum - w).abs() < 0.01, "W = {w}: columns sum to {sum}");
        }
        assert_eq!(columns_logical(711.0), None);
    }

    #[test]
    fn the_resolving_header_replaces_the_counts() {
        let m = OverviewModel {
            resolving: true,
            counts: Counts {
                all: 3,
                ..Counts::default()
            },
            ..OverviewModel::default()
        };
        let l = layout_overview(&m, [1440.0, 900.0], 1.0, 0);
        assert!(l.texts.iter().any(|t| t.text == "resolving…"));
        assert!(!l.texts.iter().any(|t| t.text.contains("sessions")));
        assert!(!l.texts.iter().any(|t| t.text == EMPTY_FILTER));
    }

    #[test]
    fn filter_tabs_are_hits_with_their_labels() {
        let m = OverviewModel {
            counts: Counts {
                all: 5,
                needs_you: 1,
                working: 2,
                idle: 1,
            },
            filter: Filter::Working,
            ..OverviewModel::default()
        };
        let l = layout_overview(&m, [1600.0, 1000.0], 1.0, 0);
        for (f, want) in [
            (Filter::All, "All 5"),
            (Filter::NeedsYou, "Needs you 1"),
            (Filter::Working, "Working 2"),
            (Filter::Idle, "Idle 1"),
        ] {
            let (_, r) = l
                .hits
                .iter()
                .find(|(h, _)| *h == OverviewHit::Filter(f))
                .expect("hit");
            let t = l
                .texts
                .iter()
                .find(|t| t.text == want)
                .expect("label laid out");
            let c = [t.rect[0] + t.rect[2] / 2.0, t.rect[1] + t.rect[3] / 2.0];
            assert_eq!(l.hit_at(c), Some(OverviewHit::Filter(f)), "{want} in {r:?}");
        }
    }

    #[test]
    fn each_tile_holds_one_display_number_at_scale_2() {
        let m = OverviewModel {
            counts: Counts {
                all: 15,
                needs_you: 3,
                working: 5,
                idle: 7,
            },
            ..OverviewModel::default()
        };
        let l = layout_overview(&m, [2880.0, 1800.0], 2.0, 0);
        assert_eq!(l.tiles.len(), 4);
        let inside = |t: [f32; 4], r: [f32; 4]| {
            t[0] >= r[0] && t[1] >= r[1] && t[0] + t[2] <= r[0] + r[2] && t[1] + t[3] <= r[1] + r[3]
        };
        for (tile, want) in l.tiles.iter().zip(["3", "5", "7", "0"]) {
            let d: Vec<&str> = l
                .texts
                .iter()
                .filter(|t| t.face == OverviewFace::Display && inside(t.rect, *tile))
                .map(|t| t.text.as_str())
                .collect();
            assert_eq!(d, vec![want]);
        }
        assert_eq!(OverviewFace::Display.ppem(2.0), 60.0);
    }

    #[test]
    fn detail_hits_and_copy_only_with_a_resume_command() {
        let mut d = DetailData {
            tab: 4,
            pane: 9,
            project: "p".into(),
            has_timeline: true,
            ..DetailData::default()
        };
        let l = layout_detail(&d, [1440.0, 900.0], 1.0, 0);
        let hits: Vec<OverviewHit> = l.hits.iter().map(|(h, _)| *h).collect();
        assert!(hits.contains(&OverviewHit::Back));
        assert!(hits.contains(&OverviewHit::Focus(4, 9)));
        assert!(!hits.contains(&OverviewHit::CopyResume));
        d.resume_cmd = Some("claude --resume x".into());
        let l = layout_detail(&d, [1440.0, 900.0], 1.0, 0);
        assert!(l.hits.iter().any(|(h, _)| *h == OverviewHit::CopyResume));
    }
}
