---
name: insta-review
description: |
  Review and accept/reject insta snapshot diffs after changing the vte→grid
  pipeline or GridSnapshot output. Use when term-core snapshot tests fail with a
  snapshot mismatch. Triggers: "snapshot failed", "update snapshots", "/insta-review".
argument-hint: ""
allowed-tools:
  - Bash
  - Read
---

# /insta-review — review snapshot changes

The `term-core` snapshot tests (`crates/term-core/tests/grid_snapshot.rs`) pin the
`vte → grid` output. When you change parsing or grid semantics, they fail with a
diff; that is expected. Review the diff and decide if the new output is correct.

## Steps

1. Run the tests to produce pending snapshots:
   ```sh
   cargo test -p term-core --test grid_snapshot
   ```
2. Review each pending change. If `cargo-insta` is installed:
   ```sh
   cargo insta review     # interactive accept/reject
   ```
   Otherwise inspect the `*.snap.new` files under
   `crates/term-core/tests/snapshots/` and compare against the committed `.snap`.
3. **Verify the new output is actually correct** before accepting — read the test
   body and confirm the grid text matches what that input should produce. Do not
   blanket-accept.
4. Accept (if `cargo-insta` absent, rename `.snap.new` → `.snap`), then re-run
   `cargo test -p term-core` to confirm green.

## Install (once)

```sh
cargo install cargo-insta
```
