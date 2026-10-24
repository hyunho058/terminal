//! Acceptance tests for session-flow-graph R5: the graph's shape (D1, D2, D3).
//!
//! Written from the spec's Thens against `app::build_graph`, the pure builder T4 adds. Every
//! fixture is hand-built — `RepoId`s, `TabMeta`s, `PaneMeta`s and `SidebarRow`s — so nothing
//! here reads the live machine. Subagents and folds are empty: R8 owns them.
//!
//! **Assumed signature.** The spec names `build_graph(rows, metas, home, subagents, folds)`,
//! but D3/R5.6 need the active tab, which no `SidebarRow` field carries (the strip's active
//! index lives on `SidebarModel.active`). These tests pass it as a sixth argument, a strip
//! index, through the one helper [`build`] — the only line to change if T4 places it otherwise.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ai_monitor::telemetry::Subagent;
use app::{
    build_graph, row_meta, Agent, AgentKind, AgentState, FoldSet, PaneMeta, RepoId, TabId, TabMeta,
};
use renderer::graph_layout::{FlowGraph, FlowNode, FlowTarget, NodeKind};
use renderer::sidebar_card::CardStatus;
use renderer::sidebar_row::SidebarRow;
use renderer::sidebar_theme::AgentSkin;

// ---- fixtures ---------------------------------------------------------------

const HOME: &str = "/Users/u";

fn home() -> PathBuf {
    PathBuf::from(HOME)
}

/// A repository whose main worktree is `$HOME/<rel>`.
fn repo_main(rel: &str) -> RepoId {
    let main_root = home().join(rel);
    RepoId {
        common: main_root.join(".git"),
        main_root,
        linked: false,
    }
}

/// The same repository seen from one of its linked worktrees.
fn linked(r: &RepoId) -> RepoId {
    RepoId {
        linked: true,
        ..r.clone()
    }
}

fn claude(state: AgentState) -> Option<Agent> {
    Some(Agent {
        kind: AgentKind::Claude,
        state,
    })
}

fn row(tab: u64) -> SidebarRow {
    SidebarRow {
        tab_id: tab,
        title: format!("tab{tab}"),
        ..SidebarRow::default()
    }
}

/// An unsplit Claude tab working in `cwd` (relative to `$HOME`), in `repo`.
fn claude_meta(cwd: &str, repo: Option<RepoId>) -> TabMeta {
    TabMeta {
        agent: claude(AgentState::Busy),
        cwd: Some(home().join(cwd)),
        repo,
        ..TabMeta::default()
    }
}

fn pane(pid: u32, active: bool, cwd: &str, agent: Option<Agent>, repo: Option<RepoId>) -> PaneMeta {
    PaneMeta {
        pane_pid: pid,
        active,
        agent,
        log: None,
        cwd: Some(home().join(cwd)),
        branch: None,
        repo,
    }
}

/// The one call site of the builder (see the module doc for the assumed `active` argument).
fn build(rows: &[SidebarRow], active: usize, metas: &HashMap<TabId, TabMeta>) -> FlowGraph {
    let subagents: HashMap<PathBuf, Vec<Subagent>> = HashMap::new();
    let folds = FoldSet::new();
    let h = home();
    let (graph, _folds) = build_graph(rows, metas, Some(h.as_path()), &subagents, &folds, active);
    graph
}

fn metas(entries: Vec<(u64, TabMeta)>) -> HashMap<TabId, TabMeta> {
    entries.into_iter().map(|(t, m)| (TabId(t), m)).collect()
}

/// What a node is, where it hangs, and what a press on it does — the shape, without content.
fn shape(g: &FlowGraph) -> Vec<(NodeKind, Option<usize>, FlowTarget)> {
    g.nodes
        .iter()
        .map(|n| (n.kind, n.parent, n.target))
        .collect()
}

fn sessions(g: &FlowGraph) -> Vec<&FlowNode> {
    g.nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Session)
        .collect()
}

// ---- R5.1 -------------------------------------------------------------------

/// R5.1: one project card first, then its main-worktree sessions in strip order, then its
/// linked-worktree sessions; a tab with no Claude agent yields no node.
#[test]
fn r5_1_one_project_main_sessions_before_worktree_sessions() {
    let r = repo_main("IdeaProjects/terminal");
    let rows = vec![row(1), row(2), row(3), row(4)];
    let metas = metas(vec![
        (1, claude_meta("wt/terminal-w", Some(linked(&r)))),
        (2, claude_meta("IdeaProjects/terminal", Some(r.clone()))),
        (
            3,
            TabMeta {
                cwd: Some(home().join("IdeaProjects/terminal")),
                ..TabMeta::default()
            },
        ),
        (4, claude_meta("IdeaProjects/terminal", Some(r.clone()))),
    ]);

    let g = build(&rows, 0, &metas);

    assert_eq!(
        shape(&g),
        vec![
            (NodeKind::Project, None, FlowTarget::None),
            (NodeKind::Session, Some(0), FlowTarget::Node(2)),
            (NodeKind::Session, Some(0), FlowTarget::Node(4)),
            (NodeKind::Session, Some(0), FlowTarget::Node(1)),
        ]
    );
    assert_eq!(g.nodes[0].title, "terminal", "the project is R");
}

// ---- R5.2 -------------------------------------------------------------------

/// R5.2: the project card exists with only a linked-worktree session, titled by the main
/// worktree's basename, folder `~`-abbreviated, no branch, no status, generic skin, inert.
#[test]
fn r5_2_project_card_content() {
    let r = repo_main("IdeaProjects/terminal");
    let rows = vec![row(1)];
    let mut m = claude_meta("wt/terminal-w", Some(linked(&r)));
    m.branch = Some("feat/w".to_string());
    let metas = metas(vec![(1, m)]);

    let g = build(&rows, 0, &metas);

    let project = g
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Project)
        .expect("a project node for a repo with only a linked-worktree session");
    assert_eq!(project.title, "terminal");
    assert_eq!(project.folder, "~/IdeaProjects/terminal");
    assert_eq!(project.branch, None);
    assert_eq!(project.status, CardStatus::Hidden);
    assert_eq!(project.skin, AgentSkin::Generic);
    assert_eq!(project.target, FlowTarget::None);
}

// ---- R5.3 -------------------------------------------------------------------

/// R5.3: repository groups follow the strip position of their first session; a session
/// outside any repository comes after every group, as a column-1 session with no parent.
#[test]
fn r5_3_groups_ordered_by_first_session_non_git_last_unparented() {
    let b = repo_main("code/b-repo");
    let a = repo_main("code/a-repo");
    let rows = vec![row(1), row(2), row(3)];
    let metas = metas(vec![
        (1, claude_meta("code/b-repo", Some(b))),
        (2, claude_meta("scratch", None)),
        (3, claude_meta("code/a-repo", Some(a))),
    ]);

    let g = build(&rows, 0, &metas);

    assert_eq!(
        shape(&g),
        vec![
            (NodeKind::Project, None, FlowTarget::None),
            (NodeKind::Session, Some(0), FlowTarget::Node(1)),
            (NodeKind::Project, None, FlowTarget::None),
            (NodeKind::Session, Some(2), FlowTarget::Node(3)),
            (NodeKind::Session, None, FlowTarget::Node(2)),
        ]
    );
    assert_eq!(g.nodes[0].title, "b-repo", "Project(B) first");
    assert_eq!(g.nodes[2].title, "a-repo", "Project(A) second");
}

// ---- R5.4 -------------------------------------------------------------------

/// R5.4: a split tab yields one session card per Claude pane, each in its own worktree's
/// group; a pane with no agent yields none; a non-active pane's folder is its own cwd.
#[test]
fn r5_4_split_tab_one_card_per_pane_session() {
    let r = repo_main("IdeaProjects/terminal");
    let rows = vec![row(7)];
    // tmux order puts the linked pane first: the main group must still come first (D1).
    let tab = TabMeta {
        agent: claude(AgentState::Busy),
        cwd: Some(home().join("IdeaProjects/terminal")),
        repo: Some(r.clone()),
        panes: vec![
            pane(
                11,
                false,
                "wt/terminal-w",
                claude(AgentState::Busy),
                Some(linked(&r)),
            ),
            pane(
                10,
                true,
                "IdeaProjects/terminal",
                claude(AgentState::Busy),
                Some(r.clone()),
            ),
            pane(12, false, "tmp", None, None),
        ],
        ..TabMeta::default()
    };
    let metas = metas(vec![(7, tab)]);

    let g = build(&rows, 0, &metas);

    // The folder pane 11's own cwd shows as on a card: the card's join, fed that cwd.
    let pane11_folder = row_meta(
        &row(7),
        Some(&TabMeta {
            cwd: Some(home().join("wt/terminal-w")),
            ..TabMeta::default()
        }),
        Some(Path::new(HOME)),
    )
    .cwd_display;
    let pane10_folder = row_meta(&row(7), metas.get(&TabId(7)), Some(Path::new(HOME))).cwd_display;

    let project = g
        .nodes
        .iter()
        .position(|n| n.kind == NodeKind::Project)
        .expect("Project(R)");
    let s = sessions(&g);
    assert_eq!(s.len(), 2, "panes 10 and 11 only; pane 12 runs no agent");
    assert!(s.iter().all(|n| n.parent == Some(project)));
    assert_eq!(
        s[0].folder, pane10_folder,
        "pane 10's session leads: main group"
    );
    assert_eq!(
        s[1].folder, pane11_folder,
        "pane 11's session follows: linked group"
    );
}

// ---- R5.5 -------------------------------------------------------------------

/// R5.5: a session card's content is exactly the sidebar card's `RowMeta`.
#[test]
fn r5_5_session_card_content_is_row_meta() {
    let r = repo_main("IdeaProjects/terminal");
    let rows = vec![SidebarRow {
        program: Some("zsh".to_string()),
        ..row(1)
    }];
    let meta = TabMeta {
        agent: claude(AgentState::Waiting),
        cwd: Some(home().join("IdeaProjects/terminal")),
        branch: Some("feat/session-flow".to_string()),
        task: Some("draw the graph".to_string()),
        repo: Some(r),
        ..TabMeta::default()
    };
    let expected = row_meta(&rows[0], Some(&meta), Some(Path::new(HOME)));
    let metas = metas(vec![(1, meta)]);

    let g = build(&rows, 0, &metas);

    let s = sessions(&g);
    assert_eq!(s.len(), 1);
    let n = s[0];
    assert_eq!(n.title, expected.line1);
    assert_eq!(n.folder, expected.cwd_display);
    assert_eq!(n.branch, expected.branch);
    assert_eq!(n.skin, expected.skin);
    assert_eq!(n.status, expected.status);
}

// ---- R5.6 -------------------------------------------------------------------

/// R5.6: exactly one node is selected — the active tab's session, its active pane when split.
#[test]
fn r5_6_active_tab_session_is_selected() {
    let r = repo_main("IdeaProjects/terminal");
    let rows = vec![row(1), row(2), row(3)];
    let split = TabMeta {
        agent: claude(AgentState::Busy),
        cwd: Some(home().join("IdeaProjects/terminal")),
        repo: Some(r.clone()),
        panes: vec![
            pane(
                20,
                false,
                "wt/other",
                claude(AgentState::Busy),
                Some(linked(&r)),
            ),
            pane(
                21,
                true,
                "IdeaProjects/terminal",
                claude(AgentState::Busy),
                Some(r.clone()),
            ),
        ],
        ..TabMeta::default()
    };
    let metas = metas(vec![
        (1, claude_meta("IdeaProjects/terminal", Some(r.clone()))),
        (2, split),
        (3, claude_meta("IdeaProjects/terminal", Some(r.clone()))),
    ]);
    let pane21_folder = row_meta(&row(2), metas.get(&TabId(2)), Some(Path::new(HOME))).cwd_display;

    // Tab 2 is strip index 1.
    let g = build(&rows, 1, &metas);

    let selected: Vec<&FlowNode> = g.nodes.iter().filter(|n| n.selected).collect();
    assert_eq!(selected.len(), 1, "exactly one selected node");
    let n = selected[0];
    assert_eq!(n.kind, NodeKind::Session);
    assert_eq!(n.target, FlowTarget::Node(2));
    assert_eq!(n.folder, pane21_folder, "pane 21's session, not pane 20's");
}
