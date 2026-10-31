//! TT2 — acceptance tests for `specs/session-overview-ui` R2 (D3): a row's identity — project
//! name, agent label and tag, line 2 (branch, worktree directory, cwd fallback) and the prompt
//! line.
//!
//! Everything is hand-built data handed to `app::overview::build_overview`; nothing reads the
//! live machine. The agent's cwd is the graph-only `TabMeta.graph_agent_cwd` (D3, T3), and the
//! agent/task are set on both the sidebar fields and their graph-only twins so the tests do not
//! depend on which of the two the builder reads.
//!
//! R2.2's colours are a renderer palette role: the model carries `AgentTag`, which is what is
//! asserted here. The renderer exposes no public tag → colour accessor, so the hex values are
//! not asserted from this crate.
//!
//! R2.5 (resolving) is driven through the overlay and lives in `tt2_overview_overlay.rs`.

use std::collections::HashMap;
use std::path::PathBuf;

use app::overview::{build_overview, ExpandSet};
use app::{Agent, AgentKind, AgentState, RepoId, SessionDigest, TabId, TabMeta};
use renderer::overview_model::{AgentTag, Filter, OverviewModel, SessionRowData};
use renderer::sidebar_row::SidebarRow;

// ---- fixtures ---------------------------------------------------------------

const HOME: &str = "/u";
const NOW_MS: u64 = 1_800_000_000_000;

fn home() -> PathBuf {
    PathBuf::from(HOME)
}

fn repo(main_root: &str, linked: bool) -> RepoId {
    let main_root = PathBuf::from(main_root);
    RepoId {
        common: main_root.join(".git"),
        main_root,
        linked,
    }
}

fn row(tab: u64) -> SidebarRow {
    SidebarRow {
        tab_id: tab,
        title: format!("tab{tab}"),
        ..SidebarRow::default()
    }
}

/// An unsplit tab running `kind`, its agent process in `agent_cwd`.
fn meta(
    kind: AgentKind,
    repo: Option<RepoId>,
    branch: Option<&str>,
    agent_cwd: &str,
    task: Option<&str>,
) -> TabMeta {
    let agent = Some(Agent {
        kind,
        state: AgentState::Busy,
    });
    TabMeta {
        agent,
        graph_agent: agent,
        cwd: Some(PathBuf::from(agent_cwd)),
        graph_agent_cwd: Some(PathBuf::from(agent_cwd)),
        branch: branch.map(str::to_string),
        task: task.map(str::to_string),
        graph_task: task.map(str::to_string),
        repo,
        ..TabMeta::default()
    }
}

fn build(rows: &[SidebarRow], metas: Vec<(u64, TabMeta)>) -> OverviewModel {
    let metas: HashMap<TabId, TabMeta> = metas.into_iter().map(|(t, m)| (TabId(t), m)).collect();
    let digests: HashMap<PathBuf, SessionDigest> = HashMap::new();
    let h = home();
    let (model, _expanded) = build_overview(
        rows,
        &metas,
        Some(h.as_path()),
        &digests,
        &ExpandSet::new(),
        Filter::All,
        0,
        NOW_MS,
    );
    model
}

fn row_of(model: &OverviewModel, tab: u64) -> &SessionRowData {
    model
        .rows
        .iter()
        .find(|r| r.tab == tab)
        .unwrap_or_else(|| panic!("no row for tab {tab}: {:?}", model.rows))
}

// ---- R2.1 -------------------------------------------------------------------

/// R2.1: the title is the main worktree's basename and the agent kind's label; the task text
/// is the quoted prompt line and nothing else.
#[test]
fn r2_1_title_is_project_and_agent_task_only_as_prompt() {
    let task = "Base directory for this skill: …";
    let model = build(
        &[row(1)],
        vec![(
            1,
            meta(
                AgentKind::Claude,
                Some(repo("/u/IdeaProjects/terminal", false)),
                Some("main"),
                "/u/IdeaProjects/terminal",
                Some(task),
            ),
        )],
    );
    let r = row_of(&model, 1);
    assert_eq!(r.project, "terminal");
    assert_eq!(r.agent_label, "Claude Code");
    assert_eq!(
        r.prompt.as_deref(),
        Some("“Base directory for this skill: …”")
    );
    for (field, text) in [
        ("project", &r.project),
        ("agent_label", &r.agent_label),
        ("branch_line", &r.branch_line),
        ("now_doing", &r.now_doing),
    ] {
        assert!(
            !text.contains("Base directory"),
            "the task text must appear only as the prompt line, found in {field}: {text:?}"
        );
    }
}

// ---- R2.2 -------------------------------------------------------------------

/// R2.2: every agent kind's label, and the colour role (`AgentTag`) the palette maps to
/// `#E9967A`, `#A9B8FF`, `#86D1C4`, `#B8B1A7`, `#B8B1A7`.
#[test]
fn r2_2_agent_labels_and_tags_for_every_kind() {
    let kinds = [
        (1, AgentKind::Claude, "Claude Code", AgentTag::Claude),
        (2, AgentKind::Codex, "Codex", AgentTag::Codex),
        (
            3,
            AgentKind::Antigravity,
            "Antigravity",
            AgentTag::Antigravity,
        ),
        (4, AgentKind::Gemini, "Gemini", AgentTag::Gemini),
        (5, AgentKind::Aider, "Aider", AgentTag::Aider),
    ];
    let rows: Vec<SidebarRow> = kinds.iter().map(|(t, ..)| row(*t)).collect();
    let metas = kinds
        .iter()
        .map(|(t, k, ..)| {
            (
                *t,
                meta(
                    *k,
                    Some(repo("/u/IdeaProjects/terminal", false)),
                    Some("main"),
                    "/u/IdeaProjects/terminal",
                    None,
                ),
            )
        })
        .collect();
    let model = build(&rows, metas);
    let got: Vec<(u64, String, AgentTag)> = kinds
        .iter()
        .map(|(t, ..)| {
            let r = row_of(&model, *t);
            (*t, r.agent_label.clone(), r.agent)
        })
        .collect();
    let want: Vec<(u64, String, AgentTag)> = kinds
        .iter()
        .map(|(t, _, label, tag)| (*t, label.to_string(), *tag))
        .collect();
    assert_eq!(got, want);
}

// ---- R2.3 -------------------------------------------------------------------

/// R2.3: a linked-worktree session's line 2 is `{branch} · {worktree dir}`; a main-worktree
/// session's line 2 is the branch alone.
#[test]
fn r2_3_linked_worktree_line_two_names_the_worktree_directory() {
    let model = build(
        &[row(1), row(2)],
        vec![
            (
                1,
                meta(
                    AgentKind::Claude,
                    Some(repo("/u/IdeaProjects/terminal", true)),
                    Some("fix/mouse-drag-not-working"),
                    "/u/IdeaProjects/terminal-fix-mouse-drag-not-working",
                    None,
                ),
            ),
            (
                2,
                meta(
                    AgentKind::Claude,
                    Some(repo("/u/IdeaProjects/terminal", false)),
                    Some("main"),
                    "/u/IdeaProjects/terminal",
                    None,
                ),
            ),
        ],
    );
    assert_eq!(
        row_of(&model, 1).branch_line,
        "fix/mouse-drag-not-working · terminal-fix-mouse-drag-not-working"
    );
    assert_eq!(row_of(&model, 2).branch_line, "main");
}

// ---- R2.4 -------------------------------------------------------------------

/// R2.4: outside any repository, with no branch and no task: project is the cwd's folder
/// name, line 2 the `~`-abbreviated cwd, and no prompt line.
#[test]
fn r2_4_no_repo_no_branch_no_prompt_falls_back_to_the_cwd() {
    let model = build(
        &[row(1)],
        vec![(
            1,
            meta(AgentKind::Claude, None, None, "/u/scratch/notes", None),
        )],
    );
    let r = row_of(&model, 1);
    assert_eq!(r.project, "notes");
    assert_eq!(r.branch_line, "~/scratch/notes");
    assert_eq!(r.prompt, None);
}
