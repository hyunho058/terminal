//! TT10 — acceptance test for `specs/session-overview-ui` R10.3 (D17): a Claude session
//! whose log cannot be resolved degrades to state-only text, never to a guess.
//!
//! Driven through `app::overview::build_overview` on hand-built fixtures — a `SidebarRow`,
//! a `TabMeta` with a Claude agent and `log: None`, and an empty digest map — so nothing
//! here reads the live machine.

use std::collections::HashMap;
use std::path::PathBuf;

use app::overview::{build_overview, ExpandSet};
use app::{Agent, AgentKind, AgentState, SessionDigest, TabId, TabMeta};
use renderer::overview_model::Filter;
use renderer::sidebar_row::SidebarRow;

const TAB: u64 = 5;

/// 2026-10-31T10:00:00Z, epoch ms.
const NOW_MS: u64 = 1_793_440_800_000;

fn row() -> SidebarRow {
    SidebarRow {
        tab_id: TAB,
        title: "work".to_string(),
        ..SidebarRow::default()
    }
}

/// A Claude tab in `state` with no resolvable log. The toggle-free `graph_agent` and the
/// card's `agent` agree, as they do with `[sidebar].show_agent_status` on.
fn claude_without_log(state: AgentState) -> TabMeta {
    let agent = Agent {
        kind: AgentKind::Claude,
        state,
    };
    TabMeta {
        agent: Some(agent),
        graph_agent: Some(agent),
        cwd: Some(PathBuf::from("/Users/u/scratch")),
        graph_agent_cwd: Some(PathBuf::from("/Users/u/scratch")),
        log: None,
        ..TabMeta::default()
    }
}

// R10.3
#[test]
fn r10_3_claude_session_without_a_log_reads_state_only_and_dashes() {
    // (state, the D4 state-only now-doing text for its bucket)
    let cases = [
        (AgentState::Busy, "Working"),
        (AgentState::Idle, "Turn finished · ready for next prompt"),
    ];
    for (state, want_now_doing) in cases {
        let mut metas = HashMap::new();
        metas.insert(TabId(TAB), claude_without_log(state));
        let digests: HashMap<PathBuf, SessionDigest> = HashMap::new();
        let (model, _) = build_overview(
            &[row()],
            &metas,
            None,
            &digests,
            &ExpandSet::new(),
            Filter::All,
            0,
            NOW_MS,
        );

        assert_eq!(
            model.rows.len(),
            1,
            "{state:?}: the Claude tab is one row: {:?}",
            model.rows
        );
        let r = &model.rows[0];
        assert_eq!(r.now_doing, want_now_doing, "{state:?}: now-doing");
        assert_eq!(r.elapsed, "—", "{state:?}: elapsed");
        assert!(
            r.subagents.is_empty(),
            "{state:?}: no squares: {:?}",
            r.subagents
        );
        assert_eq!(r.subagent_summary, "—", "{state:?}: subagents");
    }
}
