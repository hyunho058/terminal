//! Detecting a **blocking prompt** on a tab's own screen.
//!
//! Some things an agent blocks on leave no trace anywhere except the screen. The clearest
//! case is Claude Code's tool-permission dialog ("Do you want to proceed?"): while it is
//! open, Claude Code writes **nothing** to its session JSONL — measured on a live blocked
//! session, 3+ minutes with zero bytes appended, its log still ending at the human's own
//! prompt. So `ai_monitor`'s log-derived activity read (`tabmeta::activity`) reports `Busy`
//! forever and the sidebar never surfaces the tab.
//!
//! The one place the prompt is unambiguously present is the tab's grid — which this
//! terminal already owns. That makes this the only signal that needs no cooperation from
//! the agent and no configuration from the user. (The alternative, Claude Code's
//! terminal-bell notification, was measured working but requires the user to set
//! `preferredNotifChannel: terminal_bell`; depending on the agent's config to make the
//! terminal's own sidebar work is the wrong trade.)
//!
//! **This is app policy, deliberately.** `term-core` supplies the grid and nothing more —
//! it never learns what an agent prompt looks like (spec D21, `dependency-direction`).
//!
//! ## Fragility, stated plainly
//!
//! This matches another program's UI text, so a wording change upstream silently disables
//! it. Two things bound the damage: the match is a **level**, not an event — a false
//! positive clears itself as soon as the text scrolls away, and a missed prompt costs a
//! band entry, never a wrong action — and it is gated on the tab actually having a detected
//! agent. Patterns are checked against a real captured session (see the tests).

/// Rows from the bottom of the viewport that may carry a live prompt. The dialog renders
/// at the bottom, so a match higher up is almost certainly the agent *quoting* a prompt in
/// its own output rather than showing one. Sized to the tallest real dialog observed
/// (question + up to ~6 options + footer) plus slack.
pub(crate) const TAIL_ROWS: usize = 20;

/// How many distinct markers must co-occur. One alone is too weak: an agent that *writes
/// about* permission prompts (a code review, a spec, this very file) would otherwise mark
/// its own tab as waiting. Requiring two structurally different lines — the question and
/// the footer, or a numbered choice — makes incidental prose very unlikely to match.
const REQUIRED_MARKERS: usize = 2;

/// Whether the tail of `screen` (a viewport rendered by `GridSnapshot::to_text`) shows a
/// prompt that blocks on a human.
///
/// Verified against a real captured Claude Code permission dialog; see
/// `real_permission_dialog_is_detected`.
pub(crate) fn shows_blocking_prompt(screen: &str) -> bool {
    let mut lines: Vec<&str> = screen.lines().collect();
    // A viewport is padded with blank rows to the full grid height, and a dialog rarely
    // ends on the last row — the real capture had ~16 blank rows below it. Measuring the
    // tail from the last row would push the question and the choices out of the window and
    // leave only the footer, so measure from the last row that has content.
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let tail = &lines[lines.len().saturating_sub(TAIL_ROWS)..];
    let mut markers = 0;
    if tail.iter().any(|l| is_question(l)) {
        markers += 1;
    }
    if tail.iter().any(|l| is_footer(l)) {
        markers += 1;
    }
    if tail.iter().any(|l| is_selected_choice(l)) {
        markers += 1;
    }
    markers >= REQUIRED_MARKERS
}

/// The dialog's question line, e.g. `Do you want to proceed?`.
fn is_question(line: &str) -> bool {
    let t = line.trim();
    t == "Do you want to proceed?" || t == "Do you want to make this edit?"
}

/// The dialog's key-hint footer, e.g. `Esc to cancel · Tab to amend · ctrl+e to explain`.
/// Both fragments are required — "Esc to cancel" alone is common enough elsewhere.
fn is_footer(line: &str) -> bool {
    line.contains("Esc to cancel") && line.contains("to amend")
}

/// The *selected* numbered choice, e.g. `❯ 1. Yes`. The pointer matters: it means an
/// interactive chooser is focused right now, not that a list of numbers is on screen.
fn is_selected_choice(line: &str) -> bool {
    let t = line.trim_start();
    let Some(rest) = t.strip_prefix('❯') else {
        return false;
    };
    let rest = rest.trim_start();
    // `1.` through `9.` — a chooser's first option is the one the pointer starts on.
    let mut chars = rest.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_digit() && c != '0')
        && matches!(chars.next(), Some('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The permission dialog exactly as `term-core` rendered it from a REAL captured
    /// Claude Code session (`ls -d ~/.config`, blocked awaiting approval). Transcribed
    /// verbatim from the grid dump, blank lines and leading spaces included.
    const REAL_DIALOG: &str = "\
⏺ Listing 1 directory…
  ⎿  $ ls -d ~/.config

────────────────────────────────────────────────────────────────────────────────
 Bash command

   ls -d ~/.config
   Check if ~/.config exists

 Do you want to proceed?
 ❯ 1. Yes
   2. Yes, allow reading from .config/ from this project
   3. No

 Esc to cancel · Tab to amend · ctrl+e to explain
";

    #[test]
    fn real_permission_dialog_is_detected() {
        assert!(shows_blocking_prompt(REAL_DIALOG));
    }

    #[test]
    fn ordinary_output_is_not_a_prompt() {
        for screen in [
            "",
            "$ cargo build\n   Compiling app v0.1.0\n    Finished in 2.01s\n",
            "⏺ I'll run that command.\n⏺ Listing 1 directory…\n  ⎿  $ ls -d ~/.config\n",
            "error: could not compile `term-core`\n",
        ] {
            assert!(
                !shows_blocking_prompt(screen),
                "false positive on {screen:?}"
            );
        }
    }

    #[test]
    fn a_single_marker_is_not_enough() {
        // An agent that merely WRITES the question — a spec, a code review, this module's
        // own doc comment — must not mark its tab as waiting.
        assert!(!shows_blocking_prompt(
            "I need to explain what happens when Claude asks\n\
             Do you want to proceed?\n\
             and how the sidebar reacts to it.\n"
        ));
        // The footer alone, likewise.
        assert!(!shows_blocking_prompt(
            "the hint line reads Esc to cancel · Tab to amend · ctrl+e to explain\n"
        ));
    }

    #[test]
    fn a_numbered_list_without_the_pointer_is_not_a_chooser() {
        // Plain enumerated prose shares the digits but not the focused pointer.
        assert!(!shows_blocking_prompt(
            "Steps:\n 1. Yes\n 2. No\n 3. Maybe\nEsc to cancel · Tab to amend\n"
        ));
        assert!(is_selected_choice("❯ 1. Yes"));
        assert!(is_selected_choice("  ❯ 3. No"));
        assert!(!is_selected_choice("  1. Yes"));
        assert!(!is_selected_choice(
            "❯ Run this bash command and show me the output"
        ));
        assert!(!is_selected_choice("❯ 0. Zero"));
    }

    #[test]
    fn a_prompt_scrolled_far_up_is_ignored() {
        // Only the tail can hold a LIVE dialog; the same text buried under a screenful of
        // later output is history the agent already moved past.
        let mut s = String::from(REAL_DIALOG);
        for i in 0..TAIL_ROWS + 5 {
            s.push_str(&format!("later output line {i}\n"));
        }
        assert!(!shows_blocking_prompt(&s));
    }

    #[test]
    fn detection_survives_trailing_blank_rows() {
        // REGRESSION (found on real data, not in review): a 40-row viewport holding a
        // dialog that ends at row 23 is padded with ~16 blank rows. Measuring the tail
        // from the last ROW left only the footer inside the window — one marker, below the
        // threshold — so a real prompt went undetected while a 4-blank-line unit test
        // passed. Use a padding depth that actually exceeds TAIL_ROWS.
        for pad in [1, 4, TAIL_ROWS, TAIL_ROWS * 2] {
            let mut s = String::from(REAL_DIALOG);
            for _ in 0..pad {
                s.push('\n');
            }
            assert!(
                shows_blocking_prompt(&s),
                "dialog must survive {pad} blank padding rows"
            );
        }
    }

    /// Drive the dialog through a REAL `Terminal` at a realistic size and read it back the
    /// way the app does — `viewport_tail_text(TAIL_ROWS)`. This is the combination that
    /// actually failed once: the unit tests above passed on a bare string while a real
    /// 40-row viewport (dialog ending at row 23, ~16 blank rows below) fed only the footer
    /// into the window. Any future change to the tail accessor, the padding trim, or
    /// `TAIL_ROWS` must keep this green.
    fn render_in_terminal(cols: u16, rows: u16, body: &str) -> String {
        let mut t = term_core::Terminal::new(
            cols,
            rows,
            term_core::Color(0xE0E0E0),
            term_core::Color(0x101010),
        );
        // CRLF: a bare \n only line-feeds, leaving the cursor's column — real PTY output
        // carries the carriage return.
        t.advance(body.replace('\n', "\r\n").as_bytes());
        t.viewport_tail_text(TAIL_ROWS)
    }

    #[test]
    fn detected_through_a_real_terminal_viewport() {
        for (cols, rows) in [(80, 24), (100, 40), (65, 31), (120, 50)] {
            let screen = render_in_terminal(cols, rows, REAL_DIALOG);
            assert!(
                shows_blocking_prompt(&screen),
                "undetected in a real {cols}x{rows} viewport; tail was:\n{screen}"
            );
        }
    }

    #[test]
    fn real_terminal_without_a_dialog_is_not_detected() {
        let screen = render_in_terminal(
            100,
            40,
            "$ cargo build\n   Compiling app v0.1.0\n    Finished in 2.01s\n",
        );
        assert!(!shows_blocking_prompt(&screen));
    }

    #[test]
    fn answering_the_dialog_clears_the_signal_in_a_real_terminal() {
        // The falling edge the whole level-based design rests on: once the dialog is gone
        // from the screen, the tab must stop reading as waiting with no extra bookkeeping.
        let mut t = term_core::Terminal::new(
            100,
            40,
            term_core::Color(0xE0E0E0),
            term_core::Color(0x101010),
        );
        t.advance(REAL_DIALOG.replace('\n', "\r\n").as_bytes());
        assert!(shows_blocking_prompt(&t.viewport_tail_text(TAIL_ROWS)));
        // The agent proceeds: clear screen + fresh output, as answering really does.
        t.advance(b"\x1b[2J\x1b[H");
        t.advance(b"\xe2\x8f\xba Ran 1 shell command\r\n  /Users/me/.config\r\n");
        assert!(
            !shows_blocking_prompt(&t.viewport_tail_text(TAIL_ROWS)),
            "answered dialog must fall back to not-waiting"
        );
    }

    #[test]
    fn blank_padding_does_not_resurrect_a_scrolled_away_prompt() {
        // The blank-row trim must not become a way to reach arbitrarily far back: real
        // output below the dialog still pushes it out of the window, padding or not.
        let mut s = String::from(REAL_DIALOG);
        for i in 0..TAIL_ROWS + 5 {
            s.push_str(&format!("later output line {i}\n"));
        }
        for _ in 0..30 {
            s.push('\n');
        }
        assert!(!shows_blocking_prompt(&s));
    }
}
