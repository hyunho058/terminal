//! TT11 — acceptance test for `specs/session-overview-ui` R11.2 (D26): the row's prompt is
//! drawn with exactly one pair of quotes. The app's model already carries the prompt as
//! `“…”`; `layout_overview` must not wrap it again (which drew `““…”`), and when it has to
//! ellipsise it does so *inside* the existing pair.
//!
//! Pure layout: no GPU. The prompt's text run is found inside the row's `OverviewHit::Row`
//! rect, as the run carrying the prompt's own words.

use renderer::overview_layout::{layout_overview, LaidText, OverviewLayout};
use renderer::overview_model::{
    AgentTag, Bucket, Counts, Filter, OverviewHit, OverviewModel, SessionRowData,
};

const SCALE: f64 = 1.0;
/// The widest content box (D15's 1376 px cap), so a 32-character prompt fits its column.
const VIEWPORT: [f32; 2] = [1440.0, 900.0];

const SHORT_TAB: u64 = 1;
const LONG_TAB: u64 = 2;

/// 300 characters of prompt, distinctive enough not to collide with any other run.
fn long_body() -> String {
    let body: String = "quokka ".repeat(43).chars().take(300).collect();
    assert_eq!(body.chars().count(), 300);
    body
}

fn row(tab: u64, prompt: String) -> SessionRowData {
    SessionRowData {
        tab,
        pane: 0,
        bucket: Bucket::Working,
        project: "terminal".into(),
        agent_label: "Claude Code".into(),
        agent: AgentTag::Claude,
        branch_line: "main".into(),
        has_branch: true,
        now_doing: "Bash · cargo test".into(),
        prompt: Some(prompt),
        elapsed: "3m".into(),
        subagent_summary: "—".into(),
        ..SessionRowData::default()
    }
}

fn model() -> OverviewModel {
    OverviewModel {
        resolving: false,
        rows: vec![
            row(SHORT_TAB, "“fix the duplicate G7 numbering”".into()),
            row(LONG_TAB, format!("“{}”", long_body())),
        ],
        counts: Counts {
            all: 2,
            needs_you: 0,
            working: 2,
            idle: 0,
        },
        banner: None,
        filter: Filter::All,
        ..OverviewModel::default()
    }
}

fn row_rect(lay: &OverviewLayout, tab: u64) -> [f32; 4] {
    lay.hits
        .iter()
        .find_map(|(h, r)| match h {
            OverviewHit::Row(t, 0) if *t == tab => Some(*r),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no Row({tab}, 0) hit in {:#?}", lay.hits))
}

fn inside(r: [f32; 4], t: &LaidText) -> bool {
    let cx = t.rect[0] + t.rect[2] / 2.0;
    let cy = t.rect[1] + t.rect[3] / 2.0;
    cx >= r[0] && cx <= r[0] + r[2] && cy >= r[1] && cy <= r[1] + r[3]
}

/// The one text run inside `tab`'s row carrying `word`.
fn prompt_run<'a>(lay: &'a OverviewLayout, tab: u64, word: &str) -> &'a str {
    let r = row_rect(lay, tab);
    let runs: Vec<&LaidText> = lay
        .texts
        .iter()
        .filter(|t| inside(r, t) && t.text.contains(word))
        .collect();
    assert_eq!(
        runs.len(),
        1,
        "exactly one prompt run in row {tab}: {:#?}",
        lay.texts
            .iter()
            .filter(|t| inside(r, t))
            .map(|t| &t.text)
            .collect::<Vec<_>>()
    );
    &runs[0].text
}

#[test]
fn r11_2_prompt_is_drawn_with_one_pair_of_quotes() {
    let lay = layout_overview(&model(), VIEWPORT, SCALE, 0);

    let short = prompt_run(&lay, SHORT_TAB, "duplicate G7");
    assert_eq!(short, "“fix the duplicate G7 numbering”");

    let long = prompt_run(&lay, LONG_TAB, "quokka");
    assert!(
        long.starts_with('“') && !long.starts_with("““"),
        "the long prompt starts with exactly one `“`: {long:?}"
    );
    assert!(
        long.ends_with("…”"),
        "the long prompt ends with `…”`: {long:?}"
    );
    assert!(!long.contains("““"), "no doubled opening quote: {long:?}");
}
