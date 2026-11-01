//! TT8 — acceptance tests for `specs/session-overview-ui` R8.2–R8.5, R8.7 and R8.8's data
//! half (D12, D13): `app::overview_detail::build_detail` turns a session row and its log
//! digest into the detail screen's window, ticks, lanes, main-lane segments, selection,
//! panel and recent events.
//!
//! Pure data in, pure data out: every `SessionDigest` is built by hand (no log, no thread),
//! and every call passes `tz_offset_s = 0`, so each `HH:MM` in the spec is UTC wall-clock.
//! Times are on 2026-10-31.

use ai_monitor::telemetry::{Event, EventKind, Subagent, Turn};
use app::overview_detail::build_detail;
use app::SessionDigest;
use renderer::overview_model::{
    AgentTag, Bucket, DetailData, Segment, SegmentKind, SessionRowData, SubagentStatus,
};

const TZ: i32 = 0;
const EPS: f32 = 1e-3;

/// Epoch ms of 2026-10-31 `h:m:s` UTC.
fn at(h: u64, m: u64, s: u64) -> u64 {
    // 2026-10-31T00:00:00Z
    const DAY_MS: u64 = 1_793_404_800_000;
    DAY_MS + h * 3_600_000 + m * 60_000 + s * 1_000
}

fn claude_row() -> SessionRowData {
    SessionRowData {
        tab: 7,
        pane: 0,
        bucket: Bucket::Idle,
        project: "terminal".to_string(),
        agent_label: "Claude Code".to_string(),
        agent: AgentTag::Claude,
        branch_line: "main".to_string(),
        has_branch: true,
        now_doing: "Turn finished · ready for next prompt".to_string(),
        prompt: Some("scan the repo".to_string()),
        elapsed: "20m".to_string(),
        ..SessionRowData::default()
    }
}

struct Sub<'a> {
    id: &'a str,
    ty: &'a str,
    desc: &'a str,
    start: u64,
    end: Option<u64>,
    ok: bool,
    report: Option<&'a str>,
}

fn subagent(s: Sub<'_>) -> Subagent {
    Subagent {
        id: s.id.to_string(),
        subagent_type: Some(s.ty.to_string()),
        description: Some(s.desc.to_string()),
        model: None,
        start_ms: Some(s.start),
        end_ms: s.end,
        completed: s.end.is_some(),
        agent_id: None,
        ended_ok: s.ok,
        report_preview: s.report.map(str::to_string),
    }
}

fn done(id: &str, desc: &str, start: u64, end: u64) -> Subagent {
    subagent(Sub {
        id,
        ty: "general-purpose",
        desc,
        start,
        end: Some(end),
        ok: true,
        report: None,
    })
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < EPS
}

/// The main lane without zero-width segments, which carry nothing drawable.
fn drawn(segments: &[Segment]) -> Vec<Segment> {
    segments
        .iter()
        .copied()
        .filter(|s| s.end - s.start > EPS)
        .collect()
}

fn assert_segments(got: &[Segment], want: &[(SegmentKind, f32, f32)]) {
    let got = drawn(got);
    assert_eq!(
        got.len(),
        want.len(),
        "segment count: got {got:#?}, want {want:#?}"
    );
    for (i, (g, (kind, start, end))) in got.iter().zip(want).enumerate() {
        assert!(
            g.kind == *kind && near(g.start, *start) && near(g.end, *end),
            "segment {i}: got {g:?}, want {kind:?} {start}..{end}; all {got:#?}"
        );
    }
}

// ---- R8.2 -------------------------------------------------------------------------

/// R8.2: the turn `[10:00, 10:10]`, a subagent running 10:01–10:09, now 10:20 → the main
/// lane is working 10:00–10:01, waiting on subagent 10:01–10:09, working 10:09–10:10, idle
/// 10:10–10:20. The session's first event is the prompt at 10:00, so the window is
/// 10:00–10:20 and one minute is 1/20 of it.
#[test]
fn r8_2_main_lane_segments() {
    let digest = SessionDigest {
        subagents: vec![done("toolu_bg", "Scan", at(10, 1, 0), at(10, 9, 0))],
        last_prompt_ms: Some(at(10, 0, 0)),
        turns: vec![Turn {
            start_ms: at(10, 0, 0),
            end_ms: Some(at(10, 10, 0)),
        }],
        first_event_ms: Some(at(10, 0, 0)),
        session_id: Some("sess-r82".to_string()),
        ..SessionDigest::default()
    };

    let d = build_detail(&claude_row(), Some(&digest), None, at(10, 20, 0), TZ);

    let m = 1.0 / 20.0;
    assert_segments(
        &d.main_lane,
        &[
            (SegmentKind::Working, 0.0, m),
            (SegmentKind::WaitingOnSubagent, m, 9.0 * m),
            (SegmentKind::Working, 9.0 * m, 10.0 * m),
            (SegmentKind::Idle, 10.0 * m, 1.0),
        ],
    );
}

// ---- R8.3 -------------------------------------------------------------------------

/// R8.3: first event 3 h ago, now 14:37 → the window is 13:37–14:37, ticks at 13:40 …
/// 14:30; a subagent 13:20–13:45 is clipped to start at 0; one ending 13:30 has no lane.
#[test]
fn r8_3_window_ticks_and_clipping() {
    let now = at(14, 37, 0);
    let digest = SessionDigest {
        // Newest first, as the telemetry ring keeps them.
        subagents: vec![
            done("toolu_clip", "Clipped scan", at(13, 20, 0), at(13, 45, 0)),
            done("toolu_gone", "Early scan", at(13, 0, 0), at(13, 30, 0)),
        ],
        first_event_ms: Some(at(11, 37, 0)),
        last_prompt_ms: Some(at(11, 37, 0)),
        session_id: Some("sess-r83".to_string()),
        ..SessionDigest::default()
    };

    let d = build_detail(&claude_row(), Some(&digest), None, now, TZ);

    let labels: Vec<&str> = d.ticks.iter().map(|t| t.label.as_str()).collect();
    assert_eq!(
        labels,
        ["13:40", "13:50", "14:00", "14:10", "14:20", "14:30"]
    );
    // 13:37 is fraction 0 and 14:37 is 1: tick k sits at (3 + 10k) / 60.
    for (k, t) in d.ticks.iter().enumerate() {
        let want = (3.0 + 10.0 * k as f32) / 60.0;
        assert!(near(t.at, want), "tick {} at {} want {want}", t.label, t.at);
    }

    assert_eq!(
        d.lanes.len(),
        1,
        "only the subagent overlapping the window has a lane: {:#?}",
        d.lanes
    );
    let lane = &d.lanes[0];
    assert!(
        lane.label.contains("Clipped scan"),
        "the lane is the 13:20–13:45 subagent: {lane:?}"
    );
    assert!(near(lane.start, 0.0), "clipped to start at 0: {lane:?}");
    assert!(near(lane.end, 8.0 / 60.0), "ends at 13:45: {lane:?}");
}

// ---- R8.4 -------------------------------------------------------------------------

/// R8.4: a session whose first event is 20 min ago → the window starts at that event, not
/// 60 minutes back. First event 10:05, now 10:25: the 10:10 tick sits at 1/4, the 10:20
/// tick at 3/4, and a subagent 10:05–10:15 spans `[0, 1/2]`.
#[test]
fn r8_4_short_session_window_starts_at_first_event() {
    let digest = SessionDigest {
        subagents: vec![done("toolu_s", "Short scan", at(10, 5, 0), at(10, 15, 0))],
        first_event_ms: Some(at(10, 5, 0)),
        last_prompt_ms: Some(at(10, 5, 0)),
        session_id: Some("sess-r84".to_string()),
        ..SessionDigest::default()
    };

    let d = build_detail(&claude_row(), Some(&digest), None, at(10, 25, 0), TZ);

    let ticks: Vec<(&str, f32)> = d.ticks.iter().map(|t| (t.label.as_str(), t.at)).collect();
    assert_eq!(ticks.len(), 2, "ticks: {ticks:?}");
    assert_eq!(ticks[0].0, "10:10");
    assert!(near(ticks[0].1, 0.25), "10:10 at 1/4: {ticks:?}");
    assert_eq!(ticks[1].0, "10:20");
    assert!(near(ticks[1].1, 0.75), "10:20 at 3/4: {ticks:?}");

    assert_eq!(d.lanes.len(), 1, "lanes: {:#?}", d.lanes);
    assert!(near(d.lanes[0].start, 0.0), "lane: {:?}", d.lanes[0]);
    assert!(near(d.lanes[0].end, 0.5), "lane: {:?}", d.lanes[0]);
}

// ---- R8.5 -------------------------------------------------------------------------

/// Seven subagents inside one window (first event 10:00, now 10:50), started in order
/// 10:02, 10:07, … 10:32. Lane `i` (oldest first) is `task {i}`. Lane 2 ran
/// 10:12:34–10:15:39 (3 min 5 s), done, with a 6-line report preview capped to its first
/// four lines by the panel; lane 6 is still running; lane 3 has no report.
fn seven_lane_digest() -> SessionDigest {
    let mut subs: Vec<Subagent> = (0..7u64)
        .map(|i| {
            let start = at(10, 2 + 5 * i, 0);
            match i {
                2 => subagent(Sub {
                    id: "toolu_2",
                    ty: "Explore",
                    desc: "task 2",
                    start: at(10, 12, 34),
                    end: Some(at(10, 15, 39)),
                    ok: true,
                    report: Some("Found the parser\nIt is in vte.rs\nNo leaks\nAll good"),
                }),
                6 => subagent(Sub {
                    id: "toolu_6",
                    ty: "Plan",
                    desc: "task 6",
                    start,
                    end: None,
                    ok: true,
                    report: None,
                }),
                _ => done(
                    &format!("toolu_{i}"),
                    &format!("task {i}"),
                    start,
                    start + 60_000,
                ),
            }
        })
        .collect();
    subs.reverse(); // the telemetry ring is newest first
    SessionDigest {
        subagents: subs,
        first_event_ms: Some(at(10, 0, 0)),
        last_prompt_ms: Some(at(10, 0, 0)),
        turns: vec![Turn {
            start_ms: at(10, 0, 0),
            end_ms: None,
        }],
        session_id: Some("sess-r85".to_string()),
        ..SessionDigest::default()
    }
}

fn lane_selection(d: &DetailData) -> Vec<bool> {
    d.lanes.iter().map(|l| l.selected).collect()
}

/// R8.5, first half: with no selection the newest lane (6) is selected and the panel is
/// its subagent.
#[test]
fn r8_5_default_selection_is_the_newest_lane() {
    let digest = seven_lane_digest();
    let d = build_detail(&claude_row(), Some(&digest), None, at(10, 50, 0), TZ);

    assert_eq!(d.lanes.len(), 7, "seven lanes: {:#?}", d.lanes);
    assert_eq!(d.selected, Some(6));
    assert_eq!(
        lane_selection(&d),
        [false, false, false, false, false, false, true]
    );
    let panel = d.panel.expect("a panel for the selected lane");
    assert_eq!(panel.title, "task 6");
    assert_eq!(panel.subagent_type, "Plan");
    assert_eq!(panel.status, SubagentStatus::Running);
    assert_eq!(panel.status_word, "running");
}

/// R8.5, second half: the press on lane 2 arrives as `selected: Some(2)` → lane 2 is
/// selected and the panel shows its description, type, status, `Started` as `HH:MM:SS`,
/// duration and up to four report lines.
#[test]
fn r8_5_selected_lane_two_fills_the_panel() {
    let digest = seven_lane_digest();
    let d = build_detail(&claude_row(), Some(&digest), Some(2), at(10, 50, 0), TZ);

    assert_eq!(d.selected, Some(2));
    assert_eq!(
        lane_selection(&d),
        [false, false, true, false, false, false, false]
    );
    let panel = d.panel.clone().expect("a panel for lane 2");
    assert_eq!(panel.title, "task 2");
    assert_eq!(panel.subagent_type, "Explore");
    assert_eq!(panel.status, SubagentStatus::Done);
    assert_eq!(panel.status_word, "done");
    assert_eq!(panel.started, "10:12:34");
    assert_eq!(panel.duration, "3m 05s");
    assert_eq!(
        panel.report,
        [
            "Found the parser",
            "It is in vte.rs",
            "No leaks",
            "All good"
        ]
    );
}

/// R8.5: a selected subagent with no recorded report shows `—`.
#[test]
fn r8_5_panel_without_report_reads_dash() {
    let digest = seven_lane_digest();
    let d = build_detail(&claude_row(), Some(&digest), Some(3), at(10, 50, 0), TZ);

    let panel = d.panel.expect("a panel for lane 3");
    assert_eq!(panel.title, "task 3");
    assert_eq!(panel.report, ["—"]);
}

// ---- R8.7 -------------------------------------------------------------------------

/// R8.7: 12 events in the window → the newest 8, newest first, each `HH:MM` + its text.
#[test]
fn r8_7_recent_events_newest_eight() {
    let tool = |name: &str, summary: &str| EventKind::Tool {
        name: name.to_string(),
        summary: Some(summary.to_string()),
    };
    let desc = |s: &str| s.to_string();
    // Oldest first here; the digest keeps them newest first.
    let kinds = vec![
        tool("Read", "lib.rs"), // 10:01
        tool("Bash", "ls"),     // 10:02
        EventKind::TaskStart {
            description: desc("Old scan"),
        }, // 10:03
        tool("Grep", "fn main"), // 10:04
        EventKind::TaskStart {
            description: desc("Scan"),
        }, // 10:05
        tool("Bash", "cargo test"), // 10:06
        EventKind::TaskDone {
            description: desc("Scan"),
        }, // 10:07
        EventKind::TurnFinished, // 10:08
        EventKind::TaskStart {
            description: desc("Fix"),
        }, // 10:09
        EventKind::TaskFailed {
            description: desc("Fix"),
        }, // 10:10
        tool("Edit", "main.rs"), // 10:11
        EventKind::TurnFinished, // 10:12
    ];
    let mut events: Vec<Event> = kinds
        .into_iter()
        .enumerate()
        .map(|(i, kind)| Event {
            at_ms: at(10, 1 + i as u64, 0),
            kind,
        })
        .collect();
    events.reverse();
    let digest = SessionDigest {
        events,
        first_event_ms: Some(at(10, 0, 0)),
        last_prompt_ms: Some(at(10, 0, 0)),
        session_id: Some("sess-r87".to_string()),
        ..SessionDigest::default()
    };

    let d = build_detail(&claude_row(), Some(&digest), None, at(10, 30, 0), TZ);

    let got: Vec<(&str, &str)> = d
        .events
        .iter()
        .map(|e| (e.time.as_str(), e.text.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("10:12", "Turn finished"),
            ("10:11", "Edit · main.rs"),
            ("10:10", "Task failed · Fix"),
            ("10:09", "Task start · Fix"),
            ("10:08", "Turn finished"),
            ("10:07", "Task done · Scan"),
            ("10:06", "Bash · cargo test"),
            ("10:05", "Task start · Scan"),
        ]
    );
}

// ---- R8.8 (data) ------------------------------------------------------------------

/// R8.8: an Antigravity session with no digest → the header data and no lanes.
#[test]
fn r8_8_non_claude_detail_has_header_and_no_lanes() {
    let row = SessionRowData {
        tab: 12,
        pane: 0,
        bucket: Bucket::Working,
        project: "webapp".to_string(),
        agent_label: "Antigravity".to_string(),
        agent: AgentTag::Antigravity,
        branch_line: "feature/login".to_string(),
        has_branch: true,
        now_doing: "Working".to_string(),
        prompt: Some("add the login page".to_string()),
        elapsed: "—".to_string(),
        ..SessionRowData::default()
    };

    let d = build_detail(&row, None, None, at(10, 30, 0), TZ);

    assert_eq!((d.tab, d.pane), (12, 0));
    assert_eq!(d.project, "webapp");
    assert_eq!(d.agent_label, "Antigravity");
    assert_eq!(d.agent, AgentTag::Antigravity);
    assert_eq!(d.bucket, Bucket::Working);
    assert_eq!(d.branch_line, "feature/login");
    assert_eq!(d.now_doing, "Working");
    assert_eq!(d.prompt.as_deref(), Some("add the login page"));
    assert_eq!(d.elapsed, "—");
    assert!(!d.has_timeline, "no timeline without a digest");
    assert!(d.lanes.is_empty(), "no lanes: {:#?}", d.lanes);
}
