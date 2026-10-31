//! TT6 — acceptance tests for `specs/session-flow-graph` R6 (old D2, old D13), carried by
//! `specs/session-overview-ui` (D3 of the graph spec is kept): a session's state in the
//! Agent Monitor overview is the sidebar card's, computed by the card's own join, then split
//! into a bucket by `app::overview::bucket_of` (overview D2).
//!
//! * An unsplit session and a split tab's **active** pane take `row_meta(..).status`, the
//!   blocking-dialog override included (R6.1).
//! * A **non-active** pane is log-only — `resolve_status(Some(&agent), false, false, None)`,
//!   never the tab's dialog — and an `Unknown` Claude state is still a session row, in the
//!   `Agent` bucket (R6.2).
//!
//! Every fixture is hand-built data: no tmux, no logs, no process table. Every app item is
//! reached through the `app` crate root; the model's types through `renderer::overview_model`.
//! Rows sort by bucket, so panes are told apart by their pane pid, not their position.

use std::collections::HashMap;

use app::overview::{bucket_of, build_overview, ExpandSet};
use app::{resolve_status, row_meta, Agent, AgentKind, AgentState, PaneMeta, TabId, TabMeta};
use renderer::overview_model::{Bucket, Filter, OverviewModel, SessionRowData};
use renderer::sidebar_card::CardStatus;
use renderer::sidebar_row::SidebarRow;

const TAB: u64 = 7;

const ALL_STATES: [AgentState; 4] = [
    AgentState::Busy,
    AgentState::Waiting,
    AgentState::Idle,
    AgentState::Unknown,
];

fn claude(state: AgentState) -> Agent {
    Agent {
        kind: AgentKind::Claude,
        state,
    }
}

fn row(blocking_dialog: bool) -> SidebarRow {
    SidebarRow {
        tab_id: TAB,
        title: "work".to_string(),
        blocking_dialog,
        ..Default::default()
    }
}

fn unsplit_meta(state: AgentState) -> TabMeta {
    TabMeta {
        agent: Some(claude(state)),
        ..Default::default()
    }
}

fn pane(pid: u32, active: bool, agent: Option<Agent>) -> PaneMeta {
    PaneMeta {
        graph_agent_cwd: None,
        graph_asks_user: false,
        pane_pid: pid,
        active,
        agent,
        log: None,
        cwd: None,
        branch: None,
        repo: None,
    }
}

/// `build_overview` over one row and its meta, with no digests and nothing expanded.
fn model_of(row: &SidebarRow, meta: TabMeta) -> OverviewModel {
    let mut metas = HashMap::new();
    metas.insert(TabId(row.tab_id), meta);
    // The active strip index only decides `selected`, which R6 does not read.
    let (model, _) = build_overview(
        std::slice::from_ref(row),
        &metas,
        None,
        &HashMap::new(),
        &ExpandSet::new(),
        Filter::All,
        0,
        1_793_404_800_000,
    );
    model
}

/// The session rows of tab `TAB`, in model order.
fn sessions(model: &OverviewModel) -> Vec<&SessionRowData> {
    model.rows.iter().filter(|r| r.tab == TAB).collect()
}

/// Tab `TAB`'s row for pane `pid`.
fn pane_row(model: &OverviewModel, pid: u32) -> &SessionRowData {
    sessions(model)
        .into_iter()
        .find(|r| r.pane == pid)
        .unwrap_or_else(|| panic!("a row for pane {pid}: {:#?}", model.rows))
}

// R6.1
#[test]
fn r6_1_unsplit_session_status_equals_the_cards_in_every_combination() {
    for state in ALL_STATES {
        for dialog in [false, true] {
            let r = row(dialog);
            let meta = unsplit_meta(state);
            let card = row_meta(&r, Some(&meta), None).status;
            let model = model_of(&r, meta);
            let got = sessions(&model);
            assert_eq!(
                got.len(),
                1,
                "one session row expected (state={state:?}, dialog={dialog})"
            );
            assert_eq!(
                got[0].bucket,
                bucket_of(card, dialog, false),
                "the session's bucket must be row_meta(..).status's (state={state:?}, dialog={dialog})"
            );
        }
    }
}

// R6.1
#[test]
fn r6_1_active_pane_takes_the_cards_status_and_the_other_pane_does_not() {
    let r = row(true);
    let meta = TabMeta {
        agent: Some(claude(AgentState::Busy)),
        panes: vec![
            pane(10, true, Some(claude(AgentState::Busy))),
            pane(11, false, Some(claude(AgentState::Busy))),
        ],
        ..Default::default()
    };
    let model = model_of(&r, meta);
    assert_eq!(sessions(&model).len(), 2, "one session row per Claude pane");
    assert_eq!(
        pane_row(&model, 10).bucket,
        bucket_of(CardStatus::WaitingForInput, true, false),
        "active pane 10 takes the card's status, dialog override included"
    );
    assert_eq!(pane_row(&model, 10).bucket, Bucket::NeedsYou);
    assert_eq!(
        pane_row(&model, 11).bucket,
        Bucket::Working,
        "non-active pane 11 ignores the tab's blocking dialog"
    );
}

// R6.2
#[test]
fn r6_2_non_active_pane_is_log_only() {
    let waiting = claude(AgentState::Waiting);
    let r = row(false);
    let meta = TabMeta {
        // The card reads Working for the tab; the non-active pane must not follow it.
        agent: Some(claude(AgentState::Busy)),
        panes: vec![
            pane(10, true, Some(claude(AgentState::Busy))),
            pane(11, false, Some(waiting)),
        ],
        ..Default::default()
    };
    let model = model_of(&r, meta);
    assert_eq!(sessions(&model).len(), 2, "one session row per Claude pane");
    assert_eq!(
        pane_row(&model, 11).bucket,
        bucket_of(
            resolve_status(Some(&waiting), false, false, None),
            false,
            false
        ),
        "non-active pane 11's status is resolve_status(Some(&agent), false, false, None)"
    );
    assert_eq!(pane_row(&model, 11).bucket, Bucket::Idle);
}

// R6.2
#[test]
fn r6_2_unknown_state_is_a_session_row_in_the_agent_bucket() {
    let r = row(false);
    let meta = unsplit_meta(AgentState::Unknown);
    assert_eq!(row_meta(&r, Some(&meta), None).status, CardStatus::Hidden);
    let model = model_of(&r, meta);
    let got = sessions(&model);
    assert_eq!(
        got.len(),
        1,
        "an Unknown Claude state is still a session row"
    );
    assert_eq!(got[0].bucket, Bucket::Agent);
}
