//! Per-tab metadata — git branch + AI-agent status — computed off the UI thread and
//! cached, so no render frame performs filesystem or process I/O (SD4).
//!
//! One worker thread ([`worker`]) polls each tab's descendant processes and its
//! cwd's `.git`, writes results into an `Arc`-shared, generation-stamped cache, and
//! wakes the UI thread through the existing `OverlayPending` path (D1, D3). The UI
//! thread only ever *reads*, via [`TabMetaHandle::sync`] into a caller-owned
//! [`TabMetaSnapshot`] (D2). `sidebar-row-redesign` maps [`AgentKind`] to a renderer
//! skin — this module never names a renderer sidebar-vocabulary type (D11).
//!
//! On R9.1 (no dependency on `sidebar-foundations`' new renderer types): this module
//! imports only the structural `Overlay::layer()` seam types `DrawLayer` / `FrameCtx`,
//! which predate that feature, so it still compiles when the sidebar renderer files
//! are absent — R9.1's actual build-order intent. It names no renderer skin/theme
//! vocabulary (the intent-grep in T1's gate checks for exactly those symbols).

mod activity;
mod antigravity;
// `pub(crate)` since ai-monitor-antigravity T4: `tabsession` asks which agent a tab is
// running so the overlay can pick the matching watcher, and the classification tables
// (including the auto-updater's `agy.<id>.old` rename) already live here. Re-deriving them
// in `tabsession` would be a second copy of a table that took its own spec to get right.
pub(crate) mod agent;
mod git;
// `pub(crate)` unlike its private siblings: the crate-root `tabsession` module consumes
// `start_ms_of_pid` from here (tabmeta-session-keying D6), so that the process start time
// feeding `ai_monitor::validated_session_id` has exactly one implementation.
pub(crate) mod proc;
// `pub(crate)` unlike its private siblings: the crate-root `tabsession` module consumes
// `effective_root_uncached` from here, and `super::` from there is the crate root
// (tmux-root-resolution R8.3).
pub(crate) mod tmux;
mod worker;

/// Re-exported for the crate root's test-facing surface (session-flow-viewer D17,
/// session-flow-graph R4); `worker` and `git` themselves stay private.
pub use git::{repo_of, RepoId};
pub use worker::{graph_pass, publish_graph_state};

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use renderer::{DrawLayer, FrameCtx};

use crate::seam::{AppCtx, Handled, KeyInput, Overlay};
use crate::session::TabId;

// ---------------------------------------------------------------------------
// Public types (D8, D11) — illegal states unrepresentable.
// ---------------------------------------------------------------------------

/// A tab's derived metadata. `default()` = a plain zsh tab in a non-repo.
///
/// **`cwd` is the one field where `default()` is not simply "the correct value for a
/// not-yet-polled tab"** (tmux-pane-cwd D8). `cwd: None` means two distinguishable things —
/// this tab has not been polled yet, or it was polled and no cwd could be resolved — and the
/// two are not equivalent to a consumer: the first is a gap to fall back through, the second
/// is an answer. `Session::cwd()` treats both as "fall back to my own shell", which is right
/// for the title but would silently open the ai-monitor on the wrong project during the
/// unpolled window; that is why the overlay resolves its own cwd on open rather than reading
/// this field (D12). Only `branch`, `task` and `cwd` allocate.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct TabMeta {
    pub branch: Option<String>,
    pub agent: Option<Agent>,
    /// The tab's working directory — the **active tmux pane's** when the tab runs a client,
    /// else the tab's own shell's (tmux-pane-cwd D1/R1.2). Resolved here, on the worker
    /// thread, so `Session::cwd()`'s hot callers (every keystroke, every sidebar resync) read
    /// a published value instead of paying a syscall each. Deliberately **not** gated on the
    /// sidebar display toggles (D13) — see `Deriver::tab`.
    pub cwd: Option<PathBuf>,
    /// The live "current task" label for the tab (tab-identity): the session's latest
    /// substantive Claude user prompt, one-line normalised. `None` when there is no
    /// agent, no resolvable log, or no substantive prompt ever seen — the renderer then
    /// falls back to the cwd title. Derived by the worker (T2), consumed by the sidebar
    /// card (T3); persisted across ticks so an idle agent keeps its last label (D3/D6).
    pub task: Option<String>,
    /// The pid of this tab's **Claude** agent process, or `None` when it has none.
    ///
    /// Published for a consumer that draws nothing with it: the ai-monitor panel derives its
    /// log directory from the chosen `claude`'s own cwd, and that cwd is fixed at the process's
    /// launch — so the directory goes stale exactly when the *pid* changes. A user who quits
    /// `claude`, `cd`s elsewhere in the same pane and starts a new one would otherwise keep
    /// seeing the old session indefinitely, because the panel stops re-resolving once attached.
    /// This worker already walks each tab's tree every 500 ms and already re-roots at the tmux
    /// pane, so the pid is **already computed**; publishing it turns the panel's staleness check
    /// into an integer compare on a wake it already receives, adding no subprocess to any path.
    ///
    /// **Claude-kind only** (D9), and the scoping is a correctness constraint rather than a
    /// preference: the walk finds *an agent*, and an `agy` or `codex` pid could never equal the
    /// `claude` pid the panel opened on, so an unscoped value would read as "a different pid"
    /// on every single tick — a re-point storm, each iteration re-forking `/bin/ps`, respawning
    /// the watcher and re-running the whole backfill.
    ///
    /// **Not gated on `[sidebar].show_agent_status`** for the active tab — see `Deriver::tab`.
    /// A display preference must not be able to silently disable the panel's staleness check.
    pub agent_pid: Option<u32>,
    /// The panes of the tmux window this tab's client shows, in tmux's order
    /// (session-flow-viewer D9). **Empty** unless the flow graph has asked for them
    /// ([`TabMetaHandle::set_graph_wanted`]) and the window has at least two panes — so a
    /// non-tmux tab, a single-pane window, a dead server and a closed graph all publish the
    /// same "unsplit" answer (D14). Exactly one entry is `active`: the pane whose pid the card's
    /// `agent` was derived from, never tmux's `#{pane_active}`.
    pub panes: Vec<PaneMeta>,
    /// The session log the tab's Claude status was classified from — `activity::Found.log`,
    /// published so the session graph reads that session's subagents from the same file the card
    /// read its state from (session-flow-viewer D9/D10). `None` with no Claude agent or no
    /// resolvable log.
    pub log: Option<PathBuf>,
    /// The repository the tab's agent session runs in (session-flow-graph D10, R14.5), resolved
    /// from the detected agent process's own cwd, of any [`AgentKind`] — not the shell's, so a
    /// `cd` does not regroup a session that never moved (G5). `None` means "no repository"
    /// **only while [`TabMetaSnapshot::graph_ready`]**; with the graph closed it is always
    /// `None`, because nothing is resolved then (G1). `None` too for a tab with no detected
    /// agent.
    pub repo: Option<RepoId>,
    /// The tab's detected agent **as the session graph sees it** — [`TabMeta::agent`] without
    /// `[sidebar].show_agent_status` applied (session-flow-graph, user decision 2026-10-22: the
    /// graph ignores that toggle). Classified by the same `activity` chain as `agent`, so with
    /// the toggle on the two are equal; with it off `agent` is `None` and this is not.
    ///
    /// Set only on a pass that derives the graph and `None` otherwise, like `panes` and `repo`
    /// — a closed graph publishes nothing here and pays nothing for it (G1). Read by the
    /// Cmd+Shift+F overlay alone ([`crate::overview::build_overview`], which falls back to
    /// `agent`/`task` when this is `None`); the sidebar card, the title and the dialog scan keep reading `agent`, so the
    /// toggle governs them exactly as before.
    pub graph_agent: Option<Agent>,
    /// [`TabMeta::task`] without the toggle, on the same terms as `graph_agent` — the graph
    /// card's line 1 is the label the card would show with the toggle on.
    pub graph_task: Option<String>,
    /// Whether the agent's resolved log's last meaningful message is
    /// `LastMessage::AssistantAsksUser` (session-overview-ui D2). Set on graph passes only,
    /// like [`TabMeta::graph_agent`]; `false` otherwise.
    pub graph_asks_user: bool,
    /// The detected agent process's own cwd (session-overview-ui D3) — the one `repo` is
    /// resolved from, not the shell's `cwd` (G5). Graph passes only; `None` otherwise.
    pub graph_agent_cwd: Option<PathBuf>,
    /// The transcript of the tab's **Antigravity** session — the one its `agy`'s presence
    /// lock names, which the card's state was just classified from (agent-card-status D11) —
    /// published so the overview's digest thread can fold its subagents, prompts and turns
    /// (session-overview-ui D24). The Antigravity counterpart of [`TabMeta::log`]. Graph
    /// passes only; `None` otherwise, and `None` for every other agent kind.
    pub graph_transcript: Option<PathBuf>,
}

/// One pane of a split tab (session-flow-viewer D9).
///
/// For the `active` pane, `agent` and `log` are copies of the tab's own
/// [`TabMeta::graph_agent`] and [`TabMeta::log`] — one derivation, not two, so the graph
/// cannot disagree with the card. For every other pane they come from the same
/// `agent_of_pid → resolve_log → state_of_found` chain run against that pane's shell.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PaneMeta {
    pub pane_pid: u32,
    pub active: bool,
    pub agent: Option<Agent>,
    pub log: Option<PathBuf>,
    /// The pane's shell cwd and its branch, for the pane card's folder and branch lines
    /// (session-flow-graph D3/D10); the active pane's are the tab's own. Read only once an agent
    /// is found, so a plain shell pane publishes `None` and still costs one walk. `branch` stays
    /// behind `[sidebar].show_git_branch`, exactly as the tab's does — the card it feeds is the
    /// sidebar's card.
    pub cwd: Option<PathBuf>,
    pub branch: Option<String>,
    /// The pane's agent session's repository (any kind), by the same rule as [`TabMeta::repo`].
    pub repo: Option<RepoId>,
    /// [`TabMeta::graph_asks_user`] for this pane's session (session-overview-ui D2).
    pub graph_asks_user: bool,
    /// [`TabMeta::graph_agent_cwd`] for this pane's agent (session-overview-ui D3).
    pub graph_agent_cwd: Option<PathBuf>,
    /// [`TabMeta::graph_transcript`] for this pane's `agy` (session-overview-ui D24).
    pub graph_transcript: Option<PathBuf>,
}

/// The worker's per-tab graph decision ([`graph_pass`], session-flow-viewer R6.6 —
/// `pane_pass` there): whether this pass derives the tab's panes and repository identity.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanePass {
    /// Publish no panes and run no `list-panes` for this tab.
    Skip,
    /// Enumerate the tab's panes and derive each one.
    Derive,
}

/// What the worker publishes for the graph after a pass ([`publish_graph_state`],
/// session-flow-graph R4.7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GraphPublish {
    /// [`TabMetaSnapshot::graph_ready`]'s next value.
    pub ready: bool,
    /// Publish every `repo`, every pane list (and so every `PaneMeta`) empty.
    pub clear: bool,
}

/// A running agent: which one, and what it is doing. `Copy` — `Option<Agent>` on
/// `TabMeta` is what makes "no agent but Busy" unrepresentable (D8).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Agent {
    pub kind: AgentKind,
    pub state: AgentState,
}

/// The identity of *this tab's* `claude` process — the input that lets the card resolve its
/// own session instead of ranking the project directory (tabmeta-session-keying).
///
/// Lives here rather than in `agent` or `activity` because it is the interface *between*
/// them: `agent::agent_of_pid` supplies the pid, `worker` pairs it with the start time, and
/// `activity::resolve_log` consumes both.
///
/// `start_ms` is deliberately `Option`. `proc::start_ms_of_pid` returns `None` for a dead pid
/// or a start time in the future, and the two resolution tiers need it differently: the
/// registry tier cannot run without it (validating a record against an unknown start is not
/// validation), while the argv tier needs only the pid. So a missing start time must
/// *downgrade* the tier rather than discard the identity.
// Constructed by `worker::Deriver::tab`, consumed by `activity::resolve_log`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ClaudeId {
    pub(crate) pid: u32,
    pub(crate) start_ms: Option<u64>,
}

/// Agent activity. **No `Done`** — a finished agent is a process that exited, i.e.
/// `agent: None` (D8). `Unknown` asserts presence only: the consumer must render the
/// "agent present" affordance for it, never "ready".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentState {
    /// Mid-turn: the agent is actively working (a tool_use turn, a tool_result the agent is
    /// consuming, a fresh human prompt it is about to answer, or a recent append).
    Busy,
    /// The agent handed control back to the human — waiting for MY input (D4/D16 of
    /// `left-sidebar-waiting-area`). From the log it is produced by exactly two precise
    /// signals: the last meaningful message is an assistant `stop_reason == "end_turn"` (the
    /// turn is over), or it is an assistant turn calling a tool that blocks on a person
    /// (`ai_monitor::activity::LastMessage::AssistantAsksUser`).
    ///
    /// Only Antigravity reaches that second signal today, through its `ask_question`
    /// dispatch. Claude Code 2.1.28x writes a `tool_use` to the log only once the tool
    /// returns, so a pending `AskUserQuestion` or `ExitPlanMode` is never on disk: the log
    /// typically reads the previous tool's `tool_result`, which is `Busy`. A Claude question
    /// or plan dialog can reach `Waiting` only from the **screen**:
    /// `prompt::shows_blocking_prompt` sets the session's `blocking_dialog` level, and
    /// `sidebar_model::effective_state` reads `Waiting` while it is up, whatever this field
    /// says — that is what the card and the band read. Do not wire a new Claude feature to
    /// the log signal; under 2.1.28x it is dead.
    ///
    /// Never produced by the recency fallback — a false "waiting" is the worst failure. The
    /// waiting-band feature (T4) renders the input-waiting affordance for this; earlier
    /// features treat it as an attention state (see `sidebar_model::resolve_status`).
    Waiting,
    Idle,
    Unknown,
}

/// Which agent CLI is running. App-side vocabulary only — never a renderer type
/// (D11); `sidebar-row-redesign` owns the mapping from `AgentKind` to a renderer skin
/// with an exhaustive match (no `_` arm), so adding a variant is a compile error there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentKind {
    Claude,
    Antigravity,
    Codex,
    Gemini,
    Aider,
}

impl AgentKind {
    /// A printable identity for logging / future tooltips (not stringly-typed at the seam).
    pub fn label(self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude",
            AgentKind::Antigravity => "Antigravity",
            AgentKind::Codex => "Codex",
            AgentKind::Gemini => "Gemini",
            AgentKind::Aider => "Aider",
        }
    }
}

/// One live tab: its stable [`TabId`] and its PTY child pid (the walk root). The
/// roster carries pids, not cwds, so a `cd` needs no invalidation (D9).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TabPid {
    pub id: TabId,
    pub pid: u32,
}

// ---------------------------------------------------------------------------
// Shared cache (D1) + command channel.
// ---------------------------------------------------------------------------

/// The cross-thread cache: a monotonic generation plus the last-published map. A
/// `Mutex` (not `RwLock`) because there is no read contention — the UI thread takes
/// it at most once per generation change, the worker at most once per tick.
pub(crate) struct TabMetaShared {
    pub(crate) generation: AtomicU64,
    pub(crate) tabs: Mutex<Vec<(TabId, TabMeta)>>,
    /// Whether the graph's identity has been resolved for every tab since it was last wanted
    /// (session-flow-graph D10). Written only by the worker, before the generation bump it
    /// rides, so a reader that saw the new generation sees it too.
    pub(crate) graph_ready: AtomicBool,
    /// Which open of the graph `graph_ready` describes — the epoch the worker last adopted
    /// from a `SetGraphWanted { wanted: true, .. }` (session-flow-graph D10). Written with
    /// `graph_ready`, before the same bump. A reopen reads the previous open's readiness until
    /// the worker reaches its own command; this is what tells the two apart.
    pub(crate) graph_epoch: AtomicU64,
    /// The last epoch [`TabMetaHandle::set_graph_wanted`] handed out. UI-side only: the worker
    /// never reads it, it learns each epoch from the command that carries it.
    pub(crate) graph_opens: AtomicU64,
}

/// Commands the UI thread sends to the worker; they arrive on the channel the worker
/// blocks on, so `recv_timeout` doubles as the sleep and the inbox (D4).
#[allow(clippy::enum_variant_names)] // the `Set*` prefix mirrors the setter methods, intentionally
pub(crate) enum Cmd {
    SetRoster(Vec<TabPid>),
    SetActive(Option<TabId>),
    SetEnabled {
        git: bool,
        agent: bool,
    },
    /// The ai-monitor panel's one-shot deadline: wake the loop once at this epoch-millis
    /// instant, then forget it. `None` disarms (ai-monitor-idle-timestamps D6/D7/D8).
    ///
    /// A `Cmd` rather than a shared atomic because a deadline is the same *shape* of fact as
    /// the three above — something the app tells the worker — and the channel already
    /// guarantees the ordering an atomic would not: an arm that overtakes a disarm would
    /// leave a wake pending for a panel that had closed.
    SetPanelDeadline(Option<u64>),
    /// Whether the flow graph wants per-pane metadata and repository identity
    /// (session-flow-viewer D9, session-flow-graph D10). Off by default; while off the worker
    /// runs no `list-panes`, derives no pane and resolves no repository.
    ///
    /// `epoch` names this open (0 when `wanted` is false); the worker publishes it beside the
    /// readiness so the overlay can ignore a readiness left over from an earlier open.
    SetGraphWanted {
        wanted: bool,
        epoch: u64,
    },
}

// ---------------------------------------------------------------------------
// The handle (D2) — the contract sidebar-row-redesign programs against.
// ---------------------------------------------------------------------------

/// Handle to the metadata worker. `Clone` is load-bearing: the consumer clones it
/// into the `DrawLayer` that reads per frame (D2).
#[derive(Clone)]
pub struct TabMetaHandle {
    shared: Arc<TabMetaShared>,
    tx: Sender<Cmd>,
}

impl TabMetaHandle {
    /// Start the worker thread. `wake` runs ON THE WORKER THREAD once per tick in
    /// which at least one tab changed — keep it cheap (an `EventLoopProxy::send_event`).
    /// `None` if the thread cannot be spawned (the caller then simply has no metadata).
    pub fn spawn(wake: impl Fn() + Send + 'static) -> Option<TabMetaHandle> {
        let shared = Arc::new(TabMetaShared {
            generation: AtomicU64::new(0),
            tabs: Mutex::new(Vec::new()),
            graph_ready: AtomicBool::new(false),
            graph_epoch: AtomicU64::new(0),
            graph_opens: AtomicU64::new(0),
        });
        let (tx, rx) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("tabmeta".to_string())
            .spawn(move || worker::run(worker_shared, rx, wake))
            .ok()?;
        Some(TabMetaHandle { shared, tx })
    }

    /// Declare the live tab set. Cheap and idempotent — call it wherever the tab set
    /// can change (alongside `Tabs::titles()`), never per frame.
    pub fn set_roster(&self, roster: &[TabPid]) {
        let _ = self.tx.send(Cmd::SetRoster(roster.to_vec()));
    }

    /// Which tab is focused (it polls at the fast rate). The worker refreshes that tab
    /// IN BAND on receiving this, before returning to `recv_timeout`, so a switch never
    /// waits out a background interval; the global tick phase is NOT reset (D4).
    pub fn set_active(&self, id: Option<TabId>) {
        let _ = self.tx.send(Cmd::SetActive(id));
    }

    /// Display toggles (D12). Disabling a source stops the worker computing it and
    /// clears that field on the next publish. Both default to `true`.
    pub fn set_enabled(&self, git: bool, agent: bool) {
        let _ = self.tx.send(Cmd::SetEnabled { git, agent });
    }

    /// Arm the ai-monitor panel's one-shot wake at `at` (epoch millis), or disarm with `None`
    /// (ai-monitor-idle-timestamps D6/D7).
    ///
    /// **One slot, replaced on every arm.** The worker holds at most one deadline; arming
    /// over a pending one replaces it, which is the requirement rather than an optimisation —
    /// a session that resumes and re-quiets must not leave two deadlines racing. There is no
    /// generation token: the wake this produces carries no payload (`UserEvent::OverlayPending`
    /// is a unit variant), and it does not need one, because the pill is recomputed from the
    /// current snapshot against the current clock, so a late or spurious wake can only produce
    /// the right answer one rebuild early.
    ///
    /// **This is one edge, not a cadence.** The worker emits nothing on the other ticks of the
    /// wait, which is what keeps `ControlFlow::Wait` and the panel's idle-CPU guarantee intact.
    pub fn set_panel_deadline(&self, at: Option<u64>) {
        let _ = self.tx.send(Cmd::SetPanelDeadline(at));
    }

    /// Turn pane enumeration and repository identity on or off (session-flow-viewer D9, R6.6;
    /// session-flow-graph D10). The flow graph sends `true` on open and `false` on close, so an
    /// app with the graph closed spends nothing new (G1).
    ///
    /// On the off→on edge the worker derives **every** tab at once, ignoring the background
    /// stagger, so split tabs and groups appear within one tick of opening, and that pass sets
    /// [`TabMetaSnapshot::graph_ready`]. On the on→off edge it clears every published `panes`
    /// and `repo` and the readiness without deriving anything, so a reopen starts unresolved.
    ///
    /// Returns the epoch that names this open (0 for `false`). The readiness the worker
    /// publishes for it carries the same [`TabMetaSnapshot::graph_epoch`]; until a snapshot
    /// does, its readiness belongs to an earlier open, because the channel is FIFO and the
    /// worker may not have reached this command (or the close before it) yet.
    pub fn set_graph_wanted(&self, wanted: bool) -> u64 {
        let epoch = if wanted {
            self.shared.graph_opens.fetch_add(1, Ordering::Relaxed) + 1
        } else {
            0
        };
        let _ = self.tx.send(Cmd::SetGraphWanted { wanted, epoch });
        epoch
    }

    /// A handle with no worker behind it, for tests in sibling modules: the test holds the
    /// command inbox and plays the worker through [`publish_for_test`](Self::publish_for_test),
    /// so it can order a publish against queued commands — which a live worker never lets a
    /// test do (session-flow-graph D10's reopen race lives in exactly that gap).
    #[cfg(test)]
    pub(crate) fn detached() -> (TabMetaHandle, mpsc::Receiver<Cmd>) {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(TabMetaShared {
            generation: AtomicU64::new(0),
            tabs: Mutex::new(Vec::new()),
            graph_ready: AtomicBool::new(false),
            graph_epoch: AtomicU64::new(0),
            graph_opens: AtomicU64::new(0),
        });
        (TabMetaHandle { shared, tx }, rx)
    }

    /// Publish as the worker would — tabs, readiness and epoch, then the bump.
    #[cfg(test)]
    pub(crate) fn publish_for_test(&self, tabs: Vec<(TabId, TabMeta)>, ready: bool, epoch: u64) {
        *self.shared.tabs.lock().expect("tabs lock") = tabs;
        self.shared.graph_ready.store(ready, Ordering::Release);
        self.shared.graph_epoch.store(epoch, Ordering::Release);
        self.shared.generation.fetch_add(1, Ordering::Release);
    }

    /// The current publication generation. Monotonic; starts at 0; bumped only when a
    /// publish actually changed something (D4).
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }

    /// Refill `out` iff the generation moved since it was last filled. Returns `true`
    /// when `out` was rewritten. On `false` (the hot path) this takes NO lock beyond a
    /// single `Acquire` load and performs NO allocation — safe to call every frame (D2).
    pub fn sync(&self, out: &mut TabMetaSnapshot) -> bool {
        let g = self.shared.generation.load(Ordering::Acquire);
        if g == out.generation {
            return false;
        }
        let tabs = match self.shared.tabs.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        out.tabs.clear();
        out.tabs.extend(tabs.iter().cloned());
        out.graph_ready = self.shared.graph_ready.load(Ordering::Acquire);
        out.graph_epoch = self.shared.graph_epoch.load(Ordering::Acquire);
        out.generation = g;
        true
    }
}

/// A caller-owned snapshot of the per-tab metadata, kept across frames so `sync`'s
/// refill reuses its allocation (D2).
#[derive(Default)]
pub struct TabMetaSnapshot {
    generation: u64,
    tabs: Vec<(TabId, TabMeta)>,
    graph_ready: bool,
    graph_epoch: u64,
}

impl TabMetaSnapshot {
    /// A snapshot standing in for one the worker published, for tests in sibling
    /// modules.
    ///
    /// `tabs` is private to this module, so without this the only reachable snapshot
    /// is the empty `default()` — and against an empty one every consumer correctly
    /// answers `None`, which makes "always answer `None`" indistinguishable from the
    /// real implementation. Mutation testing found exactly that: `Tabs::active_cwd`
    /// and `Tabs::active_agent_pid` could both be replaced by `None` with the whole
    /// suite green.
    #[cfg(test)]
    pub(crate) fn from_entries(entries: Vec<(TabId, TabMeta)>) -> TabMetaSnapshot {
        TabMetaSnapshot::for_test(entries, false)
    }

    /// A published snapshot for the crate's acceptance tests (session-flow-graph R7.2): the
    /// same stand-in as `from_entries`, with the graph readiness stated. Generation 1, as a
    /// first publish would be.
    pub fn for_test(entries: Vec<(TabId, TabMeta)>, graph_ready: bool) -> TabMetaSnapshot {
        TabMetaSnapshot {
            generation: 1,
            tabs: entries,
            graph_ready,
            graph_epoch: 0,
        }
    }

    /// Whether the worker has resolved every tab's repository identity since the graph was
    /// last wanted (session-flow-graph D10, R7.2). Until it has, a `repo: None` means "not
    /// resolved yet" rather than "no repository", and the graph reads `resolving…`.
    pub fn graph_ready(&self) -> bool {
        self.graph_ready
    }

    /// The open of the graph [`graph_ready`](Self::graph_ready) belongs to — the epoch
    /// [`TabMetaHandle::set_graph_wanted`] returned for it (session-flow-graph D10). 0 until
    /// the graph was first wanted, and in a `for_test` stand-in.
    pub fn graph_epoch(&self) -> u64 {
        self.graph_epoch
    }

    /// This tab's metadata, if present in the last publish.
    pub fn get(&self, id: TabId) -> Option<&TabMeta> {
        self.tabs.iter().find(|(i, _)| *i == id).map(|(_, m)| m)
    }

    /// Iterate `(TabId, &TabMeta)` in publish order.
    pub fn iter(&self) -> impl Iterator<Item = (TabId, &TabMeta)> {
        self.tabs.iter().map(|(i, m)| (*i, m))
    }

    /// True when nothing has been published (or the roster is empty).
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// The publication generation this snapshot was filled at (card-live-cwd D5/R3.4).
    ///
    /// Exposed so a consumer can tell "the metadata moved" **level-triggered**, by comparing
    /// against a generation it stored, rather than edge-triggered on [`TabMetaHandle::sync`]'s
    /// `bool` — which another caller's `sync` can consume first (`tmux-pane-cwd` D6 records
    /// that trap: `displayed_label` syncs this same snapshot on a keystroke).
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

// ---------------------------------------------------------------------------
// The pump (D3) — a headless Overlay that turns a worker wake into a poll+redraw.
// ---------------------------------------------------------------------------

/// A permanently-invisible `Overlay` registered from `main.rs`. It never receives a
/// key; its `poll()` (called on `OverlayPending`) reports whether the worker bumped
/// the generation, so the UI redraws exactly when metadata changed (D3).
pub struct TabMetaPump {
    handle: TabMetaHandle,
    seen: u64,
}

impl TabMetaPump {
    pub fn new(handle: TabMetaHandle) -> TabMetaPump {
        TabMetaPump { handle, seen: 0 }
    }
}

impl Overlay for TabMetaPump {
    fn priority(&self) -> i32 {
        i32::MIN // never ahead of a real overlay
    }
    fn visible(&self) -> bool {
        false // headless — never receives a key
    }
    fn on_key(&mut self, _input: &KeyInput, _ctx: &mut AppCtx) -> Handled {
        Handled::Passthrough
    }
    fn layer(&self) -> Box<dyn DrawLayer> {
        Box::new(NoOpLayer)
    }
    fn poll(&mut self) -> bool {
        let g = self.handle.generation();
        let changed = g != self.seen;
        self.seen = g;
        changed
    }
}

/// The pump's draw layer: draws nothing, sits behind everything.
struct NoOpLayer;

impl DrawLayer for NoOpLayer {
    fn z_order(&self) -> i32 {
        i32::MIN
    }
    fn record(&mut self, _ctx: &mut FrameCtx<'_>) {}
}

// ---------------------------------------------------------------------------
// Locator (D2) — only for the two methods that must fire from inside `Tabs`.
// ---------------------------------------------------------------------------

static HANDLE: OnceLock<TabMetaHandle> = OnceLock::new();

/// Install the process-global handle. Called once, from `main.rs`. A locator only —
/// `set_roster`/`set_active` reach it from `tabs.rs`, which cannot be handed a handle
/// by construction (its constructor is out of surface, D2). Removable once
/// `sidebar-interaction` owns `terminal_app.rs`.
pub fn install(handle: TabMetaHandle) {
    let _ = HANDLE.set(handle);
}

/// The installed handle, if any. Returns `None` before `install` (e.g. in tests,
/// which never touch the global — every test builds its own handle).
pub fn global() -> Option<&'static TabMetaHandle> {
    HANDLE.get()
}

// ---------------------------------------------------------------------------
// Cost probes (D13(6)) — the full counter set, declared here so the data-source
// modules never depend on each other to observe their own cost. Test-only.
// ---------------------------------------------------------------------------

// The full counter set is declared here (D13(6)) so the T2–T5 data-source modules
// never depend on each other to observe their own cost; each is incremented as its
// module lands, and read by T6's cost tests.
#[cfg(test)]
pub(crate) mod probe {
    use std::sync::atomic::{AtomicU64, Ordering};

    /// One syscall/read counter.
    pub(crate) struct Counter(AtomicU64);
    impl Counter {
        pub(crate) const fn new() -> Counter {
            Counter(AtomicU64::new(0))
        }
        pub(crate) fn bump(&self, n: u64) {
            self.0.fetch_add(n, Ordering::Relaxed);
        }
        pub(crate) fn get(&self) -> u64 {
            self.0.load(Ordering::Relaxed)
        }
        pub(crate) fn reset(&self) {
            self.0.store(0, Ordering::Relaxed);
        }
    }

    // proc.rs (T2) — the four raw syscall entry points (D13(6)).
    pub(crate) static PROC_CWD: Counter = Counter::new();
    pub(crate) static PROC_CHILDREN: Counter = Counter::new();
    pub(crate) static PROC_EXE: Counter = Counter::new();
    pub(crate) static PROC_ARGS: Counter = Counter::new();
    /// proc.rs (tabmeta-session-keying) — `start_ms_of_pid`, the process-start read that
    /// `ai_monitor::validated_session_id` needs.
    ///
    /// **Assert a FLOOR or a DELTA on this, never an absolute equality.**
    /// `tabsession::start_ms` delegates to the same function, and `tabsession`'s tests do not
    /// hold [`serial()`] — deliberately: they are pre-existing ai-monitor panel tests, and
    /// making them cooperate with a sidebar counter would be this feature reaching into the
    /// panel's suite to protect its own assertions. So the counter can be bumped by a test
    /// that is not coordinating with the lock, and only a floor is sound.
    ///
    /// (The `PROC_EXE` note in `worker.rs` reaches the same remedy from a different cause —
    /// there the nondeterminism is a varying live *child count*, not an uncoordinated caller.)
    pub(crate) static PROC_START: Counter = Counter::new();
    /// proc.rs (agent-classifier-deleted-binary) — `name_of_pid`, the exec-time process name
    /// the classifier falls back to when `proc_pidpath` reports nothing.
    ///
    /// Counted because it is per-pid-per-walk and, like every other read here, uncounted I/O
    /// is what lets a cost test keep passing while the real cost grows. An **absolute**
    /// assertion on it is sound only where the pid population is fixed by the test: the
    /// classifier reaches this read solely on the `exe_path == None` branch, and the count of
    /// pids in that state under a live walk is exactly as nondeterministic as the live-child
    /// count that forced `worker.rs`'s `PROC_EXE` assertion to a floor. Note also that
    /// `tabsession::agent_for` — outside `tabmeta` — reaches `classify_pid`; no default-suite
    /// test drives it today (`presence_fd`'s live gate is `#[ignore]`d, and the overlay paths
    /// need a winit loop), but that is a weaker guarantee than sole ownership.
    pub(crate) static PROC_NAME: Counter = Counter::new();
    /// activity.rs (tabmeta-session-keying) — one `~/.claude/sessions/<pid>.json` read.
    /// Counted because it is per-Claude-tab-per-refresh and uncounted I/O is exactly what
    /// lets a cost test keep passing while the real cost grows.
    pub(crate) static SESSION_REG: Counter = Counter::new();
    // git.rs (T4) — the .git filesystem entry points (D13(6)).
    pub(crate) static GIT_STAT: Counter = Counter::new();
    pub(crate) static GIT_READ: Counter = Counter::new();
    pub(crate) static GIT_READ_BYTES: Counter = Counter::new();
    // proc.rs / tmux.rs (tmux-root-resolution) — the tty join + the client-map spawn.
    // `TTY_STAT` is the third counter that feature's D15 adds beyond SD6's sketched two:
    // it covers the *tmux* side of the join (one `stat` per client tty), which `PROC_TTY`
    // (the tab's side) does not, and which would otherwise be uncounted cost.
    pub(crate) static PROC_TTY: Counter = Counter::new();
    pub(crate) static TTY_STAT: Counter = Counter::new();
    pub(crate) static TMUX_LIST: Counter = Counter::new();
    /// tmux.rs (session-flow-viewer D9) — one `list-panes -a` spawn attempt.
    pub(crate) static TMUX_PANES: Counter = Counter::new();
    // activity.rs (T5) — the log-resolution + tail-read entry points (D13(6)).
    pub(crate) static LOG_RESOLVE: Counter = Counter::new();
    pub(crate) static LOG_STAT: Counter = Counter::new();
    pub(crate) static LOG_READ: Counter = Counter::new();
    pub(crate) static LOG_READ_BYTES: Counter = Counter::new();

    /// Reset every counter (call at the start of a cost test).
    pub(crate) fn reset_all() {
        for c in [
            &PROC_CWD,
            &PROC_CHILDREN,
            &PROC_EXE,
            &PROC_ARGS,
            &PROC_START,
            &PROC_NAME,
            &SESSION_REG,
            &PROC_TTY,
            &TTY_STAT,
            &TMUX_LIST,
            &TMUX_PANES,
            &GIT_STAT,
            &GIT_READ,
            &GIT_READ_BYTES,
            &LOG_RESOLVE,
            &LOG_STAT,
            &LOG_READ,
            &LOG_READ_BYTES,
        ] {
            c.reset();
        }
    }

    /// The counters are process-global statics, but `cargo test` runs tests in
    /// parallel — so any test that resets/asserts a counter, **or** whose code path
    /// bumps one, must hold this lock for its duration. Serialising them is what makes
    /// the D13(6) counter-delta assertions deterministic. Poison is ignored: a panicking
    /// (failing) cost test must not wedge the rest.
    pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_starts_at_zero() {
        let h = TabMetaHandle::spawn(|| {}).expect("spawn worker");
        assert_eq!(h.generation(), 0);
    }

    #[test]
    fn sync_hot_path_is_false_and_allocation_free() {
        // R1.2 — with nothing published, sync returns false, leaves `out` empty, and
        // does not grow its Vec (the property that makes per-frame calls safe).
        let h = TabMetaHandle::spawn(|| {}).expect("spawn worker");
        let mut snap = TabMetaSnapshot::default();
        assert!(!h.sync(&mut snap), "first sync: nothing published yet");
        let cap = snap.tabs.capacity();
        assert!(!h.sync(&mut snap), "second sync: still unchanged");
        assert_eq!(
            snap.tabs.capacity(),
            cap,
            "no reallocation on the false path"
        );
        assert!(snap.is_empty());
    }

    #[test]
    fn sync_copies_the_graph_readiness_with_the_tabs() {
        // session-flow-graph R7.2's seam: the readiness reaches a snapshot by `sync`, under the
        // same generation as the tabs it describes, and `for_test` states it directly.
        let (tx, _rx) = mpsc::channel();
        let h = TabMetaHandle {
            shared: Arc::new(TabMetaShared {
                generation: AtomicU64::new(0),
                tabs: Mutex::new(Vec::new()),
                graph_ready: AtomicBool::new(false),
                graph_epoch: AtomicU64::new(0),
                graph_opens: AtomicU64::new(0),
            }),
            tx,
        };
        let mut snap = TabMetaSnapshot::default();
        assert!(!snap.graph_ready());
        h.shared.graph_ready.store(true, Ordering::Release);
        h.shared.generation.store(1, Ordering::Release);
        assert!(h.sync(&mut snap));
        assert!(snap.graph_ready());
        h.shared.graph_ready.store(false, Ordering::Release);
        h.shared.generation.store(2, Ordering::Release);
        assert!(h.sync(&mut snap));
        assert!(!snap.graph_ready());

        let t = TabMetaSnapshot::for_test(vec![(TabId(4), TabMeta::default())], true);
        assert!(t.graph_ready());
        assert_eq!(t.generation(), 1);
        assert!(t.get(TabId(4)).is_some());
        assert!(!TabMetaSnapshot::for_test(Vec::new(), false).graph_ready());
    }

    #[test]
    fn wanting_the_graph_through_the_handle_reaches_the_worker() {
        // The one line between the overlay's open and the worker's edge: a no-op
        // `set_graph_wanted` would leave the graph reading `resolving…` forever.
        let (woke_tx, woke_rx) = mpsc::channel::<()>();
        let h = TabMetaHandle::spawn(move || {
            let _ = woke_tx.send(());
        })
        .expect("spawn worker");
        let mut snap = TabMetaSnapshot::default();
        h.set_graph_wanted(true);
        woke_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("opening the graph publishes its readiness");
        assert!(h.sync(&mut snap));
        assert!(snap.graph_ready());
        h.set_graph_wanted(false);
        woke_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("closing the graph publishes the cleared readiness");
        assert!(h.sync(&mut snap));
        assert!(!snap.graph_ready());
    }

    #[test]
    fn snapshot_get_iter_lookup() {
        // Construct a snapshot directly (fields are module-private, so this in-module
        // test is the only place that can) to exercise get/iter/is_empty.
        let a = TabId(1);
        let b = TabId(2);
        let snap = TabMetaSnapshot {
            generation: 3,
            graph_ready: false,
            graph_epoch: 0,
            tabs: vec![
                (
                    a,
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
                ),
                (
                    b,
                    TabMeta {
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
                    },
                ),
            ],
        };
        assert_eq!(snap.get(a).and_then(|m| m.branch.as_deref()), Some("main"));
        assert_eq!(
            snap.get(b).and_then(|m| m.agent).map(|ag| ag.kind),
            Some(AgentKind::Claude)
        );
        assert!(snap.get(TabId(99)).is_none());
        assert_eq!(snap.iter().count(), 2);
        assert!(!snap.is_empty());
    }

    #[test]
    fn agent_kind_labels_are_distinct_and_nonempty() {
        let all = [
            AgentKind::Claude,
            AgentKind::Antigravity,
            AgentKind::Codex,
            AgentKind::Gemini,
            AgentKind::Aider,
        ];
        for k in all {
            assert!(!k.label().is_empty());
        }
        // labels are distinct
        let labels: Vec<_> = all.iter().map(|k| k.label()).collect();
        for (i, a) in labels.iter().enumerate() {
            for b in &labels[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn arming_through_the_handle_reaches_the_worker_and_wakes_the_loop() {
        // The seam the panel actually arms through, end to end: handle → channel → worker →
        // wake. Everything on either side of this method was already covered — the overlay's
        // decision to arm, and the worker's loop firing a `Cmd::SetPanelDeadline` sent
        // straight down a test's own channel — and this one line between them was not.
        //
        // `cargo mutants` is how that was found, and what it reported is the reason this test
        // exists rather than a comment: replacing the body with `()` left all 1602 tests
        // green. A no-op here sends no command, so nothing is ever armed, no wake is ever
        // produced, and the pill never leaves CONNECTED — the whole feature inert, silently.
        // It is the same shape as the `run` survivor recorded in `mutants-scope.txt`, reached
        // the same way, and it escaped the scope guard for a reason now written down there:
        // a diff hunk inside an `impl` block is attributed to the `impl`, not to the method.
        let (woke_tx, woke_rx) = mpsc::channel::<()>();
        let h = TabMetaHandle::spawn(move || {
            let _ = woke_tx.send(());
        })
        .expect("spawn worker");

        // Epoch-millis 1 is long past due, so the worker's very next pass fires it.
        h.set_panel_deadline(Some(1));

        woke_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect(
                "arming through the handle must wake the loop — if this times out, the panel's \
                 route to the worker is broken and a quiet session's pill will read CONNECTED \
                 forever",
            );
    }

    #[test]
    fn the_three_setters_each_send_their_command_to_the_worker() {
        // `set_roster`, `set_active` and `set_enabled` are the whole of how the app tells the
        // worker what to watch; each is one `send`, and a no-op body left every test green
        // (cargo mutants). The detached handle holds the inbox, so the command itself is the
        // observable — no worker timing involved.
        let (h, rx) = TabMetaHandle::detached();
        let roster = [TabPid {
            id: TabId(7),
            pid: 4242,
        }];
        h.set_roster(&roster);
        h.set_active(Some(TabId(7)));
        h.set_enabled(false, true);

        match rx.try_recv() {
            Ok(Cmd::SetRoster(r)) => assert_eq!(r, roster.to_vec()),
            _ => panic!("set_roster sent no SetRoster"),
        }
        assert!(
            matches!(rx.try_recv(), Ok(Cmd::SetActive(Some(TabId(7))))),
            "set_active sent no SetActive(Some(7))"
        );
        assert!(
            matches!(
                rx.try_recv(),
                Ok(Cmd::SetEnabled {
                    git: false,
                    agent: true
                })
            ),
            "set_enabled sent no SetEnabled {{ git: false, agent: true }}"
        );
        assert!(rx.try_recv().is_err(), "one command per call");
    }

    #[test]
    fn the_pump_reports_a_change_exactly_once_per_generation_bump() {
        // D3: the pump's `poll` is the redraw trigger for metadata. It must say `true` on the
        // wake after a publish and `false` on every other wake (tabmeta and the ai-monitor
        // share the `OverlayPending` event, so most wakes are not this worker's).
        let (h, _rx) = TabMetaHandle::detached();
        let mut pump = TabMetaPump::new(h.clone());
        assert!(!pump.poll(), "nothing published: no redraw");
        h.publish_for_test(Vec::new(), false, 0);
        assert!(pump.poll(), "a publish bumped the generation: redraw");
        assert!(!pump.poll(), "the same generation again: no second redraw");
        h.publish_for_test(Vec::new(), false, 0);
        assert!(pump.poll(), "the next publish is seen too");
    }

    #[test]
    fn a_synced_snapshot_reports_the_generation_it_was_filled_at() {
        // card-live-cwd D5: consumers (the waiting band, the flow graph's rebuild key) detect
        // "metadata moved" by comparing this against a stored value, so it must move with
        // every publish — a constant would freeze the band after the first one.
        let (h, _rx) = TabMetaHandle::detached();
        let mut snap = TabMetaSnapshot::default();
        assert_eq!(snap.generation(), 0, "never filled");
        h.publish_for_test(Vec::new(), false, 0);
        assert!(h.sync(&mut snap));
        assert_eq!(snap.generation(), 1);
        h.publish_for_test(Vec::new(), false, 0);
        assert!(h.sync(&mut snap));
        assert_eq!(snap.generation(), 2, "the second publish moved it again");
    }

    #[test]
    fn the_pump_is_headless_and_behind_everything() {
        // D3: permanently invisible (it owns no pixels, so a press or a covering route must
        // never be offered to it) and its layer sorts behind every real layer.
        let (h, _rx) = TabMetaHandle::detached();
        let pump = TabMetaPump::new(h);
        assert!(!pump.visible());
        assert_eq!(pump.priority(), i32::MIN);
        assert_eq!(pump.layer().z_order(), i32::MIN);
    }

    /// Set only in the child process [`the_locator_returns_the_installed_handle`] spawns.
    const LOCATOR_CHILD: &str = "MAC_TERMINAL_TABMETA_LOCATOR_CHILD";

    /// The child half: installs into the process-global `OnceLock`. A no-op unless
    /// [`LOCATOR_CHILD`] is set, because installing in the shared test process would hand
    /// every later test a handle — `feature_overlays` builds the flow overlay from
    /// `global()`, and its tests rely on that being `None`.
    #[test]
    fn locator_child_installs_and_finds_the_handle() {
        if std::env::var_os(LOCATOR_CHILD).is_none() {
            return;
        }
        assert!(global().is_none(), "nothing is installed before install");
        let (h, _rx) = TabMetaHandle::detached();
        let mine = Arc::clone(&h.shared);
        install(h);
        let found = global().expect("install must make the handle reachable");
        assert!(
            Arc::ptr_eq(&found.shared, &mine),
            "global returns the installed handle"
        );
    }

    #[test]
    fn the_locator_returns_the_installed_handle() {
        // `install`/`global` are how `main.rs` hands `tabs.rs` the handle every roster change
        // goes through; a no-op `install` or a `global` that always answers `None` left the
        // suite green (cargo mutants) while the sidebar's metadata silently stopped. The
        // global is a `OnceLock`, so the check runs in a child process of this test binary.
        let out = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "tabmeta::tests::locator_child_installs_and_finds_the_handle",
                "--test-threads=1",
            ])
            .env(LOCATOR_CHILD, "1")
            .output()
            .expect("run the child test");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "child failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            stdout.contains("1 passed"),
            "the child test did not run:\n{stdout}"
        );
    }

    #[test]
    fn default_tabmeta_is_blank() {
        assert_eq!(
            TabMeta::default(),
            TabMeta {
                graph_agent_cwd: None,
                graph_transcript: None,
                graph_asks_user: false,
                branch: None,
                agent: None,
                task: None,
                cwd: None,
                agent_pid: None,
                panes: Vec::new(),
                log: None,
                repo: None,
                graph_agent: None,
                graph_task: None,
            }
        );
    }
}
