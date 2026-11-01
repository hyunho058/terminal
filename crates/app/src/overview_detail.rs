//! One session's detail screen (`session-overview-ui` D10, D12, D13, D22): a pure function
//! from the session's overview row and its log digest to the renderer's [`DetailData`] —
//! window, ticks, lanes and main-lane segments already in window fractions, events and the
//! selected subagent's panel already formatted.
//!
//! Times are wall-clock, computed as `ms / 1000 + tz_offset_s` with plain arithmetic: the
//! caller reads the local offset once per rebuild, so this module needs no time crate and
//! every test can pin the offset.

use ai_monitor::telemetry::{Event, EventKind, Subagent, Turn};
use renderer::overview_model::{
    AgentTag, DetailData, EventLine, EventTone, Lane, Segment, SegmentKind, SessionRowData,
    SubagentPanel, Tick,
};

use crate::flow_telemetry::SessionDigest;
use crate::overview::{status_of as status, status_word, subagent_duration as duration};

/// The longest window the timeline shows (D12).
const WINDOW_MS: u64 = 60 * 60_000;
/// Ticks sit on whole multiples of this, in local wall-clock time (D12).
const TICK_MS: i64 = 10 * 60_000;
/// How many recent events the detail lists (D13).
const MAX_EVENTS: usize = 8;
/// How many report lines the panel shows (D13).
const REPORT_LINES: usize = 4;

/// Build `row`'s detail (D12, D13, D22).
///
/// `digest` is `None` for a session with no log digest (Codex, Gemini, Aider, or a log not
/// yet read; Claude and Antigravity have one, D24): the header only, no
/// timeline. `selected` is the lane index the user picked, `None` for the default (the
/// newest lane). `tz_offset_s` is the local UTC offset, for the `HH:MM` labels.
pub fn build_detail(
    row: &SessionRowData,
    digest: Option<&SessionDigest>,
    selected: Option<usize>,
    now_ms: u64,
    tz_offset_s: i32,
) -> DetailData {
    let mut detail = DetailData {
        tab: row.tab,
        pane: row.pane,
        project: row.project.clone(),
        agent_label: row.agent_label.clone(),
        agent: row.agent,
        bucket: row.bucket,
        branch_line: row.branch_line.clone(),
        now_doing: row.now_doing.clone(),
        prompt: row.prompt.clone(),
        elapsed: row.elapsed.clone(),
        ..DetailData::default()
    };
    let Some(digest) = digest else {
        return detail;
    };
    detail.has_timeline = true;
    detail.resume_cmd = resume_cmd(row.agent, digest.session_id.as_deref());

    let win = Window::new(digest.first_event_ms, now_ms);
    detail.ticks = ticks(&win, tz_offset_s);

    // Oldest first, only those overlapping the window (D12). The ring is newest first; a
    // stable sort on the start keeps log order among equal starts either way.
    let mut subs: Vec<&Subagent> = digest
        .subagents
        .iter()
        .filter(|s| sub_span(s, now_ms).is_some_and(|(a, b)| b >= win.start && a <= win.end))
        .collect();
    subs.sort_by_key(|s| s.start_ms);

    let chosen = match selected {
        Some(i) if i < subs.len() => Some(i),
        _ => subs.len().checked_sub(1),
    };
    detail.lanes = subs
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let (a, b) = sub_span(s, now_ms)?;
            Some(Lane {
                label: format!("└ {}", description(s)),
                status: status(s),
                start: win.frac(a),
                end: win.frac(b),
                duration: duration(s, now_ms),
                selected: chosen == Some(i),
            })
        })
        .collect();
    detail.selected = chosen;
    detail.panel = chosen
        .and_then(|i| subs.get(i))
        .map(|s| panel(s, now_ms, tz_offset_s));

    detail.main_lane = main_lane(&win, &digest.turns, &digest.subagents, now_ms);
    detail.events = events(&win, &digest.events, tz_offset_s);
    detail
}

/// `claude --resume {id}` for a Claude session with a known id (D22).
fn resume_cmd(agent: AgentTag, session_id: Option<&str>) -> Option<String> {
    match (agent, session_id) {
        (AgentTag::Claude, Some(id)) if !id.trim().is_empty() => {
            Some(format!("claude --resume {}", id.trim()))
        }
        _ => None,
    }
}

/// The timeline window `[start, end]` in epoch ms (D12): the last hour, or from the first
/// event when that is more recent, to now.
#[derive(Clone, Copy, Debug)]
struct Window {
    start: u64,
    end: u64,
}

impl Window {
    fn new(first_event_ms: Option<u64>, now_ms: u64) -> Window {
        let hour_ago = now_ms.saturating_sub(WINDOW_MS);
        let start = first_event_ms.map_or(hour_ago, |f| f.max(hour_ago).min(now_ms));
        Window { start, end: now_ms }
    }

    /// `t` as a fraction of the window, clamped to `0..=1`.
    fn frac(&self, t: u64) -> f32 {
        let span = self.end.saturating_sub(self.start);
        if span == 0 {
            return 0.0;
        }
        let t = t.clamp(self.start, self.end);
        ((t - self.start) as f64 / span as f64) as f32
    }
}

/// Ticks at whole 10-minute marks of the local wall clock inside the window (D12).
fn ticks(win: &Window, tz_offset_s: i32) -> Vec<Tick> {
    let tz_ms = i64::from(tz_offset_s) * 1000;
    let to_local = |t: u64| i64::try_from(t).unwrap_or(i64::MAX).saturating_add(tz_ms);
    let (lo, hi) = (to_local(win.start), to_local(win.end));
    let mut mark = lo.div_euclid(TICK_MS) * TICK_MS;
    if mark < lo {
        mark += TICK_MS;
    }
    let mut out = Vec::new();
    while mark <= hi {
        let utc = u64::try_from(mark - tz_ms).unwrap_or(0);
        out.push(Tick {
            at: win.frac(utc),
            label: hh_mm(utc, tz_offset_s),
        });
        mark += TICK_MS;
    }
    out
}

/// Local wall-clock seconds of the day for `ms`.
fn day_seconds(ms: u64, tz_offset_s: i32) -> i64 {
    let secs = i64::try_from(ms / 1000).unwrap_or(i64::MAX);
    secs.saturating_add(i64::from(tz_offset_s))
        .rem_euclid(86_400)
}

/// Local `HH:MM`.
fn hh_mm(ms: u64, tz_offset_s: i32) -> String {
    let s = day_seconds(ms, tz_offset_s);
    format!("{:02}:{:02}", s / 3600, s % 3600 / 60)
}

/// Local `HH:MM:SS`.
fn hh_mm_ss(ms: u64, tz_offset_s: i32) -> String {
    let s = day_seconds(ms, tz_offset_s);
    format!("{:02}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
}

/// A subagent's `[start, end or now]`; `None` when its start is unknown, or it finished
/// at an unknown time (nothing to place).
fn sub_span(s: &Subagent, now_ms: u64) -> Option<(u64, u64)> {
    let start = s.start_ms?;
    let end = if s.completed { s.end_ms? } else { now_ms };
    Some((start, end.max(start)))
}

fn description(s: &Subagent) -> &str {
    s.description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .unwrap_or("—")
}

/// The selected subagent's panel (D13).
fn panel(s: &Subagent, now_ms: u64, tz_offset_s: i32) -> SubagentPanel {
    let st = status(s);
    let mut report: Vec<String> = s
        .report_preview
        .as_deref()
        .unwrap_or("")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(REPORT_LINES)
        .map(str::to_string)
        .collect();
    if report.is_empty() {
        report.push("—".to_string());
    }
    SubagentPanel {
        title: description(s).to_string(),
        subagent_type: s
            .subagent_type
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or("agent")
            .to_string(),
        status: st,
        status_word: status_word(st).to_string(),
        started: s
            .start_ms
            .map_or_else(|| "—".to_string(), |t| hh_mm_ss(t, tz_offset_s)),
        duration: duration(s, now_ms),
        report,
    }
}

/// The main session lane (D12): blank before the oldest kept turn; inside a turn, waiting
/// on subagent where at least one subagent runs and working elsewhere; idle from a turn's
/// end to the next turn's start (the next human prompt) or now.
fn main_lane(win: &Window, turns: &[Turn], subs: &[Subagent], now_ms: u64) -> Vec<Segment> {
    let mut turns: Vec<&Turn> = turns.iter().collect();
    turns.sort_by_key(|t| t.start_ms);
    let busy: Vec<(u64, u64)> = subs.iter().filter_map(|s| sub_span(s, now_ms)).collect();

    let mut spans: Vec<(SegmentKind, u64, u64)> = Vec::new();
    let first = turns.first().map_or(win.end, |t| t.start_ms);
    spans.push((SegmentKind::Blank, win.start, first));

    for (i, turn) in turns.iter().enumerate() {
        let next = turns.get(i + 1).map_or(now_ms, |t| t.start_ms);
        let end = turn.end_ms.unwrap_or(next).min(next).max(turn.start_ms);
        // Cut the turn at every subagent boundary inside it, then colour each piece by
        // whether any subagent covers it.
        let mut cuts: Vec<u64> = vec![turn.start_ms, end];
        for &(a, b) in &busy {
            for t in [a, b] {
                if t > turn.start_ms && t < end {
                    cuts.push(t);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for pair in cuts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let waiting = busy.iter().any(|&(s, e)| s <= a && e >= b);
            let kind = if waiting {
                SegmentKind::WaitingOnSubagent
            } else {
                SegmentKind::Working
            };
            spans.push((kind, a, b));
        }
        spans.push((SegmentKind::Idle, end, next));
    }

    let mut out: Vec<Segment> = Vec::new();
    for (kind, a, b) in spans {
        let (a, b) = (a.max(win.start), b.min(win.end));
        if b <= a {
            continue;
        }
        let (start, end) = (win.frac(a), win.frac(b));
        match out.last_mut() {
            Some(last) if last.kind == kind && last.end >= start => last.end = end,
            _ => out.push(Segment { kind, start, end }),
        }
    }
    out
}

/// The newest [`MAX_EVENTS`] events inside the window, newest first (D13).
fn events(win: &Window, events: &[Event], tz_offset_s: i32) -> Vec<EventLine> {
    let mut inside: Vec<&Event> = events
        .iter()
        .filter(|e| e.at_ms >= win.start && e.at_ms <= win.end)
        .collect();
    inside.sort_by_key(|e| std::cmp::Reverse(e.at_ms));
    inside
        .into_iter()
        .take(MAX_EVENTS)
        .map(|e| {
            let (text, tone) = event_text(&e.kind);
            EventLine {
                time: hh_mm(e.at_ms, tz_offset_s),
                text,
                tone,
            }
        })
        .collect()
}

fn event_text(kind: &EventKind) -> (String, EventTone) {
    match kind {
        EventKind::TurnFinished => ("Turn finished".to_string(), EventTone::Plain),
        EventKind::TaskStart { description } => {
            (format!("Task start · {description}"), EventTone::Plain)
        }
        EventKind::TaskDone { description } => {
            (format!("Task done · {description}"), EventTone::Done)
        }
        EventKind::TaskFailed { description } => {
            (format!("Task failed · {description}"), EventTone::Failed)
        }
        EventKind::Tool { name, summary } => match summary.as_deref() {
            Some(s) if !s.is_empty() => (format!("{name} · {s}"), EventTone::Plain),
            _ => (name.clone(), EventTone::Plain),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use renderer::overview_model::SubagentStatus;

    const MIN: u64 = 60_000;
    const BASE: u64 = 1_793_404_800_000; // 2026-10-31T00:00:00Z

    fn sub(start: Option<u64>, end: Option<u64>, completed: bool, ok: bool) -> Subagent {
        Subagent {
            id: "t".to_string(),
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

    #[test]
    fn wall_clock_formats_apply_the_offset() {
        let t = BASE + 10 * 60 * MIN + 5 * MIN + 7_000;
        assert_eq!(hh_mm(t, 0), "10:05");
        assert_eq!(hh_mm_ss(t, 0), "10:05:07");
        assert_eq!(hh_mm(t, 9 * 3600), "19:05");
        assert_eq!(hh_mm(t, -11 * 3600), "23:05");
        assert_eq!(hh_mm_ss(BASE + 59_999, 0), "00:00:59");
    }

    #[test]
    fn ticks_follow_the_local_clock() {
        // 10:05–10:25 UTC at +00:30 is 10:35–10:55 local: marks at 10:40 and 10:50 local.
        let win = Window {
            start: BASE + 605 * MIN,
            end: BASE + 625 * MIN,
        };
        let t = ticks(&win, 30 * 60);
        let labels: Vec<&str> = t.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(labels, ["10:40", "10:50"]);
        assert!((t[0].at - 0.25).abs() < 1e-4);
        assert!((t[1].at - 0.75).abs() < 1e-4);
        // A mark exactly on both edges is kept.
        let edge = Window {
            start: BASE + 600 * MIN,
            end: BASE + 610 * MIN,
        };
        assert_eq!(ticks(&edge, 0).len(), 2);
    }

    #[test]
    fn window_is_the_last_hour_or_from_the_first_event() {
        let now = BASE + 600 * MIN;
        let w = Window::new(None, now);
        assert_eq!((w.start, w.end), (now - 60 * MIN, now));
        let w = Window::new(Some(now - 20 * MIN), now);
        assert_eq!(w.start, now - 20 * MIN);
        let w = Window::new(Some(now - 200 * MIN), now);
        assert_eq!(w.start, now - 60 * MIN);
        let w = Window::new(Some(now + MIN), now);
        assert_eq!(w.start, now);
        assert_eq!(w.frac(now + 5), 0.0, "an empty window maps everything to 0");
        let w = Window::new(Some(now - 20 * MIN), now);
        assert_eq!(w.frac(now - 30 * MIN), 0.0);
        assert_eq!(w.frac(now + MIN), 1.0);
        assert!((w.frac(now - 10 * MIN) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn durations_and_statuses() {
        let now = BASE + 100 * MIN;
        let done = sub(Some(BASE), Some(BASE + 3 * MIN + 5_000), true, true);
        assert_eq!(duration(&done, now), "3m 05s");
        assert_eq!(status(&done), SubagentStatus::Done);
        let failed = sub(Some(BASE), Some(BASE + 61 * MIN), true, false);
        assert_eq!(duration(&failed, now), "61m 00s");
        assert_eq!(status(&failed), SubagentStatus::Failed);
        let running = sub(Some(now - 59_999), None, false, true);
        assert_eq!(duration(&running, now), "<1m");
        assert_eq!(status(&running), SubagentStatus::Running);
        let running = sub(Some(now - 12 * MIN), None, false, true);
        assert_eq!(duration(&running, now), "12m");
        assert_eq!(duration(&sub(None, None, false, true), now), "—");
        assert_eq!(duration(&sub(Some(BASE), None, true, true), now), "—");
        assert_eq!(sub_span(&sub(Some(BASE), None, true, true), now), None);
        assert_eq!(
            sub_span(&sub(Some(BASE), None, false, true), now),
            Some((BASE, now))
        );
    }

    #[test]
    fn panel_trims_report_and_falls_back() {
        let mut s = sub(Some(BASE), Some(BASE + MIN), true, true);
        s.report_preview = Some("\n a \n\nb\nc\nd\ne\n".to_string());
        let p = panel(&s, BASE + 2 * MIN, 0);
        assert_eq!(p.report, ["a", "b", "c", "d"]);
        assert_eq!(p.title, "—");
        assert_eq!(p.subagent_type, "agent");
        assert_eq!(p.started, "00:00:00");
        s.report_preview = Some("  \n".to_string());
        assert_eq!(panel(&s, BASE, 0).report, ["—"]);
    }

    #[test]
    fn resume_only_for_claude_with_an_id() {
        assert_eq!(
            resume_cmd(AgentTag::Claude, Some("abc")).as_deref(),
            Some("claude --resume abc")
        );
        assert_eq!(resume_cmd(AgentTag::Claude, Some(" ")), None);
        assert_eq!(resume_cmd(AgentTag::Claude, None), None);
        assert_eq!(resume_cmd(AgentTag::Codex, Some("abc")), None);
    }

    #[test]
    fn main_lane_blank_before_first_turn_and_idle_between_turns() {
        let win = Window {
            start: BASE,
            end: BASE + 40 * MIN,
        };
        let turns = [
            Turn {
                start_ms: BASE + 20 * MIN,
                end_ms: None,
            },
            Turn {
                start_ms: BASE + 10 * MIN,
                end_ms: Some(BASE + 15 * MIN),
            },
        ];
        let lane = main_lane(&win, &turns, &[], BASE + 40 * MIN);
        let kinds: Vec<(SegmentKind, f32, f32)> =
            lane.iter().map(|s| (s.kind, s.start, s.end)).collect();
        assert_eq!(
            kinds,
            [
                (SegmentKind::Blank, 0.0, 0.25),
                (SegmentKind::Working, 0.25, 0.375),
                (SegmentKind::Idle, 0.375, 0.5),
                (SegmentKind::Working, 0.5, 1.0),
            ]
        );
        // No turns at all: the whole window is blank.
        let lane = main_lane(&win, &[], &[], BASE + 40 * MIN);
        assert_eq!(lane.len(), 1);
        assert_eq!(lane[0].kind, SegmentKind::Blank);
    }

    #[test]
    fn events_text_and_tone() {
        assert_eq!(
            event_text(&EventKind::Tool {
                name: "Bash".to_string(),
                summary: None
            }),
            ("Bash".to_string(), EventTone::Plain)
        );
        assert_eq!(
            event_text(&EventKind::TaskDone {
                description: "x".to_string()
            })
            .1,
            EventTone::Done
        );
        assert_eq!(
            event_text(&EventKind::TaskFailed {
                description: "x".to_string()
            })
            .1,
            EventTone::Failed
        );
        let win = Window {
            start: BASE + MIN,
            end: BASE + 2 * MIN,
        };
        let evs = [
            Event {
                at_ms: BASE,
                kind: EventKind::TurnFinished,
            },
            Event {
                at_ms: BASE + MIN,
                kind: EventKind::TurnFinished,
            },
            Event {
                at_ms: BASE + 3 * MIN,
                kind: EventKind::TurnFinished,
            },
        ];
        assert_eq!(
            events(&win, &evs, 0).len(),
            1,
            "only events inside the window"
        );
    }
}
