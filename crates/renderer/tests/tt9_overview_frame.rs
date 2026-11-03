//! TT9 — acceptance tests for session-overview-ui R9.5–R9.6: the overview layer's frame
//! carries the design's colours (D21, D8), and its tile numbers use the Display face.
//!
//! Both go through `Renderer::capture`, as `golden_frame.rs` and `tt12_graph_frame.rs` do:
//! the layout's rects and the layer's counters report *decisions*, and stay correct while
//! the frame they describe is blank. The GPU harness is `renderer::testing::headless`,
//! which skips only when no adapter exists (`SKIP_GPU_TESTS=1`) and fails otherwise.
//!
//! The rects sampled are re-derived from the public `layout_overview` with the full
//! viewport (the overlay is full-window), the renderer's scale and scroll 0. At scale 1 a
//! 1440 px wide frame gives D15's content width 1376.

use std::cell::RefCell;
use std::rc::Rc;

use common::config::Config;
use renderer::overview_layout::{layout_overview, OverviewFace};
use renderer::overview_model::{
    AgentTag, Banner, Bucket, Counts, Filter, OverviewModel, SessionRowData, SubagentCell,
    SubagentStatus, SubagentTotals, TreeLine,
};
use renderer::overview_panel::{OverviewHandle, OverviewPanel};
use renderer::{CapturedFrame, DrawLayer, FrameCtx, Renderer};
use term_core::{Color, GridSnapshot, Terminal};

const FRAME_W: u32 = 1440;
const FRAME_H: u32 = 900;
/// R9.5's per-channel tolerance, 8-bit sRGB.
const TOL: i32 = 3;

/// D21: the Needs-you tile's opaque fill.
const NEEDS_YOU_FILL: [u8; 3] = [0x30, 0x27, 0x1B];
/// D8: a running square.
const RUNNING: [u8; 3] = [0xE8, 0x74, 0x4F];
/// D8: a done square.
const DONE: [u8; 3] = [0x6F, 0xA6, 0x7F];

fn snapshot(r: &Renderer) -> GridSnapshot {
    let cfg = Config::default();
    let (cols, rows) = r.grid_size();
    let mut t = Terminal::new(
        cols,
        rows,
        Color(cfg.theme.foreground),
        Color(cfg.theme.background),
    );
    t.advance(b"$ ls");
    t.snapshot()
}

/// Forwards to a shared `OverviewPanel`, so the test can read its observers after handing
/// the layer to the renderer (which takes it by `Box`).
struct Shared(Rc<RefCell<OverviewPanel>>);

impl DrawLayer for Shared {
    fn z_order(&self) -> i32 {
        self.0.borrow().z_order()
    }
    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        self.0.borrow_mut().record(ctx);
    }
}

fn row(tab: u64, bucket: Bucket, subagents: Vec<SubagentStatus>) -> SessionRowData {
    SessionRowData {
        tab,
        pane: 0,
        bucket,
        project: format!("project-{tab}"),
        agent_label: "Claude Code".into(),
        agent: AgentTag::Claude,
        branch_line: "main".into(),
        has_branch: true,
        now_doing: "Bash · cargo test".into(),
        prompt: None,
        elapsed: "3m".into(),
        subagent_summary: if subagents.is_empty() {
            "—".into()
        } else {
            "1 running · 1 done".into()
        },
        tree: subagents
            .iter()
            .map(|s| TreeLine {
                subagent_type: "general-purpose".into(),
                description: "—".into(),
                status_word: "running".into(),
                duration: "<1m".into(),
                status: *s,
            })
            .collect(),
        subagents: subagents.into_iter().map(SubagentCell::Square).collect(),
        expanded: false,
        selected: false,
    }
}

/// R9.5's model: one Needs-you row and a Working row with one running and one done
/// subagent.
fn model() -> OverviewModel {
    OverviewModel {
        resolving: false,
        rows: vec![
            row(1, Bucket::NeedsYou, vec![]),
            row(
                2,
                Bucket::Working,
                vec![SubagentStatus::Running, SubagentStatus::Done],
            ),
        ],
        counts: Counts {
            all: 2,
            needs_you: 1,
            working: 1,
            idle: 0,
        },
        banner: Some(Banner {
            tab: 1,
            pane: 0,
            text: "Claude Code · project-1 — Approval needed".into(),
            more: 0,
        }),
        filter: Filter::All,
        total_subagents: SubagentTotals {
            running: 1,
            done: 1,
            failed: 0,
        },
        view: Default::default(),
    }
}

fn near(px: [u8; 4], want: [u8; 3]) -> bool {
    (0..3).all(|k| (i32::from(px[k]) - i32::from(want[k])).abs() <= TOL)
}

fn sample(frame: &CapturedFrame, x: f32, y: f32) -> [u8; 4] {
    frame.pixel(x.floor() as u32, y.floor() as u32)
}

// ---- R9.5 ------------------------------------------------------------------------------

#[test]
fn r9_5_needs_you_tile_and_squares_show_the_design_colours() {
    let cfg = Config::default();
    let Some(mut r) = renderer::testing::headless(FRAME_W, FRAME_H, &cfg, "the overview frame")
    else {
        return;
    };
    let handle = OverviewHandle::new();
    let m = model();
    handle.set(m.clone());
    handle.set_visible(true);
    r.add_layer(Box::new(OverviewPanel::new(handle.clone())));
    let frame = r
        .capture(&snapshot(&r))
        .expect("read the rendered frame back");

    let lay = layout_overview(&m, [FRAME_W as f32, FRAME_H as f32], r.scale_factor(), 0);
    assert_eq!(lay.tiles.len(), 4, "the overview lays out four tiles");

    let tile = lay.tiles[0];
    let (x, y) = (tile[0] + 8.0, tile[1] + tile[3] / 2.0);
    let px = sample(&frame, x, y);
    assert!(
        near(px, NEEDS_YOU_FILL),
        "Needs-you tile {tile:?}: pixel ({x}, {y}) is {px:?}, want #30271B ±{TOL}"
    );

    for (status, want, name) in [
        (SubagentStatus::Running, RUNNING, "#E8744F"),
        (SubagentStatus::Done, DONE, "#6FA67F"),
    ] {
        let sq = lay
            .squares
            .iter()
            .find(|q| q.cell == SubagentCell::Square(status))
            .unwrap_or_else(|| panic!("no {status:?} square was laid out: {:?}", lay.squares));
        let (cx, cy) = (sq.rect[0] + sq.rect[2] / 2.0, sq.rect[1] + sq.rect[3] / 2.0);
        let px = sample(&frame, cx, cy);
        assert!(
            near(px, want),
            "{status:?} square {:?}: centre ({cx}, {cy}) is {px:?}, want {name} ±{TOL}",
            sq.rect
        );
    }
}

// ---- R9.6 ------------------------------------------------------------------------------

#[test]
fn r9_6_display_face_at_60_px_for_the_tile_numbers_at_scale_2() {
    let cfg = Config::default();
    let scale = 2.0;
    // Twice the 1440 × 900 frame, so the logical width at scale 2 is still 1440 and D15's
    // content width 1376 (at 1440 physical px it would be 720 logical — D15's too-narrow
    // state, which has no tiles). Fixture size only; no assertion changed.
    let (frame_w, frame_h) = (FRAME_W * 2, FRAME_H * 2);
    let Some(mut r) = renderer::testing::headless_scaled(
        frame_w,
        frame_h,
        scale,
        &cfg,
        "the overview Display-face frame",
    ) else {
        return;
    };
    let handle = OverviewHandle::new();
    let mut m = model();
    m.counts = Counts {
        all: 15,
        needs_you: 3,
        working: 5,
        idle: 7,
    };
    handle.set(m.clone());
    handle.set_visible(true);
    let panel = Rc::new(RefCell::new(OverviewPanel::new(handle.clone())));
    r.add_layer(Box::new(Shared(panel.clone())));
    r.capture(&snapshot(&r)).expect("the first visible frame");

    let ppems = panel.borrow().atlas_ppems();
    assert!(
        ppems
            .iter()
            .any(|&(f, p)| f == OverviewFace::Display && (p - 60.0).abs() < 0.01),
        "atlas_ppems {ppems:?} lists no Display atlas at 60 physical px"
    );
    for face in [OverviewFace::Title, OverviewFace::Meta] {
        assert!(
            ppems.iter().any(|&(f, _)| f == face),
            "atlas_ppems {ppems:?} lists no {face:?} atlas"
        );
    }

    let lay = layout_overview(&m, [frame_w as f32, frame_h as f32], scale, 0);
    assert_eq!(lay.tiles.len(), 4, "the overview lays out four tiles");
    let in_rect = |t: [f32; 4], r: [f32; 4]| {
        t[0] >= r[0] && t[1] >= r[1] && t[0] + t[2] <= r[0] + r[2] && t[1] + t[3] <= r[1] + r[3]
    };
    for (i, (tile, number)) in lay
        .tiles
        .iter()
        .zip([Some("3"), Some("5"), Some("7"), None])
        .enumerate()
    {
        let display: Vec<_> = lay
            .texts
            .iter()
            .filter(|t| t.face == OverviewFace::Display && in_rect(t.rect, *tile))
            .collect();
        assert_eq!(
            display.len(),
            1,
            "tile {i} {tile:?} holds {} Display runs, want its one number",
            display.len()
        );
        if let Some(n) = number {
            assert_eq!(display[0].text, n, "tile {i}'s number");
            assert!(
                !lay.texts.iter().any(|t| t.face != OverviewFace::Display
                    && in_rect(t.rect, *tile)
                    && t.text == n),
                "tile {i}'s number {n} is also laid out in a non-Display face"
            );
        }
    }
}
