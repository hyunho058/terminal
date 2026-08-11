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

use std::path::{Path, PathBuf};

use portable_pty::CommandBuilder;

/// The generated `.zshenv` (chains the user's env for every zsh).
const ZSHENV: &str = include_str!("../assets/shell-integration/zshenv.zsh");
/// The generated `.zshrc` (chains the user's interactive config, then adds the
/// OSC 133 command markers).
const ZSHRC: &str = include_str!("../assets/shell-integration/zshrc.zsh");

/// Env var carrying the user's *real* `ZDOTDIR` (or `$HOME`) into the child, so
/// the generated scripts can source the user's files after we override `ZDOTDIR`.
const USER_ZDOTDIR_VAR: &str = "MAC_TERMINAL_USER_ZDOTDIR";

/// Where the generated scripts live, relative to `$HOME`.
const SUPPORT_DIR_SUFFIX: &str = "Library/Application Support/mac-terminal/shell-integration";

/// Configure `cmd` to load our OSC 133 shell integration via `ZDOTDIR`.
///
/// No-op on any failure (graceful degradation, spec D13). Reads the process
/// environment through `get_env` (injected so this is unit-testable without
/// touching real env vars).
pub(crate) fn inject(cmd: &mut CommandBuilder) {
    let get_env = |k: &str| std::env::var(k).ok();
    inject_with(cmd, &get_env, || materialize(&get_env))
}

/// Testable core of [`inject`]. `get_env` resolves environment variables and
/// `write_dir` materializes the scripts and returns the generated ZDOTDIR path.
///
/// Both of our markers are *re-derived*, never inherited
/// (`pty-session-env-isolation` D7/D9/D13): when the app is itself launched from
/// a mac-terminal tab, the parent env already carries a `ZDOTDIR` pointing at our
/// generated dir and a `MAC_TERMINAL_USER_ZDOTDIR` we wrote one level up. Passing
/// those on would make `zshrc.zsh:9` source *itself*.
fn inject_with(
    cmd: &mut CommandBuilder,
    get_env: &dyn Fn(&str) -> Option<String>,
    write_dir: impl Fn() -> Option<PathBuf>,
) {
    let inherited_user = get_env(USER_ZDOTDIR_VAR).filter(|s| !s.is_empty());
    let inherited_zdotdir = get_env("ZDOTDIR").filter(|s| !s.is_empty());

    // Is the parent one of *our* shells? Answered primarily by NAME: an inherited
    // MAC_TERMINAL_USER_ZDOTDIR exists only because one of our shells set it, and
    // we always set it together with ZDOTDIR. The path comparison is the secondary
    // guard, catching a hand-set ZDOTDIR=<our dir> with no companion marker. With
    // HOME absent the helper returns None, the path guard simply does not fire,
    // and the name rule still carries the ordinary nested case (D13).
    let zdotdir_is_ours = inherited_user.is_some()
        || inherited_zdotdir.as_deref().is_some_and(|z| {
            generated_zdotdir_path(get_env).is_some_and(|ours| ours.as_path() == Path::new(z))
        });

    // Remove both markers BEFORE every early return below (D13). On the degrade
    // paths nothing is re-set, so a marker left in place ships the *parent's*
    // value straight to the child — which is the bug, on the path meant to fix it.
    cmd.env_remove(USER_ZDOTDIR_VAR); // purely ours, never user-set
    if zdotdir_is_ours {
        cmd.env_remove("ZDOTDIR"); // a user's real ZDOTDIR is config — it survives
    }

    // Capture the user's real ZDOTDIR BEFORE we override it, so the generated rc
    // can chain the user's files instead of clobbering them. Three tiers, first
    // non-empty wins (D9):
    //   1. inherited MAC_TERMINAL_USER_ZDOTDIR — ground truth, we wrote it one
    //      level up, so nesting depth is irrelevant
    //   2. inherited ZDOTDIR, unless it is recognised as ours
    //   3. $HOME
    let user_zdotdir = inherited_user
        .or_else(|| inherited_zdotdir.filter(|_| !zdotdir_is_ours))
        .or_else(|| get_env("HOME").filter(|s| !s.is_empty()));
    let Some(user_zdotdir) = user_zdotdir else {
        return; // no HOME/ZDOTDIR to chain from → run a plain shell (degrade)
    };
    let Some(dir) = write_dir() else {
        return; // could not materialize the scripts → degrade
    };
    cmd.env("ZDOTDIR", dir);
    cmd.env(USER_ZDOTDIR_VAR, user_zdotdir);
}

/// Where our generated `ZDOTDIR` *would* live: `$HOME` joined with the support-dir
/// suffix. **Pure** — no I/O of any kind (spec D13), so the "is this ZDOTDIR ours?"
/// predicate above works even on the path where [`materialize`] fails.
fn generated_zdotdir_path(get_env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let home = get_env("HOME").filter(|s| !s.is_empty())?;
    Some(PathBuf::from(home).join(SUPPORT_DIR_SUFFIX))
}

/// Write the generated scripts into a stable per-user support directory and
/// return it. `None` on any I/O failure (caller degrades). Rewritten on every
/// spawn so the scripts never go stale across app versions (cheap: two ~1 KiB
/// files, and an idempotent overwrite).
fn materialize(get_env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let dir = generated_zdotdir_path(get_env)?;
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

    // --- pty-session-env-isolation: re-derive, never inherit (D7/D9/D13) -----
    //
    // Everything below seeds a stub oracle and a cleared `CommandBuilder`. No
    // test reads or writes the process environment — `std::env::set_var` is
    // unsound under a threaded harness — so the results are identical inside and
    // outside a Claude session (R6.1).

    /// The file's `match k` stub oracle, made table-driven so each case reads as
    /// its requirement's *Given*.
    fn oracle(vars: Vec<(&'static str, String)>) -> impl Fn(&str) -> Option<String> {
        move |k| {
            vars.iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| v.clone())
        }
    }

    const HOME: &str = "/Users/tester";
    /// A user's real, *foreign* ZDOTDIR — legitimate configuration.
    const REAL_ZDOTDIR: &str = "/Users/tester/.config/zsh";
    /// The dir `write_dir` hands back on the success path.
    const WRITTEN_DIR: &str = "/tmp/mac-terminal-test-zdotdir";

    /// Our generated dir for [`HOME`] — what a nested parent's `ZDOTDIR` holds.
    fn our_dir() -> String {
        generated_zdotdir_path(&oracle(vec![("HOME", HOME.to_string())]))
            .expect("HOME is present in the stub")
            .to_string_lossy()
            .into_owned()
    }

    fn wrote_dir() -> Option<PathBuf> {
        Some(PathBuf::from(WRITTEN_DIR))
    }

    /// R3.6: the path helper is pure — it derives `$HOME` + the support suffix
    /// and touches the filesystem not at all, so the "is this ZDOTDIR ours?"
    /// predicate works on the path where `materialize` *fails*.
    #[test]
    fn generated_zdotdir_path_is_pure() {
        let home = std::env::temp_dir().join(format!("mt-pure-{}", std::process::id()));
        let env = oracle(vec![("HOME", home.to_string_lossy().into_owned())]);

        let got = generated_zdotdir_path(&env).expect("HOME is present");
        assert_eq!(got, home.join(SUPPORT_DIR_SUFFIX));
        assert!(!got.exists(), "helper must not create the generated dir");
        assert!(!home.exists(), "helper must not create anything at all");

        // No HOME (and an empty HOME) → None, which is what disables the path
        // guard in `inject_with` without disabling the name rule (R3.5).
        assert!(generated_zdotdir_path(&oracle(vec![])).is_none());
        assert!(generated_zdotdir_path(&oracle(vec![("HOME", String::new())])).is_none());
    }

    /// R3.6: `materialize` is that same helper plus the I/O — it writes both
    /// scripts at exactly the path the pure helper names.
    #[test]
    fn materialize_writes_at_the_pure_path() {
        let home = std::env::temp_dir().join(format!("mt-materialize-{}", std::process::id()));
        let env = oracle(vec![("HOME", home.to_string_lossy().into_owned())]);

        let want = generated_zdotdir_path(&env).expect("HOME is present");
        let got = materialize(&env).expect("writing under a temp HOME succeeds");

        assert_eq!(got, want, "materialize must land on the pure helper's path");
        assert_eq!(
            std::fs::read_to_string(got.join(".zshenv")).ok().as_deref(),
            Some(ZSHENV)
        );
        assert_eq!(
            std::fs::read_to_string(got.join(".zshrc")).ok().as_deref(),
            Some(ZSHRC)
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// R2.2: a user's real ZDOTDIR is configuration — it is captured as the
    /// chained source and it is never stripped from the child.
    #[test]
    fn inject_keeps_users_real_zdotdir() {
        let env = oracle(vec![
            ("ZDOTDIR", REAL_ZDOTDIR.to_string()),
            ("HOME", HOME.to_string()),
        ]);

        let mut cmd = clean_cmd();
        inject_with(&mut cmd, &env, wrote_dir);
        assert_eq!(
            cmd.get_env(USER_ZDOTDIR_VAR),
            Some(OsStr::new(REAL_ZDOTDIR))
        );

        // …and on the degrade path, where we re-set nothing, the inherited
        // foreign ZDOTDIR survives untouched rather than being stripped.
        let mut cmd = clean_cmd();
        cmd.env("ZDOTDIR", REAL_ZDOTDIR);
        inject_with(&mut cmd, &env, || None);
        assert_eq!(cmd.get_env("ZDOTDIR"), Some(OsStr::new(REAL_ZDOTDIR)));
    }

    /// R3.1: `MAC_TERMINAL_USER_ZDOTDIR` is removed unconditionally by name, on
    /// every one of the three exit paths — the stale value never reaches a child.
    #[test]
    fn inject_never_ships_an_inherited_user_marker() {
        // (a) degrade: nothing to chain from → early return, marker still gone.
        let mut cmd = clean_cmd();
        cmd.env(USER_ZDOTDIR_VAR, "/stale");
        inject_with(&mut cmd, &oracle(vec![]), wrote_dir);
        assert!(cmd.get_env(USER_ZDOTDIR_VAR).is_none());

        // (b) degrade: materialization failed → early return, marker still gone.
        let mut cmd = clean_cmd();
        cmd.env(USER_ZDOTDIR_VAR, "/stale");
        inject_with(&mut cmd, &oracle(vec![("HOME", HOME.to_string())]), || None);
        assert!(cmd.get_env(USER_ZDOTDIR_VAR).is_none());

        // (c) success: overwritten with the value we derived, not the stale one.
        let mut cmd = clean_cmd();
        cmd.env(USER_ZDOTDIR_VAR, "/stale");
        inject_with(
            &mut cmd,
            &oracle(vec![("HOME", HOME.to_string())]),
            wrote_dir,
        );
        assert_eq!(cmd.get_env(USER_ZDOTDIR_VAR), Some(OsStr::new(HOME)));
    }

    /// R3.2: nested launch — tier 1. The inherited marker is ground truth (we
    /// wrote it one level up), so nesting depth is irrelevant: neither our own
    /// generated dir nor `$HOME` is captured.
    #[test]
    fn inject_nested_captures_from_our_marker() {
        let mut cmd = clean_cmd();
        cmd.env("ZDOTDIR", our_dir());
        cmd.env(USER_ZDOTDIR_VAR, REAL_ZDOTDIR);
        inject_with(
            &mut cmd,
            &oracle(vec![
                (USER_ZDOTDIR_VAR, REAL_ZDOTDIR.to_string()),
                ("ZDOTDIR", our_dir()),
                ("HOME", HOME.to_string()),
            ]),
            wrote_dir,
        );
        assert_eq!(
            cmd.get_env(USER_ZDOTDIR_VAR),
            Some(OsStr::new(REAL_ZDOTDIR))
        );
        assert_eq!(cmd.get_env("ZDOTDIR"), Some(OsStr::new(WRITTEN_DIR)));
    }

    /// R3.7 (as far as the builder can show it): in the nested case the two vars
    /// name different directories, so `zshrc.zsh:9`'s
    /// `source "$MAC_TERMINAL_USER_ZDOTDIR/.zshrc"` reaches the user's real rc
    /// instead of re-sourcing our generated one. The actual sourcing is only
    /// observable in a live child.
    #[test]
    fn inject_nested_marker_never_points_at_our_own_dir() {
        let mut cmd = clean_cmd();
        inject_with(
            &mut cmd,
            &oracle(vec![
                (USER_ZDOTDIR_VAR, REAL_ZDOTDIR.to_string()),
                ("ZDOTDIR", our_dir()),
                ("HOME", HOME.to_string()),
            ]),
            wrote_dir,
        );
        let user = cmd.get_env(USER_ZDOTDIR_VAR).map(OsStr::to_os_string);
        assert_ne!(user.as_deref(), cmd.get_env("ZDOTDIR"));
        assert_ne!(user.as_deref(), Some(OsStr::new(our_dir().as_str())));
    }

    /// R3.3: tier 2's secondary guard — a hand-set `ZDOTDIR=<our dir>` with no
    /// companion marker is recognised by path and skipped, falling through to
    /// `$HOME`.
    #[test]
    fn inject_hand_set_zdotdir_at_our_dir_is_not_captured() {
        let mut cmd = clean_cmd();
        cmd.env("ZDOTDIR", our_dir());
        inject_with(
            &mut cmd,
            &oracle(vec![("ZDOTDIR", our_dir()), ("HOME", HOME.to_string())]),
            wrote_dir,
        );
        assert_eq!(cmd.get_env(USER_ZDOTDIR_VAR), Some(OsStr::new(HOME)));

        // The same guard must also strip it on the degrade path, where we re-set
        // nothing — otherwise our dir ships to the child as "the user's".
        let mut cmd = clean_cmd();
        cmd.env("ZDOTDIR", our_dir());
        inject_with(
            &mut cmd,
            &oracle(vec![("ZDOTDIR", our_dir()), ("HOME", HOME.to_string())]),
            || None,
        );
        assert!(cmd.get_env("ZDOTDIR").is_none());
    }

    /// R3.4: with no inherited marker the three-tier rule is byte-identical to
    /// the two-tier rule it replaces — `ZDOTDIR` if set, else `$HOME`.
    #[test]
    fn inject_non_nested_capture_matches_the_two_tier_rule() {
        // (requirement, inherited ZDOTDIR, inherited HOME, expected capture)
        let cases: [(&str, Option<&str>, Option<&str>, &str); 3] = [
            (
                "R3.4 foreign ZDOTDIR wins",
                Some(REAL_ZDOTDIR),
                Some(HOME),
                REAL_ZDOTDIR,
            ),
            ("R3.4 no ZDOTDIR → $HOME", None, Some(HOME), HOME),
            ("R3.4 empty ZDOTDIR → $HOME", Some(""), Some(HOME), HOME),
        ];
        for (label, zdotdir, home, want) in cases {
            let mut vars = Vec::new();
            if let Some(z) = zdotdir {
                vars.push(("ZDOTDIR", z.to_string()));
            }
            if let Some(h) = home {
                vars.push(("HOME", h.to_string()));
            }
            let mut cmd = clean_cmd();
            inject_with(&mut cmd, &oracle(vars), wrote_dir);
            assert_eq!(
                cmd.get_env(USER_ZDOTDIR_VAR),
                Some(OsStr::new(want)),
                "{label}"
            );
        }
    }

    /// R3.5: `HOME` unset → the pure helper returns `None`, so the *path* guard
    /// cannot fire — but the *name* rule still recognises the nested parent,
    /// still supplies the capture, and still leaves neither marker inherited on
    /// either exit path. An earlier draft got exactly this case wrong.
    #[test]
    fn inject_nested_without_home_still_re_derives() {
        let env = || {
            oracle(vec![
                (USER_ZDOTDIR_VAR, REAL_ZDOTDIR.to_string()),
                ("ZDOTDIR", our_dir()),
            ])
        };
        assert!(
            generated_zdotdir_path(&env()).is_none(),
            "no HOME → no path guard"
        );

        // Success path: tier 1 carries the capture with no HOME anywhere.
        let mut cmd = clean_cmd();
        cmd.env("ZDOTDIR", our_dir());
        cmd.env(USER_ZDOTDIR_VAR, REAL_ZDOTDIR);
        inject_with(&mut cmd, &env(), wrote_dir);
        assert_eq!(
            cmd.get_env(USER_ZDOTDIR_VAR),
            Some(OsStr::new(REAL_ZDOTDIR))
        );
        assert_eq!(cmd.get_env("ZDOTDIR"), Some(OsStr::new(WRITTEN_DIR)));

        // Degrade path (materialization fails): nothing is re-set, so both
        // markers must already be gone — this is what the removal-before-every-
        // early-return ordering buys.
        let mut cmd = clean_cmd();
        cmd.env("ZDOTDIR", our_dir());
        cmd.env(USER_ZDOTDIR_VAR, REAL_ZDOTDIR);
        inject_with(&mut cmd, &env(), || None);
        assert!(cmd.get_env("ZDOTDIR").is_none());
        assert!(cmd.get_env(USER_ZDOTDIR_VAR).is_none());
    }
}
