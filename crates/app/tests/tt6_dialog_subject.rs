//! TT6 — acceptance tests for session-overview-ui R6, the dialog subject (D9).
//!
//! R6.1–R6.3 and R6.5 read the pure `app::blocking_subject` over captured screens. R6.4
//! drives a real zsh session (the `vertical_slice.rs` seam): the dialog is put on the live
//! screen by `cat`-ing the fixture, `Session::rescan_dialog` re-reads it, and the tab row is
//! assembled by `Tabs::titles` — the row-building half reachable from `tests/`. The event
//! loop's incremental patch (`terminal_app.rs`'s private `patch_dialog_rows`) is not
//! reachable from here.
//!
//! The four fixtures are copied VERBATIM from `crates/app/src/prompt.rs`, where they are
//! `#[cfg(test)] pub(crate)` and so invisible to an integration test.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use app::blocking_subject;
use app::{Session, TabId, TabMetaSnapshot, Tabs, Wake};
use common::config::Config;
use renderer::overview_model::DialogSubject;
use term_core::Color;

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

const REAL_ANTIGRAVITY_DIALOG: &str = "\
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

const REAL_ASK_MULTI: &str = "
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

const REAL_PLAN_WIDE: &str = "
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

/// A plain zsh screen with no dialog on it.
const PLAIN_ZSH: &str = "\
user@host ~/proj % ls
Cargo.toml  crates  docs  specs
user@host ~/proj % git status --short
 M crates/app/src/prompt.rs
user@host ~/proj % 
";

// ---------------------------------------------------------------- R6.1

#[test]
fn r6_1_claude_permission_dialog_reads_its_command() {
    assert_eq!(
        blocking_subject(REAL_DIALOG),
        Some(DialogSubject::Command("ls -d ~/.config".to_string()))
    );
}

// ---------------------------------------------------------------- R6.2

#[test]
fn r6_2_antigravity_dialog_reads_its_command() {
    assert_eq!(
        blocking_subject(REAL_ANTIGRAVITY_DIALOG),
        Some(DialogSubject::Command("ls -la".to_string()))
    );
}

// ---------------------------------------------------------------- R6.3

#[test]
fn r6_3_question_plan_and_plain_screens() {
    assert_eq!(
        blocking_subject(REAL_ASK_MULTI),
        Some(DialogSubject::Question),
        "AskUserQuestion dialog"
    );
    assert_eq!(
        blocking_subject(REAL_PLAN_WIDE),
        Some(DialogSubject::Plan),
        "ExitPlanMode approval"
    );
    assert_eq!(blocking_subject(PLAIN_ZSH), None, "plain zsh screen");
}

// ---------------------------------------------------------------- R6.5

#[test]
fn r6_5_long_command_truncates_to_80_chars_ending_in_ellipsis() {
    // A command row of exactly 150 characters (after trimming), in both places the
    // fixture names the command.
    let long = format!("echo {}", "a".repeat(145));
    assert_eq!(long.chars().count(), 150);
    let screen = REAL_DIALOG.replace("ls -d ~/.config", &long);
    assert!(
        screen.contains(&format!("   {long}\n")),
        "command row present"
    );

    match blocking_subject(&screen) {
        Some(DialogSubject::Command(c)) => {
            assert_eq!(c.chars().count(), 80, "truncated length: {c:?}");
            assert_eq!(c.chars().last(), Some('…'), "ends in an ellipsis: {c:?}");
        }
        other => panic!("expected a Command subject, got {other:?}"),
    }
}

// ---------------------------------------------------------------- R6.4

const SHELL_TIMEOUT: Duration = Duration::from_secs(15);
const COLS: u16 = 80;
const ROWS: u16 = 24;

fn fg_bg() -> (Color, Color) {
    let cfg = Config::default();
    (Color(cfg.theme.foreground), Color(cfg.theme.background))
}

/// A one-tab `Tabs` over a real zsh, or `None` when shell tests are opted out of.
fn live_tabs() -> Option<Tabs> {
    if !term_core::testing::shell_tests_enabled() {
        return None;
    }
    let (fg, bg) = fg_bg();
    let wake: Wake = Box::new(|_| {});
    let first = match Session::spawn(TabId(0), COLS, ROWS, fg, bg, wake) {
        Ok(s) => s,
        Err(e) => panic!("could not spawn a session ({e}); SKIP_SHELL_TESTS=1 opts out"),
    };
    let source: std::rc::Rc<dyn Fn() -> Wake> = std::rc::Rc::new(|| -> Wake { Box::new(|_| {}) });
    Some(Tabs::new(first, fg, bg, source))
}

/// Drain the session until every needle is on the grid; panic with the grid on timeout.
fn grid_until(session: &mut Session, needles: &[&str], absent: &[&str]) -> String {
    let deadline = Instant::now() + SHELL_TIMEOUT;
    loop {
        session.drain();
        let text = session.terminal.snapshot().to_text();
        if needles.iter().all(|n| text.contains(n)) && absent.iter().all(|a| !text.contains(a)) {
            // Let any trailing chunk (the next prompt) land too.
            std::thread::sleep(Duration::from_millis(150));
            session.drain();
            return session.terminal.snapshot().to_text();
        }
        if Instant::now() > deadline {
            panic!("timed out waiting for {needles:?} (absent {absent:?}); grid:\n{text}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn fixture_file(tag: &str, body: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("tt6-dialog-{}-{tag}.txt", std::process::id()));
    std::fs::write(&path, body).expect("write fixture file");
    path
}

fn show(session: &mut Session, path: &Path) {
    session
        .write(format!("clear; cat '{}'\r", path.display()).as_bytes())
        .expect("write to shell");
}

fn row0(tabs: &Tabs) -> renderer::sidebar_row::SidebarRow {
    let rows = tabs.titles(&TabMetaSnapshot::default());
    rows.into_iter()
        .find(|r| r.tab_id == 0)
        .expect("tab 0's row")
}

#[test]
fn r6_4_subject_is_published_with_the_level() {
    let Some(mut tabs) = live_tabs() else {
        eprintln!("SKIP_SHELL_TESTS set: r6_4 skipped");
        return;
    };
    let second = REAL_DIALOG.replace("ls -d ~/.config", "git push --force origin main");
    let f1 = fixture_file("first", REAL_DIALOG);
    let f2 = fixture_file("second", &second);

    // 1. REAL_DIALOG on the live screen.
    show(tabs.active_mut(), &f1);
    grid_until(
        tabs.active_mut(),
        &["Do you want to proceed?", "Esc to cancel"],
        &[],
    );
    let (level, _) = tabs.active_mut().rescan_dialog(true);
    assert!(level, "the first dialog raises the level");
    let row = row0(&tabs);
    assert!(row.blocking_dialog);
    assert_eq!(
        row.blocking_subject,
        Some(DialogSubject::Command("ls -d ~/.config".to_string())),
        "first dialog's subject on the row"
    );

    // 2. A second dialog with a different command: the level stays true, yet the rescan
    //    reports a change because the subject moved.
    show(tabs.active_mut(), &f2);
    grid_until(
        tabs.active_mut(),
        &["git push --force origin main", "Esc to cancel"],
        &["ls -d ~/.config"],
    );
    assert_eq!(
        tabs.active_mut().rescan_dialog(true),
        (true, true),
        "same level, new subject: reported as a change"
    );
    let row = row0(&tabs);
    assert!(row.blocking_dialog);
    assert_eq!(
        row.blocking_subject,
        Some(DialogSubject::Command(
            "git push --force origin main".to_string()
        )),
        "second dialog's subject on the row"
    );

    // 3. No dialog: subject None, level false.
    tabs.active_mut().write(b"clear\r").expect("write to shell");
    grid_until(tabs.active_mut(), &[], &["Do you want to proceed?"]);
    let (level, _) = tabs.active_mut().rescan_dialog(true);
    assert!(!level, "no dialog, no level");
    let row = row0(&tabs);
    assert!(!row.blocking_dialog, "blocking_dialog = false");
    assert_eq!(row.blocking_subject, None, "no subject without a dialog");

    let _ = std::fs::remove_file(f1);
    let _ = std::fs::remove_file(f2);
}
