//! What `main` installs, asserted as a value.
//!
//! Registration is the quietest way a feature dies. An overlay that is never
//! registered, or registered behind another that eats its keys, raises no error
//! anywhere — the chord simply does nothing, while every unit test of that
//! overlay's own logic keeps passing, because those tests call it directly and
//! never ask whether anything calls it.
//!
//! `main` cannot be tested here: it builds a winit `EventLoop`, which on macOS
//! exists only on the process's main thread, and the harness runs tests on
//! spawned ones. So the part worth pinning was moved *out* of it. Every extension
//! the app has now arrives through `feature_registrations(..)` into `App::new`,
//! which is the only entry; what remains inline in `main` is what genuinely
//! cannot be a value, and it is smaller than it reads: the event loop, and the
//! two `tabmeta` calls that install *singletons*.
//!
//! An earlier draft of this file claimed `main` contained no registration logic
//! at all. It was not true, and the exception was the whole problem: `main` also
//! registered the `tabmeta` pump, the tests here asserted a set of four, and
//! deleting that one line stopped the sidebar's git branch and agent status from
//! ever refreshing with the entire suite green. The pump now travels in the value
//! (`feature_registrations` takes the worker's handle), and the seven-overlay shape
//! is asserted below.
//!
//! These tests run the set all the way into the **live registries**, not just to
//! the `Vec` that feeds them. That distinction is the point: asserting the list
//! alone left `Registrations::install` — the step that decides whether anything
//! reaches `OverlayHost`/`InputRouter` at all — coverable by nothing, and it
//! could be replaced with a no-op while the whole suite stayed green.
//!
//! Two lines are still beyond reach, and are named rather than implied: `App`
//! storing the set, and `resumed` calling `install()`. Both need an `App`, which
//! needs an `EventLoopProxy`, which no test on this platform can obtain.

use app::no_deadline;
use app::Overlay;
use app::TabMetaHandle;
use app::{feature_handlers, feature_overlays, feature_registrations};

/// Priorities as the registered set actually reports them, highest first.
fn priorities(overlays: &[Box<dyn Overlay>]) -> Vec<i32> {
    overlays.iter().map(|o| o.priority()).collect()
}

/// The set is complete. A count is a blunt assertion, and that is the point: it
/// fails when a feature is added to the app but never wired in, which is a
/// mistake no other test in the crate can see.
#[test]
fn every_feature_extension_is_registered() {
    assert_eq!(
        feature_overlays(None, no_deadline()).len(),
        6,
        "the overlay set changed — session graph, rename, ai-monitor, image presenter, \
         selection presenter, region capture. If a feature was added, assert its priority below \
         too; if one was removed, say so here."
    );
    assert_eq!(feature_handlers().len(), 2, "Cmd+V paste and Cmd+C copy");
}

/// The set survives the trip into the live registries.
///
/// The gap this closes: every other test here reads the `Vec` that *feeds*
/// registration, and a `Registrations::install` that dropped its input — or an
/// `App` that never called it — would leave all of them passing while the running
/// app had no rename field, no ai-monitor, no Cmd+V and no Cmd+C. Nothing else in
/// the workspace looks at what a registry actually holds.
///
/// The layer count is asserted alongside, because an overlay contributes two
/// separate things (keys and a `DrawLayer`) and losing one of them is invisible
/// in the other.
#[test]
fn the_registered_set_reaches_the_live_registries() {
    let mut set = feature_registrations(None, None);
    assert_eq!(
        set.counts(),
        (2, 6),
        "the assembled set changed — 2 handlers (Cmd+V, Cmd+C), 6 overlays"
    );

    let (router, host, layers) = set.install();
    assert_eq!(
        host.priorities(),
        vec![300, 200, 100, 0, 0, 0],
        "the overlays that reached the host, in the order it offers keys to them"
    );
    assert_eq!(
        router.handler_count(),
        2,
        "handlers were assembled but did not reach the router"
    );
    assert_eq!(
        layers.len(),
        6,
        "every overlay contributes a DrawLayer; one that reaches the host without \
         its layer eats keys and draws nothing"
    );
    assert_eq!(
        set.counts(),
        (0, 0),
        "install left items pending; a second resumed would register them twice"
    );
}

/// The shape the app actually runs: the metadata pump joins the set.
///
/// This is the assertion whose absence let `main` own a fifth overlay in silence.
/// The pump is headless — priority `i32::MIN`, never visible, consumes no key,
/// draws a no-op layer — so nothing about a running app looks wrong when it is
/// missing; the sidebar's git branch and agent status simply stop refreshing,
/// because its `poll` is what turns the worker's wake into a repaint.
///
/// `TabMetaHandle::spawn` is safe to call here: it creates a channel and a thread
/// that blocks on an empty roster, and installs none of the singletons `main`
/// installs beside it (`tabmeta::install`, `TabModelHandle::shared`). Dropping the
/// handle disconnects the channel and the worker returns.
#[test]
fn the_metadata_pump_joins_the_set_the_app_runs() {
    let Some(handle) = TabMetaHandle::spawn(|| {}) else {
        panic!("could not start the metadata worker, so its registration is untestable");
    };
    let mut set = feature_registrations(None, Some(handle));
    assert_eq!(
        set.counts(),
        (2, 7),
        "the pump did not join the assembled set"
    );

    let (_, host, layers) = set.install();
    assert_eq!(
        host.priorities(),
        vec![300, 200, 100, 0, 0, 0, i32::MIN],
        "the pump must reach the host, and must sit behind every real overlay so \
         it can never take a key from one"
    );
    assert_eq!(layers.len(), 7, "the pump's no-op layer went missing");
}

/// D8a.1: an open rename field owns the keyboard, so it must sit ahead of the
/// ai-monitor panel. This is a *relative* fact between two independently written
/// features, which is exactly the kind neither feature's own tests can hold.
#[test]
fn the_rename_field_outranks_the_ai_monitor_panel() {
    let overlays = feature_overlays(None, no_deadline());
    let prios = priorities(&overlays);
    assert_eq!(
        prios,
        vec![300, 200, 100, 0, 0, 0],
        "registration priorities changed; keyboard ownership between features \
         depends on this order (D8a.1). region-capture sits at 0, BELOW the rename \
         field — its D1 'an open rename field swallows the chord' is only true \
         while it does."
    );
}

/// session-flow-graph D5/R1.4: the open graph covers the whole window — the rename field
/// and the ai-monitor included — so it must see every key before either of them. It is
/// the one overlay that covers the window, and it outranks every other registered overlay.
/// A relative fact between independently written features, like the rename field's lead
/// over the monitor above.
#[test]
fn the_session_graph_outranks_every_other_overlay() {
    let overlays = feature_overlays(None, no_deadline());
    let graphs: Vec<i32> = overlays
        .iter()
        .filter(|o| o.covers_window())
        .map(|o| o.priority())
        .collect();
    assert_eq!(
        graphs,
        vec![300],
        "exactly one window-covering overlay, at 300"
    );
    let highest_other = overlays
        .iter()
        .filter(|o| !o.covers_window())
        .map(|o| o.priority())
        .max()
        .expect("other overlays are registered");
    assert!(
        300 > highest_other,
        "the session graph must outrank every other overlay (highest other: {highest_other})"
    );
}

/// Dispatch offers keys in priority order, so registration order must not be the
/// thing that decides it — but a set that arrives already sorted hides a broken
/// sort. Asserting it is descending pins the intent: the list reads top-down the
/// way keys are offered.
#[test]
fn the_registration_list_reads_in_the_order_keys_are_offered() {
    let overlays = feature_overlays(None, no_deadline());
    let prios = priorities(&overlays);
    let mut sorted = prios.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(
        prios, sorted,
        "the registration list is no longer in descending priority order, so it \
         no longer reads the way keys are dispatched"
    );
}

/// None of the extensions is visible before anything opens it. A feature that
/// registered itself already-open would reserve columns, or eat keys, from the
/// first frame — and would look like a broken terminal, not like a broken overlay.
#[test]
fn no_extension_starts_visible() {
    for (i, o) in feature_overlays(None, no_deadline()).iter().enumerate() {
        assert!(
            !o.visible(),
            "overlay #{i} (priority {}) is visible before anything opened it",
            o.priority()
        );
    }
}

/// R10.4: the new background-write seam is inert for everything that was already here.
///
/// `Overlay::take_pending_write` was added for region-capture and defaults to `None`,
/// which is what let it arrive without touching rename, the ai-monitor, or either
/// presenter. Asserted on the **registered** set rather than on hand-built stubs,
/// because that is the set the running app dispatches through — and because a capture
/// overlay that somehow started with bytes queued would put them on a PTY at the first
/// wake, before any chord was pressed.
#[test]
fn no_registered_overlay_starts_with_bytes_for_a_pty() {
    for (i, o) in feature_overlays(None, no_deadline()).iter_mut().enumerate() {
        assert_eq!(
            o.take_pending_write(),
            None,
            "overlay #{i} (priority {}) had bytes queued for a shell before anything \
             happened",
            o.priority()
        );
    }
}
