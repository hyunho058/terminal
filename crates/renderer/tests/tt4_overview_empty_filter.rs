//! TT4 — acceptance test for `specs/session-overview-ui` R4.4 (D6): a filter that matches no
//! session leaves the table holding `No sessions in this filter.`, centred in it.
//!
//! Written from the spec's Then by the checker side, against the layout contract
//! (`overview_layout::layout_overview`) — no GPU: the layout is the text the frame draws.

use renderer::overview_layout::layout_overview;
use renderer::overview_model::{Counts, Filter, OverviewModel};

// R4.4
#[test]
fn r4_4_empty_filter_shows_no_sessions_in_this_filter() {
    // Given: no Idle session (one Working, one Needs you) and filter `Idle` — so the
    // filtered table is empty while the counts are not.
    let model = OverviewModel {
        resolving: false,
        rows: Vec::new(),
        counts: Counts {
            all: 2,
            needs_you: 1,
            working: 1,
            idle: 0,
        },
        filter: Filter::Idle,
        ..OverviewModel::default()
    };

    // When.
    let layout = layout_overview(&model, [1600.0, 1000.0], 1.0, 0);

    // Then: the table contains the text.
    let [tx, ty, tw, th] = layout.table;
    let hit = layout.texts.iter().find(|t| {
        let cx = t.rect[0] + t.rect[2] / 2.0;
        let cy = t.rect[1] + t.rect[3] / 2.0;
        t.text == "No sessions in this filter."
            && cx >= tx
            && cx < tx + tw
            && cy >= ty
            && cy < ty + th
    });
    assert!(
        hit.is_some(),
        "`No sessions in this filter.` inside the table {:?}; texts: {:?}",
        layout.table,
        layout
            .texts
            .iter()
            .map(|t| (t.text.as_str(), t.rect))
            .collect::<Vec<_>>()
    );
}
