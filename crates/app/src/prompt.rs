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
//! ## One dialog, one rule, never pooled
//!
//! Claude Code and the Antigravity CLI draw different dialogs, and Claude Code draws several,
//! so each dialog is recognised by its own rule and a screen never mixes them
//! (agent-card-status D5, claude-ask-dialog-waiting D2): Claude's `❯ 1.` beside an
//! Antigravity question is two agents' fragments, not one agent's prompt, and a permission
//! question over a question dialog's footer is two dialogs' fragments.
//!
//! - **Claude's permission dialog**: two of its three markers — question, footer, selected
//!   choice — anywhere in the tail.
//! - **Claude's question dialog** (`AskUserQuestion`): its `Enter to select … Esc to cancel`
//!   footer, with its `N. Chat about this` row as the nearest non-blank row above it. The
//!   session log cannot stand in for any of Claude's dialogs: Claude Code 2.1.28x writes a
//!   `tool_use` only once the tool returns, so a pending question is not on disk.
//! - **Claude's review and plan screens**: a fixed question (`Ready to submit your answers?`,
//!   and `ExitPlanMode`'s `… Would you like to proceed?`, wrapped onto two rows on a narrow
//!   grid) with its first choice as the next non-blank row.
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
//! only fragments. Claude's question, review and plan rules come from real screens taken
//! the same way, through `term_core`'s own `Pty` and `Terminal`
//! (`specs/claude-ask-dialog-waiting/captures/`).

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
/// Verified against real captured dialogs; see `real_permission_dialog_is_detected`,
/// `real_question_dialogs_are_detected`, `real_plan_dialogs_are_detected` and
/// `real_antigravity_dialog_is_detected`.
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
    claude_dialog(tail)
        || claude_ask_dialog(tail)
        || claude_review_dialog(tail)
        || claude_plan_dialog(tail)
        || antigravity_dialog(tail)
}

/// Claude Code's question dialog (`AskUserQuestion`, claude-ask-dialog-waiting D3): a
/// footer whose nearest non-blank row above is the `N. Chat about this` row.
///
/// Claude Code draws the two as one block (a separator, the chat row, a one-row margin, the
/// footer), and they are the last rows of the dialog however tall its question and options
/// are, so the pair stays in the tail when the `❯ 1.` pointer does not. Either line can be
/// quoted alone; the pair, in that order with only blank rows between, is what prose does
/// not produce. Not anchored to the last row, so a tmux status bar or a statusline under the
/// dialog does not hide it. Unmatched when the block is not drawn at all (D3's `Nt` layout):
/// that fails toward "not detected", never toward a false "waiting".
fn claude_ask_dialog(tail: &[&str]) -> bool {
    (0..tail.len()).any(|i| {
        is_ask_footer_at(tail, i)
            && tail[..i]
                .iter()
                .rev()
                .find(|r| !r.trim().is_empty())
                .is_some_and(|r| is_chat_row(r))
    })
}

/// The question dialog's key-hint footer, starting at `tail[i]`: its trimmed text starts
/// with `Enter to select` and ends with `Esc to cancel`, on one row or wrapped onto exactly
/// one more. The middle is not enumerated, so `↑/↓ to navigate` (one question),
/// `Tab/Arrow keys to navigate` (several) and the `ctrl+g to edit in <editor>` hint all
/// match. Starts-with and ends-with rather than `contains`, so the footer quoted inside a
/// sentence is not a footer, and neither are Claude Code's other pickers, whose footers end
/// the same way but start differently (`/model`'s `Enter to set as default`, the trust
/// prompt's `Enter to confirm`).
///
/// The wrapped form is read by joining the two rows with one space, because the wrap can
/// fall inside the last phrase: at 57 columns the captured footer is `… · Esc to` over
/// `cancel`, so the second row on its own ends with nothing recognisable.
fn is_ask_footer_at(tail: &[&str], i: usize) -> bool {
    let first = tail[i].trim();
    if !first.starts_with("Enter to select") {
        return false;
    }
    first.ends_with("Esc to cancel")
        || tail
            .get(i + 1)
            .is_some_and(|next| format!("{first} {}", next.trim()).ends_with("Esc to cancel"))
}

/// The question dialog's last option, e.g. `  5. Chat about this`: one digit 1–9, then
/// exactly `. Chat about this`, with or without the `❯` pointer in front.
fn is_chat_row(line: &str) -> bool {
    let mut chars = without_pointer(line).chars();
    matches!(chars.next(), Some('1'..='9')) && chars.as_str() == ". Chat about this"
}

/// The question dialog's review screen (claude-ask-dialog-waiting R2.11), drawn before the
/// answers are sent for several questions and for a multi-select one: `Ready to submit your
/// answers?` with `1. Submit answers` as the next non-blank row. It draws no footer, so the
/// question dialog's rule cannot see it. The choice is matched exactly, as captured: both
/// rows are short and generic, and the question over any numbered list item would let an
/// agent's own prose (`Ready to submit your answers?` / `1. Keep the label short`) read as
/// a dialog.
fn claude_review_dialog(tail: &[&str]) -> bool {
    question_over_choice(tail, "Ready to submit your answers?", |row| {
        without_pointer(row) == "1. Submit answers"
    })
}

/// The plan-approval dialog (`ExitPlanMode`, claude-ask-dialog-waiting D5 as amended by its
/// captures): its question, then its first choice as the next non-blank row. It has no `Esc
/// to cancel` footer, so D3's shape cannot apply; the box title `Ready to code?` sits above
/// the plan body and is usually out of the tail.
fn claude_plan_dialog(tail: &[&str]) -> bool {
    question_over_choice(
        tail,
        "Claude has written up a plan and is ready to execute. Would you like to proceed?",
        is_first_choice,
    )
}

/// `question` on one row of `tail`, or wrapped onto exactly two adjacent rows, with a row
/// that `is_choice` accepts as the next non-blank row after it. Blank rows between are
/// skipped; any other row breaks the pair.
///
/// The two-row form must be the whole question once the rows are joined with one space,
/// because the wrap point moves with the width: the captured plan question breaks after
/// `Would you` at 70 columns, after `ready to` at 57, after `ready` at 47, and takes a third
/// row at 46. Exact equality means only a real word-wrap passes — a row cut mid-word, a
/// stray `?` or `d?` under the first row, or the continuation without its first row does
/// not. Three rows do not match: below 47 columns the plan dialog is a Known Gap. Checked by
/// stripping rather than by building the joined string, because this runs on every row of
/// every scan (G1).
fn question_over_choice(tail: &[&str], question: &str, is_choice: fn(&str) -> bool) -> bool {
    (0..tail.len()).any(|i| {
        let first = tail[i].trim();
        let rows = if first == question {
            1
        } else if tail.get(i + 1).is_some_and(|next| {
            question
                .strip_prefix(first)
                .and_then(|rest| rest.strip_prefix(' '))
                == Some(next.trim())
        }) {
            2
        } else {
            return false;
        };
        tail[i + rows..]
            .iter()
            .find(|r| !r.trim().is_empty())
            .is_some_and(|r| is_choice(r))
    })
}

/// A chooser's first option, `1. …`, with or without the `❯` pointer in front (D5).
fn is_first_choice(line: &str) -> bool {
    without_pointer(line).starts_with("1. ")
}

/// `line` trimmed, minus Claude's `❯` pointer and the whitespace after it when the row
/// starts with one. The pointer sits on whichever option is selected, so a chooser row is
/// drawn with or without it; D3 and D5 both read it as `❯` *followed by whitespace*, so
/// `❯1.` keeps its glyph and is no option.
fn without_pointer(line: &str) -> &str {
    let t = line.trim();
    match t.strip_prefix('❯') {
        Some(rest) if rest.starts_with(char::is_whitespace) => rest.trim_start(),
        _ => t,
    }
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

/// Claude Code 2.1.289's `AskUserQuestion` dialog with three questions — the tab strip, the
/// first question focused, the `Tab/Arrow keys to navigate` footer. Captured at 88×36 by
/// `specs/claude-ask-dialog-waiting`'s harness, which spawns zsh through `term_core::Pty`
/// (the tab environment), feeds its output through `term_core::Terminal` and logs the
/// matcher's input, `viewport_tail_text(TAIL_ROWS)`, in [`scan_dump`]'s format; decoded back
/// byte for byte (`captures/c2_ask_multi.txt`). The old rules saw one marker here (`❯ 1.`),
/// so the tab read Working: the bug that spec fixes.
#[cfg(test)]
pub(crate) const REAL_ASK_MULTI: &str = "
❯ Use the AskUserQuestion tool ONCE with THREE questions in one call: fruit
  (Apple/Banana/Cherry), colour (Red/Green/Blue), size (Small/Medium/Large). Do nothing
  else.
────────────────────────────────────────────────────────────────────────────────────────
←  ☐ Fruit  ☐ Colour  ☐ Size  ✔ Submit  →

Which fruit should the example use?

❯ 1. Apple
     Use Apple in the example.
  2. Banana
     Use Banana in the example.
  3. Cherry
     Use Cherry in the example.
  4. Type something.
────────────────────────────────────────────────────────────────────────────────────────
  5. Chat about this

Enter to select · Tab/Arrow keys to navigate · Esc to cancel
";

/// The screen right after answering a question dialog: the transcript has moved on and the
/// input row is back (`captures/c10_after_answer.txt`, same harness and session).
#[cfg(test)]
pub(crate) const REAL_ASK_ANSWERED: &str = "















────────────────────────────────────────────────────────────────────────────────────────
❯\u{a0}
────────────────────────────────────────────────────────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

/// Claude Code 2.1.289's plan-approval dialog (`ExitPlanMode`) at 88 columns: the plan body
/// scrolls inside its own box, so the question, the chooser and the `ctrl+g` row are the last
/// rows whatever the plan's length (`captures/c7_plan_88.txt`, same harness and session).
#[cfg(test)]
pub(crate) const REAL_PLAN_WIDE: &str = "
   File to create

   - /private/tmp/ask-capture/README.md (new file; nothing is overwritten)

   Format

   - Line 1: # ask-capture
   - Then one line per section, in the form N. **Title**: one-sentence placeholder.
   - No sub-headings or extra paragraphs, so each section stays on exactly one line.  ↓
  ────────────────────────────────────────────────────────────────────────────────────
   Claude has written up a plan and is ready to execute. Would you like to proceed?

   ❯ 1. Yes, and use auto mode
     2. Yes, manually approve edits
     3. Tell Claude what to change
        shift+tab to approve with this feedback

   ctrl+g to edit in Vim ·
   ~/.claude/plans/plan-adding-a-readme-md-partitioned-dusk.md
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
    fn any_two_of_the_permission_dialogs_three_markers_are_enough() {
        // The real dialog with one marker taken out still has two, whichever one it is: a
        // footer scrolled off or drawn differently must not hide it, and neither question
        // wording may be dropped. Every other test screen carries all three or at most one,
        // so without this the question and footer markers could each be deleted unnoticed
        // (claude-ask-dialog-waiting R8.2: three survivors in this rule).
        let without = |row: &str| -> String {
            assert!(REAL_DIALOG.lines().any(|l| l == row), "no row {row:?}");
            REAL_DIALOG
                .lines()
                .filter(|l| *l != row)
                .map(|l| format!("{l}\n"))
                .collect()
        };
        let question = " Do you want to proceed?";
        let choice = " ❯ 1. Yes";
        let footer = " Esc to cancel · Tab to amend · ctrl+e to explain";
        assert!(shows_blocking_prompt(&without(question)), "footer + choice");
        assert!(shows_blocking_prompt(&without(footer)), "question + choice");
        assert!(shows_blocking_prompt(&without(choice)), "question + footer");
        let edit = without(footer).replace(question, " Do you want to make this edit?");
        assert_ne!(edit, without(footer), "the edit landed");
        assert!(shows_blocking_prompt(&edit), "the edit wording + choice");
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

    // ---- claude-ask-dialog-waiting: Claude's question, review and plan screens ----
    //
    // Every fixture below is a real screen from `specs/claude-ask-dialog-waiting/captures/`,
    // one `claude` process (2.1.289), byte for byte; see that directory's README.

    /// A single question (`↑/↓ to navigate`), pointer on option 1 (`c1_ask_single.txt`).
    const REAL_ASK_SINGLE: &str = "

❯ Use the AskUserQuestion tool to ask me ONE question: \"Which fruit should the example
  use?\" with exactly three options: Apple, Banana, Cherry. Do nothing else.
────────────────────────────────────────────────────────────────────────────────────────
 ☐ Fruit

Which fruit should the example use?

❯ 1. Apple
     Use Apple in the example.
  2. Banana
     Use Banana in the example.
  3. Cherry
     Use Cherry in the example.
  4. Type something.
────────────────────────────────────────────────────────────────────────────────────────
  5. Chat about this

Enter to select · ↑/↓ to navigate · Esc to cancel
";

    /// The review screen after three questions (`c3_ask_review.txt`): no footer at all.
    const REAL_ASK_REVIEW: &str = "\
❯ Use the AskUserQuestion tool ONCE with THREE questions in one call: fruit
  (Apple/Banana/Cherry), colour (Red/Green/Blue), size (Small/Medium/Large). Do nothing
  else.

────────────────────────────────────────────────────────────────────────────────────────
←  ☒ Fruit  ☒ Colour  ☒ Size  ✔ Submit  →

Review your answers

 ● Which fruit should the example use?
   → Apple
 ● Which colour should the example use?
   → Red
 ● Which size should the example use?
   → Small

Ready to submit your answers?

❯ 1. Submit answers
  2. Cancel
";

    /// A multi-select question (`c4_ask_multiselect.txt`).
    const REAL_ASK_MULTISELECT: &str = "\
\x20 toppings do I want? Options: Cheese, Olives, Basil, Mushrooms. Do nothing else.
────────────────────────────────────────────────────────────────────────────────────────
←  ☐ Toppings  ✔ Submit  →

Which toppings do you want?

❯ 1. [ ] Cheese
         Add Cheese.
  2. [ ] Olives
         Add Olives.
  3. [ ] Basil
         Add Basil.
  4. [ ] Mushrooms
         Add Mushrooms.
  5. [ ] Type something
     Submit
────────────────────────────────────────────────────────────────────────────────────────
  6. Chat about this

Enter to select · ↑/↓ to navigate · Esc to cancel
";

    /// Typing a free-text answer at 88 columns (`c5_ask_freetext_typed.txt`): the footer
    /// gains `ctrl+g to edit in Vim` and still fits one row.
    const REAL_ASK_FREETEXT: &str = "

❯ Use the AskUserQuestion tool to ask me ONE question: \"Which fruit should the example
  use?\" with exactly three options: Apple, Banana, Cherry. Do nothing else.
────────────────────────────────────────────────────────────────────────────────────────
 ☐ Fruit

Which fruit should the example use?

  1. Apple
     Use Apple in the example.
  2. Banana
     Use Banana in the example.
  3. Cherry
     Use Cherry in the example.
❯ 4. hello world
────────────────────────────────────────────────────────────────────────────────────────
  5. Chat about this

Enter to select · ↑/↓ to navigate · ctrl+g to edit in Vim · Esc to cancel
";

    /// A tall question whose `❯ 1.` row is above the 20-row tail (`c6_ask_tall.txt`).
    const REAL_ASK_TALL: &str = "\
\x20    rates, latency and business metrics, then widen the rollout step by step if things
     look healthy. This catches problems early with limited blast radius, but it needs
     good monitoring and traffic-splitting tooling.
  3. Rolling
     Replace instances of the old version with the new one gradually, a few at a time,
     until the whole fleet runs the new release. No extra environment is needed, and
     capacity stays mostly intact throughout. It is the default in many orchestrators
     like Kubernetes. The downside is that both versions serve traffic at once during
     rollout, and rollback means rolling back again.
  4. Recreate
     Stop every instance of the old version, then start the new version in its place. It
     is the simplest strategy to set up and reason about, and it avoids running two
     versions side by side, which helps when versions are incompatible. The cost is
     downtime between shutdown and startup, so it suits development environments, batch
     jobs or systems that tolerate brief outages.
  5. Type something.
────────────────────────────────────────────────────────────────────────────────────────
  6. Chat about this

Enter to select · ↑/↓ to navigate · Esc to cancel
";

    /// [`REAL_ASK_MULTI`]'s dialog at 57 columns (`c11_ask_multi_57.txt`). The footer wraps,
    /// and the wrap splits its last phrase: `… · Esc to` / `cancel`. One redaction: the
    /// username in the plan path on row 2 (same length, so the wrap is as captured).
    const REAL_ASK_NARROW: &str = "\
─────────────────────────────────────────────────────────
Planning: /Users/anonymous/.claude/plans/plan-adding-a-re
adme-md-partitioned-dusk.md
─────────────────────────────────────────────────────────
←  ☐ Fruit  ☐ Colour  ☐ Size  ✔ Submit  →

Which fruit should the example use?

❯ 1. Apple
     Use Apple in the example.
  2. Banana
     Use Banana in the example.
  3. Cherry
     Use Cherry in the example.
  4. Type something.
─────────────────────────────────────────────────────────
  5. Chat about this

Enter to select · Tab/Arrow keys to navigate · Esc to
cancel
";

    /// `/model`, opened by the user in a Claude tab (`c9_model_picker.txt`).
    const REAL_MODEL_PICKER: &str = "\
▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   Select model
   Switch between Claude models. Your pick becomes the default for new sessions. For
   other/previous model names, specify with --model.

   ❯ 1.  Default (recommended) ✔  Opus 5.5 · Best for everyday, complex tasks
     2.  Opus 5.5                 For complex work and everyday tasks
     3.  Fable 5.1                For your toughest challenges
     4.  Sonnet 5.5               Most efficient for simpler tasks
     5.  Haiku 4.5                Fastest for quick answers
     6.  Sonnet 5                 Efficient for routine tasks
     7.  Opus 5                   Best for everyday, complex tasks
     8.  Fable 5                  Most capable for your hardest and longest-running
                                  tasks
   ↓ 9.  Opus 4.8                 Best for everyday, complex tasks
      … +3 models

   ◉ xHigh effort ←/→ to adjust

   Enter to set as default · s to use this session only · Esc to cancel
";

    /// `/resume`, opened by the user in a Claude tab (`c9_resume_picker.txt`): its footer
    /// wraps and ends `Esc to cancel`.
    const REAL_RESUME_PICKER: &str = "\
\x20 ⎿ \u{a0}· Which deployment strategy should the example use? → Blue-green

⏺ You picked Blue-green. As you asked, I'm stopping here.

✻ Sautéed for 7s · done 3:55 PM

❯ /model
  ⎿  Kept model as Opus 5.5 (default)
▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   Resume session
   ╭────────────────────────────────────────────────────────────────────────────────╮
   │ ⌕ Search…                                                                      │
   ╰────────────────────────────────────────────────────────────────────────────────╯
     ask-capture

    No conversations found in this project.
    Ctrl+A to show all projects

     Ctrl+A to show all projects · Ctrl+B to only show current branch · Type to
     search · Esc to cancel
";

    /// The workspace-trust prompt `claude` showed on start (`x_workspace_trust_dialog.txt`):
    /// `Enter to confirm · Esc to cancel`, a Claude footer that is not the question dialog's.
    const REAL_TRUST_DIALOG: &str = "
────────────────────────────────────────────────────────────────────────────────────────
 Accessing workspace:

 /private/tmp/ask-capture

 Quick safety check: Is this a project you created or one you trust? (Like your own
 code, a well-known open source project, or work from your team). If not, take a moment
 to review what's in this folder first.

 Claude Code'll be able to read, edit, and execute files here.

 Security guide

 ❯ No, exit
   Yes, I trust this folder

 Enter to confirm · Esc to cancel
";

    /// The screen after declining the plan with Esc (`c7_plan_88_after_esc.txt`).
    const REAL_PLAN_ANSWERED: &str = "\
\x20    │ 25. Changelog: Where release notes are kept.                                    │
     │ 26. Contributing: How to propose changes.                                       │
     │ 27. Code of conduct: Expected behavior for contributors.                        │
     │ 28. Security: How to report vulnerabilities.                                    │
     │ 29. License: The license this project uses.                                     │
     │ 30. Acknowledgements: Credits and thanks.                                       │
     │                                                                                 │
     │ Verification                                                                    │
     │                                                                                 │
     │ - wc -l README.md shows 31 lines (title plus 30 sections).                      │
     │ - grep -cE '^[0-9]+\\. ' README.md returns 30.                                   │
     │ - Open the file and check that the numbers run from 1 to 30 with no gaps.       │
     ╰─────────────────────────────────────────────────────────────────────────────────╯

✻ Cogitated for 12s · done 3:57 PM

────────────────────────────────────────────────────────────────────────────────────────
❯\u{a0}
────────────────────────────────────────────────────────────────────────────────────────
  ⏸ plan mode on (shift+tab to cycle) · ← for agents
";

    /// The plan dialog drawn at 57 columns (`c8_plan_57.txt`): the question wraps after
    /// `ready to`, inside its first sentence.
    const REAL_PLAN_NARROW: &str = "\
\x20  because the user asked for none.

   File to create

   - /private/tmp/ask-capture/README.md (new file;
     nothing is overwritten)

   Format
                                                       ↓
  ─────────────────────────────────────────────────────
   Claude has written up a plan and is ready to
   execute. Would you like to proceed?

   ❯ 1. Yes, and use auto mode
     2. Yes, manually approve edits
     3. Tell Claude what to change
        shift+tab to approve with this feedback

   ctrl+g to edit in Vim · ~/.claude/plans/plan-adding
   -a-readme-md-partitioned-dusk.md
";

    /// The plan dialog drawn at 47 columns (`c8_plan_47.txt`), the narrowest grid whose
    /// question still takes two rows.
    const REAL_PLAN_MIN: &str = "\
\x20  with 25 numbered sections, one line each.
   No exploration was done because the user
   asked for none.

   File to create

   - /private/tmp/ask-capture/README.md (new ↓
  ───────────────────────────────────────────
   Claude has written up a plan and is ready
   to execute. Would you like to proceed?

   ❯ 1. Yes, and use auto mode
     2. Yes, manually approve edits
     3. Tell Claude what to change
        shift+tab to approve with this
        feedback

   ctrl+g to edit in Vim ·
   ~/.claude/plans/plan-adding-a-readme-md-p
   artitioned-dusk.md
";

    /// The same dialog re-rendered at 46 columns (`c8_plan_46_resized.txt`): three rows.
    const REAL_PLAN_TOO_NARROW: &str = "\
\x20  README.md with 25 numbered sections, one
   line each. No exploration was done
   because the user asked for none.

   File to create
                                            ↓
  ──────────────────────────────────────────
   Claude has written up a plan and is
   ready to execute. Would you like to
   proceed?

   ❯ 1. Yes, and use auto mode
     2. Yes, manually approve edits
     3. Tell Claude what to change
        shift+tab to approve with this
        feedback

   ctrl+g to edit in Vim ·
   ~/.claude/plans/plan-adding-a-readme-md-
   partitioned-dusk.md
";

    /// `fixture` with its row `row` (exact text) replaced by `with`. Panics if `row` is not
    /// there, so an edit that misses fails loudly rather than testing the unedited screen.
    fn edit_row(fixture: &str, row: &str, with: &[&str]) -> String {
        assert!(fixture.lines().any(|l| l == row), "no row {row:?} to edit");
        fixture
            .lines()
            .flat_map(|l| if l == row { with.to_vec() } else { vec![l] })
            .map(|l| format!("{l}\n"))
            .collect()
    }

    const ASK_CHAT: &str = "  5. Chat about this";
    const ASK_FOOTER: &str = "Enter to select · ↑/↓ to navigate · Esc to cancel";
    const NARROW_FOOTER: [&str; 2] = [
        "Enter to select · Tab/Arrow keys to navigate · Esc to",
        "cancel",
    ];

    #[test]
    fn real_question_dialogs_are_detected() {
        // R2.1 / R2.3 / R2.11 (C4, C5) — each by the question-dialog rule, none by the
        // permission rule: the old matcher saw one marker at most, which is the bug.
        for (what, screen) in [
            ("single question", REAL_ASK_SINGLE),
            ("several questions", REAL_ASK_MULTI),
            ("multi-select", REAL_ASK_MULTISELECT),
            ("free-text answer being typed", REAL_ASK_FREETEXT),
            ("tall question", REAL_ASK_TALL),
            ("narrow, wrapped footer", REAL_ASK_NARROW),
        ] {
            assert!(shows_blocking_prompt(screen), "{what}");
            assert!(claude_ask_dialog(&rows(screen)), "{what}: by its own rule");
            assert!(
                !claude_dialog(&rows(screen)),
                "{what}: not by the permission rule"
            );
        }
    }

    #[test]
    fn a_tall_question_is_detected_with_its_pointer_outside_the_tail() {
        // R1.3 / R2.3 — the capture is only evidence if the pointer really is off the tail.
        assert!(!REAL_ASK_TALL.lines().any(is_selected_choice));
        assert!(shows_blocking_prompt(REAL_ASK_TALL));
    }

    #[test]
    fn question_dialogs_are_detected_through_a_real_terminal_viewport() {
        // R2.1 / R2.2 through the path the app reads, at the captured width and wider.
        for (what, screen, sizes) in [
            (
                "single",
                REAL_ASK_SINGLE,
                &[(88, 36), (100, 40), (120, 50)][..],
            ),
            ("several", REAL_ASK_MULTI, &[(88, 36), (100, 40)][..]),
            ("narrow", REAL_ASK_NARROW, &[(57, 36)][..]),
        ] {
            for &(cols, rows) in sizes {
                let tail = render_in_terminal(cols, rows, screen);
                assert!(
                    shows_blocking_prompt(&tail),
                    "{what} undetected at {cols}x{rows}; tail was:\n{tail}"
                );
            }
        }
    }

    #[test]
    fn the_wrapped_footer_is_one_footer() {
        // R2.2 — the captured wrap splits `Esc to cancel` itself; the two rows read as one.
        let tail = rows(REAL_ASK_NARROW);
        let i = tail
            .iter()
            .position(|r| *r == NARROW_FOOTER[0])
            .expect("footer row");
        assert_eq!(tail[i + 1], NARROW_FOOTER[1]);
        assert!(is_ask_footer_at(&tail, i));
        assert!(
            !is_ask_footer_at(&tail, i + 1),
            "its second row starts nothing"
        );
    }

    #[test]
    fn only_blank_rows_may_separate_the_chat_row_from_the_footer() {
        // R2.4 — a separator or a text row between them breaks the pair; a blank does not.
        let between = |row: &str| edit_row(REAL_ASK_SINGLE, ASK_CHAT, &[ASK_CHAT, row]);
        assert!(
            !shows_blocking_prompt(&between("────────────────────")),
            "separator"
        );
        assert!(!shows_blocking_prompt(&between("some text")), "text");
        assert!(shows_blocking_prompt(&between("")), "an extra blank row");
        assert!(
            shows_blocking_prompt(&between("   ")),
            "a whitespace-only row is blank"
        );
    }

    #[test]
    fn one_marker_alone_is_not_a_question_dialog() {
        // R2.5 — the footer without the chat row, the chat row without the footer.
        assert!(!shows_blocking_prompt(&edit_row(
            REAL_ASK_SINGLE,
            ASK_CHAT,
            &[]
        )));
        assert!(!shows_blocking_prompt(&edit_row(
            REAL_ASK_SINGLE,
            ASK_FOOTER,
            &[]
        )));
    }

    #[test]
    fn a_quoted_footer_is_not_a_footer() {
        // R2.6 — the footer's words inside a sentence, or with anything after them.
        for quoted in [
            "the hint reads Enter to select · ↑/↓ to navigate · Esc to cancel here",
            "the hint reads Enter to select · ↑/↓ to navigate · Esc to cancel",
            "Enter to select · ↑/↓ to navigate · Esc to cancel, it says",
        ] {
            let screen = edit_row(REAL_ASK_SINGLE, ASK_FOOTER, &[quoted]);
            assert!(!shows_blocking_prompt(&screen), "{quoted}");
        }
    }

    #[test]
    fn claude_pickers_and_the_trust_prompt_are_not_a_question_dialog() {
        // R2.7 — screens the user opens (or `claude` shows on start) whose footers also end
        // `Esc to cancel` but do not start `Enter to select`.
        for (what, screen) in [
            ("/model", REAL_MODEL_PICKER),
            ("/resume", REAL_RESUME_PICKER),
            ("workspace trust", REAL_TRUST_DIALOG),
        ] {
            assert!(!shows_blocking_prompt(screen), "{what}");
        }
    }

    #[test]
    fn the_wrapped_footer_is_exactly_two_adjacent_rows() {
        // R2.8 — a blank between the two rows, or a footer over three rows, is no footer.
        let gap = edit_row(REAL_ASK_NARROW, NARROW_FOOTER[0], &[NARROW_FOOTER[0], ""]);
        assert!(!shows_blocking_prompt(&gap), "blank between the rows");
        let three = edit_row(
            &edit_row(REAL_ASK_NARROW, NARROW_FOOTER[1], &[]),
            NARROW_FOOTER[0],
            &[
                "Enter to select · Tab/Arrow keys to",
                "navigate · Esc to",
                "cancel",
            ],
        );
        assert!(!shows_blocking_prompt(&three), "three rows");
    }

    #[test]
    fn rows_under_the_footer_do_not_break_the_rule() {
        // R2.9 — a tmux status bar or a statusline below the dialog.
        let screen = edit_row(
            REAL_ASK_SINGLE,
            ASK_FOOTER,
            &[
                ASK_FOOTER,
                "[askcap] 0:claude*                         \"host\" 16:00 04-Oct-26",
            ],
        );
        assert!(shows_blocking_prompt(&screen));
    }

    #[test]
    fn the_permission_rule_is_unchanged_and_not_pooled_with_the_question_dialog() {
        // R2.10 — the permission dialog still reads by its own rule, and the permission
        // question over the question dialog's footer is two dialogs' fragments.
        assert!(shows_blocking_prompt(REAL_DIALOG));
        assert!(!claude_ask_dialog(&rows(REAL_DIALOG)));
        assert!(!shows_blocking_prompt(&format!(
            " Do you want to proceed?\n\n{ASK_FOOTER}\n"
        )));
        // Nor does the question dialog's footer count toward the permission rule's two.
        assert!(!claude_dialog(&[ASK_FOOTER, " ❯ 1. Yes"]));
    }

    #[test]
    fn the_review_screen_is_detected_by_its_own_rule() {
        // R2.11 (C3) — no footer, so its question row over its first choice, the bar D3 and
        // D5 meet; the question or the choice alone, or a row between them, is not.
        assert!(shows_blocking_prompt(REAL_ASK_REVIEW));
        assert!(claude_review_dialog(&rows(REAL_ASK_REVIEW)));
        assert!(!claude_ask_dialog(&rows(REAL_ASK_REVIEW)));
        let q = "Ready to submit your answers?";
        let c = "❯ 1. Submit answers";
        assert!(
            !shows_blocking_prompt(&edit_row(REAL_ASK_REVIEW, c, &[])),
            "question alone"
        );
        assert!(
            !shows_blocking_prompt(&edit_row(REAL_ASK_REVIEW, q, &[])),
            "choice alone"
        );
        assert!(
            !shows_blocking_prompt(&edit_row(REAL_ASK_REVIEW, q, &[q, "text"])),
            "a row between"
        );
        assert!(
            shows_blocking_prompt(&edit_row(REAL_ASK_REVIEW, c, &["  1. Submit answers"])),
            "option 1 without the pointer"
        );
        // The choice is the captured one, not any numbered item: an agent's own prose with
        // the question over a list is not the screen.
        assert!(
            !shows_blocking_prompt(&edit_row(
                REAL_ASK_REVIEW,
                c,
                &["  1. Keep the button label short"]
            )),
            "the question over another list item"
        );
        assert!(
            !shows_blocking_prompt(
                "  Ready to submit your answers?\n\n  1. Keep the button label short\n"
            ),
            "assistant prose"
        );
        // And the question is the whole row, not a quote of it.
        assert!(
            !shows_blocking_prompt(&edit_row(
                REAL_ASK_REVIEW,
                q,
                &["the screen asks Ready to submit your answers? at the end"]
            )),
            "the question quoted in a sentence"
        );
        for row in [
            "Ready to submit your answers? (y/n)",
            "Q: Ready to submit your answers?",
        ] {
            let screen = edit_row(REAL_ASK_REVIEW, q, &[row]);
            assert!(!shows_blocking_prompt(&screen), "{row}");
        }
    }

    #[test]
    fn a_question_dialog_scrolled_above_the_tail_is_ignored() {
        // R4.1 — through a real terminal, and the window is still 20 rows.
        assert_eq!(TAIL_ROWS, 20);
        let mut t = term_core::Terminal::new(
            88,
            36,
            term_core::Color(0xE0E0E0),
            term_core::Color(0x101010),
        );
        t.advance(REAL_ASK_SINGLE.replace('\n', "\r\n").as_bytes());
        assert!(shows_blocking_prompt(&t.viewport_tail_text(TAIL_ROWS)));
        for i in 0..TAIL_ROWS + 1 {
            t.advance(format!("later output line {i}\r\n").as_bytes());
        }
        assert!(!shows_blocking_prompt(&t.viewport_tail_text(TAIL_ROWS)));
    }

    #[test]
    fn answering_the_question_dialog_clears_the_signal_in_a_real_terminal() {
        // R0.3's screen half — the dialog, then the screen Claude drew once it was answered.
        let mut t = term_core::Terminal::new(
            88,
            36,
            term_core::Color(0xE0E0E0),
            term_core::Color(0x101010),
        );
        t.advance(REAL_ASK_SINGLE.replace('\n', "\r\n").as_bytes());
        assert!(shows_blocking_prompt(&t.viewport_tail_text(TAIL_ROWS)));
        t.advance(REAL_ASK_ANSWERED.replace('\n', "\r\n").as_bytes());
        assert!(!shows_blocking_prompt(&t.viewport_tail_text(TAIL_ROWS)));
    }

    #[test]
    fn side_by_side_tmux_panes_fail_toward_not_detected() {
        // R5.2 / D11 — every row carries the other pane after `│`.
        let split: String = REAL_ASK_SINGLE
            .lines()
            .map(|l| format!("{l:<88}│ other pane\n"))
            .collect();
        assert!(!shows_blocking_prompt(&split));
    }

    #[test]
    fn the_chat_row_is_exactly_one_numbered_option() {
        assert!(is_chat_row("  5. Chat about this"));
        assert!(is_chat_row("❯ 6. Chat about this"));
        assert!(
            !is_chat_row("❯6. Chat about this"),
            "D3: the pointer is followed by whitespace"
        );
        assert!(is_chat_row("1. Chat about this"));
        assert!(is_chat_row("9. Chat about this"));
        assert!(!is_chat_row("0. Chat about this"));
        assert!(!is_chat_row("10. Chat about this"));
        assert!(!is_chat_row("5. Chat about this one"));
        assert!(!is_chat_row("5.Chat about this"));
        assert!(!is_chat_row("Chat about this"));
        assert!(!is_chat_row("> 5. Chat about this"));
        assert!(!is_chat_row(""));
    }

    #[test]
    fn the_footer_starts_with_enter_to_select_and_ends_with_esc_to_cancel() {
        let at = |rows: &[&str]| is_ask_footer_at(rows, 0);
        assert!(at(&[ASK_FOOTER]));
        assert!(at(&[&format!("  {ASK_FOOTER}  ")]));
        assert!(at(&["Enter to select · Esc to cancel"]));
        assert!(at(&[
            "Enter to select · ↑/↓ to navigate · ctrl+g to edit in",
            "Vim · Esc to cancel"
        ]));
        assert!(at(&NARROW_FOOTER));
        assert!(at(&["Enter to select · ↑/↓ to navigate", "Esc to cancel"]));
        assert!(!at(&["↑/↓ to navigate · Enter to select · Esc to cancel"]));
        assert!(!at(&["Enter to confirm · Esc to cancel"]));
        assert!(!at(&["Enter to select · ↑/↓ to navigate"]));
        assert!(
            !at(&["Enter to select · ↑/↓ to navigate · Esc to"]),
            "half a wrap"
        );
        assert!(!at(&["Enter to select · ↑/↓ to navigate · Esc to", ""]));
        assert!(!at(&[
            "Enter to select · ↑/↓ to navigate · Esc to",
            "cancel now"
        ]));
        assert!(!at(&[
            "Enter to select · ↑/↓ to navigate · Esc to",
            "",
            "cancel"
        ]));
    }

    // ---- the plan-approval dialog (D5, as amended by its captures) ----

    const PLAN_Q: &str =
        "   Claude has written up a plan and is ready to execute. Would you like to proceed?";
    const PLAN_CHOICE: &str = "   ❯ 1. Yes, and use auto mode";
    const PLAN_NARROW_Q: [&str; 2] = [
        "   Claude has written up a plan and is ready to",
        "   execute. Would you like to proceed?",
    ];

    #[test]
    fn real_plan_dialogs_are_detected() {
        // R3.1 / R3.3 — unwrapped at 88 columns, wrapped at 57 and at 47 (two rows each,
        // split at different words), each by the plan rule.
        for (what, screen, cols) in [
            ("88 columns", REAL_PLAN_WIDE, 88),
            ("57 columns", REAL_PLAN_NARROW, 57),
            ("47 columns", REAL_PLAN_MIN, 47),
        ] {
            assert!(shows_blocking_prompt(screen), "{what}");
            assert!(claude_plan_dialog(&rows(screen)), "{what}: by its own rule");
            let tail = render_in_terminal(cols, 36, screen);
            assert!(
                shows_blocking_prompt(&tail),
                "{what} through a real terminal:\n{tail}"
            );
        }
    }

    #[test]
    fn blank_rows_between_the_plan_question_and_its_first_choice_are_skipped() {
        // R3.2 — the capture has one blank row there; a second is skipped the same way.
        assert!(rows(REAL_PLAN_WIDE)
            .windows(3)
            .any(|w| w == [PLAN_Q, "", PLAN_CHOICE]));
        assert!(shows_blocking_prompt(&edit_row(
            REAL_PLAN_WIDE,
            PLAN_Q,
            &[PLAN_Q, ""]
        )));
    }

    #[test]
    fn the_plan_rules_pair_is_strict() {
        // R3.4 — the question with no first choice under it, or a text row between.
        assert!(!shows_blocking_prompt(&edit_row(
            REAL_PLAN_WIDE,
            PLAN_CHOICE,
            &[]
        )));
        assert!(!shows_blocking_prompt(&edit_row(
            REAL_PLAN_WIDE,
            PLAN_Q,
            &[PLAN_Q, "text"]
        )));
        assert!(
            shows_blocking_prompt(&edit_row(
                REAL_PLAN_WIDE,
                PLAN_CHOICE,
                &["     1. Yes, and use auto mode"]
            )),
            "option 1 without the pointer"
        );
        // The question is the whole row, not a quote of it.
        let q = PLAN_Q.trim();
        for row in [
            format!("   see \"{q}\" in the docs"),
            format!("   {q} (y/n)"),
            format!("   Q: {q}"),
        ] {
            let screen = edit_row(REAL_PLAN_WIDE, PLAN_Q, &[&row]);
            assert!(!shows_blocking_prompt(&screen), "{row}");
        }
    }

    #[test]
    fn the_wrapped_plan_question_is_exactly_two_rows_that_join_to_it() {
        // R3.5, as amended — the continuation alone, a continuation that does not complete
        // the question (`?`, `d?`), and a question over three rows (the real 46-column
        // screen, below the 47-column minimum) are not the question.
        let (first, rest) = (PLAN_NARROW_Q[0], PLAN_NARROW_Q[1]);
        assert!(
            !shows_blocking_prompt(&edit_row(REAL_PLAN_NARROW, first, &[])),
            "continuation alone"
        );
        for tail in ["   ?", "   d?"] {
            let screen = edit_row(REAL_PLAN_NARROW, rest, &[tail]);
            assert!(!shows_blocking_prompt(&screen), "{tail:?}");
        }
        let three = edit_row(
            REAL_PLAN_NARROW,
            rest,
            &["   execute. Would you like to", "   proceed?"],
        );
        assert!(!shows_blocking_prompt(&three), "three rows");
        assert!(
            !shows_blocking_prompt(REAL_PLAN_TOO_NARROW),
            "the real 46-column screen"
        );
    }

    #[test]
    fn answering_the_plan_dialog_clears_the_signal() {
        // R3.6 — the dialog, then the screen after declining it with Esc.
        let mut t = term_core::Terminal::new(
            88,
            36,
            term_core::Color(0xE0E0E0),
            term_core::Color(0x101010),
        );
        t.advance(REAL_PLAN_WIDE.replace('\n', "\r\n").as_bytes());
        assert!(shows_blocking_prompt(&t.viewport_tail_text(TAIL_ROWS)));
        t.advance(REAL_PLAN_ANSWERED.replace('\n', "\r\n").as_bytes());
        assert!(!shows_blocking_prompt(&t.viewport_tail_text(TAIL_ROWS)));
    }

    #[test]
    fn the_first_choice_is_option_one_with_or_without_the_pointer() {
        assert!(is_first_choice("   ❯ 1. Yes, and use auto mode"));
        assert!(is_first_choice("     1. Yes, and use auto mode"));
        assert!(
            !is_first_choice("❯1. Submit answers"),
            "D5: the pointer is followed by whitespace"
        );
        assert!(!is_first_choice("     2. Yes, manually approve edits"));
        assert!(!is_first_choice("   ❯ 10. Ten"));
        assert!(!is_first_choice("1.Yes"));
        assert!(!is_first_choice("> 1. Yes, run command"));
        assert!(!is_first_choice(""));
    }
}
