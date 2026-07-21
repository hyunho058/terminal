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
            out.push(RowMeta {
                branch: meta.and_then(|m| m.branch.clone()),
                mark: mark_for(agent),
                skin: skin_for(agent),
            });
        }
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
pub fn mark_for(agent: Option<&Agent>) -> RowMark {
    match agent.map(|a| a.state) {
        Some(AgentState::Busy) => RowMark::Agent,
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
        assert_eq!(mark_for(None), RowMark::None);
    }
}
