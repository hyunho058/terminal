# loop.md — Loop Contract: ai-monitor-antigravity (verification)

## Goal

Verify that the **already-complete** implementation of `specs/ai-monitor-antigravity/spec.md`
satisfies that spec's `## Requirements` — 27 GWT sub-requirements across R0–R5 — and that it
did not regress the Claude path of the ai-monitor panel.

This is a **verification loop, not a build loop**. All six implementation tasks (T1–T6) are
done. Iteration 1's WORK phase is therefore a no-op: the loop starts at VERIFY. It may
auto-fix what the gates surface (formatting, a missing in-scope test, a comment that misstates
the code), but it must not re-derive or redesign the feature.

The spec is the bar. Where the implementation and the spec disagree, that is a **finding**,
not a licence to change either one — see the Autonomy Boundary.

## Scope & non-goals

- **In:** `crates/ai-monitor/src/{lib,telemetry,antigravity}.rs`, `crates/ai-monitor/tests/`,
  `crates/app/src/{overlay,tabsession,dashboard,presence_fd}.rs`,
  `crates/app/src/tabmeta/mod.rs`, and this spec dir. Formatting, missing in-scope tests,
  comment/doc accuracy.
- **Out:** the twelve approved decisions AG0–AG11 themselves; the Claude resolution design
  (`resolve_owned_session`, `Wanted`, rivals) which this feature deliberately did not touch;
  the declared gaps G1–G8; `term-core`; `renderer`'s layout; any new crate dependency.

---

## Gate 1 — Pass/Fail (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| format | `cargo fmt --all -- --check` |
| tests | `cargo test --workspace` |
| typecheck | N/A — in Rust `cargo build` + `clippy` *are* the typecheck; no separate command |
| **`ai-monitor` stays headless (AG6/AG11, dependency-direction rule)** | `test $(sed -n '/\[dependencies\]/,/^\[/p' crates/ai-monitor/Cargo.toml \| grep -cE '^(libc\|wgpu\|winit\|rusqlite\|sqlite)') -eq 0` |
| **no SQLite crept back in (AG6)** | `test $(grep -rniE 'rusqlite\|sqlite\|conversation_summaries' crates/ --include="*.rs" \| grep -vE ':[[:space:]]*(//\|\*)' \| wc -l) -eq 0` — and `test $(grep -rniE 'rusqlite\|sqlite' crates/*/Cargo.toml Cargo.toml \| wc -l) -eq 0` |

> **Command corrected during run 1, intent unchanged.** The first spelling omitted the
> comment filter and the quotes around `*.rs`. Unquoted, zsh failed to glob it and the grep
> never ran — the check reported a *false PASS*. Quoted, it then caught its own
> documentation: `antigravity.rs:189` is the comment explaining **why**
> `conversation_summaries.db` was rejected, which AG6 wants kept. The bar is "no SQLite
> dependency and no SQLite usage" and that is what the corrected pair tests (measured: 0
> non-comment hits, 0 manifest hits). Narrowing a check that flagged its own rationale is not
> lowering the bar; leaving a check that cannot run would have been.
| **read-only under `~/.gemini` (Constraints)** | `test $(grep -nE 'fs::(write\|create\|remove\|OpenOptions)' crates/ai-monitor/src/antigravity.rs \| wc -l) -eq 0` |
| **no recency tiebreak across `brain/` (AG6/R1.4)** | `cargo test -p ai-monitor --test antigravity 2>&1 \| grep -q "test result: ok"` |
| **R5.6 — the nine pre-existing panel snapshots are byte-identical** | `test $(git diff --name-only HEAD -- crates/app/src/snapshots/ \| wc -l) -eq 0` |
| **the Antigravity panel form is pinned** | `cargo test --bin app the_antigravity_panel_matches_its_pinned_form 2>&1 \| grep -q "1 passed"` |
| **the Claude fold is untouched by format dispatch (R2.8)** | `cargo test -p ai-monitor --test telemetry 2>&1 \| grep -q "test result: ok"` |

## Gate 2 — Quantitative (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| workspace tests passing | `≥ 1252` | `cargo test --workspace`, summed |
| `ai-monitor` manifest dependencies | `= 2` (`serde_json`, `notify`) | `crates/ai-monitor/Cargo.toml` |
| pre-existing panel snapshots changed | `= 0` of 9 | `git diff --name-only HEAD -- crates/app/src/snapshots/` |
| new pinned snapshots added | `= 1` (the Antigravity panel) | `git status --short crates/app/src/snapshots/` |
| sub-requirements traced to a discharging test | `≥ 24 / 27` | the R→test matrix built in VERIFY |
| sub-requirements **openly recorded as gaps** | every one of the remaining `27 − traced` | the same matrix |
| approved decisions AG0–AG11 traced to code | `≥ 11 / 12` | decision→symbol audit in VERIFY |

> The last two pairs are deliberate. The bar is not "27/27 tested" — it is **27/27 accounted
> for**: traced to a test, or written down as a gap with its reason. A silent untested
> requirement fails this gate; an honestly-recorded one does not. R0.1 is the known case (it
> needs two live tabs and real processes), and it must appear as a *recorded* gap, not as a
> claimed pass.

## Gate 3 — Qualitative (score + grounds + action; checker-scored by a separate subagent)

- **Spec alignment (AG0–AG11)** — does the code implement the decisions as written, especially
  AG2's dispatch↔result correlation (the largest interpretive call in the feature) and AG6's
  two-tier ladder with its deliberately absent third tier?
- **`is_the_one_log` correctness** — under a layout where every transcript is named
  `transcript.jsonl` inside a directory named `logs`, does the predicate actually exclude a
  sibling conversation, the chunk mirrors, and `transcript_full.jsonl`, and admit the
  canonical (`/private/var/…`) spelling FSEvents delivers?
- **Test strength** — would these tests fail if the defect they guard were reintroduced, or do
  they pass for incidental reasons? Specifically: the R1.4 subagent test, the R2.2
  count-once test, and the R5.5 `gauge=none` assertion.
- **R0.1 honesty** — the end-to-end two-tab scenario was never run against live processes.
  Is that recorded as a gap in the right place, or is it papered over by unit tests that
  cover only its pieces?
- **Claude-path neutrality (R5.6)** — beyond the snapshots being byte-identical, is there any
  behavioural change to a Claude tab from `WatchScope`, `tab_agent`, or the `agent_for`/
  `hint_for` fallback?
- **Naming / readability at the call site** — `WatchScope`, `SingleLog`, `is_the_one_log`,
  `is_monitored`, `reports_usage`, `provider_tag`, `antigravity_pid` as read in context.

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
  payment/security-sensitive changes *(none are plausible here; listed because the fence is
  unconditional)*
- **Changing any approved decision AG0–AG11** — in particular AG6's ladder (adding the
  rejected SQLite or mtime tier), AG2's counting event, or AG9's format-at-construction.
- **Editing `spec.md`'s Requirements to match the code.** If a requirement is unmet, the
  answer is a finding or a new test — never a quietly relaxed requirement.
- **Weakening or deleting an existing assertion to make a gate pass** — including re-pinning
  any of the nine Claude snapshots.
- **Writing anything under `~/.gemini/antigravity-cli/`**, or adding a crate dependency to
  `ai-monitor`.
- Closing a declared gap (G1–G8) by implementing it — that is new scope, not verification.

---

## Lessons

### L1 — a Gate-1 command that cannot run reports a false PASS
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `test $(grep -rl "fn main" crates/ --include="*.rs" | wc -l) -ge 1`
- note: run 1's SQLite check used an unquoted `--include=*.rs`; zsh does not glob it, the
  grep never ran, `wc -l` counted an empty stream and the gate reported PASS. This is a
  positive control on the grep machinery itself — it must find a pattern that always exists,
  so a broken glob fails loudly instead of passing silently. Verified both ways at record
  time: quoted finds 1, unquoted finds 0.

### L2 — pin an ordering rule by the observable that distinguishes it, not by counts
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `cargo test -p ai-monitor --test telemetry an_async_result_claims_the_oldest_pending_call_not_the_nearest 2>&1 | grep -q "1 passed"`
- note: mutating `.position(` → `.rposition(` in the dispatch↔result join — inverting AG2's
  oldest-pending rule to the nearest-preceding one the spec rejects on measurement — left all
  1252 tests green, including the test named for that rule. Counts, names and ring lengths
  are identical under both orders; only `duration_ms` differs. A rule is only pinned by the
  quantity that separates it from its alternative.

### L3 — a comment asserting a safety property needs a test or a proof
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: Every comment in the diff that asserts a *guarantee* ("X can never happen", "this is
  a floor", "that function would produce Y") — is it actually true of the code as written?
  Name each such claim and how you checked it.
- note: run 1 shipped two false guarantees. `.or_else(..)` was documented as a floor that
  stopped a Claude tab losing its wording, but `or_else` only fires on `None`, so an
  unmonitored agent nearer the root displaced a confirmed Claude — a real R5.6 regression
  described as impossible. Separately, a comment claimed `context_tile` "would compute a
  real-looking 0%", when `context_pct` returns `None` on its first line for a zero numerator.
  Both took under a minute to disprove and neither was caught by any gate.
