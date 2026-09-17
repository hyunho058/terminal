//! ai-monitor panel (D4): a Cmd+Shift+M toggle showing the active session's folded
//! Claude JSONL telemetry, registered through the SD4 `Overlay` seam and drawn as a
//! right-docked band by the renderer's `monitor_panel` (SD3). Read-only (C3).
//!
//! **This file is impure by design, and that is what keeps `dashboard.rs` pure** (D19).
//! It already reads the config, the clock and the process table; it also resolves the
//! local UTC offset here and passes it in, so the composer stays a function of its
//! arguments and its `insta` snapshots stay byte-stable on any machine.
//!
//! Visibility-gated watcher lifecycle (D6): opening resolves the active tab's log
//! (from `cwd`), folds the existing file into a `SessionTelemetry`, and hands that
//! accumulator to a tail watcher which sends back `Arc<TelemetrySnapshot>`s; hiding
//! drops the watcher. Switching tabs while open re-points (D5/R3.1).
//!
//! **One panel per window, bound to the ACTIVE tab** (D13/R10). Docking changed where the
//! panel is painted, not what it is attached to: the attachment rule settled by
//! `specs/overlay-tab-binding` and `specs/ai-monitor-session-isolation` is carried forward
//! unchanged. What docking *did* add is D3's cumulative counters, which are session totals
//! rather than capped rings — so a re-point must rebuild them from the new tab's log
//! instead of continuing the old tab's. [`Session`] is the value that makes that one act.

use std::path::PathBuf;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ai_monitor::telemetry::{SessionTelemetry, TelemetrySnapshot};
use ai_monitor::{
    backfill_telemetry, project_dir, resolve_owned_session, LogWatcher, Wanted, Watch,
};
use renderer::monitor_model::MonitorModel;
use renderer::monitor_panel::{MonitorHandle, MonitorPanel, ScrollState};
use renderer::DrawLayer;
use winit::event_loop::EventLoopProxy;
use winit::keyboard::Key;

use crate::dashboard;
use crate::input::{self, WheelDelta};
use crate::seam::{AppCtx, Handled, KeyInput, Overlay, OverlayCommand};
use crate::tabsession;
use crate::terminal_app::UserEvent;

/// A rung's identity across snapshots (ai-monitor-panel-scroll D19).
///
/// The panel's rows carry **no identifier** — `Card`, `Row` and `Bar` are display strings and
/// nothing else — so an anchor has to be re-found by what it looks like. Two rejected
/// alternatives are worth remembering:
///
/// * **A stable id from the telemetry crate** would be exact, but the change would reach past
///   the renderer into `ai-monitor` and shake every snapshot test there. C8 keeps that crate
///   out of this feature's blast radius.
/// * **"Nth from the end of the section"** needs no identity at all, because new entries are
///   prepended — but a ring buffer *removes from the end*, so the invariant breaks precisely
///   once the ring is full. That is the steady state of a busy session, and RECENT CALLS at
///   its 32-entry cap is the acceptance criterion itself. The failure would concentrate
///   exactly where the feature is judged.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum RungKey {
    /// A section heading. The six labels are fixed and distinct, so no hash and no collision
    /// rule is needed — and a heading IS a normal anchor position: it is where every jump
    /// lands and where `max_index` sits.
    Heading(String),
    /// A body rung, keyed on everything it displays.
    Body(u64),
}

/// Where the view is holding on, if anywhere (D37).
#[derive(Clone, Debug, Default)]
enum Anchor {
    /// Top of the document. **No anchor is armed**, so new entries arriving above simply
    /// appear — which is what keeps the panel a live monitor for the reader who has not
    /// scrolled. Deriving this from `top == 0` rather than latching it is what stops the two
    /// notions disagreeing when the document shrinks under an anchored view.
    #[default]
    Following,
    /// The user scrolled away from the top; hold this rung where it is (D8).
    Held {
        key: RungKey,
        /// Its index in the document this anchor was taken from — the starting point for
        /// D15's nearest-survivor walk.
        index: usize,
    },
}

/// Everything a rung displays, folded into one number.
///
/// Deliberately covers the value colours and the outcome glyph as well as the text: a call
/// that changes from running to failed is a *different* row to the reader, and an anchor that
/// silently followed it would be holding onto something that is no longer what was being read.
fn rung_key(model: &MonitorModel, item: &renderer::monitor_layout::Item) -> RungKey {
    use renderer::monitor_layout::ItemKind;
    use renderer::monitor_model::SectionBody;
    use std::hash::{Hash, Hasher};
    let Some(section) = model.sections.get(item.section) else {
        return RungKey::Body(0);
    };
    if item.kind == ItemKind::Heading {
        return RungKey::Heading(section.label.clone());
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    section.label.hash(&mut h);
    match (&section.body, item.entry) {
        (SectionBody::Cards(v), Some(i)) => {
            if let Some(c) = v.get(i) {
                c.title.hash(&mut h);
                c.value.hash(&mut h);
                c.detail.hash(&mut h);
                format!("{:?}", c.accent).hash(&mut h);
            }
        }
        (SectionBody::Rows(v), Some(i)) => {
            if let Some(r) = v.get(i) {
                r.text.hash(&mut h);
                r.value.hash(&mut h);
                r.glyph.hash(&mut h);
                format!("{:?}", r.value_color).hash(&mut h);
                format!("{:?}", r.label.as_ref().map(|l| (&l.text, l.color))).hash(&mut h);
            }
        }
        (SectionBody::Bars(v), Some(i)) => {
            if let Some(b) = v.get(i) {
                b.name.hash(&mut h);
                b.count.hash(&mut h);
            }
        }
        _ => {}
    }
    RungKey::Body(h.finish())
}

/// How many rungs above `top` are new since the previous document (D11/R7.1).
///
/// "New" is set difference, not arithmetic on lengths: entries arrive at the head of a ring
/// but they also *leave* at its tail, so `new_len - old_len` undercounts on any session long
/// enough to be evicting — which is exactly the session where the badge matters.
///
/// Zero while following (D37): at the top there is nothing above to have missed, and D33
/// makes reaching index 0 the badge's clear condition for the same reason.
fn count_new_above(prev: &[RungKey], next: &[RungKey], top: usize) -> u32 {
    if top == 0 {
        return 0;
    }
    let seen: std::collections::HashSet<&RungKey> = prev.iter().collect();
    next.iter()
        .take(top)
        .filter(|k| !seen.contains(k))
        .count()
        .min(u32::MAX as usize) as u32
}

/// Where `anchor` has ended up in the new document (D8/D15/D19).
///
/// Returns the new top index. Three cases, in order:
///
/// 1. **Following** — index 0, always. Nothing is held, so nothing has to be found.
/// 2. **The anchor still exists** — its new index. On duplicate keys (the same tool called
///    twice looks identical) the candidate nearest the old index wins; without that rule the
///    view teleports between two identical rows as the list shifts.
/// 3. **The anchor is gone** (D15) — walk outward from its old position through the *previous*
///    document and take the first rung that survived. Outward rather than downward so the view
///    lands next to what was being read instead of at an end.
fn resolve_anchor(anchor: &Anchor, prev: &[RungKey], next: &[RungKey]) -> usize {
    let Anchor::Held { key, index } = anchor else {
        return 0;
    };
    let nearest = |k: &RungKey, from: usize| -> Option<usize> {
        next.iter()
            .enumerate()
            .filter(|(_, c)| *c == k)
            .min_by_key(|(i, _)| i.abs_diff(from))
            .map(|(i, _)| i)
    };
    if let Some(i) = nearest(key, *index) {
        return i;
    }
    // D15: the anchored rung was evicted. Its neighbours in the OLD document are the closest
    // thing to "where the reader was".
    for d in 1..prev.len().max(1) {
        for cand in [index.checked_sub(d), index.checked_add(d)]
            .into_iter()
            .flatten()
        {
            if let Some(k) = prev.get(cand) {
                if let Some(i) = nearest(k, *index) {
                    return i;
                }
            }
        }
    }
    0
}

/// How the panel asks for a one-shot wake at an epoch-millis instant; `None` disarms
/// (ai-monitor-idle-timestamps D7).
///
/// An injected callback rather than a `TabMetaHandle`, and that is the same move
/// `Session::spawn` already makes with `session::Wake`: the overlay needs one verb, not a
/// handle, and taking the verb keeps the whole arming path constructible in a test. The two
/// routes it replaces both fail for concrete reasons — `tabmeta::global()` returns `None`
/// before `install`, so the entire arming path would be dead under `cargo test`; threading the
/// handle would couple the overlay to a type it never otherwise names.
pub type ArmDeadline = Box<dyn Fn(Option<u64>)>;

/// An [`ArmDeadline`] that goes nowhere, for a build with no metadata worker and for tests
/// that are not about the deadline.
pub fn no_deadline() -> ArmDeadline {
    Box::new(|_| {})
}

pub struct AiMonitorOverlay {
    visible: bool,
    handle: MonitorHandle,
    /// Sub-rung wheel remainder — the panel's own, never shared with the grid's (D23).
    wheel_residue: f32,
    /// Which sections are folded shut, by section index.
    ///
    /// The app's, not the model's, for the same reason the anchor is: the model is rebuilt
    /// from telemetry several times a second, so anything the *user* decided has to live
    /// outside it and be stamped back on. Sized lazily — the section count is the model's to
    /// state, and a fixed six here would be a second place that number is written down.
    folds: Vec<bool>,
    /// What the view is holding on to across snapshots (D8/D37).
    anchor: Anchor,
    /// Every rung of the document currently published, in ladder order. The anchor is
    /// re-found against the next one, and D11's badge is the difference between the two.
    keys: Vec<RungKey>,
    /// Everything that belongs to **one tab's session**, in one slot (D13/R10.1).
    /// There is exactly one — the panel is per *window*, not per tab — and a re-point
    /// replaces it whole rather than merging into it (R10.3). See [`Session`].
    session: Session,
    /// The re-point bookkeeping: which notification this panel was last opened for, and the
    /// rule for deciding whether a new one is a real change (D6/R4.3). Owning it as a unit
    /// rather than as loose fields is what puts the *choice of key* under test — the storm
    /// bug was in which value the call site stored, not in the comparison.
    repoint: Repoint,
    watcher: Option<LogWatcher>,
    /// The event-loop waker handed to every watcher this panel spawns.
    ///
    /// **`Option` for a test reason, and the reason is worth the field.** An
    /// `EventLoopProxy` can only come from a real winit `EventLoop`, which on macOS can
    /// only be built on the main thread — so while this was non-optional,
    /// `AiMonitorOverlay` could not be constructed under `cargo test` at all. Every other
    /// decision in this file was split into a free-standing unit to work around that
    /// ([`Repoint`], [`Attachment`], [`pane_cwd_with`], [`watch_with_rivals`], and the
    /// comments on each say so) — but D1's keystroke contract is a *verdict returned by a
    /// trait method*, and there is no unit to split out: what R1.2/R1.3 require is that
    /// `Overlay::on_key` itself answers `Passthrough` for `Esc` and `q`. A re-implemented
    /// copy of that rule in a test body would assert nothing about the shipped one.
    ///
    /// Production always holds `Some`: `main` passes the real proxy through
    /// [`crate::feature_overlays`]. A `None` waker means a watcher's wake never reaches
    /// the loop, which is why the constructors that produce one are documented as being
    /// for the decision paths only.
    proxy: Option<EventLoopProxy<UserEvent>>,
    /// This window's start time. A log whose last event precedes it stopped being
    /// written before the window existed, so it belongs to some other session.
    /// Captured here rather than at open, because open happens *after* the tab's own
    /// session may have started writing.
    ///
    /// A **window** fact, so it deliberately survives a re-point: it is when *this
    /// window* opened, not when the active tab's session started.
    started_ms: u64,
    /// The live rival set the watcher reads (D11/R6.2). Held here so membership can be
    /// republished during an open without dropping and respawning the watcher.
    rivals: ai_monitor::RivalHandle,
    /// Guards against overlapping `refresh_rivals` workers — see that method.
    refreshing: Arc<AtomicBool>,
    /// `[ai_monitor] context_window`, re-read on each open.
    context_window: Option<u64>,
    /// Whether this tab's process tree holds a `claude` process at all — the
    /// `is_some()` of the hint, captured before `unwrap_or_default()` flattens it
    /// away. It is the *only* thing that separates the two empty states the dashboard
    /// can show: "no Claude session in this tab" from "this tab's session has not
    /// written a log yet". Same lifecycle as `context_window`: re-decided on every
    /// `open()`, read by `refresh_model`, never recomputed per frame.
    tab_agent: Option<crate::tabmeta::AgentKind>,
    /// Which `claude` process this open asked, and what it answered — the baseline the
    /// rotation check compares against. See [`OpenedOn`] and [`session_rotated_with`].
    ///
    /// Same lifecycle as `tab_agent`: re-decided on every `open()`, never merged.
    /// `None` only when the tab has no `claude` at all; a `claude` that has not yet named a
    /// session is `Some` with a `None` id, which the first valid record ARMS.
    ///
    opened_on: Option<OpenedOn>,
    /// The directory this open actually settled on — the **resolved** value, not the notified
    /// one `Repoint` keys on. The two are deliberately different quantities, and conflating
    /// them is the storm.
    ///
    /// Kept as its own field rather than folded into [`OpenedOn`] because `baseline_from` is
    /// built from the hint alone and is pinned that way; the directory is decided one step
    /// later, by the resolver. Read only by [`reopen_is_inert`].
    opened_dir: Option<PathBuf>,
    /// The last **published** `claude` pid this panel was told about (`TabMeta::agent_pid`) —
    /// the comparand for [`agent_pid_moved`].
    ///
    /// Deliberately **not** `opened_on.pid`: that is level-triggered (it changes only on a
    /// re-open) and it is `None` for a perfectly live session whose start time could not be
    /// read, while this is edge-triggered against what the worker last said. The two
    /// derivations are different algorithms and may legitimately disagree; against
    /// `opened_on.pid` a stable disagreement would be permanent inequality, i.e. a per-wake
    /// re-point storm. Against this it fires once, on the value *changing*, and then rests.
    ///
    /// Updated only by a `Some` publish, so a transient walk failure or a flipped display
    /// toggle cannot erase the memory and make the next real publish look like a change.
    ///
    /// Maintained by `on_active_changed` rather than seeded inside `open()`, which is what
    /// makes all three `open()` callers work uniformly: the chord and the launch restore carry
    /// no agent pid of their own, and this field is already current by the time either runs,
    /// because the seam fires on every metadata sync. A panel opened before any sync simply has
    /// `None` here, and the `Some`-guard makes the first publish a no-op rather than a
    /// re-point. `reopen_is_inert` remains the backstop if one ever slips through.
    last_seen_pid: Option<u32>,
    /// Set by the rival worker when it finds a `claude` under a tab whose panel opened
    /// WITHOUT one — the state `opened_on: None` describes (D-rot2).
    ///
    /// Cross-thread because the only cheap-enough place to look is the thread that already
    /// forks `/bin/ps`: asking on the event loop would put a fork on every wake. The event
    /// loop consumes the flag and answers with a full re-point.
    claude_appeared: Arc<AtomicBool>,
    /// The last model handed to [`MonitorHandle::set`] — D9's short-circuit, and the only
    /// thing that makes D2's "rebuild on every wake" affordable.
    ///
    /// Held here rather than read back from the handle because the comparison is an *app*
    /// decision: the model is the authority on whether anything the reader can see has moved,
    /// and it is already stage one's cache key, so the diff that gates the frame is one the
    /// renderer would otherwise pay for a frame later. `None` before the first publish, which
    /// is what makes that first rebuild take the full path.
    ///
    /// **Cost, stated rather than glossed:** a *changed* rebuild clones the model once to keep
    /// this copy. D9 says a real change costs "exactly what `refresh_model` costs today, not a
    /// byte more"; that is the one byte more. It rides only on rebuilds that already trigger a
    /// full stage-one re-shape, which walks every glyph in the document, so it is noise against
    /// the work it gates — but it is not nothing, and the alternative (a hand-maintained
    /// fingerprint instead of the real `PartialEq`) is the exact "both halves or neither"
    /// failure `monitor_panel.rs` warns about.
    published: Option<MonitorModel>,
    /// How many times `dashboard::model` has been composed here (R1.1/R1.3/R1.4).
    ///
    /// A counter rather than an output comparison, for the reason `monitor_panel.rs` records
    /// for its own `layouts`: composing is pure, so a test that only compared the published
    /// model would pass with D9's gate deleted — it would just recompute the same answer.
    /// Only a count can tell "skipped" from "recomputed to an equal value".
    #[cfg(test)]
    models_built: u32,
    /// How many times the work **past** D9's gate has run: the ladder, the rung keys, the
    /// anchor walk and the badge count (R1.2). Distinct from `models_built` on purpose — the
    /// gap between the two IS the short-circuit, and a test asserting one without the other
    /// cannot see it.
    #[cfg(test)]
    ladders_built: u32,
    /// How the panel asks the metadata worker for its one-shot wake (D7).
    arm_deadline: ArmDeadline,
    /// The `last_event_ms` the currently-armed deadline was computed from; `None` = disarmed.
    ///
    /// **This one field is both arming triggers.** D11 names two — a rebuild that sees a newer
    /// `last_event_ms`, and the tab's agent leaving `Busy` — because the two inputs arrive on
    /// different wakes at different cadences, and a design that waited for them together arms
    /// nothing on the common path. Storing what the pending deadline was computed *from*
    /// collapses both into one test: arm when this is `None`, or when the snapshot's
    /// `last_event_ms` is newer than it. While the agent is `Busy` nothing is armed, so this
    /// stays `None` and the first non-`Busy` rebuild arms — the deferred arm, executing itself
    /// with no second mechanism. And once armed it holds the timestamp, so further non-`Busy`
    /// wakes with an unchanged timestamp arm nothing: the trigger is the edge, not the level.
    armed_from: Option<u64>,
    /// The tab's agent state from the last publish, stored **unconditionally** — including
    /// when it is absent (D18).
    ///
    /// Not guarded by `is_some()` the way `last_seen_pid` beside it is. That guard is the
    /// seam's no-information rule for an *identity*; applying it here would keep a stale
    /// `Busy` forever once the agent exited, suppressing every future deadline and pinning the
    /// pill at CONNECTED — fail-closed by the back door. Absent means "not mid-turn".
    agent_state: Option<crate::tabmeta::AgentState>,
}

impl AiMonitorOverlay {
    /// An overlay with **no** event-loop waker, for asserting the decisions that are not
    /// the watcher — D1's keystroke contract (R1.1/R1.2/R1.3).
    ///
    /// A watcher spawned by one of these would fold its log and never wake the loop to
    /// show it, so this must not be used to exercise the tail path. Drive it with no cwd
    /// and no shell pid and `open()` spawns nothing at all.
    #[cfg(test)]
    fn detached() -> Self {
        Self::with_waker(None, no_deadline())
    }

    /// Build the overlay over an optional event-loop waker.
    ///
    /// Public, and taking an `Option`, so [`crate::feature_overlays`] can hand back
    /// the registration set without an `EventLoop` — which on macOS exists only on
    /// the main thread, and so never inside a test.
    pub fn with_waker(proxy: Option<EventLoopProxy<UserEvent>>, arm_deadline: ArmDeadline) -> Self {
        AiMonitorOverlay {
            visible: false,
            handle: MonitorHandle::default(),
            wheel_residue: 0.0,
            folds: Vec::new(),
            anchor: Anchor::default(),
            keys: Vec::new(),
            session: Session::default(),
            repoint: Repoint::default(),
            watcher: None,
            proxy,
            started_ms: now_ms(),
            context_window: None,
            tab_agent: None,
            opened_on: None,
            opened_dir: None,
            last_seen_pid: None,
            claude_appeared: Arc::new(AtomicBool::new(false)),
            rivals: Arc::new(Mutex::new(ai_monitor::RivalSet::default())),
            refreshing: Arc::new(AtomicBool::new(false)),
            published: None,
            #[cfg(test)]
            models_built: 0,
            #[cfg(test)]
            ladders_built: 0,
            arm_deadline,
            armed_from: None,
            agent_state: None,
        }
    }

    /// Is this the Cmd+Shift+M toggle chord?
    fn is_toggle(input: &KeyInput) -> bool {
        if !(input.mods.super_key() && input.mods.shift_key()) {
            return false;
        }
        matches!(&input.key, Key::Character(s) if s.eq_ignore_ascii_case("m"))
    }

    /// The **active tmux pane's** cwd for the tab rooted at `shell_pid`, or `None` when this
    /// tab is not running tmux (tmux-pane-cwd D12/R3.1).
    ///
    /// Why this exists at all: under tmux the pane's shell is a child of the tmux *server*,
    /// not of the tab, so the tab's own cwd is the **client's** — measured as `~` while the
    /// pane sat in a worktree, which makes `project_dir` resolve a directory that does not
    /// exist and blanks every dashboard row.
    ///
    /// Resolved here rather than read from the tabmeta snapshot because the snapshot has no
    /// entry until a roster round-trip completes, and its absence falls back to exactly the
    /// wrong answer — so `tmux attach` followed quickly by the toggle would open on the
    /// client's project. A toggle is user-initiated and on no cadence, which is the cost
    /// licence `effective_root_uncached` is written for.
    ///
    /// Uses [`tabsession::resolved_root`]'s bounded wait, not the raw resolver: this runs on
    /// the winit event-loop thread, and `tmux list-clients` can block indefinitely against a
    /// wedged server. A hang must cost the feature, never the UI.
    fn pane_cwd(shell_pid: Option<u32>) -> Option<PathBuf> {
        pane_cwd_with(
            tabsession::resolved_root,
            crate::session::cwd_of_pid,
            shell_pid,
        )
    }

    /// Open on `cwd`: resolve the log, backfill, and start tailing (D1/D6).
    fn open(&mut self, cwd: Option<PathBuf>, shell_pid: Option<u32>) {
        // Ask the tab's own process tree which session it is running. This is the
        // only signal that distinguishes it from a session another application is
        // running in the same project directory.
        //
        // ABOVE the directory, where it used to sit below: the hint now carries the chosen
        // `claude`'s own cwd, and that is what the panel opens on, so the hint has to exist
        // before the directory can be chosen. Two places deciding the directory is the exact
        // shape that produced the re-open storm, so there is still only one.
        //
        // Which agent this tab runs decides which of the two attachment paths below applies
        // (AG7/AG9). Asked once, here, so the format handed to the backfill and the format
        // handed to the watcher come from the same answer — seeding in one and tailing in
        // another would fold a session into the wrong shape and report it as parse errors.
        //
        // An `agy` tab has no `claude`, so `hint_for` answers `None` and the directory below
        // falls through to the pane's cwd — which is the one the Antigravity branch wants.
        let agent = shell_pid.and_then(tabsession::agent_for);
        let h = shell_pid.and_then(tabsession::hint_for);
        // One call does both halves of the bookkeeping: it records the *notified* value as the
        // re-point key and returns the *resolved* value to open on, preferring the chosen
        // `claude`'s directory and falling back to the tmux pane's (R3.3). Keeping the two
        // straight is the whole storm fix, so it lives in a unit a test can drive (see
        // `Repoint`). A tmux tab that has a session now pays ONE `resolved_root` rather than
        // two, because `pane_cwd` — the other caller of it — is reached only when the hint
        // supplied no directory at all.
        let cwd = self
            .repoint
            .opened(cwd, shell_pid, h.as_ref(), hint_or_pane_cwd);
        // Recorded from the `Option`'s *shape*, before `unwrap_or_default()` destroys
        // it: `None` means no `claude` under this tab, while an empty-but-present hint
        // means one is running that named neither an id nor a start time. Assigned
        // here — above the `project_dir` branch below — so a tab running `claude` on a
        // cwd with no project directory still gets the "has not written a log yet"
        // wording rather than being told there is no session at all. Unconditional, so
        // re-pointing to a tab with no `claude` clears it.
        // The KIND now, not a bool (AG10). `agent_for`'s answer takes precedence, but only
        // for a kind this monitor actually reads: the two detectors walk the process tree by
        // different mechanisms (`proc_listchildpids` here, a `/bin/ps` table in `hint_for`),
        // and R5.6 requires a Claude tab to be unchanged by this feature.
        //
        // The `filter` is what makes `hint_for` a real floor. An earlier spelling was
        // `agent.map(..).or_else(..)` with a comment claiming the same property, and the
        // claim was false: `or_else` runs only when the left side is `None`, so a tab where
        // `agent_for` reached an UNMONITORED agent first (a `codex` nearer the root than a
        // deeper `claude`) lost its confirmed Claude verdict and dropped from "has not
        // written a log yet" to "no Claude session in this tab". Filtering to the monitored
        // kinds first means an unmonitored answer can no longer displace a confirmed one,
        // which is what the floor was always supposed to mean.
        self.tab_agent = agent
            .map(|(k, _)| k)
            .filter(|k| {
                matches!(
                    k,
                    crate::tabmeta::AgentKind::Claude | crate::tabmeta::AgentKind::Antigravity
                )
            })
            .or_else(|| h.is_some().then_some(crate::tabmeta::AgentKind::Claude));
        let hint = h.unwrap_or_default();
        // D10 — everything below rebuilds; if it would rebuild exactly what is already on
        // screen, stop here instead.
        //
        // Deriving the directory from `claude`'s cwd made this worth having: `Repoint` keys on
        // the **notified** cwd, which under tmux is the pane *shell's*, republished by the
        // tabmeta worker on every `cd`. So a `cd` inside the pane fires a full re-point — fork,
        // watcher respawn, complete backfill — that resolves the same `claude`, the same
        // directory and the same session. Correct, and entirely wasted, on the path this
        // feature exists to make cheaper.
        //
        // **AFTER `Repoint::opened` has re-keyed, and that ordering is load-bearing.** The
        // motivating case IS a changed notified cwd — that is why `changed()` fired at all — so
        // returning before `key_cwd` was updated would leave `changed()` true on every
        // subsequent wake and re-enter `open()` forever: the storm this decision opens by
        // warning about. Safe by construction here (condition 1 needs `opened`'s return value),
        // but this file has shipped that exact bug once, so it is stated rather than inferred.
        //
        // Honest scope: the hint lookup above still ran, so a pane `cd` still costs a `/bin/ps`
        // fork and a 250 ms-budgeted `tmux list-clients`. What this saves is the teardown, the
        // respawn and the full backfill. It also skips the `Config::load()` below, so an edited
        // `ai_monitor.context_window` waits for a real re-point — acceptable, since that value
        // is already documented as applying the next time the panel is opened.
        if reopen_is_inert(
            self.visible,
            self.watcher.is_some(),
            self.opened_dir.as_deref(),
            self.opened_on.as_ref(),
            cwd.as_deref(),
            &hint,
        ) {
            return;
        }
        // Re-read on open rather than holding a live handle: `settings::apply` does
        // not reach the overlays, and a value that takes effect the next time the
        // panel is opened is honest about that. A malformed config keeps the table.
        self.context_window = common::config::Config::load()
            .ok()
            .and_then(|c| c.ai_monitor.context_window);
        // What this open settled on, so the next one can tell an identical re-point from a real
        // move (D10 condition 1). Recorded unconditionally, beside the baseline it is compared
        // with: a stale directory here would make a genuine move look inert.
        self.opened_dir = cwd.clone();
        // The rotation baseline, from the SAME hint the resolution below uses, so the two can
        // never disagree about which session this open is for. Unconditional, like
        // `tab_agent` above: re-pointing to a tab with no agent must CLEAR it — a
        // stale baseline would compare the new tab's process against the old tab's session id
        // and re-point on every wake.
        self.opened_on = baseline_from(&hint);
        // Whatever a worker observed before this open is now answered by this open itself.
        //
        // A FRESH ALLOCATION, not `store(false)`, and for exactly the reason
        // `Session::repoint` gives for `attached_flag`: a `refresh_rivals` worker spawned by
        // the previous open may still be inside `hint_for` (which forks `/bin/ps` and can sit
        // 250 ms in `resolved_root`), and it holds a clone of whatever `Arc` was current when
        // it started. Clearing in place leaves that worker writing into THIS open's decision —
        // tab A's worker finding tab A's `claude` and telling tab B's unarmed panel that one
        // appeared, which costs a spurious full re-point: another fork, two 250 ms-budgeted
        // tmux waits on the event-loop thread, a watcher respawn and a whole backfill.
        // Re-allocating orphans the in-flight writer instead, so its answer lands somewhere
        // nothing reads.
        self.claude_appeared = Arc::new(AtomicBool::new(false));
        // The previous tab's session goes as ONE value, and it goes here — *above* the
        // `project_dir` branch, so a tab that resolves no log still clears it. That
        // placement is what makes the worst case survivable: re-pointing to a tab with no
        // Claude at all would otherwise leave the last tab's snapshot on screen with
        // nothing ever arriving to replace it (D13/R10.3).
        self.session.repoint();
        self.watcher = None;

        if let Some(agy_pid) = antigravity_pid(agent) {
            // The Antigravity path (AG1/AG6/AG7). Deliberately NOT a variation on the Claude
            // path below: there is no project directory, no `Wanted`, and no rival contest,
            // because the presence lock names the conversation outright. What is left is a
            // single question — which transcript — asked once here.
            //
            // The FD probe lives in `app` (AG11) because enumerating another process's open
            // descriptors needs syscalls `ai-monitor` deliberately cannot make; the crate
            // takes the answer, not the capability.
            // A live agent is confirmed here by construction — `antigravity_pid` only
            // answers `Some` when the tree walk found `agy` — so a probe that names nothing
            // is `LiveButUnnamed`, never `NoAgent`. That distinction is the whole fix: it
            // stops the history tier adopting an older, possibly `/exit`ed conversation
            // during the ~7s before `agy` writes its lock.
            let probed = crate::presence_fd::conversation_of_pid(agy_pid);
            let probed = match probed.as_deref() {
                Some(id) => ai_monitor::antigravity::Probed::Conversation(id),
                None => ai_monitor::antigravity::Probed::LiveButUnnamed,
            };
            let resolved = cwd
                .as_deref()
                .and_then(|c| ai_monitor::antigravity::resolve(probed, c));
            let owned = resolved.as_ref().is_some_and(|r| r.owned);
            let log = resolved.map(|r| r.path);
            let (telemetry, offset) = match log.as_deref() {
                Some(p) => ai_monitor::backfill_telemetry_with(
                    p,
                    ai_monitor::telemetry::LogFormat::Antigravity,
                ),
                // Unresolved is a real, reachable state, not an error: a cold-started `agy`
                // has no transcript yet, and AG6 leaves a tab whose conversation cannot be
                // named unattached rather than adopting a neighbour's (R1.5/R1.6).
                None => (
                    SessionTelemetry::with_format(ai_monitor::telemetry::LogFormat::Antigravity),
                    0,
                ),
            };
            self.session
                .note(Arc::new(telemetry.snapshot()), self.started_ms, owned);
            // ARMED EITHER WAY, and the "either" is the fix for a reported defect.
            //
            // The original spelling armed nothing when no log resolved, reasoning that "the
            // conversation id IS the directory name, so until one is resolved there is no
            // path that could become right — the next re-point asks again". The first half is
            // true and the conclusion does not follow: **there is no next re-point**. The
            // panel's re-probe hangs off `on_active_changed`, which only runs on a wake, and
            // with no watcher armed the sole wake source is the tab-metadata worker — which
            // publishes only when a `TabMeta` actually changed and so goes silent next to a
            // steadily-running `agy`. Measured: a panel opened seconds before `agy` became
            // detectable stayed on "(no Claude session in this tab)" indefinitely, while
            // `agent_for` answered `Some((Antigravity, …))` for that very shell when probed
            // directly. Claude's branch has armed unconditionally all along, for this reason.
            //
            // So: tail the transcript when there is one, and otherwise watch the presence
            // directory for the lock that will name it.
            if let Some(log) = log.as_deref() {
                let proxy = self.proxy.clone();
                self.watcher = LogWatcher::spawn(
                    Watch {
                        dir: log.parent().map(PathBuf::from).unwrap_or_default(),
                        log: Some(log.to_path_buf()),
                        owned,
                        attached: self.session.attached_flag.clone(),
                        scope: ai_monitor::WatchScope::SingleLog,
                    },
                    telemetry,
                    offset,
                    move || {
                        if let Some(p) = proxy.as_ref() {
                            let _ = p.send_event(UserEvent::OverlayPending);
                        }
                    },
                );
            } else if let Some(presence) = ai_monitor::antigravity::presence_dir() {
                let proxy = self.proxy.clone();
                self.watcher = LogWatcher::spawn(
                    Watch {
                        dir: presence,
                        log: None,
                        owned: false,
                        attached: self.session.attached_flag.clone(),
                        scope: ai_monitor::WatchScope::AwaitingLock,
                    },
                    telemetry,
                    0,
                    move || {
                        if let Some(p) = proxy.as_ref() {
                            let _ = p.send_event(UserEvent::OverlayPending);
                        }
                    },
                );
            }
        } else if let Some(dir) = cwd.as_deref().and_then(project_dir) {
            // Resolve *here* rather than latching one log for the watcher's lifetime:
            // it may not be this tab's session, and the tab's own session may not have
            // written a file yet. The watcher re-resolves with the same hint.
            let want = match hint.is_empty() {
                // No claude process under this tab: nothing to confirm with. An empty
                // `Wanted` confirms nothing on either tier, so strict resolution
                // answers `None` — the panel shows an explained empty state rather
                // than the directory's most recently active log, which is very often
                // some other application's.
                true => Wanted::default(),
                false => Wanted {
                    session_id: hint.session_id.clone(),
                    // TIER 2 ONLY AGAINST A PROVABLE DIRECTORY. This is where the protection
                    // the deleted cwd gate used to provide actually lives now, and in a
                    // stronger form: not "compare two directories and refuse on mismatch", but
                    // "the timing window only ever runs against a directory the chosen `claude`
                    // is provably in". Tier 1 is untouched either way — it looks the log up by
                    // exact filename, so a wrong directory merely fails to find one, while tier
                    // 2's asymmetric window plus rival assignment can return a STRANGER's log
                    // with `owned: true`, which this file then latches.
                    //
                    // Expressed here, on `Wanted`, and emphatically NOT by zeroing
                    // `hint.started_ms` — the two hold the same quantity and are easy to
                    // confuse. Zeroing the hint field would make `baseline_from` yield `None`,
                    // so `opened_on` would be `None`, so `session_rotated` would short-circuit
                    // and rotation detection would be dead for the life of the open; and
                    // `rival_refresh_needed(attached, armed = false)` would then return `true`
                    // forever, putting a `/bin/ps` fork and a 250 ms `resolved_root` on every
                    // append burst — the exact cost `refresh_rivals`' attached guard exists to
                    // prevent.
                    since_ms: match hint.cwd.as_ref().is_some_and(tabsession::HintCwd::tier2_ok) {
                        true => hint.started_ms,
                        false => None,
                    },
                },
            };
            // STRICT: confirmation-only (R4.1). A session this tab cannot prove is its
            // own resolves to `None`, and `None` means no file is opened, read, or
            // backfilled — the strictness lives where the file is *chosen*, not where
            // it is displayed, so a foreign log is never folded into a
            // `SessionTelemetry` in the first place. `resolve_session`'s ranked
            // fallback remains public and under test; it has no production caller.
            // The rival set travels by HANDLE, not by value: membership changes during an
            // open (a sibling starts or exits `claude`), and a frozen list gets both
            // directions wrong — see `ai_monitor::Watch::rivals`. `rivals_known` rides
            // with it, because an empty list from a failed read must never earn the
            // unbounded uniqueness rescue.
            // `publish_rivals_into` STORES the handle in `self.rivals` and returns the clone
            // that `watch_with_rivals` puts in the `Watch`. The L2 checker found this broken:
            // the handle was built locally, handed to the watcher, and `self.rivals` left
            // pointing at the one from `new()` — so `refresh_rivals` wrote somewhere nothing
            // read, and membership was frozen for the life of the open, exactly as before D11.
            //
            // This is NOT "unrepresentable" — an earlier comment here claimed that and it was
            // false. `RivalHandle` is a public alias, so any caller can still build its own
            // `Arc`. What is true: the store-and-return pairing means the omitted-call
            // spelling fails the dead-code lint, and `watch_with_rivals` puts the
            // slot→`Watch` pairing under a test that a *substituted* handle fails (M9).
            // A return value discarded by `open()` itself (M8) is caught by NOTHING — see
            // `specs/ai-monitor-rival-assignment/loop.md` § Partly covered. Naming the
            // mutation this seam actually catches, rather than the one it does not, because
            // three iterations of this loop each shipped a comment claiming more
            // verification than existed.
            let rivals = publish_rivals_into(&mut self.rivals, &hint);
            let resolved =
                resolve_owned_session(&dir, &want, &hint.rival_starts, hint.rivals_known);
            let owned = resolved.as_ref().is_some_and(|r| r.owned);
            let log = resolved.map(|r| r.path);
            // Seed on THIS thread, exactly where `backfill` ran before: publish the
            // seeded snapshot first, then hand the accumulator to the watch thread.
            let (telemetry, offset) = backfill_from_scratch(log.as_deref());
            self.session
                .note(Arc::new(telemetry.snapshot()), self.started_ms, owned);
            let proxy = self.proxy.clone();
            // UNCONDITIONAL — the watch is armed even when the hint knows nothing and
            // strict resolution will therefore answer `None` on every re-resolution
            // (R6.1). Skipping the spawn here would restate the strictness rule in a
            // second place that has to agree with the resolver, and the last time this
            // rule was written twice the two copies disagreed and pinned the watcher to
            // a log the display would never show. One fs watch on one directory is the
            // cheaper of the two costs.
            //
            // The watcher re-resolves only until the shared flag below says we are
            // attached — it holds no copy of the rule. That flag is seeded from the
            // backfill decision just made above, which is the case that used to
            // disagree: work for a while, then open the panel, and the app was attached
            // while the watcher was not.
            self.watcher = LogWatcher::spawn(
                watch_with_rivals(
                    rivals,
                    dir,
                    log,
                    want,
                    owned,
                    self.session.attached_flag.clone(),
                ),
                telemetry,
                offset,
                move || {
                    if let Some(p) = proxy.as_ref() {
                        let _ = p.send_event(UserEvent::OverlayPending);
                    }
                },
            );
        }
        self.visible = true;
        self.reset_scroll();
        self.refresh_model();
    }

    /// Has the tab's session been rotated under us? See [`session_rotated_with`], which
    /// holds the rule; this supplies the one impure input.
    ///
    /// **The cadence, corrected.** An earlier draft of this comment said "the metadata tick,
    /// 500 ms". That was wrong in both directions, and the right answer was already written
    /// 20 lines below in `refresh_rivals`. The path is `OverlayPending` ->
    /// `feed_agent_states` -> `notify_active_changed` -> here, and `OverlayPending` is *also*
    /// the ai-monitor watcher's own wake — so while a session is writing this fires **per
    /// append burst**, much hotter than 500 ms. Colder, too: the tabmeta worker wakes the
    /// loop only when a `TabMeta` actually changed (`tabmeta::worker::publish_if_changed`),
    /// so an idle tab produces no tick at all. There is no cadence here to lean on.
    ///
    /// **So why no debounce?** Because the read was measured rather than assumed:
    /// `registered_session` costs **14 µs** warm (20k iterations, release, on the real
    /// `~/.claude/sessions/` — a 429-byte mean record plus a `serde_json` parse). The wake it
    /// rides on already runs `telemetry.snapshot()`, which deep-clones five rings and the
    /// task map and sorts. A debounce would add a clock to a module whose approved design
    /// (right-panel D2, pinned by `r7_4_the_loop_still_waits_and_the_panel_schedules_no_wake`)
    /// says it schedules nothing against one — a real cost, to save 14 µs against an 8 ms
    /// input-to-screen goal. The first draft added that clock. This is the correction.
    ///
    /// **Cost, stated plainly:** one 14 µs read per wake, only while the panel is open and
    /// only while the tab has a `claude` to ask. A closed panel and a tab with no `claude`
    /// add nothing at all — no read, no fork, no clock.
    fn session_rotated(&mut self) -> bool {
        // UNARMED — this open found no `claude` in the tab, so there is no process to ask and
        // no baseline to compare against. Reported by a user: open the window, press
        // Cmd+Shift+M, *then* start `claude`. The unattached watcher still finds the session
        // and fills the panel, so it looks healthy — while being blind to `/clear` for the
        // rest of the open, because `open()` is the only writer of the baseline and nothing
        // re-opens a tab that never changed.
        //
        // The answer is a full re-point rather than an arming, and the difference matters:
        // this open resolved with an EMPTY `Wanted` (there was no process to name a session),
        // so it is not merely missing an id — it is running the weakest resolution tier.
        // `open()` redoes the whole thing against a process that now exists.
        if self.opened_on.is_none() {
            return self.claude_appeared.swap(false, Ordering::AcqRel);
        }
        let outcome = session_rotated_with(ai_monitor::registered_session, self.opened_on.as_ref());
        apply_rotation(outcome, &mut self.opened_on)
    }

    /// Re-enumerate the live rivals into the shared handle (D11/R6.2).
    ///
    /// On a WORKER THREAD, never here: `hint_for` spawns `/bin/ps` and calls
    /// `proc_pidinfo` per `claude`, and this runs on the winit event loop after every
    /// metadata sync. The handle exists precisely so the answer can arrive late — the
    /// watcher reads whatever is current at its next re-resolution.
    ///
    /// A failed lookup publishes `RivalSet::default()`, i.e. `known: false`, which
    /// WITHHOLDS the uniqueness rescue. Failing toward "we do not know" is the whole
    /// point of the flag: the alternative grants the resolver's strongest branch on the
    /// strength of a read that did not work.
    ///
    /// TWO guards, because the trigger is far hotter than "once per metadata sync" reads:
    /// `on_active_changed` is reached from the `OverlayPending` arm, which is *also* the
    /// ai-monitor watcher's own wake — so while a session is writing, this fires per
    /// append burst, not on the worker's 500 ms tick.
    ///
    /// - **Attached ⇒ nothing.** Re-resolution stops once the panel has attached, so
    ///   membership can no longer change the answer; continuing to read would be pure
    ///   cost, and R6.3 requires the reads to stop.
    /// - **One in flight at a time.** `hint_for` forks `/bin/ps` and can block up to
    ///   250 ms inside `resolved_root`, so at these rates refreshes would otherwise
    ///   overlap and land out of order — a stale set overwriting a fresher one.
    fn refresh_rivals(&self, shell_pid: Option<u32>) {
        let Some(pid) = shell_pid else { return };
        if !rival_refresh_needed(
            self.session.attached_flag.load(Ordering::Relaxed),
            self.opened_on.is_some(),
        ) {
            return;
        }
        if self.refreshing.swap(true, Ordering::AcqRel) {
            return; // one outstanding refresh is enough
        }
        let handle = self.rivals.clone();
        // Only meaningful while unarmed; harmless otherwise, since `session_rotated` reads it
        // only on that branch and `open()` clears it.
        let appeared = self.claude_appeared.clone();
        // Cleared by `Drop`, not by a statement at the end of the closure. A panic anywhere
        // in `hint_for` would skip a manual `store(false)` and latch this `true` for the
        // life of the process — rival refresh silently dead forever, with nothing to
        // indicate it. Low probability, unbounded consequence, four lines to remove.
        let inflight = InFlight(self.refreshing.clone());
        // The flag alone is not enough to re-point, and this is why the proxy comes along.
        // `session_rotated` READS `appeared` on the same tick that `refresh_rivals` spawns
        // the worker that SETS it, so the value it reads is always the pre-worker one: the
        // re-point needs a *second* wake, and on a quiet tab there may never be one. Rather
        // than depend on unrelated traffic arriving later, the worker asks for that wake
        // itself, the moment it has something new to report.
        let proxy = self.proxy.clone();
        let ping = move || {
            if let Some(p) = proxy.as_ref() {
                let _ = p.send_event(UserEvent::OverlayPending);
            }
        };
        std::thread::spawn(move || {
            // The Antigravity half of "an agent appeared", and the reason a cold-started
            // `agy` tab heals itself (AG6/G8). `agy` publishes its presence lock ~7s after
            // the process starts, so the `open()` that ran at launch legitimately resolved
            // nothing; this re-probes off-thread and reports the moment the conversation
            // becomes nameable, which flips the unarmed branch of `session_rotated` and
            // triggers a full re-point. Without it the panel would sit on the honest but
            // permanent "has not written a log yet".
            //
            // Checked BEFORE `hint_for` because that call forks `/bin/ps`: an `agy` tab has
            // no `claude` to find, and paying a fork to learn so on every wake is the cost
            // this ordering avoids.
            if let Some((crate::tabmeta::AgentKind::Antigravity, agy)) = tabsession::agent_for(pid)
            {
                if crate::presence_fd::conversation_of_pid(agy).is_some() {
                    appeared.store(true, Ordering::Release);
                    ping();
                }
                drop(inflight);
                return;
            }
            let hint = tabsession::hint_for(pid);
            // A `claude` exists under this tab now. Reported unconditionally — the event loop
            // decides whether it cares, and it only cares while unarmed.
            if hint.is_some() {
                appeared.store(true, Ordering::Release);
                ping();
            }
            let set = rival_set_of(hint);
            if let Ok(mut g) = handle.lock() {
                *g = set;
            }
            drop(inflight);
        });
    }

    /// Hide + stop the watcher (D6).
    fn close(&mut self) {
        self.visible = false;
        self.watcher = None;
        self.refresh_model();
    }

    /// Push the current visibility + composed model into the draw layer.
    ///
    /// Composition policy lives in [`crate::dashboard`] and pixel layout in
    /// `renderer::monitor_layout` (F5) — this is wiring: supply the impure inputs (clock,
    /// UTC offset, config, process-tree answer), hand over the snapshot, publish the
    /// result. `visible` is stamped here rather than in the composer because whether the
    /// user is looking is this file's fact, not the snapshot's.
    /// Take the anchor from wherever the view now sits (D8/D37).
    ///
    /// The mode is **derived from the index, never latched**: at 0 there is nothing above to
    /// miss, so nothing is held. Latching it instead would leave an armed anchor sitting at
    /// index 0 whenever the document shrank under an anchored view — and then a new entry
    /// arriving would push the reader down from a position they never chose.
    fn rearm_anchor(&mut self) {
        let top = self.handle.scroll().top;
        self.anchor = match self.keys.get(top) {
            Some(key) if top > 0 => Anchor::Held {
                key: key.clone(),
                index: top,
            },
            _ => Anchor::Following,
        };
    }

    /// Put section `n`'s heading in the sticky slot (D16/R8.1/R8.2).
    ///
    /// The target is the heading's **own** ladder index, not the rung after it: D31 has the
    /// slot consume a heading, so landing on `index + 1` would show the same frame while
    /// leaving the scroll state one rung from where every other path puts it.
    ///
    /// `Cmd+Opt+1` doubles as "back to the top" (D30) with no new binding, because TASKS is
    /// the first section — and reaching index 0 clears the badge under D33's one rule.
    fn jump_to(&mut self, section: usize) {
        let Some(target) = self.handle.jump_target(section) else {
            return; // no document laid out yet
        };
        let badge = if target == 0 {
            0
        } else {
            self.handle.scroll().badge
        };
        self.handle.set_scroll(ScrollState { top: target, badge });
        self.wheel_residue = 0.0;
        self.rearm_anchor();
    }

    /// Flip section `n`'s fold and rebuild the document around it.
    ///
    /// The anchor is re-armed from where the view ends up rather than carried: folding a
    /// section **above** the reader removes rungs from under them, and `resolve_anchor`'s
    /// content-hash walk already lands them on the nearest survivor — but the rung they
    /// end on is a new one, and D37 says the mode is a function of that index.
    fn toggle_fold(&mut self, n: usize) {
        if self.folds.len() <= n {
            self.folds.resize(n + 1, true);
        }
        self.folds[n] = !self.folds[n];
        self.wheel_residue = 0.0; // a bank measured against a document that no longer exists
        self.refresh_model();
        self.rearm_anchor();
    }

    /// Back to the top, holding nothing (D25/R11).
    ///
    /// Opening the panel re-resolves the log, respawns the watcher and re-runs the backfill,
    /// so the document is new anyway — carrying a scroll position into it would land the view
    /// by way of D15's survivor walk at a rung the reader never chose. And the reason to open
    /// the panel is usually "what is happening now", which is at the top.
    fn reset_scroll(&mut self) {
        self.anchor = Anchor::Following;
        self.wheel_residue = 0.0;
        self.keys.clear();
        self.handle.set_scroll(ScrollState { top: 0, badge: 0 });
    }

    /// Compose the model, and publish it **only if it differs** from the last one published.
    ///
    /// Returns whether it differed — which is what the `OverlayPending` arm redraws on (D2).
    ///
    /// **The diff runs first, and everything below it runs only on a change (D9).** Before
    /// this feature the ladder, the rung keys, the anchor walk and the badge count ran on
    /// every call, because every call came from a watcher update that had carried content.
    /// D2 now calls this on wakes that carried nothing, so that work would run to produce a
    /// computed nothing several times a second. Gating it on the model comparison puts the
    /// cost where the result is: a no-change wake costs one `dashboard::model` and one
    /// `PartialEq`, and a real change costs what it always did.
    ///
    /// Correctness does not depend on the gate — an equal model yields equal keys, hence an
    /// unmoved anchor and a zero badge — so this is purely about not paying for nothing. What
    /// the gate does *not* excuse is the unequal branch, which D13 says is the common one on a
    /// young session: see `r6_*`, which pin that a rebuild whose only delta is a ticking tile
    /// still leaves the badge and a held anchor where they were.
    /// Decide whether the panel wants a one-shot wake, given the pill `status` it is about to
    /// show (D3/D11/D14/D18).
    ///
    /// **The pill is the authority, which is what collapses D14's four rules into one test.**
    /// A deadline exists only to correct a CONNECTED that has gone stale, so any other status
    /// means there is nothing to correct and the slot is cleared. That covers all four edges at
    /// once, and none of them needed its own branch: a past-due session already reads IDLE; a
    /// `last_event_ms` of `None` already reads IDLE; a detached snapshot already reads
    /// NO SESSION; and `Some(a) → Some(b)` with `b <= a` leaves a CONNECTED pill whose
    /// `armed_from` is not older, so nothing re-arms and the pending deadline is left alone.
    ///
    /// **`Busy` defers rather than disarms.** A tab mid-turn is silent for a reason the log
    /// cannot show — the `tool_result` has not been written yet — and that is exactly the
    /// scenario the five-minute threshold was sized for, so arming through it would
    /// manufacture a false IDLE for the rest of a long call. Returning early leaves
    /// `armed_from` untouched, so the arm is *deferred*, and the first non-`Busy` rebuild
    /// performs it. The cost of this gate is recorded rather than hidden: the `Busy` signals
    /// do not decay, so a turn interrupted before its `end_turn` is written leaves the tab
    /// `Busy` indefinitely and this pill stays CONNECTED — which is today's behaviour,
    /// unchanged, and a better trade than introducing a false IDLE into a working session.
    fn reconsider_deadline(&mut self, status: renderer::monitor_model::PanelStatus) {
        use renderer::monitor_model::PanelStatus;
        let last = self.session.latest.as_ref().and_then(|t| t.last_event_ms);
        // Nothing costs anything while the panel is closed — including this.
        let (Some(last), true, PanelStatus::Connected) = (last, self.visible, status) else {
            self.disarm();
            return;
        };
        if self.agent_state == Some(crate::tabmeta::AgentState::Busy) {
            return; // deferred, not dropped — see above
        }
        if self.armed_from.is_some_and(|from| last <= from) {
            return; // already armed from this timestamp or a newer one
        }
        self.armed_from = Some(last);
        // Never `last + IDLE_AFTER_MS`: the threshold comparison is `<=`, so that instant is
        // one millisecond before the pill actually flips. `dashboard` owns the rule and hands
        // back the instant, so the overlay never learns the threshold at all.
        (self.arm_deadline)(Some(dashboard::idle_deadline_ms(last)));
    }

    /// Clear the slot, telling the worker only if there was something to clear.
    ///
    /// Idempotent on purpose: a detached or already-idle panel passes through
    /// [`Self::reconsider_deadline`] on every wake, and an unconditional send would put one
    /// `Cmd` per wake on the channel to repeat a fact the worker already holds.
    fn disarm(&mut self) {
        if self.armed_from.take().is_some() {
            (self.arm_deadline)(None);
        }
    }

    fn refresh_model(&mut self) -> bool {
        #[cfg(test)]
        {
            self.models_built += 1;
        }
        let env = dashboard::Env {
            now_ms: now_ms(),
            attached: self.session.attach.attached,
            context_window: self.context_window,
            tab_agent: self.tab_agent,
            utc_offset_secs: local_utc_offset_secs(),
        };
        let mut model = dashboard::model(self.session.latest.as_deref(), &env);
        model.visible = self.visible;
        // Stamp the user's fold decisions back on. A section long enough to fold starts
        // **shut**: the panel's job is to stay readable at a glance, and RECENT CALLS alone
        // can run to dozens of rows. Opening one is a click; being unable to see the other
        // five sections without scrolling past those dozens is not.
        // `!=`, not `<`, and the difference is the one `CLAUDE.md` names: the condition now
        // says what it means, which is "the fold vector is per-section, so it tracks the
        // section count". Written as `<` it left three mutants alive that no test could tell
        // apart from the original, because two of them differ only in a case where `resize` is
        // a no-op — the tell that the comparison was saying less than the code intended.
        if self.folds.len() != model.sections.len() {
            self.folds.resize(model.sections.len(), true);
        }
        for (s, &shut) in model.sections.iter_mut().zip(self.folds.iter()) {
            s.collapsed = shut;
        }

        // The deadline is reconsidered ABOVE D9's gate, and that placement is load-bearing.
        // An unchanged model does **not** imply an unchanged `last_event_ms`: the two places
        // the timestamp reaches the screen are `clock(last_event_ms)`, which is
        // second-granular, and `dur(now - last_event_ms)`, which is second-granular below an
        // hour and minute-granular above it — so a burst that moves the timestamp by a few
        // hundred milliseconds can leave every formatted string identical. Below the gate,
        // such a burst would arm from the previous timestamp, fire early while the pill still
        // read CONNECTED, and then re-arm nothing, which is the same dead-pill failure the
        // `idle_deadline_ms` boundary exists to prevent, reached by a different road.
        self.reconsider_deadline(model.status);

        // D9 — the gate, and it sits HERE for a reason: below the two stamps and above every
        // consumer of the document. `visible` and the folds are part of what the reader sees,
        // so a model compared before them would report "unchanged" for a panel that had just
        // closed or a section that had just folded. Everything after this line is a function
        // of the document, so none of it can differ while the document does not.
        if self.published.as_ref() == Some(&model) {
            return false;
        }

        // D8/D19/D37 — carry the reading position across the swap.
        //
        // The ladder comes from the renderer's own function rather than a second enumeration
        // here (C11): two counts that drift apart put the view a rung from where the user
        // asked, and they drift silently. Resolved BEFORE the model is published so the
        // panel never draws a frame at an index belonging to the previous document.
        //
        // The counter is bound to the CALL, not to a line after the gate. Written as a bare
        // statement below the early return it measured position rather than work: moving
        // `item_ladder` above the gate — the exact regression D9 exists to prevent — would
        // leave the count unchanged and `r1_2` still green. Inside the initialiser it travels
        // with the call it stands for.
        let ladder = {
            #[cfg(test)]
            {
                self.ladders_built += 1;
            }
            renderer::monitor_layout::item_ladder(&model)
        };
        let keys: Vec<RungKey> = ladder.iter().map(|i| rung_key(&model, i)).collect();
        let top = resolve_anchor(&self.anchor, &self.keys, &keys);
        // Re-arm from the new document: the key is unchanged but its index moved, and D15's
        // outward walk starts from an index, so a stale one would search from the wrong place.
        self.anchor = match &self.anchor {
            Anchor::Following => Anchor::Following,
            Anchor::Held { .. } => match keys.get(top) {
                Some(key) if top > 0 => Anchor::Held {
                    key: key.clone(),
                    index: top,
                },
                // The anchor collapsed back to the top — nothing above it survived. That is
                // `Following` again, by D37's rule that the mode is a function of the index.
                _ => Anchor::Following,
            },
        };
        // D11/D33 — how much has piled up above the reader while they were away.
        let badge = count_new_above(&self.keys, &keys, top);
        self.keys = keys;
        self.published = Some(model.clone());
        self.handle.set(model);
        self.handle.set_scroll(ScrollState { top, badge });
        true
    }
}

/// The re-point bookkeeping for the ai-monitor panel: what it was last opened *for*, and
/// whether a new notification is a real change (D6/R4.3).
///
/// **Why this is a struct and not two fields plus a predicate.** The storm bug was never in
/// the comparison — it was in *which value the call site stored*. `open` has two cwds in hand:
/// the one it was **notified** with (`Tabs::active_cwd`, which falls back to the tab's own
/// shell — under tmux, the client's) and the one it **resolved** (`pane_cwd`, the tmux pane's).
/// Storing the resolved one and comparing it against incoming notifications meant the two
/// disagreed by construction for the whole window before the worker's first publish, so the
/// guard fired on every wake and never converged — watcher dropped and respawned, full backfill
/// re-run, and two 250 ms UI-thread `tmux list-clients` calls each time. The overlay's own
/// watcher emits `OverlayPending`, so the storm fed itself.
///
/// A free predicate could not catch that: a test would hand it literals and pass either way
/// (verified — the earlier test passed against the buggy call site). Making [`opened`] own the
/// choice puts it under test, because a test drives the *transition* and never types the key.
///
/// [`opened`]: Repoint::opened
#[derive(Default)]
struct Repoint {
    /// The cwd this panel was last **notified** with — never the resolved one.
    key_cwd: Option<PathBuf>,
    key_pid: Option<u32>,
}

impl Repoint {
    /// Record one `open` and return the directory it should actually open on.
    ///
    /// `notified` becomes the key; `resolve()` (the tmux pane's cwd, when one is found) is
    /// preferred for the directory, falling back to `notified` (R3.3). Returning the choice
    /// rather than storing it keeps this the single place the two are told apart.
    ///
    /// **There is exactly one `Option<PathBuf>` parameter, so there is nothing to swap it
    /// with.** That is deliberate and was arrived at the hard way. With `notified` and
    /// `resolved` both plain options, transposing them at the call site compiled and left all
    /// tests green while reintroducing *both* original bugs at once — the key became the
    /// resolved cwd (the re-open storm) and the panel opened on the client's directory (the
    /// originally reported symptom). An intermediate form took the resolver as
    /// `impl FnOnce() -> Option<PathBuf>`; that was **not** sufficient either, because a swap
    /// wrapped in `||` still type-checks. Passing the pid *into* the resolver removes the
    /// second candidate entirely, so reintroducing the bug now requires writing a visibly wrong
    /// `|_| cwd` rather than transposing two arguments.
    ///
    /// **The hint now travels through the resolver, and that is a strengthening rather than a
    /// concession.** Deriving the directory from the chosen `claude` puts a second candidate
    /// (`hint.cwd`) back within reach of this call, so the "nothing to swap" property had to be
    /// re-earned rather than assumed. It is re-earned twice over: `Option<PathBuf>` and
    /// `Option<&SessionHint>` are not interchangeable types, so the transposition is no longer
    /// *expressible*; and the resolver is passed **by name** ([`hint_or_pane_cwd`]), never as a
    /// closure, so there is no capture scope for `|_, _| cwd` to reach a second candidate from.
    /// The spelling an implementer reaches for first — keeping the arity-1 resolver and writing
    /// `|pid| hint_or_pane_cwd(h.as_ref(), pid)` — compiles silently and is exactly the
    /// captured-closure state this guard depends on not existing. It is rejected knowingly.
    ///
    /// A unit test cannot cover this: the hazard lives at the **`open()` call site**, and such a
    /// test types its own arguments — the note above records an earlier version of these tests
    /// doing exactly that and passing against the buggy call site. The backstop is a
    /// source-text pin over `open()`, in the idiom used for the other pins in this file.
    fn opened(
        &mut self,
        notified: Option<PathBuf>,
        shell_pid: Option<u32>,
        hint: Option<&tabsession::SessionHint>,
        resolve: impl FnOnce(Option<&tabsession::SessionHint>, Option<u32>) -> Option<PathBuf>,
    ) -> Option<PathBuf> {
        self.key_cwd = notified.clone();
        self.key_pid = shell_pid;
        resolve(hint, shell_pid).or(notified)
    }

    /// Whether an incoming notification is a real change worth re-opening for.
    ///
    /// Re-point only while open, and only if the tab actually changed. Keyed on the shell pid
    /// as well as the cwd: two tabs in the SAME directory are different sessions, which is what
    /// tab binding exists for. Returning `true` is **not** cheap — it runs `open`, which drops
    /// and respawns the watcher, re-runs the full backfill, and performs two 250 ms-budgeted
    /// `tmux list-clients` calls on the event-loop thread. Since D6 fires this after *every*
    /// metadata sync, the common case must be inert.
    fn changed(
        &self,
        visible: bool,
        cwd: Option<&std::path::Path>,
        shell_pid: Option<u32>,
    ) -> bool {
        visible && (self.key_cwd.as_deref(), self.key_pid) != (cwd, shell_pid)
    }
}

/// The directory this open should watch: the chosen `claude`'s own, falling back to the tmux
/// pane's only when the hint supplied none at all.
///
/// **A named free function, and the name is the guard.** It is passed to [`Repoint::opened`] by
/// name rather than wrapped in a closure, so it has no capture scope — which is the property
/// `opened`'s doc says the anti-transposition guard actually depends on. A closure here, however
/// innocuous, makes `|_, _| <some other cwd>` a legitimate spelling again and puts a
/// one-token, type-checking re-introduction of the storm back on the table.
///
/// **The card and this panel may therefore name different directories, and that is correct
/// rather than a defect.** After a `cd` in a tmux pane the sidebar card shows the pane shell's
/// directory while the panel watches where `claude` runs. They answer different questions —
/// "where is this tab working" versus "where is this session's log" — and forcing agreement
/// would require one of them to report something untrue. Recorded here, and in
/// `docs/architecture.md`, so the next reader does not file it as a bug.
///
/// **Both `HintCwd` variants supply the directory; only `None` falls back.** `Disputed` carries
/// the *registry's* path — `claude`'s own statement about where it is running — and the panel
/// opens on it, with tier 2 suppressed back in `open()`. Sweeping `Disputed` into the `pane_cwd`
/// fallback instead would open on a **third** directory, the pane *shell's*: neither the
/// kernel's nor the registry's, so tier 1's exact-filename lookup would miss, the whole reason
/// for preferring the registry on a disagreement would be nullified, and `Disputed`'s payload
/// would be read by nobody. An earlier draft did precisely that.
fn hint_or_pane_cwd(
    hint: Option<&tabsession::SessionHint>,
    shell_pid: Option<u32>,
) -> Option<PathBuf> {
    hint_or_pane_cwd_with(AiMonitorOverlay::pane_cwd, hint, shell_pid)
}

/// [`hint_or_pane_cwd`]'s body, with the fallback injected — the same split `pane_cwd_with` and
/// `resolved_root_with` already use, and here it is what makes the *non*-occurrence assertable:
/// on a tmux tab that has a session the pane resolver must not run at all, which is how the open
/// path went from two 250 ms-budgeted `tmux list-clients` calls on the winit event-loop thread
/// to one (`hint_for` performs the surviving one internally).
fn hint_or_pane_cwd_with(
    pane: impl FnOnce(Option<u32>) -> Option<PathBuf>,
    hint: Option<&tabsession::SessionHint>,
    shell_pid: Option<u32>,
) -> Option<PathBuf> {
    match hint.and_then(|h| h.cwd.as_ref()) {
        Some(cwd) => Some(cwd.path().to_path_buf()),
        // No `claude`, or neither the kernel nor its own record could say where it is. The
        // pane's cwd is a guess about the right project rather than a claim about this process,
        // which is why `open()` runs tier 1 only against it.
        None => pane(shell_pid),
    }
}

/// [`AiMonitorOverlay::pane_cwd`]'s body, generic over the two syscall-backed steps so the
/// re-root decision is testable on a machine with no tmux (tmux-pane-cwd R10.3) — the same
/// split `tabsession::resolved_root_with` uses.
fn pane_cwd_with(
    resolve: impl FnOnce(u32) -> u32,
    cwd_of: impl FnOnce(u32) -> Option<PathBuf>,
    shell_pid: Option<u32>,
) -> Option<PathBuf> {
    let shell_pid = shell_pid?;
    let root = resolve(shell_pid);
    // `resolved_root` returns the input pid for "no tmux here" *and* for every failure (its
    // `-> u32` is deliberately not an `Option`, so the two are indistinguishable). Either way
    // there is nothing to re-root to, so yield `None` and let the caller keep the cwd it was
    // passed — which for a non-tmux tab is already the right answer.
    (root != shell_pid).then(|| cwd_of(root))?
}

/// The rival set a hint publishes into the shared handle.
///
/// Split out of `refresh_rivals`' worker closure so the recovery this feature delivers is
/// **assertable**. `refresh_rivals` is the second `hint_for` caller, and the deleted cwd gate
/// degraded it silently: under tmux the gate made `hint_for` answer `None` on every refresh, so
/// this published `RivalSet::default()` for the life of the panel — `known: false`, the
/// uniqueness rescue permanently withheld, and `claude_appeared` never set. It was a second
/// victim of the gate rather than a constraint on removing it, and nothing could see that,
/// because the mapping lived inside a spawned closure no test can reach.
///
/// `None` still means `known: false`, and that direction must not drift: failing toward "we do
/// not know" is the whole point of the flag, since the alternative grants the resolver's
/// strongest branch on the strength of a read that did not work.
fn rival_set_of(hint: Option<tabsession::SessionHint>) -> ai_monitor::RivalSet {
    match hint {
        Some(h) => ai_monitor::RivalSet {
            list: h.rival_starts,
            known: h.rivals_known,
        },
        None => ai_monitor::RivalSet::default(),
    }
}

/// Clears the in-flight flag on drop, so a panicking refresh worker cannot latch it.
struct InFlight(Arc<AtomicBool>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// The pid of the tab's `agy`, when this open takes the Antigravity path (T4).
///
/// A named function rather than an inline pattern because of what it must NOT do.
/// [`crate::tabmeta::AgentKind`] has five variants and this feature implements exactly one of
/// them; the other four — and a tab running no agent at all — take the Claude path, which is
/// correct only because that path degrades to "resolve nothing, show an explained empty
/// state" rather than to something wrong. The day `Codex` or `Gemini` grows a real
/// implementation, this is the single place that has to learn about it, and the test below
/// fails until it does. An inline `if let` would just silently keep routing it to Claude.
fn antigravity_pid(agent: Option<(crate::tabmeta::AgentKind, u32)>) -> Option<u32> {
    match agent {
        Some((crate::tabmeta::AgentKind::Antigravity, pid)) => Some(pid),
        _ => None,
    }
}

/// Build the [`Watch`] whose rival handle is *the one stored in `slot`*.
///
/// The pairing lives in one function so a test can assert it (`Arc::ptr_eq`) without an
/// `EventLoopProxy`. This is the seam that actually catches the iteration-2 defect: pinning
/// only `publish_rivals_into` left `open()` free to discard its return and hand `Watch` a
/// fresh `Arc` — the identical bug, with no test and no lint failing. Verified by mutation.
fn watch_with_rivals(
    rivals: ai_monitor::RivalHandle,
    dir: PathBuf,
    log: Option<PathBuf>,
    want: Wanted,
    owned: bool,
    attached: Arc<AtomicBool>,
) -> Watch {
    Watch {
        dir,
        log,
        owned,
        attached,
        scope: ai_monitor::WatchScope::Project { want, rivals },
    }
}

/// Install `hint`'s rivals into `slot` and return the clone for [`ai_monitor::Watch`].
///
/// Free-standing, and taking the slot by `&mut`, for one reason: the overlay needs an
/// `EventLoopProxy` to construct, so a method could only ever be verified by re-implementing
/// it in the test — which is exactly how the original miswiring survived. Here a test drives
/// the real code.
fn publish_rivals_into(
    slot: &mut ai_monitor::RivalHandle,
    hint: &tabsession::SessionHint,
) -> ai_monitor::RivalHandle {
    *slot = Arc::new(Mutex::new(ai_monitor::RivalSet {
        list: hint.rival_starts.clone(),
        known: hint.rivals_known,
    }));
    slot.clone()
}

/// Wall-clock millis since the epoch, saturating.
///
/// Both failure modes are impossible-in-practice but neither may `unwrap`: a clock
/// set before 1970 yields `0`, and a `u128` past `u64::MAX` millis (year 584942417)
/// clamps. Either way the dashboard reads a `now_ms` at or below `first_event_ms`,
/// which saturating subtraction renders as `0s` rather than a wrapped duration.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Seconds to add to a UTC epoch to get local wall-clock time, from `NSTimeZone` (D7/D19).
///
/// The original dashboard chose a relative age (`12s ago`) because `std` alone cannot
/// resolve a local timezone and a UTC clock on a local-session dashboard is worse than no
/// clock. That reasoning assumed `std` was the only option: this is a macOS-only app that
/// already depends on `objc2` for platform bindings, so the offset is available for the
/// asking and the constraint is removed rather than worked around.
///
/// Resolved **here**, per model rebuild, and passed into the pure composer — never called
/// from `dashboard.rs`, which would make its output machine- and season-dependent and
/// flake the snapshots that pin composition policy (D19).
///
/// Two accepted limits, both recorded as Known Gaps: it is the offset *now* applied to
/// every timestamp on the frame (a session spanning a DST transition renders its earlier
/// rows an hour off), and a timezone change mid-session is not observed until the next
/// rebuild. `secondsFromGMT` is an `NSInteger`; a value outside `i32` is not a real
/// timezone, so it falls back to UTC rather than wrapping into a plausible-looking one.
fn local_utc_offset_secs() -> i32 {
    i32::try_from(objc2_foundation::NSTimeZone::localTimeZone().secondsFromGMT()).unwrap_or(0)
}

/// The one tab session the panel is showing (D13/R10.1).
///
/// **Why this is a struct rather than three fields on the overlay.** The panel is per
/// *window* and re-points to whichever tab is active, so every field in here has the
/// lifetime of one attachment and none of them may survive a tab switch. As loose fields
/// that rule was three separate assignments in the middle of `open()`, and nothing could
/// test it: `AiMonitorOverlay` needs an `EventLoopProxy`, which needs a real winit
/// `EventLoop` (main-thread-only, once per process on macOS), so a test could only
/// re-implement the reset in its own body — which is exactly the shape this file has been
/// burned by twice before (see [`Attachment`] and [`Repoint`]). One value with one
/// [`repoint`](Session::repoint) puts the reset itself under test.
///
/// **What is deliberately NOT in here** is as load-bearing as what is:
/// - the panel's **width** (R10.4). It is a window property, and this file holds no width
///   at all: the chord records `AppCtx::set_monitor_open` → `AppRequest::SetMonitorOpen`,
///   a bare `bool`, and the width is derived one layer down from `[ai_monitor].width_px`
///   plus the runtime override. There is therefore no per-tab width to carry across a
///   switch, and no value a switch could carry it *in* — so a switch cannot move the band
///   and cannot refit the grid.
/// - `started_ms`, which is when *this window* opened.
///
/// The cumulative counters D3 added (`total_tool_calls`, the per-name tally,
/// `failed_tool_calls`) live inside the snapshot held here, so they are re-pointed by the
/// same act that re-points everything else — tab A's 200 `bash` calls leave with tab A's
/// `Session`, and tab B's are folded from scratch by [`backfill_from_scratch`] (R10.3).
#[derive(Default)]
struct Session {
    /// The newest snapshot the watcher folded. `None` means exactly one thing: no
    /// log was resolved for this tab. Because `open()` seeds it from the backfill
    /// before spawning, it is `Some` immediately after a successful open — never
    /// "opened but nothing folded yet".
    latest: Option<Arc<TelemetrySnapshot>>,
    /// Which session is being shown, and whether one has been settled on at all.
    /// Cleared only by [`repoint`](Session::repoint), i.e. only by `open()`.
    ///
    /// Deciding this once, at attach, is the whole point. As a per-frame predicate it
    /// re-litigates an attachment already on screen and erases a live panel the
    /// moment the held snapshot ages past the window start.
    attach: Attachment,
    /// The same answer, published to the watch thread, which reads it to decide
    /// whether to keep re-resolving — so the rule has exactly one author. When the
    /// watcher kept its own copy the two drifted, and the common case (work for a
    /// while, then open the panel) left the app attached while the watcher still
    /// believed it was free to switch logs and reset the accumulator underneath it.
    attached_flag: Arc<AtomicBool>,
}

impl Session {
    /// Drop the previous tab's session entirely (R10.3).
    ///
    /// A whole-value overwrite, not a field-by-field clear, so a field added later is
    /// re-pointed by default instead of leaking by default — the failure mode here is
    /// silent and looks like data: tab A's tally rendered under tab B's heading.
    ///
    /// The `attached_flag` is replaced with a **fresh allocation** rather than stored
    /// `false`: the previous watcher may still be running for an instant, and it must not
    /// observe this open's decision — nor may this open's decision be undone by a
    /// callback still in flight for the old tab.
    fn repoint(&mut self) {
        *self = Session::default();
    }

    /// Record a snapshot and, once, the fact that it proved ownership.
    ///
    /// `attached` only ever goes false -> true within an open. `last_event_ms` is a
    /// last-seen value, not a maximum, so a single out-of-order timestamp would
    /// otherwise revoke an attachment mid-session and blank the panel.
    fn note(&mut self, snap: Arc<TelemetrySnapshot>, started_ms: u64, owned: bool) {
        if !self.attach.admit(&snap, started_ms, owned) {
            return;
        }
        self.attached_flag
            .store(self.attach.attached, Ordering::Relaxed);
        self.latest = Some(snap);
    }
}

/// Fold this tab's log into a **fresh** accumulator, and report where the tail resumes.
///
/// The pair is one contract, which is why it is produced in one place. The accumulator is
/// new every time — the cumulative counters D3 added are session facts, so a re-point must
/// rebuild them from the new tab's log rather than continue the old tab's totals (R10.3).
/// The `u64` is the byte count *already folded into it*, which `LogWatcher::spawn` takes as
/// its start offset; handing the watcher a fresh accumulator with offset `0` would re-read
/// the whole file and fold every line a second time. That failure is invisible in the
/// rings — they are capped, so they would still look right — while `total_tool_calls` read
/// 400 for 200 calls and every usage bar kept its proportions.
///
/// A tab with no resolvable log yields an empty accumulator, never the previous tab's.
fn backfill_from_scratch(log: Option<&std::path::Path>) -> (SessionTelemetry, u64) {
    match log {
        Some(p) => backfill_telemetry(p),
        None => (SessionTelemetry::new(), 0),
    }
}

/// Everything the overlay knows about *which* session it is showing.
///
/// One struct so the whole rule is in one place and directly unit-testable: the
/// previous shape put half of it in a method on `AiMonitorOverlay`, which needs an
/// `EventLoopProxy` and so cannot be constructed under `cargo test` — the test that
/// was supposed to guard it ended up re-implementing the rule in its own body and
/// asserting that, which proves nothing about the code.
#[derive(Debug, Default)]
struct Attachment {
    /// Set once a log proves it is this window's session. Never cleared: a
    /// `last_event_ms` that moves backward must not revoke it.
    attached: bool,
    /// The session we attached to, when it named itself.
    session_id: Option<String>,
    /// Whether the attached log was *confirmed* to be this tab's, by the process
    /// tree. An unconfirmed attachment is the best guess available at the time and
    /// yields to a confirmed one; a confirmed attachment yields to nothing.
    owned: bool,
}

impl Attachment {
    /// Does this snapshot replace what is on screen? Updates the attachment.
    fn admit(&mut self, snap: &TelemetrySnapshot, started_ms: u64, owned: bool) -> bool {
        if !self.attached {
            // A CONFIRMED resolution attaches on CONTENT, not on freshness.
            // `proves_ownership` exists to keep a *foreign* session off the panel;
            // once resolution is confirmation-only a foreign session can no longer be
            // resolved at all, so applying a freshness gate to a confirmed one only
            // hides the user's own data — a tab running `claude --resume <id>` that
            // went idle before the panel opened would be told `(no active session —
            // newest log idle 12m)` about its own session.
            //
            // **The emptiness condition is not incidental — it is what preserves
            // self-correction.** `proves_ownership` was quietly doing a second job:
            // it is the gate that fails OPEN. `note()` runs in `open()` *before*
            // `LogWatcher::spawn`, and it publishes this decision to the flag the
            // watch thread reads to decide whether to keep re-resolving. A bare
            // `owned` short-circuit would attach on an owned-but-EMPTY backfill
            // snapshot, so the watcher would be born frozen and any near-miss by the
            // resolver's dating tier would be permanent from its first callback.
            // Attaching empty also latches `session_id = None`, after which the
            // identity guard below hits its `_ => true` arm until a snapshot names one
            // (see the adoption arm) — satisfying "a different session never replaces
            // an owned attachment" *vacuously* for that whole window, which is a
            // weakening of that contract dressed up as compliance. Requiring content
            // leaves a confirmed-but-empty log unattached, the shared flag unset and
            // the watcher free; the first appended line re-emits and attaches then.
            //
            // The `proves_ownership` fallback below is entered whenever the
            // confirmed-and-non-empty test fails — including with `owned: true` and an
            // empty snapshot, where it answers `false` because a default snapshot has
            // no `last_event_ms`. What production can no longer produce is an ATTACH
            // through it: `open()` resolves via `resolve_owned_session`, every result
            // of which is `owned: true`, so the only case reaching here is that empty
            // one. It is retained deliberately — it is the attach contract for a caller
            // using the ranked `resolve_session` fallback, and it is what the
            // `owned: false` tests in this module exercise.
            if (owned && *snap != TelemetrySnapshot::default())
                || proves_ownership(snap, started_ms)
            {
                self.attached = true;
                self.session_id = snap.session_id.clone();
                self.owned = owned;
            }
            return true;
        }
        // A CONFIRMED session displaces an unconfirmed one. Without this the panel
        // could lock onto another application's session — one running in the same
        // project directory, legitimately active — and never let go, which is how a
        // brand-new tab came to show `up 116h40m` from a session in Warp.
        //
        // PRODUCTION-UNREACHABLE, honestly: strict resolution leaves no unconfirmed
        // attachment to displace, so `owned && !self.owned` can never fire from the
        // overlay's own call sites. Retained as the contract for a caller using
        // `resolve_session` — deleting it would take the `owned: false` half of the
        // rule out of the suite in the same move that makes the code depend on
        // nothing else.
        if owned && !self.owned {
            self.session_id = snap.session_id.clone();
            self.owned = true;
            return true;
        }
        // An EMPTY snapshot never erases real data. The watcher resets its
        // accumulator when a log is truncated under it and emits the empty result;
        // accepting that would blank a panel the user is reading, which is the one
        // thing this feature must not do. The next append re-sends.
        if *snap == TelemetrySnapshot::default() {
            return false;
        }
        // Neither does a DIFFERENT session. Telling the watcher to stop re-resolving
        // is not sufficient on its own: it reads that flag at the top of its
        // callback, so a callback already in flight can finish a switch after we
        // attach. That is a check-then-act window no memory ordering closes — the
        // reader has to be able to reject the result, not merely to decline the
        // switch.
        //
        // ADOPTION closes what non-emptiness alone cannot: `session_id` is filled only
        // from a `sessionId`/`session_id` field on a folded line, while non-emptiness
        // can come from any field at all (`total_output`, a tool count). A confirmed
        // log whose backfilled content names no session therefore attaches holding
        // `None`, and every comparison below would fall through `_ => true` forever.
        // So when the attachment holds no identity and a snapshot offers one, record
        // it: from that point the check compares two `Some`s and means something. It
        // adopts an identity it did not have and NEVER changes one it holds — a
        // snapshot whose id differs from an already-recorded `Some` is still refused
        // by the arm above.
        match (&self.session_id, &snap.session_id) {
            (Some(mine), Some(theirs)) => mine == theirs,
            (None, Some(theirs)) => {
                self.session_id = Some(theirs.clone());
                true
            }
            _ => true,
        }
    }
}

/// What a metadata tick should do to the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tick {
    /// Re-point: the tab moved, or its session was rotated under us. Both go through
    /// `open()`, which is the whole re-point — see the `on_active_changed` call site.
    Reopen,
    /// Same tab, same session: republish rival membership and nothing else (D11/R6.2).
    RefreshRivals,
    /// The panel is closed. Nothing is on screen to be wrong.
    Idle,
}

/// Which of the three a tick is, **and in which order the questions are asked.**
///
/// Split out for the reason every other decision in this file is: the ordering is the part a
/// wrong edit breaks, and it is invisible to a unit test that only drives
/// [`session_rotated_with`]. Three properties are pinned here that the rule alone cannot
/// pin — a tab change outranks a rotation, a closed panel decides nothing, and a rotation
/// outranks the rival refresh.
///
/// `rotated` arrives as a **thunk, and the laziness is the contract, not an optimisation**:
/// it reads a file, and this runs on the winit event-loop thread after every metadata sync.
/// It must not be called when the answer cannot matter — when the tab changed (we are
/// re-opening regardless, and `open()` re-reads everything anyway) or when the panel is
/// closed (there is nothing on screen to correct). Passing it lazily is what lets a test
/// prove those reads do not happen, with a thunk that panics if it is touched.
fn tick_action(
    tab_changed: bool,
    visible: bool,
    agent_moved: bool,
    rotated: impl FnOnce() -> bool,
) -> Tick {
    if tab_changed {
        return Tick::Reopen;
    }
    if !visible {
        return Tick::Idle;
    }
    // A DIFFERENT `claude` in the same tab. The panel's directory is that process's cwd, fixed
    // at its launch, so it goes stale exactly when the pid changes — and the panel stops
    // re-resolving once attached, so a user who quits `claude`, `cd`s elsewhere in the same
    // pane and starts a new one would otherwise keep seeing the old session indefinitely.
    // `apply_rotation` already handles "same pid, new session id" (`/clear`); this is the same
    // question one level up.
    //
    // **Its rank is stated rather than left to a reader.** Below `tab_changed`, because a tab
    // change re-opens unconditionally and re-reads everything anyway. Below `!visible`, because
    // a closed panel must decide nothing: above that gate a pid change would return
    // `Tick::Reopen` for a CLOSED panel, and `open()` sets `self.visible = true` without the
    // `ctx.set_monitor_open` only the chord path performs — so the panel would paint into a
    // band the renderer never reserved. Above `rotated()`, because a changed pid makes the
    // rotation question moot: it concerns a process that is no longer the one being tracked,
    // and asking costs a file read.
    if agent_moved {
        return Tick::Reopen;
    }
    if rotated() {
        return Tick::Reopen;
    }
    Tick::RefreshRivals
}

/// Has the tab's `claude` been **replaced** since the last value we were told about?
///
/// **`Some`-guarded, not a bare `published != last_seen`, and the difference is a storm.** Both
/// are `Option<u32>`, so `None != Some(x)` is `true`: an absent published value would read as
/// "a different pid" and re-point on every wake. And absent is not exotic — it is what the
/// worker publishes for a tab whose agent walk failed transiently, and what the two derivations
/// legitimately disagree about (the panel's `hint_for` walks `ps` argv to depth 6 pane-first;
/// the worker walks `proc_pidpath` to depth 3 with a different fallback trigger, so a
/// `node …/cli.js` install or a pane-less-but-tab-ful tree yields one answer from one and
/// another from the other). A `None` publish therefore means **no information**: it re-points
/// nothing and, at the call site, it does not overwrite `last_seen_pid` either — so a toggle
/// flip or one failed walk cannot erase the trigger's memory.
///
/// **Edge-triggered against `last_seen_pid`, not level-triggered against `opened_on.pid`.** A
/// stable disagreement between the two derivations then costs exactly one re-point rather than
/// one per wake. `opened_on.pid` would be the wrong comparand twice over: it changes only on a
/// re-open, and it is `None` for a perfectly live session whose start time could not be read.
fn agent_pid_moved(published: Option<u32>, last_seen: Option<u32>) -> bool {
    matches!((published, last_seen), (Some(a), Some(b)) if a != b)
}

/// The identity an `open()` was based on: the tab's `claude` process, and the session that
/// process named at the time.
///
/// Recorded so the panel can ask the SAME process the same question again without re-running
/// `hint_for` (which forks `/bin/ps`). The pid is **not** an identity on its own —
/// `~/.claude/sessions/` files outlive their processes, so a recycled pid presents a
/// stranger's record — which is why `process_start_ms` rides with it and every re-read goes
/// through [`ai_monitor::validated_session_id`].
#[derive(Clone, Debug, PartialEq, Eq)]
struct OpenedOn {
    pid: u32,
    /// The process start `hint_for` measured, i.e. what `validated_session_id` compares
    /// the re-read record against.
    process_start_ms: u64,
    /// The session that process named at open — the baseline a rotation is measured from.
    ///
    /// **`None` is armable, not terminal.** A `claude` measured 1.1-2.2s from start to its
    /// first registry record can easily be younger than that when the panel opens, and with
    /// no `--resume` in argv the hint then names no session at all. The first tick that
    /// reads a valid record fills this in ([`Rotation::Arm`]) — *without* re-pointing,
    /// because learning an id we never had is an arrival, not a rotation. Leaving it `None`
    /// for the life of the open, as the first draft of this did, left such a panel
    /// permanently blind to `/clear` while looking perfectly healthy.
    session_id: Option<String>,
}

/// What a registry re-read means for the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Rotation {
    /// Nothing to do: same session, no readable record, or a record we may not believe.
    None,
    /// We had no baseline and now have one. Record it; do **not** re-point.
    Arm(String),
    /// The tab's own process names a different session than the one on screen.
    Rotated,
}

/// Has the tab's own process moved to a **different** session than this open was based on?
///
/// `/clear` retires a session log and opens a new one under a new id, from the same
/// `claude` process, in the same project directory. Nothing in the tail path fires — the
/// old file is abandoned, not truncated — so without this the panel keeps painting a
/// session that is over (`specs/ai-monitor-session-rotation/loop.md`).
///
/// **Authority, not recency.** This asks the tab's own process which session it is running
/// now; it never looks at the directory, and a newer or larger log in that directory is not
/// evidence of anything. That distinction is the whole reason this can coexist with
/// `ai-monitor-rival-assignment` R6.3 ("refresh stops once attached"): R6.3 stops the panel
/// being pulled to a *sibling's* session by mere file activity, and nothing here restores
/// that. `refresh_rivals` keeps its attached early-return untouched.
///
/// **What this DOES narrow, stated rather than left implicit.** R6.3 is written as "no
/// registry reads and no membership reads occur" once attached. The *anti-yank* half is
/// intact; the *cost* half is now literally false on the app side, because an attached panel
/// performs one registry read per wake (see [`AiMonitorOverlay::session_rotated`] for the
/// measured 14 µs). The watcher's reads still stop dead. That narrowing was pre-authorised by
/// this feature's contract, and it is recorded here so the next reader of R6.3 finds the
/// exception where the exception lives.
///
/// **Failure means "no rotation".** A record that cannot be read, or that fails
/// [`ai_monitor::validated_session_id`] against the process start recorded at open, answers
/// [`Rotation::None`] — the panel keeps what it has. The cost of being wrong this way is the
/// pre-existing bug for one more wake; the other direction tears down a live attachment on a
/// transient file read.
///
/// **On storms.** An earlier draft of this comment claimed a storm was *structurally
/// impossible* because both sides of the comparison read one source. That was wrong, and the
/// counter-example is worth keeping: `hint_for` fills `session_id` from
/// `registered_id(..).or_else(|| table.resume_id(..))`, so under `claude --resume <id>` the
/// baseline can come from **argv** while this check always reads the **registry**. If the
/// registry read inside `open()` then fails transiently, the two disagree and a spurious
/// re-point follows. What is true is weaker and sufficient: any disagreement converges in one
/// iteration, because `open()` rewrites the baseline from whatever it resolved — so the cost is
/// one spurious re-point, not a sustained storm.
fn session_rotated_with(
    read: impl FnOnce(u32) -> Option<ai_monitor::Registered>,
    opened_on: Option<&OpenedOn>,
) -> Rotation {
    let Some(prev) = opened_on else {
        return Rotation::None; // no `claude` in this tab — nothing to ask
    };
    let Some(reg) = read(prev.pid) else {
        return Rotation::None; // no record right now; keep what is on screen
    };
    // The SAME guard our own id got at open (`tabsession::registered_id_with`) and every
    // rival's gets in `resolve_owned_session`. A stale file left by a dead process whose
    // pid has been reused would otherwise read as a rotation and re-point the panel onto a
    // stranger's session — the exact failure this feature exists to prevent, arrived at
    // from the other side.
    let Some(current) = ai_monitor::validated_session_id(&reg, prev.process_start_ms) else {
        return Rotation::None;
    };
    match prev.session_id.as_deref() {
        Some(base) if base == current => Rotation::None,
        Some(_) => Rotation::Rotated,
        None => Rotation::Arm(current.to_string()),
    }
}

/// Would this `open()` tear down and rebuild exactly what is already on screen? (D10)
///
/// **All four conditions, and none of them is belt-and-braces** — without each of the last two
/// this breaks a shipped feature:
///
/// 1. the resolved directory equals the one currently open on;
/// 2. the chosen `claude` pid equals the one this open was based on;
/// 3. the resolved session id equals the baseline's; and
/// 4. the panel is visible **and** holds a watcher — i.e. there is actually a live one to keep.
///
/// **Without (3) this swallows `/clear` re-pointing.** `tick_action` returns `Tick::Reopen` for a
/// *rotation* as well as a tab change, and a rotation is by definition **same directory, same
/// pid, new session id** — precisely what conditions 1 and 2 compare equal. A two-value early-out
/// would discard exactly the re-point `ai-monitor-session-rotation` shipped to add. (It works
/// because `apply_rotation` answers `Rotated` *without* moving the baseline, so the old id is
/// still there to disagree with the hint's new one.)
///
/// **Without (4) the toggle chord stops working.** `close()` sets `visible = false` and
/// `watcher = None` but leaves `opened_on`, `repoint` and the session intact, while `open()` sets
/// `visible = true` only at its end. Open → Cmd+Shift+M → Cmd+Shift+M on an unchanged tab would
/// match conditions 1–3, return early, and leave the panel invisible with no watcher until the
/// user switched tabs. "Keep the existing watcher" presumes a watcher `close()` has already
/// dropped.
///
/// Unavailable for the whole `opened_on: None` class — a live session whose start time could not
/// be read — because condition 2 then has nothing to compare. Those tabs keep today's re-point
/// cost, which is the honest trade rather than a guess.
fn reopen_is_inert(
    visible: bool,
    has_watcher: bool,
    opened_dir: Option<&std::path::Path>,
    opened_on: Option<&OpenedOn>,
    cwd: Option<&std::path::Path>,
    hint: &tabsession::SessionHint,
) -> bool {
    // (4) — first, because it is the one that is about the panel rather than about the tab.
    if !visible || !has_watcher {
        return false;
    }
    let (Some(dir), Some(prev)) = (opened_dir, opened_on) else {
        return false;
    };
    // (1) and (2) — the same place, watching the same process.
    if cwd != Some(dir) || hint.pid != Some(prev.pid) {
        return false;
    }
    // (3) — and running the same session it was running then.
    hint.session_id.as_deref() == prev.session_id.as_deref()
}

/// Is a rival refresh still worth its `/bin/ps` fork?
///
/// **Two conditions, and the second is the one this feature added.** R6.3 stops the refresh
/// once attached, because re-resolution has stopped and membership can no longer change the
/// answer. That reasoning holds only for a panel that HAS a baseline. A panel that opened
/// before its tab had a `claude` is attached-but-unarmed: it can be showing the right session
/// (the unattached watcher found it by dating) while having no process recorded to ask about
/// rotations — and the only thing that can arm it is the very lookup R6.3 switched off.
///
/// So the refresh continues while unarmed. It is rare, self-terminating (one re-point arms
/// the baseline and the guard closes again), and no worse than what an unattached panel
/// already did on every wake.
fn rival_refresh_needed(attached: bool, armed: bool) -> bool {
    !attached || !armed
}

/// The rotation baseline this hint supports, if any.
///
/// **Which parts are required, and why they differ.** `pid` and `process_start_ms` are
/// mandatory: together they are the *identity* of the process to re-ask, and neither is
/// usable alone — `~/.claude/sessions/` files outlive their processes, so a pid without a
/// start cannot be told from a stranger's record. `session_id` is optional, because a
/// `claude` younger than its first registry record (measured 1.1-2.2s) with no `--resume` in
/// argv genuinely names nothing yet; that baseline is ARMED later by [`Rotation::Arm`]
/// rather than abandoned.
///
/// Split out of `open()` because a checker mutated that assignment to `None` and all 1133
/// tests stayed green, with rotation detection entirely dead.
fn baseline_from(hint: &tabsession::SessionHint) -> Option<OpenedOn> {
    match (hint.pid, hint.started_ms) {
        (Some(pid), Some(process_start_ms)) => Some(OpenedOn {
            pid,
            process_start_ms,
            session_id: hint.session_id.clone(),
        }),
        // No `claude` under this tab, or no measurable start for one: there is no process to
        // re-ask, so the check disables itself for this open rather than guessing.
        _ => None,
    }
}

/// Apply a [`Rotation`] to the baseline, and answer whether the panel must re-point.
///
/// **The one stateful transition this feature adds, split out so a test can drive it.** The
/// rule ([`session_rotated_with`]) is pure and was already covered; the *state change* was
/// not, and a checker proved the gap by mutating this arm to `Rotation::Arm(_) => false` —
/// with all 1133 tests still green, while a panel opened during `claude`'s first ~1.5 s went
/// permanently blind to `/clear`. That is precisely the regression the arming path exists to
/// fix, so the transition gets the same treatment every other decision in this file gets.
fn apply_rotation(outcome: Rotation, baseline: &mut Option<OpenedOn>) -> bool {
    match outcome {
        Rotation::Rotated => true,
        // Learning an id we never had is an ARRIVAL, not a rotation: record it and answer
        // "no re-point", so this wake falls through to the rival refresh. Re-opening here
        // would drop a live watcher and re-run a full backfill to learn nothing. The next
        // `/clear` then has a baseline to be measured against, which is the whole point.
        // One consequence to be explicit about, since it follows from the decision rather
        // than from an oversight: arming updates the BASELINE only, never the live
        // `Watch.want.session_id`. A panel that opened before its session was named therefore
        // keeps resolving on the weaker tier-2 dating path for the rest of that open. Pushing
        // the id into the watcher would mean re-opening, which is exactly what arming exists
        // not to do; the next `/clear` re-opens anyway and repairs it.
        Rotation::Arm(id) => {
            if let Some(prev) = baseline.as_mut() {
                prev.session_id = Some(id);
            }
            false
        }
        Rotation::None => false,
    }
}

/// Has this log written since the window opened?
///
/// **The single site of this question**, and the only writer of the shared
/// `attached` flag the watch thread reads. It has been implemented twice before —
/// once in the renderer with the opposite handling of undated logs, once in the
/// watcher where it silently disagreed with this one about the backfill — and both
/// times the duplicate is what broke (`specs/overlay-session-attach/loop.md`, L1/L7).
/// It is also why the answer is *stored* rather than recomputed: as a per-frame
/// predicate it erases a panel already showing a live session.
///
/// A log with no parseable event time answers **no**: absence of a clock is not
/// evidence of freshness. It costs nothing — the watcher keeps re-resolving until
/// some log does answer yes.
///
/// **Production can no longer *attach through* this** — note the wording: the call is
/// still made. `Attachment::admit` reaches it whenever its confirmed-and-non-empty test
/// fails, and since `open()` resolves via `resolve_owned_session` (every result
/// `owned: true`), the only production case that gets here is an empty snapshot from a
/// resolution that returned `None` — which answers `false`, because a default snapshot
/// carries no `last_event_ms`. So the *outcome* is unreachable, not the code: do not
/// delete the call believing it dead. It is retained as the attach contract for a
/// caller using the ranked `resolve_session` fallback, and it is still directly
/// unit-tested below.
fn proves_ownership(snap: &TelemetrySnapshot, started_ms: u64) -> bool {
    snap.last_event_ms.is_some_and(|last| last >= started_ms)
}

/// Drain a snapshot channel, keeping only the newest value.
///
/// Snapshots are CUMULATIVE, so every one but the last is already stale by the time
/// the UI thread wakes — rendering each would cost N model writes and N redraws for
/// one visible state. Split out of `poll` so the coalescing is unit-testable:
/// `AiMonitorOverlay` needs an `EventLoopProxy`, which needs a real winit
/// `EventLoop` (main-thread-only, once per process on macOS) and so cannot be
/// constructed under `cargo test`.
fn drain_to_newest<T>(rx: &std::sync::mpsc::Receiver<T>) -> Option<T> {
    let mut newest = None;
    while let Ok(v) = rx.try_recv() {
        newest = Some(v);
    }
    newest
}

impl Overlay for AiMonitorOverlay {
    fn priority(&self) -> i32 {
        100
    }

    fn visible(&self) -> bool {
        self.visible
    }

    fn on_key(&mut self, input: &KeyInput, ctx: &mut AppCtx) -> Handled {
        if Self::is_toggle(input) {
            if self.visible {
                self.close();
            } else {
                self.open(ctx.active_cwd.clone(), ctx.active_shell_pid);
            }
            // Reserve (or release) the band the panel paints. The renderer derives
            // `right_inset` from this, so it is what makes the panel *docked* rather
            // than an overlay: the grid loses those columns instead of being covered.
            //
            // Open/closed is all that crosses. The *width* comes from
            // `[ai_monitor].width_px` plus the runtime override, clamped by D4's
            // 20-column terminal floor — none of which this file knows or should: the
            // chord's only decision is whether the panel is up. That is also what makes
            // this the one writer of `[ai_monitor].open` (D8): `terminal_app` persists on
            // exactly this request, and the launch restore takes a different route that
            // never reaches the persistence path (R6.3/R6.6).
            ctx.set_monitor_open(self.visible);
            ctx.request_redraw();
            return Handled::Consumed; // R2.1 — never reaches the shell
        }
        // D12/D13/D16 — jump straight to a section. **This reverses the decision recorded
        // two lines below**, and the reversal is gated rather than wholesale: the panel takes
        // these six chords ONLY while it is on screen. That is what preserves the reason the
        // original decision existed — a panel that is not up captures nothing, so the class
        // of accident it feared (a key stolen from `vim` or `less`) remains impossible.
        //
        // Visibility is checked here and not left to the dispatcher, which deliberately
        // offers keys to hidden overlays so an overlay can consume the chord that opens it.
        if self.visible {
            if let Some(section) = input::monitor_jump(input) {
                self.jump_to(section);
                ctx.request_redraw();
                return Handled::Consumed;
            }
        }
        // A monitor doesn't capture other keys (D1/R1.2/R1.3): no `Esc`, no `q`. The
        // panel never takes focus, so an `Esc` handler here would fire while the user is
        // in `vim`'s insert mode, and `q` is the key that quits `less`.
        //
        // **That rule is now narrower than it reads, and the narrowing was deliberate**
        // (ai-monitor-panel-scroll D16): the six jump chords above are an exception, gated on
        // the panel being visible. What it still forbids — and what made it right — is bare
        // keys the shell is waiting for. `Esc` and `q` remain untouchable at every panel
        // state; `Cmd+Shift+<digit>` is a chord no shell reads. Do not "restore" the rule by
        // deleting the block above without re-reading D16: the panel's only way back to the
        // top (D30) goes through it.
        Handled::Passthrough
    }

    /// D7/D9/D10/C12 — the panel takes the wheel while the pointer is over it, and hands it
    /// back at an edge only in the direction that pushes further into that edge.
    ///
    /// The trade this makes is deliberate and was named as a risk when it was made: scrolling
    /// the terminal's scrollback with the pointer parked over the panel stops working. D9
    /// recovers it at the two ends, which is a partial recovery, not a full one.
    ///
    /// The residue lives here — one accumulator for the panel, strictly apart from the grid's
    /// `wheel_accum` (D23). A rung and a text line are different units, so carrying leftovers
    /// between them would make a reversal lurch by whatever the other side had banked.
    ///
    /// The geometry is entirely the renderer's: this asks the handle to move by a distance
    /// and reads back whether it could, rather than rebuilding the ladder from its own copy
    /// of the model and guessing a viewport height (SD2).
    fn on_wheel(&mut self, delta: WheelDelta, cell_h: f32) -> Handled {
        if !self.visible {
            return Handled::Passthrough;
        }
        let px = input::panel_wheel_px(delta, cell_h);
        if self.handle.wheel_px(px, &mut self.wheel_residue) {
            self.rearm_anchor();
            // D33: at the top there is nothing above left unseen, so the badge's own claim
            // has become false. One rule, and it lines up exactly with what the badge means —
            // which is why "clear only on the return chord" was rejected: scrolling all the
            // way up by hand would leave it lying.
            if self.handle.scroll().top == 0 {
                let s = self.handle.scroll();
                self.handle.set_scroll(ScrollState { badge: 0, ..s });
            }
            return Handled::Consumed;
        }
        // It did not move. Either the gesture is still banking below one rung — in which
        // case the panel kept the event and there is simply nothing to draw yet — or this is
        // the edge in this direction, which is D9's hand-off. `scroll_by_px` tells the two
        // apart by zeroing the residue at an edge and keeping it while banking.
        if self.wheel_residue == 0.0 {
            Handled::Passthrough
        } else {
            Handled::Consumed
        }
    }

    fn on_press(&mut self, point: [f32; 2]) -> Handled {
        if !self.visible {
            return Handled::Passthrough;
        }
        // The panel resolves the point against its own published bands; this file does no
        // geometry (SD2). `None` is a press on a row or on dead space — the panel owns the
        // pixels either way, so it is consumed rather than dropped through onto the grid,
        // which would put a selection caret under a panel the user was clicking.
        match self.handle.press(point) {
            Some(section) => {
                self.toggle_fold(section);
                Handled::Consumed
            }
            None => Handled::Consumed,
        }
    }

    fn layer(&self) -> Box<dyn DrawLayer> {
        Box::new(MonitorPanel::from_handle(self.handle.clone()))
    }

    /// Fold any tailed data in, then **rebuild the model regardless** — and report whether
    /// the model moved, not whether the watcher carried anything (D2).
    ///
    /// The old form returned `newest.is_some()`, so a wake that found the queue empty was
    /// declined outright. That was wrong in a specific way: this arm also receives the
    /// tabmeta worker's wakes and `refresh_rivals`' probe ping, and on those the app goes on
    /// to redraw anyway if the waiting band moved — drawing a frame from a model nobody had
    /// rebuilt. The panel was declining a frame it was already paying for.
    ///
    /// **What gates the redraw now is the model, not the source of the wake.** That is the
    /// whole of D2: the model is a tree of already-formatted strings and is already stage
    /// one's cache key, so "did anything the reader can see move" is exactly the question
    /// `refresh_model`'s diff answers. A time-derived value that ticks earns its frame with no
    /// special case, and so will the next one.
    ///
    /// **The one gate that survives is `self.visible`.** D2 argues against gating the rebuild
    /// on whether a frame is coming; it does not argue against gating on whether the panel
    /// exists. A closed panel rebuilds nothing, arms nothing and costs nothing, which the
    /// Constraints require. The drain still runs first so a watcher that outlived its panel
    /// cannot back its channel up.
    fn poll(&mut self) -> bool {
        // The drain is conditional on a watcher; the rebuild deliberately is NOT. An open
        // panel whose `open()` resolved no log to tail has no watcher, and the old early
        // return meant such a panel could never rebuild — so a deadline wake aimed at it
        // would land on a function that returns before doing anything. Its model is static in
        // that state (no telemetry, so the tiles and footer render `DASH`), which means the
        // rebuild costs one `dashboard::model` and then loses the diff, exactly as it should.
        if let Some(watcher) = self.watcher.as_ref() {
            if let Some(update) = drain_to_newest(&watcher.rx) {
                self.session
                    .note(update.snap, self.started_ms, update.owned);
            }
        }
        if !self.visible {
            return false;
        }
        self.refresh_model()
    }

    /// The launch restore (R6.6): reopen the panel because `[ai_monitor].open` says the
    /// last session left it open.
    ///
    /// **It goes through `open()`, exactly as the chord does, and that is the requirement
    /// rather than an implementation detail.** Everything that makes the panel show
    /// anything happens in there — resolving *this tab's* log, folding it into a fresh
    /// accumulator, spawning the tail watcher. Setting `visible = true` here instead would
    /// reserve the band and paint an empty panel that never fills, because nothing calls
    /// `open()` for a panel that is already open.
    ///
    /// Guarded on `!self.visible` for the reason `Repoint` exists: re-opening a live panel
    /// drops and respawns the watcher and re-runs the whole backfill.
    fn on_command(&mut self, cmd: &OverlayCommand) -> Handled {
        match cmd {
            OverlayCommand::RestoreMonitor { cwd, shell_pid } => {
                if !self.visible {
                    self.open(cwd.clone(), *shell_pid);
                }
                Handled::Consumed
            }
            _ => Handled::Passthrough,
        }
    }

    fn on_active_changed(
        &mut self,
        cwd: Option<PathBuf>,
        shell_pid: Option<u32>,
        agent_pid: Option<u32>,
        agent_state: Option<crate::tabmeta::AgentState>,
    ) {
        let visible = self.visible;
        let tab_changed = self.repoint.changed(visible, cwd.as_deref(), shell_pid);
        let agent_moved = agent_pid_moved(agent_pid, self.last_seen_pid);
        // A `None` publish is NO INFORMATION: it must not overwrite the memory, or a transient
        // walk failure (or a flipped display toggle, before D6 lifted the active tab's walk out
        // of it) would erase `last_seen_pid` and make the next real publish look like a change.
        if agent_pid.is_some() {
            self.last_seen_pid = agent_pid;
        }
        // The agent STATE, stored unconditionally — note the contrast with the guard directly
        // above, which is the whole of D18's asymmetry (argued at the seam). Both stores sit
        // above the dispatch because `tick_action` no-ops on every wake where the tab did not
        // change, which is every wake in the quiet case this feature exists for.
        self.agent_state = agent_state;
        // Execute a deferred arm here, on the wake that delivered the state, rather than
        // leaving it to the next rebuild. The two arming inputs arrive on different wakes at
        // different cadences: `last_event_ms` rides the panel's own watcher, which fires per
        // append burst, while the state rides the worker's 500 ms publish. On a clean turn
        // ending the watcher wins that race — the timestamp moves while the state still reads
        // a stale `Busy`, so the rebuild defers — and the later publish that flips the state
        // finds the timestamp unchanged. Waiting for a rebuild after it would mean waiting for
        // a wake that a now-quiet session will never produce, which is the primary path this
        // feature targets, so the arm would never happen at all.
        //
        // It reads the status of the model already on screen, because that is the pill the
        // deadline would be correcting; there is nothing to correct before the first publish.
        if let Some(status) = self.published.as_ref().map(|m| m.status) {
            self.reconsider_deadline(status);
        }
        // The order, and the laziness, live in `tick_action` — one author for both.
        match tick_action(tab_changed, visible, agent_moved, || self.session_rotated()) {
            // A tab change and a rotation take the SAME action, and deliberately so.
            // `open()` is the whole re-point in both cases: `Session::repoint` drops the
            // old attachment and its flag, the accumulator is rebuilt from scratch
            // (R10.3) rather than continued, and the watcher is respawned wanting the id
            // the tab's process names *now*.
            //
            // Routing a rotation through `open()` also settles what the panel shows in the
            // gap, and the honest answer is "usually there is no gap". Measured on the
            // 88187 chain: `/clear` opens the successor log within ~5 ms and writes the
            // `/clear` command into it as one of its first records — the marker lives at
            // the START of the successor, not the end of the predecessor. So by the time
            // this fires the new file almost always exists, strict resolution finds it, and
            // the panel re-points straight onto it. When it does not, resolution answers
            // `None` and nothing is folded, so what the panel holds is an EMPTY snapshot
            // (not the absence of one) — `Detached::of` reads that as `NoLog` and, with
            // `tab_agent`, shows "this tab's session has not written a log yet" rather
            // than a dead session's numbers. The freshly-spawned watcher, now unattached,
            // adopts the log when it lands.
            //
            // WHEN this fires is a separate question and is NOT established — see
            // `session_rotated`. It depends on when `claude` rewrites its registry record
            // after a `/clear`, which historical logs cannot show, because only the final
            // state of `~/.claude/sessions/<pid>.json` survives.
            Tick::Reopen => self.open(cwd, shell_pid),
            // Same tab, same session: republish the rival set rather than re-opening. D6
            // fires this on every wake, which is the cadence D11 wants — and it is why the
            // refresh must not re-run `open`, which would drop the watcher and re-run the
            // full backfill (the storm `Repoint` exists to stop).
            Tick::RefreshRivals => self.refresh_rivals(shell_pid),
            Tick::Idle => {}
        }
    }
}

#[cfg(test)]
mod tests {

    use crate::input::WheelDelta;
    use renderer::monitor_model::MonitorModel;
    use renderer::monitor_panel::{MonitorHandle, ScrollState};

    // ---- ai-monitor-panel-scroll T8 / R6.1-R6.7 / R11: anchoring the reading position ----

    fn body_keys(n: usize, tag: &str) -> Vec<RungKey> {
        use std::hash::{Hash, Hasher};
        (0..n)
            .map(|i| {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                format!("{tag}{i}").hash(&mut h);
                RungKey::Body(h.finish())
            })
            .collect()
    }

    #[test]
    fn r6_4_a_rungs_key_covers_what_it_displays_including_its_outcome() {
        // D19 keys on appearance because the rows carry no id. That makes *what counts as
        // appearance* a real decision: a call that turns from running into failed is a
        // different row to the reader, and an anchor that silently followed it would be
        // holding onto something that is no longer what was being read. So the outcome glyph
        // and the value colour are in the key, not just the text.
        use renderer::monitor_layout::{Item, ItemKind};
        use renderer::monitor_model::{ColorRole, Row, Section, SectionBody};
        let row = |glyph, color| Row {
            glyph: Some(glyph),
            text: "Bash".into(),
            value: "1.8s".into(),
            value_color: color,
            ..Row::default()
        };
        let model = |r: Row| MonitorModel {
            sections: vec![Section {
                collapsed: false,
                label: "RECENT CALLS".into(),
                total: 1,
                lost: 0,
                body: SectionBody::Rows(vec![r]),
            }],
            ..MonitorModel::default()
        };
        let item = Item {
            section: 0,
            kind: ItemKind::Row,
            entry: Some(0),
        };
        let ok = rung_key(&model(row('✓', ColorRole::Teal)), &item);
        let failed = rung_key(&model(row('✗', ColorRole::Amber)), &item);
        assert_ne!(
            ok, failed,
            "an outcome change must not be followed silently"
        );
        assert_eq!(
            ok,
            rung_key(&model(row('✓', ColorRole::Teal)), &item),
            "and an unchanged row must key the same, or the anchor is lost every snapshot"
        );

        // A heading keys on its label alone — and it must, since jumps land there.
        let heading = rung_key(
            &model(row('✓', ColorRole::Teal)),
            &Item {
                section: 0,
                kind: ItemKind::Heading,
                entry: None,
            },
        );
        assert_eq!(heading, RungKey::Heading("RECENT CALLS".into()));
    }

    #[test]
    fn r6_2_an_insertion_above_the_anchor_moves_the_index_and_not_the_reader() {
        // D8. Three calls arrive at the head of RECENT CALLS while the reader is partway
        // down it. The row under their eye must stay put — which means the INDEX has to
        // move, by exactly as many rungs as were inserted above it.
        let prev = body_keys(10, "row");
        let anchor = Anchor::Held {
            key: prev[6].clone(),
            index: 6,
        };
        let mut next = body_keys(3, "new");
        next.extend(prev.iter().cloned());
        assert_eq!(
            resolve_anchor(&anchor, &prev, &next),
            9,
            "6 + 3 inserted above"
        );
    }

    #[test]
    fn r6_1_following_ignores_the_anchor_machinery_entirely() {
        // D37, and the reason it exists: without a Following state the reader who has NOT
        // scrolled loses live updates on the very first snapshot after opening the panel —
        // an anchor at index 0 would be pushed down by anything arriving above it, and the
        // panel would stop being a monitor without anyone touching it.
        let prev = body_keys(4, "row");
        let mut next = body_keys(2, "new");
        next.extend(prev.iter().cloned());
        assert_eq!(resolve_anchor(&Anchor::Following, &prev, &next), 0);
    }

    #[test]
    fn r6_4_a_heading_anchor_is_matched_by_label_not_by_content() {
        // D32 made headings first-class rungs, so an anchor ON a heading is the normal case:
        // it is where every jump lands and where `max_index` sits. Their labels are fixed and
        // distinct, so no hash and no collision rule is needed.
        let prev = vec![
            RungKey::Heading("TASKS".into()),
            RungKey::Heading("RECENT CALLS".into()),
        ];
        let anchor = Anchor::Held {
            key: RungKey::Heading("RECENT CALLS".into()),
            index: 1,
        };
        let mut next = vec![RungKey::Heading("TASKS".into())];
        next.extend(body_keys(5, "row"));
        next.push(RungKey::Heading("RECENT CALLS".into()));
        assert_eq!(resolve_anchor(&anchor, &prev, &next), 6);
    }

    #[test]
    fn r6_4_duplicate_rows_resolve_to_the_nearest_candidate() {
        // The same tool called twice renders identically, so the key genuinely collides.
        // Without a nearest rule the view teleports between the two as the list shifts —
        // a jump the user did not ask for, which is what C7 forbids.
        let dup = body_keys(1, "same")[0].clone();
        let prev = vec![dup.clone(); 1];
        let next = vec![dup.clone(), RungKey::Heading("X".into()), dup.clone()];
        assert_eq!(
            resolve_anchor(
                &Anchor::Held {
                    key: dup.clone(),
                    index: 2
                },
                &prev,
                &next
            ),
            2,
            "the far copy is nearer to index 2"
        );
        assert_eq!(
            resolve_anchor(&Anchor::Held { key: dup, index: 0 }, &prev, &next),
            0,
            "and the near copy to index 0"
        );
    }

    #[test]
    fn r6_6_an_idle_anchored_view_does_not_drift_across_repeated_snapshots() {
        // D14/C7: the panel is on a live feed, so "the view holds still" has to survive many
        // updates, not one. A resolver that was off by a rung per snapshot would look correct
        // in a single-step test and walk the reader down the document over a few minutes —
        // the failure mode nobody reports because nobody sees it happen.
        let prev = body_keys(12, "row");
        let mut anchor = Anchor::Held {
            key: prev[7].clone(),
            index: 7,
        };
        let mut keys = prev.clone();
        for _ in 0..50 {
            // A snapshot that changed nothing at all.
            let top = resolve_anchor(&anchor, &keys, &keys);
            assert_eq!(top, 7, "the view drifted on an unchanged document");
            anchor = Anchor::Held {
                key: keys[top].clone(),
                index: top,
            };
        }
        // And with churn that never touches what is above the anchor: rows appended BELOW it
        // must not move it either.
        for extra in 1..20 {
            let mut next = keys.clone();
            next.extend(body_keys(extra, "tail"));
            let top = resolve_anchor(&anchor, &keys, &next);
            assert_eq!(top, 7, "appending below the anchor moved it");
            keys = next;
        }
    }

    #[test]
    fn r6_5_an_evicted_anchor_lands_on_its_nearest_surviving_neighbour() {
        // D15. The alternative — snapping to an end — throws away the reader's position at
        // the exact moment they are least likely to have caused it: a ring eviction is the
        // panel's doing, not theirs.
        let prev = body_keys(6, "row");
        let anchor = Anchor::Held {
            key: prev[3].clone(),
            index: 3,
        };
        // Rung 3 is gone; its neighbours survive, reindexed by one insertion at the head.
        let mut next = body_keys(1, "new");
        next.extend(
            prev.iter()
                .enumerate()
                .filter(|(i, _)| *i != 3)
                .map(|(_, k)| k.clone()),
        );
        let at = resolve_anchor(&anchor, &prev, &next);
        assert!(
            at == 3 || at == 4,
            "must land beside the lost rung (old 2 or old 4), got {at}"
        );
        assert_ne!(at, 0, "and never snap to an end");
    }

    #[test]
    fn r6_5_an_anchor_with_no_survivors_at_all_returns_to_following() {
        // Every rung gone — a re-point onto a different session. There is nothing to hold,
        // and pretending otherwise would strand the view at an arbitrary index.
        let prev = body_keys(4, "old");
        let anchor = Anchor::Held {
            key: prev[2].clone(),
            index: 2,
        };
        assert_eq!(resolve_anchor(&anchor, &prev, &body_keys(4, "new")), 0);
    }

    #[test]
    fn r6_3_the_mode_is_a_function_of_the_index_not_a_latch() {
        // The self-contradiction D37 had to close. If `Following` were latched at the moment
        // the user returned to the top, a document that SHRANK under an anchored view could
        // leave the index at 0 with an anchor still armed — and the next arrival would push
        // the reader down from a position they never chose.
        let mut o = seeded_overlay(8, 20.0);
        o.keys = body_keys(8, "row");
        o.handle.set_scroll(ScrollState { top: 4, badge: 0 });
        o.rearm_anchor();
        assert!(matches!(o.anchor, Anchor::Held { index: 4, .. }));
        o.handle.set_scroll(ScrollState { top: 0, badge: 0 });
        o.rearm_anchor();
        assert!(
            matches!(o.anchor, Anchor::Following),
            "index 0 IS following — no latch survives it"
        );
    }

    #[test]
    fn r11_1_and_r11_2_opening_or_repointing_starts_at_the_top_holding_nothing() {
        // D25. The re-open path respawns the watcher and re-runs the backfill, so the
        // document is new regardless; a carried-over position would land by way of D15's
        // survivor walk at a rung the reader never picked.
        //
        // R11.2 (a tab switch) rides the same path rather than getting its own: `Tick::Reopen`
        // calls `open()`, and `open()` calls this. Asserting the reset here is asserting it
        // for both — and a second test that re-implemented the reset would prove nothing
        // about the shipped one.
        let mut o = seeded_overlay(8, 20.0);
        o.keys = body_keys(8, "row");
        o.handle.set_scroll(ScrollState { top: 5, badge: 0 });
        o.rearm_anchor();
        o.wheel_residue = 3.5;
        assert!(matches!(o.anchor, Anchor::Held { .. }));

        o.reset_scroll();
        assert_eq!(o.handle.scroll().top, 0);
        assert!(matches!(o.anchor, Anchor::Following));
        assert_eq!(
            o.wheel_residue, 0.0,
            "a stale bank would lurch the first gesture"
        );
    }

    // ---- ai-monitor-panel-scroll T10 / R8.1-R8.6: the six section jumps ----

    fn jump_chord(code: winit::keyboard::KeyCode) -> KeyInput {
        KeyInput {
            // What a US layout actually delivers for `Cmd+Opt+1` — the physical code is
            // what `monitor_jump` reads, and this field being a symbol is the point.
            key: Key::Character("¡".into()),
            text: Some("¡".into()),
            mods: ModifiersState::SUPER | ModifiersState::ALT,
            physical: winit::keyboard::PhysicalKey::Code(code),
        }
    }

    /// A handle whose document is six sections of `n` rows each — the shape the panel ships.
    fn six_section_handle(n: usize, viewport_h: f32) -> MonitorHandle {
        use renderer::monitor_layout::{Item, ItemKind};
        use renderer::monitor_panel::ScrollView;
        let mut ladder = Vec::new();
        for section in 0..6 {
            ladder.push(Item {
                section,
                kind: ItemKind::Heading,
                entry: None,
            });
            for e in 0..n {
                ladder.push(Item {
                    section,
                    kind: ItemKind::Row,
                    entry: Some(e),
                });
            }
        }
        let heights = vec![10.0; ladder.len()];
        let h = MonitorHandle::default();
        h.publish_view(ScrollView {
            viewport: renderer::monitor_layout::NO_BAND,
            slot: renderer::monitor_layout::NO_BAND,
            ladder,
            heights,
            viewport_h,
        });
        h
    }

    /// The same six-section handle, but with real bands so a press can be resolved.
    fn clickable_handle(n: usize, viewport_h: f32) -> MonitorHandle {
        use renderer::monitor_panel::ScrollView;
        use renderer::sidebar_layout::Rect;
        let h = six_section_handle(n, viewport_h);
        let v = h.view();
        h.publish_view(ScrollView {
            // A 200-wide panel at x=800, its slot one 10px line and the content band below.
            slot: Rect {
                x: 800.0,
                y: 100.0,
                w: 200.0,
                h: 10.0,
            },
            viewport: Rect {
                x: 800.0,
                y: 110.0,
                w: 200.0,
                h: viewport_h,
            },
            ..v
        });
        h
    }

    #[test]
    fn r13_6_only_a_heading_or_a_fold_control_answers_a_press() {
        // The panel has exactly two click targets, and rows are not among them. Giving rows
        // a meaning would make every stray click in a live list change the document.
        let o = {
            let mut o = AiMonitorOverlay::detached();
            o.handle = clickable_handle(4, 100.0);
            o.visible = true;
            o
        };
        // Rung 0 is section 0's heading; at top=0 the slot shows it and the band starts at
        // rung 1, so the heading is reachable **only** through the slot.
        assert_eq!(
            o.handle.press([850.0, 105.0]),
            Some(0),
            "the sticky heading"
        );
        // First rung inside the band is a row of section 0 — content, not a control.
        assert_eq!(
            o.handle.press([850.0, 115.0]),
            None,
            "a row is not a control"
        );
        // Section 1's heading sits at rung 5 (4 rows + the next heading): band rungs are
        // 1,2,3,4 then 5, so y = 110 + 4*10 = 150.
        assert_eq!(
            o.handle.press([850.0, 155.0]),
            Some(1),
            "a heading in the band"
        );
        // Outside the panel entirely.
        assert_eq!(o.handle.press([100.0, 155.0]), None);
    }

    #[test]
    fn r13_7_a_press_on_a_heading_folds_that_section_and_the_next_one_unfolds_it() {
        // The click contract end to end: the overlay flips exactly the section that was
        // pressed, leaves its neighbours alone, and the second press is the way back.
        let mut o = AiMonitorOverlay::detached();
        o.handle = clickable_handle(4, 100.0);
        o.visible = true;
        o.folds = vec![false; 6];

        assert_eq!(o.on_press([850.0, 155.0]), Handled::Consumed);
        assert!(o.folds[1], "the pressed section folded");
        assert!(!o.folds[0] && !o.folds[2], "its neighbours did not move");

        assert_eq!(o.on_press([850.0, 155.0]), Handled::Consumed);
        assert!(!o.folds[1], "the second press is the way back");
    }

    #[test]
    fn r13_8_a_press_the_panel_owns_but_no_control_claims_is_still_the_panels() {
        // Dead space inside the band is consumed rather than passed down: the panel reserves
        // its columns, so letting the press through would put a selection caret in a grid
        // the user cannot even see there.
        let mut o = AiMonitorOverlay::detached();
        o.handle = clickable_handle(4, 100.0);
        o.visible = true;
        assert_eq!(o.on_press([850.0, 115.0]), Handled::Consumed);
        // and a closed panel claims nothing at all
        o.visible = false;
        assert_eq!(o.on_press([850.0, 115.0]), Handled::Passthrough);
    }

    #[test]
    fn r8_1_and_r8_5_each_chord_lands_on_its_section_and_sends_no_byte_to_the_shell() {
        // D16 chose six separate keys over cycling precisely so one press reaches the target.
        // The landing index is the heading's OWN — not the rung after it — because D31 has
        // the slot consume a heading, so `index + 1` would draw the identical frame while
        // leaving the scroll state a rung off from where every other path puts it.
        let mut o = AiMonitorOverlay::detached();
        o.handle = six_section_handle(4, 30.0);
        o.visible = true;
        let mut ctx_writer = Vec::new();
        for (i, code) in [
            winit::keyboard::KeyCode::Digit1,
            winit::keyboard::KeyCode::Digit2,
            winit::keyboard::KeyCode::Digit3,
            winit::keyboard::KeyCode::Digit4,
            winit::keyboard::KeyCode::Digit5,
            winit::keyboard::KeyCode::Digit6,
        ]
        .into_iter()
        .enumerate()
        {
            let mut probe = WriteProbe::new(&mut ctx_writer);
            let mut ctx = AppCtx::new(&mut probe, 1, 0, None);
            assert_eq!(
                o.on_key(&jump_chord(code), &mut ctx),
                Handled::Consumed,
                "section {i} chord must be consumed"
            );
            // Asserted against the LADDER, not against `jump_target` — the call the code
            // itself makes. Comparing the two would be a tautology that passes even if the
            // target were clamped away from the heading it names.
            let top = o.handle.scroll().top;
            let rung = o
                .handle
                .rung_at(top)
                .unwrap_or_else(|| panic!("section {i}: index {top} is off the ladder"));
            assert!(rung.is_heading(), "section {i} landed on a body rung");
            assert_eq!(
                rung.section, i,
                "section {i} landed on section {}",
                rung.section
            );
        }
        assert!(
            ctx_writer.is_empty(),
            "R8.5: not one byte of a jump chord may reach the shell"
        );
    }

    #[test]
    fn r8_2_the_last_section_can_still_reach_the_top() {
        // The case a sidebar-style clamp gets wrong, end to end: a long document ending in a
        // short section. If `max_index` stopped before the final heading, `Cmd+Shift+6` would
        // move the view and STILL not show TOOL USAGE at the top — D12 left unsolved while
        // looking implemented.
        let mut o = AiMonitorOverlay::detached();
        o.handle = six_section_handle(8, 40.0);
        o.visible = true;
        let mut w = Vec::new();
        let mut probe = WriteProbe::new(&mut w);
        let mut ctx = AppCtx::new(&mut probe, 1, 0, None);
        o.on_key(&jump_chord(winit::keyboard::KeyCode::Digit6), &mut ctx);
        assert_eq!(
            o.handle.scroll().top,
            o.handle.jump_target(5).unwrap(),
            "the sixth heading must be reachable, not merely approached"
        );
    }

    #[test]
    fn r8_4_a_closed_panel_leaves_the_chord_alone() {
        // The gate that makes reversing `overlay.rs`'s "no other keys" decision safe: while
        // the panel is not on screen the six chords go wherever they went before, so the
        // accident the original decision feared — a key taken from `vim` or `less` — is still
        // impossible.
        let mut o = AiMonitorOverlay::detached();
        o.handle = six_section_handle(4, 30.0);
        o.visible = false;
        let mut w = Vec::new();
        let mut probe = WriteProbe::new(&mut w);
        let mut ctx = AppCtx::new(&mut probe, 1, 0, None);
        assert_eq!(
            o.on_key(&jump_chord(winit::keyboard::KeyCode::Digit1), &mut ctx),
            Handled::Passthrough
        );
    }

    #[test]
    fn r8_6_an_open_panel_still_passes_everything_else_through() {
        // D1/R1.2/R1.3 survive the reversal: the panel gained six chords, not a focus. `Esc`
        // in `vim` and `q` in `less` must still reach the shell with the panel wide open.
        let mut o = AiMonitorOverlay::detached();
        o.handle = six_section_handle(4, 30.0);
        o.visible = true;
        let mut w = Vec::new();
        for k in [
            chord(Key::Named(NamedKey::Escape), None, ModifiersState::empty()),
            chord(
                Key::Character("q".into()),
                Some("q"),
                ModifiersState::empty(),
            ),
            chord(
                Key::Character("7".into()),
                Some("7"),
                ModifiersState::SUPER | ModifiersState::SHIFT,
            ),
        ] {
            let mut probe = WriteProbe::new(&mut w);
            let mut ctx = AppCtx::new(&mut probe, 1, 0, None);
            assert_eq!(o.on_key(&k, &mut ctx), Handled::Passthrough, "{:?}", k.key);
        }
    }

    #[test]
    fn r8_1_and_r7_8_the_first_chord_is_also_the_way_back_and_clears_the_badge() {
        // D30 invented no binding: TASKS is the first section, so `Cmd+Opt+1` already
        // means "top". And arriving at index 0 clears the badge under D33's single rule —
        // which is why the badge can advertise this chord as the way back.
        let mut o = AiMonitorOverlay::detached();
        o.handle = six_section_handle(4, 30.0);
        o.visible = true;
        o.handle.set_scroll(ScrollState { top: 12, badge: 9 });
        let mut w = Vec::new();
        let mut probe = WriteProbe::new(&mut w);
        let mut ctx = AppCtx::new(&mut probe, 1, 0, None);
        o.on_key(&jump_chord(winit::keyboard::KeyCode::Digit1), &mut ctx);
        assert_eq!(o.handle.scroll().top, 0);
        assert_eq!(o.handle.scroll().badge, 0);
    }

    // ---- ai-monitor-panel-scroll T9 / R7.1 / R7.7 / R7.8: counting and clearing ----

    #[test]
    fn r7_1_new_is_a_set_difference_not_a_length_difference() {
        // Arithmetic on lengths undercounts the moment a ring starts evicting: three arrive
        // at the head while two fall off the tail and the length has not moved at all. That
        // is the steady state of a busy session — precisely when the badge matters.
        let old = body_keys(6, "row");
        let mut next = body_keys(3, "new");
        next.extend(old.iter().take(4).cloned()); // 3 arrived, 2 evicted; len 6 -> 7
        assert_eq!(
            count_new_above(&old, &next, 5),
            3,
            "all three arrivals are above the reader"
        );
        assert_eq!(
            count_new_above(&old, &next, 2),
            2,
            "only what is actually above the anchor counts"
        );
    }

    #[test]
    fn r7_1_and_r7_8_following_never_carries_a_badge() {
        // D33/D37 line up here: at index 0 there is nothing above to have missed, so the
        // badge's own claim is false. One rule, and it is the same rule that clears it.
        let old = body_keys(4, "row");
        let mut next = body_keys(9, "new");
        next.extend(old.iter().cloned());
        assert_eq!(count_new_above(&old, &next, 0), 0);
    }

    #[test]
    fn r7_8_scrolling_back_to_the_top_clears_the_badge() {
        // Rejected alternative: clear only on the return chord. Then wheeling all the way up
        // by hand leaves the badge standing while nothing is unseen — the badge lying is
        // worse than no badge, because the reader learns to ignore it.
        let mut o = seeded_overlay(8, 20.0);
        o.keys = body_keys(9, "row");
        o.handle.set_scroll(ScrollState { top: 4, badge: 12 });
        // Wheel up far enough to reach the top.
        for _ in 0..12 {
            o.on_wheel(WheelDelta::Lines(1.0), 16.0);
        }
        assert_eq!(o.handle.scroll().top, 0, "premise: we got back to the top");
        assert_eq!(o.handle.scroll().badge, 0, "and the badge went with it");
    }

    // ---- ai-monitor-panel-scroll T7 / R5.3-R5.5: the panel's side of the wheel ----

    /// A handle whose published view is a document of `n` equal rungs in one section.
    fn seeded_overlay(n: usize, viewport_h: f32) -> AiMonitorOverlay {
        let mut o = AiMonitorOverlay::detached();
        o.handle = seeded_handle(n, viewport_h);
        o.visible = true;
        o
    }

    fn seeded_handle(n: usize, viewport_h: f32) -> MonitorHandle {
        use renderer::monitor_layout::{Item, ItemKind};
        use renderer::monitor_panel::ScrollView;
        let mut ladder = vec![Item {
            section: 0,
            kind: ItemKind::Heading,
            entry: None,
        }];
        for e in 0..n {
            ladder.push(Item {
                section: 0,
                kind: ItemKind::Row,
                entry: Some(e),
            });
        }
        let heights = vec![10.0; ladder.len()];
        let h = MonitorHandle::default();
        h.publish_view(ScrollView {
            viewport: renderer::monitor_layout::NO_BAND,
            slot: renderer::monitor_layout::NO_BAND,
            ladder,
            heights,
            viewport_h,
        });
        h
    }

    #[test]
    fn r5_3_the_edge_hands_the_wheel_on_but_only_in_the_direction_that_pushes_into_it() {
        // C12, at the seam where it actually decides something. Read literally, "at the end,
        // give the wheel to the grid" means the panel never gets it back — scrolling up
        // leaves it at that same end, so the condition stays true forever. Direction is what
        // makes the hand-off temporary.
        let mut o = seeded_overlay(8, 20.0);
        let max = o.handle.metrics().max_index;
        assert!(max > 0, "premise: this document scrolls");
        o.handle.set_scroll(ScrollState { top: max, badge: 0 });

        assert_eq!(
            o.on_wheel(WheelDelta::Lines(-1.0), 16.0),
            Handled::Passthrough,
            "at the bottom, pushing further down belongs to the grid (D9)"
        );
        assert_eq!(
            o.handle.scroll().top,
            max,
            "and the panel did not move to produce that answer"
        );
        assert_eq!(
            o.on_wheel(WheelDelta::Lines(1.0), 16.0),
            Handled::Consumed,
            "the same position, pushed up, is still the panel's"
        );
        assert!(o.handle.scroll().top < max);
    }

    #[test]
    fn r5_5_a_sub_rung_gesture_is_kept_rather_than_leaked_to_the_grid() {
        // The distinction the `Handled` answer turns on: "did not move because it is still
        // banking" is the panel's event, "did not move because it is at the edge" is the
        // grid's. Getting this wrong makes a slow trackpad drag over the panel scroll the
        // TERMINAL instead — with nothing visibly wrong to point at.
        let mut o = seeded_overlay(8, 20.0);
        assert_eq!(
            o.on_wheel(WheelDelta::Pixels(-4.0), 16.0),
            Handled::Consumed,
            "4px of a 10px rung: kept, banked, nothing drawn yet"
        );
        assert_eq!(o.handle.scroll().top, 0, "and nothing moved");
        assert!(o.wheel_residue != 0.0, "the bank holds it");
        // Completing the rung spends the bank and moves the view.
        assert_eq!(
            o.on_wheel(WheelDelta::Pixels(-8.0), 16.0),
            Handled::Consumed
        );
        assert!(o.handle.scroll().top > 0);
    }

    #[test]
    fn a_hidden_panel_never_takes_the_wheel() {
        // `OverlayHost::wheel` gates on visibility too, but the overlay must not depend on
        // that: a band that is not on screen has nothing to scroll, and claiming the event
        // would make the terminal stop scrolling for no visible reason.
        let mut o = seeded_overlay(8, 20.0);
        o.visible = false;
        assert_eq!(
            o.on_wheel(WheelDelta::Lines(-1.0), 16.0),
            Handled::Passthrough
        );
    }
    use super::{
        agent_pid_moved, apply_rotation, backfill_from_scratch, baseline_from, count_new_above,
        drain_to_newest, hint_or_pane_cwd, hint_or_pane_cwd_with, now_ms, pane_cwd_with,
        proves_ownership, reopen_is_inert, resolve_anchor, rival_refresh_needed, rival_set_of,
        rung_key, session_rotated_with, tick_action, AiMonitorOverlay, Anchor, Attachment,
        OpenedOn, Repoint, Rotation, RungKey, Session, Tick,
    };
    use crate::dashboard;
    use crate::seam::{AppCtx, AppRequest, Handled, KeyInput, Overlay, WriteProbe};
    use crate::tabsession;
    use ai_monitor::telemetry::TelemetrySnapshot;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use winit::keyboard::{Key, ModifiersState, NamedKey};

    // ---- ai-monitor-right-panel R1: Cmd+Shift+M is the ONLY key the panel consumes ----
    //
    // D1 rejected all three of the design's keys, and the rejection is not a preference:
    // the panel never takes focus. It is a painted band, not a focusable view, so a
    // panel-level `Esc` handler fires while the user is in `vim`'s insert mode — the panel
    // closes and they are still inserting — and a `q` handler fires while `less` is paged
    // open. "No terminal keystroke is ever intercepted" is the invariant that matters for
    // a terminal, and one chord that toggles both ways is the only rule that holds it.
    //
    // These drive the REAL `Overlay::on_key`, which is why `detached()` exists: a verdict
    // returned by a trait method is the one decision in this file that cannot be split
    // into a free-standing unit and tested there instead. A test that re-implemented the
    // `is_toggle` branch in its own body would assert nothing about the shipped one.

    fn chord(key: Key, text: Option<&str>, mods: ModifiersState) -> KeyInput {
        KeyInput {
            key,
            text: text.map(str::to_owned),
            mods,
            physical: crate::seam::NO_PHYSICAL,
        }
    }

    /// Cmd+Shift+M.
    fn toggle_chord() -> KeyInput {
        chord(
            Key::Character("m".into()),
            Some("m"),
            ModifiersState::SUPER | ModifiersState::SHIFT,
        )
    }

    /// Deliver one key to the overlay and report the verdict, whether anything reached the
    /// PTY, and what the overlay asked the app to do. The `WriteProbe` is the production
    /// wrapper — `terminal_app` wraps the active tab's writer in exactly this before both
    /// dispatch points — so "the byte reached the writer" is measured the way the app
    /// measures it, not by a stand-in.
    fn deliver(
        overlay: &mut AiMonitorOverlay,
        input: &KeyInput,
        pty: &mut Vec<u8>,
    ) -> (Handled, bool, Vec<AppRequest>) {
        let mut probe = WriteProbe::new(pty);
        let mut ctx = AppCtx::new(&mut probe, 1, 0, None);
        let verdict = overlay.on_key(input, &mut ctx);
        let requests = ctx.take_requests();
        (verdict, probe.wrote(), requests)
    }

    /// What `terminal_app` step 4 would send to the shell for this key, if dispatch let it
    /// through. Reproducing the encoding here would prove nothing, so this calls the real
    /// `encode_key` the default path calls.
    fn to_pty(input: &KeyInput) -> Option<Vec<u8>> {
        crate::input::encode_key(&input.key, input.text.as_deref(), input.mods, false)
    }

    #[test]
    fn r1_1_the_chord_toggles_the_panel_both_ways() {
        // One chord, both directions (D1). A separate close key is what D1 refused, so
        // "opens" without "closes" would leave the panel unclosable by keyboard.
        let mut overlay = AiMonitorOverlay::detached();
        let mut pty = Vec::new();
        assert!(!overlay.visible(), "premise: the panel starts closed");

        let (verdict, wrote, requests) = deliver(&mut overlay, &toggle_chord(), &mut pty);
        assert!(overlay.visible(), "the chord must open a closed panel");
        assert_eq!(
            verdict,
            Handled::Consumed,
            "and must be swallowed — an `m` reaching zsh on every toggle is the bug \
             `Handled::Consumed` exists to prevent"
        );
        assert!(
            !wrote,
            "the toggle is not user input; nothing may reach the PTY"
        );
        assert_eq!(
            requests,
            vec![AppRequest::SetMonitorOpen(true)],
            "the band must be reserved through the deferred path — the overlay cannot \
             touch the Renderer, so a missing request is a panel with no columns"
        );

        let (verdict, _, requests) = deliver(&mut overlay, &toggle_chord(), &mut pty);
        assert!(
            !overlay.visible(),
            "the SAME chord must close an open panel"
        );
        assert_eq!(verdict, Handled::Consumed);
        assert_eq!(
            requests,
            vec![AppRequest::SetMonitorOpen(false)],
            "and must release the band, or the terminal keeps losing the columns"
        );
        assert!(
            pty.is_empty(),
            "no keystroke of a toggle ever reaches the shell"
        );
    }

    #[test]
    fn r1_2_r1_3_esc_and_q_reach_the_pty_while_the_panel_is_open() {
        // THE D1 test. Both keys are delivered with the panel OPEN, which is the state a
        // panel-level close handler would fire in — with it closed, every overlay passes
        // everything through and the assertion would be vacuous.
        let mut overlay = AiMonitorOverlay::detached();
        let mut pty = Vec::new();
        let _ = deliver(&mut overlay, &toggle_chord(), &mut pty);
        assert!(overlay.visible(), "premise: the panel is open");

        // R1.2 — `Esc` while `vim` is in insert mode. The design closes the panel here;
        // D1 does not, because the panel never took focus and the user is still inserting.
        let esc = chord(Key::Named(NamedKey::Escape), None, ModifiersState::empty());
        let (verdict, wrote, requests) = deliver(&mut overlay, &esc, &mut pty);
        assert_eq!(
            verdict,
            Handled::Passthrough,
            "Esc must fall through to the shell — consuming it strands `vim` in insert mode"
        );
        assert!(
            overlay.visible(),
            "and the panel must NOT close underneath the user"
        );
        assert!(
            !wrote,
            "the overlay writes nothing itself; the default path does"
        );
        assert!(requests.is_empty(), "and asks the app for nothing");
        assert_eq!(
            to_pty(&esc).as_deref(),
            Some(&[0x1b][..]),
            "and the byte the default path then sends is ESC itself"
        );

        // R1.3 — `q` while `less` is paging. An ordinary character, and the panel has no
        // claim on it.
        let q = chord(
            Key::Character("q".into()),
            Some("q"),
            ModifiersState::empty(),
        );
        let (verdict, wrote, requests) = deliver(&mut overlay, &q, &mut pty);
        assert_eq!(
            verdict,
            Handled::Passthrough,
            "`q` must reach `less`, which it quits"
        );
        assert!(overlay.visible());
        assert!(!wrote);
        assert!(requests.is_empty());
        assert_eq!(to_pty(&q).as_deref(), Some(&b"q"[..]));

        // Nothing between the overlay and the shell claims either key either: the two
        // interceptors that sit after overlay dispatch in `terminal_app` are the sidebar
        // chord and the tab chords, and a bare `Esc`/`q` is neither. Without this, the
        // passthrough above could be true while the key still never arrived.
        for input in [&esc, &q] {
            assert!(
                !crate::input::is_sidebar_toggle_chord(&input.key, &input.mods),
                "the sidebar chord must not claim a bare terminal key"
            );
            assert!(
                crate::input::tab_action(&input.key, &input.mods).is_none(),
                "nor may a tab chord"
            );
        }
    }

    #[test]
    fn r1_2_r1_3_the_panel_consumes_nothing_but_its_own_chord() {
        // The general form of the two cases above, because D1's promise is about *every*
        // keystroke, not two of them. A close key added for any other key — the design also
        // draws a header `✕`, and `w`/`Backspace`/`Enter` are the usual next guesses —
        // fails here.
        let mut overlay = AiMonitorOverlay::detached();
        let mut pty = Vec::new();
        let _ = deliver(&mut overlay, &toggle_chord(), &mut pty);

        let plain = ModifiersState::empty();
        for input in [
            chord(Key::Named(NamedKey::Escape), None, plain),
            chord(Key::Character("q".into()), Some("q"), plain),
            chord(Key::Character("Q".into()), Some("Q"), ModifiersState::SHIFT),
            chord(Key::Named(NamedKey::Enter), None, plain),
            chord(Key::Named(NamedKey::Backspace), None, plain),
            chord(Key::Named(NamedKey::Space), Some(" "), plain),
            chord(Key::Named(NamedKey::ArrowUp), None, plain),
            // Cmd+M is the design's toggle and macOS's minimise — D1 rejected it, so the
            // panel must not answer to it either.
            chord(Key::Character("m".into()), Some("m"), ModifiersState::SUPER),
            // Shift+M without Cmd is a capital M someone is typing.
            chord(Key::Character("M".into()), Some("M"), ModifiersState::SHIFT),
        ] {
            let (verdict, wrote, requests) = deliver(&mut overlay, &input, &mut pty);
            assert_eq!(
                verdict,
                Handled::Passthrough,
                "{:?} was intercepted by the panel",
                input.key
            );
            assert!(!wrote, "{:?} made the panel write to the PTY", input.key);
            assert!(requests.is_empty(), "{:?} moved the band", input.key);
            assert!(
                overlay.visible(),
                "{:?} closed the panel — only the chord may",
                input.key
            );
        }
    }

    // ---- rival-assignment R6.2: the handle the watcher reads is the one we refresh ----

    #[test]
    fn publish_rivals_stores_the_same_handle_it_hands_to_the_watcher() {
        // REGRESSION. The first implementation built the handle locally in `open()`, moved
        // it into `Watch`, and never assigned `self.rivals` — so `refresh_rivals` wrote
        // into the `Arc` from `new()` that nothing read. Membership stayed frozen for the
        // life of the open, which is precisely the state D11 exists to end: a sibling that
        // starts `claude` after the panel opened never vetoes, and its log can be adopted
        // with `owned: true`, then latched.
        //
        // The suite could not see it — every rival test drove the resolver directly. This
        // drives the REAL wiring function: the handle handed to the watcher and the handle
        // the refresher writes must be one allocation.
        let mut slot: ai_monitor::RivalHandle =
            Arc::new(Mutex::new(ai_monitor::RivalSet::default()));
        let stale = slot.clone(); // stands in for the handle from `new()`
        let hint = tabsession::SessionHint {
            // Not what this test is about: the rival-handle plumbing never reads it.
            pid: None,
            session_id: None,
            started_ms: Some(1_000),
            cwd: None,
            rival_starts: vec![ai_monitor::Rival {
                pid: 42,
                started_ms: 999,
            }],
            rivals_known: true,
        };

        let for_watcher = super::publish_rivals_into(&mut slot, &hint);

        assert!(
            Arc::ptr_eq(&slot, &for_watcher),
            "the stored handle and the watched handle must be one allocation"
        );
        assert!(
            !Arc::ptr_eq(&stale, &for_watcher),
            "a fresh Arc per open, so a previous open's watcher cannot observe this set"
        );

        // A refresh written through the stored slot is visible to the watcher's clone.
        slot.lock().unwrap().list.clear();
        assert!(
            for_watcher.lock().unwrap().list.is_empty(),
            "a refresh must be observable by the watcher"
        );
        assert!(
            !stale.lock().unwrap().known,
            "and the orphaned handle stays at its safe default"
        );
    }

    #[test]
    fn the_watch_carries_the_handle_that_was_stored_not_a_fresh_one() {
        // REGRESSION, one level up from `publish_rivals_stores_...`. Pinning only the
        // publish helper left `open()` free to call it, DISCARD the return, and hand
        // `Watch` a brand-new `Arc` — the identical iteration-2 defect, which the L2
        // checker reproduced with zero tests and zero lints failing.
        //
        // This asserts the pairing `open()` actually relies on: the handle inside the
        // `Watch` must be the same allocation as the one left in the slot.
        let mut slot: ai_monitor::RivalHandle =
            Arc::new(Mutex::new(ai_monitor::RivalSet::default()));
        let hint = tabsession::SessionHint {
            // Not what this test is about: the rival-handle plumbing never reads it.
            pid: None,
            session_id: None,
            started_ms: Some(1_000),
            cwd: None,
            rival_starts: vec![ai_monitor::Rival {
                pid: 7,
                started_ms: 999,
            }],
            rivals_known: true,
        };

        let for_watch = super::publish_rivals_into(&mut slot, &hint);
        let watch = super::watch_with_rivals(
            for_watch,
            PathBuf::from("/tmp/does-not-matter"),
            None,
            ai_monitor::Wanted::default(),
            false,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );

        // The handle now lives inside the scope enum (AG7). Reaching through it rather than
        // matching loosely keeps this test failing for the reason it was written — a
        // *different* `Arc` — instead of passing on a wrong-variant watch.
        let ai_monitor::WatchScope::Project { rivals, .. } = &watch.scope else {
            panic!("a Claude watch must be built with WatchScope::Project");
        };

        assert!(
            Arc::ptr_eq(&slot, rivals),
            "the watcher must read the handle refresh_rivals writes"
        );

        // And a membership change through the slot is visible to the Watch's copy.
        slot.lock().unwrap().list.clear();
        assert!(
            rivals.lock().unwrap().list.is_empty(),
            "a refresh must reach the watcher, or D11 is inert"
        );
    }

    // ---- ai-monitor-antigravity T4: which attachment path an open takes (R0.1) ----

    /// Only Antigravity takes the `SingleLog` path — every other kind, and no agent at all,
    /// falls to the Claude path.
    ///
    /// Enumerated over ALL of `AgentKind` on purpose. The routing is a match on one variant
    /// out of five, so the interesting failure is not "Antigravity stopped working" but "a
    /// newly-implemented agent silently kept using Claude's resolver" — which no test of the
    /// Antigravity case alone would catch.
    #[test]
    fn only_an_antigravity_tab_takes_the_single_log_path() {
        use crate::tabmeta::AgentKind::*;

        assert_eq!(
            super::antigravity_pid(Some((Antigravity, 4242))),
            Some(4242),
            "an agy tab must route to the Antigravity path, carrying its pid for the FD probe"
        );

        for other in [Claude, Codex, Gemini, Aider] {
            assert_eq!(
                super::antigravity_pid(Some((other, 4242))),
                None,
                "{other:?} has no Antigravity implementation and must not take that path"
            );
        }

        assert_eq!(
            super::antigravity_pid(None),
            None,
            "a tab running no agent at all keeps the Claude path's explained empty state"
        );
    }

    // ---- tmux-pane-cwd: the overlay's own cwd resolution (R3.1, R3.3) ----

    // ---- tmux-pane-cwd D6: the re-point trigger (R4.2, R4.3) ----
    //
    // These drive `Repoint` through the SAME transition `open` performs — `opened(notified,
    // resolved, pid)` then `changed(...)` — so the key selection is under test. An earlier
    // version of these tests called a free predicate with hand-typed arguments and passed
    // against the buggy call site; that is exactly the hole this shape closes.

    const CLIENT: &str = "/Users/u"; // what the app notifies with (tab shell = tmux client)
    const PANE: &str = "/Users/u/proj"; // what `pane_cwd` resolves to under tmux

    #[test]
    fn repeated_identical_notifications_never_restart_the_watcher() {
        // REGRESSION for the re-open storm. Cold start under tmux: the app notifies with the
        // CLIENT cwd (the worker has not published yet), while `open` resolves the PANE cwd.
        // If `opened` keys on the resolved value, every subsequent identical notification looks
        // like a change and re-opens forever — and the overlay's own watcher supplies the wakes.
        let mut r = Repoint::default();
        let opened_on = r.opened(Some(PathBuf::from(CLIENT)), Some(8060), None, |_, _| {
            Some(PathBuf::from(PANE))
        });
        // D12: the panel opens on the pane, not the client — that part must not regress either.
        assert_eq!(
            opened_on,
            Some(PathBuf::from(PANE)),
            "must open on the resolved pane cwd"
        );
        // ...and the key must nonetheless be the CLIENT cwd, so repeats are inert.
        for i in 0..5 {
            assert!(
                !r.changed(true, Some(Path::new(CLIENT)), Some(8060)),
                "wake {i}: an unchanged notification must not re-open (the storm)"
            );
        }
    }

    #[test]
    fn a_changed_cwd_repoints_an_open_overlay() {
        // R4.2 — the case the trigger exists for: a tmux window switch moves the pane while the
        // tab is unchanged, so the published cwd changes under a stable shell pid.
        let mut r = Repoint::default();
        r.opened(Some(PathBuf::from("/proj/a")), Some(99), None, |_, _| None);
        assert!(r.changed(true, Some(Path::new("/proj/b")), Some(99)));
        // A genuine tab switch (both change) still re-points.
        assert!(r.changed(true, Some(Path::new("/proj/b")), Some(100)));
        // Two tabs in the SAME directory are still different sessions.
        assert!(
            r.changed(true, Some(Path::new("/proj/a")), Some(100)),
            "same cwd, different tab must re-point — this is what tab binding is for"
        );
    }

    #[test]
    fn an_unchanged_cwd_is_a_no_op() {
        // R4.3 — load-bearing for cost: D6 fires this after EVERY metadata sync.
        let mut r = Repoint::default();
        r.opened(Some(PathBuf::from("/proj/a")), Some(99), None, |_, _| None);
        assert!(!r.changed(true, Some(Path::new("/proj/a")), Some(99)));

        let mut empty = Repoint::default();
        empty.opened(None, None, None, |_, _| None);
        assert!(!empty.changed(true, None, None));
    }

    #[test]
    fn a_hidden_overlay_never_repoints() {
        // R4.3 — a closed panel holds no watcher and must not spin one up.
        let mut r = Repoint::default();
        r.opened(Some(PathBuf::from("/proj/a")), Some(99), None, |_, _| None);
        assert!(!r.changed(false, Some(Path::new("/proj/b")), Some(1)));
        assert!(!r.changed(false, None, None));
    }

    // ---- tabsession-pane-cwd-gate: the hint supplies the directory (R5) ----

    /// A hint carrying `cwd`, which is the only field these tests read.
    fn hint_at(cwd: Option<tabsession::HintCwd>) -> tabsession::SessionHint {
        tabsession::SessionHint {
            pid: Some(4242),
            started_ms: Some(1_000),
            cwd,
            ..Default::default()
        }
    }

    #[test]
    fn both_hint_cwd_variants_supply_the_directory_only_none_falls_back() {
        // R5.3 / R5.5 — and the counter is the point, not decoration.
        //
        // `Disputed` must NOT fall back: its payload is the registry's path, and routing it to
        // `pane_cwd` would open the panel on a THIRD directory — the pane shell's — making tier
        // 1's exact-filename lookup miss and leaving the payload read by nobody. An earlier
        // draft of this rule did exactly that.
        let calls = std::cell::Cell::new(0usize);
        let pane = |_: Option<u32>| {
            calls.set(calls.get() + 1);
            Some(PathBuf::from(PANE))
        };

        for variant in [
            tabsession::HintCwd::Proven(PathBuf::from("/proj/a")),
            tabsession::HintCwd::Disputed(PathBuf::from("/proj/a")),
        ] {
            calls.set(0);
            let h = hint_at(Some(variant.clone()));
            assert_eq!(
                hint_or_pane_cwd_with(pane, Some(&h), Some(8060)),
                Some(PathBuf::from("/proj/a")),
                "{variant:?} names the directory itself"
            );
            assert_eq!(
                calls.get(),
                0,
                "R5.5: a tab whose hint has a directory must not pay a second tmux resolve"
            );
        }

        // Only a hint with NO directory at all reaches the pane — and so does no hint.
        for h in [Some(hint_at(None)), None] {
            calls.set(0);
            assert_eq!(
                hint_or_pane_cwd_with(pane, h.as_ref(), Some(8060)),
                Some(PathBuf::from(PANE))
            );
            assert_eq!(calls.get(), 1);
        }
    }

    #[test]
    fn refresh_rivals_recovers_under_tmux() {
        // R4.3 — the second victim of the deleted gate, and the one nothing could see. Under
        // tmux the gate made `hint_for` answer `None` on every refresh, so this path published
        // `RivalSet::default()` for the life of the panel: `known: false`, the uniqueness
        // rescue permanently withheld, "a claude appeared" never reported. The fix is entirely
        // in `hint_for` — so what must be pinned HERE is that a `Some` hint's rival data
        // actually survives the mapping rather than being flattened on the way to the handle.
        let h = tabsession::SessionHint {
            pid: Some(74922),
            started_ms: Some(1_000),
            cwd: Some(tabsession::HintCwd::Proven(PathBuf::from("/proj/a"))),
            rival_starts: vec![ai_monitor::Rival {
                pid: 55502,
                started_ms: 999,
            }],
            rivals_known: true,
            ..Default::default()
        };
        let set = rival_set_of(Some(h));
        assert_eq!(
            set.list.iter().map(|r| r.pid).collect::<Vec<_>>(),
            vec![55502]
        );
        assert!(
            set.known,
            "a complete enumeration must publish known:true, or the rescue stays withheld \
             forever — which is exactly what the gate used to cause"
        );

        // The failure direction must NOT drift: no hint means we did not look, and that has to
        // withhold the rescue rather than grant it on the strength of a read that did not work.
        let empty = rival_set_of(None);
        assert!(empty.list.is_empty());
        assert!(!empty.known, "a failed lookup is never known-empty");

        // ...and a `Some` hint that itself came back unknown stays unknown.
        let unknown = rival_set_of(Some(tabsession::SessionHint {
            pid: Some(74922),
            rivals_known: false,
            ..Default::default()
        }));
        assert!(!unknown.known);
    }

    #[test]
    fn the_repoint_key_stays_the_notified_cwd_when_the_hint_names_another() {
        // R5.4 — THE storm assertion, re-run for the case this feature creates. The resolved
        // directory is now the chosen `claude`'s, which under tmux routinely differs from the
        // notified one; if `opened` keyed on what it returned, every subsequent identical
        // notification would look like a change and re-open forever.
        let mut r = Repoint::default();
        let h = hint_at(Some(tabsession::HintCwd::Proven(PathBuf::from(PANE))));
        let opened_on = r.opened(
            Some(PathBuf::from(CLIENT)),
            Some(8060),
            Some(&h),
            hint_or_pane_cwd,
        );
        assert_eq!(
            opened_on,
            Some(PathBuf::from(PANE)),
            "the panel opens on the claude's own directory"
        );
        for i in 0..5 {
            assert!(
                !r.changed(true, Some(Path::new(CLIENT)), Some(8060)),
                "wake {i}: the key is the NOTIFIED cwd, so repeats stay inert"
            );
        }
    }

    /// This file's production half, comments stripped — the same split
    /// `a_rotation_re_points_rather_than_refreshing_rivals` documents at length.
    fn production_half() -> String {
        let src = include_str!("overlay.rs");
        let (production, tests) = src
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("no `mod tests` marker to split on");
        assert!(
            !tests.is_empty() && production.len() > src.len() / 4,
            "the split found the marker in an implausible place"
        );
        production
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // ---- tabsession-pane-cwd-gate D10: the idempotence early-out (R7) ----

    const DIR: &str = "/proj/a";
    const CLAUDE_PID: u32 = 74922;
    const SESSION: &str = "2b1ca0f2-8a99-4aff-90f1-115839f17f01";

    fn baseline(session_id: Option<&str>) -> OpenedOn {
        OpenedOn {
            pid: CLAUDE_PID,
            process_start_ms: 1_785_449_383_000,
            session_id: session_id.map(str::to_string),
        }
    }

    /// A hint that resolves to exactly the state `baseline(Some(SESSION))` was opened for.
    fn same_hint() -> tabsession::SessionHint {
        tabsession::SessionHint {
            pid: Some(CLAUDE_PID),
            session_id: Some(SESSION.to_string()),
            started_ms: Some(1_785_449_383_000),
            cwd: Some(tabsession::HintCwd::Proven(PathBuf::from(DIR))),
            ..Default::default()
        }
    }

    #[test]
    fn a_pane_cd_that_does_not_move_claude_keeps_the_watcher() {
        // R7.1 — the case D10 exists for. `Repoint` keys on the NOTIFIED cwd (the pane shell's,
        // republished on every `cd`), so `changed()` fires and `open()` re-runs; everything it
        // resolves is identical, so the teardown, respawn and full backfill are pure waste.
        assert!(reopen_is_inert(
            true,
            true,
            Some(Path::new(DIR)),
            Some(&baseline(Some(SESSION))),
            Some(Path::new(DIR)),
            &same_hint(),
        ));
    }

    #[test]
    fn a_clear_rotation_still_re_points() {
        // R7.2 — condition (3), and it is not belt-and-braces. A `/clear` rotation is by
        // definition SAME directory, SAME pid, NEW session id, which is exactly what conditions
        // 1 and 2 compare equal: a two-value early-out would silently discard the re-point
        // `ai-monitor-session-rotation` shipped to add.
        let mut rotated = same_hint();
        rotated.session_id = Some("11111111-2222-3333-4444-555555555555".into());
        assert!(!reopen_is_inert(
            true,
            true,
            Some(Path::new(DIR)),
            Some(&baseline(Some(SESSION))),
            Some(Path::new(DIR)),
            &rotated,
        ));
        // And the arming direction — a baseline that named nothing, now naming something — is a
        // real change too, so it must not be swallowed either.
        assert!(!reopen_is_inert(
            true,
            true,
            Some(Path::new(DIR)),
            Some(&baseline(None)),
            Some(Path::new(DIR)),
            &same_hint(),
        ));
    }

    #[test]
    fn the_toggle_chord_still_shows_the_panel() {
        // R7.3 — condition (4). `close()` drops the watcher and clears `visible` but leaves
        // `opened_on`, `repoint` and the session intact, so conditions 1-3 all still match on an
        // unchanged tab. Without (4), Cmd+Shift+M off then on would return early and leave the
        // panel invisible with no watcher until the user switched tabs.
        for (visible, has_watcher) in [(false, false), (false, true), (true, false)] {
            assert!(
                !reopen_is_inert(
                    visible,
                    has_watcher,
                    Some(Path::new(DIR)),
                    Some(&baseline(Some(SESSION))),
                    Some(Path::new(DIR)),
                    &same_hint(),
                ),
                "visible={visible} watcher={has_watcher}: there is no live watcher to keep"
            );
        }
    }

    #[test]
    fn a_real_move_is_never_absorbed() {
        // The other half of R7: everything the early-out must NOT swallow.
        let elsewhere = Path::new("/proj/b");
        // A different directory — the tab moved.
        assert!(!reopen_is_inert(
            true,
            true,
            Some(Path::new(DIR)),
            Some(&baseline(Some(SESSION))),
            Some(elsewhere),
            &same_hint(),
        ));
        // A different `claude` in the same pane (D5's trigger, one level down).
        let mut new_pid = same_hint();
        new_pid.pid = Some(CLAUDE_PID + 1);
        assert!(!reopen_is_inert(
            true,
            true,
            Some(Path::new(DIR)),
            Some(&baseline(Some(SESSION))),
            Some(Path::new(DIR)),
            &new_pid,
        ));
        // No baseline at all — the `opened_on: None` class D10 declares itself unavailable for,
        // because condition 2 has nothing to compare.
        assert!(!reopen_is_inert(
            true,
            true,
            Some(Path::new(DIR)),
            None,
            Some(Path::new(DIR)),
            &same_hint(),
        ));
        // And a tab with no `claude` at all can never match, whatever else agrees.
        assert!(!reopen_is_inert(
            true,
            true,
            Some(Path::new(DIR)),
            Some(&baseline(Some(SESSION))),
            Some(Path::new(DIR)),
            &tabsession::SessionHint::default(),
        ));
    }

    #[test]
    fn the_early_out_returns_only_after_repoint_has_re_keyed() {
        // R7.4 — a source-text pin, because the hazard is an ORDERING inside `open()` and its
        // failure mode is silent: D10's motivating case is a CHANGED notified cwd (that is why
        // `changed()` fired), so returning before `key_cwd` was updated leaves `changed()` true
        // on every subsequent wake and re-enters `open()` forever. This file has shipped that
        // exact bug once.
        let p = production_half()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();
        let (before, after) = p
            .split_once("ifreopen_is_inert(")
            .expect("open() must consult the early-out");
        assert!(
            before.contains(".repoint.opened(cwd,shell_pid,h.as_ref(),hint_or_pane_cwd);"),
            "R7.4: `Repoint::opened` must have re-keyed BEFORE the early-out can return"
        );
        // ...and what it returns before must genuinely be the teardown, not a no-op.
        assert!(
            after.contains("self.session.repoint();") && after.contains("self.watcher=None;"),
            "the early-out must sit above the teardown it exists to skip"
        );
    }

    #[test]
    fn open_resolves_the_hint_first_through_a_named_resolver() {
        // R5.1 / R5.2 — a source-text pin, because BOTH properties live at the `open()` call
        // site and a unit test there types its own arguments (the note above this section
        // records an earlier version doing exactly that and passing against a buggy call site).
        //
        // The hazard is specific: deriving the directory from the hint puts a second candidate
        // back inside reach of the `opened` call, so `|_, _| <captured cwd>` becomes a
        // legitimate spelling again — a type-checking, one-token reintroduction of the storm.
        // Passing the resolver BY NAME is what removes the capture scope, and that is the
        // property being pinned here, not merely the ordering.
        // ALL whitespace stripped, so these pin the calls rather than rustfmt's line breaking —
        // this file's other pins have been broken by re-wrapping before.
        let p = production_half()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();
        let (before, after) = p
            .split_once("letcwd=self")
            .expect("open() must still resolve a directory through `self.repoint`");
        assert!(
            before.contains("leth=shell_pid.and_then(tabsession::hint_for);"),
            "R5.1: the hint must be resolved ABOVE the directory — it is what supplies it"
        );
        assert!(
            after.starts_with(".repoint.opened(cwd,shell_pid,h.as_ref(),hint_or_pane_cwd);"),
            "R5.2: the resolver must be passed BY NAME, never wrapped in a closure — a closure \
             has a capture scope and `|_, _| cwd` becomes writable again. Found: {:?}",
            &after[..after.len().min(90)]
        );
        assert!(
            p.contains(
                "resolve:implFnOnce(Option<&tabsession::SessionHint>,Option<u32>)->Option<PathBuf>,"
            ),
            "R5.2: the resolver parameter carries the hint, so a transposition with \
             `Option<PathBuf>` is not even expressible"
        );
        // And the suppression must be expressed on `Wanted`, never by blanking the hint.
        assert!(
            p.contains(
                "since_ms:matchhint.cwd.as_ref().is_some_and(tabsession::HintCwd::tier2_ok)"
            ),
            "tier 2 must be gated on the directory's provenance at the point `Wanted` is built"
        );
    }

    #[test]
    fn with_no_tmux_the_key_and_the_opened_directory_agree() {
        // A non-tmux tab resolves nothing, so both the key and the directory are the notified
        // value — and the first published cwd equal to it is correctly inert.
        let mut r = Repoint::default();
        let opened_on = r.opened(Some(PathBuf::from(CLIENT)), Some(4242), None, |_, _| None);
        assert_eq!(opened_on, Some(PathBuf::from(CLIENT)));
        assert!(!r.changed(true, Some(Path::new(CLIENT)), Some(4242)));
    }

    #[test]
    fn a_tmux_tab_opens_on_the_pane_not_the_client() {
        // R3.1 — the reported bug, in miniature. The tab's shell sits in `~` while its tmux
        // pane is in a worktree; without the re-root `project_dir` resolves
        // `~/.claude/projects/-Users-<u>/`, which does not exist, and every dashboard row
        // renders empty.
        let got = pane_cwd_with(
            |shell| {
                assert_eq!(shell, 8060);
                7503 // the pane's shell, a child of the tmux *server*
            },
            |root| {
                assert_eq!(root, 7503, "the cwd must come from the pane, not the tab");
                Some(PathBuf::from("/Users/u/IdeaProjects/worktree"))
            },
            Some(8060),
        );
        assert_eq!(got, Some(PathBuf::from("/Users/u/IdeaProjects/worktree")));
    }

    #[test]
    fn no_tmux_yields_none_so_the_caller_keeps_its_own_cwd() {
        // R3.3 — `resolved_root` returns the input pid both when there is no tmux and on every
        // failure (timeout, non-zero exit, unparseable output), so the two are one code path:
        // yield `None` and let `open` fall back to the cwd it was passed. The cwd resolver
        // must not even be called — there is nothing to resolve.
        let got = pane_cwd_with(
            |shell| shell, // no re-root: no tmux, or the lookup failed
            |_| panic!("must not resolve a cwd when there is no re-root"),
            Some(4242),
        );
        assert_eq!(got, None);
    }

    #[test]
    fn a_pane_that_died_between_resolve_and_read_yields_none() {
        // R3.3 / R5.4 — the pane re-rooted fine but its cwd read failed (it exited in the
        // window between). Degrade to the passed cwd rather than panicking.
        let got = pane_cwd_with(|_| 7503, |_| None, Some(8060));
        assert_eq!(got, None);
    }

    #[test]
    fn a_tab_with_no_shell_pid_resolves_nothing() {
        // R5.4 — a tab whose pid is unavailable must not reach either syscall.
        let got = pane_cwd_with(
            |_| panic!("must not resolve without a pid"),
            |_| panic!("must not read a cwd without a pid"),
            None,
        );
        assert_eq!(got, None);
    }

    fn snap_at(last_event_ms: Option<u64>) -> TelemetrySnapshot {
        TelemetrySnapshot {
            session_id: Some("abcd1234".into()),
            model: Some("claude-opus-5".into()),
            total_output: 4_200,
            last_event_ms,
            ..TelemetrySnapshot::default()
        }
    }

    #[test]
    fn a_log_quiet_since_before_the_window_opened_does_not_attach() {
        // The originally reported bug: a fresh window must not adopt the session that
        // happened to write here last.
        let launched = 1_785_226_000_000;
        assert!(!proves_ownership(&snap_at(Some(launched - 1)), launched));
    }

    #[test]
    fn a_log_written_since_the_window_opened_attaches() {
        let launched = 1_785_226_000_000;
        assert!(
            proves_ownership(&snap_at(Some(launched)), launched),
            "boundary"
        );
        assert!(proves_ownership(&snap_at(Some(launched + 1)), launched));
    }

    #[test]
    fn an_undated_log_does_not_attach() {
        // Absence of a clock is not evidence of freshness, and the watcher keeps
        // looking, so answering "no" costs nothing.
        let launched = 1_785_226_000_000;
        assert!(!proves_ownership(&snap_at(None), launched));
    }

    /// A snapshot naming a session, at `last_event_ms`.
    fn snap_of(session: &str, last_event_ms: Option<u64>) -> TelemetrySnapshot {
        TelemetrySnapshot {
            session_id: Some(session.into()),
            ..snap_at(last_event_ms)
        }
    }

    #[test]
    fn an_empty_snapshot_never_replaces_an_attached_panel() {
        // THE requirement, at the last place it can be violated: `compose` cannot
        // blank an attached panel, but it can only render what it is handed. The
        // watcher emits an empty snapshot when a log is truncated under it.
        let launched = 1_785_226_000_000;
        let mut a = Attachment::default();
        assert!(a.admit(&snap_of("s1", Some(launched + 1)), launched, false));
        assert!(a.attached);
        assert!(
            !a.admit(&TelemetrySnapshot::default(), launched, false),
            "dropped — the data on screen survives"
        );

        // Unattached there is nothing to lose, and this is how the empty frame renders.
        let mut b = Attachment::default();
        assert!(b.admit(&TelemetrySnapshot::default(), launched, false));
    }

    #[test]
    fn a_different_session_never_replaces_an_attached_panel() {
        // Telling the watcher to stop re-resolving is not enough on its own: it reads
        // that flag at the top of its callback, so a callback already in flight can
        // finish a switch just after we attach. The reader has to be able to reject
        // the result.
        let launched = 1_785_226_000_000;
        let mut a = Attachment::default();
        assert!(a.admit(&snap_of("mine", Some(launched + 1)), launched, false));
        assert!(
            !a.admit(&snap_of("rival", Some(launched + 9_000)), launched, false),
            "a rival session's data must not take the panel"
        );
        assert!(
            a.admit(&snap_of("mine", Some(launched + 20_000)), launched, false),
            "our own later snapshots still flow"
        );
    }

    #[test]
    fn a_confirmed_session_displaces_an_unconfirmed_one() {
        // The `owned: false` half of the attach rule, kept under test. This once
        // described the reported case — a panel attaching to another application's
        // live session in the same project directory, then freezing on it — but
        // strict resolution makes that path unreachable: an unconfirmed session is
        // never resolved, so production never builds an unconfirmed attachment to
        // displace. What this now pins is the contract for a caller using the ranked
        // `resolve_session` fallback: an unconfirmed attachment yields to a confirmed
        // one. The assertions are unchanged.
        let launched = 1_785_226_000_000;
        let mut a = Attachment::default();
        assert!(a.admit(&snap_of("warp", Some(launched + 1)), launched, false));
        assert!(a.attached && !a.owned, "attached, but only as a guess");

        assert!(
            a.admit(&snap_of("mine", Some(launched + 2)), launched, true),
            "the tab's own session takes the panel"
        );
        assert!(a.owned);
        assert_eq!(a.session_id.as_deref(), Some("mine"));
    }

    #[test]
    fn a_confirmed_attachment_yields_to_nothing() {
        // The other direction: once confirmed, neither a rival session nor an empty
        // snapshot can take it — the data-preservation guarantee is strongest here.
        let launched = 1_785_226_000_000;
        let mut a = Attachment::default();
        assert!(a.admit(&snap_of("mine", Some(launched + 1)), launched, true));
        assert!(a.owned);
        assert!(
            !a.admit(&snap_of("rival", Some(launched + 9_000)), launched, false),
            "an unconfirmed rival cannot displace a confirmed attachment"
        );
        assert!(
            !a.admit(&TelemetrySnapshot::default(), launched, true),
            "nor can an empty snapshot, however it is labelled"
        );
        assert_eq!(a.session_id.as_deref(), Some("mine"));
    }

    #[test]
    fn attachment_is_monotonic_within_an_open() {
        // `last_event_ms` is last-seen, not a maximum, so one out-of-order timestamp
        // must not revoke an attachment and blank the panel. This drives the REAL
        // rule — writing the `||` in the test body would only assert the test.
        let launched = 1_785_226_000_000;
        let mut a = Attachment::default();
        for last in [Some(launched + 5_000), Some(launched - 900_000), None] {
            a.admit(&snap_of("mine", last), launched, false);
            assert!(a.attached, "never revoked once granted (last={last:?})");
        }
    }

    #[test]
    fn nothing_attaches_from_a_log_that_predates_the_window() {
        // ... and an unattached overlay keeps accepting snapshots, so it can still
        // find the right session.
        let launched = 1_785_226_000_000;
        let mut a = Attachment::default();
        assert!(a.admit(&snap_of("old", Some(launched - 1)), launched, false));
        assert!(!a.attached);
        assert!(a.admit(&snap_of("new", Some(launched)), launched, false));
        assert!(a.attached);
    }

    #[test]
    fn a_confirmed_session_attaches_however_stale_it_is() {
        // R5.1 / D4 — a tab running `claude --resume <id>` on a session that went
        // idle before the panel opened. Freshness answers "no" about the user's OWN
        // session, and the panel would report `(no active session — newest log idle
        // 12m)` about data it has positively identified. Confirmation decides.
        let launched = 1_785_226_000_000;
        let stale = snap_of("mine", Some(launched - 720_000)); // idle 12 minutes
        assert!(
            !proves_ownership(&stale, launched),
            "premise: the freshness gate says no about this very log"
        );

        let mut a = Attachment::default();
        assert!(a.admit(&stale, launched, true));
        assert!(a.attached, "a confirmed, non-empty log attaches on content");
        assert!(a.owned);
        assert_eq!(a.session_id.as_deref(), Some("mine"));
    }

    #[test]
    fn a_confirmed_but_empty_session_does_not_attach() {
        // R5.2 / D4 — the emptiness condition, which is the whole of what keeps the
        // watcher self-correcting. `note()` runs in `open()` BEFORE `LogWatcher::spawn`
        // and publishes this decision to the flag the watch thread reads, so attaching
        // on an owned-but-empty backfill would freeze the watcher at birth *and* latch
        // `session_id = None`, making "a different session never replaces an owned
        // attachment" true only vacuously.
        let launched = 1_785_226_000_000;
        let mut a = Attachment::default();
        assert!(a.admit(&TelemetrySnapshot::default(), launched, true));
        assert!(
            !a.attached,
            "confirmed but with nothing in it: the flag stays unset and the watcher \
             stays free to re-resolve"
        );

        // The first appended line re-emits, and attaches — stale though it is.
        assert!(a.admit(&snap_of("mine", Some(launched - 1)), launched, true));
        assert!(a.attached);
        assert_eq!(a.session_id.as_deref(), Some("mine"));
    }

    #[test]
    fn an_attachment_holding_no_identity_adopts_the_first_one_offered() {
        // R5.3 / D4b — non-emptiness alone does not restore the identity guard.
        // `session_id` is filled only from a `sessionId` field on a folded line, while
        // non-emptiness can come from any field at all, so a confirmed log can attach
        // holding `None` — after which every comparison falls through `_ => true`.
        let launched = 1_785_226_000_000;
        let anonymous = TelemetrySnapshot {
            session_id: None,
            ..snap_of("names-nothing", Some(launched - 1))
        };
        let mut a = Attachment::default();
        assert!(a.admit(&anonymous, launched, true));
        assert!(
            a.attached && a.session_id.is_none(),
            "premise: attached, with no identity to compare against"
        );

        assert!(a.admit(&snap_of("abc", Some(launched + 1)), launched, true));
        assert_eq!(
            a.session_id.as_deref(),
            Some("abc"),
            "the gap is filled the first time a snapshot names a session"
        );
        // And from here the guard means something rather than admitting everything.
        assert!(
            !a.admit(&snap_of("rival", Some(launched + 2)), launched, true),
            "once an identity is held, a differently-named session is refused"
        );
    }

    #[test]
    fn an_identity_already_held_is_never_replaced() {
        // R5.4 / D4b — adoption fills a gap; it never overwrites. Without this the
        // adopt arm would quietly become a rename of the rule it exists to shore up.
        let launched = 1_785_226_000_000;
        let mut a = Attachment::default();
        assert!(a.admit(&snap_of("mine", Some(launched - 1)), launched, true));
        assert_eq!(a.session_id.as_deref(), Some("mine"));

        assert!(
            !a.admit(&snap_of("other", Some(launched + 9_000)), launched, true),
            "confirmed or not, a different session cannot take a held identity"
        );
        assert_eq!(a.session_id.as_deref(), Some("mine"));
    }

    #[test]
    fn poll_coalesces_a_burst_to_the_newest_snapshot() {
        // R4.1 — three batches queued while the UI thread was busy must produce ONE
        // model write, from the last. Uses a plain std channel: no `insta`, so no
        // dev-dependency and `crates/app/Cargo.toml` stays out of this surface.
        let (tx, rx) = mpsc::channel::<u32>();
        for v in [1, 2, 3] {
            tx.send(v).unwrap();
        }
        assert_eq!(
            drain_to_newest(&rx),
            Some(3),
            "newest wins, earlier are stale"
        );
        assert_eq!(drain_to_newest(&rx), None, "channel fully drained");
    }

    #[test]
    fn poll_reports_no_change_on_an_empty_channel() {
        // R4.2 — nothing queued must not rewrite the model or request a redraw.
        let (_tx, rx) = mpsc::channel::<u32>();
        assert_eq!(drain_to_newest(&rx), None);
    }

    // ---- ai-monitor-right-panel R10: one panel per window, re-pointed on tab switch ----
    //
    // D13 carries the attachment rule forward from `overlay-tab-binding` and
    // `ai-monitor-session-isolation` UNCHANGED; docking changed where the panel is painted,
    // not what it is attached to. What is new is D3's cumulative counters, and they are
    // exactly the kind of state a re-point can leak: the display rings are capped, so a
    // stale ring looks plausible, while `total_tool_calls` and the per-name tally are
    // session totals that would read as tab B's history while being tab A's.
    //
    // These drive `Session` — the value `open()` throws away and rebuilds — because
    // `AiMonitorOverlay` needs an `EventLoopProxy` and cannot be constructed under
    // `cargo test`. That is the same reason `Repoint` and `Attachment` are structs.

    const LAUNCHED: u64 = 1_785_226_000_000;

    /// A snapshot for `session`, backed by `calls` cumulative `Bash` calls (D3).
    fn tallied(session: &str, calls: u64) -> Arc<TelemetrySnapshot> {
        Arc::new(TelemetrySnapshot {
            session_id: Some(session.into()),
            tool_calls: vec![("Bash".into(), calls)],
            total_tool_calls: calls,
            failed_tool_calls: calls / 10,
            ..snap_at(Some(LAUNCHED + 1))
        })
    }

    /// What the panel would render as the session total — `dashboard::model` reads exactly
    /// this slot, so `None` is an empty panel and `Some` is whatever it holds.
    fn shown_total(s: &Session) -> Option<u64> {
        s.latest.as_ref().map(|snap| snap.total_tool_calls)
    }

    #[test]
    fn a_repoint_shows_the_new_tabs_counters_and_never_the_old_tabs() {
        // R10.3 — THE leak test. Tab A has 200 `Bash` calls on the panel; the user switches
        // to tab B, whose log holds 3. Both failure modes below were run as mutations and
        // both are silent in production, because the panel still looks like a dashboard —
        // the display rings are capped, so a stale ring reads as plausible, while
        // `total_tool_calls` and the per-name tally are session totals shown under the
        // wrong tab's heading:
        //
        // - `repoint()` made a no-op: the panel keeps rendering tab A's 200 forever, since
        //   tab A's identity also survives in `attach` and `Attachment::admit` then refuses
        //   tab B by the `(Some(mine), Some(theirs))` arm. Caught at the first assert after
        //   the switch.
        // - `repoint()` reduced to `self.latest = None`: tab A's identity survives alone,
        //   so every one of tab B's snapshots is refused and the panel stays permanently
        //   empty on a tab that has data. Caught at the `attached` assert below.
        let mut s = Session::default();

        s.note(tallied("tab-a", 200), LAUNCHED, true);
        assert_eq!(shown_total(&s), Some(200), "premise: tab A is on the panel");
        assert!(s.attach.attached && s.attached_flag.load(Ordering::Relaxed));
        let flag_a = s.attached_flag.clone();

        s.repoint(); // the user switches tabs; `open()` does this above every branch

        assert_eq!(
            shown_total(&s),
            None,
            "tab A's counters leave with tab A — nothing is on the panel until tab B folds"
        );
        assert!(
            !s.attach.attached,
            "and no attachment survives to refuse tab B"
        );
        assert!(
            !Arc::ptr_eq(&flag_a, &s.attached_flag),
            "a FRESH flag, not a store: tab A's watcher may still be running for an \
             instant and must not observe this open's decision"
        );

        s.note(tallied("tab-b", 3), LAUNCHED, true);
        assert_eq!(
            shown_total(&s),
            Some(3),
            "tab B's log only: 203 would be an accumulated counter, 200 a refused re-point"
        );
        assert_eq!(
            s.latest.as_ref().and_then(|snap| snap.tool_calls.first()),
            Some(&("Bash".to_string(), 3)),
            "and the per-name tally is tab B's too, not a merge of the two"
        );
    }

    #[test]
    fn repointing_to_a_tab_with_no_log_empties_the_panel() {
        // R10.3, the case that makes the *placement* of the reset load-bearing. `open()`
        // calls `repoint()` ABOVE the `project_dir` branch, so a tab with no resolvable log
        // clears the previous tab and then notes nothing. Move the reset inside the branch
        // and tab A's 200 calls stay on screen with no watcher running and nothing that can
        // ever replace them — the panel would claim a session the tab does not have.
        let mut s = Session::default();
        s.note(tallied("tab-a", 200), LAUNCHED, true);

        s.repoint();
        // `open()`'s no-log arm: an empty accumulator, and no `note` (it lives in the
        // branch), so the slot stays empty.
        let (telemetry, offset) = backfill_from_scratch(None);
        assert_eq!(offset, 0, "nothing folded, so nothing to skip");
        assert_eq!(
            telemetry.snapshot(),
            TelemetrySnapshot::default(),
            "a tab with no log folds an EMPTY accumulator, never the previous tab's"
        );
        assert_eq!(shown_total(&s), None, "and the panel shows that emptiness");
    }

    #[test]
    fn each_open_folds_the_new_tabs_log_into_a_fresh_accumulator() {
        // R10.3's other half, against real files and the real fold. Two logs, two opens:
        // the second must report its own log's totals, so a `backfill_from_scratch` that
        // reused or continued an accumulator reads 203 here.
        //
        // The offset assertions are the no-double-count half. The pair this function
        // returns is a contract — a fresh accumulator plus the byte count already folded
        // into it — and `LogWatcher::spawn` resumes its tail at that offset. Passing 0
        // instead re-reads the whole file and folds every line twice: the rings would still
        // look right (they are capped) while `total_tool_calls` read 400 for 200 calls.
        // The watcher's own "resume at the offset, emit each append once" behaviour is
        // pinned in `ai-monitor` (`tail_emits_each_append_exactly_once`); what is pinned
        // here is the value `open()` hands it.
        let dir = temp_dir("repoint-fold");
        let a = dir.join("tab-a.jsonl");
        let b = dir.join("tab-b.jsonl");
        std::fs::write(&a, bash_calls(0, 200)).unwrap();
        std::fs::write(&b, bash_calls(1_000, 3)).unwrap();

        let (mut ta, off_a) = backfill_from_scratch(Some(&a));
        assert_eq!(ta.snapshot().total_tool_calls, 200, "premise: tab A's log");
        assert_eq!(
            off_a,
            std::fs::metadata(&a).unwrap().len(),
            "the tail resumes at EOF — not one backfilled byte is folded a second time"
        );

        let (tb, off_b) = backfill_from_scratch(Some(&b));
        let sb = tb.snapshot();
        assert_eq!(
            sb.total_tool_calls, 3,
            "tab B's log only — 203 means the accumulator was carried across the re-point"
        );
        assert_eq!(sb.tool_calls, vec![("Bash".to_string(), 3)]);
        assert_eq!(off_b, std::fs::metadata(&b).unwrap().len());

        // What the watcher does with the pair, done here deterministically: resume at the
        // offset and fold what follows. One more call must read 201, not 401.
        std::fs::write(&a, bash_calls(0, 200) + &bash_calls(500, 1)).unwrap();
        let bytes = std::fs::read(&a).unwrap();
        let tail = String::from_utf8_lossy(&bytes[off_a as usize..]).into_owned();
        for line in tail.lines() {
            ta.apply(line);
        }
        assert_eq!(
            ta.snapshot().total_tool_calls,
            201,
            "an offset of 0 would have re-folded the whole file: 401"
        );
    }

    #[test]
    fn the_panel_holds_one_session_and_re_points_it_in_place() {
        // R10.1 — "exactly one panel, showing the ACTIVE tab's session". The *count* of
        // panels is a registration fact one level up: `main.rs` registers one
        // `AiMonitorOverlay`, which owns one `MonitorHandle`, and `layer()` hands the
        // renderer a `MonitorPanel` cloned from it — so every drawn panel reads one model.
        // What this file owns is the other half, asserted here: that one panel shows one
        // tab at a time, by re-pointing a single slot rather than accumulating tabs.
        let mut s = Session::default();
        for (tab, calls) in [("tab-a", 200_u64), ("tab-b", 3), ("tab-c", 41)] {
            s.repoint();
            s.note(tallied(tab, calls), LAUNCHED, true);
            assert_eq!(s.attach.session_id.as_deref(), Some(tab));
            assert_eq!(
                shown_total(&s),
                Some(calls),
                "the panel shows the active tab, and only it"
            );
        }
    }

    #[test]
    fn switching_tabs_re_points_the_panel_and_then_settles() {
        // R10.2 — the switch itself, driven through the same `opened` -> `changed`
        // transition `open()` and `on_active_changed` perform. Re-opening re-keys, which is
        // what makes the re-point converge instead of firing on every subsequent metadata
        // sync (the storm `Repoint` exists to stop).
        let mut r = Repoint::default();
        r.opened(Some(PathBuf::from("/proj/a")), Some(101), None, |_, _| None);

        assert!(
            r.changed(true, Some(Path::new("/proj/b")), Some(202)),
            "a tab switch is a real change: `on_active_changed` re-opens on tab B"
        );
        r.opened(Some(PathBuf::from("/proj/b")), Some(202), None, |_, _| None);
        assert!(
            !r.changed(true, Some(Path::new("/proj/b")), Some(202)),
            "and the panel then settles on tab B"
        );
        assert!(
            r.changed(true, Some(Path::new("/proj/a")), Some(101)),
            "switching back re-points again — the panel follows the ACTIVE tab, always"
        );
    }

    #[test]
    fn a_launch_restore_must_fold_a_log_or_the_panel_it_reopens_is_empty() {
        // R6.6 — why `on_command`'s restore calls `open()` instead of setting `visible`.
        //
        // `dashboard::model` renders `Session::latest`, and the only thing that ever fills
        // that slot is `note()`, fed by the backfill `open()` runs and then by the watcher
        // `open()` spawns. A restore that merely flipped `visible` would reserve the band
        // over a `Session` in exactly the state below — and, holding no watcher, would stay
        // there for the life of the window: a panel that is open, correctly sized, and
        // permanently blank. That reads as "the ai-monitor is broken", not as "the restore
        // is incomplete", which is why it is pinned rather than left to review.
        //
        // Scope note: the `open()` call itself is not reachable from here —
        // `AiMonitorOverlay` needs an `EventLoopProxy`, which needs a real winit `EventLoop`
        // (main-thread-only, once per process on macOS). What is driven is the work `open()`
        // does that a `visible = true` restore would skip.
        let empty = Session::default();
        assert_eq!(
            shown_total(&empty),
            None,
            "premise: a Session nobody folded into renders nothing at all"
        );

        let dir = temp_dir("launch-restore");
        let log = dir.join("restored.jsonl");
        std::fs::write(&log, bash_calls(0, 7)).unwrap();

        let (telemetry, offset) = backfill_from_scratch(Some(&log));
        let mut restored = Session::default();
        restored.note(Arc::new(telemetry.snapshot()), LAUNCHED, true);
        assert_eq!(
            shown_total(&restored),
            Some(7),
            "a restore resolves and folds this tab's log, exactly as the chord does"
        );
        assert!(
            restored.attach.attached,
            "and attaches, so the watcher's later snapshots are admitted"
        );
        assert_eq!(
            offset,
            std::fs::metadata(&log).unwrap().len(),
            "with the tail resuming at EOF — a restore must not double-count the backfill"
        );
    }

    #[test]
    fn a_tab_switch_has_no_route_to_the_panels_width() {
        // R10.4 — the panel's width is a WINDOW property, so a tab switch cannot change it
        // and therefore cannot change the column count or trigger `settings::refit`. The
        // property is structural rather than computed, so it is pinned structurally: from
        // this file there must be no *value* a width could travel in.
        //
        // Half one, a COMPILE-TIME pin: `on_active_changed` (the tab switch) is handed a
        // cwd and a pid and nothing else — no `AppCtx`, hence no `AppRequest`. Give it one
        // and this coercion stops compiling. `on_command`, the launch restore's route
        // (R6.6), is held to the same rule for the same reason.
        // RE-TYPED for the widened seam, not deleted: the property is "no value a width
        // could travel in", and a third `Option<u32>` — an agent pid — preserves it exactly.
        // Deleting the pin because the signature moved would have thrown away the guarantee
        // along with the shape it was asserted through.
        //
        // RE-TYPED a second time, for the same reason and to the same end: idle-timestamps
        // D18 adds the tab's `AgentState`. That is a three-variant enum the panel READS to
        // decide whether arming a deadline would be a lie — it carries no channel, no handle
        // and no callback, so the seam still offers nothing an `AppRequest` could ride. Each
        // re-typing is the moment to check that claim rather than to drop the pin.
        // The spelled-out type IS the assertion, so it is not factored into an alias: an
        // alias would let a parameter be added to the seam and to the alias in one edit,
        // with this line still compiling and still looking like it was checked.
        #[allow(clippy::type_complexity)]
        let _: fn(
            &mut AiMonitorOverlay,
            Option<PathBuf>,
            Option<u32>,
            Option<u32>,
            Option<crate::tabmeta::AgentState>,
        ) = <AiMonitorOverlay as Overlay>::on_active_changed;
        let _: fn(&mut AiMonitorOverlay, &crate::seam::OverlayCommand) -> crate::seam::Handled =
            <AiMonitorOverlay as Overlay>::on_command;

        // Half two, the seam this file *does* reach: even the chord — the one path that
        // holds an `AppCtx` — records a bare `bool`. This is what replaced a `PANEL_WIDTH_PX`
        // const travelling as `SetMonitorWidth(f32)`, and it is the stronger form of the
        // same guarantee: there is no longer a width in this crate's seam to send at all,
        // so a switch could not move the band even if it somehow acquired a context.
        let mut sink = Vec::new();
        let mut ctx = crate::seam::AppCtx::new(&mut sink, 1, 0, None);
        ctx.set_monitor_open(true);
        assert_eq!(
            ctx.take_requests(),
            vec![crate::seam::AppRequest::SetMonitorOpen(true)],
            "the chord decides open/closed; the width comes from config + override"
        );

        // There is no per-tab width to carry either: `Session` — everything a re-point
        // rebuilds — holds none, and this file now names no width constant.
        //
        // What this canNOT see from here is the other side of that seam: whether
        // `terminal_app`'s handling of `SetMonitorOpen` refits only on a column change.
        // The predicate is pinned (`input::needs_refit`); that `apply_monitor_open` is its
        // only caller belongs to the file that owns it.
    }

    /// `n` completed `Bash` calls as JSONL, with ids from `first` so two files never
    /// collide when their folds are compared.
    fn bash_calls(first: usize, n: usize) -> String {
        (first..first + n)
            .map(|i| {
                format!(
                    "{{\"type\":\"tool_use\",\"id\":\"c{i}\",\"name\":\"Bash\"}}\n\
                     {{\"type\":\"tool_result\",\"tool_use_id\":\"c{i}\",\"is_error\":false}}\n"
                )
            })
            .collect()
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let mut base = std::env::temp_dir();
        base.push(format!("app-overlay-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    // ---- ai-monitor-session-rotation: `/clear` retires a log and opens a new one ----
    //
    // The values below come from a real chain measured in
    // `~/.claude/projects/-Users-hyunhokim-IdeaProjects-terminal/`: one `claude` process,
    // pid 88187, whose registry record kept `startedAt: 1786169000374` while `sessionId`
    // moved three times — `a3dc815b` -> `fc326d50` -> `3150bf9c` -> `d554e3c6`.
    //
    // Two corrections to what an earlier draft of this header said, both verified against the
    // logs, because both mislead a reader into the wrong model:
    //
    //   * The `/clear` marker is at **index 3 of each SUCCESSOR**, not the last record of the
    //     predecessor. `a3dc815b` — the first predecessor — contains no `/clear` record at
    //     all, and its last record is at 06:31:47.313 while the marker sits in `fc326d50` at
    //     06:31:54.828. Reasoning about the predecessor's tail is what produced the first
    //     draft's design.
    //   * `startedAt` is NOT the process start. `ai_monitor::Registered` documents it as
    //     "slightly after its real start" (measured 1.3-2.2s), which is exactly why
    //     `validated_session_id` is a windowed check and not an equality.
    //
    // `d554e3c6-...` and the two numbers are verbatim from `~/.claude/sessions/88187.json`;
    // the earlier ids are the real 8-char prefixes with synthetic tails, because only the
    // *difference* between them is load-bearing.

    const PID: u32 = 88_187;
    const PROC_START: u64 = 1_786_169_000_374;
    const BEFORE: &str = "3150bf9c-0000-0000-0000-000000000000";
    const AFTER: &str = "d554e3c6-11ce-46c2-8132-b326a5287c8f";

    fn opened_on(session_id: &str) -> OpenedOn {
        OpenedOn {
            pid: PID,
            process_start_ms: PROC_START,
            session_id: Some(session_id.to_string()),
        }
    }

    /// A panel that opened while its `claude` had not yet written a registry record.
    fn opened_unnamed() -> OpenedOn {
        OpenedOn {
            pid: PID,
            process_start_ms: PROC_START,
            session_id: None,
        }
    }

    /// A registry record as `claude` writes it, `offset_ms` after the process started.
    fn record(session_id: &str, offset_ms: i64) -> ai_monitor::Registered {
        ai_monitor::Registered {
            session_id: session_id.to_string(),
            cwd: PathBuf::from("/Users/hyunhokim/IdeaProjects/terminal"),
            started_at_ms: (PROC_START as i64 + offset_ms) as u64,
        }
    }

    #[test]
    fn a_clear_rotates_the_session_and_the_panel_must_follow() {
        // THE BUG. The panel opened on `3150bf9c`; the user pressed `/clear`; the tab's own
        // process now names `d554e3c6`. Nothing about the tab changed — same cwd, same shell
        // pid, same `claude` — so nothing else in the app will ever ask again, and the panel
        // tails a file that is never appended to for the rest of the session.
        assert_eq!(
            session_rotated_with(
                |pid| (pid == PID).then(|| record(AFTER, 1_500)),
                Some(&opened_on(BEFORE))
            ),
            Rotation::Rotated
        );
    }

    #[test]
    fn the_same_session_is_not_a_rotation() {
        // The inert case, and it is the one that runs almost always — on every wake for the
        // whole life of an open panel. It must not re-point, because `open()` drops the
        // watcher, re-runs the full backfill and makes two 250 ms-budgeted `tmux` calls on
        // the event-loop thread.
        assert_eq!(
            session_rotated_with(|_| Some(record(BEFORE, 1_500)), Some(&opened_on(BEFORE))),
            Rotation::None
        );
    }

    #[test]
    fn only_the_opened_on_pids_record_can_rotate_the_panel() {
        // Authority is per-PROCESS, and this is the assertion that says so with content
        // rather than with a comment. A sibling tab's `claude` (pid 42) has rotated to a
        // different session; ours (pid 88187) has not. Reading "a claude" instead of "the
        // claude this open resolved against" would re-point on the sibling's `/clear` —
        // which is `ai-monitor-rival-assignment` D3 broken from a new direction.
        //
        // The stronger statement is structural and holds for the whole class: the signature
        // takes no directory, no path and no file list, so a rival's LOG cannot reach this
        // rule through any argument at all. Only a rival's registry record could, and only
        // by being read under the wrong pid — which is exactly what this pins.
        let rivals_moved = |pid: u32| match pid {
            42 => Some(record(AFTER, 1_500)),
            _ => Some(record(BEFORE, 1_500)),
        };
        assert_eq!(
            session_rotated_with(rivals_moved, Some(&opened_on(BEFORE))),
            Rotation::None,
            "the sibling rotated; we did not"
        );
    }

    #[test]
    fn a_record_that_predates_the_process_is_a_recycled_pid_not_a_rotation() {
        // `~/.claude/sessions/` files outlive their processes — 5 files for 4 live processes
        // when measured — so a recycled pid presents a stranger's record. Believing it here
        // would re-point the panel onto a session that has nothing to do with this tab: the
        // very failure this feature exists to prevent, arrived at from the opposite
        // direction. `validated_session_id` is the one guard, shared with `tabsession` and
        // with the rival check, and it must be applied to the RE-read too, not only at open.
        assert_eq!(
            session_rotated_with(|_| Some(record(AFTER, -1)), Some(&opened_on(BEFORE))),
            Rotation::None,
            "a record written before the process started cannot describe it"
        );
    }

    #[test]
    fn a_record_written_long_after_the_process_started_is_not_believed_either() {
        // The other half of the same guard: `REGISTRY_DRIFT_MS` is 60s, measured against
        // 1.3-2.2s in the field. A record an hour late is a stale file, not our process.
        assert_eq!(
            session_rotated_with(|_| Some(record(AFTER, 3_600_000)), Some(&opened_on(BEFORE))),
            Rotation::None
        );
    }

    #[test]
    fn an_unreadable_record_keeps_what_is_on_screen() {
        // Fail toward "no rotation". The cost of being wrong this way is the pre-existing
        // bug for one more wake; the cost of the other direction is tearing down a live
        // attachment — blanking a panel someone is reading — on a transient file read.
        assert_eq!(
            session_rotated_with(|_| None, Some(&opened_on(BEFORE))),
            Rotation::None
        );
    }

    #[test]
    fn a_tab_with_no_claude_never_reads_a_record() {
        // Nothing to ask. `open()` clears the baseline on every re-point precisely so a
        // previous tab's id cannot linger and be compared against a new tab's process.
        assert_eq!(
            session_rotated_with(
                |_| panic!("must not read a record with no process to ask"),
                None
            ),
            Rotation::None
        );
    }

    #[test]
    fn a_panel_opened_before_its_session_was_named_arms_instead_of_re_pointing() {
        // REGRESSION, and it was silent. A `claude` takes 1.1-2.2s to write its registry
        // record; open the panel inside that window with no `--resume` in argv and the hint
        // names no session. The first draft left the baseline `None` for the life of the
        // open and nothing re-armed it — tier-2 dating still attached the right log, so the
        // panel looked perfectly healthy while being permanently blind to `/clear`.
        //
        // Arming must NOT re-point: learning an id we never had is an arrival, and
        // re-opening here would drop a live watcher and re-run a full backfill for nothing.
        assert_eq!(
            session_rotated_with(|_| Some(record(BEFORE, 1_500)), Some(&opened_unnamed())),
            Rotation::Arm(BEFORE.to_string()),
            "record the id; do not re-point on it"
        );
    }

    #[test]
    fn arming_still_refuses_a_record_it_may_not_believe() {
        // The pid-reuse guard is not skipped just because we have no baseline to lose.
        // Arming from a stranger's record would plant a false baseline that then makes the
        // tab's own real session look like a rotation.
        assert_eq!(
            session_rotated_with(|_| Some(record(AFTER, -1)), Some(&opened_unnamed())),
            Rotation::None
        );
    }

    #[test]
    fn the_record_is_read_for_the_pid_the_panel_opened_on() {
        // Not "some claude", not "the newest claude" — the one this open resolved against.
        let asked = std::cell::Cell::new(None);
        let _ = session_rotated_with(
            |pid| {
                asked.set(Some(pid));
                None
            },
            Some(&opened_on(BEFORE)),
        );
        assert_eq!(asked.get(), Some(PID));
    }

    // ---- the tick's decision: which question is asked, in which order ----

    #[test]
    fn a_rotation_re_points_the_panel() {
        assert_eq!(tick_action(false, true, false, || true), Tick::Reopen);
    }

    #[test]
    fn no_rotation_falls_through_to_the_rival_refresh() {
        // The rotation check is ADDITIVE: it must not displace D11's per-wake republish,
        // which is what keeps rival membership live during an unattached open.
        assert_eq!(
            tick_action(false, true, false, || false),
            Tick::RefreshRivals
        );
    }

    #[test]
    fn a_tab_change_outranks_the_rotation_check_and_never_asks_it() {
        // Ordering, and the file read it saves. A tab switch re-opens regardless, and
        // `open()` re-reads the hint from scratch — so asking here would be a wasted read
        // whose answer is discarded, on the event-loop thread, after every wake.
        assert_eq!(
            tick_action(true, true, false, || panic!(
                "a tab change must not consult the registry"
            )),
            Tick::Reopen
        );
    }

    #[test]
    fn a_closed_panel_never_reads_the_registry() {
        // The cost claim, pinned. Nothing is on screen to be stale, so a closed panel must
        // add exactly zero I/O to a wake — and wakes arrive for the whole life of the
        // process whether or not anyone has ever pressed Cmd+Shift+M.
        assert_eq!(
            tick_action(false, false, false, || panic!(
                "a closed panel must not read anything"
            )),
            Tick::Idle
        );
    }

    // ---- tabsession-pane-cwd-gate D5/D6/D9: a changed `claude` pid re-points (R6) ----

    #[test]
    fn the_compare_is_some_guarded() {
        // R6.4 — a bare `published != last_seen` reads `None != Some(x)` as `true`, so an
        // absent published value would be "a different pid" and re-point on EVERY wake. Absent
        // is not exotic: it is a transient walk failure, and it is what the two derivations
        // legitimately disagree about.
        assert!(
            !agent_pid_moved(None, Some(74922)),
            "absent is no information"
        );
        assert!(
            !agent_pid_moved(Some(74922), None),
            "nothing to compare yet"
        );
        assert!(!agent_pid_moved(None, None));
        assert!(!agent_pid_moved(Some(74922), Some(74922)), "unchanged");
        assert!(agent_pid_moved(Some(74923), Some(74922)), "a real change");
    }

    #[test]
    fn a_none_publish_never_erases_the_memory() {
        // R6.4, the other half — and it is the half a pure predicate cannot cover, because the
        // hazard is in the WRITE. If a `None` publish overwrote `last_seen_pid`, the next real
        // publish would compare against `None`, and the wake after that against the new value:
        // one transient walk failure would cost a spurious full re-point.
        let mut o = AiMonitorOverlay::detached();
        // Closed, so every one of these is `Tick::Idle` and no `open()` is attempted — the
        // memory rule is what is under test, not the action.
        o.on_active_changed(None, Some(8060), Some(74922), None);
        assert_eq!(o.last_seen_pid, Some(74922));
        o.on_active_changed(None, Some(8060), None, None);
        assert_eq!(
            o.last_seen_pid,
            Some(74922),
            "a None publish is no information, not an erasure"
        );
        o.on_active_changed(None, Some(8060), Some(74923), None);
        assert_eq!(o.last_seen_pid, Some(74923), "a real publish moves it");
    }

    #[test]
    fn a_changed_pid_re_points_and_an_unchanged_one_does_not() {
        // R6.5 — and note the rotation thunk PANICS on the changed path: a changed pid makes
        // the rotation question moot (it concerns a process that is no longer the one being
        // tracked), so asking it would be a wasted file read on the event-loop thread.
        assert_eq!(
            tick_action(false, true, true, || panic!(
                "a changed pid must not consult the registry"
            )),
            Tick::Reopen
        );
        assert_eq!(
            tick_action(false, true, false, || false),
            Tick::RefreshRivals,
            "an unchanged pid falls through to D11's per-wake republish"
        );
    }

    #[test]
    fn a_closed_panel_decides_nothing_about_a_changed_pid() {
        // R6.6 — the pid check sits BELOW the `!visible` gate, and the placement is not
        // cosmetic. Above it, a pid change would return `Tick::Reopen` for a closed panel;
        // `open()` sets `self.visible = true` without the `ctx.set_monitor_open` that only the
        // chord path performs, so the panel would paint into a band the renderer never
        // reserved.
        assert_eq!(
            tick_action(false, false, true, || panic!(
                "a closed panel must not read anything"
            )),
            Tick::Idle,
            "closed outranks a changed pid"
        );
        // ...and a tab change still outranks it, since `open()` re-reads everything anyway.
        assert_eq!(tick_action(true, true, true, || false), Tick::Reopen);
    }

    // ---- the state transition: arming, and what it does to the baseline ----

    #[test]
    fn arming_writes_the_id_into_the_baseline_and_does_not_re_point() {
        // REGRESSION, and the checker's mutation is the reason it exists: replacing this arm
        // with `Rotation::Arm(_) => false` left all 1133 tests green while a panel opened
        // during `claude`'s first ~1.5 s stayed permanently blind to `/clear`. Asserting the
        // *return* alone cannot see that — the write-back is the whole fix, so the write-back
        // is what this asserts.
        let mut baseline = Some(opened_unnamed());
        let re_point = apply_rotation(Rotation::Arm(BEFORE.to_string()), &mut baseline);
        assert!(!re_point, "an arrival must not drop a live watcher");
        assert_eq!(
            baseline.as_ref().and_then(|b| b.session_id.as_deref()),
            Some(BEFORE),
            "the id must be RECORDED, or the next /clear has nothing to be measured against"
        );
    }

    #[test]
    fn an_armed_baseline_then_detects_the_next_clear() {
        // The whole sequence, in the order it happens in production: open before the session
        // is named -> arm from the first believable record -> `/clear` -> rotate. Each step
        // feeds the next, so a break anywhere in the chain fails here even if every step
        // passes in isolation.
        let mut baseline = Some(opened_unnamed());

        let armed = session_rotated_with(|_| Some(record(BEFORE, 1_500)), baseline.as_ref());
        assert!(!apply_rotation(armed, &mut baseline));

        let after_clear = session_rotated_with(|_| Some(record(AFTER, 1_500)), baseline.as_ref());
        assert_eq!(after_clear, Rotation::Rotated);
        assert!(
            apply_rotation(after_clear, &mut baseline),
            "a panel that opened before its session was named must still follow the /clear"
        );
    }

    #[test]
    fn a_rotation_leaves_the_baseline_alone_because_open_rewrites_it() {
        // Deliberate, and worth pinning: `apply_rotation` does NOT write the new id. The
        // re-point runs `open()`, which rebuilds the baseline from a fresh `hint_for` — the
        // authoritative source, including the pid, which may itself have changed. Writing a
        // half-baseline here would leave `pid`/`process_start_ms` describing one process and
        // `session_id` another, and every later comparison would be against a mixture.
        let mut baseline = Some(opened_on(BEFORE));
        assert!(apply_rotation(Rotation::Rotated, &mut baseline));
        assert_eq!(
            baseline.as_ref().and_then(|b| b.session_id.as_deref()),
            Some(BEFORE),
            "open() is the only writer of a post-rotation baseline"
        );
    }

    #[test]
    fn nothing_is_written_when_there_is_no_baseline_to_write_to() {
        let mut baseline: Option<OpenedOn> = None;
        assert!(!apply_rotation(
            Rotation::Arm(BEFORE.to_string()),
            &mut baseline
        ));
        assert!(baseline.is_none());
    }

    /// A rotation re-points through `open()` — the same call a tab switch makes — so the new
    /// session's counters replace the old session's rather than continuing them (R10.3).
    ///
    /// Driven at the units the re-point actually uses (`Session::repoint` +
    /// `backfill_from_scratch`), because `open()` itself needs a live process tree and a real
    /// `~/.claude`. This is the contract's "totals rebuilt, not continued" row.
    #[test]
    fn a_rotation_rebuilds_the_totals_from_the_new_log_and_never_continues_the_old() {
        let dir = temp_dir("rotation-totals");
        let old = dir.join("before.jsonl");
        let new = dir.join("after.jsonl");
        std::fs::write(&old, bash_calls(0, 7)).unwrap();
        std::fs::write(&new, bash_calls(100, 2)).unwrap();

        let mut session = Session::default();
        let (pre, _) = backfill_from_scratch(Some(&old));
        session.note(Arc::new(pre.snapshot()), 0, true);
        assert_eq!(
            session.latest.as_ref().unwrap().total_tool_calls,
            7,
            "premise: the pre-rotation session's own tally"
        );

        // The rotation. `open()` does exactly these two things, in this order.
        session.repoint();
        let (post, offset) = backfill_from_scratch(Some(&new));
        session.note(Arc::new(post.snapshot()), 0, true);

        assert_eq!(
            session.latest.as_ref().unwrap().total_tool_calls,
            2,
            "the new session's tally ALONE — 9 would mean the accumulator was continued"
        );
        assert_eq!(
            offset,
            std::fs::metadata(&new).unwrap().len(),
            "the tail resumes past what was folded, or the watcher re-folds every line"
        );
    }

    /// An unattached Antigravity panel must always have something to wake it (loop run 2).
    ///
    /// Pinned by asserting the source, for the reason the sibling test below gives: `open()`
    /// needs a live process tree and an `EventLoopProxy`, so no unit can reach the branch.
    /// It earns its place the same way too — the defect it guards SHIPPED. With no watcher
    /// armed on the no-log path, a panel opened before `agy` became detectable had no wake
    /// source at all, so `on_active_changed` never ran, the re-probe never ran, and the panel
    /// sat on "(no Claude session in this tab)" indefinitely while `agent_for` answered
    /// `Some((Antigravity, ..))` for that shell. Every test stayed green throughout.
    #[test]
    fn an_unattached_antigravity_panel_arms_a_wake_source() {
        let src = include_str!("overlay.rs");
        let (production, _) = src
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("no `mod tests` marker to split on");
        let production = production
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        // The no-log arm exists and watches the presence directory.
        assert!(
            production.contains("ai_monitor::antigravity::presence_dir()"),
            "with no transcript to tail, the panel must still watch for the lock that will \
             name one — otherwise nothing can ever wake it"
        );
        assert!(
            production.contains("scope: ai_monitor::WatchScope::AwaitingLock"),
            "and that watch must use the wake-only scope, not a tailing one"
        );
        // The flag the re-point reads is set on a worker; the wake it needs must be
        // requested by that same worker, because `session_rotated` already read the flag
        // this tick.
        assert!(
            production.contains("ping();"),
            "the worker that sets `appeared` must also request the follow-up wake, or the \
             re-point waits for traffic that may never come"
        );
    }

    #[test]
    fn a_rotation_re_points_rather_than_refreshing_rivals() {
        // The `Tick::Reopen -> open()` mapping is one line and no unit can reach it: `open()`
        // needs a live process tree, a real `~/.claude` and an `EventLoopProxy`. So it is
        // pinned the way this crate already pins unreachable wiring (see
        // `r7_4_the_loop_still_waits_and_the_panel_schedules_no_wake`, which scans this same
        // production half for scheduler names): by asserting the source.
        //
        // Crude, and it earns its place — a checker mutated this arm to `refresh_rivals` and
        // every one of the 1133 tests stayed green while the panel never re-pointed at all.
        let src = include_str!("overlay.rs");
        // Split on the FULL `mod tests` marker, not on `#[cfg(test)]` — this file has a
        // test-only constructor at the top, so the bare attribute appears ~1000 lines above
        // the real boundary and the first draft of this test cut there and failed on correct
        // code. Comment lines are stripped for the same reason the sibling helper strips
        // them: a comment quoting the arm must not be able to satisfy the assertion.
        let (production, tests) = src
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("no `mod tests` marker to split on");
        assert!(
            !tests.is_empty() && production.len() > src.len() / 4,
            "the split found the marker in an implausible place"
        );
        let production = production
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            production.contains("Tick::Reopen => self.open(cwd, shell_pid)"),
            "a rotation must re-point through open(), which is the whole re-point"
        );
        assert!(
            production.contains("Tick::RefreshRivals => self.refresh_rivals(shell_pid)"),
            "and the inert case must still republish rival membership (D11)"
        );
        // Same class, same reason: a checker mutated the one line that arms the whole
        // feature to `self.opened_on = None` — rotation detection entirely dead, 1133 green.
        assert!(
            production.contains("self.opened_on = baseline_from(&hint);"),
            "open() must record the baseline, or nothing is compared against anything"
        );
        // And the two lines that CONNECT the tested rule to production. A checker mutated
        // each to a constant — `|| false`, and `|_| None` for the registry read — and all
        // 524 tests stayed green with rotation detection completely dead. These are the most
        // load-bearing lines in the feature precisely because everything either side of them
        // is well covered: the rule is unit-tested, `open()` is pinned above, and neither
        // notices that nothing calls the one or feeds the other.
        assert!(
            production.contains("|| self.session_rotated()"),
            "the tick must actually ASK, or the rule below it is dead code"
        );
        assert!(
            production.contains("session_rotated_with(ai_monitor::registered_session,"),
            "and it must ask the real registry, not a stub"
        );
        // A CONCURRENCY fence, unreachable from a unit for the same reason: proving it needs
        // an in-flight `refresh_rivals` worker, which needs a live process tree. The hazard
        // is the one `Session::repoint` documents for `attached_flag` — a worker from the
        // previous open still holds its clone, so clearing in place lets tab A's answer land
        // in tab B's decision. Re-allocating orphans the writer; `store(false)` does not, and
        // that mutation passes all 528 tests.
        assert!(
            production.contains("self.claude_appeared = Arc::new(AtomicBool::new(false));"),
            "a re-point must ORPHAN an in-flight worker's flag, not clear it in place"
        );
    }

    fn hint_of(
        pid: Option<u32>,
        started: Option<u64>,
        sid: Option<&str>,
    ) -> tabsession::SessionHint {
        tabsession::SessionHint {
            pid,
            session_id: sid.map(str::to_string),
            started_ms: started,
            // These fixtures are about the rotation baseline, which reads pid/started/session
            // only. The directory has its own tests against the `hint_or_pane_cwd` resolver.
            cwd: None,
            rival_starts: vec![],
            rivals_known: true,
        }
    }

    #[test]
    fn a_hint_naming_a_process_and_a_session_becomes_the_baseline() {
        let hint = hint_of(Some(PID), Some(PROC_START), Some(BEFORE));
        assert_eq!(baseline_from(&hint), Some(opened_on(BEFORE)));
    }

    #[test]
    fn a_hint_with_no_session_yet_still_becomes_an_armable_baseline() {
        // The case the first draft dropped on the floor. `claude` takes 1.1-2.2s to write its
        // registry record; a hint taken inside that window names a process but no session,
        // and that is a baseline worth keeping — `Rotation::Arm` fills the id in later.
        let hint = hint_of(Some(PID), Some(PROC_START), None);
        assert_eq!(baseline_from(&hint), Some(opened_unnamed()));
    }

    #[test]
    fn half_an_identity_is_no_baseline_at_all() {
        // A pid with no start, or a start with no pid, is not an identity: registry files
        // outlive their processes, so half an identity cannot be told from a stranger's.
        for hint in [
            hint_of(None, Some(PROC_START), Some(BEFORE)),
            hint_of(Some(PID), None, Some(BEFORE)),
            tabsession::SessionHint::default(),
        ] {
            assert_eq!(baseline_from(&hint), None);
        }
    }

    // ---- opened before the tab had a `claude` (user-reported, 2026-08-08) ----

    #[test]
    fn an_unarmed_panel_re_points_when_a_claude_finally_appears() {
        // The reported sequence: open the window, press Cmd+Shift+M, THEN start `claude`.
        // `hint_for` found nothing at open, so there is no baseline and no process to ask.
        // Without this the panel fills in (the unattached watcher finds the log by dating)
        // and is then blind to `/clear` for the rest of the open.
        let mut o = AiMonitorOverlay::detached();
        o.visible = true;
        assert!(
            o.opened_on.is_none(),
            "premise: opened with no claude in the tab"
        );

        assert!(
            !o.session_rotated(),
            "nothing has appeared yet — a bare unarmed panel must not re-point"
        );

        o.claude_appeared.store(true, Ordering::Release);
        assert!(
            o.session_rotated(),
            "a claude appeared: re-point, so the whole resolution is redone against it"
        );
    }

    #[test]
    fn one_appearance_causes_exactly_one_re_point() {
        // The flag is CONSUMED. Left set, every later wake would re-open — each one a
        // /bin/ps fork, two 250 ms-budgeted tmux waits on the event-loop thread, a watcher
        // respawn and a full backfill. That is the storm `Repoint` exists to prevent.
        let mut o = AiMonitorOverlay::detached();
        o.visible = true;
        o.claude_appeared.store(true, Ordering::Release);

        assert!(o.session_rotated());
        assert!(!o.session_rotated(), "the appearance must not fire twice");
    }

    #[test]
    fn an_armed_panel_never_consumes_the_appearance_flag() {
        // Once a baseline exists the question is "did MY session rotate", answered from the
        // registry — the appearance flag belongs to the unarmed branch and must not be read
        // or consumed on this one.
        //
        // Asserted on the flag rather than on the return value ON PURPOSE: `session_rotated`
        // reads the real `~/.claude/sessions/` on the armed path, so asserting its answer
        // would make this test depend on which sessions happen to exist on the machine
        // running it. The first draft did exactly that and failed here — pid 88187 still has
        // a live record, the very one these constants were taken from, so the rule correctly
        // reported a rotation and the test was wrong, not the code. Flag-consumption is the
        // property that actually belongs to this branch, and it is machine-independent.
        let mut o = AiMonitorOverlay::detached();
        o.visible = true;
        o.opened_on = Some(opened_on(BEFORE));
        o.claude_appeared.store(true, Ordering::Release);

        let _ = o.session_rotated();
        assert!(
            o.claude_appeared.load(Ordering::Acquire),
            "armed: the appearance flag is not this branch's business"
        );
    }

    #[test]
    fn the_rival_refresh_keeps_running_while_unarmed_even_once_attached() {
        // R6.3 turns the refresh off once attached — but that reasoning assumes a baseline
        // exists. Attached-and-unarmed is exactly the reported state, and the lookup R6.3
        // switched off is the only thing that can arm it.
        assert!(
            rival_refresh_needed(true, false),
            "attached but unarmed: keep looking, or nothing ever arms the baseline"
        );
        assert!(
            !rival_refresh_needed(true, true),
            "attached and armed: R6.3 — stop, the reads can no longer change anything"
        );
        assert!(rival_refresh_needed(false, true));
        assert!(rival_refresh_needed(false, false));
    }

    // ---- idle-timestamps T3 / R2: arming the deadline ----

    /// An open panel plus the log of every arming call it made.
    ///
    /// The recorder IS the test seam D7 chose the injected callback for: production hands the
    /// overlay `TabMetaHandle::set_panel_deadline`, a test hands it this, and the overlay
    /// needs no handle, no global and no knowledge that a worker exists.
    fn armed_panel(age_ms: u64) -> (AiMonitorOverlay, Arc<Mutex<Vec<Option<u64>>>>) {
        let log: Arc<Mutex<Vec<Option<u64>>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&log);
        let mut o = AiMonitorOverlay::with_waker(
            None,
            Box::new(move |at| sink.lock().expect("arming log").push(at)),
        );
        o.visible = true;
        o.session.attach.attached = true;
        o.session.latest = Some(snapshot_aged(age_ms));
        (o, log)
    }

    fn arms(log: &Arc<Mutex<Vec<Option<u64>>>>) -> Vec<Option<u64>> {
        log.lock().expect("arming log").clone()
    }

    /// The `last_event_ms` currently on the panel — what a correct arm must be computed from.
    fn last_event(o: &AiMonitorOverlay) -> u64 {
        o.session
            .latest
            .as_ref()
            .and_then(|t| t.last_event_ms)
            .expect("the fixture sets a timestamp")
    }

    #[test]
    fn r0_2_the_instant_armed_is_an_instant_at_which_the_pill_reads_idle() {
        // The feature's whole point, composed end to end as far as a headless test can reach:
        // T1's arithmetic, T3's arming rule and the panel's own composer, checked against each
        // other. The one step missing is the event loop delivering the wake, which `cargo
        // test` cannot host — that is what `/run-term` is for.
        //
        // An inert feature passes every part of this and fails here. Arming one millisecond
        // early makes the wake land while `status` still says CONNECTED; because that wake
        // moves no timestamp it re-arms nothing, so the pill would stay CONNECTED for the rest
        // of the session — the defect being fixed, reintroduced by an off-by-one.
        let (mut o, log) = armed_panel(30_000);
        let t = last_event(&o);
        o.poll();

        let armed = match arms(&log).as_slice() {
            [Some(at)] => *at,
            other => panic!("expected exactly one arming call, got {other:?}"),
        };

        let snap = o
            .session
            .latest
            .as_deref()
            .expect("the fixture attached one");
        let at = |now_ms| {
            dashboard::model(
                Some(snap),
                &dashboard::Env {
                    now_ms,
                    attached: true,
                    context_window: None,
                    tab_agent: None,
                    utc_offset_secs: 0,
                },
            )
            .status
        };

        assert_eq!(
            at(armed),
            renderer::monitor_model::PanelStatus::Idle,
            "the wake must land on a frame whose pill has flipped"
        );
        assert_eq!(
            at(armed - 1),
            renderer::monitor_model::PanelStatus::Connected,
            "and not one millisecond before it — that wake would be wasted, and it is the \
             only one this session gets"
        );
        assert_eq!(armed, dashboard::idle_deadline_ms(t));
    }

    #[test]
    fn r0_4_a_session_that_goes_quiet_costs_exactly_one_arm() {
        // R0.3's companion, and the reason R0.3 had to grow a precondition: a panel open on a
        // session that is quiet but not yet past the threshold does NOT cost zero wakes. It
        // costs exactly one, ever, which is what separates this from the repaint cadence the
        // panel's approved design rejected. The firing half is `worker::tests::r3_1`.
        let (mut o, log) = armed_panel(30_000);
        for _ in 0..40 {
            o.poll();
        }
        assert_eq!(
            arms(&log).len(),
            1,
            "forty wakes over one quiet session, one deadline"
        );
    }

    #[test]
    fn r0_3_an_idle_machine_costs_no_wake_and_no_frame() {
        // R0.3 previously had no test of its own — it was implied by `r2_7`, which asserts
        // only that a past-due session arms nothing. That is half the requirement. R0.3 also
        // claims no FRAME is drawn, which is the `refresh_model` diff's job, not the arming
        // rule's, and nothing tied the two halves together.
        //
        // Note the precondition the requirement carries, and why it is not a weakening: the
        // session must already be past the threshold. A panel opened onto a session that is
        // quiet but NOT yet past it costs exactly one wake, by design — that is R0.4, and the
        // first form of R0.3 contradicted it by claiming zero wakes for any open panel.
        let (mut o, log) = armed_panel(19 * 60_000);

        // The first poll publishes the initial model, which is a frame the panel owes anyway.
        assert!(o.poll());
        assert_eq!(
            o.published.as_ref().map(|m| m.status),
            Some(renderer::monitor_model::PanelStatus::Idle),
            "the fixture is past the threshold, so the pill is already correct"
        );
        let ladders_after_open = o.ladders_built;

        // Now five minutes of nothing: no append, no agent, no tab change.
        for _ in 0..600 {
            assert!(
                !o.poll(),
                "an idle machine must not ask for a frame it has no news for"
            );
        }

        assert!(arms(&log).is_empty(), "nothing was armed, so nothing wakes");
        assert_eq!(
            o.ladders_built, ladders_after_open,
            "and no rebuild reached the ladder, the anchor or the badge"
        );
    }

    #[test]
    fn a_deadline_armed_before_busy_survives_into_the_busy_window() {
        // The THIRD bypass of D18's suppression, and the only one caused by this feature's own
        // structure rather than by a config toggle or a transient failure.
        //
        // `reconsider_deadline` returns early on `Busy` WITHOUT disarming, because the gate is
        // built to defer an arm rather than cancel one — D11's second trigger is what later
        // executes that deferral. The consequence is that a deadline armed on the last append
        // *before* the worker publishes `Busy` stays pending straight through the busy window.
        //
        // Pinned rather than fixed. Disarming on `Busy` would close this and reopen something
        // strictly worse: the deferred arm is the mechanism the primary path depends on, so
        // cancelling it would put a clean turn ending back on the single-trigger design that
        // arms nothing at all. The Known Gaps record the trade; this test makes sure a future
        // reader who "fixes" it finds out immediately which of the two they chose.
        let (mut o, log) = armed_panel(30_000);
        let t = last_event(&o);

        // The append lands while the tab still reads not-Busy: the arm happens.
        o.poll();
        assert_eq!(arms(&log), vec![Some(dashboard::idle_deadline_ms(t))]);

        // Up to 500ms later the worker publishes Busy — the long tool call has begun.
        o.on_active_changed(None, None, None, Some(crate::tabmeta::AgentState::Busy));

        // The pending deadline is NOT cancelled by that transition.
        assert_eq!(
            o.armed_from,
            Some(t),
            "today's behaviour: the arm placed before Busy survives into the busy window"
        );
        assert_eq!(
            arms(&log).len(),
            1,
            "and no disarm was sent — if this starts failing, `Busy` now cancels rather than \
             defers, which breaks D11's second trigger and the primary path with it"
        );
    }

    #[test]
    fn r2_1_a_newer_last_event_ms_arms_the_deadline() {
        // D11 trigger one, and D15's arithmetic: the instant armed is the first at which the
        // pill reads IDLE, never `last + IDLE_AFTER_MS`, which is one millisecond too early
        // and would burn the one-shot while the pill still read CONNECTED.
        let (mut o, log) = armed_panel(30_000);
        let first = last_event(&o);
        o.poll();
        assert_eq!(arms(&log), vec![Some(dashboard::idle_deadline_ms(first))]);

        // A later burst moves the timestamp: re-arm, once, from the NEW value.
        o.session.latest = Some(snapshot_aged(1_000));
        let second = last_event(&o);
        assert!(second > first);
        o.poll();
        assert_eq!(
            arms(&log),
            vec![
                Some(dashboard::idle_deadline_ms(first)),
                Some(dashboard::idle_deadline_ms(second)),
            ]
        );
    }

    #[test]
    fn r2_2_an_unchanged_last_event_ms_arms_nothing() {
        // One `Cmd` per burst, not per line. A streaming session wakes this path far more
        // often than 500 ms, and re-sending an identical instant on every one of those wakes
        // would put the channel traffic of the deferred heartbeat on a feature that is
        // supposed to be a single edge.
        let (mut o, log) = armed_panel(30_000);
        o.poll();
        assert_eq!(arms(&log).len(), 1);
        for _ in 0..6 {
            o.poll();
        }
        assert_eq!(
            arms(&log).len(),
            1,
            "an unchanged timestamp re-arms nothing"
        );
    }

    #[test]
    fn r2_3_a_busy_tab_defers_the_arm() {
        // D18 — the regression this gate exists to prevent. A long `cargo build` writes its
        // `tool_use` line and then nothing until the result lands, so the log is silent while
        // the session is emphatically working. That is the scenario `IDLE_AFTER_MS` was sized
        // for, and arming through it would manufacture a false IDLE for the rest of the call.
        let (mut o, log) = armed_panel(30_000);
        o.agent_state = Some(crate::tabmeta::AgentState::Busy);
        for _ in 0..4 {
            o.poll();
        }
        assert!(
            arms(&log).is_empty(),
            "a working session must not be told it is idle"
        );
    }

    #[test]
    fn r2_4_leaving_busy_executes_the_deferred_arm() {
        // The ordering fix, and the test must drive the two inputs on SEPARATE wakes or it
        // passes against the inert version. The timestamp arrives first, on the panel's own
        // watcher, while the state still reads a stale `Busy`; the state arrives up to 500 ms
        // later, on the worker's publish, by which time the timestamp is unchanged. A
        // single-trigger design arms on neither and the pill stays CONNECTED forever.
        let (mut o, log) = armed_panel(30_000);
        o.agent_state = Some(crate::tabmeta::AgentState::Busy);

        // Wake one: the timestamp moves, the state is still stale. Deferred.
        o.session.latest = Some(snapshot_aged(1_000));
        let t = last_event(&o);
        o.poll();
        assert!(arms(&log).is_empty(), "still Busy — the arm is deferred");

        // Wake two: the state flips, carrying no new timestamp. This is the arm.
        o.on_active_changed(None, None, None, Some(crate::tabmeta::AgentState::Waiting));
        assert_eq!(
            arms(&log),
            vec![Some(dashboard::idle_deadline_ms(t))],
            "leaving Busy executes the deferred arm, from the unchanged timestamp"
        );
    }

    #[test]
    fn r2_5_the_second_trigger_is_an_edge_not_a_level() {
        // What bounds the arming rate is the `Busy →` transition, not a pending flag — the
        // overlay has no way to clear such a flag, because the wake its deadline produces
        // carries no payload and never tells it that the deadline fired.
        let (mut o, log) = armed_panel(30_000);
        o.agent_state = Some(crate::tabmeta::AgentState::Busy);
        o.poll();
        o.on_active_changed(None, None, None, Some(crate::tabmeta::AgentState::Waiting));
        assert_eq!(arms(&log).len(), 1);

        for _ in 0..5 {
            o.on_active_changed(None, None, None, Some(crate::tabmeta::AgentState::Waiting));
            o.poll();
        }
        assert_eq!(
            arms(&log).len(),
            1,
            "further non-Busy wakes with an unchanged timestamp arm nothing"
        );
    }

    #[test]
    fn r2_6_the_incoming_agent_state_survives_the_repoint_guard() {
        // `on_active_changed` dispatches through `tick_action`, which no-ops on every wake
        // where the tab did not change — i.e. every wake in the quiet case this feature
        // targets. The state must be stored above that, and stored UNCONDITIONALLY: the guard
        // one line up ignores a `None` pid, and mirroring it here would keep a stale `Busy`
        // forever once the agent exited, suppressing every future deadline.
        let (mut o, log) = armed_panel(30_000);
        o.agent_state = Some(crate::tabmeta::AgentState::Busy);
        o.poll();
        assert!(arms(&log).is_empty());

        // No tab change: `Repoint::changed` reports nothing moved. The state must land anyway.
        o.on_active_changed(None, None, None, Some(crate::tabmeta::AgentState::Idle));
        assert_eq!(o.agent_state, Some(crate::tabmeta::AgentState::Idle));
        assert_eq!(arms(&log).len(), 1, "and the deferred arm fires from it");

        // An agent that exits publishes `None`. That is "not mid-turn", not "no information".
        o.agent_state = Some(crate::tabmeta::AgentState::Busy);
        o.on_active_changed(None, None, None, None);
        assert_eq!(
            o.agent_state, None,
            "an absent state overwrites Busy, or the suppression latches forever"
        );
    }

    #[test]
    fn an_exited_agent_executes_the_deferred_arm_rather_than_latching_busy() {
        // The `Some(Busy) -> None` half of R2.4's "anything other than Busy, INCLUDING absent".
        // The sibling test above proves the STORE is unconditional; this proves the store
        // actually releases the deferred arm, which is the behaviour the store exists for and
        // which was previously covered by argument rather than by assertion.
        //
        // This is the common shape of a finished session: the user quits `claude`, the process
        // disappears, the worker publishes no `Agent` at all, and that session's pill must
        // still be able to go IDLE on schedule. Reading the absence as "keep the last state"
        // would leave it CONNECTED forever.
        let (mut o, log) = armed_panel(30_000);
        o.agent_state = Some(crate::tabmeta::AgentState::Busy);
        let t = last_event(&o);
        o.poll();
        assert!(arms(&log).is_empty(), "Busy defers the arm");

        o.on_active_changed(None, None, None, None);
        assert_eq!(
            arms(&log),
            vec![Some(dashboard::idle_deadline_ms(t))],
            "an exited agent is not mid-turn, so the deferred arm executes"
        );
    }

    #[test]
    fn r2_7_a_past_due_deadline_is_never_armed() {
        // Opening onto a session last written 19 minutes ago: `status` already returns IDLE,
        // so the model is correct with no wake at all. Arming would queue an immediately-due
        // wake to recompute a value that is already right.
        let (mut o, log) = armed_panel(19 * 60_000);
        o.poll();
        assert_eq!(
            o.published.as_ref().map(|m| m.status),
            Some(renderer::monitor_model::PanelStatus::Idle),
            "the fixture is past the threshold"
        );
        assert!(arms(&log).is_empty());
    }

    #[test]
    fn r2_8_a_detached_snapshot_arms_nothing() {
        // `status` returns NO SESSION whenever the snapshot is detached, never CONNECTED, so
        // the pill cannot be the thing a deadline would correct. Left unstated, this would
        // cost one wake and one full re-shape per detached quiet session, to move tiles the
        // spec puts out of scope.
        let (mut o, log) = armed_panel(30_000);
        o.session.attach.attached = false;
        o.poll();
        assert_eq!(
            o.published.as_ref().map(|m| m.status),
            Some(renderer::monitor_model::PanelStatus::NoSession)
        );
        assert!(arms(&log).is_empty());
    }

    #[test]
    fn r2_9_option_transitions_follow_the_decision_table() {
        // `last_event_ms` is an `Option`, so "newer" has to be defined over the Option and not
        // just over the number.
        let (mut o, log) = armed_panel(30_000);

        // None -> Some(t): the ordinary first arming.
        o.session.latest = Some(Arc::new(TelemetrySnapshot::default()));
        o.poll();
        assert!(arms(&log).is_empty(), "no timestamp, nothing to arm from");
        o.session.latest = Some(snapshot_aged(30_000));
        let t = last_event(&o);
        o.poll();
        assert_eq!(arms(&log), vec![Some(dashboard::idle_deadline_ms(t))]);

        // Some(a) -> Some(b) with b <= a: a re-ordered or duplicate record must not push the
        // deadline out. The pending one is left exactly as it is.
        o.session.latest = Some(snapshot_aged(45_000));
        o.poll();
        assert_eq!(
            arms(&log).len(),
            1,
            "an older timestamp must not move the deadline"
        );

        // Some(_) -> None: a re-point to a log with no parseable time. `status` reads IDLE, so
        // the model is already right and a deadline computed from a timestamp that no longer
        // exists has nothing to assert.
        o.session.latest = Some(Arc::new(TelemetrySnapshot::default()));
        o.poll();
        assert_eq!(
            arms(&log),
            vec![Some(dashboard::idle_deadline_ms(t)), None],
            "the pending deadline is disarmed, once"
        );
    }

    #[test]
    fn r2_10_closing_the_panel_disarms() {
        // Nothing costs anything while the panel is closed, and no later wake may flip a
        // reopened panel on the strength of a deadline armed before it closed.
        let (mut o, log) = armed_panel(30_000);
        o.poll();
        assert_eq!(arms(&log).len(), 1);

        o.close();
        assert_eq!(
            arms(&log).last(),
            Some(&None),
            "closing disarms the pending deadline"
        );

        // And a closed panel keeps arming nothing, without repeating the disarm.
        let after = arms(&log).len();
        for _ in 0..4 {
            o.poll();
        }
        assert_eq!(arms(&log).len(), after, "a closed panel is silent");
    }

    /// **A finding, pinned rather than fixed: a LIVE agent card's timer does move the badge.**
    ///
    /// R6 asks whether a rebuild whose only delta is clock-derived can disturb the reading
    /// position, and for the tiles, the footer and the pill the answer is no, by construction —
    /// none of them is part of a rung. But a *live* subagent's card carries `agent_elapsed` in
    /// its `value`, and `rung_key` hashes `c.value`, so that rung's key changes every second
    /// the agent runs. To `count_new_above` a changed key is a new item, so a reader who has
    /// scrolled away from the bottom watches the new-items badge climb by one per second while
    /// nothing has arrived. That is exactly the shape D17 warned about — "a silent badge that
    /// counts new items every time a clock ticks would be a worse defect than the one being
    /// fixed, and it would look like a feature".
    ///
    /// It is **pre-existing**: `refresh_model` has always run this path on watcher updates, and
    /// a running subagent produces those constantly. What this feature changes is the floor —
    /// D2 now reaches the same path on wakes that carried nothing, so the badge climbs on a
    /// metadata tick too, not only when the log moved.
    ///
    /// It is **not fixed here**, and that is a scope judgement rather than an oversight. The
    /// fix is in `rung_key`, as T4 anticipated, but choosing it means deciding whether a card
    /// whose timer advanced is "the same row" — which is D19's keys-on-appearance rule, an
    /// approved decision this spec does not reopen. The spec's own instruction for this case is
    /// that it is a finding. So the behaviour is pinned here, where a future fix will trip over
    /// it and read the argument, instead of being left for someone to rediscover as a bug.
    #[test]
    fn a_live_agents_ticking_timer_does_move_the_badge_and_that_is_recorded_not_fixed() {
        let (mut o, _log) = armed_panel(30_000);
        let last = last_event(&o);
        let with_agent_running_for = |elapsed_ms: u64| {
            Arc::new(TelemetrySnapshot {
                first_event_ms: Some(now_ms() - 600_000),
                last_event_ms: Some(last),
                subagents: vec![ai_monitor::telemetry::Subagent {
                    id: "t1".into(),
                    subagent_type: Some("researcher".into()),
                    start_ms: Some(now_ms() - elapsed_ms),
                    // The state the crate models explicitly, and the one that keeps a timer
                    // climbing: an in-flight agent, or one whose ending was never observed.
                    completed: false,
                    description: None,
                    model: None,
                    end_ms: None,
                    agent_id: None,
                    ended_ok: true,
                }],
                ..Default::default()
            })
        };

        o.session.latest = Some(with_agent_running_for(60_000));
        o.poll();

        // Put the reader at the bottom, so the agent card is above them and counts.
        let n = o.keys.len();
        let held = o.keys.last().cloned().expect("the document has rungs");
        o.anchor = Anchor::Held {
            key: held,
            index: n - 1,
        };
        o.poll();
        assert_eq!(o.handle.scroll().badge, 0, "nothing new yet");

        // One second of the agent running. No row arrived, no row changed meaning.
        o.session.latest = Some(with_agent_running_for(61_000));
        o.poll();
        assert_eq!(
            o.handle.scroll().badge,
            1,
            "TODAY'S BEHAVIOUR, pinned deliberately: the ticking timer reads as one new \
             item. If this assertion starts failing, `rung_key` has been narrowed — check \
             that the narrowing was argued against D19 before accepting it."
        );
    }

    // ---- idle-timestamps T4 / R6: view state survives a clock-only rebuild ----
    //
    // Nothing here was known to be broken. R6 exists because D17 found that the design
    // *asserts* key-set stability under a tile-only change without anywhere proving it, and
    // D2 is what makes the question urgent: before this feature the ladder, the anchor walk
    // and the badge count ran only when the watcher delivered CONTENT. They now run on time —
    // on a wake whose only delta is a tile ticking from `4m03s` to `4m04s`. A badge that
    // counted "new items" every time a clock ticked would be a worse defect than the one
    // being fixed, and it would look like a feature.
    //
    // The answer, now pinned rather than assumed: `rung_key` is built from the section label
    // plus the entry's own fields, and the tiles, the footer and the pill are none of those —
    // they are siblings of `sections` on the model, not members of it. So a clock-only rebuild
    // cannot move a key, hence cannot move the badge or the anchor.

    /// A rebuild in which only the UPTIME tile has ticked.
    ///
    /// `first_event_ms` moves and `last_event_ms` does not, so the tile changes while the
    /// footer (`clock(last_event_ms)`) and every section stay exactly as they were.
    fn tick_only_the_uptime_tile(o: &mut AiMonitorOverlay, age_ms: u64) {
        let last = last_event(o);
        o.session.latest = Some(Arc::new(TelemetrySnapshot {
            first_event_ms: Some(now_ms() - age_ms),
            last_event_ms: Some(last),
            ..Default::default()
        }));
    }

    #[test]
    fn a_rebuild_sizes_the_fold_vector_to_the_sections_and_starts_them_shut() {
        // Found by `cargo mutants`, not by reading: the fold resize could be turned into a
        // no-op with the whole suite green. Every section must get a stamp, so the vector has
        // to track the section count — and a fresh panel starts every foldable section SHUT,
        // which is the decision that stamp exists to carry.
        let (mut o, _log) = armed_panel(30_000);
        assert!(o.folds.is_empty(), "a fresh panel has decided nothing yet");
        o.poll();

        let sections = o
            .published
            .as_ref()
            .map(|m| m.sections.len())
            .expect("the first rebuild published");
        assert_eq!(
            o.folds.len(),
            sections,
            "one fold decision per section, or the last sections never get stamped"
        );
        assert!(
            o.published
                .as_ref()
                .is_some_and(|m| m.sections.iter().all(|s| s.collapsed)),
            "and every section starts shut"
        );
    }

    #[test]
    fn an_anchor_that_resolves_back_to_the_top_returns_to_following() {
        // Also a mutants finding. `refresh_model` re-arms the anchor from the document it just
        // built, and the `top > 0` guard is what keeps D37's rule — the mode is a function of
        // the index, so an anchor that has landed on rung zero is not "held at the top", it is
        // Following. Without the guard the panel latches into Held at index 0 and stops
        // following new arrivals, which looks like a frozen panel rather than a bug.
        let (mut o, _log) = armed_panel(30_000);
        o.poll();
        let top_key = o.keys.first().cloned().expect("the document has rungs");
        o.anchor = Anchor::Held {
            key: top_key,
            index: 0,
        };

        tick_only_the_uptime_tile(&mut o, 31_000);
        o.poll();
        assert!(
            matches!(o.anchor, Anchor::Following),
            "an anchor resolving to rung zero is Following, not Held at zero"
        );
    }

    #[test]
    fn r6_1_a_tile_only_change_does_not_move_the_new_items_badge() {
        let (mut o, _log) = armed_panel(30_000);
        assert!(o.poll(), "the first publish");
        assert_eq!(o.handle.scroll().badge, 0);

        for extra in 1..6 {
            tick_only_the_uptime_tile(&mut o, 30_000 + extra * 1_000);
            o.poll();
            assert_eq!(
                o.handle.scroll().badge,
                0,
                "a ticking clock is not an arriving item"
            );
        }
    }

    #[test]
    fn r6_2_a_tile_only_change_does_not_move_a_held_anchor() {
        let (mut o, _log) = armed_panel(30_000);
        o.poll();

        // Hold the reader somewhere that is not the top, keyed on what that rung displays.
        let held = o.keys.get(2).cloned().expect("the document has rungs");
        o.anchor = Anchor::Held {
            key: held.clone(),
            index: 2,
        };

        for extra in 1..6 {
            tick_only_the_uptime_tile(&mut o, 30_000 + extra * 1_000);
            o.poll();
            assert_eq!(
                o.handle.scroll().top,
                2,
                "the reading position must not slide because a tile ticked"
            );
            assert!(
                matches!(&o.anchor, Anchor::Held { key, index } if *key == held && *index == 2),
                "and the anchor must still be held on the same rung"
            );
        }
    }

    #[test]
    fn r6_3_fold_state_survives_every_rebuild() {
        // Fine by inspection — `self.folds` is stamped onto the fresh model independently of
        // its contents — and pinned anyway, because "by inspection" is what D17 found the rest
        // of this area resting on.
        let (mut o, _log) = armed_panel(30_000);
        o.poll();
        let n = o.published.as_ref().map(|m| m.sections.len()).unwrap_or(0);
        assert!(n > 3, "the panel has its six sections");

        o.toggle_fold(3);
        let want = o.folds[3];

        for extra in 1..8 {
            tick_only_the_uptime_tile(&mut o, 30_000 + extra * 1_000);
            o.poll();
            assert_eq!(o.folds[3], want, "the user's fold decision is theirs");
            assert_eq!(
                o.published.as_ref().map(|m| m.sections[3].collapsed),
                Some(want),
                "and it is stamped onto every rebuilt model"
            );
        }
    }

    // ---- idle-timestamps T2 / R1: the rebuild rides the wake, the diff gates the frame ----

    /// A snapshot whose first and last event both landed `age_ms` ago.
    ///
    /// Ageing the *snapshot* is how these tests advance the clock. `refresh_model` reads
    /// `now_ms()` internally and there is no seam to inject it through, but the tiles are
    /// `dur(now - first_event_ms)`, so moving the timestamp one second further into the past
    /// is arithmetically identical to waiting one second — `dur` cannot tell them apart, and
    /// the resulting model is the model a real tick produces.
    fn snapshot_aged(age_ms: u64) -> Arc<TelemetrySnapshot> {
        let now = now_ms();
        Arc::new(TelemetrySnapshot {
            first_event_ms: Some(now - age_ms),
            last_event_ms: Some(now - age_ms),
            ..Default::default()
        })
    }

    /// An **open** panel holding such a snapshot, with no watcher and no waker.
    ///
    /// `detached()` is exactly the right shape for R1: every `poll` on it finds nothing to
    /// drain, which is the case the old `poll` declined outright and the one D2 exists for.
    fn open_on(age_ms: u64) -> AiMonitorOverlay {
        let mut o = AiMonitorOverlay::detached();
        o.visible = true;
        o.session.attach.attached = true;
        o.session.latest = Some(snapshot_aged(age_ms));
        o
    }

    #[test]
    fn r1_1_a_wake_carrying_no_log_data_still_rebuilds_the_model() {
        // D2 — the gap. The watcher carried nothing, so the old `poll` returned false and
        // `refresh_model` never ran; but the same arm goes on to redraw when the waiting band
        // moved, so a frame could be drawn from a model nobody had rebuilt. The rebuild has
        // to ride the wake, not the payload.
        let mut o = open_on(90_000);
        assert_eq!(o.models_built, 0);

        // Nothing has ever been published, so the first model differs by construction.
        assert!(o.poll(), "the first rebuild publishes and earns its frame");
        assert_eq!(
            o.models_built, 1,
            "an empty queue must not skip the rebuild"
        );
        assert_eq!(o.ladders_built, 1);

        // One second later: both tiles tick, so this is a changed model on a wake that still
        // carried no log data whatsoever.
        o.session.latest = Some(snapshot_aged(91_000));
        assert!(
            o.poll(),
            "a ticked tile is a changed model — it earns a frame"
        );
        assert_eq!(o.models_built, 2);
        assert_eq!(o.ladders_built, 2);
    }

    #[test]
    fn r1_2_an_unchanged_model_costs_one_build_and_one_compare() {
        // D9 — asserted by COUNTING the work past the gate, never by comparing the published
        // model. Composing is pure, so an equal model publishes identically whether the
        // ladder ran or not: only the counter separates "skipped" from "recomputed to the
        // same answer", which is the same reason `monitor_panel.rs` counts its own layouts.
        //
        // Five hours old, because `dur()` goes minute-granular above an hour
        // (`{h}h{:02}m`) — so the model genuinely does not move between these polls. Below
        // an hour the tiles tick every second and D13 says the gate would rarely fire, which
        // is the honest cost this feature accepts rather than the property under test here.
        let mut o = open_on(5 * 3_600_000);
        assert!(o.poll());
        let (built, ladders) = (o.models_built, o.ladders_built);

        for _ in 0..8 {
            assert!(!o.poll(), "an unchanged model must not ask for a frame");
        }
        assert_eq!(
            o.models_built,
            built + 8,
            "every wake still composes a model — that is the cost D13 accepts"
        );
        assert_eq!(
            o.ladders_built, ladders,
            "and not one of them walked the ladder, keyed a rung or counted a badge"
        );
    }

    #[test]
    fn r1_3_a_closed_panel_rebuilds_nothing() {
        // The one gate that survived D2's revision. What D2 rejected was gating on whether a
        // frame is coming; gating on whether the panel exists is required by the Constraints,
        // and a reader of D2's rationale could otherwise think both gates died together.
        let mut o = open_on(90_000);
        o.visible = false;
        for _ in 0..5 {
            assert!(!o.poll());
        }
        assert_eq!(
            o.models_built, 0,
            "a closed panel composes no model, so it cannot cost a frame"
        );
    }

    #[test]
    fn r1_4_the_rebuild_hangs_off_the_wake_arm_not_the_redraw_seam() {
        // D1 — `redraw()` has 24 call sites and fires on every `CursorMoved` of a sidebar
        // resize drag, which `monitor_panel.rs` names as the exact load stage one's cache was
        // built to absorb. Hooking the rebuild there would differ on most frames of a drag,
        // because the tiles tick every second below an hour, and would re-shape the whole
        // document per drag frame. So the number of rebuilds must be bounded by the wake
        // count, and that is asserted in BOTH the ways it can go wrong.
        //
        // Half one, behavioural, and the one the requirement actually states. `layer()` is the
        // overlay's entire frame-facing surface: the app asks for a draw layer and the renderer
        // composes from the published handle. It takes `&self`, so a frame structurally cannot
        // rebuild — but "structurally cannot" is what a later `&mut self` signature change
        // would quietly undo, so the count is asserted rather than reasoned about.
        let (mut o, _log) = armed_panel(30_000);
        o.poll();
        let after_one_wake = o.models_built;

        for _ in 0..60 {
            let _ = o.layer();
        }
        assert_eq!(
            o.models_built, after_one_wake,
            "60 drag frames must not compose a single model — this is the R7.3 regression, \
             and it is invisible in output because composing is pure"
        );

        o.poll();
        assert_eq!(
            o.models_built,
            after_one_wake + 1,
            "the count tracks WAKES; 60 frames plus one wake is one more rebuild, not 61"
        );

        // Half two, structural: the rebuild is reached from exactly one site, and that site is
        // the wake arm rather than the `redraw()` seam.
        let src = include_str!("terminal_app.rs");
        let (production, _) = src
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("terminal_app.rs has a test module to split on");
        let code = production
            .lines()
            .map(|l| l.split("//").next().unwrap_or(l))
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(
            code.matches("poll_all()").count(),
            1,
            "the panel's model is rebuilt from exactly one site; a second one is the \
             `redraw()` seam creeping back in"
        );
        let arm = code
            .split("UserEvent::OverlayPending =>")
            .nth(1)
            .expect("the wake arm exists");
        assert!(
            arm.contains("poll_all()"),
            "and that site is the OverlayPending arm"
        );
    }
}
