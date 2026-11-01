//! TT10 — acceptance tests for `specs/session-overview-ui` R10.4 and R10.5 (D17, D18).
//!
//! * R10.4 lays out a ready model with no session through the pure
//!   `overview_layout::layout_overview` — no GPU.
//! * R10.5 records two frames through `Renderer::capture` with the overview layer added,
//!   as `golden_frame.rs` / `tt12_graph_frame.rs` do, and reads the layer's
//!   `compose_count()`. The GPU harness is `renderer::testing::headless`: it skips only
//!   when no adapter is available (`SKIP_GPU_TESTS=1`) and fails otherwise.

use std::cell::RefCell;
use std::rc::Rc;

use common::config::Config;
use renderer::overview_layout::{layout_overview, OverviewFace, OverviewLayout};
use renderer::overview_model::{
    AgentTag, Bucket, Counts, Filter, OverviewModel, SessionRowData, View,
};
use renderer::overview_panel::{OverviewHandle, OverviewPanel};
use renderer::{DrawLayer, FrameCtx, Renderer};
use term_core::{Color, GridSnapshot, Terminal};

/// Wide enough that the content width (viewport − 64) is past every D15 threshold.
const VIEWPORT: [f32; 2] = [1600.0, 1000.0];

fn inside(outer: [f32; 4], inner: [f32; 4]) -> bool {
    let [ox, oy, ow, oh] = outer;
    let [ix, iy, iw, ih] = inner;
    ix >= ox && iy >= oy && ix + iw <= ox + ow && iy + ih <= oy + oh
}

/// The Display-face (tile number) runs laid out inside `tile`.
fn tile_numbers(l: &OverviewLayout, tile: [f32; 4]) -> Vec<&str> {
    l.texts
        .iter()
        .filter(|t| t.face == OverviewFace::Display && inside(tile, t.rect))
        .map(|t| t.text.as_str())
        .collect()
}

// ---- R10.4 -------------------------------------------------------------------------------

/// The model a ready snapshot with no agent session builds: not resolving, no rows,
/// every count 0, no banner, filter `All`.
fn no_sessions() -> OverviewModel {
    OverviewModel {
        resolving: false,
        rows: Vec::new(),
        counts: Counts::default(),
        banner: None,
        filter: Filter::All,
        view: View::Overview,
        ..OverviewModel::default()
    }
}

// R10.4
#[test]
fn r10_4_no_sessions_tiles_read_zero_and_table_says_so() {
    let l = layout_overview(&no_sessions(), VIEWPORT, 1.0, 0);

    assert_eq!(
        l.tiles.len(),
        4,
        "the four tiles are laid out: {:?}",
        l.tiles
    );
    for (i, tile) in l.tiles.iter().enumerate() {
        let numbers = tile_numbers(&l, *tile);
        assert!(
            !numbers.is_empty(),
            "tile {i} has a Display-face number: {:?}",
            l.texts
        );
        for n in numbers {
            assert!(
                n.chars().any(|c| c.is_ascii_digit())
                    && n.chars().filter(char::is_ascii_digit).all(|c| c == '0'),
                "tile {i} reads 0, got {n:?}"
            );
        }
    }

    let msg = l
        .texts
        .iter()
        .find(|t| t.text == "No agent sessions in this window.")
        .unwrap_or_else(|| panic!("the empty-table line is laid out: {:?}", l.texts));
    assert!(
        inside(l.table, msg.rect),
        "the line sits in the table box {:?}, got {:?}",
        l.table,
        msg.rect
    );
}

// ---- R10.5 -------------------------------------------------------------------------------

/// One Working Claude row, ready — something real for the layer to compose.
fn one_row() -> OverviewModel {
    OverviewModel {
        resolving: false,
        rows: vec![SessionRowData {
            tab: 1,
            pane: 0,
            bucket: Bucket::Working,
            project: "terminal".into(),
            agent_label: "Claude Code".into(),
            agent: AgentTag::Claude,
            branch_line: "main".into(),
            has_branch: true,
            now_doing: "Bash · cargo test".into(),
            prompt: Some("fix the build".into()),
            elapsed: "12m".into(),
            subagent_summary: "—".into(),
            ..SessionRowData::default()
        }],
        counts: Counts {
            all: 1,
            working: 1,
            ..Counts::default()
        },
        ..OverviewModel::default()
    }
}

/// Forwards to a shared `OverviewPanel`, so the test can read its counter after handing
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

fn snapshot_for(r: &Renderer) -> GridSnapshot {
    let cfg = Config::default();
    let (cols, rows) = r.grid_size();
    let mut t = Terminal::new(
        cols,
        rows,
        Color(cfg.theme.foreground),
        Color(cfg.theme.background),
    );
    t.advance(b"$ ");
    t.snapshot()
}

// R10.5
#[test]
fn r10_5_an_unchanged_second_frame_does_not_compose() {
    let cfg = Config::default();
    let handle = OverviewHandle::new();
    handle.set(one_row());
    handle.set_visible(true);
    let Some(mut r) =
        renderer::testing::headless(1200, 800, &cfg, "the overview's unchanged-frame check")
    else {
        return;
    };
    let panel = Rc::new(RefCell::new(OverviewPanel::new(handle.clone())));
    r.add_layer(Box::new(Shared(panel.clone())));
    let snap = snapshot_for(&r);

    r.capture(&snap).expect("frame 1");
    let first = panel.borrow().compose_count();
    assert!(
        first > 0,
        "the first visible frame composes (else the check below is vacuous)"
    );

    // Model, scroll, hover and view untouched.
    r.capture(&snap).expect("frame 2");
    assert_eq!(
        panel.borrow().compose_count(),
        first,
        "an unchanged second frame re-composed"
    );
}
