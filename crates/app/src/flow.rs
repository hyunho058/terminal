//! The session flow graph (session-flow-graph): a Cmd+Shift+F full-window overlay showing
//! this window's agent sessions as the sidebar's own cards, in three columns joined by
//! curved edges — project → session → subagent — with a header counting sessions by state.
//! Registered through the SD4 `Overlay` seam and drawn by the renderer's `graph_panel` over
//! the whole viewport, the ai-monitor included (D4).
//!
//! Two halves, kept apart on purpose:
//!
//! * [`build_graph`] is the whole graph policy, pure, so every requirement about grouping,
//!   order, status, count and fold is a unit-testable function of plain data (D17).
//! * [`FlowOverlay`] is the impure shell: the open/close edges, the graph request to the
//!   tabmeta worker, the subagent thread, the key, press, wheel and hover routes, and the
//!   per-frame "did the key move" check that decides whether `build_graph` runs at all (D11).
//!
//! **Status is the card's, by construction** (old D2, Constraint). A tab's session — or a
//! split tab's *active* pane's — takes its whole card from
//! [`row_meta`](crate::sidebar_model::row_meta), the very join the sidebar card renders
//! from, fed by the same metadata snapshot and the same
//! `SidebarRow.blocking_dialog`. A non-active pane takes [`resolve_status`] on its own
//! log-derived agent with no dialog (old D13). There is no third path: the graph never
//! classifies anything itself.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ai_monitor::telemetry::Subagent;
use renderer::graph_layout::{FlowGraph, FlowNode, FlowTarget, NodeKind};
use renderer::graph_panel::{FlowHandle, FlowHit, FlowPanel};
use renderer::sidebar_card::CardStatus;
use renderer::sidebar_row::{SidebarRow, TabModelHandle};
use renderer::{DrawLayer, FrameCtx};
use winit::keyboard::{Key, NamedKey};

use crate::flow_telemetry::{self, FlowTelemetry, SubagentSlot};
use crate::input::{self, WheelDelta};
use crate::seam::{AppCtx, Handled, KeyInput, Overlay, OverlayCommand};
use crate::session::TabId;
use crate::sidebar_model::{display_cwd, folder_name, resolve_status, row_meta_with, skin_for};
use crate::tabmeta::{Agent, AgentKind, RepoId, TabMeta, TabMetaHandle, TabMetaSnapshot};

/// The sessions whose finished-subagent list is expanded, by session identity
/// `(tab, pane pid)` (D8) — never by log path, so two sessions sharing one log fold
/// independently (old D18).
///
/// **An unsplit tab's session is keyed `(tab, 0)`.** Old D8 names "the tab's effective root
/// pid" for that case, but no published field carries it: `TabMeta.agent_pid` is the
/// detected *agent* process (it changes on every agent restart, which would silently unfold the
/// list), and `panes` is empty for an unsplit tab by definition (old D9). A tab has exactly
/// one session while unsplit, so the constant is as unique as the pid would have been; `0`
/// is never a real pane pid, so it cannot collide with a split tab's keys either. A tab that
/// becomes split gets pane-pid keys and its `(tab, 0)` entry is pruned like any vanished
/// session.
pub type FoldSet = HashSet<(TabId, u32)>;

/// The fold key's pane part for an unsplit tab's one session — see [`FoldSet`].
const UNSPLIT_PANE: u32 = 0;

/// One agent session, its card already resolved, before it is placed in the graph.
struct Session<'a> {
    tab: u64,
    /// The fold key's pane part: the pane pid, or [`UNSPLIT_PANE`].
    pane: u32,
    /// Where it groups (D10): `None` is the non-git tail.
    repo: Option<&'a RepoId>,
    /// The log its subagents are read from — only ever `Some` for a **Claude** session
    /// (R14.3, D8): subagents come from Claude's session log, which the other agents do not
    /// write, so a non-Claude session has no column-2 children even when its tab publishes a
    /// log path, and the subagent thread never follows that path.
    log: Option<&'a Path>,
    /// The session's card, `parent` and `extra` still unset.
    card: FlowNode,
}

/// Whether `agent` makes a session: any detected agent, of any [`AgentKind`] (R14, the
/// 2026-10-21 amendment to D1). Only "no agent" is not a session.
fn is_session(agent: Option<&Agent>) -> bool {
    agent.is_some()
}

/// The log a session with `agent` reads subagents from: `log` for a Claude session, `None`
/// for any other kind (R14.3 — see [`Session::log`]).
fn subagent_log<'a>(agent: Option<&Agent>, log: Option<&'a Path>) -> Option<&'a Path> {
    log.filter(|_| agent.is_some_and(|a| a.kind == AgentKind::Claude))
}

/// A tab's agent sessions in strip-then-pane order (D1, D3, old D5/D9/D13, R14).
///
/// * **Unsplit** (`panes` empty): one session iff the tab has a detected agent (any kind),
///   its card the sidebar card's `RowMeta` exactly.
/// * **Split**: the active pane has a session iff the tab has an agent (`graph_agent`, else
///   `agent`; old D9: its `PaneMeta.agent` is a copy that is never read here) and takes
///   the tab's card whole. A non-active pane has one iff it has its own agent; it takes
///   the tab's line 1, its own status — log-only,
///   `resolve_status(agent, false, false, None)`, exactly old D13's expression, whatever
///   the kind — and the folder and branch of its own cwd. Its skin is its own agent's
///   (D3), so a Codex pane beside a Claude active pane reads as Codex, and a pane agent
///   beside a plain-shell active pane keeps its own skin rather than the generic.
///
/// Only a Claude session carries a subagent log ([`subagent_log`]).
///
/// `selected` marks the strip's active tab: its unsplit session, or its active pane's.
fn sessions_of<'a>(
    row: &SidebarRow,
    meta: Option<&'a TabMeta>,
    home: Option<&Path>,
    selected: bool,
) -> Vec<Session<'a>> {
    let Some(m) = meta else {
        // Old D18: a row with no meta is a tab with no known session.
        return Vec::new();
    };
    let tab = row.tab_id;
    // The graph ignores `[sidebar].show_agent_status` (user decision 2026-10-22): the worker
    // publishes the toggle-free agent and label as `graph_agent`/`graph_task` on every graph
    // pass, and `agent`/`task` are the fallback for a meta that carries only the card's copy.
    // The card — status included — is still the sidebar's own join, fed that pair.
    let (agent, task) = match m.graph_agent {
        Some(_) => (m.graph_agent.as_ref(), m.graph_task.as_deref()),
        None => (m.agent.as_ref(), m.task.as_deref()),
    };
    let rm = row_meta_with(row, Some(m), agent, task, home);
    // `FlowNode::default()`'s kind is `Session`.
    let card = FlowNode {
        title: rm.line1.clone(),
        folder: rm.cwd_display.clone(),
        branch: rm.branch.clone(),
        skin: rm.skin,
        status: rm.status,
        selected,
        target: FlowTarget::Node(tab),
        ..FlowNode::default()
    };
    let tab_session = |pane: u32, log: Option<&'a Path>| Session {
        tab,
        pane,
        repo: m.repo.as_ref(),
        log: subagent_log(agent, log),
        card: card.clone(),
    };
    if m.panes.is_empty() {
        return match is_session(agent) {
            true => vec![tab_session(UNSPLIT_PANE, m.log.as_deref())],
            false => Vec::new(),
        };
    }
    let mut out = Vec::new();
    for p in &m.panes {
        if p.active {
            if is_session(agent) {
                out.push(tab_session(
                    p.pane_pid,
                    p.log.as_deref().or(m.log.as_deref()),
                ));
            }
        } else if is_session(p.agent.as_ref()) {
            out.push(Session {
                tab,
                pane: p.pane_pid,
                repo: p.repo.as_ref(),
                log: subagent_log(p.agent.as_ref(), p.log.as_deref()),
                card: FlowNode {
                    folder: p
                        .cwd
                        .as_deref()
                        .map(|c| display_cwd(c, home))
                        .unwrap_or_default(),
                    branch: p.branch.clone(),
                    skin: skin_for(p.agent.as_ref()),
                    status: resolve_status(p.agent.as_ref(), false, false, None),
                    selected: false,
                    ..card.clone()
                },
            });
        }
    }
    out
}

/// A subagent card's two lines (D8): its type, else its description, else the literal
/// `agent`; the folder line is the description unless the description is already the title.
fn subagent_text(s: &Subagent) -> (String, String) {
    let non_empty = |o: &Option<String>| o.as_deref().filter(|t| !t.is_empty()).map(str::to_string);
    match (non_empty(&s.subagent_type), non_empty(&s.description)) {
        (Some(ty), desc) => (ty, desc.unwrap_or_default()),
        (None, Some(desc)) => (desc, String::new()),
        (None, None) => ("agent".to_string(), String::new()),
    }
}

/// A repository's project card (D2): the main worktree's basename over its `~`-abbreviated
/// path. The rest is `FlowNode::default()`'s, which is exactly D2's — no branch,
/// `CardStatus::Hidden`, `AgentSkin::Generic`, `FlowTarget::None` (inert) — so it is not
/// restated field by field.
fn project_card(repo: &RepoId, home: Option<&Path>) -> FlowNode {
    let root = repo.main_root.as_path();
    FlowNode {
        kind: NodeKind::Project,
        title: folder_name(Some(root)).unwrap_or_else(|| root.display().to_string()),
        folder: display_cwd(root, home),
        ..FlowNode::default()
    }
}

/// Append `s`'s card under `parent`, then its column-2 nodes (D8): its running subagents in
/// log order, then — when any finished — one `N finished` fold node, followed by the
/// finished ones as muted cards when its key is in `folds`. Records the key as live.
fn place(
    nodes: &mut Vec<FlowNode>,
    s: &Session<'_>,
    parent: Option<usize>,
    subagents: &HashMap<PathBuf, Vec<Subagent>>,
    folds: &FoldSet,
    live: &mut FoldSet,
) {
    let key = (TabId(s.tab), s.pane);
    live.insert(key);
    let subs = s.log.and_then(|l| subagents.get(l)).map(Vec::as_slice);
    let (finished, running): (Vec<&Subagent>, Vec<&Subagent>) =
        subs.unwrap_or_default().iter().partition(|a| a.completed);
    let at = nodes.len();
    nodes.push(FlowNode {
        parent,
        // D12's `+N` (narrow mode): running plus finished. Old D15 caps a log at 32.
        extra: u32::try_from(running.len() + finished.len()).unwrap_or(u32::MAX),
        ..s.card.clone()
    });
    let card = |a: &Subagent, muted: bool| {
        let (title, folder) = subagent_text(a);
        FlowNode {
            kind: NodeKind::Subagent,
            parent: Some(at),
            title,
            folder,
            branch: None,
            skin: s.card.skin,
            status: if muted {
                CardStatus::Hidden
            } else {
                CardStatus::Working
            },
            muted,
            selected: false,
            target: FlowTarget::Node(s.tab),
            extra: 0,
        }
    };
    nodes.extend(running.iter().map(|a| card(a, false)));
    if finished.is_empty() {
        return;
    }
    let expanded = folds.contains(&key);
    let mark = if expanded { '▾' } else { '▸' };
    nodes.push(FlowNode {
        kind: NodeKind::Fold,
        parent: Some(at),
        title: format!("{mark} {} finished", finished.len()),
        target: FlowTarget::Fold(s.tab, s.pane),
        ..FlowNode::default()
    });
    if expanded {
        nodes.extend(finished.iter().map(|a| card(a, true)));
    }
}

/// The graph's nodes in pre-order, plus `folds` with every key whose session no longer
/// exists removed (D8 — the overlay stores the returned set as its own).
///
/// Shape (D1): one project card per repository (grouped by `RepoId.common`), groups in the
/// strip order of their first session; under each, its main-worktree sessions, then its
/// linked-worktree ones, each in strip (then pane) order; then every session outside a
/// repository, unparented. Each session is followed by its column-2 nodes ([`place`]).
///
/// `active` is the strip index of the active tab (`SidebarModel.active`): its session is
/// `selected` (D3) — the one argument beyond the spec's five, since no row carries it.
///
/// Rows own order and membership (old D18): a meta with no row is ignored; a row with no meta
/// yields nothing. Subagents are looked up by the session's log path, so two sessions on one
/// log each list that log's subagents, and a session with no log or no entry in `subagents`
/// simply has none (old D14). Always emits nodes: `resolving` is the overlay's call (D16).
pub fn build_graph(
    rows: &[SidebarRow],
    metas: &HashMap<TabId, TabMeta>,
    home: Option<&Path>,
    subagents: &HashMap<PathBuf, Vec<Subagent>>,
    folds: &FoldSet,
    active: usize,
) -> (FlowGraph, FoldSet) {
    let all: Vec<Session<'_>> = rows
        .iter()
        .enumerate()
        .flat_map(|(i, row)| sessions_of(row, metas.get(&TabId(row.tab_id)), home, i == active))
        .collect();
    let mut groups: Vec<(&RepoId, Vec<&Session<'_>>)> = Vec::new();
    let mut tail = Vec::new();
    for s in &all {
        let Some(repo) = s.repo else {
            tail.push(s);
            continue;
        };
        match groups.iter_mut().find(|(g, _)| g.common == repo.common) {
            Some((_, members)) => members.push(s),
            None => groups.push((repo, vec![s])),
        }
    }
    let mut nodes = Vec::new();
    let mut live = FoldSet::new();
    for (repo, members) in groups {
        let project = nodes.len();
        nodes.push(project_card(repo, home));
        let (main, linked): (Vec<_>, Vec<_>) = members
            .into_iter()
            .partition(|s| s.repo.is_some_and(|r| !r.linked));
        for s in main.into_iter().chain(linked) {
            place(&mut nodes, s, Some(project), subagents, folds, &mut live);
        }
    }
    for s in tail {
        place(&mut nodes, s, None, subagents, folds, &mut live);
    }
    let kept = folds.intersection(&live).copied().collect();
    let graph = FlowGraph {
        nodes,
        resolving: false,
    };
    (graph, kept)
}

/// Every log a session of `rows` reads its subagents from, sorted and deduplicated — the
/// set the subagent thread follows (old D10). Derived from the same session enumeration as
/// the graph, so the thread follows exactly the logs the graph can show.
fn referenced_logs(rows: &[SidebarRow], metas: &HashMap<TabId, TabMeta>) -> Vec<PathBuf> {
    let mut logs: Vec<PathBuf> = rows
        .iter()
        .flat_map(|row| {
            sessions_of(row, metas.get(&TabId(row.tab_id)), None, false)
                .into_iter()
                .filter_map(|s| s.log.map(Path::to_path_buf))
        })
        .collect();
    logs.sort();
    logs.dedup();
    logs
}

/// The graph a reader sees until the worker's first full pass with the graph wanted (D16).
fn resolving() -> FlowGraph {
    FlowGraph {
        nodes: Vec::new(),
        resolving: true,
    }
}

/// D11's cache key: the model is rebuilt only when one of these moved.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct RebuildKey {
    tabmeta: u64,
    /// Beside the generation, not folded into it: a snapshot stood in through
    /// [`FlowOverlay::sync_snapshot`] may flip readiness at an unchanged generation.
    ready: bool,
    rows_rev: u64,
    active: usize,
    subagents: u64,
    folds: u64,
}

/// Whether `snap` says this open's repositories are resolved (D10, D16): ready, and — when
/// there is a worker — ready for the open that asked (`epoch`), not an earlier one.
fn snapshot_ready(snap: &TabMetaSnapshot, epoch: Option<u64>) -> bool {
    snap.graph_ready() && epoch.is_none_or(|e| snap.graph_epoch() == e)
}

/// What the overlay and its draw layer share.
///
/// Shared because the rebuild runs in **two** places: `Overlay::poll` (a worker or
/// subagent wake) and the layer's `record`, once per frame. The second is what "a session
/// card's state always equals its tab's sidebar card" needs: a blocking dialog appearing
/// changes `rows_rev` on the PTY-output path, which wakes no overlay, and the card picks it
/// up in its own layer's per-frame pull — so the graph must check its key there too, or it
/// would lag the card until the next wake. The check is a few integer compares;
/// `build_graph` runs only when one differs.
struct Core {
    handle: FlowHandle,
    tabmeta: Option<TabMetaHandle>,
    snap: TabMetaSnapshot,
    /// The set `build_graph` reads and prunes; the overlay's own copy follows it.
    folds: FoldSet,
    /// Bumped on every fold toggle — the `folds` part of the key.
    fold_rev: u64,
    /// The current spawn's slot (old D10: replaced on every open).
    slot: SubagentSlot,
    /// Monotonic across opens (old D10).
    generation: Arc<AtomicU64>,
    telemetry: Option<FlowTelemetry>,
    /// What the thread was last told to follow.
    logs: Vec<PathBuf>,
    key: Option<RebuildKey>,
    /// The graph last handed to the panel.
    graph: FlowGraph,
    open: bool,
    /// The epoch this open asked the worker for (D10), `None` with no worker. A snapshot's
    /// readiness counts only under this epoch: a reopen's first `sync` can copy the
    /// previous open's `graph_ready` and repositories before the worker has reached the
    /// queued close and open, and those are exactly the stale groups D16 forbids showing.
    epoch: Option<u64>,
}

impl Core {
    /// Rebuild iff the key moved; `true` when the graph changed.
    fn refresh(&mut self) -> bool {
        if !self.open {
            return false;
        }
        if let Some(h) = &self.tabmeta {
            h.sync(&mut self.snap);
        }
        let shared = TabModelHandle::shared();
        let (rows_rev, active) = shared.with(|m| (m.rows_rev, m.active));
        let key = RebuildKey {
            tabmeta: self.snap.generation(),
            ready: snapshot_ready(&self.snap, self.epoch),
            rows_rev,
            active,
            subagents: self.generation.load(Ordering::SeqCst),
            folds: self.fold_rev,
        };
        if self.key == Some(key) {
            return false;
        }
        self.key = Some(key);
        let rows = shared.with(|m| m.rows.clone());
        let metas: HashMap<TabId, TabMeta> =
            self.snap.iter().map(|(id, m)| (id, m.clone())).collect();
        let logs = referenced_logs(&rows, &metas);
        if logs != self.logs {
            if let Some(t) = &self.telemetry {
                t.update_logs(logs.clone());
            }
            self.logs = logs;
        }
        let graph = if key.ready {
            let subagents = match self.slot.lock() {
                Ok(g) => g.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            };
            // `$HOME` at the point of use, as `SidebarMetaSource::fill` reads it, so the cards
            // resolve exactly as the sidebar's do.
            let home = std::env::var_os("HOME").map(PathBuf::from);
            let (graph, folds) = build_graph(
                &rows,
                &metas,
                home.as_deref(),
                &subagents,
                &self.folds,
                active,
            );
            self.folds = folds;
            graph
        } else {
            // D16: no groups until every tab's repository is known, so a session never
            // appears ungrouped and then jumps into its group.
            resolving()
        };
        if graph == self.graph {
            return false;
        }
        self.graph = graph.clone();
        self.handle.set(graph);
        true
    }
}

/// The session graph's overlay (D4–D7, D11, D16).
pub struct FlowOverlay {
    visible: bool,
    wake: Arc<dyn Fn() + Send + Sync>,
    core: Rc<RefCell<Core>>,
    /// The overlay's view of the fold set, for [`folds`](Self::folds). The authoritative
    /// copy is `Core::folds`, which a rebuild prunes; this one re-reads it before every use.
    folds: FoldSet,
    /// Sub-slot wheel travel, in slots, carried between events.
    wheel_residue: f32,
}

impl FlowOverlay {
    /// `wake` is the capture-style loop wake the subagent thread calls after a publish;
    /// `tabmeta` is the worker's handle (`None` in tests: the graph then builds from the
    /// snapshot a test stands in through [`sync_snapshot`](Self::sync_snapshot)); `handle` is
    /// shared with the draw layer.
    pub fn new(
        wake: Arc<dyn Fn() + Send + Sync>,
        tabmeta: Option<TabMetaHandle>,
        handle: FlowHandle,
    ) -> FlowOverlay {
        // The graph starts closed and is never restored open (D4); say so on the handle
        // rather than trust its default.
        handle.set_visible(false);
        let core = Core {
            handle,
            tabmeta,
            snap: TabMetaSnapshot::default(),
            folds: FoldSet::new(),
            fold_rev: 0,
            slot: Arc::new(Mutex::new(HashMap::new())),
            generation: Arc::new(AtomicU64::new(0)),
            telemetry: None,
            logs: Vec::new(),
            key: None,
            graph: FlowGraph::default(),
            open: false,
            epoch: None,
        };
        FlowOverlay {
            visible: false,
            wake,
            core: Rc::new(RefCell::new(core)),
            folds: FoldSet::new(),
            wheel_residue: 0.0,
        }
    }

    /// The sessions whose finished list is expanded (D8).
    pub fn folds(&self) -> &FoldSet {
        &self.folds
    }

    /// Stand `snap` in as the worker's latest publish (R7.2's test seam). `poll` reaches the
    /// same state through the worker handle's own `sync`; the key is dropped here because a
    /// stood-in snapshot need not move the generation.
    pub fn sync_snapshot(&mut self, snap: TabMetaSnapshot) {
        if let Ok(mut core) = self.core.try_borrow_mut() {
            core.snap = snap;
            core.key = None;
        }
    }

    /// Is this the Cmd+Shift+F chord (D5)? Case-folded like the monitor's `is_toggle`.
    fn is_toggle(input: &KeyInput) -> bool {
        if !(input.mods.super_key() && input.mods.shift_key()) {
            return false;
        }
        if input.mods.control_key() || input.mods.alt_key() {
            return false;
        }
        matches!(&input.key, Key::Character(s) if s.eq_ignore_ascii_case("f"))
    }

    /// Take the pruned fold set a rebuild may have left in the core.
    fn pull_folds(&mut self) {
        if let Ok(core) = self.core.try_borrow() {
            self.folds.clone_from(&core.folds);
        }
    }

    /// Show or hide the graph — the only place `visible` changes. Edge-triggered: the open
    /// path runs only on hidden → visible and the close path only on visible → hidden, so
    /// a repeat never respawns the subagent thread on a fresh slot.
    fn set_shown(&mut self, shown: bool) {
        if shown && !self.visible {
            self.open();
        } else if !shown && self.visible {
            self.close();
        }
        self.visible = shown;
    }

    /// The hidden → visible edge (D4, D10, old D9/D10): ask the worker for panes and
    /// repositories, start a subagent thread on a fresh slot, and show the layer — reading
    /// `resolving…` until the worker's first full pass, never the previous open's groups.
    fn open(&mut self) {
        // The view opens at slot 0, so travel banked during the last open is stale.
        self.wheel_residue = 0.0;
        let Ok(mut core) = self.core.try_borrow_mut() else {
            return;
        };
        core.epoch = core.tabmeta.as_ref().map(|h| h.set_graph_wanted(true));
        let slot: SubagentSlot = Arc::new(Mutex::new(HashMap::new()));
        core.slot = slot.clone();
        // The first rebuild hands the thread its logs; spawning with none keeps one
        // derivation of "which logs" (`referenced_logs`) rather than two.
        core.logs = Vec::new();
        core.telemetry = Some(flow_telemetry::spawn(
            Vec::new(),
            slot,
            core.generation.clone(),
            self.wake.clone(),
        ));
        core.snap = TabMetaSnapshot::default();
        core.key = None;
        core.graph = resolving();
        core.handle.set(resolving());
        core.handle.set_scroll(0);
        core.open = true;
        core.handle.set_visible(true);
    }

    /// The visible → hidden edge: the reverse, and the thread is told to stop, never joined.
    fn close(&mut self) {
        let Ok(mut core) = self.core.try_borrow_mut() else {
            return;
        };
        if let Some(h) = &core.tabmeta {
            h.set_graph_wanted(false);
        }
        if let Some(t) = core.telemetry.take() {
            t.stop();
        }
        core.open = false;
        core.key = None;
        core.handle.hover(None);
        core.handle.set_visible(false);
    }
}

impl Overlay for FlowOverlay {
    /// Above rename (200) and the ai-monitor (100): an open graph covers both, so it sees
    /// every key first (D5).
    fn priority(&self) -> i32 {
        300
    }

    fn visible(&self) -> bool {
        self.visible
    }

    /// D5. Cmd+Shift+F toggles the graph, open or closed, and asks the app for nothing.
    /// Open, the graph owns the keyboard: `Esc` closes it and every other key is swallowed,
    /// since typing into a covered screen is invisible input. Closed, every other key is
    /// someone else's.
    fn on_key(&mut self, input: &KeyInput, ctx: &mut AppCtx) -> Handled {
        if Self::is_toggle(input) {
            self.set_shown(!self.visible);
            ctx.request_redraw();
            return Handled::Consumed;
        }
        if !self.visible {
            return Handled::Passthrough;
        }
        if input.key == Key::Named(NamedKey::Escape) {
            self.set_shown(false);
            ctx.request_redraw();
        }
        Handled::Consumed
    }

    /// An IME syllable or composition is a key too (D5): `Ime::Commit` bypasses `on_key`
    /// entirely, so without this a Hangul syllable typed under the open graph would land in
    /// the covered shell. Everything else — `RestoreMonitor` above all, which is first-wins
    /// and must reach the ai-monitor below (R1.5) — passes.
    fn on_command(&mut self, cmd: &OverlayCommand) -> Handled {
        match cmd {
            OverlayCommand::ImeCommit(_) | OverlayCommand::ImePreedit(_) if self.visible => {
                Handled::Consumed
            }
            _ => Handled::Passthrough,
        }
    }

    /// The graph covers the window while open (D4/D6), so the app routes every press,
    /// wheel and hover here first.
    fn covers_window(&self) -> bool {
        true
    }

    /// D13: one notch is one slot, clamped against the published view by
    /// [`FlowHandle::wheel`]. Positive notches move down the graph, while winit's positive
    /// wheel is toward the top — hence the negation. Pixel (trackpad) travel banks in slots,
    /// one slot per notch's worth of terminal travel (`LINES_PER_NOTCH` lines), so a mouse
    /// and a trackpad move the graph alike. Always consumed while open: the wheel is the
    /// graph's wherever it lands (D6), and there is nothing beneath for it to fall to.
    fn on_wheel(&mut self, delta: WheelDelta, cell_h: f32) -> Handled {
        if !self.visible {
            return Handled::Passthrough;
        }
        let notch_px = input::LINES_PER_NOTCH * cell_h;
        if notch_px > 0.0 {
            self.wheel_residue -= input::panel_wheel_px(delta, cell_h) / notch_px;
        }
        let notches = self.wheel_residue.trunc();
        self.wheel_residue -= notches;
        if notches != 0.0 {
            if let Ok(core) = self.core.try_borrow() {
                core.handle.wheel(notches as i32);
            }
        }
        Handled::Consumed
    }

    /// D7: a session or subagent card switches to its tab and closes the graph; `N finished`
    /// toggles that session's fold and keeps it open; anything else does nothing. All of them
    /// are consumed — a press falling through would act on a screen the user cannot see.
    fn on_press(&mut self, point: [f32; 2], ctx: &mut AppCtx) -> Handled {
        if !self.visible {
            return Handled::Passthrough;
        }
        self.pull_folds();
        let hit = self
            .core
            .try_borrow()
            .ok()
            .and_then(|c| c.handle.press(point));
        match hit {
            Some(FlowHit::Node(id)) => {
                ctx.activate_tab(TabId(id));
                self.set_shown(false);
                ctx.request_redraw();
            }
            Some(FlowHit::Fold(id, pid)) => {
                let key = (TabId(id), pid);
                if !self.folds.remove(&key) {
                    self.folds.insert(key);
                }
                if let Ok(mut core) = self.core.try_borrow_mut() {
                    core.folds.clone_from(&self.folds);
                    core.fold_rev = core.fold_rev.wrapping_add(1);
                }
                ctx.request_redraw();
            }
            None => {}
        }
        Handled::Consumed
    }

    /// D3's `Hover`: the layer resolves the point against the frame it draws.
    fn on_hover(&mut self, point: Option<[f32; 2]>) {
        if let Ok(core) = self.core.try_borrow() {
            core.handle.hover(point);
        }
    }

    fn layer(&self) -> Box<dyn DrawLayer> {
        let handle = self.core.borrow().handle.clone();
        Box::new(FlowLayer {
            panel: FlowPanel::new(handle),
            core: self.core.clone(),
        })
    }

    /// A worker or subagent wake: rebuild iff the key moved, and report whether the graph
    /// did. A hidden graph does nothing (Constraints: no work while closed).
    fn poll(&mut self) -> bool {
        if !self.visible {
            return false;
        }
        let changed = match self.core.try_borrow_mut() {
            Ok(mut core) => core.refresh(),
            Err(_) => false,
        };
        self.pull_folds();
        changed
    }
}

/// The graph's layer: D11's per-frame key check, then the renderer's full-window panel.
struct FlowLayer {
    panel: FlowPanel,
    core: Rc<RefCell<Core>>,
}

impl DrawLayer for FlowLayer {
    fn z_order(&self) -> i32 {
        self.panel.z_order()
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        // `try_borrow_mut`: an interleaving that ever made this re-entrant skips a rebuild
        // for one frame instead of aborting.
        if let Ok(mut core) = self.core.try_borrow_mut() {
            core.refresh();
        }
        self.panel.record(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabmeta::{AgentState, PaneMeta};
    use renderer::graph_panel::FlowView;
    use renderer::sidebar_theme::AgentSkin;
    use winit::keyboard::ModifiersState as M;

    fn claude(state: AgentState) -> Agent {
        Agent {
            kind: AgentKind::Claude,
            state,
        }
    }

    fn row(id: u64) -> SidebarRow {
        SidebarRow {
            tab_id: id,
            title: format!("t{id}"),
            ..SidebarRow::default()
        }
    }

    fn sub(completed: bool) -> Subagent {
        Subagent {
            id: "x".into(),
            subagent_type: None,
            description: None,
            model: None,
            start_ms: None,
            end_ms: None,
            completed,
            agent_id: None,
            ended_ok: true,
        }
    }

    fn pane(pid: u32, active: bool, agent: Option<Agent>, log: Option<&Path>) -> PaneMeta {
        PaneMeta {
            pane_pid: pid,
            active,
            agent,
            log: log.map(Path::to_path_buf),
            cwd: None,
            branch: None,
            repo: None,
        }
    }

    fn build(rows: &[SidebarRow], metas: &HashMap<TabId, TabMeta>) -> FlowGraph {
        build_graph(rows, metas, None, &HashMap::new(), &FoldSet::new(), 0).0
    }

    fn chord(ch: &str, mods: M) -> KeyInput {
        KeyInput {
            key: Key::Character(ch.into()),
            text: Some(ch.into()),
            mods,
            physical: winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyF),
        }
    }

    fn ctx_run<R>(f: impl FnOnce(&mut AppCtx) -> R) -> (R, Vec<crate::seam::AppRequest>) {
        let mut pty = Vec::new();
        let mut ctx = AppCtx::new(&mut pty, 1, 0, None);
        let r = f(&mut ctx);
        let reqs = ctx.take_requests();
        (r, reqs)
    }

    /// An open overlay over a published view of `total` slots, `visible` of which fit.
    fn open_over(total: usize, visible: usize) -> (FlowOverlay, FlowHandle) {
        let handle = FlowHandle::default();
        let mut o = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
        o.set_shown(true);
        handle.publish_view(FlowView {
            hits: Vec::new(),
            visible_slots: visible,
            total_slots: total,
        });
        (o, handle)
    }

    /// One unsplit Claude tab (id 1) on `log`, published to this thread's sidebar model
    /// and stood in as the worker's ready snapshot.
    fn publish_one_session(o: &mut FlowOverlay, log: &Path) {
        TabModelHandle::shared().set(renderer::sidebar_row::TabBarModel {
            titles: vec![row(1)],
            ..Default::default()
        });
        o.sync_snapshot(TabMetaSnapshot::for_test(
            vec![(
                TabId(1),
                TabMeta {
                    agent: Some(claude(AgentState::Busy)),
                    log: Some(log.to_path_buf()),
                    ..TabMeta::default()
                },
            )],
            true,
        ));
    }

    fn kinds(g: &FlowGraph) -> Vec<NodeKind> {
        g.nodes.iter().map(|n| n.kind).collect()
    }

    #[test]
    fn any_detected_agent_makes_a_session_and_only_claude_reads_subagents() {
        // R14 / R14.3: every kind is a session; "no agent" is not; only Claude carries the
        // subagent log, so only it is followed by the thread and gets column-2 children.
        let log = Path::new("/tmp/flow-unit/any.jsonl");
        assert!(!is_session(None));
        assert_eq!(subagent_log(None, Some(log)), None);
        for kind in [
            AgentKind::Claude,
            AgentKind::Antigravity,
            AgentKind::Codex,
            AgentKind::Gemini,
            AgentKind::Aider,
        ] {
            let a = Agent {
                kind,
                state: AgentState::Busy,
            };
            assert!(is_session(Some(&a)), "{kind:?} is a session");
            let want = (kind == AgentKind::Claude).then_some(log);
            assert_eq!(subagent_log(Some(&a), Some(log)), want, "{kind:?}");
            let metas = HashMap::from([(
                TabId(1),
                TabMeta {
                    agent: Some(a),
                    log: Some(log.to_path_buf()),
                    ..TabMeta::default()
                },
            )]);
            assert_eq!(kinds(&build(&[row(1)], &metas)), [NodeKind::Session]);
            let followed = referenced_logs(&[row(1)], &metas);
            assert_eq!(followed.is_empty(), kind != AgentKind::Claude, "{kind:?}");
        }
        let metas = HashMap::from([(TabId(1), TabMeta::default())]);
        assert!(build(&[row(1)], &metas).nodes.is_empty());
    }

    #[test]
    fn an_unsplit_session_folds_under_pane_zero_and_the_prune_keeps_only_live_keys() {
        let log = PathBuf::from("/tmp/flow-unit/a.jsonl");
        let meta = TabMeta {
            agent: Some(claude(AgentState::Busy)),
            log: Some(log.clone()),
            ..TabMeta::default()
        };
        let metas = HashMap::from([(TabId(3), meta)]);
        let subs = HashMap::from([(log, vec![sub(false), sub(true), sub(true), sub(true)])]);
        let folds = FoldSet::from([(TabId(3), UNSPLIT_PANE), (TabId(3), 77)]);
        let (g, kept) = build_graph(&[row(3)], &metas, None, &subs, &folds, 0);
        let fold = g.nodes.iter().find(|n| n.kind == NodeKind::Fold);
        assert_eq!(
            fold.map(|n| n.target),
            Some(FlowTarget::Fold(3, UNSPLIT_PANE))
        );
        assert_eq!(kept, FoldSet::from([(TabId(3), UNSPLIT_PANE)]));
        assert_eq!(g.nodes.last().map(|n| n.muted), Some(true));
        // D12's `+N`: the running one plus the three finished, on the session only.
        let extras: Vec<u32> = g.nodes.iter().map(|n| n.extra).collect();
        assert_eq!(extras, vec![4, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_non_active_pane_takes_its_own_branch_and_skin_and_is_never_selected() {
        let log = PathBuf::from("/tmp/flow-unit/pane.jsonl");
        let mut other = pane(6, false, Some(claude(AgentState::Busy)), Some(&log));
        other.branch = Some("feat/pane".into());
        other.cwd = Some(PathBuf::from("/srv/pane"));
        let meta = TabMeta {
            // The tab's active pane is a plain shell: its own card is generic.
            agent: None,
            branch: Some("main".into()),
            panes: vec![pane(5, true, None, None), other],
            ..TabMeta::default()
        };
        let metas = HashMap::from([(TabId(1), meta)]);
        let g = build(&[row(1)], &metas);
        assert_eq!(g.nodes.len(), 1, "{:?}", g.nodes);
        let n = &g.nodes[0];
        assert_eq!(n.branch.as_deref(), Some("feat/pane"));
        assert_eq!(n.folder, "/srv/pane");
        assert_eq!(n.skin, skin_for(Some(&claude(AgentState::Busy))));
        assert_ne!(n.skin, AgentSkin::Generic);
        assert!(!n.selected, "a non-active pane is not the tab's selection");
        assert_eq!(n.target, FlowTarget::Node(1));
    }

    #[test]
    fn an_active_pane_without_its_own_log_reads_the_tabs() {
        let log = PathBuf::from("/tmp/flow-unit/tablog.jsonl");
        let meta = TabMeta {
            agent: Some(claude(AgentState::Busy)),
            log: Some(log.clone()),
            panes: vec![pane(5, true, None, None)],
            ..TabMeta::default()
        };
        let metas = HashMap::from([(TabId(1), meta)]);
        let subs = HashMap::from([(log, vec![sub(false)])]);
        let (g, _) = build_graph(&[row(1)], &metas, None, &subs, &FoldSet::new(), 0);
        assert_eq!(kinds(&g), vec![NodeKind::Session, NodeKind::Subagent]);
    }

    #[test]
    fn the_project_card_is_the_main_roots_name_and_path() {
        let repo = RepoId {
            common: PathBuf::from("/h/u/code/r/.git"),
            main_root: PathBuf::from("/h/u/code/r"),
            linked: true,
        };
        let card = project_card(&repo, Some(Path::new("/h/u")));
        assert_eq!(card.kind, NodeKind::Project);
        assert_eq!(card.title, "r");
        assert_eq!(card.folder, "~/code/r");
        assert_eq!(
            card,
            FlowNode {
                kind: NodeKind::Project,
                title: "r".into(),
                folder: "~/code/r".into(),
                ..FlowNode::default()
            }
        );
        // A root with no final component still names itself.
        let root = RepoId {
            main_root: PathBuf::from("/"),
            ..repo
        };
        assert_eq!(project_card(&root, None).title, "/");
    }

    #[test]
    fn a_subagent_card_falls_back_from_type_to_description_to_agent() {
        let mut s = sub(false);
        s.subagent_type = Some(String::new());
        s.description = Some("fix".into());
        assert_eq!(subagent_text(&s), ("fix".into(), String::new()));
        s.subagent_type = Some("Plan".into());
        assert_eq!(subagent_text(&s), ("Plan".into(), "fix".into()));
        s.description = Some(String::new());
        assert_eq!(subagent_text(&s), ("Plan".into(), String::new()));
        s.subagent_type = None;
        s.description = None;
        assert_eq!(subagent_text(&s), ("agent".into(), String::new()));
    }

    #[test]
    fn the_thread_follows_only_the_logs_a_session_reads() {
        let a = PathBuf::from("/tmp/flow-unit/a.jsonl");
        let b = PathBuf::from("/tmp/flow-unit/b.jsonl");
        let metas = HashMap::from([
            (
                TabId(1),
                TabMeta {
                    agent: Some(claude(AgentState::Idle)),
                    log: Some(b.clone()),
                    panes: vec![
                        pane(5, true, None, None),
                        pane(6, false, Some(claude(AgentState::Busy)), Some(&a)),
                        pane(7, false, Some(claude(AgentState::Busy)), Some(&a)),
                    ],
                    ..TabMeta::default()
                },
            ),
            (
                TabId(2),
                TabMeta {
                    agent: None,
                    log: Some(PathBuf::from("/tmp/flow-unit/c.jsonl")),
                    ..TabMeta::default()
                },
            ),
        ]);
        assert_eq!(referenced_logs(&[row(1), row(2)], &metas), vec![a, b]);
    }

    #[test]
    fn the_chord_needs_exactly_cmd_and_shift() {
        assert!(FlowOverlay::is_toggle(&chord("F", M::SUPER | M::SHIFT)));
        assert!(FlowOverlay::is_toggle(&chord("f", M::SUPER | M::SHIFT)));
        assert!(!FlowOverlay::is_toggle(&chord(
            "F",
            M::SUPER | M::SHIFT | M::ALT
        )));
        assert!(!FlowOverlay::is_toggle(&chord(
            "F",
            M::SUPER | M::SHIFT | M::CONTROL
        )));
        assert!(!FlowOverlay::is_toggle(&chord("F", M::SUPER)));
        assert!(!FlowOverlay::is_toggle(&chord("F", M::SHIFT)));
        assert!(!FlowOverlay::is_toggle(&chord("g", M::SUPER | M::SHIFT)));
    }

    #[test]
    fn an_open_graph_takes_ime_text_and_a_closed_one_passes_it() {
        let mut o = FlowOverlay::new(Arc::new(|| {}), None, FlowHandle::default());
        let commit = OverlayCommand::ImeCommit("하".into());
        let preedit = OverlayCommand::ImePreedit("ㅎ".into());
        assert_eq!(o.on_command(&commit), Handled::Passthrough);
        assert_eq!(o.on_command(&preedit), Handled::Passthrough);
        o.set_shown(true);
        assert_eq!(o.on_command(&commit), Handled::Consumed);
        assert_eq!(o.on_command(&preedit), Handled::Consumed);
        assert_eq!(o.on_command(&OverlayCommand::Cancel), Handled::Passthrough);
        assert!(o.visible(), "Cancel does not close the graph (D5)");
    }

    #[test]
    fn the_wheel_moves_whole_slots_and_banks_trackpad_travel() {
        let (mut o, handle) = open_over(60, 20);
        // One notch down (winit negative) is one slot.
        assert_eq!(o.on_wheel(WheelDelta::Lines(-1.0), 10.0), Handled::Consumed);
        assert_eq!(handle.scroll(), 1);
        // A notch's worth of terminal travel is 3 lines × 10 px: 29 px banks…
        assert_eq!(
            o.on_wheel(WheelDelta::Pixels(-29.0), 10.0),
            Handled::Consumed
        );
        assert_eq!(handle.scroll(), 1);
        // …and the thirtieth completes one slot, leaving nothing behind.
        o.on_wheel(WheelDelta::Pixels(-1.0), 10.0);
        assert_eq!(handle.scroll(), 2);
        o.on_wheel(WheelDelta::Pixels(-15.0), 10.0);
        assert_eq!(handle.scroll(), 2, "half a notch does not move");
        // Up toward the top, clamped there, and still the graph's.
        assert_eq!(o.on_wheel(WheelDelta::Lines(9.0), 10.0), Handled::Consumed);
        assert_eq!(handle.scroll(), 0);
    }

    #[test]
    fn a_degenerate_cell_height_moves_nothing_and_poisons_nothing() {
        let (mut o, handle) = open_over(60, 20);
        for cell_h in [0.0, -10.0, f32::NAN] {
            assert_eq!(
                o.on_wheel(WheelDelta::Pixels(-25.0), cell_h),
                Handled::Consumed
            );
            assert_eq!(
                o.on_wheel(WheelDelta::Lines(-2.0), cell_h),
                Handled::Consumed
            );
            assert_eq!(handle.scroll(), 0, "cell_h {cell_h}");
        }
        o.on_wheel(WheelDelta::Lines(-2.0), 10.0);
        assert_eq!(handle.scroll(), 2, "the residue survived");
    }

    #[test]
    fn a_reopen_starts_with_no_banked_wheel_travel() {
        let (mut o, handle) = open_over(60, 20);
        o.on_wheel(WheelDelta::Pixels(-20.0), 10.0);
        assert_eq!(handle.scroll(), 0, "two thirds of a notch banks");
        o.set_shown(false);
        o.set_shown(true);
        o.on_wheel(WheelDelta::Pixels(-20.0), 10.0);
        assert_eq!(
            handle.scroll(),
            0,
            "the last open's travel is not carried over"
        );
    }

    #[test]
    fn a_closed_graph_passes_the_wheel_and_the_press() {
        let handle = FlowHandle::default();
        let mut o = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
        handle.publish_view(FlowView {
            hits: vec![(FlowHit::Node(1), [0.0, 0.0, 10.0, 10.0])],
            visible_slots: 1,
            total_slots: 9,
        });
        assert_eq!(
            o.on_wheel(WheelDelta::Lines(-1.0), 10.0),
            Handled::Passthrough
        );
        assert_eq!(handle.scroll(), 0);
        let (h, reqs) = ctx_run(|ctx| o.on_press([1.0, 1.0], ctx));
        assert_eq!(h, Handled::Passthrough);
        assert!(reqs.is_empty());
    }

    #[test]
    fn hover_reaches_the_handle_and_closing_clears_it() {
        let (mut o, handle) = open_over(1, 1);
        o.on_hover(Some([3.0, 4.0]));
        assert_eq!(handle.hover_point(), Some([3.0, 4.0]));
        o.set_shown(false);
        assert_eq!(handle.hover_point(), None);
    }

    #[test]
    fn opening_shows_resolving_at_the_top_and_closing_stops_the_thread() {
        let (mut o, handle) = open_over(60, 20);
        handle.set(FlowGraph {
            nodes: vec![FlowNode::default()],
            resolving: false,
        });
        handle.set_scroll(7);
        o.set_shown(false);
        assert!(!handle.visible());
        assert!(
            o.core.borrow().telemetry.is_none(),
            "close stops the thread"
        );
        o.set_shown(true);
        assert!(handle.visible());
        assert!(o.core.borrow().telemetry.is_some());
        let g = handle.graph();
        assert!(
            g.resolving && g.nodes.is_empty(),
            "never the previous open's groups"
        );
        assert_eq!(handle.scroll(), 0);
    }

    /// The next command the detached worker inbox holds, as `(wanted, epoch)`.
    fn wanted(rx: &std::sync::mpsc::Receiver<crate::tabmeta::Cmd>) -> (bool, u64) {
        match rx.try_recv() {
            Ok(crate::tabmeta::Cmd::SetGraphWanted { wanted, epoch }) => (wanted, epoch),
            Ok(_) => panic!("not a SetGraphWanted"),
            Err(e) => panic!("no command queued: {e}"),
        }
    }

    #[test]
    fn d10_a_reopen_reads_resolving_until_the_worker_answers_this_open() {
        // The race: close then reopen before the worker has run either queued command. The
        // shared cache still holds the previous open's `graph_ready` (and its repositories),
        // and the reopen's snapshot starts at generation 0, so its first `sync` copies them.
        let (h, rx) = TabMetaHandle::detached();
        let handle = FlowHandle::default();
        let mut o = FlowOverlay::new(Arc::new(|| {}), Some(h.clone()), handle.clone());
        let resolving = |o: &FlowOverlay| {
            o.core.borrow_mut().refresh();
            handle.graph().resolving
        };

        o.set_shown(true);
        assert_eq!(wanted(&rx), (true, 1));
        assert!(resolving(&o), "nothing published yet");
        h.publish_for_test(Vec::new(), true, 1);
        assert!(!resolving(&o), "the first open's own readiness is taken");

        o.set_shown(false);
        o.set_shown(true);
        assert_eq!(wanted(&rx), (false, 0));
        assert_eq!(wanted(&rx), (true, 2));
        assert!(
            resolving(&o),
            "a reopen took the previous open's readiness before the worker reached it"
        );
        // The worker reaches the close: still not this open's.
        h.publish_for_test(Vec::new(), false, 1);
        assert!(resolving(&o));
        // ... then the open, and only now does the graph resolve.
        h.publish_for_test(Vec::new(), true, 2);
        assert!(!resolving(&o));
    }

    #[test]
    fn set_shown_opens_and_closes_only_on_an_edge() {
        let handle = FlowHandle::default();
        let mut o = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
        // Hidden and staying hidden: opens nothing.
        o.set_shown(false);
        assert!(!o.visible());
        assert!(!handle.visible());
        assert!(o.core.borrow().telemetry.is_none());
        o.set_shown(true);
        let slot = o.core.borrow().slot.clone();
        // Shown and staying shown: no close, and no second spawn on a fresh slot.
        o.set_shown(true);
        assert!(o.visible());
        assert!(handle.visible());
        assert!(o.core.borrow().telemetry.is_some());
        assert!(Arc::ptr_eq(&slot, &o.core.borrow().slot));
    }

    #[test]
    fn d11_refresh_rebuilds_only_while_open_and_only_when_the_key_moves() {
        let handle = FlowHandle::default();
        let mut o = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
        let log = PathBuf::from("/tmp/flow-unit/refresh.jsonl");
        publish_one_session(&mut o, &log);
        // Closed: no work at all, whatever has been published.
        assert!(!o.core.borrow_mut().refresh());
        assert!(o.core.borrow().key.is_none());
        assert_eq!(handle.graph(), FlowGraph::default());
        o.set_shown(true);
        publish_one_session(&mut o, &log);
        assert!(o.core.borrow_mut().refresh(), "the first open rebuild");
        assert_eq!(kinds(&handle.graph()), vec![NodeKind::Session]);
        assert_eq!(o.core.borrow().graph, handle.graph());
        // Old D10: the thread is told the logs the graph reads.
        assert_eq!(o.core.borrow().logs, vec![log]);
        // Same key: no rebuild, even with a stale fold poked into the stored set.
        let stale = FoldSet::from([(TabId(9), UNSPLIT_PANE)]);
        o.core.borrow_mut().folds = stale.clone();
        assert!(!o.core.borrow_mut().refresh());
        assert_eq!(
            o.core.borrow().folds,
            stale,
            "an unchanged key does not prune"
        );
        // The key moves (a fold toggle) but the graph does not: rebuilt — the prune ran —
        // and reported unchanged.
        o.core.borrow_mut().fold_rev += 1;
        assert!(!o.core.borrow_mut().refresh());
        assert!(o.core.borrow().folds.is_empty());
        // The active tab moves: the key moves and the selection with it.
        TabModelHandle::shared().set(renderer::sidebar_row::TabBarModel {
            titles: vec![row(1)],
            active: 3,
            ..Default::default()
        });
        assert!(o.core.borrow_mut().refresh());
        assert!(!handle.graph().nodes[0].selected);
    }

    #[test]
    fn poll_reports_a_rebuild_once_and_takes_the_pruned_folds() {
        let handle = FlowHandle::default();
        let mut o = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
        publish_one_session(&mut o, Path::new("/tmp/flow-unit/poll.jsonl"));
        assert!(!o.poll(), "hidden: no work");
        o.set_shown(true);
        publish_one_session(&mut o, Path::new("/tmp/flow-unit/poll.jsonl"));
        // A fold on a live session and one on a session that no longer exists.
        let live = (TabId(1), UNSPLIT_PANE);
        let both = FoldSet::from([live, (TabId(9), UNSPLIT_PANE)]);
        o.folds = both.clone();
        o.core.borrow_mut().folds = both;
        assert!(o.poll());
        assert_eq!(
            o.folds(),
            &FoldSet::from([live]),
            "D8: the overlay sees the prune"
        );
        assert!(!o.poll(), "nothing moved since");
    }

    #[test]
    fn the_layer_draws_at_the_graph_z_and_rebuilds_on_its_own_frame() {
        let handle = FlowHandle::default();
        let mut o = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
        assert_eq!(o.layer().z_order(), renderer::graph_panel::GRAPH_PANEL_Z);
        let Some(mut r) = renderer::testing::headless(
            64,
            64,
            &common::config::Config::default(),
            "the graph layer's per-frame refresh",
        ) else {
            return;
        };
        o.set_shown(true);
        publish_one_session(&mut o, Path::new("/tmp/flow-unit/layer.jsonl"));
        r.add_layer(o.layer());
        let t = term_core::Terminal::new(4, 1, term_core::Color(0xFFFFFF), term_core::Color(0));
        r.capture(&t.snapshot()).expect("capture");
        assert_eq!(kinds(&handle.graph()), vec![NodeKind::Session]);
    }

    #[test]
    fn d5_the_key_policy_and_the_graphs_place_in_the_stack() {
        let mut o = FlowOverlay::new(Arc::new(|| {}), None, FlowHandle::default());
        assert_eq!(o.priority(), 300);
        assert!(o.covers_window());
        let esc = KeyInput {
            key: Key::Named(NamedKey::Escape),
            text: Some("\u{1b}".into()),
            mods: M::empty(),
            physical: winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Escape),
        };
        let a = chord("a", M::empty());
        // Closed: only the chord is the graph's.
        for k in [&a, &esc] {
            let (h, reqs) = ctx_run(|ctx| o.on_key(k, ctx));
            assert_eq!(h, Handled::Passthrough);
            assert!(reqs.is_empty());
            assert!(!o.visible());
        }
        let (h, _) = ctx_run(|ctx| o.on_key(&chord("F", M::SUPER | M::SHIFT), ctx));
        assert_eq!(h, Handled::Consumed);
        assert!(o.visible());
        // Open: everything is swallowed, and only Esc closes.
        let (h, reqs) = ctx_run(|ctx| o.on_key(&a, ctx));
        assert_eq!(
            (h, reqs.is_empty(), o.visible()),
            (Handled::Consumed, true, true)
        );
        let (h, reqs) = ctx_run(|ctx| o.on_key(&esc, ctx));
        assert_eq!(
            (h, reqs.is_empty(), o.visible()),
            (Handled::Consumed, true, false)
        );
    }

    #[test]
    fn d7_a_card_activates_and_closes_a_fold_toggles_and_stays() {
        let (mut o, handle) = open_over(1, 1);
        handle.publish_view(FlowView {
            hits: vec![
                (FlowHit::Fold(7, 10), [0.0, 0.0, 10.0, 10.0]),
                (FlowHit::Node(7), [20.0, 0.0, 10.0, 10.0]),
            ],
            visible_slots: 1,
            total_slots: 1,
        });
        for expanded in [true, false] {
            let (h, reqs) = ctx_run(|ctx| o.on_press([5.0, 5.0], ctx));
            assert_eq!(h, Handled::Consumed);
            assert!(reqs.is_empty());
            assert_eq!(o.folds().contains(&(TabId(7), 10)), expanded);
            assert!(o.visible());
        }
        let (h, reqs) = ctx_run(|ctx| o.on_press([50.0, 50.0], ctx));
        assert_eq!(
            (h, reqs.is_empty(), o.visible()),
            (Handled::Consumed, true, true)
        );
        let (h, reqs) = ctx_run(|ctx| o.on_press([25.0, 5.0], ctx));
        assert_eq!(h, Handled::Consumed);
        assert_eq!(reqs, vec![crate::seam::AppRequest::ActivateTab(TabId(7))]);
        assert!(!o.visible());
    }
}
