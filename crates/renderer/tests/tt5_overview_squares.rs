//! TT5 — acceptance tests for `specs/session-overview-ui` R5 (D8, D21), the layout half:
//! a row's subagent squares wrap to at most two lines of the 220 px Subagents column, the
//! last slot turning into `+{k}` (R5.2), and every square carries its status colour (R5.1).
//!
//! Headless: `layout_overview` over a hand-built `OverviewModel`, no GPU.

use renderer::overview_layout::{layout_overview, LaidSquare, OverviewLayout};
use renderer::overview_model::{
    AgentTag, Bucket, OverviewModel, SessionRowData, SubagentCell, SubagentStatus,
};

/// Wide enough that the content box is the full 1376 logical px (W ≥ 1172: every column at
/// its D15 width, Subagents 220).
const VIEWPORT: [f32; 2] = [1440.0, 900.0];
const SCALE: f64 = 1.0;
const TAB: u64 = 3;

fn model_with(cells: Vec<SubagentCell>) -> OverviewModel {
    let row = SessionRowData {
        tab: TAB,
        pane: 0,
        bucket: Bucket::Working,
        project: "terminal".to_string(),
        agent_label: "Claude Code".to_string(),
        agent: AgentTag::Claude,
        branch_line: "main".to_string(),
        has_branch: true,
        now_doing: "Working".to_string(),
        elapsed: "3m".to_string(),
        subagents: cells,
        subagent_summary: "—".to_string(),
        ..SessionRowData::default()
    };
    OverviewModel {
        rows: vec![row],
        ..OverviewModel::default()
    }
}

fn layout(model: &OverviewModel) -> OverviewLayout {
    let l = layout_overview(model, VIEWPORT, SCALE, 0);
    assert_eq!(
        l.columns.subagents,
        Some(220.0),
        "precondition: the Subagents column is 220 logical px at scale 1"
    );
    l
}

/// The laid squares in reading order: by line (y), then by x.
fn reading_order(squares: &[LaidSquare]) -> Vec<LaidSquare> {
    let mut v = squares.to_vec();
    v.sort_by(|a, b| {
        a.rect[1]
            .total_cmp(&b.rect[1])
            .then(a.rect[0].total_cmp(&b.rect[0]))
    });
    v
}

/// `#RRGGBB` as straight RGBA, opaque.
fn hex(rgb: u32) -> [f32; 4] {
    [
        ((rgb >> 16) & 0xFF) as f32 / 255.0,
        ((rgb >> 8) & 0xFF) as f32 / 255.0,
        (rgb & 0xFF) as f32 / 255.0,
        1.0,
    ]
}

fn assert_color(got: [f32; 4], want: u32, what: &str) {
    let w = hex(want);
    let close = got.iter().zip(w.iter()).all(|(g, w)| (g - w).abs() < 0.002);
    assert!(close, "{what}: got {got:?}, want #{want:06X} = {w:?}");
}

// --- R5.1 -------------------------------------------------------------------------------

#[test]
fn r5_1_squares_are_drawn_in_their_status_colours() {
    let model = model_with(vec![
        SubagentCell::Square(SubagentStatus::Running),
        SubagentCell::Square(SubagentStatus::Done),
        SubagentCell::Square(SubagentStatus::Failed),
    ]);
    let l = layout(&model);

    let squares = reading_order(&l.squares);
    assert_eq!(squares.len(), 3, "three squares: {squares:#?}");
    assert_eq!(
        squares.iter().map(|s| s.cell).collect::<Vec<_>>(),
        vec![
            SubagentCell::Square(SubagentStatus::Running),
            SubagentCell::Square(SubagentStatus::Done),
            SubagentCell::Square(SubagentStatus::Failed),
        ],
        "in log order"
    );
    assert_color(squares[0].color, 0xE8744F, "running square");
    assert_color(squares[1].color, 0x6FA67F, "done square");
    assert_color(squares[2].color, 0xE5534B, "failed square");
}

// --- R5.2 -------------------------------------------------------------------------------

#[test]
fn r5_2_thirty_two_subagents_draw_twenty_seven_squares_and_plus_five() {
    // 32 subagents in log order: 20 done, then 12 running.
    let cells: Vec<SubagentCell> = (0..32)
        .map(|i| {
            SubagentCell::Square(if i < 20 {
                SubagentStatus::Done
            } else {
                SubagentStatus::Running
            })
        })
        .collect();
    let model = model_with(cells);
    let l = layout(&model);

    let squares = reading_order(&l.squares);
    assert_eq!(
        squares.len(),
        28,
        "two lines of 14 slots, all used: {squares:#?}"
    );
    let drawn = squares
        .iter()
        .filter(|s| matches!(s.cell, SubagentCell::Square(_)))
        .count();
    assert_eq!(drawn, 27, "27 squares are drawn");
    assert_eq!(
        squares[27].cell,
        SubagentCell::More(5),
        "the 28th slot reads `+5`"
    );
}
