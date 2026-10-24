//! The app-side sidebar view model (D10/D11): the single place that joins
//! `tab-metadata`'s off-thread cache to the renderer's per-row [`RowMeta`].
//!
//! [`SidebarMetaSource`] implements the renderer's [`MetaSource`] pull trait — the layer
//! calls [`generation`](MetaSource::generation) each frame and [`fill`](MetaSource::fill)
//! only when it moves (D10), so a steady frame does no work. [`skin_for`] is the **total,
//! exhaustive** mapping from an app-side agent identity to the renderer's `AgentSkin`
//! (D11): adding an `AgentKind` variant fails to compile here, at the one site that must
//! decide, rather than silently rendering as nothing. [`resolve_status`] is the card's
//! bottom-left status by its precedence (sidebar-weather-cards D7, as amended by
//! agent-card-status D8), and [`effective_state`] is the one dialog override it shares with
//! the waiting band (agent-card-status D4).

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use renderer::sidebar_card::CardStatus;
use renderer::sidebar_row::{MetaSource, RowMeta, SidebarRow};
use renderer::sidebar_theme::AgentSkin;

use crate::session::TabId;
use crate::tabmeta::{Agent, AgentKind, AgentState, TabMetaHandle, TabMetaSnapshot};

/// Pulls each row's branch + agent state from `tab-metadata`'s generation-stamped cache
/// and maps them to `RowMeta` (D10). Joined by `tab_id`, never by index — a tab closed
/// between assembly and draw degrades to a plain card, never a mis-attributed branch.
pub struct SidebarMetaSource {
    handle: TabMetaHandle,
    /// Reused across frames so `sync`'s refill keeps its allocation (D2).
    snap: RefCell<TabMetaSnapshot>,
}

impl SidebarMetaSource {
    pub fn new(handle: TabMetaHandle) -> SidebarMetaSource {
        SidebarMetaSource {
            handle,
            snap: RefCell::new(TabMetaSnapshot::default()),
        }
    }
}

impl MetaSource for SidebarMetaSource {
    fn generation(&self) -> u64 {
        self.handle.generation()
    }

    fn fill(&self, rows: &[SidebarRow], out: &mut Vec<RowMeta>) {
        // Refresh the snapshot (a no-op allocation-wise when the generation is unchanged).
        self.handle.sync(&mut self.snap.borrow_mut());
        let snap = self.snap.borrow();
        // `$HOME` at the point of use, once per fill (sidebar-weather-cards D10), as
        // `clipboard::pasted_dir` reads it — never cached, so it is never stale.
        let home = std::env::var_os("HOME").map(PathBuf::from);
        out.clear();
        for row in rows {
            out.push(row_meta(row, snap.get(TabId(row.tab_id)), home.as_deref()));
        }
    }
}

/// One row's live metadata: the join of what `Tabs` assembled (`row`) with what the metadata
/// worker last published for that tab (`meta`).
///
/// Free and pure so the join is unit-testable without a worker thread or a GPU frame —
/// `TabMetaSnapshot`'s fields are private to its own module, so a test can construct this
/// function's inputs and not `fill`'s.
///
/// `pub` for the session graph (session-flow-graph D3): its session cards take their
/// status from this same join, which is what makes them agree with the card by
/// construction rather than by a second classifier.
pub fn row_meta(
    row: &SidebarRow,
    meta: Option<&crate::tabmeta::TabMeta>,
    home: Option<&Path>,
) -> RowMeta {
    row_meta_with(
        row,
        meta,
        meta.and_then(|m| m.agent.as_ref()),
        meta.and_then(|m| m.task.as_deref()),
        home,
    )
}

/// [`row_meta`] with the agent and task label given rather than read from `meta.agent` /
/// `meta.task` — the session graph's entry, which passes `TabMeta::graph_agent` /
/// `graph_task` so the graph ignores `[sidebar].show_agent_status` (user decision 2026-10-22)
/// while its status still comes from this one join and [`resolve_status`]. Every other field
/// is read from `meta` exactly as [`row_meta`] reads it.
pub fn row_meta_with(
    row: &SidebarRow,
    meta: Option<&crate::tabmeta::TabMeta>,
    agent: Option<&Agent>,
    task: Option<&str>,
    home: Option<&Path>,
) -> RowMeta {
    // Resolve line 1 HERE (sticky-tab-title D7/R1.5): this is the one place that
    // sees all four sources at once with the AI-task label *fresh*. `Tabs::titles`
    // cannot do it — it runs on tab actions, while the task label arrives on this
    // per-frame pull, so resolving there would freeze the auto label until the
    // next tab switch.
    // The folder name belongs on this pull for exactly the same reason, and did not used to
    // be here (card-live-cwd D1). `TabMeta::cwd` is republished whenever the shell moves,
    // while `row.title` was resolved by `Tabs::titles` on the last tab action — so reading
    // the row's copy froze the card on the directory its tab opened in, and a bare `cd` never
    // reached it. The row's title remains the answer for the not-yet-polled window, where it
    // is the only one there is (D2).
    let live_path = meta.and_then(|m| m.cwd.as_deref());
    let live_cwd = folder_name(live_path);
    let cwd = live_cwd.as_deref().unwrap_or(&row.title);
    let (line1, _) = resolve_line1_tier(row.label.as_deref(), task, row.program.as_deref(), cwd);
    RowMeta {
        branch: meta.and_then(|m| m.branch.clone()),
        status: resolve_status(agent, row.blocking_dialog, row.running, row.last_exit),
        skin: skin_for(agent),
        line1: line1.to_string(),
        // The folder line (sidebar-weather-cards D10): the SAME published cwd line 1's cwd
        // tier and the branch come from, on the same pull, so a card's folder and branch
        // always describe one directory (card-live-cwd D1). Empty before the first publish;
        // the card then shows the row's assembly-time folder name (card-live-cwd D2).
        cwd_display: live_path.map(|p| display_cwd(p, home)).unwrap_or_default(),
    }
}

/// The folder line's text for a working directory: `$HOME` shown as `~`, anything else
/// absolute (sidebar-weather-cards D10) — `~/IdeaProjects/terminal`, `~`, `/opt/homebrew`.
///
/// Component-wise, never a string prefix: `/Users/xy` is not under `/Users/x`. A `home`
/// that is relative, empty or the root is ignored — abbreviating by it would turn every
/// path into `~/…` and name nothing.
pub fn display_cwd(cwd: &Path, home: Option<&Path>) -> String {
    let home = home.filter(|h| h.is_absolute() && h.parent().is_some());
    match home.and_then(|h| cwd.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => cwd.display().to_string(),
    }
}

/// The folder name a card shows for a working directory: the path's final component,
/// lossily decoded (card-live-cwd D7).
///
/// One implementation for every surface that names a tab's directory — the sidebar row, the
/// card's live pull, the waiting band, the rename prefill — so they cannot disagree about
/// what "the folder" is. That divergence is not hypothetical: this derivation was copied at
/// two sites, and the copy that stopped being refreshed is the bug this feature fixes.
///
/// `None` when there is no cwd (an unpolled or dead tab) or when the path has no final
/// component (`/`). The fallback stays with the **caller** because the four disagree: a row
/// wants `Tab {n}`, the rename prefill wants an empty field, and the card and band want the
/// row's own title (R4.2).
pub fn folder_name(cwd: Option<&Path>) -> Option<String> {
    cwd?.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Line 1 of a card, by precedence **user > AI-task > program-OSC > cwd**
/// (sticky-tab-title D5/D12, R1.2).
///
/// The whole feature is this ordering. `user` is the sticky title the user set — nothing
/// but another user action displaces it, which is why a program's OSC 0/2 cannot reach
/// line 1 while it is set (D3). `task` is the live AI-task label; it outranks `program`
/// unconditionally (D12) because a program-set title is a weak ambient signal while the
/// task label is the tab's identity — an idle agent deliberately keeps its last label
/// (that afterglow is identity, not a liveness claim; liveness is the card's mark).
/// `program` is the last OSC 0/2 any program set, and `cwd` is the folder basename.
///
/// Pure and app-side so the policy is unit-tested without a GPU frame, and so the
/// renderer keeps no precedence branch of its own (D7).
///
/// Returns the resolved text **and which tier won** — not whether the winning string
/// happens to differ from the cwd. That distinction is load-bearing for line 2 (tab-identity R2.2): a card whose line 1 is
/// custom moves the repo/folder name down beside the branch. Inferring "custom" by
/// comparing the resolved string to the cwd gets it wrong exactly when a user names a tab
/// after its own folder — a natural thing to do — and silently drops the repo name from
/// line 2 for that card.
pub fn resolve_line1_tier<'a>(
    user: Option<&'a str>,
    task: Option<&'a str>,
    program: Option<&'a str>,
    cwd: &'a str,
) -> (&'a str, bool) {
    // Empty is treated as absent at every tier: a program that emits `ESC]2;ST` has
    // cleared its title, not set a blank one, and the same holds for a blank task label.
    let non_empty = |s: &'a str| (!s.is_empty()).then_some(s);
    match user
        .and_then(non_empty)
        .or_else(|| task.and_then(non_empty))
        .or_else(|| program.and_then(non_empty))
    {
        Some(custom) => (custom, true),
        None => (cwd, false),
    }
}

/// Agent identity → card skin (D11). Claude, Antigravity and — since sidebar-weather-cards
/// D5 — Codex carry skins of their own; Gemini, Aider and `None` → `Generic`, the plain-
/// shell card. The `match` is exhaustive with no `_` arm on purpose (R9.3): a new
/// `AgentKind` must be decided here.
pub fn skin_for(agent: Option<&Agent>) -> AgentSkin {
    match agent.map(|a| a.kind) {
        Some(AgentKind::Claude) => AgentSkin::Claude,
        Some(AgentKind::Antigravity) => AgentSkin::Antigravity,
        Some(AgentKind::Codex) => AgentSkin::Codex,
        Some(AgentKind::Gemini) | Some(AgentKind::Aider) => AgentSkin::Generic,
        None => AgentSkin::Generic,
    }
}

/// An agent's state as the band and the card both read it: `Waiting` while a blocking
/// dialog is on the tab's screen, whatever the log says (agent-card-status D4).
///
/// The log cannot see a pending permission — Claude Code appends nothing while its dialog
/// is up, and Antigravity writes a step only when it completes — while the screen can. The
/// point of this being ONE function is that the band (`push_agent_states`) and the card
/// ([`resolve_status`]) cannot disagree about a tab: before it, a Claude tab on a permission
/// dialog sat in the band while its card read `Working`.
pub fn effective_state(state: AgentState, blocking_dialog: bool) -> AgentState {
    if blocking_dialog {
        AgentState::Waiting
    } else {
        state
    }
}

/// The card's bottom-left status (sidebar-weather-cards D7, agent-card-status D8), first
/// match wins:
///
/// 1. an agent is present → its [`effective_state`]: a blocking dialog on screen reads
///    `WaitingForInput` first, then `Busy` → `Working`, `Idle` → `Ready`, `Waiting` →
///    `WaitingForInput`, and `Unknown` → [`CardStatus::Hidden`] (agent-card-status D3). An
///    agent started from zsh leaves the shell "running" for its whole session, which is
///    why the agent outranks that.
/// 2. the shell is running a foreground command → `Running` (D19: tmux and vim read so for
///    as long as they run).
/// 3. the last finished command failed by the band's own rule (D12) → `Exited N`.
/// 4. otherwise → the plain shell.
///
/// `Unknown` hides rather than claiming `Working`: that claim was sidebar-weather-cards
/// D7's least-wrong option when hiding was not one, and it is exactly what made an
/// Antigravity card blocked on a permission dialog read "Working". The dialog level is
/// ignored without an agent — the level is only ever set for an agent's tab (D7), and a
/// shell that prints dialog-like text must not change its card.
///
/// `Running` outranking `Exited` is what makes a rerun "clear on start" with no clearing
/// logic: the next command's start makes rule 2 win, and its end replaces `last_exit`.
pub fn resolve_status(
    agent: Option<&Agent>,
    blocking_dialog: bool,
    running: bool,
    last_exit: Option<i32>,
) -> CardStatus {
    match agent.map(|a| effective_state(a.state, blocking_dialog)) {
        Some(AgentState::Busy) => CardStatus::Working,
        Some(AgentState::Idle) => CardStatus::Ready,
        Some(AgentState::Waiting) => CardStatus::WaitingForInput,
        Some(AgentState::Unknown) => CardStatus::Hidden,
        None if running => CardStatus::Running,
        None => match last_exit.filter(|&code| crate::waiting::is_reportable_exit(code)) {
            Some(code) => CardStatus::Exited(code),
            None => CardStatus::Shell,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(kind: AgentKind, state: AgentState) -> Agent {
        Agent { kind, state }
    }

    #[test]
    fn fill_replaces_the_buffer_with_one_meta_per_row_in_order() {
        // D7 through the real source: before the worker publishes anything, each row's
        // status still comes from its own command state, one entry per row, in row
        // order, and whatever the buffer held before is gone.
        let Some(handle) = crate::tabmeta::TabMetaHandle::spawn(|| {}) else {
            panic!("the metadata worker could not be spawned");
        };
        let source = SidebarMetaSource::new(handle);
        let rows = [
            SidebarRow {
                tab_id: 1,
                running: true,
                ..SidebarRow::default()
            },
            SidebarRow {
                tab_id: 2,
                last_exit: Some(101),
                ..SidebarRow::default()
            },
        ];
        let mut out = vec![RowMeta::default(); 3];
        source.fill(&rows, &mut out);
        let got: Vec<CardStatus> = out.iter().map(|m| m.status).collect();
        assert_eq!(got, [CardStatus::Running, CardStatus::Exited(101)]);
    }

    // -----------------------------------------------------------------------
    // card-live-cwd R4.1 / R4.2 — one derivation, caller-owned fallbacks
    // -----------------------------------------------------------------------

    #[test]
    fn folder_name_is_the_final_component() {
        // R4.1 — the one definition every surface shares.
        assert_eq!(
            folder_name(Some(Path::new("/Users/x/web"))).as_deref(),
            Some("web")
        );
        assert_eq!(
            folder_name(Some(Path::new("/Users/x/web/"))).as_deref(),
            Some("web"),
            "a trailing separator is not a component"
        );
    }

    #[test]
    fn folder_name_carries_no_fallback_of_its_own() {
        // R4.2 — the four callers disagree about what absence means (`Tab {n}` for a row, an
        // empty rename field, the row title for the card and band), so the helper must return
        // `None` and let each supply its own. Baking one in here would silently impose the
        // wrong default on three of them.
        assert_eq!(folder_name(None), None, "no cwd -> no name");
        assert_eq!(
            folder_name(Some(Path::new("/"))),
            None,
            "root has no final component"
        );
    }

    // -----------------------------------------------------------------------
    // card-live-cwd R0-R2, R5 — the card reads the LIVE cwd, not the row's copy
    // -----------------------------------------------------------------------

    /// A row as `Tabs::titles` assembled it, with `title` standing in for the folder the tab
    /// was in at that moment.
    fn row_at(title: &str) -> SidebarRow {
        SidebarRow {
            tab_id: 7,
            title: title.to_string(),
            ..Default::default()
        }
    }

    /// The join with `$HOME` fixed at `/Users/x`, where every `meta_at` path lives.
    fn row_meta_at_home(row: &SidebarRow, meta: Option<&crate::tabmeta::TabMeta>) -> RowMeta {
        row_meta(row, meta, Some(Path::new("/Users/x")))
    }

    fn meta_at(cwd: &str) -> crate::tabmeta::TabMeta {
        crate::tabmeta::TabMeta {
            branch: None,
            agent: None,
            cwd: Some(std::path::PathBuf::from(cwd)),
            task: None,
            // Not what these tests are about — the card's folder name comes from `cwd`. Present
            // because `TabMeta` is destructured exhaustively by a pin in `tabmeta::worker`, so
            // every literal states every field deliberately.
            agent_pid: None,
            panes: Vec::new(),
            log: None,
            repo: None,
            graph_agent: None,
            graph_task: None,
        }
    }

    #[test]
    fn the_card_follows_a_cd_rather_than_the_assembled_row_title() {
        // R0.1 / R1.1 / R2.2 — the reported bug in one assertion. The row was assembled while
        // the tab sat in `api`; the shell has since moved to `web` and the worker published
        // it. Before this feature the card read `api` for the life of the tab, because
        // nothing re-runs `Tabs::titles` on a `cd`.
        let m = row_meta_at_home(&row_at("api"), Some(&meta_at("/Users/x/web")));
        assert_eq!(m.line1, "web");
        assert_eq!(m.cwd_display, "~/web");
    }

    #[test]
    fn line1_and_line2_cannot_name_two_different_folders() {
        // R1.2 — one resolved value feeds the label chain's cwd tier AND line 2's repo name.
        // A card whose line 1 is custom draws the folder on line 2, so two derivations here
        // would let one card name two directories — the defect one line down.
        let mut row = row_at("api");
        row.label = Some("Frontend".into());
        let m = row_meta_at_home(&row, Some(&meta_at("/Users/x/web")));
        assert_eq!(m.line1, "Frontend", "the user title still wins line 1");
        assert_eq!(
            m.cwd_display, "~/web",
            "and the folder line is the LIVE folder"
        );
    }

    #[test]
    fn a_renamed_tabs_folder_name_still_follows_the_shell() {
        // R0.3 — the sharper case. A user title suppresses `title_dirty` entirely
        // (`session.rs:136`), so even a shell that sets an OSC 0/2 title on every prompt never
        // triggers a row resync for a NAMED tab: its line-2 folder name was stale forever, not
        // merely until the next prompt.
        let mut row = row_at("api");
        row.label = Some("Frontend".into());
        row.program = Some("vim - main.rs".into());
        let before = row_meta_at_home(&row, Some(&meta_at("/Users/x/api")));
        let after = row_meta_at_home(&row, Some(&meta_at("/Users/x/web")));
        assert_eq!(before.cwd_display, "~/api");
        assert_eq!(after.cwd_display, "~/web");
        assert_eq!(
            after.line1, "Frontend",
            "the name the user set is untouched"
        );
    }

    #[test]
    fn the_folder_name_and_the_branch_come_from_one_snapshot() {
        // R0.2 — the visible symptom was old folder + NEW branch on one card, because the
        // branch was pulled live and the folder was not. Both now derive from the same
        // `TabMeta`, so they cannot describe different directories.
        let mut meta = meta_at("/Users/x/web");
        meta.branch = Some("dev".into());
        let m = row_meta_at_home(&row_at("api"), Some(&meta));
        assert_eq!(m.cwd_display, "~/web");
        assert_eq!(m.branch.as_deref(), Some("dev"));
    }

    #[test]
    fn the_folder_line_is_carried_for_a_plain_card_too() {
        // R1.3 / D6, as sidebar-weather-cards D10 left it — the folder line is always drawn,
        // so the value is always carried, whether or not line 1 is the cwd.
        let m = row_meta_at_home(&row_at("api"), Some(&meta_at("/Users/x/web")));
        assert_eq!(m.line1, "web", "a plain cwd card");
        assert_eq!(m.cwd_display, "~/web", "…still carries its folder line");
    }

    #[test]
    fn an_unpolled_tab_shows_the_assembled_title_rather_than_blank() {
        // R2.1 — the not-yet-published window (up to one worker tick on a new tab). The row's
        // own title is the only answer there is, and a blank card is the outcome D2 refuses.
        let m = row_meta_at_home(&row_at("api"), None);
        assert_eq!(m.line1, "api");
        // No published cwd → no folder line of its own; the card shows the row's title
        // there instead (`sidebar_card::folder_text`, R10.4).
        assert_eq!(m.cwd_display, "");
    }

    #[test]
    fn an_unresolvable_cwd_keeps_the_tab_n_fallback() {
        // R2.3 — a dead pid publishes `cwd: None`; the row's `Tab {n}` must survive it.
        let meta = crate::tabmeta::TabMeta::default();
        let m = row_meta_at_home(&row_at("Tab 3"), Some(&meta));
        assert_eq!(m.line1, "Tab 3");
        assert_eq!(
            m.cwd_display, "",
            "the card falls back to the row's `Tab 3`"
        );
    }

    #[test]
    fn the_join_performs_no_io() {
        // R5.1 — the pull consumes the PUBLISHED cwd and never re-resolves it. Asserted with
        // the project's own syscall counters rather than by inspection, because "it reads a
        // field" is exactly the kind of claim that silently stops being true.
        //
        // BOTH counters, deliberately. They cover different regressions and neither alone is
        // sufficient: `probe::PROC_CWD` counts the worker's resolver (`tabmeta::proc`), while a
        // join that reached for `Session::cwd` — the likelier mistake, since that is what the
        // row assembly does — would fall through `published_or` and bump `session::CWD_SYSCALLS`
        // instead. Watching only the first would leave this test unable to fail for the reason
        // it exists.
        let _g = crate::tabmeta::probe::serial();
        crate::tabmeta::probe::reset_all();
        let before = crate::session::CWD_SYSCALLS.with(|c| c.get());

        // A published cwd (the steady state) and an absent one (the fallback path) — the
        // fallback is where a re-resolve would be most tempting.
        let live = row_meta_at_home(&row_at("api"), Some(&meta_at("/Users/x/web")));
        let unpolled = row_meta_at_home(&row_at("api"), None);
        assert_eq!(live.cwd_display, "~/web");
        assert_eq!(unpolled.cwd_display, "");

        assert_eq!(
            crate::tabmeta::probe::PROC_CWD.get(),
            0,
            "the card must not resolve a cwd through the worker's resolver"
        );
        assert_eq!(
            crate::session::CWD_SYSCALLS.with(|c| c.get()),
            before,
            "…nor through `Session::cwd`'s fallback"
        );
    }

    #[test]
    fn two_directories_with_the_same_basename_are_told_apart_by_the_folder_line() {
        // card-live-cwd R5.3 / D4 pinned `~/a/src` → `~/b/src` as an accepted invisible
        // change. sidebar-weather-cards D10 reopens it deliberately: line 1 still names the
        // basename, but the folder line is the path, so the move is now visible.
        let a = row_meta_at_home(&row_at("x"), Some(&meta_at("/Users/x/a/src")));
        let b = row_meta_at_home(&row_at("x"), Some(&meta_at("/Users/x/b/src")));
        assert_eq!((a.line1.as_str(), b.line1.as_str()), ("src", "src"));
        assert_eq!(a.cwd_display, "~/a/src");
        assert_eq!(b.cwd_display, "~/b/src");
    }

    #[test]
    fn the_live_cwd_does_not_disturb_the_precedence_chain() {
        // R5.4 — the cwd tier is the BOTTOM of the chain; making it live must not let it
        // outrank anything. Task beats program, program beats cwd, exactly as before.
        let mut row = row_at("api");
        row.program = Some("vim - main.rs".into());
        let mut meta = meta_at("/Users/x/web");
        assert_eq!(
            row_meta_at_home(&row, Some(&meta)).line1,
            "vim - main.rs",
            "a program title still outranks the (now live) cwd"
        );
        meta.task = Some("refactor the parser".into());
        assert_eq!(
            row_meta_at_home(&row, Some(&meta)).line1,
            "refactor the parser",
            "and the task label still outranks the program title"
        );
    }

    /// The resolved line-1 text alone — most precedence tests care only about which
    /// string wins, not which tier it came from.
    fn resolve_line1<'a>(
        user: Option<&'a str>,
        task: Option<&'a str>,
        program: Option<&'a str>,
        cwd: &'a str,
    ) -> &'a str {
        resolve_line1_tier(user, task, program, cwd).0
    }

    // -----------------------------------------------------------------------
    // sticky-tab-title R1.2 / R1.6 — the label precedence chain
    // -----------------------------------------------------------------------

    #[test]
    fn user_title_outranks_every_other_source() {
        // R1.2 / R0.1 — the feature in one line: a named tab stays named no matter what
        // the agent is doing, what a program set, or where the shell is.
        assert_eq!(
            resolve_line1(
                Some("Frontend"),
                Some("refactor the parser"),
                Some("vim - main.rs"),
                "proj"
            ),
            "Frontend"
        );
    }

    #[test]
    fn chain_degrades_user_then_task_then_program_then_cwd() {
        // R1.2 — peel one tier at a time; each step exposes exactly the next source.
        assert_eq!(
            resolve_line1(
                None,
                Some("refactor the parser"),
                Some("vim - main.rs"),
                "proj"
            ),
            "refactor the parser"
        );
        assert_eq!(
            resolve_line1(None, None, Some("vim - main.rs"), "proj"),
            "vim - main.rs"
        );
        assert_eq!(resolve_line1(None, None, None, "proj"), "proj");
    }

    #[test]
    fn task_label_outranks_a_program_title() {
        // D12 — the deliberate change vs PR #13: a program's OSC 2 no longer beats the
        // AI-task label. Only an explicit user title does.
        assert_eq!(
            resolve_line1(None, Some("write the spec"), Some("~/proj"), "proj"),
            "write the spec"
        );
    }

    #[test]
    fn empty_is_absent_at_every_tier() {
        // A program emitting `ESC]2;ST` cleared its title rather than setting a blank one;
        // the same holds for a blank task label. An empty tier must not blank the card.
        assert_eq!(resolve_line1(Some(""), None, None, "proj"), "proj");
        assert_eq!(resolve_line1(None, Some(""), Some("vim"), "proj"), "vim");
        assert_eq!(resolve_line1(None, None, Some(""), "proj"), "proj");
    }

    #[test]
    fn plain_shell_tab_is_unchanged_from_today() {
        // R5.2 — the one un-named case D12 does NOT change: no user title, no agent, so
        // the program's title still wins over the cwd exactly as it did before.
        assert_eq!(
            resolve_line1(None, None, Some("~/other"), "proj"),
            "~/other"
        );
    }

    #[test]
    fn a_tab_named_after_its_own_folder_is_still_custom() {
        // tab-identity R2.2 — line 2 shows the repo/folder name only when line 1 is
        // custom. Deciding that by comparing the resolved string to the cwd gets it wrong
        // exactly when a user names a tab after its folder, silently dropping the repo
        // name from line 2. The winning TIER is the answer, not the winning string.
        let (text, custom) = resolve_line1_tier(Some("proj"), None, None, "proj");
        assert_eq!(text, "proj");
        assert!(custom, "a user title is custom even when it equals the cwd");
        // Same for a task label or a program title that happens to match the folder.
        assert!(resolve_line1_tier(None, Some("proj"), None, "proj").1);
        assert!(resolve_line1_tier(None, None, Some("proj"), "proj").1);
        // And a plain cwd card is correctly NOT custom.
        assert!(!resolve_line1_tier(None, None, None, "proj").1);
    }

    #[test]
    fn cleared_user_title_re_follows_the_live_auto_label() {
        // R4.2 / R2.3 — clearing does not freeze the card on the old value: with the user
        // title gone the live task label takes over again, as on a never-named tab.
        let named = resolve_line1(Some("Frontend"), Some("run the tests"), None, "proj");
        let cleared = resolve_line1(None, Some("run the tests"), None, "proj");
        assert_eq!(named, "Frontend");
        assert_eq!(cleared, "run the tests");
    }

    #[test]
    fn skin_mapping_is_total() {
        // R7.1–R7.3 / D11 — the two skinned agents map to their skins; the rest and None
        // fall back to Generic (a normal card).
        assert_eq!(
            skin_for(Some(&agent(AgentKind::Claude, AgentState::Busy))),
            AgentSkin::Claude
        );
        assert_eq!(
            skin_for(Some(&agent(AgentKind::Antigravity, AgentState::Idle))),
            AgentSkin::Antigravity
        );
        // sidebar-weather-cards R5.1 — Codex has its own skin now.
        assert_eq!(
            skin_for(Some(&agent(AgentKind::Codex, AgentState::Busy))),
            AgentSkin::Codex
        );
        for k in [AgentKind::Gemini, AgentKind::Aider] {
            assert_eq!(
                skin_for(Some(&agent(k, AgentState::Busy))),
                AgentSkin::Generic
            );
        }
        assert_eq!(skin_for(None), AgentSkin::Generic);
    }

    // -----------------------------------------------------------------------
    // sidebar-weather-cards R7 / R10 — the status precedence and the folder line
    // -----------------------------------------------------------------------

    const ALL_STATES: [AgentState; 4] = [
        AgentState::Busy,
        AgentState::Idle,
        AgentState::Waiting,
        AgentState::Unknown,
    ];

    #[test]
    fn an_agents_state_wins_over_everything() {
        // sidebar-weather-cards R7.1, agent-card-status R5.2 — whatever the shell is doing
        // (an agent started from zsh keeps it "running" for its whole session), a present
        // agent with no dialog reports its own state; Unknown states nothing (D3).
        for (state, want) in [
            (AgentState::Busy, CardStatus::Working),
            (AgentState::Unknown, CardStatus::Hidden),
            (AgentState::Idle, CardStatus::Ready),
            (AgentState::Waiting, CardStatus::WaitingForInput),
        ] {
            let a = agent(AgentKind::Claude, state);
            assert_eq!(
                resolve_status(Some(&a), false, true, Some(101)),
                want,
                "{state:?}"
            );
            assert_eq!(
                resolve_status(Some(&a), false, false, None),
                want,
                "{state:?}"
            );
        }
    }

    #[test]
    fn a_dialog_outranks_every_agent_state() {
        // agent-card-status R5.1 — the screen sees a pending permission the log cannot, so
        // it outranks whatever the log says, for every kind of agent.
        for kind in [AgentKind::Claude, AgentKind::Antigravity, AgentKind::Codex] {
            for state in ALL_STATES {
                let a = agent(kind, state);
                assert_eq!(
                    resolve_status(Some(&a), true, false, None),
                    CardStatus::WaitingForInput,
                    "{kind:?} {state:?}"
                );
            }
        }
    }

    #[test]
    fn without_an_agent_a_dialog_changes_nothing() {
        // agent-card-status R5.3 — a shell that prints dialog-like text keeps its card.
        for dialog in [false, true] {
            assert_eq!(
                resolve_status(None, dialog, true, None),
                CardStatus::Running
            );
            assert_eq!(
                resolve_status(None, dialog, false, Some(2)),
                CardStatus::Exited(2)
            );
            assert_eq!(resolve_status(None, dialog, false, None), CardStatus::Shell);
        }
    }

    #[test]
    fn the_effective_state_is_the_dialog_override() {
        // agent-card-status R8.1 — `Waiting` with a dialog, the state untouched without.
        for state in ALL_STATES {
            assert_eq!(
                effective_state(state, true),
                AgentState::Waiting,
                "{state:?}"
            );
            assert_eq!(effective_state(state, false), state, "{state:?}");
        }
    }

    #[test]
    fn the_row_join_reads_the_rows_dialog_level() {
        // agent-card-status R8.3 — the level rides on the row (D7), the agent on the
        // metadata; the card reads the same stored level the band was fed.
        let mut meta = meta_at("/Users/x/api");
        meta.agent = Some(agent(AgentKind::Antigravity, AgentState::Busy));
        let mut row = row_at("api");
        assert_eq!(
            row_meta_at_home(&row, Some(&meta)).status,
            CardStatus::Working
        );
        row.blocking_dialog = true;
        assert_eq!(
            row_meta_at_home(&row, Some(&meta)).status,
            CardStatus::WaitingForInput
        );
    }

    #[test]
    fn running_outranks_a_remembered_failure() {
        // R7.2 / D19 — the rerun clears `Exited` on start, with no clearing logic.
        assert_eq!(
            resolve_status(None, false, true, Some(101)),
            CardStatus::Running
        );
        assert_eq!(resolve_status(None, false, true, None), CardStatus::Running);
    }

    #[test]
    fn a_failed_last_command_shows_its_code() {
        // R7.3.
        assert_eq!(
            resolve_status(None, false, false, Some(101)),
            CardStatus::Exited(101)
        );
        assert_eq!(
            resolve_status(None, false, false, Some(2)),
            CardStatus::Exited(2)
        );
        assert_eq!(
            resolve_status(None, false, false, Some(-1)),
            CardStatus::Exited(-1)
        );
    }

    #[test]
    fn ignored_codes_and_no_history_read_as_the_shell() {
        // R7.4 / D12 — the band's ignore-list: success, grep's no-match, Ctrl-C.
        for code in [0, 1, 130] {
            assert_eq!(
                resolve_status(None, false, false, Some(code)),
                CardStatus::Shell,
                "{code}"
            );
        }
        assert_eq!(resolve_status(None, false, false, None), CardStatus::Shell);
    }

    #[test]
    fn the_row_join_resolves_the_status_from_the_rows_command_state() {
        // D8 → D7 — `running` / `last_exit` ride on the row, the agent on the metadata.
        let mut row = row_at("api");
        row.last_exit = Some(101);
        assert_eq!(
            row_meta_at_home(&row, Some(&meta_at("/Users/x/api"))).status,
            CardStatus::Exited(101)
        );
        row.running = true;
        assert_eq!(
            row_meta_at_home(&row, Some(&meta_at("/Users/x/api"))).status,
            CardStatus::Running
        );
        let mut meta = meta_at("/Users/x/api");
        meta.agent = Some(agent(AgentKind::Codex, AgentState::Busy));
        let m = row_meta_at_home(&row, Some(&meta));
        assert_eq!((m.status, m.skin), (CardStatus::Working, AgentSkin::Codex));
    }

    #[test]
    fn home_is_abbreviated_component_wise() {
        // R10.1.
        let home = Some(Path::new("/Users/x"));
        assert_eq!(
            display_cwd(Path::new("/Users/x/IdeaProjects/terminal"), home),
            "~/IdeaProjects/terminal"
        );
        assert_eq!(display_cwd(Path::new("/Users/x"), home), "~");
        assert_eq!(
            display_cwd(Path::new("/Users/x/"), home),
            "~",
            "a trailing slash"
        );
        assert_eq!(
            display_cwd(Path::new("/Users/xy/p"), home),
            "/Users/xy/p",
            "a string prefix is not a component prefix"
        );
        assert_eq!(
            display_cwd(Path::new("/Users/x/a"), Some(Path::new("/Users/x/"))),
            "~/a",
            "a HOME with a trailing slash still matches"
        );
    }

    #[test]
    fn no_home_or_outside_home_stays_absolute() {
        // R10.2 — and a degenerate HOME abbreviates nothing.
        let p = Path::new("/opt/homebrew");
        assert_eq!(display_cwd(p, Some(Path::new("/Users/x"))), "/opt/homebrew");
        assert_eq!(display_cwd(Path::new("/Users/x/a"), None), "/Users/x/a");
        for bad in ["", "/", "Users/x"] {
            assert_eq!(
                display_cwd(Path::new("/Users/x/a"), Some(Path::new(bad))),
                "/Users/x/a",
                "HOME={bad:?} is ignored"
            );
        }
    }
}
