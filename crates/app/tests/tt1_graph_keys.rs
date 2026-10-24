//! Acceptance tests for session-flow-graph R1: open, close and key ownership (D4, D5).
//! Written test-first from the spec's Thens; every item is reached through the `app` crate
//! root, plus the renderer's `graph_panel::FlowHandle` the overlay is built over (T3).

use std::sync::Arc;

use app::{
    feature_overlays, no_deadline, AppCtx, FlowOverlay, Handled, KeyInput, Overlay, OverlayCommand,
};
use renderer::graph_panel::FlowHandle;
use winit::keyboard::{Key, KeyCode, ModifiersState as M, NamedKey, PhysicalKey};

/// A closed graph overlay with no worker and a wake that does nothing.
fn graph() -> FlowOverlay {
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
    FlowOverlay::new(wake, None, FlowHandle::default())
}

fn character(ch: &str, mods: M, code: KeyCode) -> KeyInput {
    KeyInput {
        key: Key::Character(ch.into()),
        text: if mods.super_key() {
            None
        } else {
            Some(ch.into())
        },
        mods,
        physical: PhysicalKey::Code(code),
    }
}

fn named(key: NamedKey, text: Option<&str>, code: KeyCode) -> KeyInput {
    KeyInput {
        key: Key::Named(key),
        text: text.map(str::to_string),
        mods: M::empty(),
        physical: PhysicalKey::Code(code),
    }
}

fn cmd_shift_f() -> KeyInput {
    character("F", M::SUPER | M::SHIFT, KeyCode::KeyF)
}

fn cmd_shift_m() -> KeyInput {
    character("M", M::SUPER | M::SHIFT, KeyCode::KeyM)
}

fn cmd_t() -> KeyInput {
    character("t", M::SUPER, KeyCode::KeyT)
}

fn plain_a() -> KeyInput {
    character("a", M::empty(), KeyCode::KeyA)
}

fn enter() -> KeyInput {
    named(NamedKey::Enter, Some("\r"), KeyCode::Enter)
}

fn esc() -> KeyInput {
    named(NamedKey::Escape, Some("\u{1b}"), KeyCode::Escape)
}

/// Offer one key to `o` through a fresh context; return its verdict and what it queued.
fn press(o: &mut FlowOverlay, key: &KeyInput) -> (Handled, Vec<app::AppRequest>) {
    let mut pty: Vec<u8> = Vec::new();
    let mut ctx = AppCtx::new(&mut pty, 1, 0, None);
    let handled = o.on_key(key, &mut ctx);
    (handled, ctx.take_requests())
}

// R1.1
#[test]
fn r1_1_cmd_shift_f_twice_opens_then_closes_and_queues_nothing() {
    let mut o = graph();
    assert!(!o.visible(), "the Given is a closed overlay");

    let (first, queued) = press(&mut o, &cmd_shift_f());
    assert_eq!(first, Handled::Consumed);
    assert!(queued.is_empty(), "the first chord queued {queued:?}");
    assert!(o.visible(), "the first Cmd+Shift+F must open the graph");

    let (second, queued) = press(&mut o, &cmd_shift_f());
    assert_eq!(second, Handled::Consumed);
    assert!(queued.is_empty(), "the second chord queued {queued:?}");
    assert!(!o.visible(), "the second Cmd+Shift+F must close the graph");
}

// R1.2
#[test]
fn r1_2_open_graph_swallows_every_key_and_esc_closes_it() {
    let mut o = graph();
    let _ = press(&mut o, &cmd_shift_f());
    assert!(o.visible(), "the Given is an open overlay");

    for (name, key) in [
        ("a", plain_a()),
        ("Enter", enter()),
        ("Cmd+T", cmd_t()),
        ("Cmd+Shift+M", cmd_shift_m()),
    ] {
        let (handled, queued) = press(&mut o, &key);
        assert_eq!(handled, Handled::Consumed, "{name} was not swallowed");
        assert!(queued.is_empty(), "{name} queued {queued:?}");
        assert!(o.visible(), "{name} closed the graph; only Esc may");
    }

    let (handled, queued) = press(&mut o, &esc());
    assert_eq!(handled, Handled::Consumed, "Esc was not consumed");
    assert!(queued.is_empty(), "Esc queued {queued:?}");
    assert!(!o.visible(), "Esc must close the graph");
}

// R1.3
#[test]
fn r1_3_closed_graph_passes_other_keys_through() {
    let mut o = graph();
    for (name, key) in [
        ("a", plain_a()),
        ("Esc", esc()),
        ("Cmd+Shift+M", cmd_shift_m()),
    ] {
        let (handled, _) = press(&mut o, &key);
        assert_eq!(handled, Handled::Passthrough, "a closed graph took {name}");
        assert!(!o.visible(), "{name} opened the graph");
    }
}

// R1.4
#[test]
fn r1_4_graph_priority_is_300_and_highest() {
    let overlays = feature_overlays(None, no_deadline());
    // The graph is the one registered overlay that covers the window (D4/D6).
    let graphs: Vec<i32> = overlays
        .iter()
        .filter(|o| o.covers_window())
        .map(|o| o.priority())
        .collect();
    assert_eq!(graphs.len(), 1, "expected exactly one FlowOverlay");
    let graph_prio = graphs[0];
    assert_eq!(graph_prio, 300);

    let others: Vec<i32> = overlays
        .iter()
        .filter(|o| !o.covers_window())
        .map(|o| o.priority())
        .collect();
    assert!(
        others.iter().all(|&p| graph_prio > p),
        "the graph (300) must outrank every other overlay: {others:?}"
    );
}

// R1.5
#[test]
fn r1_5_restore_opens_the_monitor_and_leaves_the_graph_closed() {
    let mut overlays = feature_overlays(None, no_deadline());
    // Offer the restore exactly as `OverlayHost::command` does: highest priority first,
    // first `Consumed` wins (`RestoreMonitor` is not a fan-out).
    overlays.sort_by_key(|o| std::cmp::Reverse(o.priority()));
    let restore = OverlayCommand::RestoreMonitor {
        cwd: None,
        shell_pid: None,
    };
    for o in overlays.iter_mut() {
        if o.on_command(&restore) == Handled::Consumed {
            break;
        }
    }

    let monitor = overlays
        .iter()
        .find(|o| o.priority() == 100)
        .expect("the ai-monitor is registered at 100");
    assert!(monitor.visible(), "the restore did not open the ai-monitor");

    let graph = overlays
        .iter()
        .find(|o| o.covers_window())
        .expect("the FlowOverlay is registered");
    assert!(!graph.visible(), "the restore opened the graph");
}
