//! TT3 (renderer half) — acceptance test for session-flow-graph R3.3: the full-window
//! graph layer reserves no band (D4). The right dock is the ai-monitor's alone again, so
//! a visible `FlowPanel` with the monitor closed must leave `right_inset` at 0 and the
//! grid exactly the size it is with no flow layer at all.
//!
//! Both renderers record a real frame through `Renderer::capture` before the size is
//! read, because the inset is the renderer's state after a frame, not before one — and
//! the layer's `submit_count` is checked so the comparison is not made against a layer
//! that never drew. The GPU harness is `renderer::testing::headless`: it skips only under
//! `SKIP_GPU_TESTS=1` and fails when the renderer is present but broken.

use std::cell::RefCell;
use std::rc::Rc;

use common::config::Config;
use renderer::graph_layout::{FlowGraph, FlowNode, FlowTarget, NodeKind};
use renderer::graph_panel::{FlowHandle, FlowPanel};
use renderer::sidebar_card::CardStatus;
use renderer::sidebar_theme::AgentSkin;
use renderer::{DrawLayer, FrameCtx, Renderer};
use term_core::{Color, GridSnapshot, Terminal};

const FRAME_W: u32 = 1200;
const FRAME_H: u32 = 800;

fn headless(cfg: &Config) -> Option<Renderer> {
    renderer::testing::headless(FRAME_W, FRAME_H, cfg, "the session-flow graph inset")
}

/// An empty terminal sized to the renderer's current grid.
fn snapshot(r: &Renderer) -> GridSnapshot {
    let cfg = Config::default();
    let (cols, rows) = r.grid_size();
    let t = Terminal::new(
        cols,
        rows,
        Color(cfg.theme.foreground),
        Color(cfg.theme.background),
    );
    t.snapshot()
}

/// A project with one Claude session under it — enough for the layer to draw a frame.
fn graph() -> FlowGraph {
    let node = |kind, parent, title: &str, skin, status, target| FlowNode {
        kind,
        parent,
        title: title.into(),
        folder: "~/IdeaProjects/terminal".into(),
        branch: None,
        skin,
        status,
        muted: false,
        selected: false,
        target,
        extra: 0,
    };
    FlowGraph {
        nodes: vec![
            node(
                NodeKind::Project,
                None,
                "terminal",
                AgentSkin::Generic,
                CardStatus::Hidden,
                FlowTarget::None,
            ),
            node(
                NodeKind::Session,
                Some(0),
                "claude",
                AgentSkin::Claude,
                CardStatus::Working,
                FlowTarget::Node(1),
            ),
        ],
        resolving: false,
    }
}

/// Forwards to a shared `FlowPanel`, so the test can read its counters after handing the
/// layer to the renderer (which takes it by `Box`).
struct Shared(Rc<RefCell<FlowPanel>>);

impl DrawLayer for Shared {
    fn z_order(&self) -> i32 {
        self.0.borrow().z_order()
    }
    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        self.0.borrow_mut().record(ctx);
    }
}

// R3.3
#[test]
fn r3_3_a_visible_graph_layer_reserves_no_band() {
    let cfg = Config::default();

    // The grid with no flow layer, monitor closed, after one recorded frame.
    let Some(mut bare) = headless(&cfg) else {
        return;
    };
    bare.set_monitor_open(false);
    let snap = snapshot(&bare);
    bare.capture(&snap).expect("the bare frame captures");
    let bare_grid = bare.grid_size();

    // Given: monitor closed and a FlowPanel layer whose handle is visible.
    let Some(mut r) = headless(&cfg) else {
        return;
    };
    r.set_monitor_open(false);
    let handle = FlowHandle::default();
    handle.set(graph());
    handle.set_visible(true);
    let panel = Rc::new(RefCell::new(FlowPanel::new(handle.clone())));
    r.add_layer(Box::new(Shared(panel.clone())));

    // When: a frame records.
    let snap = snapshot(&r);
    r.capture(&snap).expect("the graph frame captures");
    assert!(
        panel.borrow().submit_count() > 0,
        "the visible graph layer drew nothing, so the frame says nothing about its inset"
    );

    // Then.
    assert_eq!(
        r.right_inset(),
        0.0,
        "the graph layer reserved a right band (D4: it covers the window, it does not dock)"
    );
    assert_eq!(
        r.grid_size(),
        bare_grid,
        "the grid with a visible graph layer differs from the grid with none"
    );
}
