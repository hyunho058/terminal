//! Golden-frame tests — the pixels, not the bookkeeping.
//!
//! Every other renderer test asserts a *decision*: `reshape_count` says the
//! instance builders ran, `Atlas::generation` says a glyph was packed, the layout
//! tests say a rect was computed. All of those stay green while the window shows
//! nothing, because none of them ever looks at a pixel. That gap is not
//! hypothetical — zeroing the glyph instance count in `Renderer::draw_to` (a blank
//! terminal: no prompt, no output, no cursor) left the entire workspace suite at
//! 1460 passed / 0 failed.
//!
//! These tests close it by rendering through the real path ([`Renderer::capture`]
//! wraps `draw_to`, it does not re-implement it) and reading the result back. The
//! contract they pin is deliberately font-independent: *which cells carry ink*,
//! not what the ink looks like. A cell holding a printable character must contain
//! pixels that differ from the background; a cell with no text anywhere near it
//! must not. That survives a font substitution or an OS antialiasing change, and
//! still fails the instant the grid stops reaching the screen, the insets mis-place
//! it, or a pipeline is dropped.
//!
//! "Anywhere near" is the whole of the imprecision, and it is deliberate: a glyph
//! may bleed antialiasing into the cell beside it, so the blank cell *next to* text
//! is the one place these tests assert nothing. `renderer::testing::touches_text`
//! is that rule, shared with `crates/app/tests/vertical_slice.rs` and applied by
//! both the geometric test and the golden map below.
//!
//! Update the ink-map snapshot with `cargo insta review` (see `/insta-review`).

use common::config::Config;
use renderer::testing::{holds_text, is_ink, touches_text, CellGeometry};
use renderer::{CapturedFrame, Renderer};
use term_core::{Color, GridSnapshot, Terminal};

/// Frame size. Large enough that the default cell metrics yield a grid worth
/// asserting over, and a multiple of 4 in both axes so nothing depends on how a
/// partial trailing cell is handled.
const FRAME_W: u32 = 1200;
const FRAME_H: u32 = 800;

/// The headless renderer, or `None` when GPU tests are opted out of.
///
/// The threshold, the cell rect, the ink predicate and this constructor all live
/// in `renderer::testing`, shared with `crates/app/tests/vertical_slice.rs`. They
/// were duplicated here, under comments in both files asserting the two copies
/// "have to agree" — and they had already drifted in two ways. See that module.
fn headless(cfg: &Config) -> Option<Renderer> {
    renderer::testing::headless(FRAME_W, FRAME_H, cfg, "the golden frames")
}

/// A human-readable map of the rendered frame. This is what makes a golden frame
/// reviewable — a byte hash would fail just as reliably and tell you nothing about
/// *how*.
///
/// Three symbols, not two:
///
/// - `#` the cell carries ink
/// - `.` the cell carries none
/// - `~` the cell holds no text but touches text, so **either** is correct and the
///   map records neither
///
/// The `~` is the same one-cell tolerance
/// [`ink_lands_exactly_in_the_cells_that_hold_text`] argues for and the same
/// [`touches_text`] predicate, and it is here because a snapshot that pins those
/// cells pins a fact that is not one. This map used to read `#####.#####` for row
/// 0: col 5 is the space in "hello world", and cols 4 and 6 hold `o` and `w`. The
/// sibling test in this very file declares that exact cell untrustworthy — a glyph
/// may bleed antialiasing past its advance width — so a font substitution (Menlo
/// for SF Mono) or an OS antialiasing change would turn correct rendering into a
/// red snapshot. What the map still pins exactly is every cell that holds text and
/// every cell far from any, which is what catches a dropped row, a mis-derived
/// pitch, an off-by-one inset and a smear.
fn ink_map(
    frame: &CapturedFrame,
    geo: &CellGeometry,
    snap: &GridSnapshot,
    cols: u16,
    rows: u16,
) -> String {
    let bg = frame.modal_pixel();
    let mut out = String::new();
    for row in 0..rows.min(geo.rows()) {
        for col in 0..cols.min(geo.cols()) {
            out.push(
                if !holds_text(snap, col, row) && touches_text(snap, col, row) {
                    '~'
                } else if geo.cell_has_ink(frame, bg, col, row) {
                    '#'
                } else {
                    '.'
                },
            );
        }
        out.push('\n');
    }
    out
}

/// A terminal sized to what the renderer actually reports, filled by `feed`.
fn render(r: &mut Renderer, feed: &[u8]) -> (GridSnapshot, CapturedFrame) {
    let cfg = Config::default();
    let (cols, rows) = r.grid_size();
    let mut t = Terminal::new(
        cols,
        rows,
        Color(cfg.theme.foreground),
        Color(cfg.theme.background),
    );
    t.advance(feed);
    let snap = t.snapshot();
    let frame = r.capture(&snap).expect("read the rendered frame back");
    (snap, frame)
}

/// The regression that started this file: the terminal draws no glyphs at all.
///
/// Asserted as a share of the text's own cells rather than "at least one pixel",
/// so a frame that renders a single stray glyph and drops the rest still fails.
#[test]
fn a_frame_of_text_is_not_blank() {
    let cfg = Config::default();
    let Some(mut r) = headless(&cfg) else { return };
    let (_, frame) = render(&mut r, b"hello world");
    let bg = frame.modal_pixel();

    let inked = frame
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|&&px| is_ink(px, bg))
        .count();
    assert!(
        inked > 0,
        "the frame is a solid field of {bg:?} — nothing was drawn"
    );

    // "hello world" is 11 cells; a glyph covers a double-digit number of pixels
    // in each, so a real frame clears this by an order of magnitude. The bound
    // exists to reject a frame that drew a cursor block and nothing else.
    assert!(
        inked > 11 * 4,
        "only {inked} ink pixels for 11 cells of text — the grid barely reached the screen"
    );
}

/// The geometric contract: ink appears in the cells that hold a printable
/// character, and nowhere near the ones that do not.
///
/// Deliberately asymmetric, because the two directions are not equally true.
///
/// **Every printable cell carries ink** is exact, and it is the half that catches
/// a dropped row, an off-by-one inset, a wrong cell size, and a glyph pipeline
/// drawing into the wrong place — none of which move any counter the other tests
/// read.
///
/// **Every blank cell is empty** is *not* exact, and asserting it was a latent
/// false failure. A glyph may put antialiasing past its own advance width into
/// the cell beside it; `crates/app/tests/vertical_slice.rs` records that same
/// measurement from the other side, where the `%` in a real zsh prompt inks the
/// empty cells on both of its sides. That is correct rendering, not a defect.
/// This feed ("hello world", "second line") happens to contain no overhanging
/// glyph, which is the only reason the exact form passed — measured, not assumed:
/// re-run with `hello% w%o%r%l%d %W%M%@ 100%` and the exact form reports three
/// blank cells as regressions. So the reverse direction carries a one-cell
/// tolerance: a blank cell **none of whose neighbours holds text** must be empty.
/// A smear, an uncleared target and a wholesale misplacement all still fail here,
/// because none of them is confined to one cell of slack.
#[test]
fn ink_lands_exactly_in_the_cells_that_hold_text() {
    let cfg = Config::default();
    let Some(mut r) = headless(&cfg) else { return };
    // Row 0 and row 2 carry text; row 1 is deliberately blank, so a renderer that
    // smears rows or mis-derives the row pitch fails here rather than looking fine.
    let (snap, frame) = render(&mut r, b"hello world\r\n\r\nsecond line");
    let geo = CellGeometry::of(&r);
    let bg = frame.modal_pixel();

    let mut wrong = Vec::new();
    for row in 0..geo.rows() {
        for col in 0..geo.cols() {
            let has_ink = geo.cell_has_ink(&frame, bg, col, row);
            if holds_text(&snap, col, row) {
                if !has_ink {
                    wrong.push(format!("({col},{row}) holds text but drew nothing"));
                }
            } else if has_ink && !touches_text(&snap, col, row) {
                wrong.push(format!(
                    "({col},{row}) drew ink, and neither it nor any cell touching it holds text"
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} cells disagree between the grid and the pixels:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// The reviewable golden: the shape of a real frame, as a per-cell ink map.
#[test]
fn the_rendered_frame_matches_its_golden_ink_map() {
    let cfg = Config::default();
    let Some(mut r) = headless(&cfg) else { return };
    let (snap, frame) = render(&mut r, b"hello world\r\n\r\nsecond line\r\n  indented");
    let geo = CellGeometry::of(&r);
    insta::assert_snapshot!(ink_map(&frame, &geo, &snap, 24, 5));
}

/// The background pipeline, and the channel order [`Renderer::capture`]
/// normalises to. A cell with a red background must read red — if the readback
/// (or the shader) transposed R and B this passes as blue, so the assertion is
/// on the ordering, not merely on "some color".
#[test]
fn a_red_background_cell_paints_red_pixels() {
    let cfg = Config::default();
    let Some(mut r) = headless(&cfg) else { return };
    // SGR 41 (red bg) on a run of spaces: background quads only, no glyph ink to
    // confuse the sample.
    let (_, frame) = render(&mut r, b"\x1b[41m    \x1b[0m");
    let geo = CellGeometry::of(&r);

    let (x0, y0, x1, y1) = geo.cell_rect(1, 0, &frame);
    let mid = frame.pixel((x0 + x1) / 2, (y0 + y1) / 2);
    // Widened to i32 before the comparison. `mid[1] + 32` is `u8` arithmetic and
    // panics with "attempt to add with overflow" as soon as green or blue exceeds
    // 223 — which is precisely a light or white readback, one of the regressions
    // this test exists to name. An overflow panic there would replace the
    // diagnostic below with a message about the test's own arithmetic.
    let [r_ch, g_ch, b_ch] = [mid[0] as i32, mid[1] as i32, mid[2] as i32];
    assert!(
        r_ch > g_ch + 32 && r_ch > b_ch + 32,
        "cell (1,0) has a red background but read back as {mid:?} (RGBA)"
    );
}

/// The control. Without it the two tests above could both pass on a frame where
/// *everything* reads as ink — a background that never got cleared, say — since
/// "differs from the modal pixel" would then be true almost everywhere.
#[test]
fn an_empty_grid_paints_no_ink_at_all() {
    let cfg = Config::default();
    let Some(mut r) = headless(&cfg) else { return };
    let (snap, frame) = render(&mut r, b"");
    let geo = CellGeometry::of(&r);
    let bg = frame.modal_pixel();

    let inked = frame
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|&&px| is_ink(px, bg))
        .count();
    assert_eq!(
        inked, 0,
        "an empty grid painted {inked} non-background pixels; \
         the ink metric is measuring something other than drawn content"
    );
    assert_eq!(
        ink_map(&frame, &geo, &snap, 8, 3),
        "........\n........\n........\n",
        "an empty grid must map to no ink"
    );
}

// ---- T7: bold reaches the screen, and is heavier when it gets there ---------

/// Ink *quantity* in one cell: how many of its pixels differ from the background.
///
/// Deliberately a different measure from [`ink_map`]'s. That map answers "does
/// this cell carry ink", which is exactly the question that stays green while
/// every bold glyph on screen is a faint ghost — see this file's header for why
/// it is written that way, and `specs/real-bold-faces` for what it cannot see.
/// Bold is a claim about *how much* ink, so it needs counting, not a predicate.
fn cell_ink(frame: &CapturedFrame, geo: &CellGeometry, col: u16, row: u16) -> u32 {
    let bg = frame.modal_pixel();
    let (x0, y0, x1, y1) = geo.cell_rect(col, row, frame);
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            if is_ink(frame.pixel(x, y), bg) {
                n += 1;
            }
        }
    }
    n
}

/// Render one line and report the ink in each of its first `n` cells.
fn row_ink(r: &mut Renderer, feed: &[u8], n: u16) -> Vec<u32> {
    let (_, frame) = render(r, feed);
    let geo = CellGeometry::of(r);
    (0..n.min(geo.cols()))
        .map(|col| cell_ink(&frame, &geo, col, 0))
        .collect()
}

/// R8.2 — the half of the contract `bold_faces.rs` cannot reach.
///
/// That file proves the *glyphs* are right by rasterizing them directly. This one
/// proves the right glyph reached the **framebuffer**, through `Renderer::capture`
/// and therefore through shaping, run splitting, the atlas and the draw call. The
/// two together are what CLAUDE.md means by covering a render-path change: a
/// decision-level assertion stays correct while the frame it describes is blank.
#[test]
fn bold_cells_carry_more_ink_than_regular_ones() {
    let cfg = Config::default();
    let Some(mut r) = headless(&cfg) else { return };

    // Same characters, same columns, differing only by SGR bold.
    let plain = row_ink(&mut r, "한글 ABC".as_bytes(), 7);
    let bold = row_ink(&mut r, "\x1b[1m한글 ABC".as_bytes(), 7);

    assert_eq!(plain.len(), bold.len());
    let compared: Vec<(u32, u32)> = plain
        .iter()
        .copied()
        .zip(bold.iter().copied())
        .filter(|&(p, _)| p > 0) // skip the space and any empty trailing cell
        .collect();
    assert!(
        compared.len() >= 4,
        "expected several inked cells to compare, got {compared:?}"
    );
    for (p, b) in &compared {
        assert!(
            b >= p,
            "a bold cell must never carry LESS ink than the same cell regular \
             (regular {p}, bold {b}) — that is the 593-syllable erosion, on screen: \
             {plain:?} vs {bold:?}"
        );
    }
    let total_plain: u32 = compared.iter().map(|&(p, _)| p).sum();
    let total_bold: u32 = compared.iter().map(|&(_, b)| b).sum();
    assert!(
        total_bold > total_plain,
        "and bold must be visibly heavier overall: {total_plain} vs {total_bold}"
    );
}

/// The erosion, stated as the screen would show it (R0.2 at the pixel level).
///
/// `안` is one of the 593 syllables `swash`'s `embolden` thins instead of
/// thickening — it loses 89% of its ink under synthesis. If bold ever routes back
/// to the synthetic path, this cell goes faint and this test says so.
#[test]
fn a_bold_hangul_syllable_that_synthesis_would_erode_stays_solid() {
    let cfg = Config::default();
    let Some(mut r) = headless(&cfg) else { return };
    let plain = row_ink(&mut r, "안간건난".as_bytes(), 8);
    let bold = row_ink(&mut r, "\x1b[1m안간건난".as_bytes(), 8);
    for (i, (p, b)) in plain.iter().zip(bold.iter()).enumerate() {
        if *p == 0 {
            continue;
        }
        assert!(
            b > p,
            "cell {i}: bold ink {b} vs regular {p} — this syllable is one of the \
             ones synthesis erodes, so equal-or-less means the real bold face is \
             not being used"
        );
    }
}

// ---------------------------------------------------------------------------
// The sidebar's weather cards (sidebar-weather-cards T8)
// ---------------------------------------------------------------------------
//
// Every other card test reads `build_card`'s output or `compose_count` — decisions
// again. These read the frame: a card whose fill never reached the screen, a glow
// dropped by the draw order, or a `≡` drawn on every card all leave those green.

use renderer::sidebar_card::CardStatus;
use renderer::sidebar_icons::PixelIcon;
use renderer::sidebar_row::{MetaSource, RowMeta, SidebarRow, TabBar, TabBarModel};
use renderer::sidebar_shapes::sample_gradient;
use renderer::sidebar_text::CardFace;
use renderer::sidebar_theme::{AgentSkin, CardMetrics, SkinFill};
use std::rc::Rc;

/// One card of a sidebar frame: skin, title, status.
type Card = (AgentSkin, &'static str, CardStatus);

/// The four cards R0.1 names, in row order.
const CARDS: [Card; 4] = [
    (AgentSkin::Claude, "claude", CardStatus::Working),
    (
        AgentSkin::Antigravity,
        "gravity",
        CardStatus::WaitingForInput,
    ),
    (AgentSkin::Codex, "codex", CardStatus::Ready),
    (AgentSkin::Generic, "zsh", CardStatus::Shell),
];
const ACTIVE: usize = 0;
const HOVERED: usize = 1;

/// Per-channel tolerance for "is this colour", in 8-bit sRGB (R16.1).
const TOL: f32 = 6.0;

/// The metadata of a fixed card table: each row's skin and status, by row index.
struct Cards(&'static [Card]);
impl MetaSource for Cards {
    fn generation(&self) -> u64 {
        1
    }
    fn fill(&self, rows: &[SidebarRow], out: &mut Vec<RowMeta>) {
        out.clear();
        out.extend(rows.iter().enumerate().map(|(i, r)| RowMeta {
            skin: self.0[i].0,
            status: self.0[i].2,
            line1: r.title.clone(),
            cwd_display: "~/work".into(),
            ..RowMeta::default()
        }));
    }
}

/// A renderer with the sidebar layer installed and the four cards in its model — `None`
/// when GPU tests are opted out of (R16.6). The card rects exist only once a frame has
/// been drawn; read them afterwards with [`card_rects`].
fn sidebar_renderer(scale: f64) -> Option<Renderer> {
    sidebar_renderer_of(scale, &CARDS, Some(HOVERED))
}

/// [`sidebar_renderer`] over any card table, card [`ACTIVE`] selected and `hovered`
/// under the pointer — the same skip-or-fail policy, through `testing::headless_scaled`.
fn sidebar_renderer_of(
    scale: f64,
    cards: &'static [Card],
    hovered: Option<usize>,
) -> Option<Renderer> {
    let (w, h) = sidebar_frame(scale);
    let mut r = renderer::testing::headless_scaled(
        w,
        h,
        scale,
        &Config::default(),
        "the sidebar golden frames",
    )?;
    let (bar, handle) = TabBar::new();
    handle.set(TabBarModel {
        titles: cards
            .iter()
            .enumerate()
            .map(|(i, &(_, title, _))| SidebarRow {
                tab_id: i as u64 + 1,
                title: title.into(),
                ..SidebarRow::default()
            })
            .collect(),
        active: ACTIVE,
        ..TabBarModel::default()
    });
    handle.set_meta_source(Rc::new(Cards(cards)));
    handle.set_hovered(hovered);
    r.add_layer(Box::new(bar));
    Some(r)
}

/// The frame a sidebar capture renders into: the shared one at 1×, and at 2× one tall
/// enough for four 208px cards and their gaps.
fn sidebar_frame(scale: f64) -> (u32, u32) {
    if scale > 1.0 {
        (1400, 1000)
    } else {
        (FRAME_W, FRAME_H)
    }
}

fn card_rects() -> Vec<[f32; 4]> {
    card_rects_of(CARDS.len())
}

/// The first `n` card rects of the layout the last frame published.
fn card_rects_of(n: usize) -> Vec<[f32; 4]> {
    let lay = renderer::sidebar_row::TabModelHandle::shared()
        .layout()
        .expect("the sidebar published its layout");
    (0..n)
        .map(|i| lay.card_rect(i).expect("every card is on screen"))
        .collect()
}

/// The fill a card in `i`'s state paints: the brighter one when hovered or selected.
fn fill_of(i: usize) -> SkinFill {
    let skin = CARDS[i].0.card();
    if i == ACTIVE || i == HOVERED {
        skin.hover
    } else {
        skin.default
    }
}

/// Linear → 8-bit sRGB, as the `Bgra8UnormSrgb` target stores it.
fn srgb8(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    s * 255.0
}

/// The body gradient at pixel `(px, py)` of a card at `rect` — the shader's own
/// projection (`sidebar_shapes.wgsl`), evaluated at the pixel's centre.
fn gradient_at(fill: &SkinFill, rect: [f32; 4], px: u32, py: u32) -> [f32; 3] {
    let rel = [px as f32 + 0.5 - rect[0], py as f32 + 0.5 - rect[1]];
    let ext = fill.dir[0].abs() * rect[2] + fill.dir[1].abs() * rect[3];
    let t = (rel[0] * fill.dir[0] + rel[1] * fill.dir[1]) / ext.max(1e-6);
    let c = sample_gradient(fill.stops, fill.mid, t);
    [srgb8(c[0]), srgb8(c[1]), srgb8(c[2])]
}

fn within(px: [u8; 4], want: [f32; 3], tol: f32) -> bool {
    (0..3).all(|k| (f32::from(px[k]) - want[k]).abs() <= tol)
}

fn differs(a: [u8; 4], b: [u8; 4]) -> bool {
    (0..3).any(|k| (i32::from(a[k]) - i32::from(b[k])).abs() > TOL as i32)
}

fn centre(rect: [f32; 4]) -> (u32, u32) {
    (
        (rect[0] + rect[2] * 0.5) as u32,
        (rect[1] + rect[3] * 0.5) as u32,
    )
}

/// Every pixel assertion R0.1 and R16.1–R16.5 make, over one frame captured at `scale`.
fn assert_cards(frame: &CapturedFrame, rects: &[[f32; 4]], scale: f64) {
    let m = CardMetrics::DEFAULT.scaled(scale);
    // The panel, clear of every card and of the glow's 3-logical ring: left of the
    // 14-logical inset at either scale.
    let panel = |y: u32| frame.pixel(4, y);

    // R0.1 / R16.1 — each body is its own skin's gradient, and no two are alike.
    let bodies: Vec<[u8; 4]> = rects
        .iter()
        .enumerate()
        .map(|(i, &rect)| {
            let (cx, cy) = centre(rect);
            let got = frame.pixel(cx, cy);
            let want = gradient_at(&fill_of(i), rect, cx, cy);
            assert!(
                within(got, want, TOL),
                "card {i} ({:?}) body {got:?} is not its skin's gradient {want:?}",
                CARDS[i].0
            );
            assert!(differs(got, panel(cy)), "card {i} body is the panel colour");
            got
        })
        .collect();
    for a in 0..bodies.len() {
        for b in a + 1..bodies.len() {
            assert!(
                differs(bodies[a], bodies[b]),
                "cards {a} and {b} read back alike: {:?} / {:?}",
                bodies[a],
                bodies[b]
            );
        }
    }

    for (i, &rect) in rects.iter().enumerate() {
        let (_, cy) = centre(rect);

        // R16.2 — 2px outside the right edge at mid-height: only the selected card glows.
        let gx = (rect[0] + rect[2] + 2.0) as u32;
        let glows = differs(frame.pixel(gx, cy), panel(cy));
        assert_eq!(glows, i == ACTIVE, "card {i}: glow drawn = {glows}");

        // R16.3 — the `≡` button's centre: only the hovered card's backplate shows.
        let b = renderer::sidebar_card::menu_button(rect, scale);
        let (bx, by) = ((b[0] + b[2] * 0.5) as u32, (b[1] + b[3] * 0.5) as u32);
        let plain = within(
            frame.pixel(bx, by),
            gradient_at(&fill_of(i), rect, bx, by),
            TOL,
        );
        assert_eq!(
            !plain,
            i == HOVERED,
            "card {i}: the button area reads {:?}",
            frame.pixel(bx, by)
        );

        // R16.4 — the title band carries ink over the body.
        let left = rect[0] + m.border + m.pad_x;
        let top = rect[1] + m.border + m.pad_y;
        let title_w = CARDS[i].1.chars().count() as f32 * CardFace::Title.column_px(scale);
        let mut ink = 0;
        for y in top as u32..(top + CardFace::Title.line_px(scale)) as u32 {
            for x in left as u32..(left + title_w) as u32 {
                let body = gradient_at(&fill_of(i), rect, x, y);
                if !within(frame.pixel(x, y), body, 40.0) {
                    ink += 1;
                }
            }
        }
        assert!(ink >= 20, "card {i}: the title band has {ink} ink pixels");
    }

    // R16.5 / D17 — each agent icon reaches the screen: one cell of each, sampled at its
    // centre, carries its table colour. The crab's cell (0,4) is R16.5's; the others are
    // cells no later cell paints over (the Codex and terminal tables overlap).
    // Keyed on the skin, not on `PixelIcon::for_skin`: the mapping is part of what is
    // under test, and asking it which icon to expect would agree with any wrong answer.
    // Each cell's colour appears at that position in no other icon.
    for (i, &rect) in rects.iter().enumerate() {
        let (icon, (col, row), rgb) = match CARDS[i].0 {
            AgentSkin::Claude => (PixelIcon::Claude, (0.0, 4.0), [230.0, 113.0, 78.0]), // #e6714e
            AgentSkin::Antigravity => (PixelIcon::Antigravity, (5.0, 0.0), [255.0, 140.0, 0.0]), // #ff8c00
            AgentSkin::Codex => (PixelIcon::Codex, (2.0, 1.0), [244.0, 239.0, 230.0]), // #f4efe6
            AgentSkin::Generic => (PixelIcon::Terminal, (2.0, 5.0), [21.0, 19.0, 15.0]), // #15130f
        };
        let c = icon.cell_px(scale);
        let [iw, _] = icon.extent(scale);
        let top = if icon == PixelIcon::Claude {
            m.icon_top_claude
        } else {
            m.icon_top_other
        };
        let ox = (rect[0] + rect[2] - m.border - m.pad_x - iw).round();
        let oy = (rect[1] + m.border + m.pad_y + top).round();
        let (px, py) = ((ox + (col + 0.5) * c) as u32, (oy + (row + 0.5) * c) as u32);
        let got = frame.pixel(px, py);
        assert!(
            within(got, rgb, TOL),
            "card {i}: {icon:?}'s cell ({col},{row}) at ({px},{py}) reads {got:?}, not {rgb:?}"
        );
    }
}

/// R0.1 and R16.1–R16.5 at `scale`; R16.6 is the `?` — it skips only through
/// `testing::headless_scaled`.
fn four_skinned_cards_at(scale: f64) {
    let Some(mut r) = sidebar_renderer(scale) else {
        return;
    };
    let (_, frame) = render(&mut r, b"");
    let rects = card_rects();
    assert_eq!(rects.len(), 4);
    for rect in &rects {
        assert!(
            rect[0] >= 0.0 && rect[1] >= 0.0 && rect[1] + rect[3] <= frame.height() as f32,
            "card {rect:?} is inside the frame"
        );
        assert_eq!(rect[3], 104.0 * scale as f32, "R1.1 at {scale}×");
    }
    assert_cards(&frame, &rects, scale);
}

#[test]
fn four_skinned_cards_reach_the_screen() {
    four_skinned_cards_at(1.0);
}

#[test]
fn four_skinned_cards_reach_a_retina_screen() {
    // The Requirements' default scale: 2×, where the crab is drawn at 85×50 and every
    // metric is doubled.
    four_skinned_cards_at(2.0);
}

#[test]
fn a_cached_sidebar_frame_is_the_same_frame() {
    // R17.3 — the second capture re-submits the resident instances (D20). If the cache
    // drew nothing, or drew a pass short, the pixels would differ; the full assertion
    // set runs on it as well, so a second frame that is identically blank also fails.
    // That the second capture takes the cache path at all is asserted where it can be
    // observed, `sidebar_row`'s `a_steady_frame_composes_nothing_and_writes_no_buffer`
    // (`compose_count` is test-only, invisible from here).
    let Some(mut r) = sidebar_renderer(2.0) else {
        return;
    };
    let (_, first) = render(&mut r, b"");
    let (_, second) = render(&mut r, b"");
    assert!(
        first.pixels() == second.pixels(),
        "the cached frame differs from the composed one"
    );
    assert_cards(&second, &card_rects(), 2.0);
}

// ---------------------------------------------------------------------------
// The no-status card (agent-card-status T1)
// ---------------------------------------------------------------------------
//
// `build_card`'s unit test (R5.4) proves a `Hidden` card *composes* no status. This
// proves none reaches the screen: a status drawn by some other path, or a `Hidden`
// that fell through to another status's look, leaves every decision-level test green.

/// R5.5's frame: one `Working` card and one with no status. The second is Codex's — an
/// agent agent-card-status D3 always hides — so the two also differ in skin, as they
/// would on a real sidebar.
const STATUS_CARDS: [Card; 2] = [
    (AgentSkin::Claude, "claude", CardStatus::Working),
    (AgentSkin::Codex, "codex", CardStatus::Hidden),
];

/// What each [`STATUS_CARDS`] row's status-icon box must hold: that icon's colour in
/// 8-bit sRGB, or nothing but the card's body. Written out rather than read from
/// `SidebarTheme`, whose `status_working_fg` is what this checks — a colour taken from
/// the code under test would agree with any wrong answer (the mock's `#ffd9bd`).
const STATUS_INK: [Option<[f32; 3]>; 2] = [Some([255.0, 217.0, 189.0]), None];

/// R5.5 at `scale`.
fn a_no_status_card_carries_no_status_ink_at(scale: f64) {
    // Nothing hovered, so the no-status card is in its default state and its body is
    // the default gradient everywhere the box reaches.
    let Some(mut r) = sidebar_renderer_of(scale, &STATUS_CARDS, None) else {
        return;
    };
    let (_, frame) = render(&mut r, b"");
    let rects = card_rects_of(STATUS_CARDS.len());
    let m = CardMetrics::DEFAULT.scaled(scale);
    let meta_line = CardFace::Meta.line_px(scale);

    for (i, &rect) in rects.iter().enumerate() {
        // The status icon's box — the padding box's bottom-left, the 13-logical
        // `Working` icon centred on the meta line (D21) — widened by a pixel each way
        // for its antialiasing, and taken identically in both cards.
        let size = m.zone_icon_lg;
        let x0 = rect[0] + m.border + m.pad_x;
        let y0 = rect[1] + rect[3] - m.border - m.pad_y - meta_line + (meta_line - size) * 0.5;
        let xs = (x0 as u32 - 1)..((x0 + size).ceil() as u32 + 1);
        let ys = (y0 as u32 - 1)..((y0 + size).ceil() as u32 + 1);
        let pixels: Vec<(u32, u32)> = ys.flat_map(|y| xs.clone().map(move |x| (x, y))).collect();

        match STATUS_INK[i] {
            Some(rgb) => {
                let hits = pixels
                    .iter()
                    .filter(|&&(x, y)| within(frame.pixel(x, y), rgb, TOL))
                    .count();
                assert!(
                    hits >= 4,
                    "{scale}× card {i} ({:?}): {hits} pixels of its status box read {rgb:?}",
                    STATUS_CARDS[i].2
                );
            }
            None => {
                let body = STATUS_CARDS[i].0.card().default;
                let ink: Vec<_> = pixels
                    .iter()
                    .filter(|&&(x, y)| {
                        !within(frame.pixel(x, y), gradient_at(&body, rect, x, y), TOL)
                    })
                    .map(|&(x, y)| ((x, y), frame.pixel(x, y)))
                    .collect();
                assert!(
                    ink.is_empty(),
                    "{scale}× card {i} ({:?}): {} of its status box's {} pixels carry ink, \
                     first {:?}",
                    STATUS_CARDS[i].2,
                    ink.len(),
                    pixels.len(),
                    &ink[..ink.len().min(6)]
                );
            }
        }
    }
}

#[test]
fn a_no_status_card_carries_no_status_ink() {
    a_no_status_card_carries_no_status_ink_at(1.0);
}

#[test]
fn a_no_status_card_carries_no_status_ink_on_a_retina_screen() {
    a_no_status_card_carries_no_status_ink_at(2.0);
}

// ---------------------------------------------------------------------------
// The selection tint follows its text (selection-scroll-anchor T5, D8)
// ---------------------------------------------------------------------------
//
// `selection_layer.rs`'s unit tests map a hand-built snapshot to rects, so they
// cannot see where `GridSnapshot::from_grid` *put* the selection: a projection that
// left the highlight on the old screen row, or dropped it while scrolled back, keeps
// every one of them green. These read the frame instead, through `Renderer::capture`
// with the real `SelectionLayer` registered, after the content under the selection
// has moved.

use renderer::selection_layer::SelectionLayer;
use std::collections::HashMap;
use std::ops::RangeInclusive;

/// The columns of the selected word. Every row of the fixture reads
/// `NN alpha alpha`, so this is the first `alpha`.
const SELECTED: RangeInclusive<u16> = 3..=7;

/// The first column of the row's second `alpha` — an unselected cell holding the
/// **same character** as the first selected cell, on every row of the fixture.
const TWIN: u16 = 9;

/// Line `i` of the fixture: a two-digit label, then the same word twice.
///
/// The label makes each row's text distinct, so the fixture can check that the
/// text it selected really moved (a precondition, not the assertion under test);
/// the word twice gives every row an unselected reference identical to the
/// selected one.
fn selection_line(i: u16) -> String {
    format!("{:02} alpha alpha", i % 100)
}

/// The dominant colour of one cell: its most common pixel value.
///
/// This is how the tests below measure the **tint and not the glyph**. A cell's
/// background — tinted or not — is one exact value over every pixel the glyph
/// leaves alone, while the glyph's own pixels are antialiased across many distinct
/// values and cover a minority of the cell. So the mode is the cell's background,
/// whatever the font: a substitution (Menlo for SF Mono), a different weight or an
/// OS antialiasing change moves the glyph's pixels, not the mode. Comparing whole
/// cells pixel for pixel would not have that property, and sampling one fixed pixel
/// (a corner) bets on where some font's descender or overhang is not.
fn cell_mode(frame: &CapturedFrame, geo: &CellGeometry, col: u16, row: u16) -> [u8; 4] {
    let (x0, y0, x1, y1) = geo.cell_rect(col, row, frame);
    let mut counts: HashMap<[u8; 4], u32> = HashMap::new();
    for y in y0..y1 {
        for x in x0..x1 {
            *counts.entry(frame.pixel(x, y)).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|&(_, n)| n)
        .map(|(px, _)| px)
        .expect("a cell inside the frame has pixels")
}

/// Every grid cell whose dominant colour differs from an unselected cell **of the
/// same row** — the row's [`TWIN`] — by more than the ink threshold.
///
/// Every row is compared against its own reference, as D8 asks, so nothing here
/// assumes what colour the tint or the theme is. The reference could itself be lit
/// by a defect that tints whole rows, which would make that row read as untinted
/// against itself; so each reference is also required to read as the frame's
/// background, and a row whose reference does not is a failure, never a pass.
///
/// The whole grid is scanned, not just the two rows the requirement names: a tint
/// that landed on the right row *and* on another one is as wrong as a missing one.
fn tinted_cells(frame: &CapturedFrame, geo: &CellGeometry, moment: &str) -> Vec<(u16, u16)> {
    let bg = frame.modal_pixel();
    let mut tinted = Vec::new();
    for row in 0..geo.rows() {
        let reference = cell_mode(frame, geo, TWIN, row);
        assert!(
            !is_ink(reference, bg),
            "{moment}: the unselected reference cell ({TWIN},{row}) reads {reference:?}, \
             not the background {bg:?} — row {row} is lit where nothing is selected"
        );
        for col in 0..geo.cols() {
            if is_ink(cell_mode(frame, geo, col, row), reference) {
                tinted.push((col, row));
            }
        }
    }
    tinted
}

/// The tint covers exactly the selected word on `row`, and no cell anywhere else.
///
/// `moved_from` is the row the text left, named in the failure because "the tint
/// stayed where the text was" is the defect this feature exists to fix. The count
/// is asserted on its own first so that a frame where nothing at all was compared
/// — no tint found anywhere — reads as a failure that says so, not as a pass.
fn assert_tint_on(found: &[(u16, u16)], row: u16, moved_from: Option<u16>, moment: &str) {
    let width = SELECTED.count();
    let on_row: Vec<u16> = found
        .iter()
        .filter(|&&(_, r)| r == row)
        .map(|&(c, _)| c)
        .collect();
    let elsewhere: Vec<(u16, u16)> = found.iter().copied().filter(|&(_, r)| r != row).collect();
    let mut stray_rows: Vec<u16> = elsewhere.iter().map(|&(_, r)| r).collect();
    stray_rows.dedup();

    if let Some(from) = moved_from {
        let stale: Vec<u16> = elsewhere
            .iter()
            .filter(|&&(_, r)| r == from)
            .map(|&(c, _)| c)
            .collect();
        assert!(
            stale.is_empty(),
            "{moment}: row {from}, where the selected text WAS, still carries the tint at \
             cols {stale:?}; the text is now on row {row}, which has it at cols {on_row:?}"
        );
    }
    assert_eq!(
        on_row.len(),
        width,
        "{moment}: {} tinted cells on row {row}, where the selected text is; the selection \
         is {width} cells wide. Cols tinted there: {on_row:?}; tint elsewhere on rows \
         {stray_rows:?}",
        on_row.len()
    );
    assert_eq!(
        on_row,
        SELECTED.collect::<Vec<_>>(),
        "{moment}: the tint on row {row} is on the wrong columns"
    );
    assert!(
        elsewhere.is_empty(),
        "{moment}: the tint also reached rows {stray_rows:?}, where nothing is selected: \
         {elsewhere:?}"
    );
}

/// A renderer with [`SelectionLayer`] registered, and a terminal at its grid size
/// whose every row is full, with `alpha` on viewport row 2 selected through the
/// content API exactly as the app's drag does (`point_at` at the moment the cell is
/// learned, then `set_selection`). `None` only when GPU tests are opted out of.
///
/// The spec's R8 names a 10×4 terminal. This uses the renderer's own grid instead
/// (107×47 at this file's frame size), as every other test here does, because the
/// terminal must match what the renderer draws. D8 asks only for `rows ≥ 4`, asserted
/// below, and the assertions scan every row, so the larger grid covers more.
fn selected_on_row_2() -> Option<(Renderer, Terminal)> {
    let cfg = Config::default();
    let mut r = headless(&cfg)?;
    r.add_layer(Box::new(SelectionLayer::new()));
    let (cols, rows) = r.grid_size();
    // D8's "rows ≥ 4": row 2 must exist, and so must a row below it to scroll up.
    assert!(
        rows >= 4 && cols > TWIN + 5,
        "the {FRAME_W}x{FRAME_H} frame yields a {cols}x{rows} grid, too small for the fixture"
    );
    let mut t = Terminal::new(
        cols,
        rows,
        Color(cfg.theme.foreground),
        Color(cfg.theme.background),
    );
    // `rows` lines and `rows - 1` line breaks: every row is filled and nothing has
    // scrolled yet, so the next line break moves the whole screen up by one.
    let screen: Vec<String> = (0..rows).map(selection_line).collect();
    t.advance(screen.join("\r\n").as_bytes());
    assert_eq!(t.history_len(), 0, "the fixture fills the screen exactly");

    let (first, last) = (*SELECTED.start(), *SELECTED.end());
    let (anchor, focus) = (t.point_at(first, 2), t.point_at(last, 2));
    t.set_selection(anchor, focus);
    Some((r, t))
}

/// The fixture's own check that the text moved as the test claims: `line` is on
/// viewport `row`. A precondition — it reads the snapshot's cells, never its
/// `selection`, which is what the pixels are asserting.
fn assert_text_on_row(snap: &GridSnapshot, row: u16, line: u16) {
    let text = snap.to_text();
    assert_eq!(
        text.lines().nth(usize::from(row)),
        Some(selection_line(line).as_str()),
        "fixture: line {line:02} should be on viewport row {row}"
    );
}

/// R8.1 — one line of output scrolls the full screen by one, and the tint goes with
/// its text from row 2 to row 1.
///
/// Captured once **before** the output as well: that frame shows the measurement
/// finds the tint on row 2 at all, so the second frame's "row 1, not row 2" is a
/// move and not a highlight that was never where the test thinks it started.
#[test]
fn output_moves_the_selection_tint_with_its_text() {
    let Some((mut r, mut t)) = selected_on_row_2() else {
        return;
    };
    let geo = CellGeometry::of(&r);

    let before = t.snapshot();
    assert_text_on_row(&before, 2, 2);
    let frame = r.capture(&before).expect("read the rendered frame back");
    assert_tint_on(
        &tinted_cells(&frame, &geo, "before output"),
        2,
        None,
        "before output",
    );

    let (_, rows) = r.grid_size();
    t.advance(format!("\r\n{}", selection_line(rows)).as_bytes());
    assert_eq!(
        t.history_len(),
        1,
        "fixture: one line scrolled into history"
    );
    let after = t.snapshot();
    assert_text_on_row(&after, 1, 2);
    let frame = r.capture(&after).expect("read the rendered frame back");
    assert_tint_on(
        &tinted_cells(&frame, &geo, "after one line of output"),
        1,
        Some(2),
        "after one line of output",
    );
}

/// R8.2 — from R8.1's state, scrolling the viewport back one line puts the text on
/// row 2 again, and the tint with it. This is the frame the old "live bottom only"
/// projection (selection-model D9) drew with no highlight at all.
#[test]
fn scrolling_back_moves_the_selection_tint_back() {
    let Some((mut r, mut t)) = selected_on_row_2() else {
        return;
    };
    let geo = CellGeometry::of(&r);
    let (_, rows) = r.grid_size();
    t.advance(format!("\r\n{}", selection_line(rows)).as_bytes());

    t.scroll_up(1);
    assert_eq!(t.view_offset(), 1, "fixture: scrolled back one line");
    let snap = t.snapshot();
    assert_text_on_row(&snap, 2, 2);
    let frame = r.capture(&snap).expect("read the rendered frame back");
    assert_tint_on(
        &tinted_cells(&frame, &geo, "scrolled back one line"),
        2,
        Some(1),
        "scrolled back one line",
    );
}

/// A runtime zoom (`Renderer::set_font_size`, terminal-font-size R2.1) has to put the
/// same pixels on screen as a renderer that was **built** at that size.
///
/// This is the pixel half of the zoom path. Its unit tests in `lib.rs` assert the
/// decisions — the override, `cell_size`, `ppem`, a bumped atlas epoch, one re-shape —
/// and all of them stay green if the grid's `glyph_bind_group` is left pointing at the
/// replaced atlas texture, which samples the old texture with the new atlas's UVs and
/// draws the wrong glyphs for the rest of the session (Known Gap G-a in
/// `specs/atlas-swap-bind-invalidation/spec.md`). Measured: deleting that rebuild left
/// the `renderer` and `app` suites at 1430 passed / 0 failed.
///
/// The oracle is a second renderer constructed at the target size, so the assertion
/// needs no font-specific fixture: the two frames must be byte-identical. The first
/// frame is drawn at the default size before zooming so the old atlas really holds
/// glyphs. Otherwise a stale bind group would sample an empty texture, and the
/// failure would come out blank rather than wrong.
#[test]
fn a_zoomed_frame_matches_one_rendered_at_that_size_from_the_start() {
    const ZOOMED_PT: f32 = 20.0;
    let feed = b"hello world\r\nsecond line";

    let cfg = Config::default();
    let Some(mut zoomed) = headless(&cfg) else {
        return;
    };
    let _ = render(&mut zoomed, feed);
    assert!(
        zoomed.set_font_size(ZOOMED_PT),
        "20pt differs from the default"
    );
    let (_, after_zoom) = render(&mut zoomed, feed);

    let mut cfg_at_size = Config::default();
    cfg_at_size.font.size_pt = ZOOMED_PT;
    let Some(mut fresh) = headless(&cfg_at_size) else {
        return;
    };
    assert_eq!(zoomed.grid_size(), fresh.grid_size());
    let (_, reference) = render(&mut fresh, feed);

    let bg = reference.modal_pixel();
    let inked = reference
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|&&px| is_ink(px, bg))
        .count();
    assert!(
        inked > 0,
        "the reference frame is blank; the comparison would be vacuous"
    );

    let differing = after_zoom
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(reference.pixels().as_chunks::<4>().0)
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing, 0,
        "{differing} pixels differ between the zoomed frame and one rendered at \
         {ZOOMED_PT}pt from the start; the grid is drawing from the wrong atlas"
    );
}
