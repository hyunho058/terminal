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
            let meta = snap.get(TabId(row.tab_id));
            let agent = meta.and_then(|m| m.agent.as_ref());
            // Resolve line 1 HERE (sticky-tab-title D7/R1.5): this is the one place that
            // sees all four sources at once with the AI-task label *fresh*. `Tabs::titles`
            // cannot do it — it runs on tab actions, while the task label arrives on this
            // per-frame pull, so resolving there would freeze the auto label until the
            // next tab switch.
            let task = meta.and_then(|m| m.task.as_deref());
            let (line1, is_custom) = resolve_line1_tier(
                row.label.as_deref(),
                task,
                row.program.as_deref(),
                &row.title,
            );
            out.push(RowMeta {
                branch: meta.and_then(|m| m.branch.clone()),
                mark: mark_for(agent),
                skin: skin_for(agent),
                line1_is_custom: is_custom,
                line1: line1.to_string(),
            });
        }
    }
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
