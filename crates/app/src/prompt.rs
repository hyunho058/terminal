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
//!
//! ## Two agents, two rules, never pooled
//!
//! Claude Code and the Antigravity CLI draw different dialogs, so each is recognised by its
//! own rule and a screen never mixes them (agent-card-status D5): Claude's `❯ 1.` beside an
//! Antigravity question is two agents' fragments, not one agent's prompt.
//!
//! - **Claude**: two of its three markers — question, footer, selected choice — anywhere in
//!   the tail.
//! - **Antigravity**: its question row, with its first choice (`1. Yes, …`) as the very next
//!   non-blank row. Not two-of-three, and no footer: `agy` draws its `esc to cancel` row under
//!   *every* running turn, dialog or not — measured in the same capture that pinned the other
//!   markers (`Generating...` screens, 24 scans) — so it would have left one ordinary line
//!   (the user's own `> 1. Yes, go ahead` echoed back) between a busy agent and a false
//!   "waiting". Adjacency is what the echo cannot fake: it sits under a separator, never
//!   directly under a question row.
//!
//! Antigravity's rule comes from a real dialog captured through this terminal's own
//! `waiting-scan` line (D12), not from a screenshot or the `agy` binary's strings — a
//! screenshot cannot say which glyph the pointer is (it is ASCII `>`), and the binary holds
//! only fragments.

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
/// prompt that blocks on a human — one agent's dialog, by that agent's rule.
///
/// Verified against a real captured dialog of each agent; see
/// `real_permission_dialog_is_detected` and `real_antigravity_dialog_is_detected`.
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
    claude_dialog(tail) || antigravity_dialog(tail)
}

/// Claude Code's dialog: at least [`REQUIRED_MARKERS`] of its three markers in `tail`.
fn claude_dialog(tail: &[&str]) -> bool {
    let markers: [fn(&str) -> bool; 3] = [is_question, is_footer, is_selected_choice];
    markers
        .iter()
        .filter(|marker| tail.iter().any(|l| marker(l)))
        .count()
        >= REQUIRED_MARKERS
}

/// The Antigravity CLI's dialog (agent-card-status D5, as amended at verification): a
/// question row whose next non-blank row is the first choice — exactly how the captured
/// dialog draws them (`Run this command?` / `> 1. Yes, run command`).
///
/// Blank rows between the two are allowed, so a dialog drawn with a spacer still matches;
/// anything else between them is not, so neither a question the agent writes in its own
/// output nor the user's `> 1. Yes, …` echoed under a separator can pair up with the other.
/// Unmeasured for the URL, tool and edit dialogs, whose rows the binary's strings name but no
/// capture has shown: a different layout there fails toward "not detected", never toward a
/// false "waiting".
fn antigravity_dialog(tail: &[&str]) -> bool {
    tail.iter().enumerate().any(|(i, row)| {
        is_antigravity_question(row)
            && tail[i + 1..]
                .iter()
                .find(|r| !r.trim().is_empty())
                .is_some_and(|r| is_antigravity_choice(r))
    })
}

/// One `waiting-scan` log value: the view offset and the matcher's **whole** input,
/// Debug-escaped.
///
/// Whole, because this line is how a real dialog becomes a detector fixture
/// (agent-card-status D12): it used to keep only the last six rows, and an Antigravity
/// dialog puts its question above its choices and a two-row footer, so the one marker a
/// fixture most needs fell off the top. Debug-escaped, so line breaks, trailing spaces and
/// the pointer glyph survive the log byte for byte.
pub(crate) fn scan_dump(off: usize, tail: &str) -> String {
    format!("off={off} | {tail:?}")
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

/// Antigravity's question line — the line, exactly, trimmed (D5/D6): run a command, open a
/// URL, call a tool, or accept an edit. Exact, so the agent quoting one inside a sentence
/// (`see "Run this command?"`) is not a question.
///
/// The workspace-trust question (`… requires permission to read, edit, and execute files
/// here.`) is **not** here yet: D12 gates it on its own real capture, which has not been
/// taken, so D6's trust case stays a Known Gap rather than shipping on a guess.
fn is_antigravity_question(line: &str) -> bool {
    matches!(
        line.trim(),
        "Run this command?"
            | "Allow access to this URL?"
            | "Allow calling this tool?"
            | "Do you want to proceed?"
    )
}

/// Antigravity's first choice, `1. Yes, …`, with or without the ASCII `>` pointer in
/// front: the pointer sits on whichever option is selected, so after an arrow key the
/// first option is drawn without it. Every `agy` approval option reads `Yes, …` (`Yes, run
/// command`, `Yes, allow access`, `Yes, accept this change`).
fn is_antigravity_choice(line: &str) -> bool {
    let t = line.trim_start();
    let t = t.strip_prefix('>').map_or(t, str::trim_start);
    t.starts_with("1. Yes, ")
}

/// The permission dialog exactly as `term-core` rendered it from a REAL captured
/// Claude Code session (`ls -d ~/.config`, blocked awaiting approval). Transcribed
/// verbatim from the grid dump, blank lines and leading spaces included.
#[cfg(test)]
pub(crate) const REAL_DIALOG: &str = "\
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

/// Antigravity CLI 1.2.12's permission dialog, captured from THIS terminal's own
/// `waiting-scan` line (the matcher's whole input, Debug-escaped, decoded back) while a
/// `run_command` for `ls -la` waited for approval — the same tail on every scan while the
/// dialog stayed up (`specs/agent-card-status/captures/`). Verbatim: the ASCII `>` pointer, the
/// wrapped option 3, the two-row footer with the model name on the last row.
#[cfg(test)]
pub(crate) const REAL_ANTIGRAVITY_DIALOG: &str = "\
────────────────────────────────────────────────────────────
> Could you explian about this project?

● Bash(ls -la) (ctrl+o to expand)

Command
───────────────────────────────────────────────────────────────────────────────────

Requesting permission for:
   ls -la

Run this command?
> 1. Yes, run command
  2. Yes, and always allow in this conversation for commands that start with 'ls'
  3. Yes, and always allow for commands that start with 'ls' (Persist to
settings.json)
  4. No, cancel

  ↑/↓ Navigate · tab Amend · ctrl+g edit/expand command
esc to cancel                                             Gemini 3.8 Flash · medium
";

#[cfg(test)]
mod tests {
    use super::*;

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

    // ---- agent-card-status T0: the capture line carries the matcher's whole input ----

    #[test]
    fn the_scan_dump_keeps_every_row_the_matcher_reads() {
        // D12 — a fixture is only as good as this line. Twenty rows in, twenty rows out:
        // the old dump kept the last six, which dropped an Antigravity dialog's question.
        let tail: String = (1..=TAIL_ROWS).map(|i| format!("row{i:02}\n")).collect();
        let dump = scan_dump(0, &tail);
        for i in 1..=TAIL_ROWS {
            assert!(dump.contains(&format!("row{i:02}")), "row {i} lost: {dump}");
        }
    }

    #[test]
    fn the_scan_dump_is_byte_exact() {
        // The bytes a matcher compares — breaks, trailing spaces, the pointer glyph — are
        // written so they can be read back, not condensed into a display form.
        assert_eq!(
            scan_dump(2, "Run this command?\n  ❯ 1. Yes \n"),
            "off=2 | \"Run this command?\\n  ❯ 1. Yes \\n\""
        );
    }

    // ---- agent-card-status D5/D6/D12: Antigravity's dialog ----

    /// A busy `agy` turn with no dialog, captured by the same `waiting-scan` line as
    /// [`REAL_ANTIGRAVITY_DIALOG`] (`captures/antigravity_generating.txt`): the spinner, the
    /// empty input row, and the `esc to cancel` row that every running turn draws. The login
    /// banner's account email is redacted (`user@example.com`); nothing else is edited.
    const REAL_ANTIGRAVITY_GENERATING: &str = "
      ▄▀▀▄        Antigravity CLI 1.2.12
     ▀▀▀▀▀▀       user@example.com (Google AI Pro)
    ▀▀▀▀▀▀▀▀      Gemini 3.8 Flash (Medium)
   ▄▀▀    ▀▀▄     ~/IdeaProjects/terminal
  ▄▀▀      ▀▀▄

────────────────────────────────────────────────────────────
> Could you explian about this project?
⣾  Generating...
───────────────────────────────────────────────────────────────────────────────────
>
───────────────────────────────────────────────────────────────────────────────────
esc to cancel                                             Gemini 3.8 Flash · medium
";

    fn rows(s: &str) -> Vec<&str> {
        s.lines().collect()
    }

    #[test]
    fn real_antigravity_dialog_is_detected() {
        // R6.1 — the screen that read `detected=false` at capture time, i.e. the bug; and it
        // is Antigravity's rule that finds it, not Claude's.
        assert!(shows_blocking_prompt(REAL_ANTIGRAVITY_DIALOG));
        assert!(antigravity_dialog(&rows(REAL_ANTIGRAVITY_DIALOG)));
        assert!(!claude_dialog(&rows(REAL_ANTIGRAVITY_DIALOG)));
    }

    #[test]
    fn a_busy_antigravity_turn_is_not_a_dialog() {
        // R6.2 — the captured Generating screen, which carries `esc to cancel`, alone and
        // with each single line a busy screen can plausibly add: the user's own reply echoed
        // back, the agent asking a question in its output, and both at once.
        assert!(!shows_blocking_prompt(REAL_ANTIGRAVITY_GENERATING));
        let with = |extra: &str| {
            REAL_ANTIGRAVITY_GENERATING.replace("> Could you explian about this project?", extra)
        };
        for (what, screen) in [
            ("the user's echo", with("> 1. Yes, go ahead")),
            ("a question in the output", with("Do you want to proceed?")),
            (
                "both, a separator apart",
                with("Do you want to proceed?\n────────\n> 1. Yes, go ahead"),
            ),
        ] {
            assert_ne!(
                screen, REAL_ANTIGRAVITY_GENERATING,
                "{what}: the edit landed"
            );
            assert!(!shows_blocking_prompt(&screen), "{what}");
        }
    }

    #[test]
    fn the_question_or_the_choice_alone_is_not_a_dialog() {
        // R6.2 — the real dialog with its choice rows, or its question row, taken out.
        // Rows removed by their literal text, not by the matchers under test.
        let without = |drop: &str| -> String {
            REAL_ANTIGRAVITY_DIALOG
                .lines()
                .filter(|l| *l != drop)
                .map(|l| format!("{l}\n"))
                .collect()
        };
        let no_choice = without("> 1. Yes, run command");
        let no_question = without("Run this command?");
        assert_ne!(no_choice, REAL_ANTIGRAVITY_DIALOG);
        assert_ne!(no_question, REAL_ANTIGRAVITY_DIALOG);
        assert!(!shows_blocking_prompt(&no_choice), "question alone");
        assert!(!shows_blocking_prompt(&no_question), "choice alone");
    }

    #[test]
    fn the_choice_must_be_the_next_row_under_the_question() {
        // D5 as amended — blank rows may sit between them; nothing else may.
        assert!(antigravity_dialog(&[
            "Run this command?",
            "> 1. Yes, run command"
        ]));
        assert!(antigravity_dialog(&[
            "Run this command?",
            "",
            "  ",
            "> 1. Yes, run command"
        ]));
        assert!(!antigravity_dialog(&[
            "Run this command?",
            "ok",
            "> 1. Yes, run command"
        ]));
        assert!(!antigravity_dialog(&[
            "> 1. Yes, run command",
            "Run this command?"
        ]));
        assert!(
            !antigravity_dialog(&["Run this command?"]),
            "nothing under it"
        );
    }

    #[test]
    fn rules_do_not_pool_across_agents() {
        // R6.3 — one agent's question over the other agent's chooser is neither dialog.
        assert!(!shows_blocking_prompt("Run this command?\n ❯ 1. Yes\n"));
        assert!(!shows_blocking_prompt(
            " Do you want to make this edit?\n> 1. Yes, run command\n"
        ));
        assert!(!shows_blocking_prompt(
            "> 1. Yes, run command\n Esc to cancel · Tab to amend\n"
        ));
    }

    #[test]
    fn every_antigravity_question_counts_as_an_exact_line() {
        // R6.4 — each question over a first choice is a dialog; the question quoted or
        // decorated is not a question, so the pair does not form.
        for q in [
            "Run this command?",
            "Allow access to this URL?",
            "Allow calling this tool?",
            "Do you want to proceed?",
        ] {
            assert!(
                shows_blocking_prompt(&format!("{q}\n> 1. Yes, allow access\n")),
                "{q}"
            );
        }
        for q in [
            "Run this command? (y/n)",
            "see \"Run this command?\" in the docs",
        ] {
            assert!(
                !shows_blocking_prompt(&format!("{q}\n> 1. Yes, allow access\n")),
                "{q}"
            );
        }
    }

    #[test]
    fn the_trust_question_waits_for_its_own_capture() {
        // R6.5 / D12 — not captured yet, so not in the vocabulary: the line is no marker.
        // When a real capture lands, this test is replaced by that fixture's.
        assert!(!is_antigravity_question(
            "Antigravity requires permission to read, edit, and execute files here."
        ));
    }

    #[test]
    fn the_first_choice_matches_with_or_without_the_pointer() {
        // The pointer follows the selection; after an arrow key option 1 has none.
        assert!(is_antigravity_choice("> 1. Yes, run command"));
        assert!(is_antigravity_choice("  1. Yes, run command"));
        assert!(is_antigravity_choice(">1. Yes, allow access"));
        assert!(!is_antigravity_choice("> 2. Yes, and always allow"));
        assert!(!is_antigravity_choice("  4. No, cancel"));
        assert!(
            !is_antigravity_choice("1. Yes"),
            "no option reads a bare Yes"
        );
        assert!(!is_antigravity_choice("> Could you explain 1. Yes, this?"));
    }

    #[test]
    fn the_antigravity_dialog_is_detected_through_a_real_terminal_viewport() {
        // R6.1 through the path the app reads: a real `Terminal`, `viewport_tail_text`.
        for (cols, rows) in [(80, 30), (100, 40), (120, 50)] {
            let screen = render_in_terminal(cols, rows, REAL_ANTIGRAVITY_DIALOG);
            assert!(
                shows_blocking_prompt(&screen),
                "undetected in a real {cols}x{rows} viewport; tail was:\n{screen}"
            );
        }
    }

    #[test]
    fn an_antigravity_dialog_above_the_tail_is_not_detected() {
        // R6.7 — the same window rule for both agents.
        let mut s = String::from(REAL_ANTIGRAVITY_DIALOG);
        for i in 0..TAIL_ROWS + 5 {
            s.push_str(&format!("later output line {i}\n"));
        }
        assert!(!shows_blocking_prompt(&s));
    }

    #[test]
    fn answering_the_antigravity_dialog_clears_the_signal() {
        // R0.2's screen half: approved, the dialog is replaced by the command's output.
        let mut t = term_core::Terminal::new(
            100,
            40,
            term_core::Color(0xE0E0E0),
            term_core::Color(0x101010),
        );
        t.advance(REAL_ANTIGRAVITY_DIALOG.replace('\n', "\r\n").as_bytes());
        assert!(shows_blocking_prompt(&t.viewport_tail_text(TAIL_ROWS)));
        t.advance(b"\x1b[2J\x1b[H");
        t.advance(b"\xe2\x97\x8f Bash(ls -la)\r\n  total 48\r\n");
        assert!(!shows_blocking_prompt(&t.viewport_tail_text(TAIL_ROWS)));
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
