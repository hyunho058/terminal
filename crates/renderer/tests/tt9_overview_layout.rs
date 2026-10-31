//! TT9 — acceptance tests for session-overview-ui R9.1–R9.4: the overview's column widths
//! and narrow thresholds, the detail's stacking, and whole-line scrolling (D15).
//!
//! Headless: everything here reads `overview_layout::layout_overview` / `layout_detail`
//! (and, for R9.4's wheel notches, `OverviewHandle::set_scroll` + `publish`), never a GPU.
//!
//! **Viewport choice.** D15's `W` is the *content* width, `min(viewport − 64, 1376)` at
//! scale 1 (32 logical px of side padding each side). Every test computes its viewport from
//! the GWT's `W` as `W + 64` at scale 1, so the content box is exactly `W` wide
//! ([`viewport_for`]). At scale 1 physical px equal the design's logical px.

use renderer::overview_layout::{layout_detail, layout_overview, OverviewLayout};
use renderer::overview_model::{
    AgentTag, Banner, Bucket, Counts, DetailData, EventLine, EventTone, Filter, Lane, OverviewHit,
    OverviewModel, Segment, SegmentKind, SessionRowData, SubagentCell, SubagentPanel,
    SubagentStatus, SubagentTotals, Tick, TreeLine,
};
use renderer::overview_panel::OverviewHandle;

const SCALE: f64 = 1.0;
/// D15's horizontal outer padding, both sides, logical px.
const SIDE_PADDING: f32 = 64.0;
const HEIGHT: f32 = 1000.0;
const TOL: f32 = 0.5;

/// The viewport whose content box is `w` wide at scale 1.
fn viewport_for(w: f32) -> [f32; 2] {
    [w + SIDE_PADDING, HEIGHT]
}

// ---- Fixtures --------------------------------------------------------------------------

fn tree_line(i: usize, status: SubagentStatus) -> TreeLine {
    TreeLine {
        subagent_type: "general-purpose".into(),
        description: format!("Investigate the flaky integration test number {i} in depth"),
        status_word: match status {
            SubagentStatus::Running => "running",
            SubagentStatus::Done => "done",
            SubagentStatus::Failed => "failed",
        }
        .into(),
        duration: "2m 05s".into(),
        status,
    }
}

fn row(tab: u64, bucket: Bucket, subagents: usize, expanded: bool) -> SessionRowData {
    let statuses: Vec<SubagentStatus> = (0..subagents)
        .map(|i| match i % 3 {
            0 => SubagentStatus::Running,
            1 => SubagentStatus::Done,
            _ => SubagentStatus::Failed,
        })
        .collect();
    SessionRowData {
        tab,
        pane: 0,
        bucket,
        project: format!("a-rather-long-project-name-number-{tab}-receipt-tracker"),
        agent_label: "Claude Code".into(),
        agent: AgentTag::Claude,
        branch_line: format!("feature/very-long-branch-name-{tab} · worktree-{tab}"),
        has_branch: true,
        now_doing: "Bash · cargo test --workspace --all-targets -- --nocapture --test-threads 1"
            .into(),
        prompt: Some("please fix the failing tests and then refactor the whole module".into()),
        elapsed: "1h 12m".into(),
        subagents: statuses.iter().map(|s| SubagentCell::Square(*s)).collect(),
        subagent_summary: if subagents == 0 {
            "—".into()
        } else {
            format!(
                "{} running · {} done",
                subagents.div_ceil(3),
                subagents / 3 * 2
            )
        },
        tree: statuses
            .iter()
            .enumerate()
            .map(|(i, s)| tree_line(i, *s))
            .collect(),
        expanded,
        selected: false,
    }
}

/// A handful of rows across every bucket, long strings everywhere, one expanded row with
/// two tree lines and a banner — so every column and the header carry text to overlap.
fn busy_model() -> OverviewModel {
    let rows = vec![
        row(1, Bucket::NeedsYou, 3, false),
        row(2, Bucket::Working, 2, true),
        row(3, Bucket::Working, 0, false),
        row(4, Bucket::Idle, 1, false),
    ];
    OverviewModel {
        resolving: false,
        rows,
        counts: Counts {
            all: 4,
            needs_you: 1,
            working: 2,
            idle: 1,
        },
        banner: Some(Banner {
            tab: 1,
            pane: 0,
            text: "Claude Code · a-rather-long-project-name — Approval needed · ./gradlew test"
                .into(),
            more: 0,
        }),
        filter: Filter::All,
        total_subagents: SubagentTotals {
            running: 3,
            done: 2,
            failed: 1,
        },
        view: Default::default(),
    }
}

fn overlap_area(a: [f32; 4], b: [f32; 4]) -> f32 {
    let w = (a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0]);
    let h = (a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1]);
    if w > 0.0 && h > 0.0 {
        w * h
    } else {
        0.0
    }
}

fn assert_no_text_overlap(lay: &OverviewLayout, w: f32) {
    for (i, a) in lay.texts.iter().enumerate() {
        for b in &lay.texts[i + 1..] {
            let area = overlap_area(a.rect, b.rect);
            assert!(
                area <= 0.01,
                "W = {w}: text runs {:?} at {:?} and {:?} at {:?} overlap by {area} px²",
                a.text,
                a.rect,
                b.text,
                b.rect
            );
        }
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= TOL
}

// ---- R9.1 ------------------------------------------------------------------------------

#[test]
fn r9_1_full_width_columns_and_the_1_2_to_2_share() {
    let w = 1376.0;
    let lay = layout_overview(&busy_model(), viewport_for(w), SCALE, 0);
    assert!(
        close(lay.content[2], w),
        "viewport {:?} should give a content box {w} wide, got {:?}",
        viewport_for(w),
        lay.content
    );
    let c = lay.columns;
    assert!(close(c.status, 120.0), "Status is {}, want 120", c.status);
    assert_eq!(c.subagents, Some(220.0), "Subagents");
    assert_eq!(c.elapsed, Some(88.0), "Elapsed");
    assert!(
        close(c.buttons, 104.0),
        "buttons is {}, want 104",
        c.buttons
    );

    // The rest: W − row padding (2 × 20) − the fixed columns − five 16 px gaps.
    let rest = w - 2.0 * 20.0 - (120.0 + 220.0 + 88.0 + 104.0) - 5.0 * 16.0;
    assert!(
        close(c.session + c.now_doing, rest),
        "Session {} + Now doing {} should share the rest {rest} (gaps 16, padding 20)",
        c.session,
        c.now_doing
    );
    assert!(
        close(c.session, rest * 1.2 / 3.2) && close(c.now_doing, rest * 2.0 / 3.2),
        "Session : Now doing should be 1.2 : 2 of {rest} ({} / {}), got {} / {}",
        rest * 1.2 / 3.2,
        rest * 2.0 / 3.2,
        c.session,
        c.now_doing
    );
}

// ---- R9.2 ------------------------------------------------------------------------------

#[test]
fn r9_2_narrow_thresholds_and_no_overlap() {
    let model = busy_model();

    // 1100: Now doing's minimum drops to 160.
    let w = 1100.0;
    let lay = layout_overview(&model, viewport_for(w), SCALE, 0);
    assert!(
        lay.columns.now_doing >= 160.0,
        "W = 1100: Now doing is {}, want ≥ 160",
        lay.columns.now_doing
    );
    assert_no_text_overlap(&lay, w);

    // 1000: Subagents 88, no summary text, tiles 2 × 2.
    let w = 1000.0;
    let lay = layout_overview(&model, viewport_for(w), SCALE, 0);
    assert_eq!(lay.columns.subagents, Some(88.0), "W = 1000: Subagents");
    assert!(
        !lay.columns.subagent_summary,
        "W = 1000: the Subagents column must show no summary text"
    );
    for r in &model.rows {
        if r.subagent_summary != "—" {
            assert!(
                !lay.texts.iter().any(|t| t.text == r.subagent_summary),
                "W = 1000: summary text {:?} was laid out",
                r.subagent_summary
            );
        }
    }
    assert!(lay.tiles_wrapped, "W = 1000: the tiles must wrap to 2 × 2");
    assert_eq!(lay.tiles.len(), 4, "W = 1000: four tiles");
    let mut xs: Vec<f32> = lay.tiles.iter().map(|t| t[0]).collect();
    let mut ys: Vec<f32> = lay.tiles.iter().map(|t| t[1]).collect();
    xs.sort_by(f32::total_cmp);
    ys.sort_by(f32::total_cmp);
    xs.dedup_by(|a, b| close(*a, *b));
    ys.dedup_by(|a, b| close(*a, *b));
    assert_eq!(
        (xs.len(), ys.len()),
        (2, 2),
        "W = 1000: tiles {:?} are not a 2 × 2 grid",
        lay.tiles
    );
    assert_no_text_overlap(&lay, w);

    // 900: no Elapsed column.
    let w = 900.0;
    let lay = layout_overview(&model, viewport_for(w), SCALE, 0);
    assert!(
        !lay.too_narrow && !lay.tiles.is_empty(),
        "W = 900 still draws the table"
    );
    assert_eq!(lay.columns.elapsed, None, "W = 900: Elapsed is dropped");
    assert_no_text_overlap(&lay, w);

    // 750: no Subagents column.
    let w = 750.0;
    let lay = layout_overview(&model, viewport_for(w), SCALE, 0);
    assert!(
        !lay.too_narrow && !lay.tiles.is_empty(),
        "W = 750 still draws the table"
    );
    assert_eq!(lay.columns.subagents, None, "W = 750: Subagents is dropped");
    assert_eq!(lay.columns.elapsed, None, "W = 750: Elapsed stays dropped");
    assert_no_text_overlap(&lay, w);

    // 700: only the header and the too-narrow line.
    let w = 700.0;
    let lay = layout_overview(&model, viewport_for(w), SCALE, 0);
    assert!(lay.too_narrow, "W = 700 is too narrow");
    assert!(
        lay.texts
            .iter()
            .any(|t| t.text == "Window too narrow — widen it to see sessions."),
        "W = 700: no `Window too narrow — widen it to see sessions.` text; texts: {:?}",
        lay.texts.iter().map(|t| &t.text).collect::<Vec<_>>()
    );
    assert!(lay.tiles.is_empty(), "W = 700: tiles drawn {:?}", lay.tiles);
    assert!(lay.squares.is_empty(), "W = 700: squares drawn");
    assert!(
        !lay.hits.iter().any(|(h, _)| matches!(
            h,
            OverviewHit::Row(..) | OverviewHit::Expand(..) | OverviewHit::Open(..)
        )),
        "W = 700: a row hit was laid out"
    );
    for r in &model.rows {
        assert!(
            !lay.texts.iter().any(|t| t.text.contains(&r.now_doing)
                || t.text == r.project
                || t.text == r.elapsed),
            "W = 700: row {} text was laid out",
            r.tab
        );
    }
    assert_no_text_overlap(&lay, w);
}

// ---- R9.3 ------------------------------------------------------------------------------

fn detail() -> DetailData {
    DetailData {
        tab: 7,
        pane: 0,
        project: "receipt-tracker".into(),
        agent_label: "Claude Code".into(),
        agent: AgentTag::Claude,
        bucket: Bucket::Working,
        branch_line: "main".into(),
        now_doing: "Bash · cargo test".into(),
        prompt: Some("fix the tests".into()),
        elapsed: "12m".into(),
        has_timeline: true,
        lanes: vec![
            Lane {
                label: "└ Explore the parser".into(),
                status: SubagentStatus::Done,
                start: 0.1,
                end: 0.4,
                duration: "3m 00s".into(),
                selected: false,
            },
            Lane {
                label: "└ Write the tests".into(),
                status: SubagentStatus::Running,
                start: 0.5,
                end: 1.0,
                duration: "5m".into(),
                selected: true,
            },
        ],
        main_lane: vec![
            Segment {
                kind: SegmentKind::Working,
                start: 0.0,
                end: 0.5,
            },
            Segment {
                kind: SegmentKind::WaitingOnSubagent,
                start: 0.5,
                end: 1.0,
            },
        ],
        ticks: vec![Tick {
            at: 0.5,
            label: "12:10".into(),
        }],
        events: vec![EventLine {
            time: "12:09".into(),
            text: "Task done · Explore the parser".into(),
            tone: EventTone::Done,
        }],
        selected: Some(1),
        panel: Some(SubagentPanel {
            title: "Write the tests".into(),
            subagent_type: "general-purpose".into(),
            status: SubagentStatus::Running,
            status_word: "running".into(),
            started: "12:05:00".into(),
            duration: "5m".into(),
            report: vec!["—".into()],
        }),
        resume_cmd: Some("claude --resume abc".into()),
    }
}

#[test]
fn r9_3_detail_side_by_side_at_1376_stacked_at_900() {
    let d = detail();

    let lay = layout_detail(&d, viewport_for(1376.0), SCALE, 0);
    let (Some(tl), Some(sp)) = (lay.timeline, lay.side_panel) else {
        panic!(
            "W = 1376: timeline {:?} and side panel {:?} must both be laid out",
            lay.timeline, lay.side_panel
        );
    };
    assert!(lay.side_by_side, "W = 1376: side panel beside the timeline");
    assert!(
        close(sp[2], 340.0),
        "W = 1376: side panel is {} wide, want 340",
        sp[2]
    );
    assert!(
        sp[0] >= tl[0] + tl[2] - TOL
            && overlap_area([0.0, tl[1], 1.0, tl[3]], [0.0, sp[1], 1.0, sp[3]]) > 0.0,
        "W = 1376: side panel {sp:?} is not beside the timeline {tl:?}"
    );

    let lay = layout_detail(&d, viewport_for(900.0), SCALE, 0);
    let (Some(tl), Some(sp)) = (lay.timeline, lay.side_panel) else {
        panic!(
            "W = 900: timeline {:?} and side panel {:?} must both be laid out",
            lay.timeline, lay.side_panel
        );
    };
    assert!(!lay.side_by_side, "W = 900: side panel stacked");
    assert!(
        sp[1] >= tl[1] + tl[3] - TOL,
        "W = 900: side panel {sp:?} is not below the timeline {tl:?}"
    );
}

// ---- R9.4 ------------------------------------------------------------------------------

/// The expanded row's index and its tree-line count.
const EXPANDED: usize = 2;
const TREE: usize = 5;
const ROWS: usize = 30;

fn thirty_rows() -> OverviewModel {
    let rows: Vec<SessionRowData> = (0..ROWS)
        .map(|i| {
            let mut r = row(i as u64 + 1, Bucket::Working, 0, false);
            if i == EXPANDED {
                r = row(i as u64 + 1, Bucket::Working, TREE, true);
            }
            r.project = format!("project-{i}");
            r
        })
        .collect();
    OverviewModel {
        rows,
        counts: Counts {
            all: ROWS,
            needs_you: 0,
            working: ROWS,
            idle: 0,
        },
        banner: Some(Banner {
            tab: 1,
            pane: 0,
            text: "Claude Code · project-0 — Approval needed".into(),
            more: 0,
        }),
        ..busy_model()
    }
}

/// The line index of row `i`: rows before the expanded one, then its tree lines.
fn line_of(i: usize) -> usize {
    if i <= EXPANDED {
        i
    } else {
        i + TREE
    }
}

fn row_hits(lay: &OverviewLayout) -> Vec<(u64, [f32; 4])> {
    let mut v: Vec<(u64, [f32; 4])> = lay
        .hits
        .iter()
        .filter_map(|(h, r)| match h {
            OverviewHit::Row(tab, _) => Some((*tab, *r)),
            _ => None,
        })
        .collect();
    v.sort_by(|a, b| a.1[1].total_cmp(&b.1[1]));
    v
}

fn fixed_hits(lay: &OverviewLayout) -> Vec<(OverviewHit, [f32; 4])> {
    lay.hits
        .iter()
        .filter(|(h, _)| matches!(h, OverviewHit::Filter(_) | OverviewHit::Banner(..)))
        .cloned()
        .collect()
}

fn texts_above(lay: &OverviewLayout, y: f32) -> Vec<(String, [f32; 4])> {
    lay.texts
        .iter()
        .filter(|t| t.rect[1] + t.rect[3] <= y + TOL)
        .map(|t| (t.text.clone(), t.rect))
        .collect()
}

fn inside(inner: [f32; 4], outer: [f32; 4]) -> bool {
    inner[0] >= outer[0] - TOL
        && inner[1] >= outer[1] - TOL
        && inner[0] + inner[2] <= outer[0] + outer[2] + TOL
        && inner[1] + inner[3] <= outer[1] + outer[3] + TOL
}

#[test]
fn r9_4_whole_line_scroll_clamps_and_keeps_the_fixed_rects() {
    let model = thirty_rows();
    let total = ROWS + TREE;
    let width = 1376.0;

    // A table area fitting 8 lines: the viewport height at which the layout reports 8.
    let height = (200..=2400)
        .map(|h| h as f32)
        .find(|&h| layout_overview(&model, [width + SIDE_PADDING, h], SCALE, 0).lines_visible == 8)
        .unwrap_or_else(|| {
            panic!("no viewport height in 200..=2400 gives a table fitting 8 lines")
        });
    let vp = [width + SIDE_PADDING, height];
    let base = layout_overview(&model, vp, SCALE, 0);
    assert_eq!(base.lines_total, total, "30 rows + 5 tree lines");
    let max = total - 8;
    assert_eq!(
        base.max_scroll(),
        max,
        "the offset is clamped to total − visible"
    );

    // Wheel notches through the handle: down past the end, then up past the top.
    let handle = OverviewHandle::new();
    handle.publish(base.hits.clone(), base.max_scroll());
    for k in 1..=total + 5 {
        handle.set_scroll(handle.scroll() + 1);
        assert_eq!(handle.scroll(), k.min(max), "after {k} notches down");
    }
    for k in 1..=total + 5 {
        handle.set_scroll(handle.scroll().saturating_sub(1));
        assert_eq!(
            handle.scroll(),
            max.saturating_sub(k),
            "after {k} notches up"
        );
    }

    assert_eq!(base.tiles.len(), 4, "four tiles");
    assert!(
        fixed_hits(&base)
            .iter()
            .any(|(h, _)| matches!(h, OverviewHit::Banner(..))),
        "the banner's hit is laid out"
    );
    assert!(
        fixed_hits(&base)
            .iter()
            .any(|(h, _)| matches!(h, OverviewHit::Filter(_))),
        "the filter tabs' hits are laid out"
    );
    let table = base.table;
    let header = texts_above(&base, table[1]);
    assert!(
        !header.is_empty(),
        "the header carries text above the table"
    );

    for s in 0..=total + 3 {
        let lay = layout_overview(&model, vp, SCALE, s);
        let eff = s.min(max);
        // Header, tiles, banner and filter keep their rects.
        assert_eq!(lay.tiles, base.tiles, "scroll {s}: tiles moved");
        assert_eq!(lay.table, table, "scroll {s}: the table box moved");
        assert_eq!(
            fixed_hits(&lay),
            fixed_hits(&base),
            "scroll {s}: banner / filter hits moved"
        );
        assert_eq!(
            texts_above(&lay, table[1]),
            header,
            "scroll {s}: the header's text moved"
        );
        // Past the end it is the same as the end.
        if s > max {
            assert_eq!(
                lay,
                layout_overview(&model, vp, SCALE, max),
                "scroll {s} is not clamped to {max}"
            );
        }
        // The offset is a line index: the first row on screen is the first row at or
        // after line `eff`, and no earlier row is drawn. While the offset sits inside the
        // expanded row's tree, that row's own tree lines may still carry its hit.
        let rows = row_hits(&lay);
        let in_tree = eff > EXPANDED && eff <= EXPANDED + TREE;
        let first = (0..ROWS).find(|&i| line_of(i) >= eff).expect("a row");
        let first_row = rows
            .iter()
            .find(|r| !(in_tree && r.0 == EXPANDED as u64 + 1))
            .map(|r| r.0);
        assert_eq!(
            first_row,
            Some(first as u64 + 1),
            "scroll {s}: the topmost row should be row {first}"
        );
        for (tab, _) in &rows {
            let i = *tab as usize - 1;
            if in_tree && i == EXPANDED {
                continue;
            }
            assert!(
                line_of(i) >= eff,
                "scroll {s}: row {i} above the offset is drawn"
            );
        }
        // No line drawn partially: every row hit, square and text that touches the table
        // box lies wholly inside it.
        for (tab, r) in &rows {
            assert!(
                inside(*r, table),
                "scroll {s}: row {tab} {r:?} leaves the table {table:?}"
            );
        }
        for q in &lay.squares {
            assert!(
                inside(q.rect, table),
                "scroll {s}: square {q:?} leaves the table {table:?}"
            );
        }
        for t in &lay.texts {
            if t.rect[1] + t.rect[3] > table[1] + TOL && overlap_area(t.rect, table) > 0.0 {
                assert!(
                    inside(t.rect, table),
                    "scroll {s}: text {:?} at {:?} is cut by the table {table:?}",
                    t.text,
                    t.rect
                );
            }
        }
    }
}
