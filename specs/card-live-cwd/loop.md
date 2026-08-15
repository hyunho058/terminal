# Loop Contract — card-live-cwd verification

- **Goal**: verify the completed implementation against `specs/card-live-cwd/spec.md`
  (`## Requirements` R0–R5, 20 GWT sub-requirements, and `## Constraints`).
- **Approved by**: user
- **Approved at**: 2026-08-13
- **Mode**: interactive (no `pre-approved-unattended`)
- **Entry state**: implementation already complete (agent-orchestrate Phase 3.5) — iteration 1
  performs no work; VERIFY is the first real action.

## Gate 1 — Pass/Fail (binary, 100%)

| # | check | command |
|---|-------|---------|
| 1 | build | `cargo build --workspace` |
| 2 | fmt | `cargo fmt --all -- --check` |
| 3 | lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| 4 | tests | `cargo test --workspace` |

## Gate 2 — Quantitative

| metric | threshold |
|--------|-----------|
| sub-requirements traced to code or test | 22/22 |
| app crate tests | ≥ 583 (no deletions) |
| renderer crate tests | ≥ 277 |
| term-core crate tests | ≥ 204 |
| tests added by this work | ≥ 17 |

> **Two thresholds were corrected at first VERIFY, before any gate was scored.** The draft
> said "20/20 sub-requirements" — the spec has **22** (`grep -c '^#### R'`); and it said
> "term-core ≥ 69", which misattributed another crate's total. Both corrections raise or
> repair the bar rather than lowering it, which Rule 1 forbids; recorded here so the change
> is auditable.
>
> **Measurement method** (the crate totals are otherwise ambiguous — `cargo test -p <crate>`
> spans several test binaries plus doc-tests, so a single `test result` line under-counts):
> `cargo test -p <crate> | grep -E '^test result' | awk '{p+=$4} END {print p}'`.
> term-core measures **204 passed / 0 failed across 8 result lines** and is untouched by this
> work — the threshold exists to catch a regression, not to pin an exact split.
| `probe::PROC_CWD` after the row join (R5.1) | exactly 0 |
| worker `Cmd`s sent on the metadata path (R5.2) | exactly 0 |
| folder-name derivation sites (D7) | exactly 1 |

## Gate 3 — Qualitative (checker subagent — maker ≠ checker)

- **Spec alignment** — R0–R5 satisfied as written, not approximately
- **R3.2 additive band gate** — both arms load-bearing, neither displaced by the other
- **R1.5 discipline** — unreachable fallback commented, and no test asserting it
- **D7 held** — no new duplication of the folder-name derivation
- **Idiom fit** — comments and naming consistent with the surrounding code

## Out of scope for these gates

The spec's `## External Dependencies › Post-work` items are outstanding by design and are
**not** gate failures:
1. amending `specs/tmux-pane-cwd/spec.md`'s false "self-correcting on the next tick" gap;
2. the `/run-term` eyeball check (no automated test covers the on-screen result).

## Lessons

### L1 — Comment claims must be traceable in the code
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: Every comment this change adds or edits asserts only mechanisms traceable in the
  code. Sweep them and flag any claim you can falsify.
- note: Fired twice in the first run — a comment inventing a borrow-order constraint that did
  not exist, and one justifying a `band_gen` asymmetry with a skip the generation/data
  coupling makes impossible. This project's defining bug is a doc that called a frozen copy
  live, so a confident-sounding false comment is a defect here, not a nit.

### L2 — No orphaned doc comment or section banner
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: No doc comment or test-section banner documents an item other than the one that
  immediately follows it.
- note: Fired twice in the first run — two new functions were spliced between
  `waiting_signal`'s doc and `waiting_signal`, silently re-attaching the doc to the wrong
  function; and an R4.1/R4.2 test banner ended up labelling zero tests after its two tests
  drifted 175 lines away. Both compile clean, so only a reader (or this gate) catches them.
