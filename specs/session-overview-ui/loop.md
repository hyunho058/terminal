# Loop contract: session-overview-ui (post-execution verification)

- **Goal**: verify the implementation of `specs/session-overview-ui/spec.md` against its
  `## Requirements` (R0–R10, 65 GWTs). Implementation is complete; verify first, fix only
  within the Auto-fix boundary.
- **Approved by**: user, 2026-10-31 (interactive, "Approve")
- **Protected files**: the 24 acceptance-test files whose hashes are in
  `/private/tmp/claude-501/-Users-hyunhokim-IdeaProjects-terminal-feat-changing-session-ui/e0f934eb-020b-4c3e-88bf-1873972491af/scratchpad/tt-hashes.txt`
  — never modified by the loop.

## Iteration 2 scope (user, 2026-10-31)
After a manual run the user reported defects; approved fixes D24 (Antigravity digest), D25
(human-prompt label), D26 (single quotes) — spec R11. Deferred: the unanswered
task-notification → Working classifier gap (spec Known Gaps). Gates below are unchanged; Gate 2's
GWT coverage now spans R1–R11, and new `tt11_*` acceptance files join the protected hash list.

## Gate 1 — Pass/Fail (100%)
1. `cargo build --workspace`
2. `cargo fmt --all --check`
3. `cargo clippy --workspace --all-targets -- -D warnings`
4. `cargo test --workspace --no-fail-fast -- --skip test_live_session_29b623ae_file_if_present`
   (the skipped test reads a live session log on this machine and fails on the untouched
   baseline too — recorded 2026-10-31, before any change)
5. `shasum -a 256 -c <tt-hashes.txt>` — every line OK

## Gate 2 — Quantitative
| metric | threshold |
|---|---|
| automatable GWTs (all but R0.1, R0.2, R1.6's tests/-level form) mapped to a passing test | 100% |
| `cargo mutants -f <file> -j 4` unexplained survivors, for `crates/app/src/overview.rs`, `crates/app/src/overview_detail.rs`, `crates/renderer/src/overview_layout.rs`, `crates/app/src/prompt.rs` | 0 (a survivor recorded as provably equivalent does not count) |

## Gate 3 — Qualitative (checker-scored, score + grounds + action, bar 7/10)
1. **Spec alignment** — each requirement's GWTs are met by code and test as written.
2. **Design fidelity** — colours, strings and sizes match the design boards (D15, D21).
3. **Constraints held** — no GUI crates; `term-core` untouched; renderer app-type-free;
   nothing new runs while the overlay is closed beyond D9's stated exception; no invented
   values (absent data reads `—`).
4. **Docs accuracy** — `docs/domain.md` rows and `docs/architecture.md` G8/G10 match the code.

## Manual (outside the gates, reported not claimed)
- R0.1, R0.2 — eyeball in the running app (`/run-term`).

## Lessons
