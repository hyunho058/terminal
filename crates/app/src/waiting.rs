//! The app-side waiting-state assembly model (T3) — pure policy, no rendering.
//!
//! It consumes two raw signal sources — OSC 133 command-end events (T1, via
//! [`Session::take_command_end`](crate::session::Session::take_command_end)) and
//! agent activity (T2, via [`AgentState`]) — and produces an ordered list of
//! [`WaitingItem`]s for the sidebar waiting band. It never renders; T4 reads
//! [`WaitingState::items`] and draws the band.
//!
//! All UI policy lives HERE so `term-core` stays policy-free (spec D21): the ≥30s
//! completion threshold, the `{1, 130}` error ignore-list, per-session precedence
//! (Error > Waiting > Completion, D17), active-tab exclusion (D15), acknowledge-
//! on-activate (D12), and rising-edge agent-waiting detection (D4/R3).
//!
//! Determinism: every mutating method takes `now: Instant` as a **parameter** and
//! never calls `Instant::now()` itself, so the policy is unit-testable with
//! injected clocks.

use std::collections::HashMap;
use std::time::Instant;

use term_core::CommandEnd;

use crate::session::TabId;
use crate::tabmeta::AgentState;

/// Minimum foreground run time for a clean (exit 0) command to raise a Completion
/// signal (spec D5/R5.1); below it a quick command is noise (R5.2). This threshold
/// lives in the app, not `term-core` (D21). Tunable — a config knob is a Known Gap.
const COMPLETION_THRESHOLD_MS: u64 = 30_000;

/// Exit codes that never raise an Error signal: `0` = success, `1` = generic
/// failure (e.g. `grep` no-match), `130` = SIGINT / Ctrl-C (spec D7/R6.2). Any
/// other non-zero code is an error. This ignore-list lives in the app (D21).
const ERROR_IGNORE: [i32; 3] = [0, 1, 130];

/// Why a session needs attention. Precedence for a session with several pending
/// signals is Error > Waiting > Completion (spec D17) — encoded by the check order
/// in [`TabPending::displayed`], not by this enum's declaration order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WaitingKind {
    /// A finished foreground command exited with a code outside `{0, 1, 130}` (D7).
    Error,
    /// The agent finished its turn and is waiting for MY input (D4).
    Waiting,
    /// A clean foreground command ran at least `COMPLETION_THRESHOLD_MS` (D5).
    Completion,
}

/// One waiting-band entry: which session needs attention, why, and since when.
/// At most one per session (spec D17/R2.4). `since` is the trigger time of the
/// winning (highest-precedence) pending signal and is the sort key (D2/R1.5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WaitingItem {
    pub tab_id: TabId,
    pub kind: WaitingKind,
    pub since: Instant,
}

/// Per-session pending signals plus the last observed agent state. Each signal
/// slot holds the trigger time of its **first, still-unacknowledged** occurrence
/// (first-wins): "oldest wait first" then reflects how long the session has
/// demanded attention, and a repeat of the same kind while still pending does not
/// reset the clock.
#[derive(Default)]
struct TabPending {
    error: Option<Instant>,
    waiting: Option<Instant>,
    completion: Option<Instant>,
    /// Last agent state observed for this tab; `None` = never observed. Compared
    /// against the next observation to detect the rising edge into `Waiting`. A
    /// factual mirror — acknowledge does NOT reset it, so recurrence needs a real
    /// new transition (R2.2), not just a re-poll.
    prev_agent: Option<AgentState>,
}

impl TabPending {
    /// Record a signal, keeping the earliest trigger time (first-wins).
    fn raise(&mut self, kind: WaitingKind, now: Instant) {
        let slot = match kind {
            WaitingKind::Error => &mut self.error,
            WaitingKind::Waiting => &mut self.waiting,
            WaitingKind::Completion => &mut self.completion,
        };
        slot.get_or_insert(now);
    }

    /// The single displayed signal: highest precedence present (Error > Waiting >
    /// Completion, D17), paired with that signal's trigger time (D2). `None` when
    /// nothing is pending.
    fn displayed(&self) -> Option<(WaitingKind, Instant)> {
        // Highest precedence first — the first present slot wins (D17).
        self.error
            .map(|t| (WaitingKind::Error, t))
            .or_else(|| self.waiting.map(|t| (WaitingKind::Waiting, t)))
            .or_else(|| self.completion.map(|t| (WaitingKind::Completion, t)))
    }
}

/// The waiting-band assembly model: per-session pending signals + rising-edge
/// agent tracking. Pure policy — the app feeds it raw signals and reads back the
/// ordered [`items`](WaitingState::items).
#[derive(Default)]
pub struct WaitingState {
    tabs: HashMap<TabId, TabPending>,
}

impl WaitingState {
    pub fn new() -> WaitingState {
        WaitingState::default()
    }

    /// Feed one OSC 133 command-end for `tab` (spec D5/D7/D14). No-op for the
    /// active tab (D15/R2.3). Applies the app-side policy (D21): a code outside
    /// `{0, 1, 130}` is an Error (even past the completion threshold — error wins,
    /// D14/R6.3); a clean exit that ran ≥ `COMPLETION_THRESHOLD_MS` is a
    /// Completion; everything else is ignored (R5.2/R6.2). (`&` background jobs
    /// emit no OSC 133 D marker, so they never reach here — R5.3.) Returns whether
    /// the displayed item set changed, so the caller can redraw.
    pub fn on_command_end(
        &mut self,
        tab: TabId,
        active: TabId,
        end: CommandEnd,
        now: Instant,
    ) -> bool {
        if tab == active {
            return false; // the active tab never produces a waiting item (D15)
        }
        let Some(kind) = classify(end) else {
            return false; // ignored code / short command → no signal
        };
        let before = self.items();
        self.tabs.entry(tab).or_default().raise(kind, now);
        self.items() != before
    }

    /// Feed the current agent state of every known tab (T2). Rising-edge semantics
    /// (spec D4/R3): a session transitioning INTO `Waiting` raises a Waiting signal
    /// (unless it is the active tab, D15); a session leaving `Waiting` drops its
    /// still-unacknowledged Waiting signal; staying in `Waiting` does nothing (no
    /// duplicate, no reset). Returns whether the displayed item set changed.
    pub fn on_agent_states(
        &mut self,
        states: &[(TabId, AgentState)],
        active: TabId,
        now: Instant,
    ) -> bool {
        let before = self.items();
        for &(tab, state) in states {
            let slot = self.tabs.entry(tab).or_default();
            let was_waiting = slot.prev_agent == Some(AgentState::Waiting);
            let is_waiting = state == AgentState::Waiting;
            if is_waiting && !was_waiting {
                if tab != active {
                    // Rising edge into Waiting on a tab the user is not watching → raise,
                    // and record the transition as consumed.
                    slot.raise(WaitingKind::Waiting, now);
                    slot.prev_agent = Some(state);
                }
                // Otherwise the edge happened on the tab the user is already looking at.
                // No item (D15/R2.3) — but `prev_agent` is deliberately NOT advanced, so
                // the edge stays PENDING. Recording it here would consume it invisibly:
                // the user would then switch away from a still-blocked agent and the tab
                // could never be raised, because every later poll would see
                // `was_waiting == is_waiting` and find no edge at all. Leaving it pending
                // is what lets the switch-away re-evaluation raise it (see
                // `waiting_seen_while_active_raises_after_switching_away`).
            } else {
                if was_waiting && !is_waiting {
                    // Left Waiting → the pending Waiting signal is gone (unless it was
                    // already acknowledged, in which case this is already `None`).
                    slot.waiting = None;
                }
                slot.prev_agent = Some(state);
            }
        }
        self.items() != before
    }

    /// Acknowledge `tab` — clear ALL of its pending signals (spec D12/R2.1). Called
    /// whenever a session BECOMES the active tab (click or keyboard). `prev_agent`
    /// is kept, so a still-`Waiting` agent will only reappear on a genuine new
    /// rising edge (R2.2), never on a mere re-poll. Returns whether anything
    /// changed.
    pub fn acknowledge(&mut self, tab: TabId) -> bool {
        let before = self.items();
        if let Some(slot) = self.tabs.get_mut(&tab) {
            slot.error = None;
            slot.waiting = None;
            slot.completion = None;
        }
        self.items() != before
    }

    /// The ordered waiting items (spec D2/R1.5): one per session with a pending
    /// signal, sorted by `since` ASCENDING (oldest wait first). Returns the FULL
    /// list — the max-5 + "+N more" visual cap is T4's concern (R1.6/D11). Tab id
    /// breaks `since` ties for deterministic output.
    pub fn items(&self) -> Vec<WaitingItem> {
        let mut out: Vec<WaitingItem> = self
            .tabs
            .iter()
            .filter_map(|(&tab_id, pending)| {
                pending.displayed().map(|(kind, since)| WaitingItem {
                    tab_id,
                    kind,
                    since,
                })
            })
            .collect();
        out.sort_by(|a, b| a.since.cmp(&b.since).then(a.tab_id.0.cmp(&b.tab_id.0)));
        out
    }
}

/// Classify a raw command-end into its signal kind, or `None` when it should be
/// ignored. Error is checked first so it wins over Completion for the same event
/// (a long-running command that also errored — D14/R6.3).
fn classify(end: CommandEnd) -> Option<WaitingKind> {
    if !ERROR_IGNORE.contains(&end.exit_code) {
        Some(WaitingKind::Error)
    } else if end.exit_code == 0 && end.elapsed_ms >= COMPLETION_THRESHOLD_MS {
        Some(WaitingKind::Completion)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn cmd(exit_code: i32, elapsed_ms: u64) -> CommandEnd {
        CommandEnd {
            exit_code,
            elapsed_ms,
        }
    }

    /// Distinct, ordered timestamps off a fixed base — deterministic in tests.
    fn t(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn error_exit_raises_error() {
        // R6.1 — a code outside {0,1,130} on an inactive session is an Error.
        let base = Instant::now();
        let mut w = WaitingState::new();
        let (tab, active) = (TabId(1), TabId(0));
        assert!(w.on_command_end(tab, active, cmd(101, 500), t(base, 10)));
        let items = w.items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].tab_id, tab);
        assert_eq!(items[0].kind, WaitingKind::Error);
        assert_eq!(items[0].since, t(base, 10));
    }

    #[test]
    fn ignored_exit_codes_raise_nothing() {
        // R6.2 — 1 (grep no-match) and 130 (Ctrl-C) never signal, even if long.
        let base = Instant::now();
        let mut w = WaitingState::new();
        let (tab, active) = (TabId(1), TabId(0));
        assert!(!w.on_command_end(tab, active, cmd(1, 90_000), t(base, 1)));
        assert!(!w.on_command_end(tab, active, cmd(130, 90_000), t(base, 2)));
        assert!(w.items().is_empty());
    }

    #[test]
    fn long_clean_exit_raises_completion() {
        // R5.1 — a clean exit that ran ≥ 30s is a Completion.
        let base = Instant::now();
        let mut w = WaitingState::new();
        let (tab, active) = (TabId(1), TabId(0));
        assert!(w.on_command_end(tab, active, cmd(0, 40_000), t(base, 5)));
        let items = w.items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, WaitingKind::Completion);
    }

    #[test]
    fn short_clean_exit_raises_nothing() {
        // R5.2 — a clean exit under the threshold is noise.
        let base = Instant::now();
        let mut w = WaitingState::new();
        let (tab, active) = (TabId(1), TabId(0));
        assert!(!w.on_command_end(tab, active, cmd(0, 5_000), t(base, 1)));
        assert!(w.items().is_empty());
    }

    #[test]
    fn error_beats_completion_same_session() {
        // R6.3 / D14 — a completion then an error on the same session → one Error
        // item, timestamped at the winning error.
        let base = Instant::now();
        let mut w = WaitingState::new();
        let (tab, active) = (TabId(1), TabId(0));
        assert!(w.on_command_end(tab, active, cmd(0, 40_000), t(base, 10)));
        assert!(w.on_command_end(tab, active, cmd(101, 200), t(base, 20)));
        let items = w.items();
        assert_eq!(items.len(), 1, "one item per session (D17)");
        assert_eq!(items[0].kind, WaitingKind::Error);
        assert_eq!(
            items[0].since,
            t(base, 20),
            "since = the winning signal's time"
        );
    }

    #[test]
    fn precedence_error_over_waiting_over_completion() {
        // R2.4 / D17 — with all three pending, Error wins, then Waiting, then
        // Completion; `since` follows the winner.
        let base = Instant::now();
        let mut w = WaitingState::new();
        let (tab, active) = (TabId(2), TabId(0));
        // Completion pending.
        w.on_command_end(tab, active, cmd(0, 40_000), t(base, 10));
        // Agent enters Waiting → Waiting beats Completion.
        w.on_agent_states(&[(tab, AgentState::Waiting)], active, t(base, 20));
        assert_eq!(w.items()[0].kind, WaitingKind::Waiting);
        // Error pending → beats both.
        w.on_command_end(tab, active, cmd(2, 10), t(base, 30));
        let items = w.items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, WaitingKind::Error);
        assert_eq!(items[0].since, t(base, 30));
    }

    #[test]
    fn active_tab_is_excluded() {
        // R2.3 / D15 — neither source creates an item for the active tab.
        let base = Instant::now();
        let mut w = WaitingState::new();
        let active = TabId(1);
        assert!(!w.on_command_end(active, active, cmd(101, 100), t(base, 10)));
        assert!(!w.on_agent_states(&[(active, AgentState::Waiting)], active, t(base, 20)));
        assert!(w.items().is_empty());
    }

    #[test]
    fn acknowledge_clears_all_signals() {
        // R2.1 / D12 — becoming active clears every pending signal.
        let base = Instant::now();
        let mut w = WaitingState::new();
        let (tab, active) = (TabId(1), TabId(0));
        w.on_command_end(tab, active, cmd(101, 100), t(base, 10));
        w.on_agent_states(&[(tab, AgentState::Waiting)], active, t(base, 20));
        assert!(!w.items().is_empty());
        assert!(w.acknowledge(tab));
        assert!(w.items().is_empty());
        // Acknowledging an already-clear tab changes nothing.
        assert!(!w.acknowledge(tab));
    }

    #[test]
    fn command_signal_recurs_after_acknowledge() {
        // R2.2 — after a clear, a NEW command-end re-creates the item with a fresh
        // trigger time.
        let base = Instant::now();
        let mut w = WaitingState::new();
        let (tab, active) = (TabId(1), TabId(0));
        w.on_command_end(tab, active, cmd(101, 100), t(base, 10));
        w.acknowledge(tab);
        assert!(w.items().is_empty());
        assert!(w.on_command_end(tab, active, cmd(101, 100), t(base, 50)));
        let items = w.items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, WaitingKind::Error);
        assert_eq!(
            items[0].since,
            t(base, 50),
            "fresh trigger time on recurrence"
        );
    }

    #[test]
    fn agent_waiting_recurs_only_on_a_new_rising_edge() {
        // R2.2 + rising-edge — after acknowledge, staying in Waiting does NOT
        // reappear; only a Busy→Waiting transition re-raises it.
        let base = Instant::now();
        let mut w = WaitingState::new();
        let (tab, active) = (TabId(1), TabId(0));
        w.on_agent_states(&[(tab, AgentState::Waiting)], active, t(base, 10));
        assert_eq!(w.items().len(), 1);
        w.acknowledge(tab);
        assert!(w.items().is_empty());
        // Still Waiting (no transition) → stays hidden.
        assert!(!w.on_agent_states(&[(tab, AgentState::Waiting)], active, t(base, 20)));
        assert!(w.items().is_empty());
        // Leave, then re-enter Waiting → a real new rising edge → reappears.
        w.on_agent_states(&[(tab, AgentState::Busy)], active, t(base, 30));
        assert!(w.on_agent_states(&[(tab, AgentState::Waiting)], active, t(base, 40)));
        assert_eq!(w.items().len(), 1);
        assert_eq!(w.items()[0].since, t(base, 40));
    }

    #[test]
    fn waiting_seen_while_active_raises_after_switching_away() {
        // THE dominant real flow, and a shipped-broken bug once: you are WATCHING the tab
        // when its permission dialog opens. The rising edge must not be consumed while it
        // is invisible — otherwise switching away leaves a still-blocked agent that can
        // never be raised, because every later poll sees `was_waiting == is_waiting`.
        let base = Instant::now();
        let mut w = WaitingState::new();
        let (tab, other) = (TabId(1), TabId(2));
        // Dialog opens while `tab` is the active tab → no item (D15), edge left pending.
        assert!(!w.on_agent_states(&[(tab, AgentState::Waiting)], tab, t(base, 10)));
        assert!(w.items().is_empty());
        // Repeated polls while still active must not consume it either.
        assert!(!w.on_agent_states(&[(tab, AgentState::Waiting)], tab, t(base, 20)));
        assert!(w.items().is_empty());
        // User switches to `other`; the re-evaluation on switch now raises it.
        assert!(w.on_agent_states(&[(tab, AgentState::Waiting)], other, t(base, 30)));
        let items = w.items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].tab_id, tab);
        assert_eq!(items[0].kind, WaitingKind::Waiting);
    }

    #[test]
    fn resolved_while_active_does_not_raise_on_switch_away() {
        // The other half: if you actually DEAL with it while watching, leaving must not
        // resurrect it. The pending edge is discharged by the state going back to Busy.
        let base = Instant::now();
        let mut w = WaitingState::new();
        let (tab, other) = (TabId(1), TabId(2));
        w.on_agent_states(&[(tab, AgentState::Waiting)], tab, t(base, 10));
        // Answered → back to work while still active.
        w.on_agent_states(&[(tab, AgentState::Busy)], tab, t(base, 20));
        // Switch away → nothing pending.
        assert!(!w.on_agent_states(&[(tab, AgentState::Busy)], other, t(base, 30)));
        assert!(w.items().is_empty());
    }

    #[test]
    fn acknowledged_tab_does_not_reappear_on_switch_away() {
        // Guards the fix against over-firing: a tab raised while INACTIVE, then visited
        // (acknowledged) and left with the agent still Waiting, must stay gone — the edge
        // was genuinely consumed and seen. Only a real new transition brings it back.
        let base = Instant::now();
        let mut w = WaitingState::new();
        let (tab, other) = (TabId(1), TabId(2));
        // Raised while the user was elsewhere.
        assert!(w.on_agent_states(&[(tab, AgentState::Waiting)], other, t(base, 10)));
        // User visits it.
        w.acknowledge(tab);
        w.on_agent_states(&[(tab, AgentState::Waiting)], tab, t(base, 20));
        assert!(w.items().is_empty());
        // …and leaves again, agent still Waiting → must NOT reappear.
        assert!(!w.on_agent_states(&[(tab, AgentState::Waiting)], other, t(base, 30)));
        assert!(w.items().is_empty());
    }

    #[test]
    fn leaving_waiting_removes_unacknowledged_signal() {
        // Rising/falling edge — an agent that resumes work drops its Waiting signal.
        let base = Instant::now();
        let mut w = WaitingState::new();
        let (tab, active) = (TabId(1), TabId(0));
        w.on_agent_states(&[(tab, AgentState::Waiting)], active, t(base, 10));
        assert_eq!(w.items().len(), 1);
        assert!(w.on_agent_states(&[(tab, AgentState::Busy)], active, t(base, 20)));
        assert!(w.items().is_empty());
    }

    #[test]
    fn staying_in_waiting_does_not_duplicate_or_reset() {
        // Rising-edge — a second poll while still Waiting is a no-op and keeps the
        // original trigger time.
        let base = Instant::now();
        let mut w = WaitingState::new();
        let (tab, active) = (TabId(1), TabId(0));
        assert!(w.on_agent_states(&[(tab, AgentState::Waiting)], active, t(base, 10)));
        assert!(!w.on_agent_states(&[(tab, AgentState::Waiting)], active, t(base, 20)));
        let items = w.items();
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].since,
            t(base, 10),
            "rising-edge time kept, not reset"
        );
    }

    #[test]
    fn items_sorted_by_since_ascending() {
        // R1.5 / D2 — oldest wait first, regardless of feed order.
        let base = Instant::now();
        let mut w = WaitingState::new();
        let active = TabId(0);
        w.on_command_end(TabId(3), active, cmd(101, 1), t(base, 300));
        w.on_command_end(TabId(1), active, cmd(101, 1), t(base, 100));
        w.on_command_end(TabId(2), active, cmd(101, 1), t(base, 200));
        let ids: Vec<u64> = w.items().iter().map(|i| i.tab_id.0).collect();
        assert_eq!(ids, vec![1, 2, 3], "oldest wait first");
    }

    #[test]
    fn graceful_degrade_without_command_end() {
        // R7 — with shell integration off, `take_command_end` is always None so
        // `on_command_end` is never called; agent-waiting still produces items and
        // nothing breaks.
        let base = Instant::now();
        let mut w = WaitingState::new();
        let (tab, active) = (TabId(1), TabId(0));
        assert!(w.items().is_empty());
        assert!(w.on_agent_states(&[(tab, AgentState::Waiting)], active, t(base, 10)));
        assert_eq!(w.items().len(), 1);
        assert_eq!(w.items()[0].kind, WaitingKind::Waiting);
    }
}
