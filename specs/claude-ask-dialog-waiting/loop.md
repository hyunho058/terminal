# loop.md — Loop Contract

> The supervisor document for `claude-ask-dialog-waiting`. Approved by the user 2026-10-04.

## Goal
Verify that the implementation of `specs/claude-ask-dialog-waiting/spec.md` meets that spec:
a Claude tab blocked on a question dialog (single, multi-question, review) or on a plan
approval reads "Waiting for input" and returns to "Working" once answered, and every rule
that shipped is pinned by verbatim real captures. The code must not violate the spec, its
amended D3/D5/D9, or the project rules in CLAUDE.md.

## Scope & non-goals
- In: `crates/app/src/prompt.rs`, the tests in `crates/app/src/tabs.rs` and
  `crates/app/src/tabmeta/activity.rs`, the doc comments in `crates/ai-monitor/src/activity.rs`
  and `crates/app/src/tabmeta/mod.rs`, `docs/architecture.md` G8, and `specs/claude-ask-dialog-waiting/**`.
- Out: `crates/term-core`, the renderer, `CardStatus`, `effective_state`, `TAIL_ROWS`, the log
  classifiers' behaviour, `Cargo.lock`, and dependencies. Do not commit.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| typecheck | N/A — part of `cargo build` / `cargo clippy` in Rust |
| format | `cargo fmt --all -- --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| declared surface | `git diff --quiet HEAD -- crates/term-core Cargo.lock '*Cargo.toml'` |
| fixtures verbatim (R1.4) | `python3 specs/claude-ask-dialog-waiting/captures/check_fixtures.py` |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| mutation survivors in `prompt.rs` (R8.2) | `0`, or each argued equivalent in the spec | `cargo mutants -f crates/app/src/prompt.rs -j 4 --cargo-test-arg -p --cargo-test-arg app` |
| fixtures byte-equal | `16/16` | `check_fixtures.py` summary line |
| mandatory captures present (D9 exit) | `7/7` | C1, C2, C5, C6, C9, C10, C11 files in `captures/` |
| `❯ 1.` rows in C6's tail (R1.3) | `0` | `grep -c '❯ 1\.' captures/c6_ask_tall.txt` |

## Gate 3 — Qualitative  (score + grounds + action; pass bar ≥ 7/10 each)
Scored by a separate checker subagent (maker ≠ checker).

- **Spec alignment**: R0–R8 each have a test or a checkable artefact. Each amendment (D3,
  D5, D9) is justified by a named capture, and nothing contradicts a non-amended decision.
- **False-positive safety**: the conservative bias holds. Quoted footers, pickers, pooled
  fragments, split panes and scrolled-away dialogs do not read as waiting, and the level clears.
- **Architecture fit**: all policy is in `prompt.rs` and nothing changes in `term-core`. The
  detector is a level re-evaluated per chunk, with no latch. G1 holds: a few line
  predicates per scan.
- **Test strength**: tests would fail if a rule broke. Negatives are built by editing real
  fixtures, with the edit asserted to have landed. The card tests reach `resolve_status`.
- **Docs accuracy**: G8, the doc comments and `captures/README.md` match the code and the
  captures.

```
<item>: <score>/10
  grounds: <specific, checkable observation>
  action: <change that raises it | "none — meets bar" + why>
```

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/type/format, missing in-scope tests,
docs/comments, local renames, behaviour-preserving refactors.

**🛑 STOP — call a human (escalate):**
- DB schema changes
- Data-loss-capable migrations
- Auth / permission / access-control policy changes
- Payment / security-sensitive changes (secrets, crypto, billing)
- Anything conflicting with this contract's Goal or the spec. This **includes changing what
  a matcher rule accepts beyond what the spec plus its captures say**.

## Exit criteria
The loop exits only when **every** gate above passes and the Evidence Report is
emitted. Hitting a 🛑 boundary ends the loop in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)

> Each lesson is a mistake this loop already made, hardened so it can't recur.
> Phase 0 loads `status: active` lessons: `gate1-command` / `gate3-item` forms are
> folded into the gates above; `context` forms are read as guidance. Lessons are
> scoped to THIS `loop.md` — they do not propagate to other contracts. The human
> owns this section (curation); see SKILL.md "Self-Learning Lessons" + Phase 5.

<!-- Append entries below. A fresh contract has none. -->
