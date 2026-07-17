# Rule: macOS ARM64 only

**Constraint C3.** This is a macOS-only (Apple Silicon) terminal. Portability is
an explicit non-goal.

## Do

- Target `aarch64-apple-darwin`. Assume Metal is available.
- Use macOS-native APIs freely via `objc2` when they simplify the code.

## Don't

- Do not add `#[cfg(target_os = "...")]` branches for Windows/Linux.
- Do not add cross-platform abstraction layers "just in case".
- Do not pull dependencies solely to support other platforms.

Keeping one target removes a whole class of conditional complexity. If
cross-platform ever becomes a goal, that is a deliberate re-scope, not a drift.
