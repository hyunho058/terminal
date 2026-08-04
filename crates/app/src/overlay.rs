//! ai-monitor overlay (D4): a Cmd+Shift+M toggle overlay showing the active
//! session's folded Claude JSONL telemetry, registered through the SD4 `Overlay` seam and
//! drawn via the renderer's `overlay_layer` (SD3). Read-only (C3).
//!
//! Visibility-gated watcher lifecycle (D6): opening resolves the active tab's log
//! (from `cwd`), folds the existing file into a `SessionTelemetry`, and hands that
//! accumulator to a tail watcher which sends back `Arc<TelemetrySnapshot>`s; hiding
//! drops the watcher. Switching tabs while open re-points (D5/R3.1).

use std::path::PathBuf;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ai_monitor::telemetry::{SessionTelemetry, TelemetrySnapshot};
use ai_monitor::{
    backfill_telemetry, project_dir, resolve_owned_session, LogWatcher, Wanted, Watch,
};
use renderer::overlay_layer::{OverlayHandle, OverlayLayer, OverlayModel};
use renderer::DrawLayer;
use winit::event_loop::EventLoopProxy;
use winit::keyboard::Key;

use crate::dashboard;
use crate::seam::{AppCtx, Handled, KeyInput, Overlay};
use crate::tabsession;
use crate::terminal_app::UserEvent;

pub struct AiMonitorOverlay {
    visible: bool,
    handle: OverlayHandle,
    /// The newest snapshot the watcher folded. `None` means exactly one thing: no
    /// log was resolved for this tab. Because `open()` seeds it from the backfill
    /// before spawning, it is `Some` immediately after a successful open — never
    /// "opened but nothing folded yet".
    latest: Option<Arc<TelemetrySnapshot>>,
    /// The re-point bookkeeping: which notification this panel was last opened for, and the
    /// rule for deciding whether a new one is a real change (D6/R4.3). Owning it as a unit
    /// rather than as loose fields is what puts the *choice of key* under test — the storm
    /// bug was in which value the call site stored, not in the comparison.
    repoint: Repoint,
    watcher: Option<LogWatcher>,
    proxy: EventLoopProxy<UserEvent>,
    /// This window's start time. A log whose last event precedes it stopped being
    /// written before the window existed, so it belongs to some other session.
    /// Captured here rather than at open, because open happens *after* the tab's own
    /// session may have started writing.
    started_ms: u64,
    /// Which session is being shown, and whether one has been settled on at all.
    /// Cleared only by `open()`.
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
    tab_has_claude: bool,
}

impl AiMonitorOverlay {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        AiMonitorOverlay {
            visible: false,
            handle: OverlayHandle::default(),
            latest: None,
            repoint: Repoint::default(),
            watcher: None,
            proxy,
            started_ms: now_ms(),
            attach: Attachment::default(),
            attached_flag: Arc::new(AtomicBool::new(false)),
            context_window: None,
            tab_has_claude: false,
            rivals: Arc::new(Mutex::new(ai_monitor::RivalSet::default())),
            refreshing: Arc::new(AtomicBool::new(false)),
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
        // Re-read on open rather than holding a live handle: `settings::apply` does
        // not reach the overlays, and a value that takes effect the next time the
        // panel is opened is honest about that. A malformed config keeps the table.
        self.context_window = common::config::Config::load()
            .ok()
            .and_then(|c| c.ai_monitor.context_window);
        // One call does both halves of the bookkeeping: it records the *notified* value as the
        // re-point key and returns the *resolved* value to open on, preferring the tmux pane's
        // cwd and falling back to the tab's own (R3.3). Keeping the two straight is the whole
        // storm fix, so it lives in a unit a test can drive (see `Repoint`).
        let cwd = self.repoint.opened(cwd, shell_pid, Self::pane_cwd);
        // Ask the tab's own process tree which session it is running. This is the
        // only signal that distinguishes it from a session another application is
        // running in the same project directory.
        let h = shell_pid.and_then(tabsession::hint_for);
        // Recorded from the `Option`'s *shape*, before `unwrap_or_default()` destroys
        // it: `None` means no `claude` under this tab, while an empty-but-present hint
        // means one is running that named neither an id nor a start time. Assigned
        // here — above the `project_dir` branch below — so a tab running `claude` on a
        // cwd with no project directory still gets the "has not written a log yet"
        // wording rather than being told there is no session at all. Unconditional, so
        // re-pointing to a tab with no `claude` clears it.
        self.tab_has_claude = h.is_some();
        let hint = h.unwrap_or_default();
        self.latest = None;
        self.attach = Attachment::default();
        // A fresh flag rather than a store: the previous watcher may still be
        // running for an instant, and it must not observe this open's decision.
        self.attached_flag = Arc::new(AtomicBool::new(false));
        self.watcher = None;

        if let Some(dir) = cwd.as_deref().and_then(project_dir) {
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
                    since_ms: hint.started_ms,
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
            let (telemetry, offset) = match log.as_deref() {
                Some(p) => backfill_telemetry(p),
                None => (SessionTelemetry::new(), 0),
            };
            self.note(Arc::new(telemetry.snapshot()), owned);
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
                watch_with_rivals(rivals, dir, log, want, owned, self.attached_flag.clone()),
                telemetry,
                offset,
                move || {
                    let _ = proxy.send_event(UserEvent::OverlayPending);
                },
            );
        }
        self.visible = true;
        self.refresh_model();
    }

    /// Record a snapshot and, once, the fact that it proved ownership.
    ///
    /// `attached` only ever goes false -> true within an open. `last_event_ms` is a
    /// last-seen value, not a maximum, so a single out-of-order timestamp would
    /// otherwise revoke an attachment mid-session and blank the panel.
    fn note(&mut self, snap: Arc<TelemetrySnapshot>, owned: bool) {
        if !self.attach.admit(&snap, self.started_ms, owned) {
            return;
        }
        self.attached_flag
            .store(self.attach.attached, Ordering::Relaxed);
        self.latest = Some(snap);
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
        if self.attached_flag.load(Ordering::Relaxed) {
            return; // R6.3 — attached: the watcher no longer re-resolves
        }
        if self.refreshing.swap(true, Ordering::AcqRel) {
            return; // one outstanding refresh is enough
        }
        let handle = self.rivals.clone();
        // Cleared by `Drop`, not by a statement at the end of the closure. A panic anywhere
        // in `hint_for` would skip a manual `store(false)` and latch this `true` for the
        // life of the process — rival refresh silently dead forever, with nothing to
        // indicate it. Low probability, unbounded consequence, four lines to remove.
        let inflight = InFlight(self.refreshing.clone());
        std::thread::spawn(move || {
            let set = match tabsession::hint_for(pid) {
                Some(h) => ai_monitor::RivalSet {
                    list: h.rival_starts,
                    known: h.rivals_known,
                },
                None => ai_monitor::RivalSet::default(),
            };
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

    /// Push the current visibility + composed lines into the draw layer.
    ///
    /// All layout lives in [`crate::dashboard`] (F5) — this is wiring: supply the
    /// clock, hand over the snapshot, publish the result.
    fn refresh_model(&self) {
        let env = dashboard::Env {
            now_ms: now_ms(),
            attached: self.attach.attached,
            context_window: self.context_window,
            tab_has_claude: self.tab_has_claude,
        };
        let lines = dashboard::compose(self.latest.as_deref(), &env);
        self.handle.set(OverlayModel {
            visible: self.visible,
            lines,
        });
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
    fn opened(
        &mut self,
        notified: Option<PathBuf>,
        shell_pid: Option<u32>,
        resolve: impl FnOnce(Option<u32>) -> Option<PathBuf>,
    ) -> Option<PathBuf> {
        self.key_cwd = notified.clone();
        self.key_pid = shell_pid;
        resolve(shell_pid).or(notified)
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

/// Clears the in-flight flag on drop, so a panicking refresh worker cannot latch it.
struct InFlight(Arc<AtomicBool>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
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
        want,
        owned,
        rivals,
        attached,
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
            ctx.request_redraw();
            return Handled::Consumed; // R2.1 — never reaches the shell
        }
        // A monitor doesn't capture other keys.
        Handled::Passthrough
    }

    fn layer(&self) -> Box<dyn DrawLayer> {
        Box::new(OverlayLayer::from_handle(self.handle.clone()))
    }

    fn poll(&mut self) -> bool {
        let Some(watcher) = self.watcher.as_ref() else {
            return false;
        };
        let newest = drain_to_newest(&watcher.rx);
        let changed = newest.is_some();
        if let Some(update) = newest {
            self.note(update.snap, update.owned);
            self.refresh_model();
        }
        changed
    }

    fn on_active_changed(&mut self, cwd: Option<PathBuf>, shell_pid: Option<u32>) {
        if self
            .repoint
            .changed(self.visible, cwd.as_deref(), shell_pid)
        {
            self.open(cwd, shell_pid);
        } else if self.visible {
            // Same tab, still open: republish the rival set rather than re-opening. D6
            // fires this after EVERY metadata sync, so this is the cadence D11 wants —
            // and it is why the refresh must not re-run `open`, which would drop the
            // watcher and re-run the full backfill (the storm `Repoint` exists to stop).
            self.refresh_rivals(shell_pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{drain_to_newest, pane_cwd_with, proves_ownership, Attachment, Repoint};
    use crate::tabsession;
    use ai_monitor::telemetry::TelemetrySnapshot;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

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
            session_id: None,
            started_ms: Some(1_000),
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
            session_id: None,
            started_ms: Some(1_000),
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

        assert!(
            Arc::ptr_eq(&slot, &watch.rivals),
            "the watcher must read the handle refresh_rivals writes"
        );

        // And a membership change through the slot is visible to the Watch's copy.
        slot.lock().unwrap().list.clear();
        assert!(
            watch.rivals.lock().unwrap().list.is_empty(),
            "a refresh must reach the watcher, or D11 is inert"
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
        let opened_on = r.opened(Some(PathBuf::from(CLIENT)), Some(8060), |_| {
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
        r.opened(Some(PathBuf::from("/proj/a")), Some(99), |_| None);
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
        r.opened(Some(PathBuf::from("/proj/a")), Some(99), |_| None);
        assert!(!r.changed(true, Some(Path::new("/proj/a")), Some(99)));

        let mut empty = Repoint::default();
        empty.opened(None, None, |_| None);
        assert!(!empty.changed(true, None, None));
    }

    #[test]
    fn a_hidden_overlay_never_repoints() {
        // R4.3 — a closed panel holds no watcher and must not spin one up.
        let mut r = Repoint::default();
        r.opened(Some(PathBuf::from("/proj/a")), Some(99), |_| None);
        assert!(!r.changed(false, Some(Path::new("/proj/b")), Some(1)));
        assert!(!r.changed(false, None, None));
    }

    #[test]
    fn with_no_tmux_the_key_and_the_opened_directory_agree() {
        // A non-tmux tab resolves nothing, so both the key and the directory are the notified
        // value — and the first published cwd equal to it is correctly inert.
        let mut r = Repoint::default();
        let opened_on = r.opened(Some(PathBuf::from(CLIENT)), Some(4242), |_| None);
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
}
