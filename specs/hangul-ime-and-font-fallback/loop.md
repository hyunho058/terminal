# loop.md — Loop Contract: hangul-ime-and-font-fallback

> Post-execution **verification** gate for an implementation that is already complete
> (8 tasks, 5 waves, landed on `fix/hangul-ime-and-font-fallback`). Iteration 1's WORK
> step is a deliberate no-op so VERIFY is effectively the first action. Code changes
> only to fix defects that verification actually surfaces.
- spec: specs/hangul-ime-and-font-fallback/spec.md

## Goal
Prove the landed implementation satisfies all 10 requirements / 59 GWT sub-requirements of
`specs/hangul-ime-and-font-fallback/spec.md` — Hangul typing (a) composes into syllables that reach
the correct shell and (b) renders as real Hangul glyphs — without violating the spec's `## Constraints`
or the subtle correctness rules in D1–D19. A green build is **not** accepted as proof on its own.

## Scope & non-goals
- **In:** verification of `crates/term-core/src/pty.rs`, `crates/renderer/src/{font_set,atlas,glyph,lib,tab_bar,overlay_layer,preedit_layer}.rs`,
  `crates/app/src/{terminal_app,input,settings}.rs`, `crates/renderer/Cargo.toml`; and auto-fix of
  defects those checks surface (missing tests, lint, docs, local corrections).
- **Out:** re-doing completed work; new features beyond the spec; the spec's declared Non-goals
  (colour emoji, JP/CN font tuning, font-config UI, CoreText cascade); anything in the spec's
  `## Known Gaps` (notably the *intra-frame* atlas evict UV invalidation — out of scope by decision).

---

## Gate 1 — Pass/Fail  (100% required, binary)

| Check | Command |
|-------|---------|
| build | `cargo build` |
| typecheck | N/A — Rust; `cargo build` type-checks |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| system-font tests | `cargo test --workspace -- --ignored` |

> The `--ignored` run is a **required** Gate-1 member here, not an optional extra: R1.1, R1.3, R2.1
> and R2.2 are only provable against the real `AppleSDGothicNeo.ttc` / `Menlo.ttc`, and the project's
> convention parks those behind `#[ignore = "depends on a system font path"]`.

## Gate 2 — Quantitative  (measured vs threshold)

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| workspace tests failed | `== 0` | `cargo test --workspace` |
| system-font (`--ignored`) tests failed | `== 0` | `cargo test --workspace -- --ignored` |
| clippy warnings | `== 0` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `.unwrap()` outside tests **added** by this change | `== 0` | per-file pre-`#[cfg(test)]` count vs `git show HEAD:<f>` baseline |
| `#[cfg(target_os` occurrences in `crates/` | `== 0` | `grep -rn 'cfg(target_os' crates/ \| wc -l` |
| `term-core` non-comment refs to wgpu/winit | `== 0` | `grep -rn 'wgpu\|winit' crates/term-core/src crates/term-core/Cargo.toml` (doc comments excluded) |
| `renderer` non-comment refs to winit | `== 0` | `grep -rn '\bwinit\b' crates/renderer/src crates/renderer/Cargo.toml` (doc comments excluded) |
| new `unsafe` blocks carrying a `// SAFETY:` comment | `2 / 2` | `grep -rn unsafe crates/*/src/*.rs` — expect exactly `font_set.rs` (mmap) + `terminal_app.rs` (NSView cast) |
| sub-requirements with traced evidence (test or code site) | `== 59 / 59` | maker-run trace of `## Requirements` R0–R9 to file:line |
| `unicode-width` version parity term-core ↔ renderer | identical | `grep unicode-width crates/*/Cargo.toml` |

## Gate 3 — Qualitative  (score + grounds + action — scored by a SEPARATE checker subagent)

Targets the rules that adversarial review already caught **once each** during spec derivation. A
passing automated suite does not establish any of these; each needs a read of the actual code.

- **D11 — commit routing & cancellation** — `composing_tab` routing sends a commit to the tab where
  composition *began*, never the newly-active tab; a closed composing tab drops the bytes; `None`
  routes to active so **no character is ever lost**. Two rejected designs must NOT be present: the
  `set_ime_allowed(false)→(true)` toggle (does not discard OS marked text) and a one-shot
  `discard_next_commit` guard (winit queues `Preedit("")` before every `Commit`, so it eats a real char).
- **D17 — generation invalidation** — `draw_to` compares `atlas.generation()` across the frame and
  clears `last_snapshot`, genuinely closing the *cross-frame permanent* stale-UV path that D8 makes
  structurally reachable (snapshot static during composition while the preedit layer rasterizes).
- **D5 — degraded-mode branch** — degraded judgement lives on `Atlas` (not `FontSet`) so all four text
  paths (grid, tab bar, overlay, preedit) reach it; the `id == 0` pack guard is conditional on
  `!is_degraded()`, so a total fallback-load failure still shows `.notdef` rather than invisible Hangul.
- **D6 / R0.3 — Latin regression is zero** — face 0 yields offset exactly `0.0`, and the `max(0.0)`
  clamp means an oversized fallback glyph overflows right only, never into the previous cell.
- **D16 — run indexing basis** — `faces`/`Run`/`char_col` are indexed over the *continuation-skipped*
  char sequence, and per-run `byte_col` resolves `cluster.source.start` to the correct absolute column.
- **Requirement coverage** — every one of R0–R9's 59 sub-requirements is actually satisfied by the
  code, not merely by a similarly-named test.
- **Architecture fit** — dependency direction `common ← term-core ← renderer ← app` intact;
  `term-core` GPU/window-free; preedit never enters the grid; no GUI crate.

```
<item>: <score>/10
  grounds: <specific, checkable observation — file/line, measured fact, comparison>
  action: <change that raises it | "none — meets bar" + why>
```

---

## Autonomy Boundary

**✅ Auto-fix (stay in the loop):** lint/format/type errors; missing tests for behaviour already in
scope; doc and comment corrections; local renames; behaviour-preserving refactors that keep every
Gate-1 command green; correcting an implementation detail that contradicts an approved decision.

**🛑 STOP — call a human (escalate):**
- Any change to `crates/term-core/**` beyond `pty.rs` (the spec's Non-goals carve out exactly that one file).
- Breaking the dependency direction, or adding a GUI/widget crate.
- A defect whose fix would **conflict with an approved decision (D1–D19)** or require amending the spec.
- Reworking a rejected design back in (the `set_ime_allowed` toggle or the `discard_next_commit` guard).
- Anything on the standard fence: DB schema, data-loss migration, auth/permission, payment/security.
- insta snapshot changes → `/insta-review`, never blind-accept.
- No fail→pass progress across two iterations (anti-spin).

## Exit criteria
Exits only when every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in
escalation, not completion. Verification is never reported as passed when it did not run.

---

## Lessons  (self-learning — carried across runs of THIS contract)

### L1 — winit queues an empty `Preedit` before every `Commit`
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: Any design that keys IME state *release* on the `Ime` event stream MUST be validated against
  winit's pre-commit empty `Preedit` (`platform_impl/macos/view.rs:412-413`, which queues
  `Ime::Preedit("")` immediately before every `Ime::Commit`). Trace the real event order end to end and
  prove the state the `Commit` arm reads is the state you intended — not just that a pure function
  returns the right value in isolation.
- note: This trap was hit TWICE on this feature — it is why `discard_next_commit` was rejected during
  spec review, and then the `composing_tab` routing that replaced it was silently inert in production
  for the same reason (owner always `None` at commit time, `CommitTarget::Tab`/`Drop` dead code).
  Pinned by `terminal_app::tests::ownership_survives_the_pre_commit_preedit`.
