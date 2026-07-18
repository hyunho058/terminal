# Rule: zsh + local PTY only

**Constraint C5.** The terminal drives exactly one thing: a local `/bin/zsh` over
a PTY.

## Do

- Spawn `/bin/zsh` (see `term_core::pty::SHELL`).
- Keep all shell I/O going through the local `Pty`.

## Don't

- No SSH or remote sessions.
- No shell selection (bash, fish, …) or `$SHELL`-based dispatch.
- No network transport for the session stream.

These were named non-goals in the requirements interview. Supporting them later
is a re-scope, not a bug fix — surface it to the human first.
