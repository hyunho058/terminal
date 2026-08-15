//! The app-side sidebar view model (D10/D11): the single place that joins
//! `tab-metadata`'s off-thread cache to the renderer's per-row [`RowMeta`].
//!
//! [`SidebarMetaSource`] implements the renderer's [`MetaSource`] pull trait — the layer
//! calls [`generation`](MetaSource::generation) each frame and [`fill`](MetaSource::fill)
//! only when it moves (D10), so a steady frame does no work. [`skin_for`] / [`mark_for`]
//! are the **total, exhaustive** mapping from an app-side agent identity to the renderer's
//! `AgentSkin` / `RowMark` (D11): adding an `AgentKind` variant fails to compile here, at
//! the one site that must decide, rather than silently rendering as nothing.

use std::cell::RefCell;
use std::path::Path;

use renderer::sidebar_row::{MetaSource, RowMark, RowMeta, SidebarRow};
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
        out.clear();
        for row in rows {
            out.push(row_meta(row, snap.get(TabId(row.tab_id))));
        }
    }
}

/// One row's live metadata: the join of what `Tabs` assembled (`row`) with what the metadata
/// worker last published for that tab (`meta`).
///
/// Free and pure so the join is unit-testable without a worker thread or a GPU frame —
/// `TabMetaSnapshot`'s fields are private to its own module, so a test can construct this
/// function's inputs and not `fill`'s.
fn row_meta(row: &SidebarRow, meta: Option<&crate::tabmeta::TabMeta>) -> RowMeta {
    let agent = meta.and_then(|m| m.agent.as_ref());
    // Resolve line 1 HERE (sticky-tab-title D7/R1.5): this is the one place that
    // sees all four sources at once with the AI-task label *fresh*. `Tabs::titles`
    // cannot do it — it runs on tab actions, while the task label arrives on this
    // per-frame pull, so resolving there would freeze the auto label until the
    // next tab switch.
    let task = meta.and_then(|m| m.task.as_deref());
    // The folder name belongs on this pull for exactly the same reason, and did not used to
    // be here (card-live-cwd D1). `TabMeta::cwd` is republished whenever the shell moves,
    // while `row.title` was resolved by `Tabs::titles` on the last tab action — so reading
    // the row's copy froze the card on the directory its tab opened in, and a bare `cd` never
    // reached it. The row's title remains the answer for the not-yet-polled window, where it
    // is the only one there is (D2).
    let live_cwd = meta.and_then(|m| folder_name(m.cwd.as_deref()));
    let cwd = live_cwd.as_deref().unwrap_or(&row.title);
    let (line1, is_custom) =
        resolve_line1_tier(row.label.as_deref(), task, row.program.as_deref(), cwd);
    RowMeta {
        branch: meta.and_then(|m| m.branch.clone()),
        mark: mark_for(agent),
        skin: skin_for(agent),
        line1_is_custom: is_custom,
        line1: line1.to_string(),
        // Carried even when line 1 already won it (D6): line 2 draws the folder name only on
        // a custom card, and which card that is changes as the label sources come and go.
        // A copy, not a move — `cwd` is a `&str` and `line1` may borrow from it.
        cwd_title: cwd.to_string(),
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

/// Agent identity → card skin (D11). `Claude`/`Antigravity` carry the mock's two skins;
/// every other agent and `None` → `Generic` (the plain card). The `match` is exhaustive
/// with no `_` arm on purpose (R9.3): a new `AgentKind` must be decided here.
pub fn skin_for(agent: Option<&Agent>) -> AgentSkin {
    match agent.map(|a| a.kind) {
        Some(AgentKind::Claude) => AgentSkin::Claude,
        Some(AgentKind::Antigravity) => AgentSkin::Antigravity,
        Some(AgentKind::Codex) | Some(AgentKind::Gemini) | Some(AgentKind::Aider) => {
            AgentSkin::Generic
        }
        None => AgentSkin::Generic,
    }
}

/// Agent state → the leading mark (D11). Working → `Agent` (`✳`); idle/ready → `Ready`
/// (`◇`); no agent → `None`. `Unknown` maps to the **present** affordance (`✳`), never
/// `Ready` (R6.6) — the one thing `Unknown` asserts is that an agent is running.
///
/// `Waiting` (input-waiting, D4/D16) maps to its own [`RowMark::Waiting`] (the ⏸ pause in
/// the waiting colour, D6), so a waiting agent's CARD shows the same affordance as its
/// waiting-band row — a consistent read across the two zones.
pub fn mark_for(agent: Option<&Agent>) -> RowMark {
    match agent.map(|a| a.state) {
        Some(AgentState::Busy) => RowMark::Agent,
        Some(AgentState::Waiting) => RowMark::Waiting,
        Some(AgentState::Unknown) => RowMark::Agent,
        Some(AgentState::Idle) => RowMark::Ready,
        None => RowMark::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(kind: AgentKind, state: AgentState) -> Agent {
        Agent { kind, state }
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
            label: None,
            program: None,
        }
    }

    fn meta_at(cwd: &str) -> crate::tabmeta::TabMeta {
        crate::tabmeta::TabMeta {
            branch: None,
            agent: None,
            cwd: Some(std::path::PathBuf::from(cwd)),
            task: None,
        }
    }

    #[test]
    fn the_card_follows_a_cd_rather_than_the_assembled_row_title() {
        // R0.1 / R1.1 / R2.2 — the reported bug in one assertion. The row was assembled while
        // the tab sat in `api`; the shell has since moved to `web` and the worker published
        // it. Before this feature the card read `api` for the life of the tab, because
        // nothing re-runs `Tabs::titles` on a `cd`.
        let m = row_meta(&row_at("api"), Some(&meta_at("/Users/x/web")));
        assert_eq!(m.line1, "web");
        assert_eq!(m.cwd_title, "web");
    }

    #[test]
    fn line1_and_line2_cannot_name_two_different_folders() {
        // R1.2 — one resolved value feeds the label chain's cwd tier AND line 2's repo name.
        // A card whose line 1 is custom draws the folder on line 2, so two derivations here
        // would let one card name two directories — the defect one line down.
        let mut row = row_at("api");
        row.label = Some("Frontend".into());
        let m = row_meta(&row, Some(&meta_at("/Users/x/web")));
        assert_eq!(m.line1, "Frontend", "the user title still wins line 1");
        assert!(m.line1_is_custom, "so line 2 draws the repo name");
        assert_eq!(m.cwd_title, "web", "and it is the LIVE folder, not `api`");
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
        let before = row_meta(&row, Some(&meta_at("/Users/x/api")));
        let after = row_meta(&row, Some(&meta_at("/Users/x/web")));
        assert_eq!(before.cwd_title, "api");
        assert_eq!(after.cwd_title, "web");
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
        let m = row_meta(&row_at("api"), Some(&meta));
        assert_eq!(m.cwd_title, "web");
        assert_eq!(m.branch.as_deref(), Some("dev"));
    }

    #[test]
    fn the_folder_name_is_carried_even_when_line_2_will_not_draw_it() {
        // R1.3 / D6 — line 2 draws the folder only on a custom card, but which card that is
        // changes as labels come and go, so the value is always carried rather than
        // conditionally filled.
        let m = row_meta(&row_at("api"), Some(&meta_at("/Users/x/web")));
        assert!(!m.line1_is_custom, "a plain cwd card");
        assert_eq!(m.cwd_title, "web", "…still carries the folder name");
    }

    #[test]
    fn an_unpolled_tab_shows_the_assembled_title_rather_than_blank() {
        // R2.1 — the not-yet-published window (up to one worker tick on a new tab). The row's
        // own title is the only answer there is, and a blank card is the outcome D2 refuses.
        let m = row_meta(&row_at("api"), None);
        assert_eq!(m.line1, "api");
        assert_eq!(m.cwd_title, "api");
    }

    #[test]
    fn an_unresolvable_cwd_keeps_the_tab_n_fallback() {
        // R2.3 — a dead pid publishes `cwd: None`; the row's `Tab {n}` must survive it.
        let meta = crate::tabmeta::TabMeta::default();
        let m = row_meta(&row_at("Tab 3"), Some(&meta));
        assert_eq!(m.line1, "Tab 3");
        assert_eq!(m.cwd_title, "Tab 3");
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
        let live = row_meta(&row_at("api"), Some(&meta_at("/Users/x/web")));
        let unpolled = row_meta(&row_at("api"), None);
        assert_eq!(live.cwd_title, "web");
        assert_eq!(unpolled.cwd_title, "api");

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
    fn two_directories_with_the_same_basename_render_identically() {
        // R5.3 / D4 — accepted behaviour, pinned so a later contributor does not "fix" the
        // invisible-change case (`~/a/src` -> `~/b/src`) without reopening the decision.
        let a = row_meta(&row_at("x"), Some(&meta_at("/Users/x/a/src")));
        let b = row_meta(&row_at("x"), Some(&meta_at("/Users/x/b/src")));
        assert_eq!(a.cwd_title, "src");
        assert_eq!(b.cwd_title, "src");
    }

    #[test]
    fn the_live_cwd_does_not_disturb_the_precedence_chain() {
        // R5.4 — the cwd tier is the BOTTOM of the chain; making it live must not let it
        // outrank anything. Task beats program, program beats cwd, exactly as before.
        let mut row = row_at("api");
        row.program = Some("vim - main.rs".into());
        let mut meta = meta_at("/Users/x/web");
        assert_eq!(
            row_meta(&row, Some(&meta)).line1,
            "vim - main.rs",
            "a program title still outranks the (now live) cwd"
        );
        meta.task = Some("refactor the parser".into());
        assert_eq!(
            row_meta(&row, Some(&meta)).line1,
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
        for k in [AgentKind::Codex, AgentKind::Gemini, AgentKind::Aider] {
            assert_eq!(
                skin_for(Some(&agent(k, AgentState::Busy))),
                AgentSkin::Generic
            );
        }
        assert_eq!(skin_for(None), AgentSkin::Generic);
    }

    #[test]
    fn mark_mapping_honors_unknown_as_present() {
        // R6.6 — Unknown is present (✳), never ready (◇).
        assert_eq!(
            mark_for(Some(&agent(AgentKind::Claude, AgentState::Busy))),
            RowMark::Agent
        );
        assert_eq!(
            mark_for(Some(&agent(AgentKind::Claude, AgentState::Idle))),
            RowMark::Ready
        );
        assert_eq!(
            mark_for(Some(&agent(AgentKind::Aider, AgentState::Unknown))),
            RowMark::Agent
        );
        // Waiting (D16) maps to its own dedicated ⏸ mark (D6/T4).
        assert_eq!(
            mark_for(Some(&agent(AgentKind::Claude, AgentState::Waiting))),
            RowMark::Waiting
        );
        assert_eq!(mark_for(None), RowMark::None);
    }
}
