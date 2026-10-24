//! TT3 (app half) — acceptance tests for session-flow-graph R3.1 and R3.2: the right
//! dock is the ai-monitor's alone again (D4). The monitor's chord toggles the monitor
//! only, and opening or closing the graph never touches the monitor's state.
//!
//! The graph overlay is taken from `app::feature_overlays(..)` — the one overlay that
//! `covers_window()` — rather than built through a constructor, so these tests drive the
//! overlay the app actually ships. Both overlays are dispatched through the real
//! `OverlayHost`, in its priority order; [`Shared`] forwards to them so each one's
//! `visible()` stays readable after the host takes ownership.
//!
//! R3.1 holds once T1 (the dock revert) is in. R3.2 is fulfilled by T4: until then the
//! graph does not close on `Esc`, and the test is red on that step.

use std::cell::RefCell;
use std::rc::Rc;

use app::{
    feature_overlays, no_deadline, AiMonitorOverlay, AppCtx, AppRequest, Handled, KeyInput,
    Overlay, OverlayCommand, OverlayHost,
};
use winit::keyboard::{Key, KeyCode, ModifiersState as M, NamedKey, PhysicalKey};

fn character(ch: &str, mods: M, code: KeyCode) -> KeyInput {
    KeyInput {
        key: Key::Character(ch.into()),
        text: None,
        mods,
        physical: PhysicalKey::Code(code),
    }
}

fn cmd_shift_m() -> KeyInput {
    character("M", M::SUPER | M::SHIFT, KeyCode::KeyM)
}

fn cmd_shift_f() -> KeyInput {
    character("F", M::SUPER | M::SHIFT, KeyCode::KeyF)
}

fn esc() -> KeyInput {
    KeyInput {
        key: Key::Named(NamedKey::Escape),
        text: Some("\u{1b}".into()),
        mods: M::empty(),
        physical: PhysicalKey::Code(KeyCode::Escape),
    }
}

/// An overlay registered with the host while the test keeps a handle on it.
type Cell = Rc<RefCell<Box<dyn Overlay>>>;

/// Forwards every trait method the host's key and command paths call to a shared
/// overlay, so its `visible()` can be read after the host owns the registration.
struct Shared(Cell);

impl Overlay for Shared {
    fn priority(&self) -> i32 {
        self.0.borrow().priority()
    }
    fn visible(&self) -> bool {
        self.0.borrow().visible()
    }
    fn on_key(&mut self, input: &KeyInput, ctx: &mut AppCtx) -> Handled {
        self.0.borrow_mut().on_key(input, ctx)
    }
    fn on_press(&mut self, point: [f32; 2], ctx: &mut AppCtx) -> Handled {
        self.0.borrow_mut().on_press(point, ctx)
    }
    fn covers_window(&self) -> bool {
        self.0.borrow().covers_window()
    }
    fn on_command(&mut self, cmd: &OverlayCommand) -> Handled {
        self.0.borrow_mut().on_command(cmd)
    }
}

/// The shipped graph overlay: the one registered overlay that covers the window.
fn graph() -> Cell {
    let mut graphs: Vec<Box<dyn Overlay>> = feature_overlays(None, no_deadline())
        .into_iter()
        .filter(|o| o.covers_window())
        .collect();
    assert_eq!(
        graphs.len(),
        1,
        "expected exactly one window-covering overlay"
    );
    Rc::new(RefCell::new(graphs.remove(0)))
}

/// A host holding a closed ai-monitor and a closed graph, plus handles on both.
fn host() -> (OverlayHost, Cell, Cell) {
    let monitor: Cell = Rc::new(RefCell::new(Box::new(AiMonitorOverlay::with_waker(
        None,
        no_deadline(),
    ))));
    let graph = graph();
    assert!(!monitor.borrow().visible(), "the monitor starts closed");
    assert!(!graph.borrow().visible(), "the graph starts closed");
    let mut host = OverlayHost::default();
    host.register(Box::new(Shared(monitor.clone())));
    host.register(Box::new(Shared(graph.clone())));
    (host, monitor, graph)
}

/// Dispatch one key through the host; return the requests it queued.
fn dispatch(host: &mut OverlayHost, key: &KeyInput) -> Vec<AppRequest> {
    let mut pty = Vec::new();
    let mut ctx = AppCtx::new(&mut pty, 1, 0, None);
    host.dispatch(key, &mut ctx);
    ctx.take_requests()
}

// R3.1
#[test]
fn r3_1_the_monitors_chord_toggles_the_monitor_only() {
    let (mut host, _monitor, graph) = host();

    assert_eq!(
        dispatch(&mut host, &cmd_shift_m()),
        vec![AppRequest::SetMonitorOpen(true)],
        "the first Cmd+Shift+M queues exactly SetMonitorOpen(true)"
    );
    assert!(
        !graph.borrow().visible(),
        "the monitor's chord opened the graph"
    );

    assert_eq!(
        dispatch(&mut host, &cmd_shift_m()),
        vec![AppRequest::SetMonitorOpen(false)],
        "the second Cmd+Shift+M queues exactly SetMonitorOpen(false)"
    );
    assert!(
        !graph.borrow().visible(),
        "the monitor's chord opened the graph"
    );
}

// R3.2
#[test]
fn r3_2_opening_the_graph_leaves_the_monitors_state_alone() {
    let (mut host, monitor, graph) = host();
    // Given: an open ai-monitor.
    dispatch(&mut host, &cmd_shift_m());
    assert!(monitor.borrow().visible(), "the monitor did not open");

    let mut pty = Vec::new();
    let mut ctx = AppCtx::new(&mut pty, 1, 0, None);

    // When: Cmd+Shift+F opens the graph ...
    host.dispatch(&cmd_shift_f(), &mut ctx);
    assert!(
        graph.borrow().visible(),
        "Cmd+Shift+F did not open the graph"
    );
    assert!(
        monitor.borrow().visible(),
        "opening the graph closed the monitor"
    );

    // ... and Esc closes it.
    host.dispatch(&esc(), &mut ctx);
    assert!(!graph.borrow().visible(), "Esc did not close the graph");
    assert!(
        monitor.borrow().visible(),
        "closing the graph closed the monitor"
    );

    // Then: no SetMonitorOpen request was queued across either key.
    let requests = ctx.take_requests();
    assert!(
        !requests
            .iter()
            .any(|r| matches!(r, AppRequest::SetMonitorOpen(_))),
        "the graph's open/close queued a monitor request: {requests:?}"
    );
}
