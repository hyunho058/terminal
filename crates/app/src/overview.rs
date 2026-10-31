//! The Agent Monitor overview's policy (`session-overview-ui` D2–D8, D14, D20): pure
//! functions from the sidebar rows, the tabmeta snapshot and the flow telemetry digests to
//! the renderer's plain-data [`OverviewModel`]. All formatting and policy live here, testable
//! without a GPU; the renderer only lays out and draws.
//!
//! **Which rows exist** (D14) is the old graph's session enumeration, now this module's
//! alone ([`session_refs`] is the overlay's view of it): a tab or tmux pane with a detected
//! agent of any kind, in strip-then-pane order; the
//! toggle-free `graph_agent`/`graph_task` win over the card's `agent`/`task`; the active pane
//! takes the tab's own status join (`row_meta_with`), a non-active pane the log-only
//! `resolve_status(pane.agent, false, false, None)`; only a Claude session reads a digest.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ai_monitor::telemetry::Subagent;
use renderer::overview_model::{
    AgentTag, Banner, Bucket, Counts, DialogSubject, Filter, OverviewModel, SessionRowData,
    SubagentCell, SubagentStatus, SubagentTotals, TreeLine, View,
};
use renderer::sidebar_card::CardStatus;
use renderer::sidebar_row::SidebarRow;

use crate::flow_telemetry::SessionDigest;
use crate::session::TabId;
use crate::sidebar_model::{display_cwd, folder_name, resolve_status, row_meta_with};
use crate::tabmeta::{Agent, AgentKind, RepoId, TabMeta};

/// The expanded rows, keyed `(tab, pane pid)` and pruned on every build exactly like the old
/// graph's `FoldSet` (D8, D10). Kept across close and reopen within one app run.
pub type ExpandSet = HashSet<(TabId, u32)>;

/// The pane key of an unsplit tab's one session — the retired graph's fold key.
const UNSPLIT_PANE: u32 = 0;

/// The dash every missing value reads as (D7, D8, D17).
const DASH: &str = "—";

/// One agent session before formatting: what [`build_overview`] reads per row.
struct Session<'a> {
    tab: u64,
    pane: u32,
    agent: Agent,
    status: CardStatus,
    /// A blocking dialog is on this session's screen (active pane / unsplit tab only).
    dialog: bool,
    /// The log's last meaningful message asks the user (D2).
    asks_user: bool,
    /// The dialog's subject — only ever `Some` with `dialog`.
    subject: Option<&'a DialogSubject>,
    repo: Option<&'a RepoId>,
    /// The agent process's cwd, else the shell's (D3).
    cwd: Option<&'a Path>,
    branch: Option<&'a str>,
    task: Option<&'a str>,
    /// The digest key — Claude sessions only (R14.3).
    log: Option<&'a Path>,
    selected: bool,
}

/// The log a session reads its digest from: `log` for a Claude session, else none (R14.3).
fn claude_log<'a>(agent: &Agent, log: Option<&'a Path>) -> Option<&'a Path> {
    log.filter(|_| agent.kind == AgentKind::Claude)
}

/// A tab's agent sessions in pane order (D14) — see the module doc.
fn sessions_of<'a>(
    row: &'a SidebarRow,
    meta: Option<&'a TabMeta>,
    home: Option<&Path>,
    selected: bool,
) -> Vec<Session<'a>> {
    let Some(m) = meta else {
        return Vec::new();
    };
    let (agent, task) = match m.graph_agent {
        Some(_) => (m.graph_agent.as_ref(), m.graph_task.as_deref()),
        None => (m.agent.as_ref(), m.task.as_deref()),
    };
    let status = row_meta_with(row, Some(m), agent, task, home).status;
    let tab_cwd = m.graph_agent_cwd.as_deref().or(m.cwd.as_deref());
    let tab_session = |a: &Agent, pane: u32, log: Option<&'a Path>, asks: bool| Session {
        tab: row.tab_id,
        pane,
        agent: *a,
        status,
        dialog: row.blocking_dialog,
        asks_user: asks,
        subject: row
            .blocking_subject
            .as_ref()
            .filter(|_| row.blocking_dialog),
        repo: m.repo.as_ref(),
        cwd: tab_cwd,
        branch: m.branch.as_deref(),
        task,
        log: claude_log(a, log),
        selected,
    };
    if m.panes.is_empty() {
        return agent
            .map(|a| tab_session(a, UNSPLIT_PANE, m.log.as_deref(), m.graph_asks_user))
            .into_iter()
            .collect();
    }
    let mut out = Vec::new();
    for p in &m.panes {
        if p.active {
            if let Some(a) = agent {
                out.push(tab_session(
                    a,
                    p.pane_pid,
                    p.log.as_deref().or(m.log.as_deref()),
                    m.graph_asks_user || p.graph_asks_user,
                ));
            }
        } else if let Some(a) = p.agent.as_ref() {
            out.push(Session {
                tab: row.tab_id,
                pane: p.pane_pid,
                agent: *a,
                status: resolve_status(Some(a), false, false, None),
                dialog: false,
                asks_user: p.graph_asks_user,
                subject: None,
                repo: p.repo.as_ref(),
                cwd: p.graph_agent_cwd.as_deref().or(p.cwd.as_deref()),
                branch: p.branch.as_deref(),
                // The tab's task is the active pane's agent's prompt, not this one's.
                task: None,
                log: claude_log(a, p.log.as_deref()),
                selected: false,
            });
        }
    }
    out
}

/// D2: the card's status split by why it waits — `dialog` is a blocking dialog on the
/// session's screen, `asks_user` the log's last message asking the user.
pub fn bucket_of(status: CardStatus, dialog: bool, asks_user: bool) -> Bucket {
    match status {
        CardStatus::WaitingForInput if dialog || asks_user => Bucket::NeedsYou,
        CardStatus::WaitingForInput | CardStatus::Ready => Bucket::Idle,
        CardStatus::Working => Bucket::Working,
        // `Hidden`, and the agent-less statuses a session never carries: unknown.
        _ => Bucket::Agent,
    }
}

/// D3's label and colour role for an agent kind.
fn agent_identity(kind: AgentKind) -> (&'static str, AgentTag) {
    match kind {
        AgentKind::Claude => ("Claude Code", AgentTag::Claude),
        AgentKind::Codex => ("Codex", AgentTag::Codex),
        AgentKind::Antigravity => ("Antigravity", AgentTag::Antigravity),
        AgentKind::Gemini => ("Gemini", AgentTag::Gemini),
        AgentKind::Aider => ("Aider", AgentTag::Aider),
    }
}

/// D3's line 1 project name, and line 2 with whether it is a branch.
fn identity(s: &Session<'_>, row: &SidebarRow, home: Option<&Path>) -> (String, String, bool) {
    let project = s
        .repo
        .and_then(|r| folder_name(Some(r.main_root.as_path())))
        .or_else(|| folder_name(s.cwd))
        .unwrap_or_else(|| row.title.clone());
    let (line2, has_branch) = match s.branch.filter(|b| !b.is_empty()) {
        Some(b) => {
            let worktree = s.repo.filter(|r| r.linked).and_then(|_| folder_name(s.cwd));
            match worktree {
                Some(dir) => (format!("{b} · {dir}"), true),
                None => (b.to_string(), true),
            }
        }
        None => (
            s.cwd.map(|c| display_cwd(c, home)).unwrap_or_default(),
            false,
        ),
    };
    (project, line2, has_branch)
}

/// The digest's subagents in log order. The ring is newest first (`push_front`), so it is
/// reversed, then stably ordered by start time — a subagent's start is its `tool_use`, so
/// this is log order, and it stays log order for a caller that already handed one in.
/// A subagent with no start goes last.
fn log_order(subagents: &[Subagent]) -> Vec<&Subagent> {
    let mut v: Vec<&Subagent> = subagents.iter().rev().collect();
    v.sort_by_key(|a| a.start_ms.unwrap_or(u64::MAX));
    v
}

/// A subagent's colour role (D8): running, done, or failed.
pub(crate) fn status_of(a: &Subagent) -> SubagentStatus {
    match (a.completed, a.ended_ok) {
        (false, _) => SubagentStatus::Running,
        (true, true) => SubagentStatus::Done,
        (true, false) => SubagentStatus::Failed,
    }
}

/// `running` / `done` / `failed` (D8, D13).
pub(crate) fn status_word(s: SubagentStatus) -> &'static str {
    match s {
        SubagentStatus::Running => "running",
        SubagentStatus::Done => "done",
        SubagentStatus::Failed => "failed",
    }
}

/// D8's subagent duration, shared by the tree, the lanes and the panel: finished
/// `{m}m {ss}s` (fixed once it ends), running `now − start` in D7's format, `—` unknown.
pub(crate) fn subagent_duration(a: &Subagent, now_ms: u64) -> String {
    match (a.completed, a.start_ms, a.end_ms) {
        (true, Some(start), Some(end)) => {
            let secs = end.saturating_sub(start) / 1_000;
            format!("{}m {:02}s", secs / 60, secs % 60)
        }
        (false, Some(start), _) => format_elapsed(Some(start), now_ms),
        _ => DASH.to_string(),
    }
}

fn tree_line(a: &Subagent, now_ms: u64) -> TreeLine {
    let non_empty = |o: &Option<String>| o.as_deref().filter(|t| !t.is_empty()).map(str::to_string);
    let status = status_of(a);
    TreeLine {
        subagent_type: non_empty(&a.subagent_type).unwrap_or_else(|| "agent".to_string()),
        description: non_empty(&a.description).unwrap_or_else(|| DASH.to_string()),
        status_word: status_word(status).to_string(),
        duration: subagent_duration(a, now_ms),
        status,
    }
}

/// Build the overview model (D2, D3, D5, D6, D14, D20).
///
/// `rows` own order and membership; `metas` supply each tab's agent, panes, repository and
/// graph-only fields; `digests` are the flow telemetry thread's, keyed by log path;
/// `expanded` is pruned to the sessions that still exist and returned beside the model;
/// `active` is the active tab's index into `rows`; `now_ms` drives every elapsed text.
///
/// Never `resolving`, always [`View::Overview`]: both are the overlay's call (D10, D17).
#[allow(clippy::too_many_arguments)]
pub fn build_overview(
    rows: &[SidebarRow],
    metas: &HashMap<TabId, TabMeta>,
    home: Option<&Path>,
    digests: &HashMap<PathBuf, SessionDigest>,
    expanded: &ExpandSet,
    filter: Filter,
    active: usize,
    now_ms: u64,
) -> (OverviewModel, ExpandSet) {
    let mut all: Vec<SessionRowData> = Vec::new();
    let mut live = ExpandSet::new();
    let mut totals = SubagentTotals::default();
    for (i, row) in rows.iter().enumerate() {
        for s in sessions_of(row, metas.get(&TabId(row.tab_id)), home, i == active) {
            let key = (TabId(s.tab), s.pane);
            live.insert(key);
            let bucket = bucket_of(s.status, s.dialog, s.asks_user);
            let digest = s.log.and_then(|l| digests.get(l));
            let (project, branch_line, has_branch) = identity(&s, row, home);
            let (agent_label, agent) = agent_identity(s.agent.kind);
            let subs = digest.map(|d| log_order(&d.subagents)).unwrap_or_default();
            for a in &subs {
                match status_of(a) {
                    SubagentStatus::Running => totals.running += 1,
                    SubagentStatus::Done => totals.done += 1,
                    SubagentStatus::Failed => totals.failed += 1,
                }
            }
            let (cells, summary) = summarize(&subs);
            all.push(SessionRowData {
                tab: s.tab,
                pane: s.pane,
                bucket,
                project,
                agent_label: agent_label.to_string(),
                agent,
                branch_line,
                has_branch,
                now_doing: now_doing(&NowDoingInput {
                    bucket,
                    subject: s.subject,
                    digest,
                }),
                prompt: s.task.filter(|t| !t.is_empty()).map(|t| format!("“{t}”")),
                elapsed: format_elapsed(digest.and_then(|d| d.last_prompt_ms), now_ms),
                subagents: cells,
                subagent_summary: summary,
                tree: subs.iter().map(|a| tree_line(a, now_ms)).collect(),
                expanded: expanded.contains(&key),
                selected: s.selected,
            });
        }
    }
    // Stable: within a bucket, rows keep strip-then-pane order (D2).
    all.sort_by_key(|r| r.bucket);
    let count = |b: Bucket| all.iter().filter(|r| r.bucket == b).count();
    let counts = Counts {
        all: all.len(),
        needs_you: count(Bucket::NeedsYou),
        working: count(Bucket::Working),
        idle: count(Bucket::Idle),
    };
    let banner = pick_banner(&all);
    let kept = expanded.intersection(&live).copied().collect();
    let model = OverviewModel {
        resolving: false,
        rows: all
            .into_iter()
            .filter(|r| admits(filter, r.bucket))
            .collect(),
        counts,
        banner,
        filter,
        total_subagents: totals,
        view: View::Overview,
    };
    (model, kept)
}

/// Whether `filter` shows a row in `bucket` (D6): `All` shows every row, `Agent` rows
/// included; each other tab shows its own bucket only.
pub fn admits(filter: Filter, bucket: Bucket) -> bool {
    match filter {
        Filter::All => true,
        Filter::NeedsYou => bucket == Bucket::NeedsYou,
        Filter::Working => bucket == Bucket::Working,
        Filter::Idle => bucket == Bucket::Idle,
    }
}

/// One session's identity and the log its digest is read from — `None` for a non-Claude
/// session (R14.3). What the overlay needs without formatting a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRef {
    pub tab: u64,
    /// The tmux pane pid, `0` for an unsplit tab.
    pub pane: u32,
    pub log: Option<PathBuf>,
}

/// Every session of `rows`, in strip-then-pane order — exactly the rows [`build_overview`]
/// would build (D14). The overlay reads it for the telemetry thread's log set (D19), for
/// whether a press's `(tab, pane)` still exists (D10), and for a detail's digest.
pub fn session_refs(rows: &[SidebarRow], metas: &HashMap<TabId, TabMeta>) -> Vec<SessionRef> {
    rows.iter()
        .flat_map(|row| {
            sessions_of(row, metas.get(&TabId(row.tab_id)), None, false)
                .into_iter()
                .map(|s| SessionRef {
                    tab: s.tab,
                    pane: s.pane,
                    log: s.log.map(Path::to_path_buf),
                })
        })
        .collect()
}

/// What [`now_doing`] reads for one row (D4).
#[derive(Clone, Copy, Debug)]
pub struct NowDoingInput<'a> {
    pub bucket: Bucket,
    /// The blocking dialog's subject, when one is on screen (D9).
    pub subject: Option<&'a DialogSubject>,
    /// The session log's digest — `None` for a non-Claude session or an unread log.
    pub digest: Option<&'a SessionDigest>,
}

/// The Now doing column's first line (D4).
pub fn now_doing(row: &NowDoingInput<'_>) -> String {
    match row.bucket {
        Bucket::NeedsYou => match row.subject {
            Some(DialogSubject::Command(c)) => format!("Approval needed · {c}"),
            Some(DialogSubject::Question) => "Question waiting".to_string(),
            Some(DialogSubject::Plan) => "Plan approval waiting".to_string(),
            None => "Waiting for your input".to_string(),
        },
        Bucket::Idle => "Turn finished · ready for next prompt".to_string(),
        Bucket::Agent => "Agent running".to_string(),
        Bucket::Working => match row.digest {
            Some(d) => working_text(d),
            None => "Working".to_string(),
        },
    }
}

/// D4's Working text from a digest: waiting on running subagents unless a work tool finished
/// after the newest of them started (equal counts as waiting), else the newest finished tool.
fn working_text(d: &SessionDigest) -> String {
    let running: Vec<&Subagent> = d.subagents.iter().filter(|a| !a.completed).collect();
    if !running.is_empty() {
        let newest_start = running.iter().filter_map(|a| a.start_ms).max();
        let waiting = match &d.last_work_tool {
            None => true,
            Some((_, _, start)) => *start <= newest_start,
        };
        if waiting {
            let k = running.len();
            let noun = if k == 1 { "subagent" } else { "subagents" };
            return format!("Waiting on {k} {noun}");
        }
    }
    match &d.last_tool {
        Some((name, Some(summary), _)) => format!("{name} · {summary}"),
        Some((name, None, _)) => name.clone(),
        None => "Working".to_string(),
    }
}

/// Time since the latest human prompt: `<1m`, `{m}m`, `{h}h {m}m`, or `—` with none (D7).
pub fn format_elapsed(last_prompt_ms: Option<u64>, now_ms: u64) -> String {
    let Some(at) = last_prompt_ms else {
        return DASH.to_string();
    };
    let mins = now_ms.saturating_sub(at) / 60_000;
    match mins {
        0 => "<1m".to_string(),
        m if m < 60 => format!("{m}m"),
        m => format!("{}h {}m", m / 60, m % 60),
    }
}

/// A row's subagent squares (at most two rows, overflow as `+{k}`) and its
/// `{r} running · {d} done` summary (D8).
///
/// `subagents` are taken in the order given — [`build_overview`] hands them in log order —
/// and every one gets a square: how many fit on two lines depends on the column's width
/// (D15), so the trailing `+{k}` is the layout's to place (R5.2).
pub fn subagent_summary(subagents: &[Subagent], now_ms: u64) -> (Vec<SubagentCell>, String) {
    // No square or summary depends on the clock; the parameter is the contract's (T4).
    let _ = now_ms;
    summarize(&subagents.iter().collect::<Vec<_>>())
}

fn summarize(subagents: &[&Subagent]) -> (Vec<SubagentCell>, String) {
    if subagents.is_empty() {
        return (Vec::new(), DASH.to_string());
    }
    let cells: Vec<SubagentCell> = subagents
        .iter()
        .map(|a| SubagentCell::Square(status_of(a)))
        .collect();
    let running = subagents.iter().filter(|a| !a.completed).count();
    let done = subagents.len() - running;
    let summary = match running {
        0 => format!("{done} done"),
        r => format!("{r} running · {done} done"),
    };
    (cells, summary)
}

/// The banner for the first Needs-you row of `rows` (already sorted), with the count of the
/// others; `None` when no row needs the user (D5).
pub fn pick_banner(rows: &[SessionRowData]) -> Option<Banner> {
    let mut waiting = rows.iter().filter(|r| r.bucket == Bucket::NeedsYou);
    let first = waiting.next()?;
    Some(Banner {
        tab: first.tab,
        pane: first.pane,
        text: format!(
            "{} · {} — {}",
            first.agent_label, first.project, first.now_doing
        ),
        more: waiting.count(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabmeta::{AgentState, PaneMeta};

    const NOW: u64 = 1_793_404_800_000;

    fn sub(id: &str, start: Option<u64>, end: Option<u64>, completed: bool, ok: bool) -> Subagent {
        Subagent {
            id: id.to_string(),
            subagent_type: None,
            description: None,
            model: None,
            start_ms: start,
            end_ms: end,
            completed,
            agent_id: None,
            ended_ok: ok,
            report_preview: None,
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
            title: format!("tab{tab}"),
            ..SidebarRow::default()
        }
    }

    fn build(
        rows: &[SidebarRow],
        metas: &HashMap<TabId, TabMeta>,
        digests: &HashMap<PathBuf, SessionDigest>,
        expanded: &ExpandSet,
        active: usize,
    ) -> (OverviewModel, ExpandSet) {
        build_overview(
            rows,
            metas,
            None,
            digests,
            expanded,
            Filter::All,
            active,
            NOW,
        )
    }

    #[test]
    fn buckets_split_waiting_by_why_it_waits() {
        use CardStatus as C;
        assert_eq!(bucket_of(C::WaitingForInput, true, false), Bucket::NeedsYou);
        assert_eq!(bucket_of(C::WaitingForInput, false, true), Bucket::NeedsYou);
        assert_eq!(bucket_of(C::WaitingForInput, false, false), Bucket::Idle);
        assert_eq!(bucket_of(C::Ready, true, true), Bucket::Idle);
        assert_eq!(bucket_of(C::Working, true, true), Bucket::Working);
        assert_eq!(bucket_of(C::Hidden, false, false), Bucket::Agent);
    }

    #[test]
    fn the_newest_first_ring_reads_in_log_order() {
        // The ring as the snapshot publishes it: newest first.
        let ring = vec![
            sub("c", Some(30), None, false, true),
            sub("b", Some(20), Some(25), true, false),
            sub("a", Some(10), Some(15), true, true),
        ];
        let ids: Vec<&str> = log_order(&ring).iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        // Already in log order: unchanged.
        let fwd: Vec<Subagent> = ring.iter().rev().cloned().collect();
        let ids: Vec<&str> = log_order(&fwd).iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        // Equal starts keep log order (the ring reversed).
        let tie = vec![
            sub("y", Some(5), None, false, true),
            sub("x", Some(5), None, false, true),
        ];
        let ids: Vec<&str> = log_order(&tie).iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["x", "y"]);
    }

    #[test]
    fn tree_durations_by_state() {
        let fin = sub(
            "f",
            Some(NOW - 200_000),
            Some(NOW - 200_000 + 65_000),
            true,
            true,
        );
        assert_eq!(subagent_duration(&fin, NOW), "1m 05s");
        let run = sub("r", Some(NOW - 3_700_000), None, false, true);
        assert_eq!(subagent_duration(&run, NOW), "1h 1m");
        let no_end = sub("n", Some(NOW - 1_000), None, true, true);
        assert_eq!(subagent_duration(&no_end, NOW), "—");
        let l = tree_line(&no_end, NOW);
        assert_eq!(
            (l.subagent_type.as_str(), l.description.as_str()),
            ("agent", "—")
        );
        assert_eq!(l.status_word, "done");
    }

    #[test]
    fn elapsed_boundaries() {
        assert_eq!(format_elapsed(Some(NOW - 59_999), NOW), "<1m");
        assert_eq!(format_elapsed(Some(NOW - 60_000), NOW), "1m");
        assert_eq!(format_elapsed(Some(NOW - 3_599_999), NOW), "59m");
        assert_eq!(format_elapsed(Some(NOW - 3_600_000), NOW), "1h 0m");
        // A prompt stamped after `now` (clock skew) is not negative.
        assert_eq!(format_elapsed(Some(NOW + 5_000), NOW), "<1m");
    }

    #[test]
    fn working_with_running_subagents_but_no_start_times_compares_as_none() {
        let d = SessionDigest {
            subagents: vec![sub("a", None, None, false, true)],
            last_tool: Some(("Read".into(), Some("x.rs".into()), Some(1))),
            last_work_tool: Some(("Read".into(), Some("x.rs".into()), Some(1))),
            ..SessionDigest::default()
        };
        let input = NowDoingInput {
            bucket: Bucket::Working,
            subject: None,
            digest: Some(&d),
        };
        assert_eq!(now_doing(&input), "Read · x.rs");
    }

    /// D4: the "waiting on subagents" comparison reads `last_work_tool`, never `last_tool`.
    /// An `Agent` launch newer than every running subagent is not work the main agent did
    /// since, so it must not turn the text back into a tool line.
    #[test]
    fn the_waiting_comparison_ignores_a_newer_agent_launch() {
        let d = SessionDigest {
            subagents: vec![
                sub("a", Some(1000), None, false, true),
                sub("b", Some(900), None, false, true),
            ],
            last_tool: Some(("Agent".into(), Some("Spec review".into()), Some(1001))),
            last_work_tool: Some(("Bash".into(), Some("cargo test".into()), Some(900))),
            ..SessionDigest::default()
        };
        let input = NowDoingInput {
            bucket: Bucket::Working,
            subject: None,
            digest: Some(&d),
        };
        assert_eq!(now_doing(&input), "Waiting on 2 subagents");
    }

    #[test]
    fn non_claude_logs_are_never_read_and_the_subject_needs_the_dialog() {
        let log = PathBuf::from("/nonexistent/overview/x.jsonl");
        let codex = Agent {
            kind: AgentKind::Codex,
            state: AgentState::Busy,
        };
        let metas = HashMap::from([(
            TabId(1),
            TabMeta {
                agent: Some(codex),
                log: Some(log.clone()),
                ..TabMeta::default()
            },
        )]);
        let digests = HashMap::from([(
            log,
            SessionDigest {
                last_prompt_ms: Some(NOW - 120_000),
                subagents: vec![sub("a", Some(1), None, false, true)],
                ..SessionDigest::default()
            },
        )]);
        // A stale subject with no dialog flag is ignored.
        let r = SidebarRow {
            blocking_subject: Some(DialogSubject::Plan),
            ..row(1)
        };
        let (m, _) = build(&[r], &metas, &digests, &ExpandSet::new(), 0);
        assert_eq!(m.rows.len(), 1);
        assert_eq!(m.rows[0].elapsed, "—");
        assert_eq!(m.rows[0].subagent_summary, "—");
        assert_eq!(m.rows[0].now_doing, "Working");
        assert_eq!(m.total_subagents, SubagentTotals::default());
    }

    #[test]
    fn selection_marks_the_active_tab_and_its_active_pane_only() {
        let a = claude(AgentState::Busy);
        let pane = |pid, active| PaneMeta {
            pane_pid: pid,
            active,
            agent: Some(a),
            log: None,
            cwd: None,
            branch: None,
            repo: None,
            graph_asks_user: false,
            graph_agent_cwd: None,
        };
        let metas = HashMap::from([
            (
                TabId(1),
                TabMeta {
                    agent: Some(a),
                    ..TabMeta::default()
                },
            ),
            (
                TabId(2),
                TabMeta {
                    agent: Some(a),
                    panes: vec![pane(20, true), pane(21, false)],
                    ..TabMeta::default()
                },
            ),
        ]);
        let (m, _) = build(
            &[row(1), row(2)],
            &metas,
            &HashMap::new(),
            &ExpandSet::new(),
            1,
        );
        let sel: Vec<(u64, u32, bool)> =
            m.rows.iter().map(|r| (r.tab, r.pane, r.selected)).collect();
        assert_eq!(sel, vec![(1, 0, false), (2, 20, true), (2, 21, false)]);
    }

    #[test]
    fn the_tree_is_filled_for_any_row_with_subagents_and_expanded_follows_the_set() {
        let log = PathBuf::from("/nonexistent/overview/t.jsonl");
        let metas = HashMap::from([(
            TabId(1),
            TabMeta {
                agent: Some(claude(AgentState::Busy)),
                log: Some(log.clone()),
                ..TabMeta::default()
            },
        )]);
        let digests = HashMap::from([(
            log,
            SessionDigest {
                subagents: vec![sub("a", Some(NOW - 1), None, false, true)],
                ..SessionDigest::default()
            },
        )]);
        let (m, kept) = build(&[row(1)], &metas, &digests, &ExpandSet::new(), 0);
        assert_eq!(m.rows[0].tree.len(), 1);
        assert!(!m.rows[0].expanded);
        assert!(kept.is_empty());
        let set = ExpandSet::from([(TabId(1), 0), (TabId(9), 0)]);
        let (m, kept) = build(&[row(1)], &metas, &digests, &set, 0);
        assert!(m.rows[0].expanded);
        assert_eq!(kept, ExpandSet::from([(TabId(1), 0)]));
    }
}
