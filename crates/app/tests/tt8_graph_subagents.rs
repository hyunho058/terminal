//! TT8 — acceptance tests for `specs/session-flow-graph` R8 (D8, old D15): a session's
//! subagents as column-2 cards, its finished ones folded behind one `N finished` node, and
//! the fold set keyed per session and pruned on rebuild.
//!
//! Everything is plain data handed to `app::build_graph` — no worker, no log file, no tmux,
//! no live machine state. Subagents are looked up by the session's log path, so the log
//! paths below are never opened.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ai_monitor::telemetry::Subagent;
use app::{build_graph, Agent, AgentKind, AgentState, FoldSet, PaneMeta, RepoId, TabId, TabMeta};
use renderer::graph_layout::{FlowGraph, FlowNode, FlowTarget, NodeKind};
use renderer::sidebar_card::CardStatus;
use renderer::sidebar_row::SidebarRow;

const TAB: u64 = 7;
/// The split fixture's active pane — the session whose subagents the tests read.
const ACTIVE_PANE: u32 = 10;
/// The split fixture's second pane: a plain shell, so the tab has exactly one session.
const SHELL_PANE: u32 = 11;

fn home() -> PathBuf {
    PathBuf::from("/Users/tester")
}

fn log_path(name: &str) -> PathBuf {
    PathBuf::from(format!("/nonexistent/tt8/{name}.jsonl"))
}

fn repo() -> RepoId {
    RepoId {
        common: PathBuf::from("/Users/tester/IdeaProjects/terminal/.git"),
        main_root: PathBuf::from("/Users/tester/IdeaProjects/terminal"),
        linked: false,
    }
}

fn claude(state: AgentState) -> Agent {
    Agent {
        kind: AgentKind::Claude,
        state,
    }
}

fn row(tab: u64) -> SidebarRow {
    SidebarRow {
        tab_id: tab,
        title: "terminal".to_string(),
        ..SidebarRow::default()
    }
}

fn subagent(id: &str, ty: Option<&str>, desc: Option<&str>, completed: bool) -> Subagent {
    Subagent {
        id: id.to_string(),
        subagent_type: ty.map(str::to_string),
        description: desc.map(str::to_string),
        model: None,
        start_ms: None,
        end_ms: None,
        completed,
        agent_id: None,
        ended_ok: true,
    }
}

/// R8.1's three running subagents, in log order.
fn running_three() -> Vec<Subagent> {
    vec![
        subagent("t1", Some("Explore"), Some("find x"), false),
        subagent("t2", None, Some("fix"), false),
        subagent("t3", None, None, false),
    ]
}

/// R8.1's running three plus four finished ones, interleaved in log order.
fn running_three_finished_four() -> Vec<Subagent> {
    vec![
        subagent("f1", Some("Plan"), Some("plan it"), true),
        subagent("t1", Some("Explore"), Some("find x"), false),
        subagent("f2", None, Some("done a"), true),
        subagent("t2", None, Some("fix"), false),
        subagent("f3", None, None, true),
        subagent("t3", None, None, false),
        subagent("f4", Some("Explore"), Some("done b"), true),
    ]
}

/// An unsplit Claude tab in the repo, reading `log`.
fn unsplit_meta(log: &Path) -> TabMeta {
    TabMeta {
        agent: Some(claude(AgentState::Busy)),
        cwd: Some(PathBuf::from("/Users/tester/IdeaProjects/terminal")),
        branch: Some("main".to_string()),
        log: Some(log.to_path_buf()),
        repo: Some(repo()),
        ..TabMeta::default()
    }
}

/// A split tab whose active pane [`ACTIVE_PANE`] runs Claude on `log` and whose other pane
/// [`SHELL_PANE`] is a plain shell — one session, with an explicit pane pid for its fold key.
fn split_meta(log: &Path) -> TabMeta {
    let cwd = PathBuf::from("/Users/tester/IdeaProjects/terminal");
    TabMeta {
        agent: Some(claude(AgentState::Busy)),
        cwd: Some(cwd.clone()),
        branch: Some("main".to_string()),
        log: Some(log.to_path_buf()),
        repo: Some(repo()),
        panes: vec![
            PaneMeta {
                pane_pid: ACTIVE_PANE,
                active: true,
                agent: Some(claude(AgentState::Busy)),
                log: Some(log.to_path_buf()),
                cwd: Some(cwd.clone()),
                branch: Some("main".to_string()),
                repo: Some(repo()),
            },
            PaneMeta {
                pane_pid: SHELL_PANE,
                active: false,
                agent: None,
                log: None,
                cwd: Some(cwd),
                branch: Some("main".to_string()),
                repo: None,
            },
        ],
        ..TabMeta::default()
    }
}

fn build(
    rows: &[SidebarRow],
    metas: &HashMap<TabId, TabMeta>,
    subagents: &HashMap<PathBuf, Vec<Subagent>>,
    folds: &FoldSet,
) -> (FlowGraph, FoldSet) {
    let home = home();
    // The active strip index only decides `selected`, which R8 does not read.
    build_graph(rows, metas, Some(home.as_path()), subagents, folds, 0)
}

/// The indices of every `Session` node, in node order.
fn sessions(g: &FlowGraph) -> Vec<usize> {
    g.nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.kind == NodeKind::Session)
        .map(|(i, _)| i)
        .collect()
}

/// The single session node's index; panics unless there is exactly one.
fn only_session(g: &FlowGraph) -> usize {
    let s = sessions(g);
    assert_eq!(
        s.len(),
        1,
        "expected exactly one session node: {:#?}",
        g.nodes
    );
    s[0]
}

/// The nodes hanging from `parent`, in node order, with their indices.
fn children(g: &FlowGraph, parent: usize) -> Vec<(usize, &FlowNode)> {
    g.nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.parent == Some(parent))
        .collect()
}

// --- R8.1 -------------------------------------------------------------------------------

#[test]
fn r8_1_running_subagents_are_column_two_cards_in_log_order() {
    let log = log_path("r8-1");
    let rows = vec![row(TAB)];
    let metas = HashMap::from([(TabId(TAB), unsplit_meta(&log))]);
    let subs = HashMap::from([(log.clone(), running_three())]);

    let (g, _) = build(&rows, &metas, &subs, &FoldSet::new());

    let session = only_session(&g);
    let session_skin = g.nodes[session].skin;
    let kids = children(&g, session);
    assert_eq!(kids.len(), 3, "three subagent children: {kids:#?}");
    let titles: Vec<&str> = kids.iter().map(|(_, n)| n.title.as_str()).collect();
    assert_eq!(titles, ["Explore", "fix", "agent"]);
    let folders: Vec<&str> = kids.iter().map(|(_, n)| n.folder.as_str()).collect();
    assert_eq!(folders, ["find x", "", ""]);
    for (i, n) in &kids {
        assert_eq!(
            n.kind,
            NodeKind::Subagent,
            "node {i} is a Subagent (column 2)"
        );
        assert_eq!(n.parent, Some(session), "node {i} hangs from the session");
        assert_eq!(n.status, CardStatus::Working, "node {i} is Working");
        assert_eq!(n.skin, session_skin, "node {i} carries the session's skin");
        assert!(!n.muted, "node {i}: a running subagent is not muted");
    }
    // In log order and directly after their session in the pre-order.
    let idx: Vec<usize> = kids.iter().map(|(i, _)| *i).collect();
    assert_eq!(idx, [session + 1, session + 2, session + 3]);
}

// --- R8.2 -------------------------------------------------------------------------------

#[test]
fn r8_2_finished_subagents_fold_behind_one_collapsed_node() {
    let log = log_path("r8-2");
    let rows = vec![row(TAB)];
    let metas = HashMap::from([(TabId(TAB), split_meta(&log))]);
    let subs = HashMap::from([(log.clone(), running_three_finished_four())]);

    let (g, _) = build(&rows, &metas, &subs, &FoldSet::new());

    let session = only_session(&g);
    let kids = children(&g, session);
    let kinds: Vec<NodeKind> = kids.iter().map(|(_, n)| n.kind).collect();
    assert_eq!(
        kinds,
        [
            NodeKind::Subagent,
            NodeKind::Subagent,
            NodeKind::Subagent,
            NodeKind::Fold
        ],
        "the running three, then exactly one Fold, and no finished subagent: {kids:#?}"
    );
    let titles: Vec<&str> = kids[..3].iter().map(|(_, n)| n.title.as_str()).collect();
    assert_eq!(titles, ["Explore", "fix", "agent"], "only the running ones");
    let fold = kids[3].1;
    assert_eq!(fold.title, "▸ 4 finished");
    assert_eq!(fold.target, FlowTarget::Fold(TAB, ACTIVE_PANE));
    assert!(
        !g.nodes.iter().any(|n| n.muted),
        "no finished subagent node while collapsed"
    );
    assert_eq!(
        g.nodes.iter().filter(|n| n.kind == NodeKind::Fold).count(),
        1,
        "exactly one Fold node in the graph"
    );
}

#[test]
fn r8_2_a_session_with_none_finished_has_no_fold_node() {
    let log = log_path("r8-2-none");
    let rows = vec![row(TAB)];
    let metas = HashMap::from([(TabId(TAB), unsplit_meta(&log))]);
    let subs = HashMap::from([(log.clone(), running_three())]);

    let (g, _) = build(&rows, &metas, &subs, &FoldSet::new());

    only_session(&g);
    assert!(
        !g.nodes.iter().any(|n| n.kind == NodeKind::Fold),
        "no Fold node without a finished subagent: {:#?}",
        g.nodes
    );
}

// --- R8.3 -------------------------------------------------------------------------------

#[test]
fn r8_3_expanded_finished_subagents_are_muted_hidden_cards() {
    let log = log_path("r8-3");
    let rows = vec![row(TAB)];
    let metas = HashMap::from([(TabId(TAB), split_meta(&log))]);
    let subs = HashMap::from([(log.clone(), running_three_finished_four())]);
    let folds: FoldSet = FoldSet::from([(TabId(TAB), ACTIVE_PANE)]);

    let (g, _) = build(&rows, &metas, &subs, &folds);

    let session = only_session(&g);
    let kids = children(&g, session);
    let fold_at = kids
        .iter()
        .position(|(_, n)| n.kind == NodeKind::Fold)
        .expect("a Fold node under the session");
    assert_eq!(kids[fold_at].1.title, "▾ 4 finished");
    let after = &kids[fold_at + 1..];
    assert_eq!(after.len(), 4, "four nodes follow the Fold: {after:#?}");
    for (i, n) in after {
        assert_eq!(n.kind, NodeKind::Subagent, "node {i} is a Subagent");
        assert!(n.muted, "node {i} is muted");
        assert_eq!(n.status, CardStatus::Hidden, "node {i} is Hidden");
    }
    // They follow the fold directly in the pre-order.
    let fold_idx = kids[fold_at].0;
    let idx: Vec<usize> = after.iter().map(|(i, _)| *i).collect();
    assert_eq!(
        idx,
        [fold_idx + 1, fold_idx + 2, fold_idx + 3, fold_idx + 4]
    );
}

// --- R8.4 -------------------------------------------------------------------------------

/// Pane pids of R8.4's split tab: `SHELL` is the active plain shell, `FIRST` and `SECOND`
/// are two non-active Claude panes on one log.
const R84_SHELL: u32 = 20;
const R84_FIRST: u32 = 21;
const R84_SECOND: u32 = 22;

fn r84_pane(pid: u32, active: bool, log: Option<&Path>) -> PaneMeta {
    PaneMeta {
        pane_pid: pid,
        active,
        agent: log.map(|_| claude(AgentState::Busy)),
        log: log.map(Path::to_path_buf),
        cwd: Some(PathBuf::from("/Users/tester/IdeaProjects/terminal")),
        branch: Some("main".to_string()),
        repo: log.map(|_| repo()),
    }
}

fn r84_meta(panes: Vec<PaneMeta>) -> TabMeta {
    TabMeta {
        agent: None,
        cwd: Some(PathBuf::from("/Users/tester/IdeaProjects/terminal")),
        branch: Some("main".to_string()),
        panes,
        ..TabMeta::default()
    }
}

/// The `Fold` node hanging from `session`.
fn fold_of(g: &FlowGraph, session: usize) -> &FlowNode {
    children(g, session)
        .into_iter()
        .map(|(_, n)| n)
        .find(|n| n.kind == NodeKind::Fold)
        .expect("a Fold node under the session")
}

#[test]
fn r8_4_folds_are_per_session_and_pruned() {
    let log = log_path("r8-4-shared");
    let rows = vec![row(TAB)];
    let subs = HashMap::from([(log.clone(), running_three_finished_four())]);
    let folds: FoldSet = FoldSet::from([(TabId(TAB), R84_FIRST)]);

    // Both sessions read one log; only the first's key is in the fold set.
    let both = r84_meta(vec![
        r84_pane(R84_SHELL, true, None),
        r84_pane(R84_FIRST, false, Some(&log)),
        r84_pane(R84_SECOND, false, Some(&log)),
    ]);
    let metas = HashMap::from([(TabId(TAB), both)]);
    let (g, kept) = build(&rows, &metas, &subs, &folds);

    let s = sessions(&g);
    assert_eq!(s.len(), 2, "two session nodes: {:#?}", g.nodes);
    let (first, second) = (s[0], s[1]);
    assert_eq!(fold_of(&g, first).target, FlowTarget::Fold(TAB, R84_FIRST));
    assert_eq!(
        fold_of(&g, second).target,
        FlowTarget::Fold(TAB, R84_SECOND)
    );
    assert_eq!(
        fold_of(&g, first).title,
        "▾ 4 finished",
        "first is expanded"
    );
    assert_eq!(fold_of(&g, second).title, "▸ 4 finished", "second is not");
    let muted_under = |p: usize| children(&g, p).iter().filter(|(_, n)| n.muted).count();
    assert_eq!(muted_under(first), 4, "first lists its finished subagents");
    assert_eq!(muted_under(second), 0, "second lists none");
    assert!(
        kept.contains(&(TabId(TAB), R84_FIRST)),
        "a live key is kept"
    );

    // The first session's pane disappears; the same fold set is handed in again.
    let gone = r84_meta(vec![
        r84_pane(R84_SHELL, true, None),
        r84_pane(R84_SECOND, false, Some(&log)),
    ]);
    let metas = HashMap::from([(TabId(TAB), gone)]);
    let (_, pruned) = build(&rows, &metas, &subs, &folds);

    assert!(
        !pruned.contains(&(TabId(TAB), R84_FIRST)),
        "the vanished session's key is pruned: {pruned:?}"
    );
}
