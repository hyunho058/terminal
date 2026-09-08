//! Local PTY: spawns zsh and exposes read/write/resize handles.
//!
//! Constraint C5 — zsh + local PTY only. No SSH, no remote, no shell selection.
//! The reader/writer are handed to the app's I/O thread (D7: threads + channels);
//! `term-core` itself stays synchronous and runtime-free.
//!
//! This module also owns the child's locale environment (D15): without a locale
//! zsh counts `한글` as 6 characters while our grid counts 2 chars / 4 columns,
//! so ZLE's cursor arithmetic and its `\b\b  \b\b` erase sequences desync.

use std::io::{Read, Write};
use std::sync::Once;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use crate::error::{Error, Result};

/// The fixed shell for this terminal (C5).
pub const SHELL: &str = "/bin/zsh";

/// The locale variables zsh consults, in POSIX precedence order (R9.6):
/// `LC_ALL` overrides `LC_CTYPE`, which overrides `LANG`.
const LOCALE_VARS: [&str; 3] = ["LC_ALL", "LC_CTYPE", "LANG"];

/// Agent session-identity variables stripped from every child environment (D1).
///
/// `CommandBuilder::new` seeds itself from `std::env::vars_os()`, so without this
/// list a tab inherits the *launching agent's* identity wholesale. Each name here
/// answers "which agent process, and which session, is my parent" — a question
/// whose inherited answer is false inside a tab.
///
/// Why each entry is on the list:
///
/// - `AI_AGENT` — generic "an agent is driving this shell"; a tab is driven by
///   the user.
/// - `CLAUDECODE` — "you are running inside Claude Code"; false for a tab.
/// - `CLAUDE_CODE_CHILD_SESSION` — **the proximate cause of the defect**: a
///   `claude` started in the tab reads it, calls itself a nested child session,
///   suppresses its transcript, and so never writes the
///   `~/.claude/projects/**/*.jsonl` that is the only file the ai-monitor reads.
/// - `CLAUDE_CODE_ENTRYPOINT` — how the *parent* was launched, not the tab.
/// - `CLAUDE_CODE_EXECPATH` — the *parent's* executable path.
/// - `CLAUDE_CODE_MESSAGING_SOCKET` — the parent's IPC socket; a tab that talks
///   on it impersonates the parent.
/// - `CLAUDE_CODE_SESSION_ID` — the parent's session UUID; inherited, it names
///   someone else's session.
/// - `CLAUDE_CODE_SSE_PORT` — the parent's SSE transport port. Retained from the
///   reported process chain but **not present in the local measurement** (the
///   other ten were live in the launching session when this list was checked on
///   this machine); kept because removing an absent name is a no-op (R1.3).
/// - `CLAUDE_CODE_VERSION` — the parent binary's version, not the tab's.
/// - `CLAUDE_EFFORT` — the parent's reasoning-effort setting for its own run.
/// - `CLAUDE_PID` — the parent agent's process id.
///
/// **Exact names, never prefixes** (R1.2). `AI_AGENT` removes `AI_AGENT` and
/// leaves `AI_AGENT_ROLE`; a `CLAUDE_*` wipe would also take `CLAUDE_CONFIG_DIR`
/// and `CLAUDE_CODE_USE_BEDROCK`, which are the user's *configuration* and must
/// keep inheriting (R2.1).
///
/// **Extended on measurement, never on speculation** (D10). Every entry was
/// observed leaking in a real process chain. Other agent CLIs (Cursor, Codex,
/// Gemini CLI) plausibly export their own identity markers, but none has been
/// measured here — and a misspelled guess reads as covered while it leaks.
///
/// **Review trigger:** re-verify this list on a Claude Code *major version* bump.
/// **Procedure:** run `claude` in a mac-terminal tab, then
/// `env | grep -E 'CLAUDE|AI_AGENT|ANTHROPIC'`; anything that names the parent's
/// identity (as opposed to user configuration) is added here. The `debug!` in
/// [`child_command`] reports agent-ish names that were *passed through*, so drift
/// surfaces as one log line rather than a manual process-tree dig (D2).
///
/// **`SHELL` is deliberately absent and must never be added** (R2.3):
/// portable-pty's `as_command` re-sets it unconditionally after `env_clear`
/// (`cmdbuilder.rs:529`), so the removal would not take — the list would lie.
const AGENT_SESSION_VARS: [&str; 11] = [
    "AI_AGENT",
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_CODE_VERSION",
    "CLAUDE_EFFORT",
    "CLAUDE_PID",
];

/// What `Pty::spawn` should do about the child's locale (D15).
///
/// Tri-state rather than an `Option`, because "no locale at all" and "a locale
/// that is explicitly not UTF-8" call for opposite handling: the first is ours
/// to fix, the second is the user's choice to respect (loudly).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocaleAction {
    /// No locale is set anywhere — inject this `(key, value)` pair.
    Inject(&'static str, &'static str),
    /// A UTF-8 locale is already in effect; leave the environment alone.
    Ok,
    /// A locale is set and it is not UTF-8; do not override, but warn.
    NonUtf8,
}

/// Decide the child's locale treatment from an environment oracle (D15, R9).
///
/// `get` is injected so this stays a pure function testable without touching the
/// process environment (`std::env::set_var` is unsound under a test harness).
/// In production the oracle is `std::env::var`, which is the right one because
/// `portable_pty::CommandBuilder::new` seeds itself from `std::env::vars_os()`
/// and `spawn` does `env_clear()` before re-applying that snapshot.
///
/// A variable counts as *absent* when it is `None` **or** `Some("")` (R9.2) — an
/// empty locale is a cleared value, not a setting.
pub fn locale_action(get: &dyn Fn(&str) -> Option<String>) -> LocaleAction {
    // Highest-precedence variable that is actually set wins (R9.6).
    for var in LOCALE_VARS {
        match get(var) {
            Some(value) if !value.is_empty() => {
                return if implies_utf8(&value) {
                    LocaleAction::Ok // R9.3
                } else {
                    LocaleAction::NonUtf8 // R9.4
                };
            }
            _ => continue,
        }
    }
    // Nothing set anywhere (R9.1). `LC_CTYPE=UTF-8` is what macOS Terminal.app
    // uses: it is valid regardless of which locale definitions are installed and
    // it does not change the message language the way `LANG=en_US.UTF-8` would.
    LocaleAction::Inject("LC_CTYPE", "UTF-8")
}

/// Does a locale string name a UTF-8 charset? Matches `utf-8` and `utf8`,
/// case-insensitively (`ko_KR.UTF-8`, `en_US.utf8`, `C.UTF-8`, …).
fn implies_utf8(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("utf-8") || lower.contains("utf8")
}

/// Warn about a non-UTF-8 locale exactly once per process (R9.4). `Pty::spawn`
/// is called per tab, and ten tabs must not produce ten identical lines.
fn warn_non_utf8_locale() {
    static WARNED: Once = Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            "non-UTF-8 locale in LC_ALL/LC_CTYPE/LANG; leaving it as-is. \
             Wide characters (e.g. 한글) may desync between the shell's line \
             editor and the grid — zsh counts them by a different width."
        );
    });
}

/// Remove every [`AGENT_SESSION_VARS`] name from `cmd`, returning the names that
/// were actually there (D3, D11, R1.1-R1.3, R4.1).
///
/// Unconditional and oracle-free: `env_remove` on an absent key is a no-op, so
/// there is nothing to gain from asking the process environment first — and
/// asking would make the result depend on whether `cargo test` happens to run
/// inside a Claude session. Presence is read off the **`CommandBuilder`** (never
/// `std::env`) *before* the removal, which is what makes the returned set
/// reportable after the names are gone from the map.
fn strip_agent_session_vars(cmd: &mut CommandBuilder) -> Vec<&'static str> {
    let mut stripped = Vec::new();
    for name in AGENT_SESSION_VARS {
        if cmd.get_env(name).is_some() {
            stripped.push(name);
        }
        cmd.env_remove(name);
    }
    stripped
}

/// Does `name` look like an agent variable for the drift survey (D11, R4.4)?
///
/// Prefix `CLAUDE` or `ANTHROPIC`, plus exact `AI_AGENT` — `AI_AGENT_ROLE` is a
/// user's own variable and is not reported. Our own shell-integration names
/// (`ZDOTDIR`, `MAC_TERMINAL_USER_ZDOTDIR`) match nothing here on purpose: the
/// survey exists to catch drift in a *third party's* variable set, and ours
/// cannot drift (R4.6).
fn is_agent_ish(name: &str) -> bool {
    name.starts_with("CLAUDE") || name.starts_with("ANTHROPIC") || name == "AI_AGENT"
}

/// Agent-ish names in the **final** builder — literally what the child receives
/// (D11, R4.2). Read from the builder, not the process environment, so it is
/// deterministic under test. Names only; values are never collected.
fn survey_agent_passthrough(cmd: &CommandBuilder) -> Vec<String> {
    cmd.iter_full_env_as_str()
        .map(|(name, _value)| name)
        .filter(|name| is_agent_ish(name))
        .map(str::to_string)
        .collect()
}

/// Compose the drift-trace line, or `None` when there is nothing to report
/// (D11, R4.3, R4.5).
///
/// **Names only, never values.** The survey's source is the whole child
/// environment, which holds `ANTHROPIC_API_KEY`, cloud credentials and a socket
/// path. A variable's name is not a secret; its value is.
fn drift_trace(stripped: &[&'static str], passed_through: &[String]) -> Option<String> {
    if stripped.is_empty() && passed_through.is_empty() {
        return None; // R4.5 — a Finder launch has nothing to say
    }
    Some(format!(
        "pty child env (names only, no values): stripped agent session identity [{}]; \
         agent-ish variables passed through [{}]",
        stripped.join(", "),
        passed_through.join(", ")
    ))
}

/// Assemble the child's environment and return the command to spawn (T1).
///
/// Split out of [`Pty::spawn`] so the assembled environment is assertable without
/// spawning a real zsh — `spawn` itself only does `openpty` + `spawn_command`.
///
/// Order is load-bearing (D12, R1.4): the agent denylist runs **first**, so our
/// own `TERM` / locale / `ZDOTDIR` additions can never be removed by a later
/// denylist entry, and the trace's survey describes the finished environment.
fn child_command() -> CommandBuilder {
    let mut cmd = CommandBuilder::new(SHELL);
    // D12: first, before anything we set. A tab is a new session (D8) — there is
    // no opt-out (R5.1).
    let stripped = strip_agent_session_vars(&mut cmd);

    cmd.env("TERM", "xterm-256color");
    // D15 / R9.5: a Finder-launched `.app` inherits a minimal environment
    // with no locale, so this must be decided here rather than assumed.
    match locale_action(&|k| std::env::var(k).ok()) {
        LocaleAction::Inject(key, value) => cmd.env(key, value),
        LocaleAction::Ok => {}
        LocaleAction::NonUtf8 => warn_non_utf8_locale(),
    }
    // Point zsh at our generated ZDOTDIR so it emits OSC 133 command markers
    // (shell-integration R4.1-R4.3). Chains the user's rc, add-only; degrades
    // to a plain shell on failure (spec D13). Preserves TERM/locale set above.
    crate::shell_integration::inject(&mut cmd);

    // D2/D11/D16: one line per spawn, at `debug!` — inert without a subscriber
    // (R5.3), silent when there is nothing to report (R4.5).
    if let Some(line) = drift_trace(&stripped, &survey_agent_passthrough(&cmd)) {
        tracing::debug!("{line}");
    }
    cmd
}

/// How long [`Pty::drop`] waits for a signalled shell before escalating.
///
/// Covers scheduling, not the shell's own work: zsh answers `SIGHUP` by exiting
/// at once, and anything still alive after this has trapped the signal.
const REAP_TIMEOUT: Duration = Duration::from_millis(500);
/// Poll interval inside that budget.
const REAP_POLL: Duration = Duration::from_millis(5);

/// A spawned zsh session behind a pseudo-terminal.
///
/// Keep this alive for the lifetime of the session: dropping it signals the
/// child and reaps it.
pub struct Pty {
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl Pty {
    /// Spawn zsh in a PTY sized to `cols * rows`.
    pub fn spawn(cols: u16, rows: u16) -> Result<Pty> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(Error::pty)?;

        let child = pair
            .slave
            .spawn_command(child_command())
            .map_err(Error::pty)?;
        // Slave handle is not needed once the child holds it.
        drop(pair.slave);

        Ok(Pty {
            master: pair.master,
            child,
        })
    }

    /// Clone a reader for the PTY output stream (move to the reader thread).
    pub fn reader(&self) -> Result<Box<dyn Read + Send>> {
        self.master.try_clone_reader().map_err(Error::pty)
    }

    /// Take the single writer for sending input to the shell.
    pub fn writer(&self) -> Result<Box<dyn Write + Send>> {
        self.master.take_writer().map_err(Error::pty)
    }

    /// The OS process id of the shell child, if still known. Lets the app query
    /// the live working directory (e.g. for tab titles / ai-monitor log
    /// resolution) via `proc_pidinfo`. Read-only observation.
    pub fn child_pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// Inform the kernel of a new window size so the shell reflows.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(Error::pty)
    }
}

/// Signal the shell **and reap it**.
///
/// Dropping a `Pty` used to do neither directly. The child noticed only
/// indirectly, when the last owner of the master fd went away, and nothing ever
/// called `wait`, so every shell that exited stayed a zombie for the life of the
/// process. Measured on the `app` lib tests, whose fixtures open a real shell per
/// tab: **55** zombie children at peak, every one of them a shell that had
/// already exited. The app leaks the same way, one per closed tab, for the whole
/// session.
///
/// Two details decide the shape of this.
///
/// The signal is not redundant with the fd closing. `Session` declares `pty`
/// before `writer`, and the reader thread holds its own clone, so at this point
/// the master is still open and the shell has had no reason to exit yet. An
/// unbounded `wait()` here would deadlock against a fd that only closes after
/// this function returns.
///
/// And the budget is not belt-and-braces. `portable_pty`'s `kill` sends
/// `SIGHUP`, not `SIGKILL`, and `SIGHUP` is catchable — the spawned zsh chains
/// the user's own `.zshrc` (D13), so a `trap '' HUP` in it would hang whoever
/// dropped this, which on a tab close is the UI thread. After the budget the
/// escalation is `SIGKILL`, which cannot be trapped, so the final `wait` is
/// bounded by the kernel rather than by the shell.
impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child.kill();

        let deadline = Instant::now() + REAP_TIMEOUT;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Err(e) => {
                    tracing::warn!("could not reap the shell: {e}");
                    return;
                }
                Ok(None) => {}
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(REAP_POLL);
        }

        if let Some(pid) = self.child.process_id() {
            // SAFETY: `kill(2)` against a pid this process spawned and has not
            // yet reaped, so the pid cannot have been recycled. The only error is
            // `ESRCH` for one that just exited, which is the outcome we want.
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    /// Build a stub oracle over `(name, value)` pairs. Never touches the real
    /// environment — `std::env::set_var` is unsound under a threaded harness.
    fn stub<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn locale_action_table() {
        let inject = LocaleAction::Inject("LC_CTYPE", "UTF-8");
        // (requirement label, stub environment, expected action).
        type Case<'a> = (&'a str, &'a [(&'a str, &'a str)], LocaleAction);
        let cases: &[Case] = &[
            // R9.1 — nothing set anywhere.
            ("R9.1 all absent", &[], inject),
            // R9.2 — an empty value is a cleared value, not a setting.
            ("R9.2 LANG empty", &[("LANG", "")], inject),
            (
                "R9.2 all three empty",
                &[("LC_ALL", ""), ("LC_CTYPE", ""), ("LANG", "")],
                inject,
            ),
            // R9.3 — an existing UTF-8 locale is left alone.
            (
                "R9.3 LANG utf-8",
                &[("LANG", "ko_KR.UTF-8")],
                LocaleAction::Ok,
            ),
            (
                "R9.3 lowercase utf8 spelling",
                &[("LC_CTYPE", "en_US.utf8")],
                LocaleAction::Ok,
            ),
            (
                "R9.3 LC_ALL utf-8",
                &[("LC_ALL", "C.UTF-8"), ("LANG", "C")],
                LocaleAction::Ok,
            ),
            // R9.4 — explicit non-UTF-8 is respected (and warned about at spawn).
            ("R9.4 LC_ALL=C", &[("LC_ALL", "C")], LocaleAction::NonUtf8),
            (
                "R9.4 LANG=POSIX",
                &[("LANG", "POSIX")],
                LocaleAction::NonUtf8,
            ),
            (
                "R9.4 legacy 8-bit charset",
                &[("LC_CTYPE", "ko_KR.eucKR")],
                LocaleAction::NonUtf8,
            ),
            // R9.6 — POSIX precedence: LC_ALL > LC_CTYPE > LANG.
            (
                "R9.6 LC_ALL beats LANG",
                &[("LC_ALL", "C"), ("LANG", "ko_KR.UTF-8")],
                LocaleAction::NonUtf8,
            ),
            (
                "R9.6 LC_CTYPE beats LANG",
                &[("LC_CTYPE", "C"), ("LANG", "ko_KR.UTF-8")],
                LocaleAction::NonUtf8,
            ),
            (
                "R9.6 empty LC_ALL falls through to LC_CTYPE",
                &[("LC_ALL", ""), ("LC_CTYPE", "ko_KR.UTF-8"), ("LANG", "C")],
                LocaleAction::Ok,
            ),
        ];

        for (name, env, expected) in cases {
            let got = locale_action(&stub(env));
            assert_eq!(got, *expected, "case: {name}");
        }
    }

    #[test]
    fn injection_targets_lc_ctype_utf8() {
        // The injected pair is load-bearing: `LC_CTYPE=UTF-8` is valid without
        // any locale definitions installed and does not change message language.
        match locale_action(&stub(&[])) {
            LocaleAction::Inject(key, value) => {
                assert_eq!(key, "LC_CTYPE");
                assert_eq!(value, "UTF-8");
            }
            other => panic!("expected Inject, got {other:?}"),
        }
    }

    #[test]
    fn precedence_order_is_posix() {
        assert_eq!(LOCALE_VARS, ["LC_ALL", "LC_CTYPE", "LANG"]);
    }

    #[test]
    fn warn_non_utf8_locale_is_once_per_process() {
        // R9.4: `Pty::spawn` runs per tab; the `Once` must swallow repeats.
        for _ in 0..10 {
            warn_non_utf8_locale();
        }
    }

    /// `CommandBuilder::new` seeds itself from the real process env, so every
    /// environment test starts from a **cleared** builder and seeds *fake*
    /// markers. That is what makes the results identical on the dev machine
    /// (where the real markers are live) and on a CI runner (where they are not)
    /// — R6.1. `std::env::set_var` is never used: it is unsound under a threaded
    /// harness, which is the same reason `locale_action` takes an oracle.
    fn seeded_cmd(pairs: &[(&str, &str)]) -> CommandBuilder {
        let mut cmd = CommandBuilder::new(SHELL);
        cmd.env_clear();
        for (key, value) in pairs {
            cmd.env(key, value);
        }
        cmd
    }

    /// All eleven markers at fake values — nothing here is a real session id,
    /// socket or pid.
    const FAKE_MARKERS: &[(&str, &str)] = &[
        ("AI_AGENT", "fake-agent"),
        ("CLAUDECODE", "1"),
        ("CLAUDE_CODE_CHILD_SESSION", "1"),
        ("CLAUDE_CODE_ENTRYPOINT", "fake-cli"),
        ("CLAUDE_CODE_EXECPATH", "/fake/bin/claude"),
        ("CLAUDE_CODE_MESSAGING_SOCKET", "/fake/cc-socks/2499.sock"),
        ("CLAUDE_CODE_SESSION_ID", "fake-02a3919c"),
        ("CLAUDE_CODE_SSE_PORT", "54321"),
        ("CLAUDE_CODE_VERSION", "0.0.0-fake"),
        ("CLAUDE_EFFORT", "fake-effort"),
        ("CLAUDE_PID", "2499"),
    ];

    /// Fake *configuration* — deliberate user state that must keep inheriting.
    const FAKE_CONFIG: &[(&str, &str)] = &[
        ("CLAUDE_CONFIG_DIR", "/fake/x"),
        ("CLAUDE_CODE_USE_BEDROCK", "1"),
        ("ANTHROPIC_API_KEY", "fake-sk-secret"),
    ];

    #[test]
    fn strip_agent_session_vars_table() {
        // (requirement label, seeded env, names the strip must report, survivors).
        type Case<'a> = (
            &'a str,
            &'a [(&'a str, &'a str)],
            &'a [&'a str],
            &'a [(&'a str, &'a str)],
        );
        let cases: &[Case] = &[
            // R1.1 / R4.1 — every name removed, and every name reported even
            // though `get_env` returns `None` for them afterwards.
            (
                "R1.1 all eleven present",
                FAKE_MARKERS,
                &AGENT_SESSION_VARS,
                &[],
            ),
            // R1.2 — exact names only; the prefix neighbours survive intact.
            (
                "R1.2 prefix neighbours survive",
                &[
                    ("AI_AGENT", "fake-agent"),
                    ("AI_AGENT_ROLE", "y"),
                    ("CLAUDECODE", "1"),
                    ("CLAUDECODE_FUTURE", "x"),
                ],
                &["AI_AGENT", "CLAUDECODE"],
                &[("AI_AGENT_ROLE", "y"), ("CLAUDECODE_FUTURE", "x")],
            ),
            // R1.3 — the Finder-launch case: nothing present, nothing changes.
            (
                "R1.3 none present is a no-op",
                &[("PATH", "/usr/bin"), ("HOME", "/Users/tester")],
                &[],
                &[("PATH", "/usr/bin"), ("HOME", "/Users/tester")],
            ),
            ("R1.3 empty builder", &[], &[], &[]),
            // R2.1 — configuration is not identity; all three keep their values.
            ("R2.1 configuration survives", FAKE_CONFIG, &[], FAKE_CONFIG),
            // R1.1 + R2.1 together: the realistic mixed environment.
            (
                "R1.1+R2.1 markers stripped, config kept",
                &[
                    ("CLAUDE_CODE_CHILD_SESSION", "1"),
                    ("CLAUDE_PID", "2499"),
                    ("CLAUDE_CONFIG_DIR", "/fake/x"),
                    ("ANTHROPIC_API_KEY", "fake-sk-secret"),
                ],
                &["CLAUDE_CODE_CHILD_SESSION", "CLAUDE_PID"],
                &[
                    ("CLAUDE_CONFIG_DIR", "/fake/x"),
                    ("ANTHROPIC_API_KEY", "fake-sk-secret"),
                ],
            ),
        ];

        for (name, seed, expect_stripped, expect_kept) in cases {
            let mut cmd = seeded_cmd(seed);
            let stripped = strip_agent_session_vars(&mut cmd);
            assert_eq!(stripped, *expect_stripped, "case: {name}");
            for var in AGENT_SESSION_VARS {
                assert!(cmd.get_env(var).is_none(), "case: {name}: {var} survived");
            }
            for (key, value) in *expect_kept {
                assert_eq!(
                    cmd.get_env(key),
                    Some(OsStr::new(*value)),
                    "case: {name}: {key}"
                );
            }
            // Nothing beyond the denylist was touched (the R1.3 "unchanged" half).
            assert_eq!(
                cmd.iter_full_env_as_str().count(),
                expect_kept.len(),
                "case: {name}: unexpected extra/missing entries"
            );
        }
    }

    /// R2.3 — `SHELL` must never join the list: `as_command` re-sets it
    /// unconditionally (`cmdbuilder.rs:529`), so the removal would not take.
    #[test]
    fn shell_is_absent_from_the_denylist() {
        assert!(!AGENT_SESSION_VARS.contains(&"SHELL"));
    }

    /// R1.4 (structural half) — the strip cannot remove anything `child_command`
    /// sets, because no name it sets is on the list.
    #[test]
    fn denylist_shape_is_eleven_unique_names_we_never_set() {
        assert_eq!(AGENT_SESSION_VARS.len(), 11);
        let mut unique = AGENT_SESSION_VARS.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 11, "duplicate entry in the denylist");
        for ours in ["TERM", "ZDOTDIR", "MAC_TERMINAL_USER_ZDOTDIR"]
            .iter()
            .chain(LOCALE_VARS.iter())
        {
            assert!(
                !AGENT_SESSION_VARS.contains(ours),
                "{ours} is set by child_command and must not be on the denylist"
            );
        }
    }

    /// R1.4 (assembled half) — after the whole assembly, `TERM` holds the value
    /// *we* set and none of the eleven reaches the child. Both assertions hold
    /// identically inside and outside a Claude session (R6.1).
    #[test]
    fn child_command_strips_first_then_sets_our_own() {
        let cmd = child_command();
        assert_eq!(cmd.get_env("TERM"), Some(OsStr::new("xterm-256color")));
        for var in AGENT_SESSION_VARS {
            assert!(cmd.get_env(var).is_none(), "{var} reached the child");
        }
    }

    #[test]
    fn agent_ish_survey_table() {
        // (requirement label, seeded env, names the survey must report — sorted).
        type Case<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a [&'a str]);
        let cases: &[Case] = &[
            // R4.4 — prefix CLAUDE/ANTHROPIC plus exact AI_AGENT, nothing else.
            (
                "R4.4 filter",
                &[
                    ("CLAUDE_X", "1"),
                    ("ANTHROPIC_Y", "2"),
                    ("AI_AGENT", "fake-agent"),
                    ("AI_AGENT_ROLE", "y"),
                    ("PATH", "/usr/bin"),
                ],
                &["AI_AGENT", "ANTHROPIC_Y", "CLAUDE_X"],
            ),
            // R4.2 — a known config var and an *unknown* marker both report,
            // because both are heading into the child.
            (
                "R4.2 known config + unknown marker",
                &[
                    ("CLAUDE_CONFIG_DIR", "/fake/x"),
                    ("CLAUDE_CODE_PARENT_RUN_ID", "abc"),
                    ("TERM", "xterm-256color"),
                ],
                &["CLAUDE_CODE_PARENT_RUN_ID", "CLAUDE_CONFIG_DIR"],
            ),
            // R4.5 — a Finder-launched environment reports nothing.
            (
                "R4.5 nothing agent-ish",
                &[("PATH", "/usr/bin"), ("TERM", "xterm-256color")],
                &[],
            ),
            // R4.6 — our own shell-integration names are not agent-ish.
            (
                "R4.6 shell-integration family is invisible here",
                &[
                    ("ZDOTDIR", "/fake/generated"),
                    ("MAC_TERMINAL_USER_ZDOTDIR", "/fake/users/zsh"),
                ],
                &[],
            ),
        ];

        for (name, seed, expected) in cases {
            let cmd = seeded_cmd(seed);
            let mut got = survey_agent_passthrough(&cmd);
            got.sort_unstable(); // don't couple the assertion to map iteration order
            assert_eq!(got, *expected, "case: {name}");
        }
    }

    /// R4.3 — the composed line names variables and leaks no value, not even the
    /// socket path or the API key that sit in the same environment.
    #[test]
    fn drift_trace_emits_names_never_values() {
        let mut cmd = seeded_cmd(
            &FAKE_MARKERS
                .iter()
                .chain(FAKE_CONFIG.iter())
                .copied()
                .collect::<Vec<_>>(),
        );
        let stripped = strip_agent_session_vars(&mut cmd);
        let line = drift_trace(&stripped, &survey_agent_passthrough(&cmd))
            .expect("something was stripped, so a line is due");

        // Names of both halves are present.
        assert!(line.contains("CLAUDE_CODE_MESSAGING_SOCKET"), "{line}");
        assert!(line.contains("ANTHROPIC_API_KEY"), "{line}");
        // No seeded value appears anywhere in the line.
        for (key, value) in FAKE_MARKERS.iter().chain(FAKE_CONFIG.iter()) {
            assert!(
                !line.contains(value),
                "value of {key} leaked into the trace: {line}"
            );
        }
    }

    /// R4.5 — nothing stripped and nothing agent-ish passed through → no line at
    /// all, so a Finder launch logs nothing.
    #[test]
    fn drift_trace_is_silent_when_there_is_nothing_to_report() {
        let cmd = seeded_cmd(&[("PATH", "/usr/bin"), ("TERM", "xterm-256color")]);
        let stripped: Vec<&'static str> = Vec::new();
        assert!(drift_trace(&stripped, &survey_agent_passthrough(&cmd)).is_none());
    }

    /// R4.6 — in a nested launch neither `ZDOTDIR` nor `MAC_TERMINAL_USER_ZDOTDIR`
    /// appears in either half of the line. `inject_with` owns that family and
    /// returns `()`; no signature is changed to surface it here.
    #[test]
    fn drift_trace_omits_the_shell_integration_family() {
        let mut seed: Vec<(&str, &str)> = FAKE_MARKERS.to_vec();
        seed.push(("ZDOTDIR", "/fake/generated"));
        seed.push(("MAC_TERMINAL_USER_ZDOTDIR", "/fake/stale"));
        let mut cmd = seeded_cmd(&seed);
        let stripped = strip_agent_session_vars(&mut cmd);
        // Stand in for what `inject` re-sets after the strip.
        cmd.env("ZDOTDIR", "/fake/generated");
        cmd.env("MAC_TERMINAL_USER_ZDOTDIR", "/fake/users/zsh");

        let line = drift_trace(&stripped, &survey_agent_passthrough(&cmd)).expect("a line is due");
        // "MAC_TERMINAL_USER_ZDOTDIR" contains "ZDOTDIR", so one check covers both.
        assert!(!line.contains("ZDOTDIR"), "{line}");
    }

    /// Dropping a `Pty` leaves no zombie.
    ///
    /// Signal 0 delivers nothing and only asks whether the pid exists, which is
    /// precisely the right question: a zombie still *answers* it, because the pid
    /// stays valid until someone waits on it. So this distinguishes "the shell
    /// exited" — which was always true — from "the shell was reaped", which was
    /// not, and which nothing observed. Before the `Drop` impl the second
    /// assertion fails while the first passes.
    ///
    /// One real shell, deliberately not `#[ignore]`d: the whole point is that the
    /// leak is invisible to every test that does not spawn one.
    #[test]
    fn dropping_a_pty_reaps_its_shell() {
        let pty = Pty::spawn(80, 24).expect("spawn a shell");
        let pid = pty.child_pid().expect("a live shell has a pid") as libc::pid_t;
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            0,
            "pid {pid} is not running before the drop, so this proves nothing"
        );

        drop(pty);

        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "pid {pid} still exists after the drop — the shell was signalled but \
             never reaped, and stays a zombie for the life of the process"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH),
            "the pid should be gone, not merely unreachable"
        );
    }
}
