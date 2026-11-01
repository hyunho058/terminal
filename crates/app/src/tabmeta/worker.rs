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
use super::git::{Memo as GitMemo, RepoId, RepoMemo};
use super::tmux::{Memo as TmuxMemo, PaneMemo};
use super::{
    Agent, AgentKind, AgentState, Cmd, GraphPublish, PaneMeta, PanePass, TabMeta, TabMetaShared,
    TabPid,
};

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

/// Whether a tab's panes and repository identity are derived on this pass (session-flow-viewer
/// D9, R6.6, as `pane_pass`; session-flow-graph D10).
///
/// `wanted` is the flow graph's switch ([`super::TabMetaHandle::set_graph_wanted`]); while it is
/// off nothing graph-related runs, which is the graph-closed-costs-nothing constraint (G1).
/// `just_enabled` is the pass right after the switch turned on: it derives every tab whatever
/// its slot, so split tabs and groups appear within one tick of opening rather than after a
/// background stripe. Otherwise they ride the tab's ordinary schedule (`due`,
/// [`should_refresh`]).
pub fn graph_pass(wanted: bool, just_enabled: bool, due: bool) -> PanePass {
    if wanted && (just_enabled || due) {
        PanePass::Derive
    } else {
        PanePass::Skip
    }
}

/// The graph readiness and clearing a publish carries (session-flow-graph D10, R4.7).
///
/// `full_pass_done` is "this publish's pass derived every tab" — the off→on edge's
/// stagger-free pass. Readiness is set by exactly that, kept by every ordinary pass while the
/// graph stays wanted, and dropped with every published identity the moment it is not, so a
/// reopen starts from `ready: false` rather than from the previous open's groups.
pub fn publish_graph_state(wanted: bool, full_pass_done: bool, prev_ready: bool) -> GraphPublish {
    GraphPublish {
        ready: wanted && (full_pass_done || prev_ready),
        clear: !wanted,
    }
}

/// `published` with every tab's `panes`, `repo`, `graph_agent`, `graph_task`,
/// `graph_asks_user`, `graph_agent_cwd` and `graph_transcript` emptied —
/// the on→off edge of the graph's switch. Pure and syscall-free, so closing the graph
/// clears it without deriving anything.
fn strip_graph(published: &[(TabId, TabMeta)]) -> Vec<(TabId, TabMeta)> {
    published
        .iter()
        .map(|(id, m)| {
            (
                *id,
                TabMeta {
                    panes: Vec::new(),
                    repo: None,
                    graph_agent: None,
                    graph_task: None,
                    graph_asks_user: false,
                    graph_agent_cwd: None,
                    graph_transcript: None,
                    ..m.clone()
                },
            )
        })
        .collect()
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
    /// The per-tick `list-panes -a` (session-flow-viewer D9) — touched only while
    /// `graph_wanted`.
    pane_memo: PaneMemo,
    /// The cwd-keyed repository memo (session-flow-graph D10) — touched only while
    /// `graph_wanted`.
    repo_memo: RepoMemo,
    /// The flow graph's switch, and the one-pass flag its off→on edge raises ([`graph_pass`]).
    /// Both default off, so a worker nobody asked for the graph behaves exactly as before.
    graph_wanted: bool,
    graph_just_enabled: bool,
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
    /// The tmux re-root ([`super::tmux::rooted`]), injected on the same terms as the three
    /// reads above. Its presence gate reads the live process tree, so without this seam no
    /// test can put a tab behind a tmux pane, and the branches that only exist for a re-rooted
    /// tab — the dead-pane retry from the tab's own shell, the pane pass — had no coverage
    /// (cargo mutants: both survived).
    rooted_of: fn(&mut TmuxMemo, u32, u64) -> super::tmux::Rooted,
    /// Antigravity's per-pid resolution and tail (agent-card-status D11), and where they
    /// read from — injected on the same terms as the three reads above, so a test drives an
    /// Antigravity tab over a temp tree and never the real `~/.gemini`.
    antigravity_memo: super::antigravity::Memo,
    antigravity: super::antigravity::Sources,
}

impl Deriver {
    fn new() -> Deriver {
        Deriver {
            git_memo: GitMemo::new(),
            act_memo: ActivityMemo::new(),
            tmux_memo: TmuxMemo::new(),
            pane_memo: PaneMemo::new(),
            repo_memo: RepoMemo::new(),
            graph_wanted: false,
            graph_just_enabled: false,
            projects_root: ai_monitor::claude_projects_dir(),
            sessions_root: ai_monitor::claude_sessions_dir(),
            debug_seen: HashMap::new(),
            enabled_git: true,
            enabled_agent: true,
            agent_of: super::agent::agent_of_pid,
            cwd_of: super::proc::cwd_of_pid,
            start_ms_of: super::proc::start_ms_of_pid,
            rooted_of: super::tmux::rooted,
            antigravity_memo: super::antigravity::Memo::default(),
            antigravity: super::antigravity::Sources::real(),
        }
    }

    /// Derive one tab. cwd is resolved once and shared by the git and activity paths (D10)
    /// **and published on `TabMeta`** (tmux-pane-cwd R1.1). The git and agent sources are each
    /// skipped when their toggle is off (R1.8); the root and cwd never are (D13).
    ///
    /// This entry point is an explicit refresh — a tab switch, a test — so it counts as `due`
    /// for the graph decision: panes and identity are derived iff the graph is wanted.
    fn tab(&mut self, tp: &TabPid, tick: u64, now: SystemTime, is_active: bool) -> TabMeta {
        let graph = graph_pass(self.graph_wanted, false, true);
        self.tab_with(tp, tick, now, is_active, graph)
    }

    /// [`Deriver::tab`] with the graph decision given, as [`Deriver::pass`] computes it.
    fn tab_with(
        &mut self,
        tp: &TabPid,
        tick: u64,
        now: SystemTime,
        is_active: bool,
        graph: PanePass,
    ) -> TabMeta {
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
        //
        // `rooted` is `effective_root` keeping the client it re-rooted through, which the pane
        // pass below needs (session-flow-viewer D9); `root` is computed by the same body.
        let rooted = (self.rooted_of)(&mut self.tmux_memo, tp.pid, tick);
        let (root, mut attached) = (rooted.root, rooted.attached);
        let (root, cwd) = match (self.cwd_of)(root) {
            Some(c) => (root, Some(c)),
            // The pane died between `effective_root`'s liveness check and here: retry from
            // the tab's own shell so the card degrades to today's metadata rather than
            // going blank. The rebound `root` also redirects `agent_of_pid` below, so
            // branch and agent never describe different processes.
            None if root != tp.pid => {
                // No longer rooted at a pane, so there is no client window to enumerate.
                attached = None;
                (tp.pid, (self.cwd_of)(tp.pid))
            }
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
        //
        // The session graph is the third gate (session-flow-graph, user decision 2026-10-22):
        // it ignores the toggle, so on a pass that derives the graph every tab is walked and
        // classified as if the toggle were on — the same work a non-active pane already gets
        // ([`Deriver::pane`]). Its result reaches the graph through `graph_agent` only; what the
        // card reads (`agent`, `task`) stays behind the toggle. On a `Skip` pass — every pass
        // while the graph is closed — this is exactly the gate above.
        let graph_derive = graph == PanePass::Derive;
        let detected = (self.enabled_agent || is_active || graph_derive)
            .then(|| (self.agent_of)(root))
            .flatten();
        // D9 — scoped to the kind the panel tracks. Already true today (`ClaudeId` is built
        // under the same test), so this *pins* the property rather than introducing it: adding
        // `agy` support must not silently widen the published pid and turn the panel's compare
        // into a per-tick storm.
        let agent_pid = detected.and_then(|super::agent::Detected { kind, pid }| {
            (kind == AgentKind::Claude).then_some(pid)
        });

        let (classified, label, log, asks) = match detected {
            // `agent`, `task` — and `activity`'s log reads — stay behind their existing toggle
            // while the graph is closed. Turning the display off still stops the `.git` I/O and
            // the log reads; only the walk itself, and only for the active tab, is unconditional.
            Some(super::agent::Detected { kind, pid }) if self.enabled_agent || graph_derive => {
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
                let (state, task, log, asks) =
                    self.activity(kind, pid, cwd.as_deref(), claude, tick, now);
                (Some(Agent { kind, state }), task, log, asks)
            }
            _ => (None, None, None, false),
        };
        // The card's fields keep the toggle; the graph's copies do not. `log` is read by the
        // graph alone (its subagent source), so it needs no split.
        let agent = classified.filter(|_| self.enabled_agent);
        let task = label.clone().filter(|_| self.enabled_agent);
        let graph_agent = classified.filter(|_| graph_derive);
        let graph_task = label.filter(|_| graph_derive);
        // session-overview-ui D2 — graph-only like `graph_agent`, so a closed graph publishes
        // `false` whatever the log says.
        let graph_asks_user = asks && graph_derive;
        // session-overview-ui D24 — the Antigravity transcript the state was just classified
        // from, for the overview's digest. Graph-only like `graph_asks_user`.
        let graph_transcript = detected
            .filter(|_| graph_derive)
            .and_then(|super::agent::Detected { kind, pid }| self.graph_transcript(kind, pid));

        let (panes, repo, graph_agent_cwd) = match graph {
            PanePass::Derive => {
                // R14.5 — the walk's pid of **any** detected kind, not `agent_pid`: that one
                // stays Claude-only (D9 above), while every detected agent is a graph session
                // and so needs its repository. Still nothing while the graph is closed.
                // session-overview-ui D3 — the agent's own cwd is the one `repo` was just
                // resolved from, published rather than read a second time.
                let (agent_cwd, repo) = match detected {
                    Some(super::agent::Detected { pid, .. }) => {
                        self.session_repo(pid, cwd.as_deref(), tick)
                    }
                    None => (None, None),
                };
                let tab = PaneMeta {
                    graph_transcript: graph_transcript.clone(),
                    graph_agent_cwd: agent_cwd.clone(),
                    graph_asks_user,
                    pane_pid: 0,
                    active: true,
                    agent: graph_agent,
                    log: log.clone(),
                    cwd: cwd.clone(),
                    branch: branch.clone(),
                    repo: repo.clone(),
                };
                (
                    self.panes(attached.as_ref(), tick, now, &tab),
                    repo,
                    agent_cwd,
                )
            }
            PanePass::Skip => (Vec::new(), None, None),
        };

        TabMeta {
            graph_transcript,
            graph_agent_cwd,
            graph_asks_user,
            branch,
            agent,
            task,
            cwd,
            agent_pid,
            panes,
            log,
            repo,
            graph_agent,
            graph_task,
        }
    }

    /// The transcript of the `agy` at `pid` when `kind` is Antigravity (session-overview-ui
    /// D24) — a memo read of the path [`Deriver::activity`] just classified, so it costs no
    /// I/O. `None` for every other kind: only Claude (`log`) and Antigravity have a log the
    /// overview reads.
    fn graph_transcript(&self, kind: AgentKind, pid: u32) -> Option<PathBuf> {
        (kind == AgentKind::Antigravity)
            .then(|| super::antigravity::transcript(&self.antigravity_memo, pid))
            .flatten()
    }

    /// The repository of the agent session at `agent_pid`, of any kind (session-flow-graph
    /// D10, R14.5): resolved
    /// from that process's own cwd, falling back to `shell_cwd` when it cannot be read (the
    /// process exited between the walk and here).
    ///
    /// Returned beside the repository: the agent process's own cwd, as read for it — `None`
    /// when it could not be read, even though the repository then came from `shell_cwd`. It is
    /// published as `graph_agent_cwd` (session-overview-ui D3), which names the agent's
    /// directory and never the shell's (G5).
    fn session_repo(
        &mut self,
        agent_pid: u32,
        shell_cwd: Option<&Path>,
        tick: u64,
    ) -> (Option<PathBuf>, Option<RepoId>) {
        let agent_cwd = (self.cwd_of)(agent_pid);
        let repo = agent_cwd
            .as_deref()
            .or(shell_cwd)
            .and_then(|c| super::git::repo_of_cwd(&mut self.repo_memo, c, tick));
        (agent_cwd, repo)
    }

    /// The tab's panes (session-flow-viewer D9): the client window's panes from
    /// [`super::tmux::panes_for`], the active one carrying the tab's own derivation (`tab`'s
    /// fields under the row's pid) and every other one derived by [`Deriver::pane`]. Empty for a
    /// tab with no client, which spawns nothing.
    fn panes(
        &mut self,
        attached: Option<&super::tmux::Attached>,
        tick: u64,
        now: SystemTime,
        tab: &PaneMeta,
    ) -> Vec<PaneMeta> {
        super::tmux::panes_for(&mut self.pane_memo, attached, tick)
            .into_iter()
            .map(|row| {
                if row.active {
                    PaneMeta {
                        graph_transcript: tab.graph_transcript.clone(),
                        graph_agent_cwd: tab.graph_agent_cwd.clone(),
                        graph_asks_user: tab.graph_asks_user,
                        pane_pid: row.pid,
                        active: true,
                        agent: tab.agent,
                        log: tab.log.clone(),
                        cwd: tab.cwd.clone(),
                        branch: tab.branch.clone(),
                        repo: tab.repo.clone(),
                    }
                } else {
                    self.pane(row.pid, tick, now)
                }
            })
            .collect()
    }

    /// One non-active pane, through the tab's own chain: `agent_of_pid` on the pane's shell,
    /// then [`Deriver::activity`] (`resolve_log` → `state_of_found` for Claude) against that
    /// shell's cwd.
    ///
    /// **Not behind `enabled_agent`.** That toggle is the sidebar card's display preference; a
    /// non-active pane is never on the card, and this runs only while the session graph is open
    /// and has asked for it — the graph being open is the gate (G1). The cwd is read only once
    /// an agent is found, so a plain shell pane costs one walk.
    ///
    /// The pane's `cwd` and `branch` are its **shell**'s, for the card's folder and branch lines;
    /// its `repo` is its agent's (any kind, R14.5), by the tab's rule ([`Deriver::session_repo`]).
    fn pane(&mut self, pid: u32, tick: u64, now: SystemTime) -> PaneMeta {
        let Some(super::agent::Detected { kind, pid: apid }) = (self.agent_of)(pid) else {
            return PaneMeta {
                graph_transcript: None,
                graph_agent_cwd: None,
                graph_asks_user: false,
                pane_pid: pid,
                active: false,
                agent: None,
                log: None,
                cwd: None,
                branch: None,
                repo: None,
            };
        };
        let cwd = (self.cwd_of)(pid);
        let claude = (kind == AgentKind::Claude).then(|| super::ClaudeId {
            pid: apid,
            start_ms: (self.start_ms_of)(apid),
        });
        let (state, _task, log, asks) =
            self.activity(kind, apid, cwd.as_deref(), claude, tick, now);
        let branch = if self.enabled_git {
            cwd.as_deref()
                .and_then(|c| super::git::branch_of_cwd(&mut self.git_memo, c, tick))
        } else {
            None
        };
        let (agent_cwd, repo) = self.session_repo(apid, cwd.as_deref(), tick);
        PaneMeta {
            graph_transcript: self.graph_transcript(kind, apid),
            graph_agent_cwd: agent_cwd,
            graph_asks_user: asks,
            pane_pid: pid,
            active: false,
            agent: Some(Agent { kind, state }),
            log,
            cwd,
            branch,
            repo,
        }
    }

    /// The activity state **and** the tab-identity label for a detected agent. Claude and
    /// Antigravity have a log source (D7; agent-card-status D11); every other agent — and a
    /// session whose log cannot be resolved — is `Unknown` with no label (R6.5), which the
    /// card shows as no status (agent-card-status D3).
    ///
    /// Resolution runs once and both readings come from the same `Found`, replacing the old
    /// arrangement where the caller re-derived the label from a shared cwd map key and a
    /// comment promised the two would agree.
    fn activity(
        &mut self,
        kind: AgentKind,
        pid: u32,
        cwd: Option<&Path>,
        claude: Option<super::ClaudeId>,
        tick: u64,
        now: SystemTime,
    ) -> (AgentState, Option<String>, Option<PathBuf>, bool) {
        // The fourth value is whether the session's last meaningful message is
        // `AssistantAsksUser` (session-overview-ui D2): a memo read off the classification
        // the state just came from, paired with that state being `Waiting` so a tail left
        // behind an unreadable log never reads as asking.
        // The third value is the resolved `Found.log`, published as `TabMeta.log`
        // (session-flow-viewer D9) — the same file the state was classified from.
        // agent-card-status D11 — the walk's own `agy` pid, which names its transcript
        // through the lock it holds. Not the published `agent_pid`: that stays Claude-only
        // (D9 above), and widening it would re-point the panel on every tick. No label: the
        // card's line 1 keeps its other sources for an Antigravity tab, as before.
        if kind == AgentKind::Antigravity {
            let state = super::antigravity::state(
                &mut self.antigravity_memo,
                &self.antigravity,
                pid,
                tick,
                now,
            );
            let asks = state == AgentState::Waiting
                && super::antigravity::asks_user(&self.antigravity_memo, pid);
            return (state, None, None, asks);
        }
        if kind != AgentKind::Claude {
            return (AgentState::Unknown, None, None, false);
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
            return (AgentState::Unknown, None, None, false);
        };
        let Some(cwd) = cwd else {
            return (AgentState::Unknown, None, None, false);
        };
        match super::activity::resolve_log(act_memo, projects, sessions, cwd, claude, tick) {
            super::activity::Resolution::Found(found) => {
                let state = super::activity::state_of_found(act_memo, &found, tick, now);
                // A pure memo read: `state_of_found` just warmed the entry for this exact
                // log, so the label costs no further I/O (R4.2).
                let label = super::activity::label_of_log(act_memo, &found.log);
                let asks = state == AgentState::Waiting
                    && super::activity::last_of_log(act_memo, &found.log)
                        == Some(ai_monitor::activity::LastMessage::AssistantAsksUser);
                (state, label, Some(found.log), asks)
            }
            // The session is named and has written nothing yet: an agent is here, but there
            // is no task to show. Never a neighbour's log.
            super::activity::Resolution::NamedButUnwritten | super::activity::Resolution::None => {
                (AgentState::Unknown, None, None, false)
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
        // Consumed by this pass, whatever the roster holds: the off→on edge buys exactly one
        // stagger-free pass (session-flow-viewer D9).
        let just_enabled = std::mem::take(&mut self.graph_just_enabled);
        for (slot, tp) in roster.iter().enumerate() {
            let is_active = active == Some(tp.id);
            let due = should_refresh(is_active, tick, slot);
            let graph = graph_pass(self.graph_wanted, just_enabled, due);
            // A not-due tab is re-derived whole when its panes are owed: a pane list beside a
            // stale tab-level `agent` could name a different active pane than the card.
            let meta = if due || graph == PanePass::Derive {
                self.tab_with(tp, tick, now, is_active, graph)
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

    /// The graph switch's edge (session-flow-viewer D9/R6.6, session-flow-graph D10/R4.7): the
    /// next publication and readiness, or `None` when `wanted` did not move the switch.
    ///
    /// Off→on raises the one-pass flag and runs the stagger-free pass over **every** tab now —
    /// the full pass that sets readiness. On→off derives nothing: it only clears what was
    /// published, readiness included. Both go through [`publish_graph_state`].
    #[allow(clippy::too_many_arguments)] // `pass`'s inputs plus the readiness it updates
    fn graph_edge(
        &mut self,
        wanted: bool,
        roster: &[TabPid],
        active: Option<TabId>,
        tick: u64,
        now: SystemTime,
        published: &[(TabId, TabMeta)],
        prev_ready: bool,
    ) -> Option<(Vec<(TabId, TabMeta)>, bool)> {
        if std::mem::replace(&mut self.graph_wanted, wanted) == wanted {
            return None;
        }
        let next = if wanted {
            self.graph_just_enabled = true;
            self.pass(roster, active, tick, now, published)
        } else {
            published.to_vec()
        };
        let state = publish_graph_state(wanted, wanted, prev_ready);
        let next = if state.clear {
            strip_graph(&next)
        } else {
            next
        };
        Some((next, state.ready))
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
    // The ai-monitor panel's one-shot deadline (idle-timestamps D6/D8). At most one, replaced
    // on arm, cleared on fire or disarm. A local rather than shared state: it is read and
    // written only here, on this thread, so it needs neither a lock nor an atomic.
    let mut panel_deadline: Option<u64> = None;
    // The graph readiness last published (session-flow-graph D10). Run-local for the same
    // reason as the deadline; every publish carries it, so a flip alone still bumps the
    // generation.
    let mut graph_ready = false;
    // The open that readiness belongs to (D10): adopted from every `wanted: true` command and
    // published beside `graph_ready`, so a reopen can tell this open's readiness from the
    // last one's.
    let mut graph_epoch = 0;

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
                    if let Some(tp) = rostered(&roster, id) {
                        // `is_active = true` by construction: this arm derives the tab that
                        // just BECAME active. That is also what makes the panel's staleness
                        // check land in band on a tab switch rather than waiting a tick.
                        let meta = deriver.tab(&tp, tick_index, SystemTime::now(), true);
                        deriver.note_activity_gap(id, &meta);
                        let next = upsert_meta(&published, id, meta);
                        publish_if_changed(
                            &shared,
                            &mut published,
                            next,
                            (graph_ready, graph_epoch),
                            &wake,
                        );
                    }
                }
            }
            Ok(Cmd::SetEnabled { git, agent }) => {
                deriver.enabled_git = git;
                deriver.enabled_agent = agent;
                // Recompute now so a disabled source's field clears on this publish and
                // its work stops from here on (R1.8).
                let next = deriver.pass(&roster, active, tick_index, SystemTime::now(), &published);
                publish_if_changed(
                    &shared,
                    &mut published,
                    next,
                    (graph_ready, graph_epoch),
                    &wake,
                );
            }
            Ok(Cmd::SetPanelDeadline(at)) => panel_deadline = at,
            Ok(Cmd::SetGraphWanted { wanted, epoch }) => {
                let edge = deriver.graph_edge(
                    wanted,
                    &roster,
                    active,
                    tick_index,
                    SystemTime::now(),
                    &published,
                    graph_ready,
                );
                // Every `true` names the open asking, edge or not: a repeat with a new epoch
                // is an open whose readiness is already true, and it must still be told so.
                if wanted {
                    graph_epoch = epoch;
                }
                let next = match edge {
                    Some((next, ready)) => {
                        graph_ready = ready;
                        next
                    }
                    None => published.clone(),
                };
                publish_if_changed(
                    &shared,
                    &mut published,
                    next,
                    (graph_ready, graph_epoch),
                    &wake,
                );
            }
            Err(RecvTimeoutError::Timeout) => {
                tick_index = tick_index.wrapping_add(1);
                let next = deriver.pass(&roster, active, tick_index, SystemTime::now(), &published);
                publish_if_changed(
                    &shared,
                    &mut published,
                    next,
                    (graph_ready, graph_epoch),
                    &wake,
                );
            }
            Err(RecvTimeoutError::Disconnected) => break, // handle dropped → shut down
        }
        // The panel's deadline, checked once per loop iteration rather than inside the
        // `Timeout` arm (idle-timestamps D6/R3.1).
        //
        // Outside the match on purpose: a machine busy enough to keep delivering commands
        // never reaches the `Timeout` arm, and a deadline that only fires there would be
        // starved for exactly as long as the app kept talking. Out here every path through
        // the loop passes over it.
        //
        // **This is the second `wake()` site in this file, and it does not mean what the
        // first one means.** `publish_if_changed` wakes only when a `TabMeta` changed, and
        // several readers reason about cadence from that. This one fires with no generation
        // bump, no lock and nothing published — the app rebuilds the panel's model on the
        // wake and finds its own reason to redraw. One wake per armed deadline, and nothing
        // on the other ticks of the wait: an edge, not the heartbeat that was deferred.
        //
        // Conditional on the roster being non-empty, because an empty roster blocks on
        // `recv()` with no timeout and this line is then unreachable until a command arrives.
        // Benign in production — a panel is open on a tab, so there is a roster — but it is a
        // precondition of the whole mechanism rather than something a reader should infer.
        if deadline_due(&mut panel_deadline, epoch_ms()) {
            wake();
        }
    }
}

/// Which rostered tab does `id` name? `None` when the roster does not hold it.
///
/// A free function for the same reason [`publish_if_changed`] is one: it is pure decision logic
/// that was inline in [`run`]'s `SetActive` arm, where **no test could reach it**. `cargo
/// mutants` proved that rather than suggesting it — `tp.id == id` could be flipped to `!=` with
/// the whole suite green, because reaching the inline form meant a non-empty roster, which
/// meant `Deriver::tab` walking a real process tree. Out here the predicate is testable with no
/// clock, no processes and no thread.
///
/// The `!=` mutant is not harmless: it hands the arm the **wrong tab**, so a tab switch derives
/// some other tab's metadata and publishes it under the active tab's id.
fn rostered(roster: &[TabPid], id: TabId) -> Option<TabPid> {
    roster.iter().find(|tp| tp.id == id).copied()
}

/// `published` with `id`'s entry replaced by `meta`, or appended when it has none.
///
/// Extracted with [`rostered`] and for the same reason: `*tid == id` was an unreachable
/// mutation survivor inline. The `!=` form is the more dangerous of the two — it overwrites a
/// *different* tab's slot with this tab's metadata and then, because it found a match, never
/// appends the tab it was actually asked about. The tab being switched to would simply never
/// appear in the published set.
fn upsert_meta(published: &[(TabId, TabMeta)], id: TabId, meta: TabMeta) -> Vec<(TabId, TabMeta)> {
    let mut next = published.to_vec();
    match next.iter_mut().find(|(tid, _)| *tid == id) {
        Some(slot) => slot.1 = meta,
        None => next.push((id, meta)),
    }
    next
}

/// Has the panel's deadline come due? Takes it if so, so it can fire **exactly once**
/// (ai-monitor-idle-timestamps D8/R3.1).
///
/// A free function, and it takes the slot by `&mut` rather than returning advice, because the
/// clearing and the reporting must not be separable: a caller that fired on the answer and
/// forgot to clear would turn one bounded edge into a wake on every remaining tick — the
/// deferred heartbeat, arrived by accident. Clearing *before* the wake is the same reasoning
/// one step further in.
///
/// `>=` and not `>`: the instant handed in is already "the first instant at which the pill
/// reads IDLE" (`dashboard::idle_deadline_ms`), so the deadline is due *at* it. Pushing the
/// comparison out by one millisecond here would silently re-introduce the off-by-one that
/// function exists to kill.
fn deadline_due(slot: &mut Option<u64>, now_ms: u64) -> bool {
    match *slot {
        Some(at) if now_ms >= at => {
            *slot = None;
            true
        }
        _ => false,
    }
}

/// Wall clock as epoch millis — the same scale the panel's `last_event_ms` and
/// `dashboard::idle_deadline_ms` are on, which is why the deadline travels as a `u64` instant
/// rather than as a `Duration` the two sides would each have to anchor.
fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Publish `next` only if it differs from the last publish (D4): otherwise take no lock,
/// bump no generation, and — crucially — do not `wake` (an unconditional wake would force
/// a full grid re-clone at 2 Hz forever, reintroducing the G1 idle-CPU debt). A free
/// function so the differ is unit-testable without the thread.
///
/// **This is no longer the only `wake()` site in this file, and readers elsewhere depended on
/// it being so** (ai-monitor-idle-timestamps D16). `run` now also wakes once when the
/// ai-monitor panel's armed deadline comes due, with no generation bump and nothing published.
/// So a wake from this worker no longer implies "a `TabMeta` changed" — it means "either a
/// `TabMeta` changed, or the panel's one-shot deadline fired". That implication was
/// load-bearing: `overlay.rs` reasons about its own cadence from it, and reads the absence of
/// wakes on a quiet tab as "no tick at all". That reading is still true of *this* function and
/// no longer true of the worker as a whole.
///
/// What has not changed is the invariant this comment exists for. The deadline wake is a
/// single bounded edge — one per armed deadline, cleared by the firing — not a cadence, so the
/// 2 Hz re-clone it warns about is still impossible. The difference is pinned by
/// `tests::r3_1_a_passed_deadline_produces_exactly_one_wake`, because a source scan cannot see
/// the difference between one wake and one wake per tick.
///
/// `graph_ready` (session-flow-graph D10) is part of what "changed" means: a readiness flip
/// with every `TabMeta` unchanged — the graph opened over tabs with nothing to resolve — must
/// still reach the UI, or it would read `resolving…` until some unrelated field moved. The
/// worker is its only writer, so reading it back here is reading the last publish. `graph`
/// is `(graph_ready, graph_epoch)`: the epoch names the open that readiness belongs to, and
/// an epoch change alone publishes for the same reason a readiness flip does.
fn publish_if_changed(
    shared: &TabMetaShared,
    published: &mut Vec<(TabId, TabMeta)>,
    next: Vec<(TabId, TabMeta)>,
    graph: (bool, u64),
    wake: &(impl Fn() + Send + 'static),
) {
    let (graph_ready, graph_epoch) = graph;
    if next == *published
        && graph_ready == shared.graph_ready.load(Ordering::Acquire)
        && graph_epoch == shared.graph_epoch.load(Ordering::Acquire)
    {
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
    // Before the bump: a reader that `Acquire`s the new generation sees this value.
    shared.graph_ready.store(graph_ready, Ordering::Release);
    shared.graph_epoch.store(graph_epoch, Ordering::Release);
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
            graph_ready: std::sync::atomic::AtomicBool::new(false),
            graph_epoch: AtomicU64::new(0),
            graph_opens: AtomicU64::new(0),
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
                graph_agent_cwd: None,
                graph_transcript: None,
                graph_asks_user: false,
                branch: Some("main".into()),
                agent: None,
                task: None,
                cwd: None,
                agent_pid: None,
                panes: Vec::new(),
                log: None,
                repo: None,
                graph_agent: None,
                graph_task: None,
            },
        )];
        publish_if_changed(&s, &mut published, one.clone(), (false, 0), &wake);
        assert_eq!(s.generation.load(Ordering::Acquire), 1);
        assert_eq!(woke.load(Ordering::Relaxed), 1);

        // Same content again → no publish, no wake, no generation bump (R3.3).
        publish_if_changed(&s, &mut published, one, (false, 0), &wake);
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
                graph_agent_cwd: None,
                graph_transcript: None,
                graph_asks_user: false,
                branch: Some("dev".into()),
                agent: None,
                task: None,
                cwd: None,
                agent_pid: None,
                panes: Vec::new(),
                log: None,
                repo: None,
                graph_agent: None,
                graph_task: None,
            },
        )];
        publish_if_changed(&s, &mut published, two, (false, 0), &wake);
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
        // Hermetic for Antigravity too: no root, so an `agy` tab in these tests is `Unknown`
        // without a probe of the real process table or a read of the real `~/.gemini`. The
        // tests that exercise Antigravity install their own temp root and fakes.
        d.antigravity.root = None;
        d
    }

    /// The fake tree the `pass()` tests run over. Every tab reports `THE_CWD`, every process
    /// started at 1_000, and the tab's shell at pid N runs a `claude` at
    /// **pid N + `AGENT_PID_OFFSET`**.
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
            publish_if_changed(&s, &mut published, next, (false, 0), &wake);
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
    fn tabmeta_has_exactly_its_thirteen_published_fields() {
        // R6.5, now session-flow-graph R4.6 — a compile-time assertion. If a field is added,
        // removed or retyped this stops building, which is the point: the renderer and the
        // snapshot contract are downstream and each feature that moves them says so.
        // session-flow-viewer D9 moved it from five to seven (`panes`, `log`);
        // session-flow-graph D10 to eight (`repo`); the 2026-10-22 decision that the graph
        // ignores `[sidebar].show_agent_status` to ten (`graph_agent`, `graph_task`);
        // session-overview-ui D2/D3 to twelve (`graph_asks_user`, `graph_agent_cwd`); its
        // D24 to thirteen (`graph_transcript`).
        let TabMeta {
            branch: _,
            agent: _,
            cwd: _,
            task: _,
            agent_pid: _,
            panes: _,
            log: _,
            repo: _,
            graph_agent: _,
            graph_task: _,
            graph_asks_user: _,
            graph_agent_cwd: _,
            graph_transcript: _,
        } = TabMeta::default();
    }

    #[test]
    fn pane_meta_has_exactly_its_ten_fields() {
        // session-flow-graph R4.6 — `cwd`, `branch` and `repo` joined the four
        // session-flow-viewer published; session-overview-ui D2/D3 added `graph_asks_user` and
        // `graph_agent_cwd`, and D24 `graph_transcript`.
        let PaneMeta {
            pane_pid: _,
            active: _,
            agent: _,
            log: _,
            cwd: _,
            branch: _,
            repo: _,
            graph_asks_user: _,
            graph_agent_cwd: _,
            graph_transcript: _,
        } = blank_pane(0);
    }

    #[test]
    fn tab_meta_shared_carries_graph_ready() {
        // session-flow-graph R4.6 — the readiness rides the shared cache, beside the
        // generation it is published under.
        let TabMetaShared {
            generation: _,
            tabs: _,
            graph_ready,
            graph_epoch: _,
            graph_opens: _,
        } = &*shared();
        assert!(
            !graph_ready.load(Ordering::Relaxed),
            "a new cache is not ready"
        );
    }

    fn blank_pane(pid: u32) -> PaneMeta {
        PaneMeta {
            graph_agent_cwd: None,
            graph_transcript: None,
            graph_asks_user: false,
            pane_pid: pid,
            active: false,
            agent: None,
            log: None,
            cwd: None,
            branch: None,
            repo: None,
        }
    }

    fn some_repo() -> RepoId {
        RepoId {
            common: PathBuf::from("/r/.git"),
            main_root: PathBuf::from("/r"),
            linked: false,
        }
    }

    // ---- graph gating (session-flow-viewer R6.6, session-flow-graph R4.7) ----

    #[test]
    fn graph_pass_covers_all_eight_inputs() {
        for just in [false, true] {
            for due in [false, true] {
                assert_eq!(graph_pass(false, just, due), PanePass::Skip, "{just} {due}");
            }
        }
        assert_eq!(graph_pass(true, true, false), PanePass::Derive);
        assert_eq!(graph_pass(true, true, true), PanePass::Derive);
        assert_eq!(graph_pass(true, false, true), PanePass::Derive);
        assert_eq!(graph_pass(true, false, false), PanePass::Skip);
    }

    #[test]
    fn publish_graph_state_covers_all_eight_inputs() {
        // R4.7
        for full in [false, true] {
            for prev in [false, true] {
                assert_eq!(
                    publish_graph_state(false, full, prev),
                    GraphPublish {
                        ready: false,
                        clear: true
                    },
                    "{full} {prev}"
                );
                assert_eq!(
                    publish_graph_state(true, full, prev),
                    GraphPublish {
                        ready: full || prev,
                        clear: false
                    },
                    "{full} {prev}"
                );
            }
        }
    }

    #[test]
    fn strip_graph_empties_every_tabs_graph_fields_and_nothing_else() {
        let pane = PaneMeta {
            active: true,
            repo: Some(some_repo()),
            ..blank_pane(7)
        };
        let meta = TabMeta {
            branch: Some("main".into()),
            panes: vec![pane],
            log: Some(PathBuf::from("/l.jsonl")),
            repo: Some(some_repo()),
            graph_agent: Some(Agent {
                kind: AgentKind::Claude,
                state: AgentState::Busy,
            }),
            graph_task: Some("t".into()),
            graph_asks_user: true,
            graph_agent_cwd: Some(PathBuf::from("/agent")),
            graph_transcript: Some(PathBuf::from("/transcript.jsonl")),
            ..Default::default()
        };
        let got = strip_graph(&[(TabId(1), meta.clone()), (TabId(2), meta.clone())]);
        let want = TabMeta {
            panes: Vec::new(),
            repo: None,
            graph_agent: None,
            graph_task: None,
            graph_asks_user: false,
            graph_agent_cwd: None,
            graph_transcript: None,
            ..meta
        };
        assert_eq!(got, vec![(TabId(1), want.clone()), (TabId(2), want)]);
    }

    #[test]
    fn panes_are_never_enumerated_while_unwanted() {
        // The default is off, and off spawns no `list-panes` and publishes no pane — even on a
        // just-enabled flag left over (which only the wanted edge raises).
        let _g = super::super::probe::serial();
        super::super::probe::reset_all();
        let (projects, sessions) = fixture_roots("panes-unwanted");
        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        assert!(!d.graph_wanted);
        let out = pass_over(&mut d, &[tp(900_301), tp(900_302)], 0);
        assert!(out.iter().all(|(_, m)| m.panes.is_empty()));
        assert_eq!(super::super::probe::TMUX_PANES.get(), 0);
    }

    #[test]
    fn the_just_enabled_pass_derives_every_tab_and_is_consumed() {
        let _g = super::super::probe::serial();
        // R6.6 — with the flag raised, a background tab whose slot is not due this tick is
        // still re-derived (its cwd appears), and the flag is cleared by the pass.
        let (projects, sessions) = fixture_roots("panes-just-enabled");
        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        let roster = [tp(900_311), tp(900_312), tp(900_313), tp(900_314)];
        let prev: Vec<(TabId, TabMeta)> = Vec::new();
        // Without panes wanted, tick 0 refreshes only slot 0; the rest come from `prev`.
        let cold = d.pass(&roster, None, 0, SystemTime::now(), &prev);
        assert_eq!(cold.iter().filter(|(_, m)| m.cwd.is_some()).count(), 1);
        d.graph_wanted = true;
        d.graph_just_enabled = true;
        let all = d.pass(&roster, None, 0, SystemTime::now(), &prev);
        assert_eq!(all.iter().filter(|(_, m)| m.cwd.is_some()).count(), 4);
        assert!(!d.graph_just_enabled, "the flag buys exactly one pass");
        let after = d.pass(&roster, None, 0, SystemTime::now(), &prev);
        assert_eq!(after.iter().filter(|(_, m)| m.cwd.is_some()).count(), 1);
    }

    #[test]
    fn a_claude_tab_publishes_its_resolved_log() {
        let _g = super::super::probe::serial();
        // session-flow-viewer R6.5 — `TabMeta.log` is the `Found.log` the state came from.
        let (projects, sessions) = fixture_roots("tab-log");
        let cwd = Path::new(THE_CWD);
        let log = write_log(&projects, cwd, SID_A, &prompt_line("the task"));
        write_registry(&sessions, agent_pid(900_321), SID_A, cwd, 1_000);
        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        let m = d.tab(&tp(900_321), 0, SystemTime::now(), true);
        assert_eq!(m.log, Some(log));
        // A non-tmux tab has no panes even when they are wanted.
        d.graph_wanted = true;
        assert!(d
            .tab(&tp(900_321), 1, SystemTime::now(), true)
            .panes
            .is_empty());
    }

    /// R14.5's fixture: a main repository `<tmp>/r` and its linked worktree `<tmp>/r-w`, as
    /// `git worktree add` lays them out. A function rather than a value because the kernel
    /// reads are plain `fn` pointers and cannot capture it.
    fn r14_5_base() -> PathBuf {
        std::env::temp_dir().join(format!("tabmeta-wk-{}-r14-5-repo", std::process::id()))
    }

    fn r14_5_worktree() -> PathBuf {
        r14_5_base().join("r-w")
    }

    fn r14_5_layout() -> PathBuf {
        let base = r14_5_base();
        let _ = std::fs::remove_dir_all(&base);
        let r = base.join("r");
        let g = r.join(".git/worktrees/w");
        std::fs::create_dir_all(&g).expect("mkdir gitdir");
        std::fs::write(r.join(".git/HEAD"), "ref: refs/heads/main\n").expect("write HEAD");
        std::fs::write(g.join("HEAD"), "ref: refs/heads/w\n").expect("write wt HEAD");
        std::fs::write(g.join("commondir"), "../..\n").expect("write commondir");
        std::fs::create_dir_all(r14_5_worktree()).expect("mkdir worktree");
        std::fs::write(
            r14_5_worktree().join(".git"),
            format!("gitdir: {}\n", g.display()),
        )
        .expect("write .git file");
        r
    }

    #[test]
    fn r14_5_a_non_claude_agent_resolves_its_repository() {
        let _g = super::super::probe::serial();
        // session-flow-graph R14.5 — with the graph wanted, a tab (and a non-active pane)
        // whose detected agent is Codex, in a linked worktree of R, publishes R's identity
        // with `linked: true`; the same tab with no agent publishes `repo: None`.
        let r = r14_5_layout();
        let want = Some(RepoId {
            common: r.join(".git"),
            main_root: r.clone(),
            linked: true,
        });
        let (projects, sessions) = fixture_roots("r14-5");
        let mut d = deriver_at(projects, sessions);
        d.cwd_of = |_| Some(r14_5_worktree());
        d.agent_of = |pid| {
            Some(super::super::agent::Detected {
                kind: AgentKind::Codex,
                pid: pid + AGENT_PID_OFFSET,
            })
        };
        d.start_ms_of = |_| None;

        // Closed graph: nothing is resolved (G1), whatever the agent.
        let closed = d.tab(&tp(900_351), 0, SystemTime::now(), true);
        assert_eq!(
            closed.repo, None,
            "nothing resolves while the graph is closed"
        );

        d.graph_wanted = true;
        let m = d.tab(&tp(900_351), 1, SystemTime::now(), true);
        assert_eq!(m.agent.map(|a| a.kind), Some(AgentKind::Codex));
        assert_eq!(
            m.agent_pid, None,
            "the published pid stays Claude-only (D9)"
        );
        assert_eq!(m.repo, want, "a Codex tab resolves its repository");
        let p = d.pane(900_352, 1, SystemTime::now());
        assert_eq!(
            p.repo, want,
            "a Codex non-active pane resolves its repository"
        );

        d.agent_of = |_| None;
        let none = d.tab(&tp(900_351), 2, SystemTime::now(), true);
        assert_eq!(none.agent, None);
        assert_eq!(
            none.repo, None,
            "a tab with no agent publishes no repository"
        );
        let _ = std::fs::remove_dir_all(r14_5_base());
    }

    // ---- The graph ignores `[sidebar].show_agent_status` (user decision 2026-10-22) ----

    /// The toggle-off overview tests' one-tab model: `m` as the only tab, selected, at a
    /// fixed clock.
    fn graph_of_one(id: u32, m: &TabMeta) -> renderer::overview_model::OverviewModel {
        let row = renderer::sidebar_row::SidebarRow {
            tab_id: id as u64,
            title: "t".into(),
            ..Default::default()
        };
        let metas = HashMap::from([(TabId(id as u64), m.clone())]);
        crate::overview::build_overview(
            &[row],
            &metas,
            None,
            &HashMap::new(),
            &crate::overview::ExpandSet::new(),
            renderer::overview_model::Filter::All,
            0,
            1_793_404_800_000,
        )
        .0
    }

    fn session_nodes(
        g: &renderer::overview_model::OverviewModel,
    ) -> Vec<&renderer::overview_model::SessionRowData> {
        g.rows.iter().collect()
    }

    /// The bucket a session with `agent` and no dialog reads (D2), from its log-only status.
    fn bucket_for(agent: &crate::tabmeta::Agent) -> renderer::overview_model::Bucket {
        crate::overview::bucket_of(
            crate::sidebar_model::resolve_status(Some(agent), false, false, None),
            false,
            false,
        )
    }

    #[test]
    fn toggle_off_an_unsplit_claude_tab_is_a_graph_session_with_its_real_status() {
        // (a) With the graph wanted and the toggle off, an unsplit Claude tab — the active one
        // and a background one alike — is a graph session whose card is exactly the card the
        // toggle-on derivation gives: same status, same label, same subagent log.
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("graph-toggle-off-claude");
        let cwd = Path::new(THE_CWD);
        let log = write_log(&projects, cwd, SID_A, &prompt_line("the graph task"));
        write_registry(&sessions, agent_pid(900_361), SID_A, cwd, 1_000);
        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        d.graph_wanted = true;
        let now = SystemTime::now();

        let on = d.tab(&tp(900_361), 0, now, true);
        let real = on.agent.expect("toggle on: the Claude agent is classified");
        assert_eq!(real.kind, AgentKind::Claude);
        assert_ne!(
            real.state,
            AgentState::Unknown,
            "a resolved log has a state"
        );

        d.enabled_agent = false;
        for (tick, is_active) in [(1, true), (2, false)] {
            let off = d.tab(&tp(900_361), tick, now, is_active);
            assert_eq!(off.agent, None, "the card's field keeps the toggle");
            assert_eq!(off.graph_agent, Some(real), "active={is_active}");
            assert_eq!(off.graph_task.as_deref(), Some("the graph task"));
            assert_eq!(
                off.log.as_ref(),
                Some(&log),
                "the subagent source is published"
            );
            let g = graph_of_one(900_361, &off);
            let nodes = session_nodes(&g);
            assert_eq!(nodes.len(), 1, "one session, active={is_active}");
            assert_eq!(
                nodes[0].bucket,
                bucket_for(&real),
                "its real status, active={is_active}"
            );
            assert_eq!(
                g,
                graph_of_one(900_361, &on),
                "identical to the toggle-on graph"
            );
        }
    }

    #[test]
    fn toggle_off_an_unsplit_codex_tab_is_a_graph_session_in_its_repository() {
        // (a) The repository half (R14.5) for a non-Claude kind: with the toggle off the
        // session is grouped under its project, not left as a repo with no session.
        let _g = super::super::probe::serial();
        let r = r14_5_layout();
        let (projects, sessions) = fixture_roots("graph-toggle-off-codex");
        let mut d = deriver_at(projects, sessions);
        d.cwd_of = |_| Some(r14_5_worktree());
        d.agent_of = |pid| {
            Some(super::super::agent::Detected {
                kind: AgentKind::Codex,
                pid: pid + AGENT_PID_OFFSET,
            })
        };
        d.start_ms_of = |_| None;
        d.enabled_agent = false;
        d.graph_wanted = true;
        let m = d.tab(&tp(900_362), 0, SystemTime::now(), true);
        let codex = m.graph_agent.expect("the graph sees the Codex agent");
        assert_eq!(codex.kind, AgentKind::Codex);
        assert_eq!(m.agent, None);
        assert_eq!(m.repo.as_ref().map(|r| &r.main_root), Some(&r));
        let g = graph_of_one(900_362, &m);
        let nodes = session_nodes(&g);
        assert_eq!(nodes.len(), 1, "the Codex tab is a session");
        assert_eq!(nodes[0].bucket, bucket_for(&codex));
        assert_eq!(
            (nodes[0].agent_label.as_str(), nodes[0].agent),
            ("Codex", renderer::overview_model::AgentTag::Codex)
        );
        // Grouped under its repository: the row is titled by the main worktree's name.
        let main_name = r.file_name().map(|n| n.to_string_lossy().into_owned());
        assert_eq!(Some(nodes[0].project.clone()), main_name);
        let _ = std::fs::remove_dir_all(r14_5_base());
    }

    #[test]
    fn toggle_off_the_sidebar_card_is_unchanged_with_the_graph_open() {
        // (b) The graph's toggle-free fields never reach the card: with the toggle off, the
        // card read from a graph-pass meta equals the card read from a graph-closed one, and
        // shows no agent — the plain-shell status, the generic skin, no task label.
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("graph-toggle-off-card");
        let cwd = Path::new(THE_CWD);
        write_log(&projects, cwd, SID_A, &prompt_line("hidden from the card"));
        write_registry(&sessions, agent_pid(900_363), SID_A, cwd, 1_000);
        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        d.enabled_agent = false;
        let now = SystemTime::now();
        let closed = d.tab(&tp(900_363), 0, now, true);
        d.graph_wanted = true;
        let open = d.tab(&tp(900_363), 1, now, true);
        assert!(open.graph_agent.is_some(), "the graph pass did classify it");

        let row = renderer::sidebar_row::SidebarRow {
            tab_id: 900_363,
            title: "t".into(),
            ..Default::default()
        };
        let card = crate::sidebar_model::row_meta(&row, Some(&open), None);
        assert_eq!(
            card,
            crate::sidebar_model::row_meta(&row, Some(&closed), None),
            "the card does not move when the graph opens"
        );
        assert_eq!(card.status, renderer::sidebar_card::CardStatus::Shell);
        assert_eq!(card.skin, crate::sidebar_model::skin_for(None));
        assert_ne!(card.line1, "hidden from the card");
        assert_eq!((open.agent, open.task), (None, None));
    }

    static TOGGLE_OFF_WALKS: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn toggle_off_with_the_graph_closed_does_no_extra_agent_work() {
        // (c) G1: graph closed + toggle off is exactly the old gate — the active tab is walked
        // (the ai-monitor's pid) and nothing else; no tab is classified, so no log is resolved
        // and no graph field is published, though every tab here runs a Claude with a log.
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("graph-closed-toggle-off");
        let cwd = Path::new(THE_CWD);
        write_log(&projects, cwd, SID_A, &prompt_line("never read"));
        let roster = [tp(900_371), tp(900_372), tp(900_373), tp(900_374)];
        for t in &roster {
            write_registry(&sessions, agent_pid(t.pid), SID_A, cwd, 1_000);
        }
        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        d.agent_of = |pid| {
            TOGGLE_OFF_WALKS.fetch_add(1, Ordering::Relaxed);
            Some(super::super::agent::Detected {
                kind: AgentKind::Claude,
                pid: pid + AGENT_PID_OFFSET,
            })
        };
        d.enabled_agent = false;
        assert!(!d.graph_wanted);
        TOGGLE_OFF_WALKS.store(0, Ordering::Relaxed);
        let mut out: Vec<(TabId, TabMeta)> = Vec::new();
        for tick in 0..BG_INTERVAL {
            out = d.pass(&roster, Some(roster[0].id), tick, SystemTime::now(), &out);
        }
        assert_eq!(
            TOGGLE_OFF_WALKS.load(Ordering::Relaxed),
            BG_INTERVAL,
            "one walk per tick, for the active tab only"
        );
        for (id, m) in &out {
            assert_eq!(
                (m.agent, m.graph_agent, &m.graph_task, &m.log),
                (None, None, &None, &None),
                "{id:?} is not classified"
            );
        }
    }

    #[test]
    fn a_non_active_pane_runs_the_tabs_chain_on_its_own_shell() {
        let _g = super::super::probe::serial();
        // session-flow-viewer D9 — `Deriver::pane` walks the pane's shell, resolves that
        // agent's log and classifies it, exactly as `tab` does for the tab.
        let (projects, sessions) = fixture_roots("pane-chain");
        let cwd = Path::new(THE_CWD);
        let log = write_log(&projects, cwd, SID_B, &prompt_line("pane task"));
        write_registry(&sessions, agent_pid(900_331), SID_B, cwd, 1_000);
        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        let p = d.pane(900_331, 0, SystemTime::now());
        assert_eq!(p.pane_pid, 900_331);
        assert!(!p.active);
        assert_eq!(p.agent.map(|a| a.kind), Some(AgentKind::Claude));
        assert_ne!(p.agent.map(|a| a.state), Some(AgentState::Unknown));
        assert_eq!(p.log, Some(log));
        // A pane with no agent publishes neither.
        with_dead_tree(&mut d);
        let empty = d.pane(900_332, 0, SystemTime::now());
        assert_eq!((empty.agent, empty.log), (None, None));
    }

    #[test]
    fn non_active_claude_panes_resolve_their_own_sessions_by_identity() {
        // `pane` builds the `ClaudeId` exactly when the agent IS Claude. Two panes in one
        // project, each registry record keyed on its own `claude` pid: identity names each
        // pane's own log. Without the id (the `==` flipped) both fall to mtime ranking and
        // collapse onto one log — the two-sessions-one-label defect, per pane.
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("pane-identity");
        let cwd = Path::new(THE_CWD);
        let log_a = write_log(&projects, cwd, SID_A, &prompt_line("pane 441 task"));
        std::thread::sleep(Duration::from_millis(20));
        let log_b = write_log(&projects, cwd, SID_B, &prompt_line("pane 442 task"));
        write_registry(&sessions, agent_pid(900_441), SID_A, cwd, 1_000);
        write_registry(&sessions, agent_pid(900_442), SID_B, cwd, 1_000);
        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        let now = SystemTime::now();
        assert_eq!(d.pane(900_441, 0, now).log, Some(log_a));
        assert_eq!(d.pane(900_442, 0, now).log, Some(log_b));
    }

    /// The tab shell the re-root tests use, and the tmux pane `rooted_of` sends it to.
    const ROOTED_TAB: u32 = 900_561;
    const ROOTED_PANE: u32 = 900_562;
    const OTHER_PANE: u32 = 900_563;

    fn rooted_at_pane(_: &mut TmuxMemo, _: u32, _: u64) -> super::super::tmux::Rooted {
        super::super::tmux::Rooted {
            root: ROOTED_PANE,
            attached: Some(super::super::tmux::Attached {
                client: super::super::tmux::ClientRow {
                    tty: PathBuf::from("/dev/ttys099"),
                    pane_pid: ROOTED_PANE,
                    window: "work:1".to_string(),
                },
                exe: PathBuf::from("/definitely/not/a/tmux/binary"),
            }),
        }
    }

    #[test]
    fn a_dead_pane_falls_back_to_the_tabs_own_shell() {
        // tmux-pane-cwd: the pane died between the re-root and the cwd read. The card must
        // degrade to the tab shell's own cwd and agent, not go blank — and no longer claim the
        // client it re-rooted through.
        let (projects, sessions) = fixture_roots("dead-pane");
        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        d.rooted_of = rooted_at_pane;
        d.cwd_of = |pid| (pid != ROOTED_PANE).then(|| PathBuf::from(THE_CWD));
        d.graph_wanted = true;
        d.pane_memo = PaneMemo::seeded(
            0,
            super::super::tmux::parse_panes(&format!(
                "work:1 1 {ROOTED_PANE}\nwork:1 0 {OTHER_PANE}\n"
            )),
        );
        let m = d.tab(&tp(ROOTED_TAB), 0, SystemTime::now(), true);
        assert_eq!(m.cwd, Some(PathBuf::from(THE_CWD)), "the shell's cwd");
        assert_eq!(
            m.agent_pid,
            Some(agent_pid(ROOTED_TAB)),
            "the shell's agent"
        );
        assert!(m.panes.is_empty(), "no longer attached: no pane pass");
    }

    #[test]
    fn a_re_rooted_tab_publishes_its_windows_panes() {
        // session-flow-viewer D9: while the graph is wanted, a tab behind a tmux client
        // publishes every pane of the client's window — the active one carrying the tab's own
        // derivation, the rest derived on their own shells.
        let (projects, sessions) = fixture_roots("pane-pass");
        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        d.rooted_of = rooted_at_pane;
        d.graph_wanted = true;
        d.pane_memo = PaneMemo::seeded(
            0,
            super::super::tmux::parse_panes(&format!(
                "work:1 1 {ROOTED_PANE}\nwork:1 0 {OTHER_PANE}\nother:0 1 1\n"
            )),
        );
        let m = d.tab(&tp(ROOTED_TAB), 0, SystemTime::now(), true);
        let got: Vec<(u32, bool)> = m.panes.iter().map(|p| (p.pane_pid, p.active)).collect();
        assert_eq!(got, [(ROOTED_PANE, true), (OTHER_PANE, false)]);
        assert_eq!(
            m.panes[0].cwd, m.cwd,
            "the active pane carries the tab's derivation"
        );
        assert_eq!(
            m.panes[1].agent.map(|a| a.kind),
            Some(AgentKind::Claude),
            "the other pane is derived on its own shell"
        );
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
            graph_ready: Default::default(),
            graph_epoch: Default::default(),
            graph_opens: Default::default(),
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
                    graph_agent_cwd: None,
                    graph_transcript: None,
                    graph_asks_user: false,
                    branch: Some("main".into()),
                    agent: None,
                    task: None,
                    cwd: Some(PathBuf::from(dir)),
                    agent_pid: None,
                    panes: Vec::new(),
                    log: None,
                    repo: None,
                    graph_agent: None,
                    graph_task: None,
                },
            )]
        };

        publish_if_changed(&s, &mut published, at("/proj/a"), (false, 0), &wake);
        assert_eq!(woke.load(Ordering::Relaxed), 1, "first publish");

        // Only the cwd differs — everything else is byte-identical.
        publish_if_changed(&s, &mut published, at("/proj/b"), (false, 0), &wake);
        assert_eq!(
            woke.load(Ordering::Relaxed),
            2,
            "a cwd-only change must publish (D6 depends on this wake)"
        );
        assert_eq!(s.generation.load(Ordering::Acquire), 2);

        // ...and an unchanged cwd still must not, so the G1 idle-CPU property holds.
        publish_if_changed(&s, &mut published, at("/proj/b"), (false, 0), &wake);
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
            graph_ready: Default::default(),
            graph_epoch: Default::default(),
            graph_opens: Default::default(),
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
        publish_if_changed(&s, &mut published, at(Some(10)), (false, 0), &wake);
        publish_if_changed(&s, &mut published, at(Some(11)), (false, 0), &wake);
        assert_eq!(
            woke.load(Ordering::Relaxed),
            2,
            "a pid-only change must publish, or the panel never learns the session moved"
        );
        publish_if_changed(&s, &mut published, at(Some(11)), (false, 0), &wake);
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

    // ---- agent-card-status T3: an Antigravity tab's state comes from its own transcript ----

    use super::super::antigravity::fake;

    /// A deriver whose every tab runs an `agy` (at the walk's usual offset) holding
    /// `conversation_of`'s lock, over a temp Antigravity root holding `transcripts`.
    fn antigravity_deriver(
        tag: &str,
        kind: AgentKind,
        conversation_of: fn(u32) -> Option<String>,
        transcripts: &[(&str, &str)],
    ) -> Deriver {
        let (projects, sessions) = fixture_roots(tag);
        let mut d = deriver_at(projects, sessions);
        d.cwd_of = |_| Some(PathBuf::from(THE_CWD));
        d.agent_of = match kind {
            AgentKind::Antigravity => |pid| {
                Some(super::super::agent::Detected {
                    kind: AgentKind::Antigravity,
                    pid: pid + AGENT_PID_OFFSET,
                })
            },
            AgentKind::Codex => |pid| {
                Some(super::super::agent::Detected {
                    kind: AgentKind::Codex,
                    pid: pid + AGENT_PID_OFFSET,
                })
            },
            AgentKind::Gemini => |pid| {
                Some(super::super::agent::Detected {
                    kind: AgentKind::Gemini,
                    pid: pid + AGENT_PID_OFFSET,
                })
            },
            AgentKind::Aider => |pid| {
                Some(super::super::agent::Detected {
                    kind: AgentKind::Aider,
                    pid: pid + AGENT_PID_OFFSET,
                })
            },
            AgentKind::Claude => panic!("a Claude tab is not an Antigravity-path fixture"),
        };
        d.antigravity = super::super::antigravity::Sources {
            root: Some(fake::root(tag, transcripts)),
            conversation_of,
            tail_of: fake::read,
        };
        d
    }

    #[test]
    fn an_antigravity_tab_publishes_its_transcripts_state() {
        // R0.3 / R4.1 / R4.6 — a finished Antigravity turn reads Waiting, as a Claude
        // `end_turn` does; the state is the transcript the lock names; and the pid the probe
        // used is still not published (the panel's re-point compare stays Claude-only).
        // `tab()` walks tmux and proc paths that bump the global cost counters, so this holds
        // the lock the cost tests measure under (`probe::serial`).
        let _g = super::super::probe::serial();
        fake::reset();
        let mut d = antigravity_deriver(
            "ag-publish",
            AgentKind::Antigravity,
            fake::holds_c,
            &[
                (fake::CONV_C, fake::FINISHED),
                (fake::CONV_D, fake::DISPATCHED),
            ],
        );
        let m = d.tab(&tp(900_101), 0, SystemTime::now(), true);
        assert_eq!(
            m.agent,
            Some(Agent {
                kind: AgentKind::Antigravity,
                state: AgentState::Waiting
            })
        );
        assert_eq!(m.agent_pid, None);
        assert_eq!((fake::probes(), fake::reads()), (1, 1));
    }

    #[test]
    fn agents_without_a_reader_are_not_probed() {
        // R4.7 — Codex, Gemini and Aider have no session reader: no probe, no read, and a
        // state the card shows as no status.
        // `tab()` walks tmux and proc paths that bump the global cost counters, so this holds
        // the lock the cost tests measure under (`probe::serial`).
        let _g = super::super::probe::serial();
        for kind in [AgentKind::Codex, AgentKind::Gemini, AgentKind::Aider] {
            fake::reset();
            let mut d = antigravity_deriver(
                "ag-noreader",
                kind,
                fake::holds_c,
                &[(fake::CONV_C, fake::FINISHED)],
            );
            let m = d.tab(&tp(900_101), 0, SystemTime::now(), true);
            assert_eq!(
                m.agent,
                Some(Agent {
                    kind,
                    state: AgentState::Unknown
                })
            );
            assert_eq!((fake::probes(), fake::reads()), (0, 0), "{kind:?}");
        }
    }

    #[test]
    fn the_agent_toggle_off_stops_the_antigravity_probe() {
        // R4.8 — the active tab is still walked (the panel needs its pid), but with
        // `show_agent_status` off nothing about its session is probed or read.
        // `tab()` walks tmux and proc paths that bump the global cost counters, so this holds
        // the lock the cost tests measure under (`probe::serial`).
        let _g = super::super::probe::serial();
        fake::reset();
        let mut d = antigravity_deriver(
            "ag-toggle",
            AgentKind::Antigravity,
            fake::holds_c,
            &[(fake::CONV_C, fake::FINISHED)],
        );
        d.enabled_agent = false;
        let m = d.tab(&tp(900_101), 0, SystemTime::now(), true);
        assert_eq!(m.agent, None);
        assert_eq!((fake::probes(), fake::reads()), (0, 0));
    }

    #[test]
    fn an_idle_antigravity_tab_costs_a_probe_per_ttl_and_publishes_nothing() {
        // R9.1 — after the first refresh, a tab whose lock and transcript do not move is
        // re-probed once per TTL, never re-read, and derives the same `TabMeta` every time —
        // so `publish_if_changed` has nothing to publish and nothing wakes the UI thread.
        // `tab()` walks tmux and proc paths that bump the global cost counters, so this holds
        // the lock the cost tests measure under (`probe::serial`).
        let _g = super::super::probe::serial();
        fake::reset();
        let mut d = antigravity_deriver(
            "ag-idle",
            AgentKind::Antigravity,
            fake::holds_c,
            &[(fake::CONV_C, fake::FINISHED)],
        );
        let now = SystemTime::now();
        let first = d.tab(&tp(900_101), 0, now, true);
        // Anchor the baseline: the first refresh really resolved and read the transcript, so
        // "nothing more" below is measured against a tab that has a state to keep.
        assert_eq!(
            first.agent,
            Some(Agent {
                kind: AgentKind::Antigravity,
                state: AgentState::Waiting
            })
        );
        assert_eq!((fake::probes(), fake::reads()), (1, 1));
        fake::reset();
        let ttl = super::super::activity::RESOLVE_TTL_TICKS;
        for tick in 1..=3 * ttl {
            assert_eq!(d.tab(&tp(900_101), tick, now, true), first, "tick {tick}");
        }
        assert_eq!(fake::probes(), 3, "one probe per TTL");
        assert_eq!(fake::reads(), 0, "an unchanged transcript is never re-read");
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
            graph_agent_cwd: None,
            graph_transcript: None,
            graph_asks_user: false,
            branch: None,
            agent: Some(Agent {
                kind: AgentKind::Aider,
                state: AgentState::Unknown,
            }),
            task: None,
            cwd: None,
            agent_pid: None,
            panes: Vec::new(),
            log: None,
            repo: None,
            graph_agent: None,
            graph_task: None,
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
            graph_agent_cwd: None,
            graph_transcript: None,
            graph_asks_user: false,
            branch: None,
            agent: Some(Agent {
                kind: AgentKind::Claude,
                state: AgentState::Busy,
            }),
            task: None,
            cwd: None,
            agent_pid: None,
            panes: Vec::new(),
            log: None,
            repo: None,
            graph_agent: None,
            graph_task: None,
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

    #[test]
    fn an_unrooted_tab_whose_cwd_is_gone_reads_it_once() {
        // The shell-retry arm is for a **re-rooted** tab whose pane died; a tab already rooted
        // at its own shell has nothing to retry, and asking again would be a second
        // `proc_pidinfo` per refresh for every tab whose cwd cannot be read.
        static CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("cwd-once");
        let mut d = deriver_at(projects, sessions);
        with_dead_tree(&mut d);
        d.cwd_of = |_| {
            CALLS.fetch_add(1, Ordering::Relaxed);
            None
        };
        CALLS.store(0, Ordering::Relaxed);
        let _ = d.tab(&tp(900_198), 0, SystemTime::now(), true);
        assert_eq!(CALLS.load(Ordering::Relaxed), 1);
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

    // ---- the `SetActive` arm's two decisions, now reachable ----

    #[test]
    fn rostered_names_the_tab_asked_for_and_nothing_else() {
        // The `!=` form of this predicate hands the arm the WRONG tab, so a switch derives
        // some other tab's metadata and publishes it under the active tab's id. It survived
        // mutation while the logic was inline in `run`, because reaching it there needed a
        // non-empty roster and therefore a real process walk.
        let roster = [
            TabPid {
                id: TabId(7),
                pid: 700,
            },
            TabPid {
                id: TabId(8),
                pid: 800,
            },
        ];

        assert_eq!(rostered(&roster, TabId(7)).map(|tp| tp.pid), Some(700));
        assert_eq!(
            rostered(&roster, TabId(8)).map(|tp| tp.pid),
            Some(800),
            "the SECOND entry, not merely 'some entry that is not the first'"
        );
        assert_eq!(
            rostered(&roster, TabId(9)),
            None,
            "a tab that is not rostered names nothing — a closed tab must not refresh a \
             surviving one under its id"
        );
        assert_eq!(rostered(&[], TabId(7)), None);
    }

    #[test]
    fn upsert_meta_replaces_that_tabs_slot_and_appends_when_it_has_none() {
        // The `!=` form is the more dangerous of the two: it overwrites a DIFFERENT tab's slot
        // and then, having found a match, never appends the tab it was asked about — so the
        // tab being switched to would never appear in the published set at all.
        let a = TabMeta {
            branch: Some("main".into()),
            ..Default::default()
        };
        let b = TabMeta {
            branch: Some("feature".into()),
            ..Default::default()
        };
        let published = vec![(TabId(1), a.clone()), (TabId(2), b.clone())];

        // Replace in place: same length, same order, only tab 2's slot moves.
        let fresh = TabMeta {
            branch: Some("hotfix".into()),
            ..Default::default()
        };
        let next = upsert_meta(&published, TabId(2), fresh.clone());
        assert_eq!(next, vec![(TabId(1), a.clone()), (TabId(2), fresh.clone())]);

        // Append when absent: the asked-for tab must actually arrive.
        let next = upsert_meta(&published, TabId(3), fresh.clone());
        assert_eq!(
            next,
            vec![
                (TabId(1), a.clone()),
                (TabId(2), b.clone()),
                (TabId(3), fresh.clone())
            ],
            "a newly-active tab with no slot yet must be added, not merged into a neighbour"
        );

        // The empty case, which both the correct and the `!=` form reach identically — pinned
        // so the append path is covered from nothing as well as from something.
        assert_eq!(
            upsert_meta(&[], TabId(1), fresh.clone()),
            vec![(TabId(1), fresh)]
        );
    }

    // ---- idle-timestamps T3 / R3: holding and firing the panel's deadline ----
    //
    // These drive `deadline_due` rather than the thread, for the reason `publish_if_changed`
    // is a free function: the loop needs a real `Receiver` and a real 500 ms tick, so a test
    // of the slot through it would be a sleep with an assertion at the end. What the slot has
    // to get right — fire once, fire never before time, replace rather than accumulate — is
    // all in this one function, and it is testable at full speed.

    #[test]
    fn epoch_ms_reads_the_wall_clock_in_milliseconds() {
        // The deadline travels as an absolute epoch-millis instant computed on the app side
        // from a log timestamp, so this function and `dashboard::idle_deadline_ms` must be on
        // the same scale and the same origin. Nothing else in the loop would notice if they
        // were not: a clock reading seconds, or zero, simply makes every deadline look long
        // past due, and the worker would fire each one on the very next tick — the arm would
        // still "work", one wake and all, just always immediately.
        let before = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the system clock is after 1970")
            .as_millis() as u64;
        let read = epoch_ms();
        let after = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the system clock is after 1970")
            .as_millis() as u64;

        assert!(
            (before..=after).contains(&read),
            "epoch_ms must read the same clock, in the same unit: {before} <= {read} <= {after}"
        );
        // Belt and braces on the unit itself, since the bracket above would also hold for a
        // clock in seconds if all three readings happened to agree.
        assert!(
            read > 1_700_000_000_000,
            "epoch_ms returned {read}, which is not milliseconds since 1970"
        );
    }

    #[test]
    fn r3_1_a_passed_deadline_produces_exactly_one_wake() {
        // THE property of this feature, and the one no source scan can see. A deadline that
        // fired on every tick past its instant would be a 2 Hz repaint cadence — precisely the
        // thing the panel's approved design rejected and the thing the deferred Option 2 would
        // have been. The difference between an edge and a heartbeat is this assertion.
        let mut slot = Some(1_000);
        let mut wakes = 0;
        for tick in 0..20 {
            if deadline_due(&mut slot, 1_500 + tick) {
                wakes += 1;
            }
        }
        assert_eq!(wakes, 1, "an armed deadline is ONE edge, not a cadence");
        assert_eq!(slot, None, "and the slot is cleared by the firing");
    }

    #[test]
    fn r3_2_an_unreached_deadline_produces_no_wake() {
        let mut slot = Some(10_000);
        for tick in 0..20 {
            assert!(!deadline_due(&mut slot, 9_000 + tick));
        }
        assert_eq!(slot, Some(10_000), "and it is still armed for later");
    }

    #[test]
    fn r3_3_arming_replaces_the_pending_deadline() {
        // The real requirement behind the single slot, and it is about REPLACEMENT rather than
        // staleness: a session that resumes and re-quiets must not leave two deadlines racing.
        let mut slot = Some(1_000);
        assert!(
            !deadline_due(&mut slot, 999),
            "T1 is pending and not yet due"
        );
        slot = Some(5_000); // the arm that arrives while the first is pending
        assert!(!deadline_due(&mut slot, 1_200), "T1 is gone, not pending");
        assert!(deadline_due(&mut slot, 5_000), "and T2 is what fires");
        assert_eq!(slot, None);
    }

    #[test]
    fn r3_4_disarming_clears_the_slot() {
        let mut slot = Some(1_000);
        assert!(!deadline_due(&mut slot, 999), "armed before the disarm");
        slot = None; // Cmd::SetPanelDeadline(None)
        for tick in 0..10 {
            assert!(!deadline_due(&mut slot, 100_000 + tick));
        }
    }

    #[test]
    fn the_deadline_fires_at_its_instant_and_not_a_millisecond_later() {
        // `>=`, not `>`. The instant handed over is already "the first instant at which the
        // pill reads IDLE", so firing one millisecond later would be a second off-by-one
        // stacked on the one `idle_deadline_ms` exists to remove.
        let mut slot = Some(300_001);
        assert!(!deadline_due(&mut slot, 300_000), "not due one ms early");
        assert!(deadline_due(&mut slot, 300_001), "due at the instant");
    }

    /// R3.1 + R3.5, driven through the **real `run` loop** rather than through `deadline_due`
    /// alone — and that distinction is the whole reason this test exists.
    ///
    /// It replaces a test that could not fail. The previous version asserted that firing a
    /// deadline bumped no generation and published nothing, but it called `deadline_due`, whose
    /// signature is `(&mut Option<u64>, u64) -> bool`: it is handed neither the shared state nor
    /// the published vector, so the assertions were true by type and would have held just as
    /// well against a `run` that published on every firing tick.
    ///
    /// Worse, `cargo mutants` showed `replace run with ()` SURVIVING: no test executed a single
    /// statement of the worker loop, so the two lines that actually turn an armed deadline into
    /// a wake could be deleted with the whole suite green — the feature wholly inert, the pill
    /// never flipping, and nothing anywhere going red. That is the failure class `CLAUDE.md`
    /// keeps `golden_frame.rs` for: every other observable reports a decision while the thing
    /// the decision was for never happens.
    ///
    /// **The roster is left EMPTY on purpose.** With no roster the loop blocks on `recv()` with
    /// no timeout (D4), so every iteration here is driven by a command this test sends — no
    /// process walks, no `SystemTime` dependence, no 500 ms tick to wait out, and no sleep. The
    /// loop still passes over the deadline check once per iteration, which is exactly the
    /// property under test. It is also the precondition D16 records: with an empty roster the
    /// deadline is only reached when something else wakes the loop.
    #[test]
    fn r3_1_r3_5_the_worker_loop_fires_an_armed_deadline_once_and_publishes_nothing() {
        use std::sync::mpsc;

        let (tx, rx) = mpsc::channel::<Cmd>();
        let (woke_tx, woke_rx) = mpsc::channel::<()>();
        let wakes = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&wakes);
        let s = shared();
        let worker_shared = Arc::clone(&s);

        let handle = std::thread::spawn(move || {
            run(worker_shared, rx, move || {
                counter.fetch_add(1, Ordering::Release);
                let _ = woke_tx.send(());
            })
        });

        let generation_before = s.generation.load(Ordering::Acquire);

        // Arm a deadline already long past due; the next pass over the check must fire it.
        tx.send(Cmd::SetPanelDeadline(Some(1)))
            .expect("worker alive");
        woke_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("an armed, past-due deadline must wake the loop");
        assert_eq!(wakes.load(Ordering::Acquire), 1);

        // Ten more iterations over a slot the firing already cleared. None may wake.
        for _ in 0..10 {
            tx.send(Cmd::SetActive(None)).expect("worker alive");
        }
        // Re-arm and wait for the second wake. Because the channel is FIFO, receiving this
        // proves all ten commands above were processed — which is what makes the count
        // assertion below meaningful without sleeping for an arbitrary interval.
        tx.send(Cmd::SetPanelDeadline(Some(1)))
            .expect("worker alive");
        woke_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("a re-armed deadline must wake the loop again");

        assert_eq!(
            wakes.load(Ordering::Acquire),
            2,
            "one wake per ARMED deadline — the ten iterations in between fired nothing, which \
             is what separates a bounded edge from the repaint cadence RP-D2 rejected"
        );

        // R3.5, now asserted where it can actually fail: the deadline rides the worker's loop
        // but not its publish path. `run` holds the shared state this test also holds, so a
        // firing tick that took the lock, bumped the generation or published would show here.
        assert_eq!(
            s.generation.load(Ordering::Acquire),
            generation_before,
            "firing a deadline is not a metadata change, and the app's band logic keys on this"
        );
        assert!(
            s.tabs.lock().expect("not poisoned").is_empty(),
            "and a firing tick publishes nothing"
        );

        drop(tx); // channel closes → `run` returns → the thread ends cleanly (D14)
        handle
            .join()
            .expect("the worker thread exits on disconnect");
    }

    // ---- repository identity and graph readiness (session-flow-graph D10, R4.6, R4.7) ----

    /// A fixed per-process fixture directory, nameable from a non-capturing `fn` — the
    /// `cwd_of` seam is a function pointer, so it cannot close over a `tmpdir` result.
    fn repo_fixture(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("tabmeta-wk-repo-{}", std::process::id()))
            .join(name)
    }

    /// Two main repositories: `shell/` (where every tab's shell sits) and `agent/` (where
    /// every `claude` runs), so a test can tell which cwd an identity was resolved from.
    fn make_repo_fixtures() {
        for name in ["shell", "agent"] {
            let git = repo_fixture(name).join(".git");
            std::fs::create_dir_all(&git).expect("mkdir .git");
            std::fs::write(git.join("HEAD"), format!("ref: refs/heads/{name}-br\n"))
                .expect("write HEAD");
        }
    }

    fn main_repo(name: &str) -> Option<RepoId> {
        Some(RepoId {
            common: repo_fixture(name).join(".git"),
            main_root: repo_fixture(name),
            linked: false,
        })
    }

    /// Shell pids (`900_xxx`) sit in `shell/`, agent pids (`901_xxx`) in `agent/` — except an
    /// agent pid ending in `99`, whose cwd cannot be read (it exited after the walk).
    fn with_split_cwds(d: &mut Deriver) {
        with_fake_tree(d);
        d.cwd_of = |pid| match pid {
            p if p >= 900_000 + AGENT_PID_OFFSET && p % 100 == 99 => None,
            p if p >= 900_000 + AGENT_PID_OFFSET => Some(repo_fixture("agent")),
            _ => Some(repo_fixture("shell")),
        };
    }

    #[test]
    fn a_wanted_tab_publishes_its_claudes_repository_not_its_shells() {
        let _g = super::super::probe::serial();
        make_repo_fixtures();
        let (projects, sessions) = fixture_roots("repo-agent-cwd");
        let mut d = deriver_at(projects, sessions);
        with_split_cwds(&mut d);
        d.graph_wanted = true;
        let m = d.tab(&tp(900_401), 0, SystemTime::now(), true);
        assert_eq!(
            m.cwd,
            Some(repo_fixture("shell")),
            "the card's cwd stays the shell's"
        );
        assert_eq!(
            m.repo,
            main_repo("agent"),
            "G5: grouped by where claude runs"
        );
        // The claude's cwd unreadable → the shell's.
        let m = d.tab(&tp(900_499), 1, SystemTime::now(), true);
        assert_eq!(m.repo, main_repo("shell"));
    }

    #[test]
    fn identity_is_resolved_only_while_wanted_and_for_any_agent() {
        let _g = super::super::probe::serial();
        make_repo_fixtures();
        let (projects, sessions) = fixture_roots("repo-gated");
        let mut d = deriver_at(projects, sessions);
        with_split_cwds(&mut d);
        super::super::probe::reset_all();
        let m = d.tab(&tp(900_402), 0, SystemTime::now(), true);
        assert_eq!(m.repo, None, "graph closed: nothing resolved");
        assert_eq!(
            super::super::probe::GIT_STAT.get(),
            0,
            "and nothing paid (G1)"
        );
        d.graph_wanted = true;
        d.agent_of = |pid| {
            Some(super::super::agent::Detected {
                kind: AgentKind::Codex,
                pid: pid + AGENT_PID_OFFSET,
            })
        };
        let m = d.tab(&tp(900_402), 1, SystemTime::now(), true);
        // R14.5 (amended 2026-10-21): every detected agent is a session, so a Codex tab
        // resolves from its own process's cwd exactly as a Claude tab does.
        assert_eq!(m.repo, main_repo("agent"), "a Codex session's repository");
        with_dead_tree(&mut d);
        assert_eq!(d.tab(&tp(900_402), 2, SystemTime::now(), true).repo, None);
    }

    #[test]
    fn a_non_active_pane_publishes_its_shells_folder_and_its_claudes_repository() {
        let _g = super::super::probe::serial();
        make_repo_fixtures();
        let (projects, sessions) = fixture_roots("repo-pane");
        let mut d = deriver_at(projects, sessions);
        with_split_cwds(&mut d);
        d.enabled_git = true;
        let p = d.pane(900_403, 0, SystemTime::now());
        assert_eq!(p.cwd, Some(repo_fixture("shell")));
        assert_eq!(
            p.branch.as_deref(),
            Some("shell-br"),
            "the branch of its own cwd"
        );
        assert_eq!(p.repo, main_repo("agent"));
        let p = d.pane(900_499, 1, SystemTime::now());
        assert_eq!(
            p.repo,
            main_repo("shell"),
            "fallback to the pane's shell cwd"
        );
        // The branch is the sidebar card's, so it follows the card's toggle.
        d.enabled_git = false;
        let p = d.pane(900_403, 2, SystemTime::now());
        assert_eq!((p.branch, p.repo), (None, main_repo("agent")));
        // A non-Claude agent is a session too (R14.5): its own process's repository.
        d.agent_of = |pid| {
            Some(super::super::agent::Detected {
                kind: AgentKind::Codex,
                pid: pid + AGENT_PID_OFFSET,
            })
        };
        let p = d.pane(900_403, 3, SystemTime::now());
        assert_eq!((p.cwd.is_some(), p.repo), (true, main_repo("agent")));
        // No agent: nothing at all, and no cwd read.
        with_dead_tree(&mut d);
        assert_eq!(d.pane(900_404, 4, SystemTime::now()), blank_pane(900_404));
    }

    #[test]
    fn the_graph_edge_sets_readiness_with_a_full_pass_and_clears_on_close() {
        let _g = super::super::probe::serial();
        make_repo_fixtures();
        let (projects, sessions) = fixture_roots("repo-edge");
        let mut d = deriver_at(projects, sessions);
        with_split_cwds(&mut d);
        let roster = [tp(900_411), tp(900_412), tp(900_413), tp(900_414)];
        // Closed: one ordinary pass publishes no identity anywhere.
        let closed = d.pass(&roster, None, 1, SystemTime::now(), &[]);
        assert!(closed.iter().all(|(_, m)| m.repo.is_none()));
        // A no-op edge (already closed) does nothing.
        assert!(d
            .graph_edge(false, &roster, None, 1, SystemTime::now(), &closed, false)
            .is_none());

        // Open: every tab — not only tick 1's stripe — resolves, and the pass sets readiness.
        let (open, ready) = d
            .graph_edge(true, &roster, None, 1, SystemTime::now(), &closed, false)
            .expect("off→on is an edge");
        assert!(ready);
        assert!(d.graph_wanted);
        assert!(
            open.iter().all(|(_, m)| m.repo == main_repo("agent")),
            "{open:?}"
        );
        assert!(
            !d.graph_just_enabled,
            "the edge's flag was consumed by its pass"
        );
        // Re-sending `true` is not an edge.
        assert!(d
            .graph_edge(true, &roster, None, 1, SystemTime::now(), &open, ready)
            .is_none());

        // Close: nothing derived, every identity and the readiness cleared.
        super::super::probe::reset_all();
        let (shut, ready) = d
            .graph_edge(false, &roster, None, 2, SystemTime::now(), &open, true)
            .expect("on→off is an edge");
        assert!(!ready);
        assert!(!d.graph_wanted);
        assert_eq!(shut, strip_graph(&open));
        assert_eq!(
            super::super::probe::GIT_STAT.get(),
            0,
            "closing derives nothing"
        );
        assert_eq!(super::super::probe::PROC_CWD.get(), 0);
    }

    #[test]
    fn a_readiness_flip_alone_publishes_and_wakes() {
        // With nothing to resolve (no tabs), opening changes no `TabMeta`; the flip itself
        // must still bump the generation, or the UI would read `resolving…` indefinitely.
        let s = shared();
        let woke = Arc::new(AtomicU64::new(0));
        let w = woke.clone();
        let wake = move || {
            w.fetch_add(1, Ordering::Relaxed);
        };
        let mut published = Vec::new();
        publish_if_changed(&s, &mut published, Vec::new(), (false, 0), &wake);
        assert_eq!(woke.load(Ordering::Relaxed), 0, "nothing changed");
        publish_if_changed(&s, &mut published, Vec::new(), (true, 0), &wake);
        assert_eq!(woke.load(Ordering::Relaxed), 1);
        assert_eq!(s.generation.load(Ordering::Acquire), 1);
        assert!(s.graph_ready.load(Ordering::Acquire));
        publish_if_changed(&s, &mut published, Vec::new(), (true, 0), &wake);
        assert_eq!(
            woke.load(Ordering::Relaxed),
            1,
            "unchanged readiness: no wake"
        );
        publish_if_changed(&s, &mut published, Vec::new(), (false, 0), &wake);
        assert_eq!(s.generation.load(Ordering::Acquire), 2);
        assert!(!s.graph_ready.load(Ordering::Acquire));
    }

    #[test]
    fn a_repeated_open_with_a_new_epoch_publishes_that_epoch() {
        // session-flow-graph D10: a `true` with no edge still names the open asking. Were the
        // epoch adopted only on the off→on edge, that open would wait under `resolving…` for
        // an epoch the worker never publishes.
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel::<Cmd>();
        let (woke_tx, woke_rx) = mpsc::channel::<()>();
        let s = shared();
        let worker_shared = Arc::clone(&s);
        let handle = std::thread::spawn(move || {
            run(worker_shared, rx, move || {
                let _ = woke_tx.send(());
            })
        });
        tx.send(Cmd::SetGraphWanted {
            wanted: true,
            epoch: 1,
        })
        .expect("worker alive");
        tx.send(Cmd::SetGraphWanted {
            wanted: true,
            epoch: 2,
        })
        .expect("worker alive");
        for _ in 0..2 {
            woke_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("each epoch publishes");
        }
        assert!(s.graph_ready.load(Ordering::Acquire));
        assert_eq!(s.graph_epoch.load(Ordering::Acquire), 2);
        drop(tx);
        handle
            .join()
            .expect("the worker thread exits on disconnect");
    }

    #[test]
    fn the_worker_loop_publishes_readiness_on_open_and_clears_it_on_close() {
        // R4.7 through `run`: the `SetGraphWanted` arm, with an empty roster so no process is
        // walked. FIFO again makes the wake counts exact without sleeping.
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel::<Cmd>();
        let (woke_tx, woke_rx) = mpsc::channel::<()>();
        let s = shared();
        let worker_shared = Arc::clone(&s);
        let handle = std::thread::spawn(move || {
            run(worker_shared, rx, move || {
                let _ = woke_tx.send(());
            })
        });
        tx.send(Cmd::SetGraphWanted {
            wanted: true,
            epoch: 1,
        })
        .expect("worker alive");
        woke_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("opening publishes readiness");
        assert!(s.graph_ready.load(Ordering::Acquire));
        assert_eq!(
            s.graph_epoch.load(Ordering::Acquire),
            1,
            "readiness names the open"
        );
        tx.send(Cmd::SetGraphWanted {
            wanted: true,
            epoch: 1,
        })
        .expect("worker alive");
        tx.send(Cmd::SetGraphWanted {
            wanted: false,
            epoch: 0,
        })
        .expect("worker alive");
        woke_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("closing publishes the cleared readiness");
        assert!(!s.graph_ready.load(Ordering::Acquire));
        assert_eq!(
            s.generation.load(Ordering::Acquire),
            2,
            "the redundant `true` published nothing"
        );
        drop(tx);
        handle
            .join()
            .expect("the worker thread exits on disconnect");
        assert!(woke_rx.try_recv().is_err(), "exactly two wakes");
    }

    // ---- session-overview-ui T3 / R1.6: `graph_asks_user` and `graph_agent_cwd` ----

    /// Where every agent process runs in these tests — deliberately not `THE_CWD`, where every
    /// shell sits, so a test can tell the agent's cwd (D3) from the shell's (G5).
    const AGENT_CWD: &str = "/proj/agent-cwd";

    /// Shell pids (`900_xxx`) report `THE_CWD`, agent pids (`901_xxx` and up) `AGENT_CWD`.
    fn agent_cwd_of(pid: u32) -> Option<PathBuf> {
        Some(PathBuf::from(if pid >= 900_000 + AGENT_PID_OFFSET {
            AGENT_CWD
        } else {
            THE_CWD
        }))
    }

    /// A Claude assistant turn that called `AskUserQuestion` — `AssistantAsksUser` (D2).
    const CLAUDE_ASKS: &str = concat!(
        r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"tool_use","#,
        r#""content":[{"type":"tool_use","id":"toolu_01","name":"AskUserQuestion","#,
        r#""input":{"questions":[]}}]}}"#,
        "\n"
    );
    /// A finished Claude turn — `AssistantEndTurn`: `Waiting`, but not asking (D2's Idle).
    const CLAUDE_ENDS: &str = concat!(
        r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn","#,
        r#""content":[{"type":"text","text":"Done."}]}}"#,
        "\n"
    );

    #[test]
    fn r1_6_an_antigravity_tab_publishes_asks_user_and_agent_cwd_on_graph_passes_only() {
        let _g = super::super::probe::serial();
        fake::reset();
        let mut d = antigravity_deriver(
            "r1-6-ag",
            AgentKind::Antigravity,
            fake::holds_c,
            &[(fake::CONV_C, fake::ASKING)],
        );
        d.cwd_of = agent_cwd_of;
        let now = SystemTime::now();

        d.graph_wanted = true;
        let m = d.tab(&tp(900_601), 0, now, true);
        assert_eq!(
            m.graph_agent.map(|a| a.state),
            Some(AgentState::Waiting),
            "the ask reads Waiting"
        );
        assert!(m.graph_asks_user, "an ask_question last step asks the user");
        assert_eq!(m.graph_agent_cwd, Some(PathBuf::from(AGENT_CWD)));
        assert_eq!(
            m.cwd,
            Some(PathBuf::from(THE_CWD)),
            "the shell's cwd is unchanged"
        );

        d.graph_wanted = false;
        let m = d.tab(&tp(900_601), 1, now, true);
        assert_eq!(
            m.agent.map(|a| a.state),
            Some(AgentState::Waiting),
            "the card still classifies it"
        );
        assert!(!m.graph_asks_user, "a non-graph pass publishes false");
        assert_eq!(m.graph_agent_cwd, None, "a non-graph pass publishes None");
    }

    #[test]
    fn r1_6_the_graph_edge_publishes_both_on_open_and_clears_both_on_close() {
        // R1.6 through the worker's own edge: the off→on pass publishes, the on→off edge
        // strips, and an ordinary closed pass derives them as false/None again.
        let _g = super::super::probe::serial();
        fake::reset();
        let mut d = antigravity_deriver(
            "r1-6-edge",
            AgentKind::Antigravity,
            fake::holds_c,
            &[(fake::CONV_C, fake::ASKING)],
        );
        d.cwd_of = agent_cwd_of;
        let roster = [tp(900_611), tp(900_612)];
        let now = SystemTime::now();
        let (open, _) = d
            .graph_edge(true, &roster, None, 0, now, &[], false)
            .expect("off→on is an edge");
        for (id, m) in &open {
            assert!(m.graph_asks_user, "{id:?}");
            assert_eq!(m.graph_agent_cwd, Some(PathBuf::from(AGENT_CWD)), "{id:?}");
        }
        let (shut, _) = d
            .graph_edge(false, &roster, None, 1, now, &open, true)
            .expect("on→off is an edge");
        let closed = d.pass(&roster, Some(roster[0].id), 2, now, &shut);
        for (id, m) in shut.iter().chain(&closed) {
            assert_eq!(
                (m.graph_asks_user, &m.graph_agent_cwd),
                (false, &None),
                "{id:?}"
            );
        }
    }

    #[test]
    fn r1_6_a_finished_antigravity_turn_is_waiting_but_does_not_ask() {
        // D2 — `Waiting` alone is not "asks the user": a finished turn publishes false even on
        // a graph pass, which is what keeps it Idle in the overview.
        let _g = super::super::probe::serial();
        fake::reset();
        let mut d = antigravity_deriver(
            "r1-6-ag-done",
            AgentKind::Antigravity,
            fake::holds_c,
            &[(fake::CONV_C, fake::FINISHED)],
        );
        d.cwd_of = agent_cwd_of;
        d.graph_wanted = true;
        let m = d.tab(&tp(900_621), 0, SystemTime::now(), true);
        assert_eq!(m.graph_agent.map(|a| a.state), Some(AgentState::Waiting));
        assert!(!m.graph_asks_user);
        assert_eq!(m.graph_agent_cwd, Some(PathBuf::from(AGENT_CWD)));
    }

    #[test]
    fn r1_6_a_claude_tab_publishes_asks_user_from_its_log() {
        let _g = super::super::probe::serial();
        let (projects, sessions) = fixture_roots("r1-6-claude");
        let cwd = Path::new(THE_CWD);
        write_log(&projects, cwd, SID_A, CLAUDE_ASKS);
        write_log(&projects, cwd, SID_B, CLAUDE_ENDS);
        write_registry(&sessions, agent_pid(900_631), SID_A, cwd, 1_000);
        write_registry(&sessions, agent_pid(900_632), SID_B, cwd, 1_000);
        let mut d = deriver_at(projects, sessions);
        with_fake_tree(&mut d);
        d.cwd_of = agent_cwd_of;
        let now = SystemTime::now();

        d.graph_wanted = true;
        let asks = d.tab(&tp(900_631), 0, now, true);
        assert_eq!(asks.graph_agent.map(|a| a.state), Some(AgentState::Waiting));
        assert!(asks.graph_asks_user, "AskUserQuestion asks the user");
        assert_eq!(asks.graph_agent_cwd, Some(PathBuf::from(AGENT_CWD)));
        let ends = d.tab(&tp(900_632), 0, now, true);
        assert_eq!(ends.graph_agent.map(|a| a.state), Some(AgentState::Waiting));
        assert!(
            !ends.graph_asks_user,
            "an end_turn is Waiting but not asking"
        );

        d.graph_wanted = false;
        let m = d.tab(&tp(900_631), 1, now, true);
        assert_eq!(m.agent.map(|a| a.state), Some(AgentState::Waiting));
        assert_eq!((m.graph_asks_user, m.graph_agent_cwd), (false, None));
    }

    #[test]
    fn r1_6_panes_publish_asks_user_and_agent_cwd_on_graph_passes_only() {
        // R1.7's input: the non-active pane is derived on its own shell and publishes its own
        // flag and agent cwd; the active pane carries the tab's.
        let _g = super::super::probe::serial();
        fake::reset();
        let mut d = antigravity_deriver(
            "r1-6-pane",
            AgentKind::Antigravity,
            fake::holds_c,
            &[(fake::CONV_C, fake::ASKING)],
        );
        d.cwd_of = agent_cwd_of;
        d.rooted_of = rooted_at_pane;
        d.graph_wanted = true;
        d.pane_memo = PaneMemo::seeded(
            0,
            super::super::tmux::parse_panes(&format!(
                "work:1 1 {ROOTED_PANE}\nwork:1 0 {OTHER_PANE}\n"
            )),
        );
        let now = SystemTime::now();
        let m = d.tab(&tp(ROOTED_TAB), 0, now, true);
        let got: Vec<(u32, bool, bool, Option<PathBuf>)> = m
            .panes
            .iter()
            .map(|p| {
                (
                    p.pane_pid,
                    p.active,
                    p.graph_asks_user,
                    p.graph_agent_cwd.clone(),
                )
            })
            .collect();
        let agent = Some(PathBuf::from(AGENT_CWD));
        assert_eq!(
            got,
            [
                (ROOTED_PANE, true, true, agent.clone()),
                (OTHER_PANE, false, true, agent)
            ]
        );
        assert_eq!(
            m.panes[1].cwd,
            Some(PathBuf::from(THE_CWD)),
            "the pane's own cwd stays its shell's"
        );

        // A non-active pane whose turn merely finished does not ask.
        let mut done = antigravity_deriver(
            "r1-6-pane-done",
            AgentKind::Antigravity,
            fake::holds_c,
            &[(fake::CONV_C, fake::FINISHED)],
        );
        done.cwd_of = agent_cwd_of;
        let p = done.pane(OTHER_PANE, 0, now);
        assert_eq!(p.agent.map(|a| a.state), Some(AgentState::Waiting));
        assert!(!p.graph_asks_user);
        assert_eq!(p.graph_agent_cwd, Some(PathBuf::from(AGENT_CWD)));

        // Not wanted: no panes at all, and the tab publishes neither field.
        d.graph_wanted = false;
        let m = d.tab(&tp(ROOTED_TAB), 1, now, true);
        assert!(m.panes.is_empty());
        assert_eq!((m.graph_asks_user, m.graph_agent_cwd), (false, None));
    }

    // ---- session-overview-ui D24 / R11.4: `graph_transcript` ----

    #[test]
    fn r11_4_an_antigravity_tab_publishes_its_transcript_on_graph_passes_only() {
        let _g = super::super::probe::serial();
        fake::reset();
        let mut d = antigravity_deriver(
            "r11-4-ag",
            AgentKind::Antigravity,
            fake::holds_c,
            &[(fake::CONV_C, fake::FINISHED)],
        );
        let want = fake::transcript(
            d.antigravity.root.as_deref().expect("a fake root"),
            fake::CONV_C,
        );
        let now = SystemTime::now();

        d.graph_wanted = true;
        let m = d.tab(&tp(900_701), 0, now, true);
        assert_eq!(m.graph_transcript, Some(want), "a graph pass publishes it");
        assert_eq!(m.log, None, "`log` stays Claude's");

        d.graph_wanted = false;
        let m = d.tab(&tp(900_701), 1, now, true);
        assert_eq!(
            m.agent.map(|a| a.state),
            Some(AgentState::Waiting),
            "the card still reads the transcript"
        );
        assert_eq!(m.graph_transcript, None, "a non-graph pass publishes None");
    }

    #[test]
    fn r11_4_no_lock_and_non_antigravity_agents_publish_no_transcript() {
        let _g = super::super::probe::serial();
        fake::reset();
        let now = SystemTime::now();
        let mut unnamed = antigravity_deriver(
            "r11-4-none",
            AgentKind::Antigravity,
            fake::holds_none,
            &[(fake::CONV_C, fake::FINISHED)],
        );
        unnamed.graph_wanted = true;
        assert_eq!(
            unnamed.tab(&tp(900_711), 0, now, true).graph_transcript,
            None
        );

        let mut codex = antigravity_deriver(
            "r11-4-codex",
            AgentKind::Codex,
            fake::holds_c,
            &[(fake::CONV_C, fake::FINISHED)],
        );
        codex.graph_wanted = true;
        assert_eq!(codex.tab(&tp(900_712), 0, now, true).graph_transcript, None);
    }

    #[test]
    fn r11_4_panes_publish_their_own_transcript_and_the_edge_clears_it() {
        let _g = super::super::probe::serial();
        fake::reset();
        let mut d = antigravity_deriver(
            "r11-4-pane",
            AgentKind::Antigravity,
            fake::holds_c,
            &[(fake::CONV_C, fake::FINISHED)],
        );
        let want = Some(fake::transcript(
            d.antigravity.root.as_deref().expect("a fake root"),
            fake::CONV_C,
        ));
        d.rooted_of = rooted_at_pane;
        d.graph_wanted = true;
        d.pane_memo = PaneMemo::seeded(
            0,
            super::super::tmux::parse_panes(&format!(
                "work:1 1 {ROOTED_PANE}\nwork:1 0 {OTHER_PANE}\n"
            )),
        );
        let now = SystemTime::now();
        let m = d.tab(&tp(ROOTED_TAB), 0, now, true);
        let got: Vec<(u32, bool, Option<PathBuf>)> = m
            .panes
            .iter()
            .map(|p| (p.pane_pid, p.active, p.graph_transcript.clone()))
            .collect();
        assert_eq!(
            got,
            [
                (ROOTED_PANE, true, want.clone()),
                (OTHER_PANE, false, want.clone())
            ]
        );
        // The non-active pane's comes from its own derivation, not a copy of the tab's.
        assert_eq!(d.pane(OTHER_PANE, 0, now).graph_transcript, want);

        // Closing the graph strips the tab's field and its panes with it.
        let shut = strip_graph(&[(TabId(ROOTED_TAB as u64), m)]);
        assert_eq!(shut[0].1.graph_transcript, None);
        assert!(shut[0].1.panes.is_empty());
    }
}
