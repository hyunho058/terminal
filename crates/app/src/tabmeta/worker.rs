//! The metadata worker thread (D1, D4). One thread for the whole feature: it blocks on
//! its command channel (which doubles as the sleep via `recv_timeout`), and on each tick
//! derives every rostered tab's [`TabMeta`], publishing only when something changed (D4).
//!
//! T6 wires the real derivations behind the T1 cadence shape: [`Deriver`] holds the git
//! and activity memos and turns a `(TabId, pid)` into a [`TabMeta`] by resolving cwd once
//! (D10), reading the git branch (T4) and walking for an agent (T3) whose activity comes
//! from the JSONL tail (T5). [`should_refresh`] staggers background tabs (D4); the differ
//! ([`publish_if_changed`]) makes publication change-only; every failure degrades to an
//! absent field and the loop cannot panic (D14).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::session::TabId;

use super::activity::ActivityMemo;
use super::git::Memo as GitMemo;
use super::{Agent, AgentKind, AgentState, Cmd, TabMeta, TabMetaShared, TabPid};

/// The base cadence. Active tabs refresh every tick (500 ms); background tabs every
/// `BG_INTERVAL`th tick (2 s), striped by tab slot so they never burst (D4).
pub(crate) const TICK: Duration = Duration::from_millis(500);

/// Background tabs refresh once every this many ticks (D4, R3.1).
const BG_INTERVAL: u64 = 4;

/// Whether a tab refreshes on this tick: the active tab always; a background tab on the
/// tick whose phase matches its slot, so 20 tabs spread across the phases rather than
/// bursting on one tick (R3.1, R3.2).
fn should_refresh(is_active: bool, tick_index: u64, tab_slot: usize) -> bool {
    is_active || (tick_index % BG_INTERVAL == tab_slot as u64 % BG_INTERVAL)
}

/// Owns the per-tick derivation state: the two cwd-keyed memos (D9/D7), the resolved
/// Claude projects root, the display toggles (D12), and the once-per-transition log guard
/// for R6.5. Extracted from the thread body so a derivation pass is unit-testable without
/// spawning the worker (D13(5)).
struct Deriver {
    git_memo: GitMemo,
    act_memo: ActivityMemo,
    projects_root: Option<PathBuf>,
    debug_seen: HashMap<TabId, AgentKind>,
    enabled_git: bool,
    enabled_agent: bool,
}

impl Deriver {
    fn new() -> Deriver {
        Deriver {
            git_memo: GitMemo::new(),
            act_memo: ActivityMemo::new(),
            projects_root: ai_monitor::claude_projects_dir(),
            debug_seen: HashMap::new(),
            enabled_git: true,
            enabled_agent: true,
        }
    }

    /// Derive one tab. cwd is resolved once and shared by the git and activity paths
    /// (D10); each source is skipped entirely when its toggle is off (R1.8).
    fn tab(&mut self, tp: &TabPid, tick: u64, now: SystemTime) -> TabMeta {
        // cwd feeds the git branch and Claude activity; skip it when neither is wanted.
        let cwd = if self.enabled_git || self.enabled_agent {
            super::proc::cwd_of_pid(tp.pid)
        } else {
            None
        };

        let branch = if self.enabled_git {
            cwd.as_deref()
                .and_then(|c| super::git::branch_of_cwd(&mut self.git_memo, c, tick))
        } else {
            None
        };

        let (agent, task) = if self.enabled_agent {
            match super::agent::agent_of_pid(tp.pid) {
                Some(kind) => {
                    let state = self.activity(kind, cwd.as_deref(), tick, now);
                    // The "current task" label (tab-identity): only Claude has a log source
                    // (D7), and `self.activity` just warmed the role memo for this cwd, so
                    // this is a pure memo read — no extra I/O (R4.2). A non-Claude agent or an
                    // unresolved log yields `None`, so the card falls back to the cwd title.
                    let task = if kind == AgentKind::Claude {
                        cwd.as_deref()
                            .and_then(|c| super::activity::label_of_cwd(&self.act_memo, c))
                    } else {
                        None
                    };
                    (Some(Agent { kind, state }), task)
                }
                None => (None, None),
            }
        } else {
            (None, None)
        };

        TabMeta {
            branch,
            agent,
            task,
        }
    }

    /// The activity state for a detected agent. Only Claude has a log source (D7); every
    /// other agent — and a Claude whose log cannot be resolved — is `Unknown` (R6.5).
    fn activity(
        &mut self,
        kind: AgentKind,
        cwd: Option<&Path>,
        tick: u64,
        now: SystemTime,
    ) -> AgentState {
        if kind == AgentKind::Claude {
            if let (Some(root), Some(cwd)) = (self.projects_root.clone(), cwd) {
                return super::activity::state_of_cwd(&mut self.act_memo, &root, cwd, tick, now);
            }
        }
        AgentState::Unknown
    }

    /// A full derivation pass: refresh the tabs due this tick (D4), reuse `prev` for the
    /// rest so unchanged tabs cost nothing and publication stays change-only (R3.3).
    fn pass(
        &mut self,
        roster: &[TabPid],
        active: Option<TabId>,
        tick: u64,
        now: SystemTime,
        prev: &[(TabId, TabMeta)],
    ) -> Vec<(TabId, TabMeta)> {
        let mut next = Vec::with_capacity(roster.len());
        for (slot, tp) in roster.iter().enumerate() {
            let is_active = active == Some(tp.id);
            let meta = if should_refresh(is_active, tick, slot) {
                self.tab(tp, tick, now)
            } else {
                prev.iter()
                    .find(|(id, _)| *id == tp.id)
                    .map(|(_, m)| m.clone())
                    .unwrap_or_default()
            };
            self.note_activity_gap(tp.id, &meta);
            next.push((tp.id, meta));
        }
        next
    }

    /// R6.5: emit one `debug!` per `(TabId, AgentKind)` transition when a *detected* agent
    /// has no activity source (state `Unknown`), so "the badge never says ready" is
    /// diagnosable (D14). Routine failures stay at `debug!`, never `warn!` (R10.4).
    fn note_activity_gap(&mut self, id: TabId, meta: &TabMeta) {
        match &meta.agent {
            Some(agent) if agent.state == AgentState::Unknown => {
                if self.debug_seen.get(&id) != Some(&agent.kind) {
                    tracing::debug!(
                        tab = id.0,
                        agent = agent.kind.label(),
                        "detected agent has no activity source"
                    );
                    self.debug_seen.insert(id, agent.kind);
                }
            }
            _ => {
                // Agent gone or now known → forget, so a later gap re-logs once.
                self.debug_seen.remove(&id);
            }
        }
    }
}

/// The worker body. Runs until the command channel disconnects (the last
/// [`super::TabMetaHandle`] dropped), then returns so the thread exits cleanly (D14).
pub(crate) fn run(shared: Arc<TabMetaShared>, rx: Receiver<Cmd>, wake: impl Fn() + Send + 'static) {
    let mut roster: Vec<TabPid> = Vec::new();
    let mut active: Option<TabId> = None;
    let mut tick_index: u64 = 0;
    let mut published: Vec<(TabId, TabMeta)> = Vec::new();
    let mut deriver = Deriver::new();

    loop {
        // Empty roster → block with no timeout: zero wakeups, zero syscalls until a
        // roster arrives (D4). Otherwise the timeout is the tick.
        let next_cmd = if roster.is_empty() {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            rx.recv_timeout(TICK)
        };

        match next_cmd {
            Ok(Cmd::SetRoster(r)) => roster = r,
            Ok(Cmd::SetActive(a)) => {
                active = a;
                // Refresh the newly-active tab in band — before returning to
                // `recv_timeout` — so a switch never waits out a background interval
                // (R1.7). The global tick phase is NOT reset (that phase staggers the
                // background tabs, D4).
                if let Some(id) = a {
                    if let Some(tp) = roster.iter().find(|tp| tp.id == id).copied() {
                        let meta = deriver.tab(&tp, tick_index, SystemTime::now());
                        deriver.note_activity_gap(id, &meta);
                        let mut next = published.clone();
                        match next.iter_mut().find(|(tid, _)| *tid == id) {
                            Some(slot) => slot.1 = meta,
                            None => next.push((id, meta)),
                        }
                        publish_if_changed(&shared, &mut published, next, &wake);
                    }
                }
            }
            Ok(Cmd::SetEnabled { git, agent }) => {
                deriver.enabled_git = git;
                deriver.enabled_agent = agent;
                // Recompute now so a disabled source's field clears on this publish and
                // its work stops from here on (R1.8).
                let next = deriver.pass(&roster, active, tick_index, SystemTime::now(), &published);
                publish_if_changed(&shared, &mut published, next, &wake);
            }
            Err(RecvTimeoutError::Timeout) => {
                tick_index = tick_index.wrapping_add(1);
                let next = deriver.pass(&roster, active, tick_index, SystemTime::now(), &published);
                publish_if_changed(&shared, &mut published, next, &wake);
            }
            Err(RecvTimeoutError::Disconnected) => break, // handle dropped → shut down
        }
    }
}

/// Publish `next` only if it differs from the last publish (D4): otherwise take no lock,
/// bump no generation, and — crucially — do not `wake` (an unconditional wake would force
/// a full grid re-clone at 2 Hz forever, reintroducing the G1 idle-CPU debt). A free
/// function so the differ is unit-testable without the thread.
fn publish_if_changed(
    shared: &TabMetaShared,
    published: &mut Vec<(TabId, TabMeta)>,
    next: Vec<(TabId, TabMeta)>,
    wake: &(impl Fn() + Send + 'static),
) {
    if next == *published {
        return;
    }
    *published = next.clone();
    {
        let mut guard = match shared.tabs.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = next;
    }
    shared.generation.fetch_add(1, Ordering::Release);
    wake();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::sync::Mutex;

    fn shared() -> Arc<TabMetaShared> {
        Arc::new(TabMetaShared {
            generation: AtomicU64::new(0),
            tabs: Mutex::new(Vec::new()),
        })
    }

    // ---- the differ (R3.3, R3.4) ----

    #[test]
    fn publish_bumps_generation_only_on_change() {
        // D4 — change-only publication, testable without the thread.
        let s = shared();
        let mut published = Vec::new();
        let woke = std::sync::Arc::new(AtomicU64::new(0));
        let w = woke.clone();
        let wake = move || {
            w.fetch_add(1, Ordering::Relaxed);
        };

        let one = vec![(
            TabId(1),
            TabMeta {
                branch: Some("main".into()),
                agent: None,
                task: None,
            },
        )];
        publish_if_changed(&s, &mut published, one.clone(), &wake);
        assert_eq!(s.generation.load(Ordering::Acquire), 1);
        assert_eq!(woke.load(Ordering::Relaxed), 1);

        // Same content again → no publish, no wake, no generation bump (R3.3).
        publish_if_changed(&s, &mut published, one, &wake);
        assert_eq!(
            s.generation.load(Ordering::Acquire),
            1,
            "unchanged → no bump"
        );
        assert_eq!(woke.load(Ordering::Relaxed), 1, "unchanged → no wake");

        // A single changed field → one bump + one wake (R3.4).
        let two = vec![(
            TabId(1),
            TabMeta {
                branch: Some("dev".into()),
                agent: None,
                task: None,
            },
        )];
        publish_if_changed(&s, &mut published, two, &wake);
        assert_eq!(s.generation.load(Ordering::Acquire), 2);
        assert_eq!(woke.load(Ordering::Relaxed), 2);
    }

    // ---- the stagger (R3.1, R3.2) ----

    #[test]
    fn active_refreshes_every_tick() {
        // R3.1 — the active tab always refreshes.
        for t in 0..8 {
            assert!(should_refresh(true, t, 0));
        }
    }

    #[test]
    fn background_refreshes_every_fourth_tick_staggered() {
        // R3.1 — a background tab refreshes once per BG_INTERVAL ticks.
        assert!(should_refresh(false, 0, 0));
        assert!(!should_refresh(false, 1, 0));
        assert!(!should_refresh(false, 2, 0));
        assert!(!should_refresh(false, 3, 0));
        assert!(should_refresh(false, 4, 0));
        // R3.2 — different slots fire on different phases: at tick 0 only slots ≡ 0.
        let firing_at_0: Vec<usize> = (0..8).filter(|s| should_refresh(false, 0, *s)).collect();
        assert_eq!(firing_at_0, vec![0, 4]);
        // slot 1 fires at tick 1, not tick 0 — no burst.
        assert!(should_refresh(false, 1, 1));
        assert!(!should_refresh(false, 0, 1));
    }

    // ---- derivation cost (R0.2, R3.6) ----

    #[test]
    fn empty_roster_does_no_work() {
        // R0.2 — a pass over an empty roster touches no source.
        let _g = super::super::probe::serial();
        super::super::probe::reset_all();
        let mut d = Deriver::new();
        let next = d.pass(&[], None, 5, SystemTime::now(), &[]);
        assert!(next.is_empty());
        for c in [
            &super::super::probe::PROC_CWD,
            &super::super::probe::PROC_CHILDREN,
            &super::super::probe::PROC_EXE,
            &super::super::probe::PROC_ARGS,
            &super::super::probe::GIT_STAT,
            &super::super::probe::GIT_READ,
            &super::super::probe::LOG_RESOLVE,
            &super::super::probe::LOG_STAT,
            &super::super::probe::LOG_READ,
        ] {
            assert_eq!(c.get(), 0, "empty roster must not touch any source");
        }
    }

    #[test]
    fn steady_state_repo_derivation_does_no_git_read() {
        // R3.6 (cwd + git half) — a warm repo tab costs one cwd resolve and one
        // revalidation stat, zero HEAD reads. (The activity-path half — LOG_STAT = 1,
        // LOG_READ = 0 warm — is proved by activity::tests::unchanged_log_is_not_reread;
        // the ≤10 total assumes a depth-1 agent, which needs /run-term.)
        let _g = super::super::probe::serial();
        let mut d = Deriver::new();
        d.enabled_agent = false; // isolate the git + cwd budget
        let tp = TabPid {
            id: TabId(0),
            pid: std::process::id(),
        };
        let now = SystemTime::now();
        // The test runs inside this project's git repo, so cwd resolves to a repo.
        let cold = d.tab(&tp, 0, now);
        assert!(cold.branch.is_some(), "self cwd is a git repo");
        super::super::probe::reset_all();
        let _ = d.tab(&tp, 1, now);
        assert_eq!(super::super::probe::PROC_CWD.get(), 1, "cwd resolved once");
        assert_eq!(
            super::super::probe::GIT_READ.get(),
            0,
            "warm repo: no HEAD re-read"
        );
        assert!(
            super::super::probe::GIT_STAT.get() <= 1,
            "warm repo: at most one revalidation stat"
        );
    }

    // ---- toggles (R1.8) ----

    #[test]
    fn disabling_git_clears_branch_and_stops_git_work() {
        // R1.8 — turning a source off clears its field and stops its I/O.
        let _g = super::super::probe::serial();
        let mut d = Deriver::new();
        d.enabled_agent = false;
        let tp = TabPid {
            id: TabId(0),
            pid: std::process::id(),
        };
        let now = SystemTime::now();
        let with = d.tab(&tp, 0, now);
        assert!(with.branch.is_some(), "self cwd is a repo → branch present");
        d.enabled_git = false;
        super::super::probe::reset_all();
        let without = d.tab(&tp, 1, now);
        assert!(without.branch.is_none(), "disabled → branch cleared");
        assert_eq!(
            super::super::probe::GIT_STAT.get(),
            0,
            "disabled → no git stat"
        );
        assert_eq!(
            super::super::probe::GIT_READ.get(),
            0,
            "disabled → no git read"
        );
        assert_eq!(
            super::super::probe::PROC_CWD.get(),
            0,
            "both off → no cwd resolve"
        );
    }

    // ---- R6.5 transition guard ----

    #[test]
    fn activity_gap_is_tracked_once_per_transition() {
        // R6.5 — the once-per-(TabId, AgentKind) guard: an Unknown agent records the tab,
        // repeats do not re-record, and a cleared agent forgets it so a later gap re-logs.
        let mut d = Deriver::new();
        let id = TabId(0);
        let unknown = TabMeta {
            branch: None,
            agent: Some(Agent {
                kind: AgentKind::Aider,
                state: AgentState::Unknown,
            }),
            task: None,
        };
        d.note_activity_gap(id, &unknown);
        assert_eq!(d.debug_seen.get(&id), Some(&AgentKind::Aider));
        d.note_activity_gap(id, &unknown);
        assert_eq!(d.debug_seen.get(&id), Some(&AgentKind::Aider));
        // Agent gone → forgotten.
        d.note_activity_gap(id, &TabMeta::default());
        assert!(!d.debug_seen.contains_key(&id));
        // A known (Busy) agent is not a gap either.
        let busy = TabMeta {
            branch: None,
            agent: Some(Agent {
                kind: AgentKind::Claude,
                state: AgentState::Busy,
            }),
            task: None,
        };
        d.note_activity_gap(id, &busy);
        assert!(!d.debug_seen.contains_key(&id));
    }

    #[test]
    fn dead_pid_derives_to_blank_without_panic() {
        // R5.6 / R10.5 — a dead root pid yields blank metadata, no panic.
        let _g = super::super::probe::serial();
        let mut d = Deriver::new();
        let tp = TabPid {
            id: TabId(9),
            pid: 0,
        };
        let meta = d.tab(&tp, 0, SystemTime::now());
        assert_eq!(meta, TabMeta::default());
    }
}
