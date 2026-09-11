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
use super::tmux::Memo as TmuxMemo;
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
    /// The per-tick tmux client→pane map. The third memo of the same kind, so its lifetime is
    /// the worker's and it needs no plumbing through `pass()` or `run()`.
    tmux_memo: TmuxMemo,
    projects_root: Option<PathBuf>,
    /// `~/.claude/sessions` — where `claude` records which session each pid is running.
    /// Held beside `projects_root` and threaded into resolution rather than read from
    /// `$HOME` at the point of use, so every tier stays testable against a temp directory.
    sessions_root: Option<PathBuf>,
    debug_seen: HashMap<TabId, AgentKind>,
    enabled_git: bool,
    enabled_agent: bool,
    /// The three kernel reads a full derivation makes, injected as plain function pointers so
    /// [`Deriver::pass`] is drivable over a **fake process tree**.
    ///
    /// Not gold-plating, and not only a testing nicety: without this seam the identity chain
    /// `walk → pid → start_ms → ClaudeId` has no test at the layer that assembles it, and a
    /// mutation test proved that concretely — hardcoding `start_ms: None` here left the whole
    /// suite green while making tier 1 unreachable for every tab in production, i.e. silently
    /// restoring the very defect this feature exists to fix. `agent::walk` was already generic
    /// over its children/classify closures for the same reason; this extends that one level up.
    ///
    /// Function pointers, not generics, so `Deriver` stays a concrete type and `run()` is
    /// unchanged.
    agent_of: fn(u32) -> Option<super::agent::Detected>,
    cwd_of: fn(u32) -> Option<PathBuf>,
    start_ms_of: fn(u32) -> Option<u64>,
}

impl Deriver {
    fn new() -> Deriver {
        Deriver {
            git_memo: GitMemo::new(),
            act_memo: ActivityMemo::new(),
            tmux_memo: TmuxMemo::new(),
            projects_root: ai_monitor::claude_projects_dir(),
            sessions_root: ai_monitor::claude_sessions_dir(),
            debug_seen: HashMap::new(),
            enabled_git: true,
            enabled_agent: true,
            agent_of: super::agent::agent_of_pid,
            cwd_of: super::proc::cwd_of_pid,
            start_ms_of: super::proc::start_ms_of_pid,
        }
    }

    /// Derive one tab. cwd is resolved once and shared by the git and activity paths (D10)
    /// **and published on `TabMeta`** (tmux-pane-cwd R1.1). The git and agent sources are each
    /// skipped when their toggle is off (R1.8); the root and cwd never are (D13).
    fn tab(&mut self, tp: &TabPid, tick: u64, now: SystemTime, is_active: bool) -> TabMeta {
        // Re-root at the tmux pane when this tab runs a client. Absent tmux this is `tp.pid`,
        // so every downstream read is byte-identical to today.
        //
        // **Deliberately not behind the display toggles** (tmux-pane-cwd D13). Both the root
        // and the cwd used to be skipped when `show_git_branch` and `show_agent_status` were
        // off, on the reasoning that nothing downstream wanted them. That stopped being true
        // once `cwd` became a published field: `Session::cwd()` reads it for the ai-monitor's
        // log directory and the tab title, so gating it would let two *sidebar display
        // preferences* silently restore the wrong-directory bug this feature exists to fix —
        // with nothing on screen connecting cause to effect. The toggles still skip the
        // expensive work (`.git` I/O, the agent walk); they no longer skip a `proc_pidinfo`.
        let root = super::tmux::effective_root(&mut self.tmux_memo, tp.pid, tick);
        let (root, cwd) = match (self.cwd_of)(root) {
            Some(c) => (root, Some(c)),
            // The pane died between `effective_root`'s liveness check and here: retry from
            // the tab's own shell so the card degrades to today's metadata rather than
            // going blank. The rebound `root` also redirects `agent_of_pid` below, so
            // branch and agent never describe different processes.
            None if root != tp.pid => (tp.pid, (self.cwd_of)(tp.pid)),
            None => (tp.pid, None),
        };

        let branch = if self.enabled_git {
            cwd.as_deref()
                .and_then(|c| super::git::branch_of_cwd(&mut self.git_memo, c, tick))
        } else {
            None
        };

        // The agent walk — and the gate on it is `enabled_agent || is_active`, which is
        // narrower than it first looks and deliberately so.
        //
        // The ai-monitor panel needs a published agent pid to notice that a tab's `claude` has
        // been replaced; behind `enabled_agent` alone that value would be `None` **forever**
        // for anyone who turns `[sidebar].show_agent_status` off, so the panel would sit on a
        // dead session with nothing on screen connecting cause to effect — a display preference
        // silently disabling a correctness check.
        //
        // But lifting the walk *wholesale* is the opposite error, and the precedent usually
        // cited for it says so outright: tmux-pane-cwd D13 lifted `root`/`cwd` out of these
        // toggles and ends "The toggles still skip the **expensive work** (`.git` I/O, **the
        // agent walk**); they no longer skip a `proc_pidinfo`." This walk is not a
        // `proc_pidinfo` — it is a bounded BFS of up to `MAX_NODES = 64` processes, each a
        // `proc_pidpath` plus, for an interpreter, a `sysctl` argv read, per rostered tab per
        // 500 ms tick. Running that for every tab of a user who has the display off would tax
        // tabs with no agent at all, which `tabmeta-session-keying`'s Confirmed Goal 4 forbids.
        //
        // The panel only ever asks about the **active** tab, so that is the whole requirement:
        // with the toggle off the added cost is one walk per tick for one tab, not one per tab.
        // Everything else stays exactly as gated as today.
        let detected = (self.enabled_agent || is_active)
            .then(|| (self.agent_of)(root))
            .flatten();
        // D9 — scoped to the kind the panel tracks. Already true today (`ClaudeId` is built
        // under the same test), so this *pins* the property rather than introducing it: adding
        // `agy` support must not silently widen the published pid and turn the panel's compare
        // into a per-tick storm.
        let agent_pid = detected.and_then(|super::agent::Detected { kind, pid }| {
            (kind == AgentKind::Claude).then_some(pid)
        });

        let (agent, task) = match detected {
            // `agent`, `task` — and `activity`'s log reads — stay behind their existing toggle.
            // Turning the display off still stops the `.git` I/O and the log reads; only the
            // walk itself, and only for the active tab, is now unconditional.
            Some(super::agent::Detected { kind, .. }) if self.enabled_agent => {
                // The tab's own agent process, paired with its start time — the identity
                // that lets resolution name this tab's session instead of ranking the
                // project directory. One `proc_pidinfo` on a pid the walk just touched; it
                // is `Option` because a dead or freshly-recycled pid has none, and
                // resolution downgrades the tier rather than discarding the identity.
                //
                // **Behind the Claude gate on purpose.** Only Claude has a session registry
                // to validate against (D7), so reading a start time for a Codex or Gemini
                // tab buys nothing and would tax every agent tab on the machine to serve
                // the Claude ones — which Confirmed Goal 4 ("tabs with no Claude agent pay
                // nothing new") forbids. The gate used to sit one frame lower, inside
                // `activity`, which read the start time first and discarded it.
                //
                // Derived from `agent_pid` rather than re-testing the kind, so the published
                // pid and the identity resolution uses can never describe different kinds.
                let claude = agent_pid.map(|pid| super::ClaudeId {
                    pid,
                    start_ms: (self.start_ms_of)(pid),
                });
                let (state, task) = self.activity(kind, cwd.as_deref(), claude, tick, now);
                (Some(Agent { kind, state }), task)
            }
            _ => (None, None),
        };

        TabMeta {
            branch,
            agent,
            task,
            cwd,
            agent_pid,
        }
    }

    /// The activity state **and** the tab-identity label for a detected agent. Only Claude
    /// has a log source (D7); every other agent — and a Claude whose log cannot be resolved —
    /// is `Unknown` with no label (R6.5).
    ///
    /// Resolution runs once and both readings come from the same `Found`, replacing the old
    /// arrangement where the caller re-derived the label from a shared cwd map key and a
    /// comment promised the two would agree.
    fn activity(
        &mut self,
        kind: AgentKind,
        cwd: Option<&Path>,
        claude: Option<super::ClaudeId>,
        tick: u64,
        now: SystemTime,
    ) -> (AgentState, Option<String>) {
        if kind != AgentKind::Claude {
            return (AgentState::Unknown, None);
        }
        // Destructured once, rather than `self.projects_root.clone()` twice inside the
        // pattern: the clones existed only to end the `&self` borrow before `&mut
        // self.act_memo` below, and two `PathBuf` allocations per Claude tab per refresh is
        // not a cost a requirement titled "honestly counted" should be paying invisibly.
        let Deriver {
            projects_root: Some(projects),
            sessions_root: Some(sessions),
            act_memo,
            ..
        } = self
        else {
            return (AgentState::Unknown, None);
        };
        let Some(cwd) = cwd else {
            return (AgentState::Unknown, None);
        };
        match super::activity::resolve_log(act_memo, projects, sessions, cwd, claude, tick) {
            super::activity::Resolution::Found(found) => {
                let state = super::activity::state_of_found(act_memo, &found, tick, now);
                // A pure memo read: `state_of_found` just warmed the entry for this exact
                // log, so the label costs no further I/O (R4.2).
                let label = super::activity::label_of_log(act_memo, &found.log);
                (state, label)
            }
            // The session is named and has written nothing yet: an agent is here, but there
            // is no task to show. Never a neighbour's log.
            super::activity::Resolution::NamedButUnwritten | super::activity::Resolution::None => {
                (AgentState::Unknown, None)
            }
        }
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
                self.tab(tp, tick, now, is_active)
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
                        // `is_active = true` by construction: this arm derives the tab that
                        // just BECAME active. That is also what makes the panel's staleness
                        // check land in band on a tab switch rather than waiting a tick.
                        let meta = deriver.tab(&tp, tick_index, SystemTime::now(), true);
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
                cwd: None,
                agent_pid: None,
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
                cwd: None,
                agent_pid: None,
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
            &super::super::probe::PROC_START,
            &super::super::probe::SESSION_REG,
            &super::super::probe::GIT_STAT,
            &super::super::probe::GIT_READ,
            &super::super::probe::LOG_RESOLVE,
            &super::super::probe::LOG_STAT,
            &super::super::probe::LOG_READ,
        ] {
            assert_eq!(c.get(), 0, "empty roster must not touch any source");
        }
    }

    // ---- the identity path's cost is gated on actually having a Claude (R5.5, R5.6) ----

    #[test]
    fn a_tab_with_no_claude_agent_pays_nothing_for_identity() {
        // R5.5 — `start_ms_of_pid` and the registry read hang off a *found* Claude, so a tab
        // running a plain shell (like this test binary) must not pay either. Without this
        // gate the feature would tax every tab on the machine to serve the ones with agents.
        let _g = super::super::probe::serial();
        let mut d = Deriver::new();
        let tp = TabPid {
            id: TabId(0),
            pid: std::process::id(),
        };
        super::super::probe::reset_all();
        let meta = d.tab(&tp, 0, SystemTime::now(), true);
        assert!(meta.agent.is_none(), "the test binary runs no agent");
        assert_eq!(
            super::super::probe::PROC_START.get(),
            0,
            "no agent → no start-time read"
        );
        assert_eq!(
            super::super::probe::SESSION_REG.get(),
            0,
            "no agent → no registry read"
        );
        assert_eq!(meta.task, None, "no agent → no task label");
    }

    // ---- the wiring: resolve once, read state AND label from the same log ----
    //
    // These drive a full `pass()` over a **fake process tree**, via the `agent_of` / `cwd_of` /
    // `start_ms_of` function pointers on `Deriver`. An earlier revision drove `Deriver::activity`
    // directly, on the reasoning that a unit test cannot manufacture a live `claude` descendant.
    // That reasoning was wrong in an important way: it is true of `agent_of_pid`, but the walk
    // is not the only thing `pass()` adds. It also assembles the identity chain
    // `walk → pid → start_ms → ClaudeId`, and a mutation test showed that chain had NO coverage
    // — hardcoding `start_ms: None` in `tab()` left all 565 tests green while making tier 1
    // unreachable for every tab in production, silently restoring the reported defect.
    //
    // So the seam moved up one level, exactly as `agent::walk` already is generic over its
    // children/classify closures. `tab()` is now driven end to end.

    fn fixture_roots(tag: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("tabmeta-wk-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let projects = base.join("projects");
        let sessions = base.join("sessions");
        std::fs::create_dir_all(&projects).expect("mkdir projects");
        std::fs::create_dir_all(&sessions).expect("mkdir sessions");
        (projects, sessions)
    }

    fn write_log(projects: &Path, cwd: &Path, session_id: &str, body: &str) -> PathBuf {
        let dir = projects.join(ai_monitor::encode_cwd(cwd));
        std::fs::create_dir_all(&dir).expect("mkdir project");
        let p = dir.join(format!("{session_id}.jsonl"));
        std::fs::write(&p, body).expect("write log");
        p
    }

    fn write_registry(sessions: &Path, pid: u32, session_id: &str, cwd: &Path, started: u64) {
        let body = format!(
            r#"{{"pid":{pid},"sessionId":"{session_id}","cwd":"{}","startedAt":{started}}}"#,
            cwd.display()
        );
        std::fs::write(sessions.join(format!("{pid}.json")), body).expect("write registry");
    }

    fn prompt_line(text: &str) -> String {
        format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        ) + "\n"
    }

    const SID_A: &str = "02a3919c-58e8-446b-babc-d3dc616543c5";
    const SID_B: &str = "26fa908b-3225-428e-9ca2-de04e31c115d";

    fn deriver_at(projects: PathBuf, sessions: PathBuf) -> Deriver {
        let mut d = Deriver::new();
        d.projects_root = Some(projects);
        d.sessions_root = Some(sessions);
        // Isolate: the fake cwds are not git repos, so leaving git on would add a `.git`
        // probe per tab and nothing else. The cost is that no `pass()` test here covers
        // `tab()`'s git half — deliberate, since `steady_state_repo_derivation_does_no_git_read`
        // already owns that against a real repo.
        d.enabled_git = false;
        d
    }

    /// The fake tree the `pass()` tests run over. Every tab reports `THE_CWD`, every process
    /// started at 1_000, and the tab's shell at pid N runs a `claude` at **pid N + `AGENT_PID_OFFSET`**.
    ///
    /// **The offset is load-bearing, not decoration.** An earlier version used the identity
    /// (`|pid| Detected { pid }`) with the comment "deliberately not pid+1 … an identity
    /// transform is what a mutation would break". That reasoning was backwards, and a mutation
    /// proved it: with the identity fake, `tab_pid == root == walk_pid` are one number, so
    /// rebuilding `ClaudeId` from `root` instead of the walk's `pid` failed **zero** tests.
    /// A transform is what separates the three candidate sources; the identity collapses them.
    const THE_CWD: &str = "/proj/shared";
    const AGENT_PID_OFFSET: u32 = 1_000;

    fn with_fake_tree(d: &mut Deriver) {
        d.cwd_of = |_| Some(PathBuf::from(THE_CWD));
        d.agent_of = |pid| {
            Some(super::super::agent::Detected {
                kind: AgentKind::Claude,
                pid: pid + AGENT_PID_OFFSET,
            })
        };
        d.start_ms_of = |_| Some(1_000);
    }

    /// The opposite fixture to [`with_fake_tree`]: every kernel read answers the way it does
    /// for a pid that is **gone** — no cwd, no agent, no start time.
    ///
    /// Injected rather than left real, for the same reason [`tp`] picks unallocatable pids.
    /// Left real, a "dead pid" test is not about a dead pid at all: the reads run against the
    /// live machine, and the assertion silently becomes a claim about what is running on the
    /// computer. That is not hypothetical — this fixture exists because
    /// `dead_pid_derives_to_blank_without_panic` did exactly that and failed on any machine
    /// with a detached `claude` on it, reporting a real live `agent_pid` where it expected
    /// nothing.
    fn with_dead_tree(d: &mut Deriver) {
        d.cwd_of = |_| None;
        d.agent_of = |_| None;
        d.start_ms_of = |_| None;
    }

    /// The `claude` pid the fake tree reports for a tab whose shell is at `n` — i.e. the pid a
    /// registry fixture must be keyed on.
    fn agent_pid(n: u32) -> u32 {
        n + AGENT_PID_OFFSET
    }

    /// A tab whose id and shell pid are the same number, so a failure message names the tab
    /// and the process at once.
    ///
    /// The numbers callers pass are deliberately far above any live pid (`900_1xx`): `tab()`
    /// still calls the real `tmux::effective_root`, which is not injected, so a pid that
    /// happened to be alive and running a tmux client would re-root the derivation and make
    /// these fixtures machine-dependent. An unallocatable pid fails the presence gate
    /// deterministically, which is exactly why `root == tp.pid` holds here.
    fn tp(n: u32) -> TabPid {
        TabPid {
            id: TabId(n as u64),
            pid: n,
        }
    }

    /// Run a full **stripe** of real `pass()` calls and return the final `(TabId, TabMeta)`.
    ///
    /// One `pass()` is not enough: `should_refresh` staggers background tabs across
    /// `BG_INTERVAL` ticks (D4), so a single call derives only the tabs whose slot matches
    /// this tick's phase and reuses `prev` for the rest. Running the whole stripe — threading
    /// `prev` exactly as `run()` does — is what guarantees every tab in the roster has been
    /// derived at least once, and it exercises the `prev`-reuse path into the bargain.
    ///
    /// **Ticks replay across calls**: `pass_over(.., 0)` runs ticks 0–3 and `pass_over(.., 1)`
    /// then runs 1–4, so a test that calls it twice sees a non-monotonic tick sequence. Benign
    /// for every current consumer — the memos compare with `saturating_sub`, so a tick going
    /// backwards reads as "fresh" — but a future test that needs `RESOLVE_TTL_TICKS` or
    /// `MEMO_TTL_TICKS` to *expire* must space its base ticks by at least `BG_INTERVAL`, or the
    /// expiry it is testing will silently not happen.
    fn pass_over(d: &mut Deriver, roster: &[TabPid], base_tick: u64) -> Vec<(TabId, TabMeta)> {
        let mut out: Vec<(TabId, TabMeta)> = Vec::new();
        for k in 0..BG_INTERVAL {
            out = d.pass(roster, None, base_tick + k, SystemTime::now(), &out);
        }
        out
    }

    fn meta_of(out: &[(TabId, TabMeta)], n: u32) -> TabMeta {
        out.iter()
            .find(|(t, _)| *t == TabId(n as u64))
            .map(|(_, m)| m.clone())
            .expect("tab in pass output")
    }

    #[test]
    fn two_tabs_in_one_project_derive_independent_label_and_state() {
        // R0.1 + R0.2 — the reported bug, driven through a real two-tab `pass()`. Same cwd,
        // two sessions: two labels, two states. Before this feature both tabs read one ranked
        // log and were guaranteed to agree.
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("two-tabs");
        let cwd = Path::new(THE_CWD);
        // A: a substantive prompt, then the agent hands back → Waiting.
        write_log(
            &projects,
            cwd,
            SID_A,
            &(prompt_line("refactor the auth module")
                + r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn"}}"#
                + "\n"),
        );
        // B: a substantive prompt, then mid-tool-call → Busy.
        write_log(
            &projects,
            cwd,
            SID_B,
            &(prompt_line("fix the flaky test")
                + r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"tool_use"}}"#
                + "\n"),
        );
        write_registry(&sessions, agent_pid(900_101), SID_A, cwd, 1_000);
        write_registry(&sessions, agent_pid(900_202), SID_B, cwd, 1_000);

        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        let out = pass_over(&mut d, &[tp(900_101), tp(900_202)], 0);

        let a = meta_of(&out, 900_101);
        let b = meta_of(&out, 900_202);
        assert_eq!(a.task.as_deref(), Some("refactor the auth module"));
        assert_eq!(b.task.as_deref(), Some("fix the flaky test"));
        assert_ne!(a.task, b.task, "two sessions must not share one label");
        assert_eq!(a.agent.map(|x| x.state), Some(AgentState::Waiting));
        assert_eq!(b.agent.map(|x| x.state), Some(AgentState::Busy));
        assert_ne!(a.agent, b.agent, "two sessions must not share one badge");
    }

    #[test]
    fn a_fresh_session_does_not_inherit_its_neighbours_label() {
        // R0.3 — the exact reported symptom, through `pass()`. Session B is NEWER by mtime (so
        // the old ranking preferred it) but has no substantive prompt yet. The old cwd-keyed
        // carry-forward then supplied A's label and both cards showed A's prompt. Now B shows
        // nothing and its card falls back to the cwd title.
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("fresh-session");
        let cwd = Path::new(THE_CWD);
        write_log(
            &projects,
            cwd,
            SID_A,
            &prompt_line("How to start no permission mode claude code"),
        );
        std::thread::sleep(Duration::from_millis(20));
        write_log(
            &projects,
            cwd,
            SID_B,
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\"}}\n",
        );
        write_registry(&sessions, agent_pid(900_101), SID_A, cwd, 1_000);
        write_registry(&sessions, agent_pid(900_202), SID_B, cwd, 1_000);

        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        // Tab 101 is derived first in roster order — this is what used to poison the shared
        // cwd entry that tab 202 then read.
        let out = pass_over(&mut d, &[tp(900_101), tp(900_202)], 0);
        assert_eq!(
            meta_of(&out, 900_101).task.as_deref(),
            Some("How to start no permission mode claude code")
        );
        assert_eq!(
            meta_of(&out, 900_202).task,
            None,
            "the fresh session has no task of its own yet, and must not borrow one"
        );
    }

    #[test]
    fn a_named_but_unwritten_session_is_unknown_with_no_task() {
        // D10 claim 1 — the tier-2 render. The directory holds another session's log and not
        // one line of it reaches this tab.
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("unwritten");
        let cwd = Path::new(THE_CWD);
        write_log(&projects, cwd, SID_B, &prompt_line("someone else's task"));
        write_registry(&sessions, agent_pid(900_101), SID_A, cwd, 1_000); // SID_A.jsonl absent

        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        let m = meta_of(&pass_over(&mut d, &[tp(900_101)], 0), 900_101);
        assert_eq!(m.agent.map(|x| x.state), Some(AgentState::Unknown));
        assert_eq!(m.task, None, "never a neighbour's label");
    }

    #[test]
    fn the_first_write_promotes_a_tab_out_of_tier_two() {
        // R4.2 — tier 2 is not a trap: the moment the session writes, the next refresh picks
        // it up. No TTL to wait out, because identity is re-derived every refresh.
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("promote");
        let cwd = Path::new(THE_CWD);
        write_registry(&sessions, agent_pid(900_101), SID_A, cwd, 1_000);

        let mut d = deriver_at(projects.clone(), sessions);
        with_fake_tree(&mut d);
        let m = meta_of(&pass_over(&mut d, &[tp(900_101)], 0), 900_101);
        assert_eq!(m.agent.map(|x| x.state), Some(AgentState::Unknown));
        assert_eq!(m.task, None);

        write_log(&projects, cwd, SID_A, &prompt_line("the first real task"));
        let m = meta_of(&pass_over(&mut d, &[tp(900_101)], 1), 900_101);
        assert_eq!(
            m.task.as_deref(),
            Some("the first real task"),
            "picked up on the very next refresh"
        );
    }

    #[test]
    fn the_walks_pid_is_what_reaches_resolution() {
        // The identity chain `walk → pid → start_ms → ClaudeId`, pinned end to end.
        //
        // This exists because a mutation test found the chain had no coverage at all:
        // hardcoding `start_ms: None` in `tab()` left the entire suite green while making
        // tier 1 unreachable in production. Here the two tabs' registry records are keyed on
        // DIFFERENT pids, so a `tab()` that passed the wrong pid — or dropped the start time
        // and fell to ranking — resolves the wrong session and this fails on the label.
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("identity-chain");
        let cwd = Path::new(THE_CWD);
        write_log(&projects, cwd, SID_A, &prompt_line("the tab-101 task"));
        write_log(&projects, cwd, SID_B, &prompt_line("the tab-202 task"));
        // Only these exact pids have records. Any other pid → no tier 1.
        write_registry(&sessions, agent_pid(900_101), SID_A, cwd, 1_000);
        write_registry(&sessions, agent_pid(900_202), SID_B, cwd, 1_000);

        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        let out = pass_over(&mut d, &[tp(900_101), tp(900_202)], 0);
        assert_eq!(
            meta_of(&out, 900_101).task.as_deref(),
            Some("the tab-101 task")
        );
        assert_eq!(
            meta_of(&out, 900_202).task.as_deref(),
            Some("the tab-202 task")
        );

        // And the start time is load-bearing: drop it and tier 1 cannot validate, so both
        // tabs fall to mtime ranking and collapse onto ONE label — the original defect.
        d.start_ms_of = |_| None;
        let out = pass_over(&mut d, &[tp(900_101), tp(900_202)], 1);
        assert_eq!(
            meta_of(&out, 900_101).task,
            meta_of(&out, 900_202).task,
            "without a start time both tabs rank the directory and agree — which is exactly \
             the defect, and is why `start_ms_of_pid` must not be dropped from `tab()`"
        );
    }

    #[test]
    fn a_warm_claude_tab_pays_one_start_time_read_per_refresh() {
        // R5.4 — asserted as a FLOOR, never an equality: `PROC_START` is a process-global
        // counter and `tabsession` calls the same function from tests this one does not
        // coordinate with. The floor is what the requirement actually bounds — that the read
        // happens once per refresh rather than being memoised away.
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("start-floor");
        let cwd = Path::new(THE_CWD);
        write_log(&projects, cwd, SID_A, &prompt_line("a task"));
        write_registry(&sessions, agent_pid(900_101), SID_A, cwd, 1_000);

        let mut d = deriver_at(projects, sessions);
        // The fake tree supplies the walk and the cwd, but `start_ms_of` stays the REAL
        // reader — it is the counter under test.
        d.cwd_of = |_| Some(PathBuf::from(THE_CWD));
        d.agent_of = |pid| {
            Some(super::super::agent::Detected {
                kind: AgentKind::Claude,
                pid,
            })
        };
        let _ = pass_over(&mut d, &[tp(900_101)], 0);
        super::super::probe::reset_all();
        let _ = pass_over(&mut d, &[tp(900_101)], 1);
        assert!(
            super::super::probe::PROC_START.get() >= 1,
            "a Claude tab must re-read its process start time every refresh"
        );
    }

    #[test]
    fn a_non_claude_agent_tab_pays_no_start_time_read() {
        // Confirmed Goal 4 / D3 — "tabs with no Claude agent pay nothing new". Only Claude
        // has a session registry to validate against, so reading a start time for a Codex tab
        // buys nothing. The gate used to sit one frame lower, which taxed every agent tab on
        // the machine to serve the Claude ones.
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("non-claude");
        let mut d = deriver_at(projects, sessions);
        d.cwd_of = |_| Some(PathBuf::from(THE_CWD));
        d.agent_of = |pid| {
            Some(super::super::agent::Detected {
                kind: AgentKind::Codex,
                pid,
            })
        };
        super::super::probe::reset_all();
        let m = meta_of(&pass_over(&mut d, &[tp(900_101)], 0), 900_101);
        assert_eq!(m.agent.map(|x| x.kind), Some(AgentKind::Codex));
        assert_eq!(m.agent.map(|x| x.state), Some(AgentState::Unknown));
        assert_eq!(
            super::super::probe::PROC_START.get(),
            0,
            "a non-Claude agent must not trigger the start-time read"
        );
        assert_eq!(
            super::super::probe::SESSION_REG.get(),
            0,
            "…nor the registry read"
        );
    }

    #[test]
    fn re_deriving_identity_does_not_perturb_the_derived_value() {
        // R6.4 / D10 claim 2 — the property that keeps the G1 idle-CPU guarantee alive. It is
        // NOT "the machine is quiet": it is that nothing the per-refresh registry read
        // produces reaches `TabMeta`. Only the resolved log's derived (state, label) does, so
        // an unchanged session yields a byte-identical result however often we re-ask.
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("stable");
        let cwd = Path::new("/proj/stable");
        write_log(
            &projects,
            cwd,
            SID_A,
            &(prompt_line("a steady task")
                + r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn"}}"#
                + "\n"),
        );
        write_registry(&sessions, agent_pid(900_101), SID_A, cwd, 1_000);

        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        let first = meta_of(&pass_over(&mut d, &[tp(900_101)], 0), 900_101);
        // Pin the CONTENT before pinning its stability. Without this the test passes under a
        // uniformly-degraded derivation — a mutation forcing `activity()` to always return
        // `(Unknown, None)` satisfied "ten passes agree" perfectly. Stability is only worth
        // asserting about a value that is also correct. (`branch` is not covered: `deriver_at`
        // disables git, so this pins agent/task/cwd.)
        assert_eq!(first.task.as_deref(), Some("a steady task"));
        assert_eq!(first.agent.map(|a| a.state), Some(AgentState::Waiting));
        for tick in 1..10 {
            let again = meta_of(&pass_over(&mut d, &[tp(900_101)], tick), 900_101);
            assert_eq!(
                again, first,
                "pass {tick} differed — a re-derivation leaked into the published value"
            );
        }
        // ...and the differ therefore stays silent, which is the consequence that matters.
        let s = shared();
        let woke = Arc::new(AtomicU64::new(0));
        let w = woke.clone();
        let wake = move || {
            w.fetch_add(1, Ordering::Relaxed);
        };
        let mut published = Vec::new();
        for _ in 0..10 {
            let next = vec![(TabId(900_101), first.clone())];
            publish_if_changed(&s, &mut published, next, &wake);
        }
        assert_eq!(
            woke.load(Ordering::Relaxed),
            1,
            "ten identical passes must wake exactly once"
        );
    }

    #[test]
    fn a_missing_start_time_downgrades_the_tier_instead_of_discarding_the_pid() {
        // R1.5 at the worker seam — a `None` start time must not throw the identity away. It
        // only rules out the registry tier (there is nothing to validate the record against);
        // resolution continues, and with no argv id either it lands on ranking.
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("no-start");
        // THE_CWD, because `with_fake_tree` is what tells the deriver where this tab is.
        let cwd = Path::new(THE_CWD);
        write_log(&projects, cwd, SID_A, &prompt_line("the ranked task"));
        write_registry(&sessions, agent_pid(900_101), SID_A, cwd, 1_000);

        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        d.start_ms_of = |_| None; // the process start time cannot be read
        super::super::probe::reset_all();
        let task = meta_of(&pass_over(&mut d, &[tp(900_101)], 0), 900_101).task;
        assert_eq!(
            super::super::probe::SESSION_REG.get(),
            0,
            "no start time → the record cannot be validated, so it is not even read"
        );
        assert_eq!(
            task.as_deref(),
            Some("the ranked task"),
            "the tab still gets today's answer rather than going blank"
        );
    }

    #[test]
    fn tabmeta_still_has_exactly_its_five_published_fields() {
        // R6.5 — a compile-time assertion. If a field is added, removed or retyped this stops
        // building, which is the point: the renderer and the snapshot contract are downstream
        // and this feature promised not to move them.
        let TabMeta {
            branch: _,
            agent: _,
            cwd: _,
            task: _,
            agent_pid: _,
        } = TabMeta::default();
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
        let cold = d.tab(&tp, 0, now, false);
        assert!(cold.branch.is_some(), "self cwd is a git repo");
        super::super::probe::reset_all();
        let _ = d.tab(&tp, 1, now, false);
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

    // ---- the tmux hop's cost + toggle discipline (R3, R5, R6) ----

    #[test]
    fn constructing_a_deriver_touches_no_source() {
        // R6.1 — `empty_roster_does_no_work` calls reset_all() BEFORE Deriver::new(), so its
        // "no source touched" assertion silently depends on every memo constructor being
        // side-effect free. Pinned explicitly so a future memo cannot break it invisibly.
        let _g = super::super::probe::serial();
        super::super::probe::reset_all();
        let d = Deriver::new();
        for c in [
            &super::super::probe::PROC_CWD,
            &super::super::probe::PROC_CHILDREN,
            &super::super::probe::PROC_EXE,
            &super::super::probe::PROC_TTY,
            &super::super::probe::TTY_STAT,
            &super::super::probe::TMUX_LIST,
            &super::super::probe::PROC_START,
            &super::super::probe::SESSION_REG,
        ] {
            assert_eq!(c.get(), 0, "Deriver::new() must perform no I/O");
        }
        // R7.3 — both Claude roots are resolved once, here, rather than from `$HOME` at each
        // point of use. Resolving a path is string work, not I/O, so the assertion above and
        // this one are not in tension. (Both are `None` only when `$HOME` is unset.)
        assert_eq!(
            d.projects_root.is_some(),
            d.sessions_root.is_some(),
            "the two roots must appear and disappear together"
        );
        if let Some(sessions) = &d.sessions_root {
            assert!(sessions.ends_with(".claude/sessions"));
        }
    }

    #[test]
    fn a_cwd_only_change_publishes_and_wakes() {
        // tmux-pane-cwd R1.5 — load-bearing for D6, and a deliberate change to the
        // publication predicate. `cwd` joining `TabMeta`'s derived `PartialEq` means a bare
        // `cd` (same branch, same agent, same task) now publishes where it previously did
        // not. Without that wake the overlay would never learn the pane moved, because a tmux
        // window switch changes no other field.
        let s = TabMetaShared {
            generation: Default::default(),
            tabs: std::sync::Mutex::new(Vec::new()),
        };
        let woke = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let w = woke.clone();
        let wake = move || {
            w.fetch_add(1, Ordering::Relaxed);
        };
        let mut published = Vec::new();

        let at = |dir: &str| {
            vec![(
                TabId(1),
                TabMeta {
                    branch: Some("main".into()),
                    agent: None,
                    task: None,
                    cwd: Some(PathBuf::from(dir)),
                    agent_pid: None,
                },
            )]
        };

        publish_if_changed(&s, &mut published, at("/proj/a"), &wake);
        assert_eq!(woke.load(Ordering::Relaxed), 1, "first publish");

        // Only the cwd differs — everything else is byte-identical.
        publish_if_changed(&s, &mut published, at("/proj/b"), &wake);
        assert_eq!(
            woke.load(Ordering::Relaxed),
            2,
            "a cwd-only change must publish (D6 depends on this wake)"
        );
        assert_eq!(s.generation.load(Ordering::Acquire), 2);

        // ...and an unchanged cwd still must not, so the G1 idle-CPU property holds.
        publish_if_changed(&s, &mut published, at("/proj/b"), &wake);
        assert_eq!(
            woke.load(Ordering::Relaxed),
            2,
            "an unchanged pass must stay silent"
        );
    }

    // ---- tabsession-pane-cwd-gate D6/D9: the published agent pid (R6.1-R6.3, R6.8) ----

    #[test]
    fn the_worker_publishes_the_agent_pid() {
        // R6.1, producer side. The value is already computed — the walk returns it and
        // `ClaudeId` is built from it — so publishing costs no new subprocess on any path.
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("agent-pid");
        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        let m = meta_of(&pass_over(&mut d, &[tp(900_101)], 0), 900_101);
        assert_eq!(
            m.agent_pid,
            Some(agent_pid(900_101)),
            "the pid the walk found, not the tab's shell pid"
        );

        // ...and a change in it publishes, which is what the panel's wake depends on.
        // `publish_if_changed` compares `TabMeta` whole, so this holds by construction — the
        // assertion is that the field is genuinely IN that comparison.
        let s = TabMetaShared {
            generation: Default::default(),
            tabs: std::sync::Mutex::new(Vec::new()),
        };
        let woke = std::sync::Arc::new(AtomicU64::new(0));
        let w = woke.clone();
        let wake = move || {
            w.fetch_add(1, Ordering::Relaxed);
        };
        let mut published = Vec::new();
        let at = |pid: Option<u32>| {
            vec![(
                TabId(1),
                TabMeta {
                    agent_pid: pid,
                    ..Default::default()
                },
            )]
        };
        publish_if_changed(&s, &mut published, at(Some(10)), &wake);
        publish_if_changed(&s, &mut published, at(Some(11)), &wake);
        assert_eq!(
            woke.load(Ordering::Relaxed),
            2,
            "a pid-only change must publish, or the panel never learns the session moved"
        );
        publish_if_changed(&s, &mut published, at(Some(11)), &wake);
        assert_eq!(
            woke.load(Ordering::Relaxed),
            2,
            "an unchanged pid must stay silent — the G1 idle-CPU property"
        );
    }

    #[test]
    fn the_active_tabs_pid_is_published_with_the_display_toggle_off() {
        // R6.2 — the hazard D6(a) exists for. Behind `enabled_agent` alone this value would be
        // `None` forever for anyone who turns `[sidebar].show_agent_status` off, so the panel
        // would sit on a dead session with nothing on screen connecting cause to effect: a
        // display preference silently disabling a correctness check.
        //
        // And the lift is only as wide as it needs to be. The panel only ever asks about the
        // ACTIVE tab, so a background tab pays nothing new — which is what keeps
        // `tabmeta-session-keying`'s Confirmed Goal 4 ("tabs with no Claude agent pay nothing
        // new") true for the other nineteen tabs.
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("toggle-off-pid");
        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        d.enabled_agent = false;
        let now = SystemTime::now();

        let active = d.tab(&tp(900_101), 0, now, true);
        assert_eq!(
            active.agent_pid,
            Some(agent_pid(900_101)),
            "the active tab's pid is published whatever the display toggle says"
        );
        // Everything the toggle actually governs stays governed by it.
        assert_eq!(active.agent, None, "the badge stays gated");
        assert_eq!(
            active.task, None,
            "and so does the label, and its log reads"
        );
        assert_eq!(active.branch, None, "git is a separate toggle, still off");

        let background = d.tab(&tp(900_102), 0, now, false);
        assert_eq!(
            background.agent_pid, None,
            "a background tab with the toggle off pays no walk and publishes nothing"
        );
        assert_eq!(background.agent, None);
    }

    #[test]
    fn only_a_claude_kind_agent_is_published() {
        // R6.3 / D9 — a correctness constraint, not a preference. `AgentKind` has five
        // variants and the walk finds *an agent*; an `agy` pid could never equal the `claude`
        // pid the panel opened on, so an unscoped value would read as "a different pid" on
        // EVERY tick — a re-point storm, each iteration re-forking `/bin/ps`, respawning the
        // watcher and re-running the whole backfill.
        //
        // Already true today (`ClaudeId` is built under the same test), so this pins the
        // property rather than introducing one: the sibling `ai-monitor-antigravity` work must
        // not be able to widen it silently.
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("kind-scoped-pid");
        let mut d = deriver_at(projects, sessions);
        d.cwd_of = |_| Some(PathBuf::from(THE_CWD));
        let now = SystemTime::now();

        for kind in [
            AgentKind::Antigravity,
            AgentKind::Codex,
            AgentKind::Gemini,
            AgentKind::Aider,
        ] {
            d.agent_of = match kind {
                AgentKind::Antigravity => |pid| {
                    Some(super::super::agent::Detected {
                        kind: AgentKind::Antigravity,
                        pid,
                    })
                },
                AgentKind::Codex => |pid| {
                    Some(super::super::agent::Detected {
                        kind: AgentKind::Codex,
                        pid,
                    })
                },
                AgentKind::Gemini => |pid| {
                    Some(super::super::agent::Detected {
                        kind: AgentKind::Gemini,
                        pid,
                    })
                },
                _ => |pid| {
                    Some(super::super::agent::Detected {
                        kind: AgentKind::Aider,
                        pid,
                    })
                },
            };
            let m = d.tab(&tp(900_101), 0, now, true);
            assert_eq!(
                m.agent.map(|a| a.kind),
                Some(kind),
                "{kind:?} is still DETECTED — only the published pid is scoped"
            );
            assert_eq!(m.agent_pid, None, "{kind:?} must publish no pid");
        }

        // The one kind that does.
        with_fake_tree(&mut d);
        let m = d.tab(&tp(900_101), 0, now, true);
        assert_eq!(m.agent_pid, Some(agent_pid(900_101)));
    }

    #[test]
    fn both_toggles_off_still_resolves_cwd_but_does_no_git_or_agent_work() {
        // tmux-pane-cwd R1.4 — **deliberately inverts** this test's previous contract
        // (`both_toggles_off_runs_no_gate_and_no_cwd_read`), which asserted that both toggles
        // off meant zero cwd reads and no presence gate.
        //
        // Why it had to change: `cwd` is now a *published* field that `Session::cwd()` reads
        // for the ai-monitor's log directory and the tab title. Leaving it gated on
        // `show_git_branch` / `show_agent_status` meant two sidebar **display preferences**
        // could silently restore the wrong-directory bug this feature exists to fix (D13).
        // So the root + cwd are now unconditional; only the expensive sources stay gated.
        let _g = super::super::probe::serial();
        let mut d = Deriver::new();
        d.enabled_git = false;
        d.enabled_agent = false;
        let tp = TabPid {
            id: TabId(0),
            pid: std::process::id(),
        };
        super::super::probe::reset_all();
        let meta = d.tab(&tp, 0, SystemTime::now(), true);

        // The cwd is published even with every display source off.
        assert!(
            meta.cwd.is_some(),
            "both off must still publish a cwd (R1.4)"
        );
        assert!(
            super::super::probe::PROC_CWD.get() >= 1,
            "the cwd read is unconditional now"
        );
        // ...and nothing else is. The toggles still buy back all the *expensive* work.
        assert_eq!(meta.branch, None, "git off -> no branch");
        assert_eq!(meta.agent, None, "agent off -> no agent");
        assert_eq!(meta.task, None, "agent off -> no task");
        assert_eq!(super::super::probe::GIT_STAT.get(), 0, "git off -> no stat");
        assert_eq!(super::super::probe::GIT_READ.get(), 0, "git off -> no read");
        // Still no subprocess: the presence gate runs, but this tab has no tmux under it.
        assert_eq!(super::super::probe::TMUX_LIST.get(), 0, "no spawn");
    }

    #[test]
    fn a_non_tmux_tab_costs_exactly_the_gate_and_spawns_nothing() {
        // R0.6 / R5.2 — the honest delta every tab pays. `k` is measured independently, before
        // the counted window: deriving it from PROC_EXE itself would make the assertion a
        // tautology.
        let _g = super::super::probe::serial();
        let me = std::process::id();

        let mut d = Deriver::new();
        d.enabled_agent = false; // isolate the gate from agent_of_pid's own walk
        let tp = TabPid {
            id: TabId(0),
            pid: me,
        };
        super::super::probe::reset_all();
        let _ = d.tab(&tp, 0, SystemTime::now(), false);

        assert_eq!(
            super::super::probe::TMUX_LIST.get(),
            0,
            "no tmux -> no spawn"
        );
        assert_eq!(super::super::probe::PROC_CWD.get(), 1, "cwd resolved once");
        assert_eq!(
            super::super::probe::PROC_CHILDREN.get(),
            1,
            "one child listing"
        );
        // NOT `== 1 + k`: k is this process's LIVE child count, and other tests in this binary
        // spawn processes without holding `probe::serial()` (tabsession's `ProcTable` shells
        // out to /bin/ps), so k can change between any sample and this call. Assert the
        // k-independent floor here — the exact `1 + k` shape is proven deterministically
        // against a fake tree by `tmux::tests::the_gate_reads_the_root_then_each_child`.
        assert!(
            super::super::probe::PROC_EXE.get() >= 1,
            "the depth-0 root read always happens"
        );
    }

    #[test]
    fn the_single_toggle_states_both_still_resolve_a_root() {
        // R3.2 / R3.3 — the guard is a disjunction, so three of its four states run the gate.
        // git-only is not hypothetical: it is what both pre-existing cost tests set.
        let _g = super::super::probe::serial();
        let tp = TabPid {
            id: TabId(0),
            pid: std::process::id(),
        };

        // git-only: the gate runs, and agent_of_pid does not.
        let mut d = Deriver::new();
        d.enabled_agent = false;
        super::super::probe::reset_all();
        let _ = d.tab(&tp, 0, SystemTime::now(), false);
        assert_eq!(
            super::super::probe::PROC_CHILDREN.get(),
            1,
            "git-only: the gate's listing only"
        );

        // agent-only: the gate runs AND agent_of_pid walks, so the listing happens twice.
        let mut d = Deriver::new();
        d.enabled_git = false;
        super::super::probe::reset_all();
        let _ = d.tab(&tp, 0, SystemTime::now(), false);
        assert!(
            super::super::probe::PROC_CHILDREN.get() >= 2,
            "agent-only: the gate's listing PLUS agent_of_pid's own walk — at least two. \
             Not an equality: `walk` also lists each visited child, and a concurrently-running \
             test that spawns a process (tabsession's /bin/ps) transiently changes this \
             process's child count."
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
        let with = d.tab(&tp, 0, now, false);
        assert!(with.branch.is_some(), "self cwd is a repo → branch present");
        d.enabled_git = false;
        super::super::probe::reset_all();
        let without = d.tab(&tp, 1, now, false);
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
        // tmux-pane-cwd R1.4 — was `PROC_CWD == 0`. The cwd read is no longer behind the
        // display toggles (D13), because `Session::cwd()` publishes it to the overlay and the
        // tab title. What the toggles still buy back is the git I/O asserted just above.
        assert!(
            super::super::probe::PROC_CWD.get() >= 1,
            "cwd resolve is unconditional now"
        );
        assert!(without.cwd.is_some(), "cwd survives both toggles being off");
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
            cwd: None,
            agent_pid: None,
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
            cwd: None,
            agent_pid: None,
        };
        d.note_activity_gap(id, &busy);
        assert!(!d.debug_seen.contains_key(&id));
    }

    /// R5.6 / R10.5 — a dead root pid yields blank metadata, no panic. Every field
    /// must stay `None`: a derivation that invented one from a pid that no longer
    /// exists would attribute another process's branch, agent or cwd to this tab.
    ///
    /// This one deliberately keeps the **real** kernel readers — no
    /// [`with_dead_tree`] — and it is the only test that does. Stubbed, `cwd_of`,
    /// `agent_of` and `start_ms_of` all answer `None` by construction, and the
    /// assertion degrades into a property of the fixture: `Deriver::tab` returns
    /// `TabMeta::default()` when its injected readers return nothing. The thing R5.6
    /// is actually about — that `proc_pidinfo` and the descendant walk *do* answer
    /// nothing for a pid that is gone — would then be covered by no test at all, and
    /// a `cwd_of_pid` that started synthesising a value would keep both cases green.
    ///
    /// It is deterministic here and was not at `pid: 0`, and the difference is
    /// `agent_of_pid`: it walks the root's **descendants**, and a pid that was never
    /// allocated (`tp` picks 900_1xx, above macOS's `pid_max`) has none — whatever
    /// else the machine is running. Pid 0's descendants are every process on it,
    /// which is how the original version of this test came to report a live
    /// `agent_pid` from a detached `claude` that had nothing to do with any tab.
    #[test]
    fn dead_pid_derives_to_blank_without_panic() {
        let _g = super::super::probe::serial();
        let mut d = Deriver::new();
        let meta = d.tab(&tp(900_199), 0, SystemTime::now(), true);
        assert_eq!(meta, TabMeta::default());
    }

    /// The same for `pid: 0`, which **does** need the injected fixture.
    ///
    /// Kept as its own case rather than folded in, because 0 is not merely a dead
    /// pid: it is the kernel. Every process on the machine descends from it, so the
    /// real readers make this input a question about what happens to be running —
    /// and it is also the one input where `tmux::effective_root` (the read
    /// `with_dead_tree` cannot inject) is asked about a pid that genuinely exists.
    /// What survives injection is still worth pinning: `tab()` must not panic on the
    /// arithmetic, and must not manufacture a field out of a root of 0.
    #[test]
    fn pid_zero_derives_to_blank_without_panic() {
        let _g = super::super::probe::serial();
        let mut d = Deriver::new();
        with_dead_tree(&mut d);
        let tp = TabPid {
            id: TabId(9),
            pid: 0,
        };
        let meta = d.tab(&tp, 0, SystemTime::now(), true);
        assert_eq!(meta, TabMeta::default());
    }
}
