# loop.md — Loop Contract: image-paste

- pre-approved-unattended: yes

## Goal
Support inline image display (Kitty protocol) AND clipboard-image → file-path paste, per `specs/image-paste/spec.md`, without violating it or the Shared Decisions. Extends scrollback's `parser.rs`/`snapshot.rs` as an ordered share.

## Scope & non-goals
- In: `crates/term-core/src/parser.rs`, `crates/term-core/src/image.rs`, `crates/term-core/src/snapshot.rs`, `crates/renderer/src/image_layer.rs`, `crates/app/src/clipboard.rs`
- Out: everything else. Registers via the SD3 `DrawLayer` + SD4 `InputHandler` seams — no edits to renderer `lib.rs` or app `terminal_app.rs`.

---

## Gate 1 — Pass/Fail

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| test failures | `== 0` | `cargo test --workspace` |
| parse/decode-failure tests | pass (malformed payload dropped, no panic) | `cargo test -p term-core` |
| pasted-file uniqueness test | pass (no overwrite on same-second pastes) | `cargo test -p app` (or unit) |

## Gate 3 — Qualitative
- **Architecture fit** — images are a `GridSnapshot` side channel (SD5), drawn via a registered `DrawLayer` (SD3); clipboard is a registered `InputHandler` (SD4); read via ordered extension of scrollback's files.
- **Spec alignment** — R0–R4 GWTs hold (inline display, path mode, decode-failure safety, unique filenames, mode coexistence).
- **Naming / readability** — the `ImageLayer` / placement model reads clearly.

---

## Autonomy Boundary
**✅ Auto-fix:** anything within the declared surface above.
**🛑 STOP — escalate (park):**
- Editing files outside the declared surface (esp. renderer `lib.rs`, app `terminal_app.rs`, scrollback's grid.rs).
- Any credential/security-sensitive path (e.g. writing outside the cache dir, executing pasted content).
- Anything conflicting with this Goal or `spec.md`.

## Exit criteria
Every gate passes and the Evidence Report is emitted. A 🛑 boundary ends the loop in escalation.
