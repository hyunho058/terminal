//! TT14 — acceptance tests for `specs/session-flow-graph` R14 (amendment 2026-10-21): every
//! agent the sidebar detects — Claude, Antigravity, Codex, Gemini, Aider — is a session, while
//! subagents and the fold node stay Claude-only (D8).
//!
//! Everything is hand-built data handed to `app::build_graph`; nothing reads the live machine,
//! and the log paths are never opened.
//!
//! R14.5 (the worker resolves `repo` for any agent kind) is not here: the worker's per-tab
//! derive path is not public — `graph_pass`, `publish_graph_state` and `repo_of` are the only
//! worker-side exports, and none takes a detected agent — so it is left to T5's in-crate test.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ai_monitor::telemetry::Subagent;
use app::{
    build_graph, resolve_status, row_meta, Agent, AgentKind, AgentState, FoldSet, PaneMeta, RepoId,
    TabId, TabMeta,
};
use renderer::graph_layout::{FlowGraph, FlowNode, FlowTarget, NodeKind};
use renderer::sidebar_row::SidebarRow;

// ---- fixtures ---------------------------------------------------------------

const HOME: &str = "/Users/u";
const REPO: &str = "IdeaProjects/terminal";

fn home() -> PathBuf {
    PathBuf::from(HOME)
}

/// Repository R, seen from its main worktree.
fn repo() -> RepoId {
    let main_root = home().join(REPO);
    RepoId {
        common: main_root.join(".git"),
        main_root,
        linked: false,
    }
}

/// R seen from one of its linked worktrees.
fn linked() -> RepoId {
    RepoId {
        linked: true,
        ..repo()
    }
}

fn agent(kind: AgentKind, state: AgentState) -> Option<Agent> {
    Some(Agent { kind, state })
}

fn row(tab: u64) -> SidebarRow {
    SidebarRow {
        tab_id: tab,
        title: format!("tab{tab}"),
        ..SidebarRow::default()
    }
}

/// An unsplit tab in R's main worktree running `agent`.
fn unsplit(agent: Option<Agent>, log: Option<&Path>) -> TabMeta {
    TabMeta {
        agent,
        cwd: Some(home().join(REPO)),
        branch: Some("main".to_string()),
        log: log.map(Path::to_path_buf),
        repo: agent.map(|_| repo()),
        ..TabMeta::default()
    }
}

fn metas(entries: Vec<(u64, TabMeta)>) -> HashMap<TabId, TabMeta> {
    entries.into_iter().map(|(t, m)| (TabId(t), m)).collect()
}

fn build(
    rows: &[SidebarRow],
    metas: &HashMap<TabId, TabMeta>,
    subagents: &HashMap<PathBuf, Vec<Subagent>>,
) -> FlowGraph {
    let h = home();
    let (graph, _folds) = build_graph(
        rows,
        metas,
        Some(h.as_path()),
        subagents,
        &FoldSet::new(),
        0,
    );
    graph
}

fn sessions(g: &FlowGraph) -> Vec<(usize, &FlowNode)> {
    g.nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.kind == NodeKind::Session)
        .collect()
}

fn children(g: &FlowGraph, parent: usize) -> Vec<&FlowNode> {
    g.nodes
        .iter()
        .filter(|n| n.parent == Some(parent))
        .collect()
}

fn subagent(id: &str, completed: bool) -> Subagent {
    Subagent {
        id: id.to_string(),
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

// ---- R14.1 ------------------------------------------------------------------

/// R14.1: Antigravity, Codex, Gemini and Aider tabs are four session cards under R's one
/// project card, in tab order; the agentless tab 5 has no node; each card is its `row_meta`.
#[test]
fn r14_1_non_claude_agents_are_session_cards_grouped_by_repository() {
    let kinds = [
        AgentKind::Antigravity,
        AgentKind::Codex,
        AgentKind::Gemini,
        AgentKind::Aider,
    ];
    let rows: Vec<SidebarRow> = (1..=5).map(row).collect();
    let mut entries: Vec<(u64, TabMeta)> = kinds
        .iter()
        .zip(1u64..)
        .map(|(&k, t)| (t, unsplit(agent(k, AgentState::Busy), None)))
        .collect();
    entries.push((5, unsplit(None, None)));
    let metas = metas(entries);

    let g = build(&rows, &metas, &HashMap::new());

    let shape: Vec<(NodeKind, Option<usize>, FlowTarget)> = g
        .nodes
        .iter()
        .map(|n| (n.kind, n.parent, n.target))
        .collect();
    assert_eq!(
        shape,
        vec![
            (NodeKind::Project, None, FlowTarget::None),
            (NodeKind::Session, Some(0), FlowTarget::Node(1)),
            (NodeKind::Session, Some(0), FlowTarget::Node(2)),
            (NodeKind::Session, Some(0), FlowTarget::Node(3)),
            (NodeKind::Session, Some(0), FlowTarget::Node(4)),
        ],
        "one project for R, then tabs 1–4; no node for tab 5: {:#?}",
        g.nodes
    );
    assert_eq!(g.nodes[0].title, "terminal", "the project is R");
    for t in 1..=4u64 {
        let n = &g.nodes[t as usize];
        let expected = row_meta(&row(t), metas.get(&TabId(t)), Some(Path::new(HOME)));
        assert_eq!(n.title, expected.line1, "tab {t} title");
        assert_eq!(n.folder, expected.cwd_display, "tab {t} folder");
        assert_eq!(n.branch, expected.branch, "tab {t} branch");
        assert_eq!(n.skin, expected.skin, "tab {t} skin");
        assert_eq!(n.status, expected.status, "tab {t} status");
    }
}

// ---- R14.2 ------------------------------------------------------------------

/// R14.2: a split tab with a Claude active pane (main worktree) and a Codex non-active pane
/// (linked worktree `w`) is two session cards under R — Claude's first — and the Codex card's
/// status is `resolve_status(pane.agent, false, false, None)`.
#[test]
fn r14_2_mixed_agents_in_one_split_tab() {
    let claude = agent(AgentKind::Claude, AgentState::Busy);
    // Waiting, so the pane's own status differs from the tab's (Claude, Busy → Working).
    let codex = agent(AgentKind::Codex, AgentState::Waiting);
    let codex_pane = PaneMeta {
        pane_pid: 21,
        active: false,
        agent: codex,
        log: None,
        cwd: Some(home().join("wt/w")),
        branch: Some("feat/w".to_string()),
        repo: Some(linked()),
    };
    let tab = TabMeta {
        agent: claude,
        cwd: Some(home().join(REPO)),
        branch: Some("main".to_string()),
        repo: Some(repo()),
        panes: vec![
            // tmux order puts the linked Codex pane first; the main group must still lead.
            codex_pane.clone(),
            PaneMeta {
                pane_pid: 20,
                active: true,
                agent: claude,
                log: None,
                cwd: Some(home().join(REPO)),
                branch: Some("main".to_string()),
                repo: Some(repo()),
            },
        ],
        ..TabMeta::default()
    };
    let rows = vec![row(7)];
    let claude_folder = row_meta(&row(7), Some(&tab), Some(Path::new(HOME))).cwd_display;
    let codex_folder = row_meta(
        &row(7),
        Some(&TabMeta {
            cwd: codex_pane.cwd.clone(),
            ..TabMeta::default()
        }),
        Some(Path::new(HOME)),
    )
    .cwd_display;
    let metas = metas(vec![(7, tab)]);

    let g = build(&rows, &metas, &HashMap::new());

    let project = g
        .nodes
        .iter()
        .position(|n| n.kind == NodeKind::Project)
        .expect("R's project card");
    let s = sessions(&g);
    assert_eq!(s.len(), 2, "two session cards: {:#?}", g.nodes);
    assert!(s.iter().all(|(_, n)| n.parent == Some(project)));
    assert_eq!(s[0].1.folder, claude_folder, "the Claude pane's card leads");
    assert_eq!(s[1].1.folder, codex_folder, "the Codex pane's card follows");
    assert_eq!(
        s[1].1.status,
        resolve_status(codex.as_ref(), false, false, None)
    );
}

// ---- R14.3 ------------------------------------------------------------------

/// R14.3: with both logs in the subagent map, the Claude session gets one Subagent then one
/// Fold node; the Codex session gets no children.
#[test]
fn r14_3_subagents_only_under_claude() {
    let codex_log = PathBuf::from("/nonexistent/tt14/codex.jsonl");
    let claude_log = PathBuf::from("/nonexistent/tt14/claude.jsonl");
    let rows = vec![row(1), row(2)];
    let metas = metas(vec![
        (
            1,
            unsplit(agent(AgentKind::Codex, AgentState::Busy), Some(&codex_log)),
        ),
        (
            2,
            unsplit(
                agent(AgentKind::Claude, AgentState::Busy),
                Some(&claude_log),
            ),
        ),
    ]);
    let subs = HashMap::from([
        (
            codex_log.clone(),
            vec![subagent("c1", false), subagent("c2", true)],
        ),
        (
            claude_log.clone(),
            vec![subagent("a1", false), subagent("a2", true)],
        ),
    ]);

    let g = build(&rows, &metas, &subs);

    let session_of = |tab: u64| {
        sessions(&g)
            .into_iter()
            .find(|(_, n)| n.target == FlowTarget::Node(tab))
            .map(|(i, _)| i)
            .unwrap_or_else(|| panic!("a session node for tab {tab}: {:#?}", g.nodes))
    };
    let claude_kids: Vec<NodeKind> = children(&g, session_of(2)).iter().map(|n| n.kind).collect();
    assert_eq!(claude_kids, [NodeKind::Subagent, NodeKind::Fold]);
    assert!(
        children(&g, session_of(1)).is_empty(),
        "the Codex session has no child nodes"
    );
}

// ---- R14.4 ------------------------------------------------------------------

/// R14.4: Antigravity (Working), Codex (Waiting for input) and Claude (Ready) sessions read
/// `1 working · 1 waiting / 3`.
#[test]
fn r14_4_header_counts_every_agent_session() {
    let rows = vec![row(1), row(2), row(3)];
    let metas = metas(vec![
        (
            1,
            unsplit(agent(AgentKind::Antigravity, AgentState::Busy), None),
        ),
        (
            2,
            unsplit(agent(AgentKind::Codex, AgentState::Waiting), None),
        ),
        (3, unsplit(agent(AgentKind::Claude, AgentState::Idle), None)),
    ]);

    let g = build(&rows, &metas, &HashMap::new());

    assert_eq!(g.header(), "1 working · 1 waiting / 3");
}
