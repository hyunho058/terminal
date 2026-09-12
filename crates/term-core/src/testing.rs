//! Test support that has to be shared across crates.
//!
//! Deliberately **not** `#[cfg(test)]`. A `cfg(test)` item exists only inside its
//! own crate's own test build, so it is unreachable from an integration test —
//! which is a separate crate — and from every other crate in the workspace. That
//! is not a theoretical limit: the shell opt-out below was extracted once into
//! `app::session` as a `#[cfg(test)] pub(crate) fn`, whose doc said "the opt-out
//! is a contract or it is nothing", and it promptly acquired three hand-written
//! copies — `app`'s integration test, `pty.rs`'s own test, and the original —
//! because none of them could name it. It lives here because `term-core` is the
//! crate all of them already depend on.

/// Whether tests that need a real `/bin/zsh` may run.
///
/// `SKIP_SHELL_TESTS=1` is the workspace-wide opt-out for a machine with no usable
/// shell, and this is the single place that reads it. Callers today:
/// `pty::tests::dropping_a_pty_reaps_its_shell`, `app::session`'s and `app::tabs`'
/// fixtures, and `crates/app/tests/vertical_slice.rs`.
///
/// Note what this deliberately does **not** do: it never turns a *failed* spawn
/// into a skip. A machine that has zsh and cannot start it is reporting a defect,
/// and every caller still panics on that. Only an explicit variable skips, so
/// skipping is always someone's decision.
pub fn shell_tests_enabled() -> bool {
    if std::env::var_os("SKIP_SHELL_TESTS").is_some() {
        eprintln!("SKIP_SHELL_TESTS set; skipping a test that needs a real shell");
        return false;
    }
    true
}
