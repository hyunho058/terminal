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
mod agent;
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

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
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
// Variants are constructed by the T3 classifier / T5 activity rule and consumed by
// sidebar-row-redesign; landed here ahead of them (like `register_overlay`).
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentState {
    /// Mid-turn: the agent is actively working (a tool_use turn, a tool_result the agent is
    /// consuming, a fresh human prompt it is about to answer, or a recent append).
    Busy,
    /// The agent handed control back to the human — waiting for MY input (D4/D16 of
    /// `left-sidebar-waiting-area`). Produced by exactly two precise log signals: the last
    /// meaningful message is an assistant `stop_reason == "end_turn"` (the turn is over), or
    /// it is an assistant turn calling a tool that blocks on a person, such as
    /// `AskUserQuestion` (`ai_monitor::activity::LastMessage::AssistantAsksUser`). Never
    /// produced by the recency fallback — a false "waiting" is the worst failure. The
    /// waiting-band feature (T4) renders the input-waiting affordance for this; earlier
    /// features treat it as an attention state (see `sidebar_model::mark_for`).
    Waiting,
    Idle,
    Unknown,
}

/// Which agent CLI is running. App-side vocabulary only — never a renderer type
/// (D11); `sidebar-row-redesign` owns the mapping from `AgentKind` to a renderer skin
/// with an exhaustive match (no `_` arm), so adding a variant is a compile error there.
#[allow(dead_code)] // variants constructed by the T3 classifier; consumed by feature 3
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
    #[allow(dead_code)] // used by T6 logging + feature 3 tooltips
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
}

/// Commands the UI thread sends to the worker; they arrive on the channel the worker
/// blocks on, so `recv_timeout` doubles as the sleep and the inbox (D4).
#[allow(dead_code)] // constructed by the handle setters, consumed by feature 3 / sidebar-interaction
#[allow(clippy::enum_variant_names)] // the `Set*` prefix mirrors the setter methods, intentionally
pub(crate) enum Cmd {
    SetRoster(Vec<TabPid>),
    SetActive(Option<TabId>),
    SetEnabled { git: bool, agent: bool },
}

// ---------------------------------------------------------------------------
// The handle (D2) — the contract sidebar-row-redesign programs against.
// ---------------------------------------------------------------------------

/// Handle to the metadata worker. `Clone` is load-bearing: the consumer clones it
/// into the `DrawLayer` that reads per frame (D2).
#[derive(Clone)]
pub struct TabMetaHandle {
    shared: Arc<TabMetaShared>,
    #[allow(dead_code)] // read by the setters below, whose consumer is feature 3
    tx: Sender<Cmd>,
}

// The read/command API is landed here for `sidebar-row-redesign` (roster/active/sync)
// and `sidebar-interaction` (set_enabled live reload); unused until they land.
#[allow(dead_code)]
impl TabMetaHandle {
    /// Start the worker thread. `wake` runs ON THE WORKER THREAD once per tick in
    /// which at least one tab changed — keep it cheap (an `EventLoopProxy::send_event`).
    /// `None` if the thread cannot be spawned (the caller then simply has no metadata).
    pub fn spawn(wake: impl Fn() + Send + 'static) -> Option<TabMetaHandle> {
        let shared = Arc::new(TabMetaShared {
            generation: AtomicU64::new(0),
            tabs: Mutex::new(Vec::new()),
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
}

// Read by sidebar-row-redesign's DrawLayer; landed ahead of it.
#[allow(dead_code)]
impl TabMetaSnapshot {
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
#[allow(dead_code)] // called from tabs.rs (sidebar-row-redesign) to push roster/active
pub fn global() -> Option<&'static TabMetaHandle> {
    HANDLE.get()
}

// ---------------------------------------------------------------------------
// Cost probes (D13(6)) — the full counter set, declared here so the data-source
// modules never depend on each other to observe their own cost. Test-only.
// ---------------------------------------------------------------------------

// The full counter set is declared here (D13(6)) so the T2–T5 data-source modules
// never depend on each other to observe their own cost; each is incremented as its
// module lands, and read by T6's cost tests — unused until then.
#[cfg(test)]
#[allow(dead_code)]
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
            &SESSION_REG,
            &PROC_TTY,
            &TTY_STAT,
            &TMUX_LIST,
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
    fn snapshot_get_iter_lookup() {
        // Construct a snapshot directly (fields are module-private, so this in-module
        // test is the only place that can) to exercise get/iter/is_empty.
        let a = TabId(1);
        let b = TabId(2);
        let snap = TabMetaSnapshot {
            generation: 3,
            tabs: vec![
                (
                    a,
                    TabMeta {
                        branch: Some("main".into()),
                        agent: None,
                        task: None,
                        cwd: None,
                    },
                ),
                (
                    b,
                    TabMeta {
                        branch: None,
                        agent: Some(Agent {
                            kind: AgentKind::Claude,
                            state: AgentState::Busy,
                        }),
                        task: None,
                        cwd: None,
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
    fn default_tabmeta_is_blank() {
        assert_eq!(
            TabMeta::default(),
            TabMeta {
                branch: None,
                agent: None,
                task: None,
                cwd: None
            }
        );
    }
}
