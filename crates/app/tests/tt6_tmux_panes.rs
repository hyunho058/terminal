//! Acceptance tests for session-flow-viewer R6: the tabmeta worker publishes panes while the
//! flow panel is open (D9, D14). Written from the spec's Thens; every item is reached through
//! the `app` crate root (D17).

use std::path::PathBuf;

use app::{
    graph_pass, panes_of, parse_clients, parse_panes, select_pane, ClientRow, PanePass, PaneRow,
    MAX_PANES,
};

fn row(window: &str, active: bool, pid: u32) -> PaneRow {
    PaneRow {
        window: window.to_string(),
        active,
        pid,
    }
}

fn client() -> ClientRow {
    ClientRow {
        tty: PathBuf::from("/dev/ttys004"),
        pane_pid: 4100,
        window: "work:1".to_string(),
    }
}

// R6.1
#[test]
fn r6_1_parse_panes_returns_three_rows_and_skips_malformed() {
    let out = "work:1 1 4100\nwork:1 0 4200\nmisc:0 1 5000\nthis is not a pane line\n";
    let rows = parse_panes(out);
    assert_eq!(
        rows,
        vec![
            row("work:1", true, 4100),
            row("work:1", false, 4200),
            row("misc:0", true, 5000),
        ]
    );
}

// R6.2
#[test]
fn r6_2_parse_clients_yields_one_row_and_select_pane_finds_its_pid() {
    let out = "/dev/ttys004 4100 work:1\n/dev/ttys005 4300\n";
    let clients = parse_clients(out);
    assert_eq!(clients, vec![client()]);
    assert_eq!(select_pane(&[(4, 4100)], 4), Some(4100));
}

// R6.3
#[test]
fn r6_3_panes_of_marks_active_by_client_pid_not_pane_active() {
    // `#{pane_active}` deliberately disagrees with the client pid.
    let panes = vec![
        row("work:1", false, 4100),
        row("work:1", true, 4200),
        row("misc:0", true, 5000),
    ];
    let got = panes_of(Some(&client()), &panes);
    let pids: Vec<u32> = got.iter().map(|p| p.pid).collect();
    assert_eq!(pids, vec![4100, 4200]);
    let active: Vec<u32> = got.iter().filter(|p| p.active).map(|p| p.pid).collect();
    assert_eq!(active, vec![4100]);
}

// R6.3
#[test]
fn r6_3_panes_of_is_empty_when_no_pane_matches_the_client_pid() {
    let panes = vec![row("work:1", true, 4200), row("work:1", false, 4300)];
    assert!(panes_of(Some(&client()), &panes).is_empty());
}

// R6.4
#[test]
fn r6_4_single_pane_window_yields_empty() {
    let panes = vec![row("work:1", true, 4100)];
    assert!(panes_of(Some(&client()), &panes).is_empty());
}

// R6.4
#[test]
fn r6_4_ten_pane_window_yields_max_panes_including_active() {
    assert_eq!(MAX_PANES, 8);
    // The client's pane is the last of ten, so a plain truncation would drop it.
    let mut panes: Vec<PaneRow> = (0..9).map(|i| row("work:1", false, 5000 + i)).collect();
    panes.push(row("work:1", false, 4100));
    let got = panes_of(Some(&client()), &panes);
    assert_eq!(got.len(), MAX_PANES);
    assert!(got.iter().any(|p| p.pid == 4100 && p.active));
}

// R6.6
#[test]
fn r6_6_pane_pass_over_all_eight_combinations() {
    for just_enabled in [false, true] {
        for due in [false, true] {
            assert_eq!(
                graph_pass(false, just_enabled, due),
                PanePass::Skip,
                "not wanted must skip (just_enabled={just_enabled}, due={due})"
            );
        }
    }
    for due in [false, true] {
        assert_eq!(
            graph_pass(true, true, due),
            PanePass::Derive,
            "just enabled must derive regardless of due={due}"
        );
    }
    assert_eq!(graph_pass(true, false, true), PanePass::Derive);
    assert_eq!(graph_pass(true, false, false), PanePass::Skip);
}

// R6.7
#[test]
fn r6_7_no_client_or_no_panes_degrades_to_empty() {
    let rows = vec![row("work:1", true, 4100), row("work:1", false, 4200)];
    assert!(panes_of(None, &rows).is_empty());
    assert!(panes_of(Some(&client()), &[]).is_empty());
}
