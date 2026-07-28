# loop.md — Loop Contract

> Supervisor document for **sticky-tab-title** (`specs/sticky-tab-title/spec.md`).
> This is a **post-implementation verification** contract: all six tasks (T1–T6) are
> already built. The loop starts by verifying, not by re-doing the work.

## Goal
Prove the shipped implementation satisfies the spec's `## Requirements` (R0–R5, 30 GWT
sub-requirements) without violating the project rules (`dependency-direction`,
`no-gui-crates`, `macos-only`, `zsh-local-only`) or the spec's `## Constraints`.

## Scope & non-goals
- **In:** the 12 files listed in the spec's task breakdown; test additions for behaviour
  already in scope; comment/doc corrections; lint/format fixes.
- **Out:** new features beyond R0–R5; changing the approved decisions D1–D13; anything
  that would require re-approving the spec.
- **Explicitly out of automated reach** (spec `## External Dependencies › Post-work` —
  these must NOT be marked passed by this loop):
  - **R2.6 end-to-end Hangul IME** — needs a human at the keyboard with a Korean input
    source via `/run-term`. The routing logic is unit-tested; the winit IME path is not.
  - **R3.1/R3.5 popup placement on screen** — hit-test correctness is unit-tested; that
    the popup *looks* right where it lands is not.

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| release build | `cargo build --release` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| format | `cargo fmt --all -- --check` |
| typecheck | N/A — `cargo build` is the type check for Rust (no separate command) |

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| tests passing | `≥ 617`, `0 failed` | `cargo test --workspace` summary lines — sum the `test result:` lines only, NEVER the `running N tests` lines (see L1) |
| clippy warnings | `= 0` | exit code of the `-D warnings` command |
| rustfmt drift | `= 0 files` | `cargo fmt --all -- --check` |
| sub-requirements with automated evidence | `≥ 25 of 30` | manual trace of each GWT to a test or code site. 5 are excluded **by name, not silently**: R2.6 (IME e2e) and R3.1/R3.5 (visual placement) are human-only; R0.3's shell-exit clause and R2.5's dead-session clause are unimplementable in this codebase (no child-exit detection exists) and are accepted as a documented gap |
| `term-core` GPU/window deps | `= 0` | `grep -rn "wgpu\|winit" crates/term-core/` must be empty (`dependency-direction`) |
| new GUI-toolkit deps | `= 0` | `git diff` on `Cargo.toml` files shows no egui/iced/druid/slint/gtk/gpui/dioxus/tao |

## Gate 3 — Qualitative  (score + grounds + action)

Scored by a **separate checker subagent** (maker ≠ checker), given only the diff, this
contract, and the spec.

- **Spec alignment** — does the code do what R0–R5 say, and are the two known deviations
  (R1.5 resolution site; R3.3 mapping landing in T2) honestly justified rather than
  quietly divergent?
- **Architecture fit** — does it honour `dependency-direction` (term-core GPU-free),
  SD2 (app performs no sidebar geometry), and D7 (renderer holds no label policy)?
- **Modal-flow coherence** — is D8a's "one modal at a time, nothing half-typed is ever
  committed, a chord dismisses AND acts" actually true on every path?
- **Naming / readability** — are the new seam types (`AppRequest`, `OverlayCommand`,
  `CardMenu`, `OpenMenu`) clear at their call sites?
- **Cursor semantics** (added for the cursor-movement fix) — is every edit operation
  expressed against the cursor position rather than the end of the string: insertion,
  Backspace vs Delete, Home/End, and the collapse of a selection to an edge? Are char
  boundaries respected (no byte-index slicing that can panic on 한글)?
- **Caret visibility** — when the name is longer than the card, does the visible window
  follow the CURSOR rather than always showing the tail?

```
<item>: <score>/10
  grounds: <specific, checkable observation — file/line, a measured fact>
  action: <change that raises it | "none — meets bar" + why>
```

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** clippy/rustfmt fixes, missing tests for behaviour
already in R0–R5, comment/doc corrections, local renames, behaviour-preserving refactors
that keep Gate 1 green.

**🛑 STOP — call a human (escalate):**
- Any change to the approved decisions D1–D13 or to R0–R5 themselves.
- Reworking where the label chain resolves (the R1.5 deviation) — that was a deliberate,
  reasoned choice; undoing it needs a human.
- Adding a GUI/widget crate, or making `term-core` depend on wgpu/winit.
- Cross-platform `#[cfg]` branches, shell selection, or SSH/remote transport.
- Marking R2.6 (IME) or R3.1/R3.5 (popup placement) as verified — they are human-only.

## Exit criteria
Every gate passes AND the Evidence Report is emitted, with the two human-only
sub-requirements reported as **open**, never as passed.

---

## Lessons  (self-learning — carried across runs of THIS contract)

### L1 — Test-count thresholds must be measured, not summed by eye
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `test -z "$(cargo test --workspace 2>&1 | grep -E '^test result' | awk -F'[ ;]' '{p+=$4; f+=$7} END {if (p>=617 && f==0) print ""; else print "FAIL"}')"`
- note: the ≥648 threshold was authored by summing `running N tests` lines INTO the `test result: N passed` lines, double-counting; the true count was 617. Sum only `test result:` lines.

### L2 — Prove the modal-exclusion invariant, don't assume it
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: "Modal exclusion: trace EVERY transition that opens a modal surface (rename field, card menu) and show `field ∧ menu` is unreachable by construction — not merely untested. Include mouse↔chord crossings in both directions."
- note: two reachable two-modal states shipped (hamburger pressed with the field open; Cmd+Shift+R/M pressed with a menu open) because only the keyboard paths were traced.
