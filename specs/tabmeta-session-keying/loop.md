# loop.md — Loop Contract: tabmeta-session-keying (verification)

## Goal

Verify that the **already-complete** implementation of `specs/tabmeta-session-keying/spec.md`
actually satisfies that spec's `## Requirements` — 9 requirements, 41 GWT sub-requirements —
and that it did not regress the ai-monitor panel's own identity path.

This is a **verification loop, not a build loop**. The five implementation tasks (T1–T5) are
done. Iteration 1's WORK phase is therefore a no-op: the loop starts at VERIFY. It may
auto-fix what the gates surface (formatting, a missing in-scope test, a doc correction), but
it must not re-derive or redesign the feature.

The spec is the bar. Where the implementation and the spec disagree, that is a **finding**,
not a licence to change either one — see the Autonomy Boundary.

## Scope & non-goals

- **In:** `crates/app/src/tabmeta/{activity,worker,agent,proc,mod}.rs`,
  `crates/app/src/{tabsession,waiting}.rs`, `crates/ai-monitor/src/lib.rs`, and this spec dir.
  Formatting, missing in-scope tests, comment/doc accuracy.
- **Out:** the resolution design itself (D1–D11 are approved and were adversarially reviewed
  three times); tier 3's ranking quality (an explicit spec non-goal); `term-core`; any new
  crate dependency; anything that would change `TabMeta`'s published shape.

---

## Gate 1 — Pass/Fail (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --all -- --check` |
| tests | `cargo test --workspace` |
| typecheck | N/A — in Rust, `cargo build` + `clippy` *are* the typecheck; there is no separate command |
| **migration complete** | `test $(grep -rn "state_of_cwd(\|label_of_cwd(\|fn state_of_cwd\|fn label_of_cwd" crates/ \| wc -l) -eq 0` |
| **panel path intact (R7.5)** | `cargo test -p app tabsession:: && cargo test -p app overlay::` |
| **the two regression tests exist and pass** | `cargo test -p app two_tabs_in_one_project_derive_independent_label_and_state a_fresh_session_does_not_inherit_its_neighbours_label -- --exact 2>&1 \| grep -q "2 passed"` |

> The "migration complete" check is deterministic proof that no caller still resolves by cwd.
> It greps for call/def sites only — a prose mention of the old name in a comment is fine and
> deliberately not matched.

## Gate 2 — Quantitative (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| app crate tests passing | `≥ 561` (pre-feature baseline: 532) | `cargo test -p app` |
| `tabmeta::activity` tests | `≥ 42` | `cargo test -p app tabmeta::activity` |
| panel tests passing | `≥ 89` (tabsession 26 + overlay 63) | `cargo test -p app tabsession:: / overlay::` |
| old cwd-keyed API sites in `crates/` | `= 0` | the Gate-1 grep above |
| new probe counters wired into `reset_all` | `= 2` (`PROC_START`, `SESSION_REG`) | grep `mod.rs` |
| resolution tiers with a dedicated test | `= 4` (1, 1b, 2, 3) | test-name audit of `activity.rs` |
| sub-requirements traced to a discharging test | `≥ 39 / 41` | the R→test matrix built in VERIFY |
| sub-requirements **openly recorded as gaps** | every one of the remaining `41 − traced` | the same matrix |

> The last two are a pair on purpose. The bar is not "41/41 tested" — it is **41/41
> accounted for**: traced to a test, or written down as a gap with its reason. A silent
> untested requirement fails this gate; an honestly-recorded one does not.

## Gate 3 — Qualitative (score + grounds + action; checker-scored by a separate subagent)

- **Spec alignment (D1–D11)** — does the code implement the decisions as written, especially
  D1's asymmetric miss-routing (tier 1 miss → `NamedButUnwritten`; tier 1b miss → ranking)?
- **R5.4 discharge** — the spec requires `PROC_START` asserted as a *floor* in a warm tier-1
  worker cost test. No such test exists (a unit test cannot spawn a live `claude` descendant).
  Is the substitute honest, or is this a gap that must be recorded?
- **R0.1–R0.3 seam** — the spec specifies these over a full two-tab `pass()`; they are
  implemented against `Deriver::activity`. Faithful discharge or a weakening?
- **R7.5 "unmodified"** — R7.5 says every pre-existing panel test passes *unmodified*. Two
  `tabsession` tests gained a `probe::serial()` guard line. Does that violate R7.5, or is it
  the necessary consequence of D6 that R7.5 failed to anticipate?
- **Test strength** — would the new tests actually fail if the original defect were
  reintroduced, or do they pass for incidental reasons?
- **Naming / readability at the call site** — `Found` / `Resolution` / `Naming` / `Detected`
  / `ClaudeId` as read from `worker.rs`.

```
<item>: <score>/10
  grounds: <specific, checkable observation — file/line, a measured fact, a comparison>
  action: <the concrete change that would raise it | "none — meets bar" + why>
```

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** `cargo fmt`; lint/type errors; adding a missing test for
behaviour already in scope; correcting a comment or doc that misstates what the code does;
local renames; behaviour-preserving refactors that keep every Gate-1 command green.

**🛑 STOP — call a human (escalate):**
- DB schema changes · data-loss-capable migrations · auth/permission changes ·
  payment/security-sensitive changes *(none are plausible in this feature; listed because the
  fence is unconditional)*
- **Changing the resolution design** — any edit to the tier order, the miss-routing, the memo
  keying, or the cadence. These are D1/D2/D3/D8, approved after three adversarial review
  passes.
- **Editing `spec.md`'s Requirements to match the code.** If a requirement is unmet, the
  answer is a finding or a new test — never a quietly relaxed requirement.
- **Weakening or deleting an existing assertion to make a gate pass.**
- Anything that changes `TabMeta`'s published shape, or adds a crate dependency.

## Exit criteria

Every gate passes AND the Evidence Report carries the full R→test matrix with each of the 41
sub-requirements either traced to a named test or recorded as an explicit gap. A 🛑 boundary
ends the loop in escalation, not completion.

---

## Lessons  (self-learning — carried across runs of THIS contract)

> Each lesson is a mistake this loop already made, hardened so it can't recur.
> Phase 0 loads `status: active` lessons. Scoped to THIS contract.

### L1 — A green suite is not evidence; mutate the fix and watch it die
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: "Mutation strength — for each decision this change implements, name the one-line mutation that reintroduces the defect, apply it, and record how many tests go red. A mutation that leaves the suite green means the requirement is untested, whatever the coverage count says."
- note: at contract time three defect-reintroducing mutations left all 565 tests green — hardcoding `start_ms: None` (makes tier 1 unreachable for every tab, i.e. the original bug restored), deleting `validated_session_id` (the recycled-pid guard), and building `ClaudeId` from the wrong pid. All three now kill 7/1/6 tests.

### L2 — A file assembled by the shell never passed the rustfmt hook
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `cargo fmt --all -- --check`
- note: iteration 1 failed on formatting alone because `activity.rs`'s test tail was built with `cat` in Bash; the project's PostToolUse rustfmt hook fires on Edit/Write, not on shell redirection.

### L3 — Never assert an absolute value on a probe counter another module can bump
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: "Counter assertions — every assertion on a `probe::` counter is a floor or a delta, unless the counter has exactly one bump site reachable only from code holding `probe::serial()`. If an absolute is genuinely needed, split the counted wrapper from the uncounted body so the foreign caller stops bumping it."
- note: `PROC_START` was bumped by `tabsession`'s tests, which hold no probe lock. Forcing guards into those tests violated R7.5; converting to a delta lost the ability to state "this tab performs NO read". The resolution was a counted wrapper over an uncounted body — and the intermediate state was a reproduced flake (`left: 2, right: 0` at `--test-threads=2`).

### L4 — An identity fake collapses the sources a mutation needs to separate
- cause: gate-fail
- source: human-approved
- form: context
- status: active
- check: —
- note: `with_fake_tree` used `agent_of = |pid| Detected { pid }`, making `tab_pid == root == walk_pid` one number, so rebuilding `ClaudeId` from the wrong source failed zero tests. A transform (`pid + 1_000`) separates them. The original comment argued for the identity *because* it was an identity — inverted reasoning, and the kind that reads as deliberate.
