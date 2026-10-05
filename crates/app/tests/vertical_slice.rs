//! The vertical slice, end to end: a keystroke becomes bytes, reaches a real
//! zsh, comes back through the reader thread, lands in the grid, and is drawn.
//!
//! Nothing else in the workspace crosses those seams. `input.rs` has 105 unit
//! tests that pin `encode_key`'s output; `parser.rs` has 74 that pin what bytes
//! do to a grid; `golden_frame.rs` pins that a snapshot reaches pixels. Every one
//! of them keeps passing when the *joins between them* break, because none of
//! them contains a join. Measured, not assumed: making [`Session::write`] discard
//! its argument — a terminal that types nothing at all — left the workspace suite
//! at 1460 passed / 0 failed.
//!
//! These tests are deliberately **not** `#[ignore]`d, unlike the older
//! `term-core/tests/pty_smoke.rs`. A gate that never runs is not a gate, and the
//! reason those are skipped (shell startup latency) is answered here with a
//! deadline and a diagnostic instead of by opting out. `SKIP_SHELL_TESTS=1` opts
//! out explicitly for an environment with no usable `/bin/zsh`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use app::encode_key;
use app::{release_drag, release_is_click, SelectionDrag};
use app::{Session, TabId, Wake};
use common::config::Config;
use renderer::selection_layer::SelectionLayer;
use renderer::testing::{holds_text, is_ink, CellGeometry};
use renderer::{CapturedFrame, Renderer};
use term_core::Color;
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// How long the shell gets to start, run one command, and echo the result.
/// Generous: this bounds a *failure*, and a slow machine must not turn a working
/// terminal into a red build.
const SHELL_TIMEOUT: Duration = Duration::from_secs(15);

const COLS: u16 = 80;
const ROWS: u16 = 24;

fn fg_bg() -> (Color, Color) {
    let cfg = Config::default();
    (Color(cfg.theme.foreground), Color(cfg.theme.background))
}

/// A live session with a real zsh, or `None` when shell tests are opted out of.
///
/// No `EventLoopProxy` anywhere: `Session::spawn` takes a [`Wake`] callback, which
/// is the entire reason this test can exist. On macOS a winit `EventLoop` may be
/// built only on the process's main thread and the test harness runs every test on
/// a spawned one, so a session that demanded a proxy would be permanently
/// unreachable from here.
fn live_session() -> Option<Session> {
    live_session_as(TabId(0))
}

/// [`live_session`] for tab `id`, for a test that needs two tabs.
fn live_session_as(id: TabId) -> Option<Session> {
    // `term_core::testing`, not a copy of the check: this file wrote the
    // `SKIP_SHELL_TESTS` contract down first and then three more copies of it
    // appeared, because the version extracted into `app::session` was
    // `#[cfg(test)]` and an integration test is a separate crate.
    if !term_core::testing::shell_tests_enabled() {
        return None;
    }
    let (fg, bg) = fg_bg();
    let wake: Wake = Box::new(|_| {});
    match Session::spawn(id, COLS, ROWS, fg, bg, wake) {
        Ok(s) => Some(s),
        Err(e) => panic!(
            "could not spawn a session ({e}). This is the only test that covers \
             keystroke → shell → grid, so it fails rather than skipping. Set \
             SKIP_SHELL_TESTS=1 to opt out deliberately."
        ),
    }
}

/// Type `text` the way the app does: one `KeyEvent` per character, through the
/// real `encode_key`, into the real `Session::write`.
///
/// Going through `encode_key` rather than writing the bytes directly is the
/// point — it is what puts the input encoder inside the tested path instead of
/// beside it.
fn type_text(session: &mut Session, text: &str) {
    for ch in text.chars() {
        let s = ch.to_string();
        let key = Key::Character(s.clone().into());
        let bytes = encode_key(&key, Some(&s), ModifiersState::empty(), false)
            .unwrap_or_else(|| panic!("no shell encoding for {ch:?}"));
        session.write(&bytes).expect("write to the shell");
    }
}

/// Frame size for the pixel leg. Large enough that the default cell metrics give
/// a grid worth asserting over.
const FRAME_W: u32 = 1200;
const FRAME_H: u32 = 800;

/// The headless renderer, or `None` when GPU tests are opted out of.
///
/// `renderer::testing`, not a copy. The `NoFont`-is-never-skipped rule this
/// function's own comment used to justify was stated *here* and not honoured in
/// `golden_frame.rs`, which is the file it delegates the font pipeline to — so on
/// the headless runner that sets `SKIP_GPU_TESTS`, the rule protected nothing.
fn headless_renderer(cfg: &Config) -> Option<Renderer> {
    renderer::testing::headless(FRAME_W, FRAME_H, cfg, "the pixel leg")
}

/// Press Return, through the same encoder.
fn press_enter(session: &mut Session) {
    let bytes = encode_key(
        &Key::Named(NamedKey::Enter),
        None,
        ModifiersState::empty(),
        false,
    )
    .expect("Enter must encode");
    session.write(&bytes).expect("write Enter");
}

/// Pump the reader thread into the grid until `needle` shows up, or the deadline
/// passes. Returns the grid text either way; the caller asserts.
fn grid_until(session: &mut Session, needle: &str) -> String {
    let deadline = Instant::now() + SHELL_TIMEOUT;
    loop {
        session.drain();
        let text = session.terminal.snapshot().to_text();
        if text.contains(needle) || Instant::now() >= deadline {
            return text;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The regression this file exists for: what is typed reaches the shell, and what
/// the shell answers reaches the grid.
///
/// The marker is assembled from a variable so the *echo* of the command line
/// cannot contain it. Asserting on the echo alone would pass on a shell that
/// never ran anything; this only passes if a command actually executed and its
/// output came back through the reader thread.
#[test]
fn a_typed_command_runs_and_its_output_reaches_the_grid() {
    let Some(mut session) = live_session() else {
        return;
    };
    type_text(&mut session, "m=SLICE; printf '%s<ok>\\n' \"$m\"");
    press_enter(&mut session);

    let text = grid_until(&mut session, "SLICE<ok>");
    assert!(
        text.contains("SLICE<ok>"),
        "the shell never answered within {SHELL_TIMEOUT:?}. Either the keystrokes \
         did not reach the PTY, or its output did not reach the grid. Grid was:\n{text}"
    );
}

/// The echo half, stated separately so a failure says *which* direction broke.
///
/// If only this one fails, bytes reach the tty but the shell produces nothing; if
/// only the test above fails, the command line arrived but its output did not come
/// back. Together they bracket the round trip.
#[test]
fn typed_characters_are_echoed_back_into_the_grid() {
    let Some(mut session) = live_session() else {
        return;
    };
    // No Enter: nothing is executed, so anything that appears is the tty echoing
    // what was written to it.
    type_text(&mut session, "ECHOPROBE");

    let text = grid_until(&mut session, "ECHOPROBE");
    assert!(
        text.contains("ECHOPROBE"),
        "typed characters never came back as echo within {SHELL_TIMEOUT:?} — \
         the write side of the PTY is broken. Grid was:\n{text}"
    );
}

/// All the way to pixels: bytes from a real shell, through the grid, onto the
/// GPU target.
///
/// The one test in the workspace that spans every seam at once. `golden_frame.rs`
/// proves a *constructed* snapshot reaches the screen; this proves the snapshot
/// the shell actually produced does, which is the thing the user looks at.
#[test]
fn shell_output_reaches_actual_pixels() {
    let Some(mut session) = live_session() else {
        return;
    };
    let cfg = Config::default();
    let Some(mut r) = headless_renderer(&cfg) else {
        return;
    };

    const MARKER: &str = "PIXEL<ok>";

    type_text(&mut session, "m=PIXEL; printf '%s<ok>\\n' \"$m\"");
    press_enter(&mut session);
    let text = grid_until(&mut session, MARKER);
    assert!(
        text.contains(MARKER),
        "shell output never reached the grid; the pixel leg is untestable. Grid:\n{text}"
    );

    let snapshot = session.terminal.snapshot();
    let frame = r.capture(&snapshot).expect("read the rendered frame back");
    let bg = frame.modal_pixel();

    // The cell rect and the ink threshold come from `renderer::testing`, shared with
    // `renderer/tests/golden_frame.rs`. Both files derived them by hand, each under
    // a comment saying the two "have to agree" — and they did not: this side
    // truncated the far edge of the rect where the other floored it after clamping,
    // so "this cell holds ink" already meant two things.
    let geo = CellGeometry::of(&r);
    let (cols, rows) = (geo.cols().min(snapshot.cols), geo.rows().min(snapshot.rows));
    let cell_has_ink = |col: u16, row: u16| geo.cell_has_ink(&frame, bg, col, row);
    let char_at = |col: u16, row: u16| snapshot.cell(col, row).map(|c| c.ch).unwrap_or(' ');

    // Every printable cell **of the marker row** must carry ink.
    //
    // Scoped to that row deliberately, and the scope is the assertion's whole
    // correctness. Everything else on this screen is the developer's *real* zsh
    // prompt: `Pty::spawn` chains the user's own `.zshrc` on purpose
    // (shell-integration D13), so p10k, starship and a Nerd Font are supported
    // configurations rather than accidents. The renderer, equally deliberately,
    // emits no glyph instance for a codepoint no loaded face covers (`atlas.rs`),
    // and the loaded set is Menlo / SF Mono / AppleSDGothicNeo — no Powerline
    // separators, no private-use glyphs, no emoji. Asserted over the whole grid
    // this test would therefore fail on any machine whose prompt uses one, while
    // the terminal renders exactly as designed, and the failure would be neither
    // reproducible elsewhere nor fixable in this crate.
    //
    // The marker row carries none of that ambiguity: this test produced it, it is
    // plain ASCII, and it is the thing the test's name claims — output from a real
    // shell that reached real pixels.
    //
    // The converse is not asserted at all. A glyph may bleed a pixel of
    // antialiasing past its advance width into the blank cell beside it —
    // measured, not assumed: the `%` in this prompt inks the empty cells on both
    // sides of it. That is correct rendering. `golden_frame.rs` states the same
    // asymmetry from the constructed-snapshot side.
    let row_text = |row: u16| (0..cols).map(|c| char_at(c, row)).collect::<String>();
    let marker_row = (0..rows)
        .find(|&row| row_text(row).contains(MARKER))
        .unwrap_or_else(|| panic!("{MARKER:?} is in the grid text but on no row of the snapshot"));

    let missing: Vec<String> = (0..cols)
        .filter(|&col| holds_text(&snapshot, col, marker_row) && !cell_has_ink(col, marker_row))
        .map(|col| {
            format!(
                "({col},{marker_row}) holds {:?} but drew nothing",
                char_at(col, marker_row)
            )
        })
        .collect();
    assert!(
        missing.is_empty(),
        "{} cells of the shell's own output reached the grid and not the GPU:\n{}",
        missing.len(),
        missing.join("\n")
    );

    let inked = (0..rows)
        .flat_map(|row| (0..cols).map(move |col| (col, row)))
        .filter(|&(col, row)| cell_has_ink(col, row))
        .count();
    assert!(
        inked > 0,
        "the frame is a solid field of {bg:?}: the shell produced output that \
         reached the grid, and none of it reached the GPU"
    );
    // Guards the metric itself: a frame where every cell reads as ink — an
    // uncleared target, say — would satisfy the assertions above trivially.
    let total = cols as usize * rows as usize;
    assert!(
        inked < total,
        "every one of the {total} cells reads as ink, so \"has ink\" is measuring \
         something other than drawn content"
    );
}

/// selection-scroll-anchor R5.8: `Session::drain` settles the selection once after its
/// chunk loop, so shell output that repaints a selected row clears the selection without
/// the caller settling anything itself.
///
/// The selection is made on a blank row well below the prompt, *after* the command line
/// has been echoed, and a drain with nothing new must keep it. The command then moves
/// the cursor onto that row and prints there. So the clear can only have come from the
/// command's output, through `drain`. As in the test above, the marker is assembled
/// from a variable so the echo cannot contain it.
///
/// Falsified: with the `settle_selection` call removed from `drain`, the final
/// assertion fails. That holds only while the shell emits no `?2026l` around this
/// output: a prompt theme that wraps its redraws in synchronized updates would clear
/// the selection at the `?2026l` itself (D5), and this test would pass without the
/// drain's settle. The zsh this runs against wraps nothing.
#[test]
fn draining_output_that_repaints_a_selected_row_clears_the_selection() {
    let Some(mut session) = live_session() else {
        return;
    };
    // 0-based row 12 is the `13` in the CUP below, far from the prompt on row 0 or 1.
    const ROW: u16 = 12;
    const MARKER: &str = "OVERWRITE";

    type_text(&mut session, "m=OVER; printf '\\033[13;1H%sWRITE' \"$m\"");
    let text = grid_until(&mut session, "%sWRITE");
    assert!(
        text.contains("%sWRITE"),
        "the command line was never echoed within {SHELL_TIMEOUT:?}. Grid was:\n{text}"
    );

    let t = &mut session.terminal;
    let row: String = t
        .snapshot()
        .to_text()
        .lines()
        .nth(usize::from(ROW))
        .unwrap_or_default()
        .to_string();
    assert!(
        row.trim().is_empty(),
        "precondition: row {ROW} is blank before the command runs, read {row:?}"
    );
    let (anchor, focus) = (t.point_at(0, ROW), t.point_at(8, ROW));
    t.set_selection(anchor, focus);

    // A drain that brings nothing new settles too, and must keep a selection whose
    // text has not changed.
    session.drain();
    assert!(
        session.terminal.selection().is_some(),
        "a settle over unchanged text cleared the selection"
    );

    press_enter(&mut session);
    let text = grid_until(&mut session, MARKER);
    assert!(
        text.contains(MARKER),
        "the command's output never reached the grid within {SHELL_TIMEOUT:?}. \
         Grid was:\n{text}"
    );
    assert_eq!(
        session.terminal.selection(),
        None,
        "output repainted the selected row and `drain` returned, but the selection \
         is still set: `Session::drain` did not settle it. Grid was:\n{text}"
    );
    assert_eq!(session.terminal.selection_text(), None);
}

// ---- selection-scroll-anchor: a drag selection over a real shell's output ----

/// The visible rows as text, one char per cell, so an index into a row is a column.
fn screen_rows(session: &Session) -> Vec<String> {
    let snap = session.terminal.snapshot();
    (0..ROWS)
        .map(|r| {
            (0..COLS)
                .map(|c| snap.cell(c, r).map_or(' ', |x| x.ch))
                .collect()
        })
        .collect()
}

/// The lowest viewport row that reads exactly `line`. Exact, so the echo of the
/// command that printed it (`printf '...bravo...'`) can never be the match.
fn row_of(session: &Session, line: &str) -> Option<u16> {
    screen_rows(session)
        .iter()
        .rposition(|l| l.trim_end() == line)
        .map(|r| r as u16)
}

/// Drain until `line` is a whole row on screen, or fail with the grid.
fn drain_until_row(session: &mut Session, line: &str) -> u16 {
    let deadline = Instant::now() + SHELL_TIMEOUT;
    loop {
        session.drain();
        if let Some(row) = row_of(session, line) {
            return row;
        }
        assert!(
            Instant::now() < deadline,
            "the shell never printed the line {line:?} within {SHELL_TIMEOUT:?}. Grid was:\n{}",
            screen_rows(session).join("\n")
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A cell's dominant colour: its background, whatever the glyph (`golden_frame.rs`'s
/// argument for the mode over a fixed sample pixel applies unchanged).
fn cell_mode(frame: &CapturedFrame, geo: &CellGeometry, col: u16, row: u16) -> [u8; 4] {
    let (x0, y0, x1, y1) = geo.cell_rect(col, row, frame);
    let mut counts: HashMap<[u8; 4], u32> = HashMap::new();
    for y in y0..y1 {
        for x in x0..x1 {
            *counts.entry(frame.pixel(x, y)).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|&(_, n)| n)
        .map(|(px, _)| px)
        .expect("a cell inside the frame has pixels")
}

/// The cells the selection tint lights in the real frame. The same snapshot is
/// drawn with and without its `selection`, and a cell counts when its dominant
/// colour differs between the two. A shell prompt or an app may paint backgrounds
/// of its own, and comparing against the frame's background would count those;
/// comparing a frame against itself cancels them.
fn tinted(r: &mut Renderer, session: &Session) -> Vec<(u16, u16)> {
    let snap = session.terminal.snapshot();
    let mut bare = snap.clone();
    bare.selection = None;
    let with = r.capture(&snap).expect("read the frame with the selection");
    let without = r.capture(&bare).expect("read the frame without it");
    let geo = CellGeometry::of(r);
    let mut lit = Vec::new();
    for row in 0..ROWS.min(geo.rows()) {
        for col in 0..COLS.min(geo.cols()) {
            if is_ink(
                cell_mode(&with, &geo, col, row),
                cell_mode(&without, &geo, col, row),
            ) {
                lit.push((col, row));
            }
        }
    }
    lit
}

fn row_cells(row: u16, cols: std::ops::RangeInclusive<u16>) -> Vec<(u16, u16)> {
    cols.map(|c| (c, row)).collect()
}

/// selection-scroll-anchor, end to end: the user's own check, with a real zsh,
/// the app's drag type, its copy source and real pixels.
///
/// Every other selection test drives one layer: `term-core`'s with fed bytes,
/// `drag.rs`'s with a fixture terminal, `golden_frame.rs`'s with a constructed
/// grid. This one crosses all of them at once:
/// keystroke → zsh → PTY reader → `drain` → grid → `SelectionDrag` →
/// `Terminal::selection_text` (what Cmd+C copies) → `Renderer::capture`.
///
/// 1. Drag over `bravo`, then run a command whose output scrolls the screen. The
///    highlight rides up with the text, and copy still returns it.
/// 2. Push `bravo` into scrollback. Copy still returns it. Scroll back and the
///    highlight is drawn there, and a new drag over a history line works.
/// 3. A press on tab A released with tab B in front, and a round trip back to A:
///    neither is a click, so no selection is cleared and no link would open.
///
/// Only winit's delivery of the mouse event is left out. `drag.rs`'s source-scan
/// test pins that the event loop hands every press, move and release to this type.
///
/// Falsified three ways, each red at the step named:
/// - projecting only at the live bottom (the pre-feature `selection-model` D9)
///   fails step 2's tint;
/// - projecting stored lines as viewport rows fails step 1's tint;
/// - a release that ignores tabs fails step 3.
#[test]
fn a_drag_selection_follows_shell_output_into_scrollback() {
    let Some(mut s) = live_session() else {
        return;
    };
    let cfg = Config::default();
    let Some(mut r) = headless_renderer(&cfg) else {
        return;
    };
    r.add_layer(Box::new(SelectionLayer::new()));
    let a = TabId(0);

    // Fill the screen first, so the next output scrolls it.
    type_text(&mut s, "seq 100; printf 'alpha\\nbravo\\ncharlie\\n'");
    press_enter(&mut s);
    drain_until_row(&mut s, "charlie");
    let row = drain_until_row(&mut s, "bravo");

    // 1. Drag over `bravo`, release, and let output scroll it.
    let mut drag = SelectionDrag::press(a, (0, row), &s.terminal);
    assert!(
        drag.drive(a, (4, row), &mut s.terminal),
        "a move off the press cell selects"
    );
    assert!(
        !release_drag(Some(drag), a, &mut s.terminal),
        "a drag is no click"
    );
    assert_eq!(s.terminal.selection_text().as_deref(), Some("bravo"));
    assert_eq!(
        tinted(&mut r, &s),
        row_cells(row, 0..=4),
        "the tint is on bravo"
    );

    type_text(&mut s, "seq 5 | sed 's/^/n/'");
    press_enter(&mut s);
    drain_until_row(&mut s, "n5");
    let moved = drain_until_row(&mut s, "bravo");
    assert!(
        moved < row,
        "precondition: the output scrolled bravo up ({row} -> {moved})"
    );
    assert_eq!(
        s.terminal.selection_text().as_deref(),
        Some("bravo"),
        "after the screen scrolled, Cmd+C still copies the text that was selected"
    );
    assert_eq!(
        tinted(&mut r, &s),
        row_cells(moved, 0..=4),
        "the tint rode up with bravo from row {row} to row {moved}"
    );

    // 2. Into scrollback, then scrolled back to.
    type_text(&mut s, "seq 80 | sed 's/^/m/'");
    press_enter(&mut s);
    drain_until_row(&mut s, "m80");
    assert_eq!(
        row_of(&s, "bravo"),
        None,
        "precondition: bravo left the live screen"
    );
    assert_eq!(
        s.terminal.selection_text().as_deref(),
        Some("bravo"),
        "copy reads a selection that has scrolled into history"
    );
    let mut back = 0;
    while row_of(&s, "bravo").is_none() {
        s.terminal.scroll_up(1);
        back += 1;
        assert!(back <= 200, "bravo never came back into view");
    }
    let shown = row_of(&s, "bravo").expect("in view");
    assert_eq!(
        tinted(&mut r, &s),
        row_cells(shown, 0..=4),
        "scrolled back {back} lines, the tint is drawn on bravo"
    );
    let history = row_of(&s, "charlie").expect("charlie is in view too");
    let mut drag = SelectionDrag::press(a, (0, history), &s.terminal);
    assert!(drag.drive(a, (6, history), &mut s.terminal));
    assert_eq!(s.terminal.selection_text().as_deref(), Some("charlie"));
    assert_eq!(
        tinted(&mut r, &s),
        row_cells(history, 0..=6),
        "a drag made while scrolled back is drawn"
    );

    // 3. Releases with another tab in front.
    let b = TabId(1);
    let mut sb = live_session_as(b).expect("shell tests are enabled: a second tab spawns");
    type_text(&mut sb, "echo tab-b-text");
    press_enter(&mut sb);
    let brow = drain_until_row(&mut sb, "tab-b-text");
    let mut drag = SelectionDrag::press(b, (0, brow), &sb.terminal);
    assert!(drag.drive(b, (9, brow), &mut sb.terminal));
    assert!(!release_drag(Some(drag), b, &mut sb.terminal));

    let click_on_a = SelectionDrag::press(a, (0, 0), &s.terminal);
    assert!(
        !release_is_click(Some(click_on_a), b),
        "pressed on A, released on B: not a click, so no link on B opens"
    );
    assert!(!release_drag(Some(click_on_a), b, &mut sb.terminal));
    let mut round_trip = SelectionDrag::press(a, (0, 0), &s.terminal);
    assert!(
        !round_trip.drive(b, (5, 3), &mut sb.terminal),
        "B is not written"
    );
    assert!(
        !release_drag(Some(round_trip), a, &mut s.terminal),
        "moved while B was in front, released back on A: not a click"
    );
    assert_eq!(s.terminal.selection_text().as_deref(), Some("charlie"));
    assert_eq!(sb.terminal.selection_text().as_deref(), Some("tab-b-text"));
}
