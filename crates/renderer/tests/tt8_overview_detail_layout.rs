//! TT8 — acceptance test for `specs/session-overview-ui` R8.8's layout half (D12): a
//! non-Claude session's detail draws, in place of the timeline,
//! `No timeline — this agent's log is not read.`
//!
//! Headless: `layout_detail` is pure, so the text runs it lays out are read directly. The
//! `DetailData` is the one `app::overview_detail::build_detail` produces for an Antigravity
//! session with no digest — header data, `has_timeline: false`, no lanes.

use renderer::overview_layout::layout_detail;
use renderer::overview_model::{AgentTag, Bucket, DetailData};

const NO_TIMELINE: &str = "No timeline — this agent's log is not read.";

fn antigravity_detail() -> DetailData {
    DetailData {
        tab: 12,
        pane: 0,
        project: "webapp".to_string(),
        agent_label: "Antigravity".to_string(),
        agent: AgentTag::Antigravity,
        bucket: Bucket::Working,
        branch_line: "feature/login".to_string(),
        now_doing: "Working".to_string(),
        prompt: Some("add the login page".to_string()),
        elapsed: "—".to_string(),
        has_timeline: false,
        ..DetailData::default()
    }
}

/// R8.8: the layout shows the no-timeline line.
#[test]
fn r8_8_non_claude_detail_layout_shows_no_timeline_line() {
    let layout = layout_detail(&antigravity_detail(), [1440.0, 900.0], 1.0, 0);

    let texts: Vec<&str> = layout.texts.iter().map(|t| t.text.as_str()).collect();
    assert!(
        texts.contains(&NO_TIMELINE),
        "the detail lays out {NO_TIMELINE:?}; texts: {texts:?}"
    );
}
