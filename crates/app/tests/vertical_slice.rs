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

use std::time::{Duration, Instant};

use app::input::encode_key;
use app::session::{Session, TabId, Wake};
use common::config::Config;
use renderer::{Renderer, RendererError};
use term_core::Color;
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// How long the shell gets to start, run one command, and echo the result.
/// Generous: this bounds a *failure*, and a slow machine must not turn a working
/// terminal into a red build.
const SHELL_TIMEOUT: Duration = Duration::from_secs(15);

const COLS: u16 = 80;
const ROWS: u16 = 24;

/// A pixel counts as ink when any channel differs from the background by more
/// than this. Same margin, and same reasoning, as `renderer/tests/golden_frame.rs`:
/// well above antialiasing noise, well below the contrast of a legible glyph.
const INK_THRESHOLD: i32 = 24;

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
    if std::env::var_os("SKIP_SHELL_TESTS").is_some() {
        eprintln!("SKIP_SHELL_TESTS set; skipping the vertical slice");
        return None;
    }
    let (fg, bg) = fg_bg();
    let wake: Wake = Box::new(|_| {});
    match Session::spawn(TabId(0), COLS, ROWS, fg, bg, wake) {
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
/// Mirrors `renderer/tests/golden_frame.rs::headless`, and for the reason stated
/// there: a helper that turns "the renderer would not start" into a **passing**
/// test reproduces in the harness the exact failure these tests exist to detect.
/// This function had precisely that shape — every `RendererError` was a bare
/// `return` with a line to stderr — and no environment variable gated it, so the
/// skip was nobody's decision.
///
/// The two failures are split because they are not the same kind of fact.
/// `Renderer::new_headless` calls `FontSet::load()` *before* it requests an
/// adapter, so `NoFont` arrives on every machine, GPU or not: it means the
/// Menlo / SF Mono / AppleSDGothicNeo chain stopped resolving, which is a
/// regression in the code under test. Break it and the renderer crate's golden
/// frames go red while this test — the only one spanning keystroke → shell → grid
/// → GPU — reported ok. That case therefore fails unconditionally, and
/// `SKIP_GPU_TESTS` does not cover it, because a machine without a GPU still has
/// fonts.
fn headless_renderer(cfg: &Config) -> Option<Renderer> {
    match Renderer::new_headless(FRAME_W, FRAME_H, 1.0, cfg) {
        Ok(r) => Some(r),
        Err(RendererError::NoFont(tried)) => panic!(
            "no usable monospace font (tried {tried}). This is the font pipeline \
             failing, not a missing GPU — it is decided before any adapter is \
             requested — so SKIP_GPU_TESTS deliberately does not opt out of it."
        ),
        Err(e) => {
            if std::env::var_os("SKIP_GPU_TESTS").is_some() {
                eprintln!("SKIP_GPU_TESTS set; skipping the pixel leg: {e}");
                return None;
            }
            panic!(
                "headless renderer unavailable ({e}). This is the only test that \
                 spans keystroke → shell → grid → GPU, so it fails rather than \
                 skipping. Set SKIP_GPU_TESTS=1 to opt out deliberately."
            );
        }
    }
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

    let (cw, ch) = r.cell_size();
    let (ox, oy) = (r.left_inset(), r.top_inset());
    let (cols, rows) = r.grid_size();
    let (cols, rows) = (cols.min(snapshot.cols), rows.min(snapshot.rows));

    // floor on both edges, so adjacent cells tile without overlapping and a
    // neighbour's pixel is never counted twice. `renderer/tests/golden_frame.rs`
    // derives its cell rect the same way, and the two have to stay that way — a
    // disagreement would make "this cell holds ink" mean something different on
    // each side of the same contract.
    let cell_has_ink = |col: u16, row: u16| {
        let x0 = (ox + col as f32 * cw).max(0.0) as u32;
        let y0 = (oy + row as f32 * ch).max(0.0) as u32;
        let x1 = ((ox + (col + 1) as f32 * cw).max(0.0) as u32).min(frame.width());
        let y1 = ((oy + (row + 1) as f32 * ch).max(0.0) as u32).min(frame.height());
        (y0..y1).any(|y| {
            (x0..x1).any(|x| {
                let px = frame.pixel(x, y);
                (0..3).any(|i| (px[i] as i32 - bg[i] as i32).abs() > INK_THRESHOLD)
            })
        })
    };
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
        .filter(|&col| {
            let c = char_at(col, marker_row);
            !c.is_whitespace() && c != '\0' && !cell_has_ink(col, marker_row)
        })
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
