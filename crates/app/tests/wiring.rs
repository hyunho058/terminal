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
//! spawned ones. So the part worth pinning was moved *out* of it — `main` now
//! installs whatever `feature_overlays`/`feature_handlers` return, and what
//! remains inline there is only what genuinely cannot be a value: the event loop
//! and the process-global `tabmeta` worker.

use app::seam::Overlay;
use app::{feature_handlers, feature_overlays};

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
        feature_overlays(None).len(),
        4,
        "the overlay set changed — rename, ai-monitor, image presenter, selection \
         presenter. If a feature was added, assert its priority below too; if one \
         was removed, say so here."
    );
    assert_eq!(feature_handlers().len(), 2, "Cmd+V paste and Cmd+C copy");
}

/// D8a.1: an open rename field owns the keyboard, so it must sit ahead of the
/// ai-monitor panel. This is a *relative* fact between two independently written
/// features, which is exactly the kind neither feature's own tests can hold.
#[test]
fn the_rename_field_outranks_the_ai_monitor_panel() {
    let overlays = feature_overlays(None);
    let prios = priorities(&overlays);
    assert_eq!(
        prios,
        vec![200, 100, 0, 0],
        "registration priorities changed; keyboard ownership between features \
         depends on this order (D8a.1)"
    );
}

/// Dispatch offers keys in priority order, so registration order must not be the
/// thing that decides it — but a set that arrives already sorted hides a broken
/// sort. Asserting it is descending pins the intent: the list reads top-down the
/// way keys are offered.
#[test]
fn the_registration_list_reads_in_the_order_keys_are_offered() {
    let overlays = feature_overlays(None);
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
    for (i, o) in feature_overlays(None).iter().enumerate() {
        assert!(
            !o.visible(),
            "overlay #{i} (priority {}) is visible before anything opened it",
            o.priority()
        );
    }
}
