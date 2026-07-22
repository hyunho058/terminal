//! ZDOTDIR-based zsh shell integration (spec D10/D18/D19).
//!
//! We materialize a private `ZDOTDIR` containing a generated `.zshenv` + `.zshrc`
//! and point the child zsh at it (`Pty::spawn`). Those scripts **chain** the
//! user's real startup files first, then register OSC 133 `preexec`/`precmd`
//! hooks add-only — so a command's exit code and shell-measured elapsed time are
//! emitted to the PTY without disturbing the user's prompt (oh-my-zsh / starship
//! / powerlevel10k keep working).
//!
//! Everything here degrades gracefully (spec D13): on any failure the shell still
//! spawns, just without OSC 133 markers. No `#[cfg(target_os)]` — this is a
//! macOS-only project.

use std::path::PathBuf;

use portable_pty::CommandBuilder;

/// The generated `.zshenv` (chains the user's env for every zsh).
const ZSHENV: &str = include_str!("../assets/shell-integration/zshenv.zsh");
/// The generated `.zshrc` (chains the user's interactive config, then adds the
/// OSC 133 command markers).
const ZSHRC: &str = include_str!("../assets/shell-integration/zshrc.zsh");

/// Env var carrying the user's *real* `ZDOTDIR` (or `$HOME`) into the child, so
/// the generated scripts can source the user's files after we override `ZDOTDIR`.
const USER_ZDOTDIR_VAR: &str = "MAC_TERMINAL_USER_ZDOTDIR";

/// Configure `cmd` to load our OSC 133 shell integration via `ZDOTDIR`.
///
/// No-op on any failure (graceful degradation, spec D13). Reads the process
/// environment through `get_env` (injected so this is unit-testable without
/// touching real env vars).
pub(crate) fn inject(cmd: &mut CommandBuilder) {
    inject_with(cmd, &|k| std::env::var(k).ok(), materialize)
}

/// Testable core of [`inject`]. `get_env` resolves environment variables and
/// `write_dir` materializes the scripts and returns the generated ZDOTDIR path.
fn inject_with(
    cmd: &mut CommandBuilder,
    get_env: &dyn Fn(&str) -> Option<String>,
    write_dir: impl Fn() -> Option<PathBuf>,
) {
    // Capture the user's real ZDOTDIR (fallback $HOME) BEFORE we override it, so
    // the generated rc can chain the user's files instead of clobbering them.
    let user_zdotdir = get_env("ZDOTDIR")
        .filter(|s| !s.is_empty())
        .or_else(|| get_env("HOME"))
        .filter(|s| !s.is_empty());
    let Some(user_zdotdir) = user_zdotdir else {
        return; // no HOME/ZDOTDIR to chain from → run a plain shell (degrade)
    };
    let Some(dir) = write_dir() else {
        return; // could not materialize the scripts → degrade
    };
    cmd.env("ZDOTDIR", dir);
    cmd.env(USER_ZDOTDIR_VAR, user_zdotdir);
}

/// Write the generated scripts into a stable per-user support directory and
/// return it. `None` on any I/O failure (caller degrades). Rewritten on every
/// spawn so the scripts never go stale across app versions (cheap: two ~1 KiB
/// files, and an idempotent overwrite).
fn materialize() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir =
        PathBuf::from(home).join("Library/Application Support/mac-terminal/shell-integration");
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(dir.join(".zshenv"), ZSHENV).ok()?;
    std::fs::write(dir.join(".zshrc"), ZSHRC).ok()?;
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    /// The generated scripts must honor the wire contract the parser expects and
    /// the chain-not-clobber invariant (spec D18/D19). A drift here silently
    /// breaks the completion/error signals.
    #[test]
    fn scripts_uphold_contract() {
        // Emits the command-end marker the parser reads (`133 ; D ; exit ; ms`).
        assert!(ZSHRC.contains(r"\033]133;D;%d;%d\a"));
        // Elapsed is measured shell-side from EPOCHREALTIME (D19).
        assert!(ZSHRC.contains("EPOCHREALTIME"));
        // Add-only hook registration — never clobbers the user's prompt (D18).
        assert!(ZSHRC.contains("add-zsh-hook"));
        // Both scripts chain the user's real config via the passed-through var.
        assert!(ZSHENV.contains(USER_ZDOTDIR_VAR));
        assert!(ZSHRC.contains(USER_ZDOTDIR_VAR));
        assert!(ZSHENV.contains(".zshenv"));
        assert!(ZSHRC.contains(".zshrc"));
    }

    /// `CommandBuilder::new` seeds itself from the real process env, so each test
    /// starts from a cleared builder to assert only what `inject_with` sets.
    fn clean_cmd() -> CommandBuilder {
        let mut cmd = CommandBuilder::new("/bin/zsh");
        cmd.env_clear();
        cmd
    }

    /// `inject_with` sets both env vars when a home dir and the scripts exist.
    #[test]
    fn inject_sets_zdotdir_and_user_passthrough() {
        let mut cmd = clean_cmd();
        let fake_dir = PathBuf::from("/tmp/mac-terminal-test-zdotdir");
        inject_with(
            &mut cmd,
            &|k| (k == "HOME").then(|| "/Users/tester".to_string()),
            || Some(fake_dir.clone()),
        );
        assert_eq!(cmd.get_env("ZDOTDIR"), Some(fake_dir.as_os_str()));
        assert_eq!(
            cmd.get_env(USER_ZDOTDIR_VAR),
            Some(OsStr::new("/Users/tester"))
        );
    }

    /// A real ZDOTDIR wins over $HOME as the chained source dir.
    #[test]
    fn inject_prefers_existing_zdotdir() {
        let mut cmd = clean_cmd();
        inject_with(
            &mut cmd,
            &|k| match k {
                "ZDOTDIR" => Some("/Users/tester/.config/zsh".to_string()),
                "HOME" => Some("/Users/tester".to_string()),
                _ => None,
            },
            || Some(PathBuf::from("/tmp/x")),
        );
        assert_eq!(
            cmd.get_env(USER_ZDOTDIR_VAR),
            Some(OsStr::new("/Users/tester/.config/zsh"))
        );
    }

    /// No home and no ZDOTDIR → no injection (graceful degradation, spec D13).
    #[test]
    fn inject_degrades_without_home() {
        let mut cmd = clean_cmd();
        inject_with(&mut cmd, &|_| None, || Some(PathBuf::from("/tmp/x")));
        assert!(cmd.get_env("ZDOTDIR").is_none());
        assert!(cmd.get_env(USER_ZDOTDIR_VAR).is_none());
    }

    /// A failed materialization → no ZDOTDIR override (degrade), even with a home.
    #[test]
    fn inject_degrades_when_materialize_fails() {
        let mut cmd = clean_cmd();
        inject_with(
            &mut cmd,
            &|k| (k == "HOME").then(|| "/Users/tester".to_string()),
            || None,
        );
        assert!(cmd.get_env("ZDOTDIR").is_none());
    }
}
