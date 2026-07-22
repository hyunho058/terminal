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

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use crate::error::{Error, Result};

/// The fixed shell for this terminal (C5).
pub const SHELL: &str = "/bin/zsh";

/// The locale variables zsh consults, in POSIX precedence order (R9.6):
/// `LC_ALL` overrides `LC_CTYPE`, which overrides `LANG`.
const LOCALE_VARS: [&str; 3] = ["LC_ALL", "LC_CTYPE", "LANG"];

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

/// A spawned zsh session behind a pseudo-terminal.
///
/// Keep this alive for the lifetime of the session: dropping it closes the
/// master and signals the child.
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

        let mut cmd = CommandBuilder::new(SHELL);
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
        let child = pair.slave.spawn_command(cmd).map_err(Error::pty)?;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
