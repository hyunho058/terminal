# loop.md — Loop Contract: housekeeping

> Supervisor document for the housekeeping chore. Approved in one batch by
> /build-order (post-pr2-backlog). Spec: specs/housekeeping/spec.md.

- pre-approved-unattended: yes

## Goal
Prune dead branches merge-safely by content, and refresh the stale
deferred/known-gaps lists in `docs/` + `CLAUDE.md` so shipped work is no longer
listed as deferred. Must not violate specs/housekeeping/spec.md (D5–D7, R0–R5).

## Scope & non-goals
- In: `docs/**`, `CLAUDE.md`, and local/remote git branch state for the 3 named
  dead branches.
- Out: `crates/**` (zero code change), unrelated doc prose, `main`, the active
  branch, and any branch with unmerged content.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` (regression guard — docs edits must not break the build) |
| typecheck | N/A — docs/branches chore |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` (regression guard) |
| format | `cargo fmt --check` |
| tests | `cargo test --workspace` (regression guard) |
| docs compile | N/A — Markdown / CLAUDE.md have no build step |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| — | N/A | Docs/branches chore has no quantitative metric; correctness is the Gate-3 verify-before-strike + content-equivalence checks |

## Gate 3 — Qualitative  (score + grounds + action)

- **Verify-before-strike (D5)** — every deferred item removed is first confirmed shipped (a `specs/<feature>/` dir + wired code); "256-color/truecolor SGR" struck ONLY if `handle_sgr` now carries the `38;5`/`38;2` arms; any unverified item stays listed (R2, R3, R5).
- **Branch merge-safety (D6)** — deletion gated on content-equivalence (`git diff main..<b>` empty AND no unique files), not reachability; `git branch -D` used only after that gate passes; `build/mac-terminal-v1` (unique content) is NOT deleted (R1).
- **Tight diff (D7)** — only the deferred/known-gaps lists + the `CLAUDE.md` block-font note change; surrounding prose untouched.
- **Accuracy** — no doc asserts a behavior the code lacks (esp. the 256/truecolor strike vs the actual parser state).

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** the doc edits (`docs/**`, `CLAUDE.md`), deleting
a branch that PASSES the content-equivalence gate (its content is fully in `main`),
lint/fmt.

**🛑 STOP — call a human (escalate):**
- Deleting a branch that FAILS the content gate — `build/mac-terminal-v1` carries
  unique unmerged content (`autopilot.md`, divergent `build_order.md` /
  `specs/glyph-rendering/loop.md`): surface it, do NOT delete (D6, "never push
  through the fence"). Branch deletion is irreversible — this is a hard boundary.
- Any deferred item that cannot be verified shipped (do not strike on assumption).
- Editing `crates/**` (out of surface) or `main` / the active branch.
- Anything conflicting with this contract's Goal or specs/housekeeping/spec.md.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary (expected on the
`build/mac-terminal-v1` deletion) ends the loop in escalation, not completion — the
docs refresh + the two content-safe branch deletions still land; the one unsafe
branch is surfaced.

---

## Lessons  (self-learning — carried across runs of THIS contract)

<!-- A fresh contract has none. -->
