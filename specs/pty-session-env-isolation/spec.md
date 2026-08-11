# Spec: pty-session-env-isolation

## Meta
- **Created**: 2026-08-10
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-08-10
- **Complexity**: Moderate (raised from Simple by the L2-reviewer) — 4 signals:
  ① a multi-actor inheritance chain (outer claude → app → tab zsh → tab claude) that only
  misbehaves in composition; ② a change split across **two modules** with a load-bearing
  ordering constraint between them (`pty.rs` + `shell_integration.rs`, D12/D13);
  ③ a **second defect family** folded in mid-derivation (D7's ZDOTDIR self-nesting, which
  spawned D9/D12/D13); ④ reliance on an **undocumented third-party contract**
  (`as_command`'s `env_clear`-then-reapply is `pub(crate)`, D5).

## Goal

`Pty::spawn` hands the launching agent's session identity to every tab. When the app is
itself launched from inside a Claude session (`/run-term`), the tab's zsh — and every
`claude` started in it — inherits `CLAUDE_CODE_CHILD_SESSION=1`, `CLAUDE_PID`,
`CLAUDE_CODE_SESSION_ID` and `CLAUDE_CODE_MESSAGING_SOCKET` from a process it has no
relationship to. The tab's claude reads the child-session marker, suppresses its own
transcript, and never writes `~/.claude/projects/**/*.jsonl` — the only file the
ai-monitor panel reads. The panel's "this tab's session has not written a log yet" is
literally accurate: there is nothing to read.

Measured chain (from the report):

```
claude   pid 2499  session 02a3919c…   ← ran /run-term
  zsh -c pid 3432  'target/debug/app'
    app  pid 3435  ← inherits CLAUDE_CODE_CHILD_SESSION=1, CLAUDE_PID=2499, …
      zsh  pid 3464  ← Pty::spawn hands the whole environment straight on
        claude pid 4330  ← reads the marker, calls itself a nested child session
```

Strip the session-identity markers in `Pty::spawn` with an explicit denylist so each
tab's shell starts with its own identity.

## Non-goals

- No change to `ai-monitor`, `crates/app/src/dashboard.rs`, or `crates/app/src/tabsession.rs`.
  The monitor is not at fault; it reports the absence correctly.
- No `CLAUDE_*` wildcard wipe — it would take `CLAUDE_CONFIG_DIR`,
  `CLAUDE_CODE_USE_BEDROCK` and other user configuration with it.
- No general environment sanitization policy (PATH, VIRTUAL_ENV, editor vars, …) —
  only session-identity markers, plus our own two shell-integration markers (D7).
- No injection of a positive "spawned by mac-terminal" marker.
- No change to how the tab's session id is *discovered* once a log does exist. In
  particular the guarded `CLAUDE_CODE_SESSION_ID` tier that this fix newly unblocks is
  **not** built here (see Known Gaps).

## Confirmed Goal

Two families of inherited state are removed from the child environment before zsh is
spawned, so a tab starts with its own identity:

1. **Agent session-identity markers** — an eleven-name `const` denylist removed
   unconditionally by name in `crates/term-core/src/pty.rs`, via
   `CommandBuilder::env_remove`. Static list, no environment oracle (D3).
2. **Our own shell-integration markers** — `MAC_TERMINAL_USER_ZDOTDIR` and a `ZDOTDIR`
   that points at our generated dir, removed and then re-derived at the top of
   `crates/term-core/src/shell_integration.rs::inject_with`, so an app launched from a
   mac-terminal tab chains the user's *real* zsh config instead of our generated scripts
   (D7/D9/D13).

A `tracing::debug!` at spawn names what was stripped and which agent-ish variables were
passed through, so the next unknown marker is one log line rather than a manual
process-tree dig (D2/D11).

Done when:

- A tab spawned from an app that was itself launched inside a Claude session carries none
  of the eleven session-identity variables, and the ai-monitor panel populates from the
  tab's own session.
- User configuration (`CLAUDE_CONFIG_DIR`, `CLAUDE_CODE_USE_BEDROCK`, `ANTHROPIC_*`, …)
  survives untouched.
- An app launched **from a mac-terminal tab** gives its tabs a `MAC_TERMINAL_USER_ZDOTDIR`
  equal to the user's real ZDOTDIR (not our generated dir), so `zshrc.zsh:9` chains the
  user's config instead of sourcing itself.
- Both the removals and the survivals are asserted by headless unit tests that never read
  or write the process environment.

## Research

**The leak site**
- `Pty::spawn` builds the child env additively only: `CommandBuilder::new(SHELL)` then
  `cmd.env("TERM", …)`, the locale decision, and `shell_integration::inject`. There is no
  removal of any kind (`crates/term-core/src/pty.rs:112-124`).
- The `locale_action` doc already names the mechanism: "`portable_pty::CommandBuilder::new`
  seeds itself from `std::env::vars_os()` and `spawn` does `env_clear()` before re-applying
  that snapshot" (`crates/term-core/src/pty.rs:44-46` — a *function* doc; the module doc is
  lines 1-9). So the parent's whole environment is the child's baseline.
- Confirmed in the dependency: `CommandBuilder::new` → `envs: get_base_env()`
  (`portable-pty-0.9.0/src/cmdbuilder.rs:218`, seeded from `std::env::vars_os()` at
  `:74-86`), and `as_command` does `cmd.env_clear()` then re-applies the map
  (`cmdbuilder.rs:528-536`), which the unix spawn path calls (`unix.rs:231`). Removing a key
  from the map therefore removes it from the child, not merely "leaves it unset".
- `pub fn env_remove<K>(&mut self, key: K)` exists in portable-pty 0.9.0
  (`cmdbuilder.rs:316-322`). The manifest says `portable-pty = "0.9.0"`
  (`crates/term-core/Cargo.toml:13`) — a caret range, locked to 0.9.0 by `Cargo.lock`, not
  pinned by the manifest. No version change is needed either way.
- Two caveats on that guarantee: `as_command` is `pub(crate)`, so env_clear-then-reapply is
  an implementation detail of 0.9.0 rather than a published contract; and `as_command:529`
  re-sets `SHELL` unconditionally while `get_base_env:88-99` back-fills it — harmless for
  all eleven names, but `SHELL` must never be added to the denylist expecting it to take.
- `pub fn get_env` (`cmdbuilder.rs:328`) makes the result assertable in a unit test without
  spawning anything.

**Nothing else in the app leaks the environment onward**
- `Pty::spawn` is the only site that hands a long-lived child the environment
  (`crates/app/src/session.rs:73` and `:360`, `crates/term-core/tests/pty_smoke.rs:18,53`).
- The only other subprocesses are read-only queries that pass nothing on:
  `/bin/ps` (`crates/app/src/tabsession.rs:261`) and `tmux list-clients`
  (`crates/app/src/tabmeta/tmux.rs:151`).
- **Zero** Rust sources in the workspace reference any of the denylisted markers:
  `grep -rn --include='*.rs' -E 'CLAUDE_CODE|CLAUDECODE|CLAUDE_PID|AI_AGENT|CHILD_SESSION' crates/`
  returns nothing. The app has no behavior keyed on them, so stripping them changes nothing
  in-process.

**The existing pattern to copy**
- `locale_action(get: &dyn Fn(&str) -> Option<String>) -> LocaleAction` is a pure decision
  function over an injected env oracle, with the reason stated in the doc: "`std::env::set_var`
  is unsound under a test harness" (`crates/term-core/src/pty.rs:40-68`). Its table-driven test
  enumerates every requirement id (`pty.rs:180-245`).
- `shell_integration::inject_with(cmd, get_env, write_dir)` is the same shape one level up: a
  testable core that mutates a `CommandBuilder`, with `inject` as the thin production wrapper
  (`crates/term-core/src/shell_integration.rs:33-58`).
- Its tests build a `clean_cmd()` — `CommandBuilder::new` + `env_clear()` — precisely because
  "`CommandBuilder::new` seeds itself from the real process env, so each test starts from a
  cleared builder to assert only what `inject_with` sets"
  (`crates/term-core/src/shell_integration.rs:97-103`). The same trick lets a test seed *fake*
  markers and assert they are gone.

**Why the panel is blameless**
- `tabsession.rs` answers "which session runs in *this tab*" from the tab's own process tree,
  not from the log directory (`crates/app/src/tabsession.rs:1-17`). It finds the tab's `claude`
  fine; there is simply no session id (no `--resume`, no `~/.claude/sessions/<pid>.json`) and no
  log dated to it.
- That lands on `Detached::NoLog` with `tab_has_claude = true`, whose message is deliberately
  worded for exactly this case (`crates/app/src/dashboard.rs:304-306`).

**Verification surface**
- CI runs `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`,
  `cargo test --workspace`, release build (`.github/workflows/ci.yml`).
- `crates/term-core/tests/pty_smoke.rs` spawns a real zsh but is `#[ignore]`d by default
  ("so CI stays deterministic"), so assertions placed *there* are opt-in.
- But CI is **not** free of real spawns: `session.rs:360` `live_session()` calls `Pty::spawn`
  and is used by a `#[test]` at `:376` that is **not** ignored. So `cargo test --workspace`
  already starts a real zsh, and both the new strip and the new `debug!` execute on every CI
  run. That is a free end-to-end exercise of the code path, and a reason the new code must not
  panic or write to stdout.

**Surveying the child env without reading the process env**
- `CommandBuilder::iter_full_env_as_str()` yields the builder's *entire* env map, inherited
  base vars included (`cmdbuilder.rs:378-390`). Its sibling `iter_extra_env_as_str()` filters
  on `is_from_base_env` and yields only caller-set vars (`cmdbuilder.rs:357-376`) — useless
  for surveying what was inherited.
- So anything that needs to know "what agent-ish variables are heading into this child" can
  read the builder, not `std::env` — which keeps it deterministic under test.

**An adjacent leak of the same family (self-nesting)**
- `inject_with` captures `ZDOTDIR` (falling back to `$HOME`) from the parent env and re-exports
  it as `MAC_TERMINAL_USER_ZDOTDIR` for the generated scripts to chain
  (`crates/term-core/src/shell_integration.rs:46-57`).
- When the app is launched *from a mac-terminal tab*, the parent's `ZDOTDIR` is already our
  generated dir. The capture then names our own generated dir as "the user's", and
  `zshrc.zsh` runs `source "$MAC_TERMINAL_USER_ZDOTDIR/.zshrc"` — the file sourcing itself
  (`crates/term-core/assets/shell-integration/zshrc.zsh:9`). Same root cause: the parent's
  environment passed straight on.
- `materialize()` computes the generated dir from `HOME` **and** does I/O in the same function
  (`shell_integration.rs:64-72`), and it is called only *after* `inject_with`'s early returns
  (`:50-55`) — which matters for where a value-conditional `ZDOTDIR` rule can live (D13).

**The predecessor spec, and what this one reverses**
- `specs/ai-monitor-session-isolation/spec.md` (Status: approved, approved 2026-08-01) lists
  this exact defect as a **Non-goal**: "Fixing `CLAUDE_CODE_CHILD_SESSION` inheritance (why the
  reporter's tab wrote no transcript). Environmental, not a code defect — `HANDOFF.md:22-24`
  says so explicitly" (`:17-19`). It also scopes `crates/term-core/**` out entirely (`:22`).
  This spec reverses both — see D17.
- That spec's Known Gaps (`:605-614`) rejected `CLAUDE_CODE_SESSION_ID` as a session-resolution
  signal *because it is inherited*: "the one signal that would work is `CLAUDE_CODE_SESSION_ID`
  … but it is *inherited* (observed alongside `CLAUDE_CODE_CHILD_SESSION=1`), so reading it
  unguarded would name a parent's session and reproduce this feature's entire bug class." This
  fix removes the inheritance, which changes the premise of that rejection.
- A **second** approved spec owns part of this ground. `specs/left-sidebar-waiting-area/spec.md`
  (Status: approved, 2026-07-21) names `pty.rs:112-120` the "ZDOTDIR/env 주입 choke point" (`:56`),
  decides the ZDOTDIR injection in its D10 (`:100-102`), and fixes the capture rule in its D18
  (`:132-134`): "원래 `$ZDOTDIR`(없으면 `$HOME`)를 먼저 캡처" — a two-tier capture, with an
  acceptance criterion at R4.2 (`:249`). D9 amends that contract; see D19.
- No spec covers the *agent-marker* half of PTY environment handling — checked this worktree and
  both siblings (`terminal`, `terminal-fix-session-card-label`). The sibling-only
  `specs/tabmeta-session-keying` addresses a different defect (sidebar labels keyed by cwd instead
  of session).

## Decisions

### D1: Strip an explicit, named denylist of agent session-identity variables
- **Status**: resolved
- **The list** (11 names): `AI_AGENT`, `CLAUDECODE`, `CLAUDE_CODE_CHILD_SESSION`,
  `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_CODE_EXECPATH`, `CLAUDE_CODE_MESSAGING_SOCKET`,
  `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_SSE_PORT`, `CLAUDE_CODE_VERSION`, `CLAUDE_EFFORT`,
  `CLAUDE_PID`. Exact names, not prefixes — `AI_AGENT` matches `AI_AGENT` only.
- **Rationale**: each names *which agent process and which session the parent is*, and every
  one of them is false inside a tab. `CLAUDE_CODE_CHILD_SESSION` is the one that suppresses the
  transcript and so empties the panel; the rest make the tab's claude misreport its own
  identity, socket and entrypoint. Rejected: a `CLAUDE_*` prefix wipe — it would also take
  `CLAUDE_CONFIG_DIR`, `CLAUDE_CODE_USE_BEDROCK` and the rest of the user's *configuration*,
  which is deliberate state the user set and which a tab should keep inheriting.

### D2: Fixed denylist, plus a debug trace naming what was passed through
- **Status**: resolved
- **Rationale**: the denylist is a policy that will drift — Claude Code will ship a marker this
  list does not know, and today the only way to discover that is the manual process-tree-plus-env
  dig that produced this spec. A `tracing::debug!` at spawn naming (a) the vars actually stripped
  and (b) the agent-ish vars seen and *not* stripped turns that rediscovery into one log line.
  Rejected: the bare fixed list with no trace — smallest diff, but it rebuilds the same blind
  spot that cost a manual investigation.
- **Rejected: inverting the policy** (strip every `CLAUDE_CODE_*` except a config allowlist).
  The strongest case for the inversion is that a denylist **fails open** — a new marker ships
  with no changelog and transcripts go silent again — while an allowlist **fails loud**, and
  that the two lists churn at opposite rates: config vars are near append-only, identity markers
  are per-release internals. Both halves of that are true, and neither is decisive here:
  - "Fails loud" overstates it. Eating `CLAUDE_CODE_USE_BEDROCK` does not produce an error; it
    silently routes to the direct API — different backend, different bill, same working
    terminal. Eating `CLAUDE_CONFIG_DIR` silently reads the *default* config dir, which exists
    and works, with the wrong settings and history. Neither announces itself.
  - The failure modes are not symmetric in cost. The denylist's failure is local and
    recoverable: a panel goes quiet, you add a name. The allowlist's failure lands on
    credentials, billing routing and config identity — and is recoverable only after you notice.
  - The churn-rate point is conceded, and answered by mechanism rather than by policy: D2's
    trace makes drift observable, and D10 states the review trigger.

### D3: A static const list, removed unconditionally — no environment oracle
- **Status**: resolved
- **Rationale**: `env_remove` on an absent key is a no-op, so nothing is gained by first asking
  whether a var is present. A `const` array plus a loop makes the test deterministic on both
  machines that matter: seed a *cleared* `CommandBuilder` with fake markers and fake config vars,
  run the strip, assert `get_env` — the result never depends on whether `cargo test` happens to be
  running inside a Claude session (it is, on the dev machine; it is not, on the CI runner).
  Rejected: a `locale_action`-style oracle function — it buys presence-knowledge that D11 gets
  from the builder instead, at the cost of machinery and a second thing to keep in sync.
- **Scope of this decision**: family 1 only. The `ZDOTDIR` rule in D7 is value-conditional by
  necessity and is deliberately *not* forced into this shape.

### D4: The change touches two modules, both in `term-core`
- **Status**: resolved (widened after D7/D13)
- **Files**: `crates/term-core/src/pty.rs` (the agent denylist) and
  `crates/term-core/src/shell_integration.rs` (our own markers + the capture rule). Nothing else.
- **Rationale**: `Pty::spawn` is the only site that hands a long-lived child the environment;
  the app's other subprocesses are read-only queries (`/bin/ps`, `tmux list-clients`) that pass
  nothing on. No Rust source in the workspace reads any of the denylisted vars, so removing them
  changes no in-process behavior. `ai-monitor`, `dashboard.rs` and `tabsession.rs` are already
  correct — they report an absence that is real.

### D5: `CommandBuilder::env_remove`, portable-pty 0.9.0 as already locked
- **Status**: assumed (auto-resolved from L1)
- **Rationale**: `env_remove` exists at `cmdbuilder.rs:316` and `as_command` does `env_clear()`
  then re-applies the map (`cmdbuilder.rs:528-536`), so a key removed from the builder is genuinely
  absent from the child rather than merely unset by us. No version change. Recorded caveat: that
  behavior is a 0.9.0 implementation detail (`as_command` is `pub(crate)`), and `SHELL` is re-set
  unconditionally by it — so `SHELL` must never be added to the denylist expecting it to take.

### D6: Table-driven unit tests over a cleared `CommandBuilder`
- **Status**: assumed (auto-resolved from L1)
- **Rationale**: the crate already has both halves of this pattern — `clean_cmd()`
  (`shell_integration.rs:97-103`, which exists for exactly this reason) and the requirement-labelled
  case table of `locale_action_table` (`pty.rs:181-245`). Rejected: asserting through
  `pty_smoke` — it is `#[ignore]`d by default so CI would never run it, and its result would depend
  on the launching environment. (Note that CI *does* spawn a real zsh via `session.rs:360`; that
  path exercises the code but asserts nothing about the environment — see D18.)

### D7: The strip covers our *own* leaked markers too, not just the agent's
- **Status**: resolved
- **Rationale**: the ZDOTDIR self-nesting bug is the same defect wearing different clothes — the
  parent's environment handed straight on. Launch the app from a mac-terminal tab and the inherited
  `ZDOTDIR` already points at our generated dir, so `inject_with` captures *our* dir as "the user's"
  and `zshrc.zsh:9` sources itself. Fixing both under one mechanism means `inject_with` always
  re-derives from a clean slate instead of compounding whatever the parent had.
  Rejected: deferring it to its own spec — it is a small addition to a function this spec is
  already touching, and leaving it means the very command used to test this fix (`/run-term` from
  a tab) still exercises the broken path.
- **Two rules, not one**: `MAC_TERMINAL_USER_ZDOTDIR` is removed unconditionally by name (it is
  purely ours and never user-set). `ZDOTDIR` is removed **only when the parent is one of our
  shells** — a user's real `ZDOTDIR` is legitimate configuration and must survive.

### D8: No escape hatch — the strip is unconditional
- **Status**: resolved
- **Rationale**: a tab is a new session, full stop. Anyone who genuinely wants the outer identity
  can `export` it inside the tab, which is explicit and visible at the point of use.
  Rejected: a TOML opt-out (`pty.inherit_agent_session`) — it buys a config surface, a serde field,
  a default, and a second code path to keep tested, for a case that has never come up, and the
  "off" path silently restores the exact bug this spec exists to remove.

### D9: In the nested case, re-capture the real ZDOTDIR from our own marker
- **Status**: resolved
- **Rationale**: when the parent env carries `MAC_TERMINAL_USER_ZDOTDIR`, the parent *is* one of our
  shells and that value is the user's real `ZDOTDIR` — we wrote it there one level up. Reading it as
  the capture source makes nesting depth irrelevant: an inner tab chains `~/.config/zsh` exactly like
  the outer one. Rejected: falling back to `$HOME` — correct only for users whose `ZDOTDIR` *is*
  `$HOME`, and silently lossy for a custom `ZDOTDIR`, which would make the fix itself introduce a
  config regression in the very workflow it targets.
- **Capture order in `inject_with`** (first non-empty wins):
  1. inherited `MAC_TERMINAL_USER_ZDOTDIR` — ground truth, written by us one level up
  2. inherited `ZDOTDIR`, unless it is recognised as ours (D13's predicate)
  3. `$HOME`
- **No ordering constraint between capture and strip.** The capture reads the *process env* through
  `get_env`; the strip mutates the *`CommandBuilder`*. Two different stores — removing a name from
  the builder cannot affect what the oracle returns, so no sequencing rule is needed between them.
  The one sequencing rule that *does* govern this family is D13's "before every early return", which
  is about our own re-set values, not about the capture.

### D10: The list covers Claude Code plus the two generic markers — nothing unmeasured
- **Status**: resolved
- **Rationale**: every one of the eleven names was observed leaking in a measured process chain.
  Other agent CLIs (Cursor, Codex, Gemini CLI) plausibly export their own identity markers, but not
  one has been measured here, and guessing variable names for tools that are not run produces
  entries nobody can test — a misspelled entry reads as covered while leaking. The list is extended
  on measurement, not on speculation.
- **Review trigger (the answer to D2's conceded churn-rate point)**: the denylist's doc comment
  states that it is re-verified against a Claude Code **major version** bump, and names the
  procedure — run `claude` in a tab, `env | grep -E 'CLAUDE|AI_AGENT|ANTHROPIC'`, compare against
  the list. Without a written trigger, "extend on measurement" has no occasion on which to fire.

### D11: The trace prints names only, filtered to agent-ish prefixes
- **Status**: resolved
- **Rationale**: the survey source is the whole child environment, which holds `ANTHROPIC_API_KEY`,
  cloud credentials and a socket path. A variable's *name* is not a secret; its *value* is — so no
  values are printed, ever. The prefix filter keeps the line to a handful of names rather than the
  100+ a full dump would produce per tab spawn.
  Rejected: printing every name unfiltered — it would catch an unguessably-named leak, but at a
  volume that makes the log unreadable. Rejected: reporting only the stripped set — zero exposure,
  but silent on exactly the unknown-marker case D2 exists to surface.
- **How the stripped set is obtained** (D12 makes the strip the first action, so the names are gone
  from the map by trace time): the strip loop records a hit when **`CommandBuilder::get_env(name)`**
  is `Some` *before* calling `env_remove`, and returns those names. The pass-through survey then
  reads the **final** builder via `iter_full_env_as_str()` after the whole environment is built, so
  what it reports is literally what the child receives.
- **Two different `get_env`s — do not conflate them.** Everything in this decision reads the
  **`CommandBuilder`**, never the process environment. `get_env` in D9 is the injected process-env
  oracle that `inject_with` takes as a parameter. Reading `std::env` here would break D3's
  determinism claim.
- **Survey filter**: names starting with `CLAUDE` or `ANTHROPIC`, plus exact `AI_AGENT`.
- **The shell-integration family is absent from BOTH halves of the line.** `ZDOTDIR` and
  `MAC_TERMINAL_USER_ZDOTDIR` are stripped inside `inject_with` (D13), which returns `()` and does
  not report to the trace — so they appear in neither the stripped set nor the survey, and no
  signature is changed to make them appear. That is the intended scope, not an oversight: the survey
  exists to detect *drift* in a third party's variable set, and our own two names are fully known and
  cannot drift. Their post-spawn state is directly observable in the child anyway (`echo $ZDOTDIR`),
  which is how D15's verification checks them.
- **Emission**: one `debug!` per spawn, emitted only when at least one of the two sets is
  non-empty — the Finder-launch case has nothing to report.

### D12: The **agent denylist** runs first, before `TERM`, the locale and `shell_integration::inject`
- **Status**: assumed (derived from D1 + D11)
- **Scope**: family 1 only — the eleven names in `pty.rs`. The shell-integration family has its own
  placement rule inside `inject_with` (D13); this decision says nothing about it.
- **Rationale**: our own additions can never be removed by a later denylist entry; and the trace's
  stripped-set half is built by that loop (D11), so it must run before the rest of the environment is
  assembled for the survey to describe the final state.

### D13: A pure path helper splits "where is our dir" from "write our dir"
- **Status**: resolved (was the L2-reviewer's confirmed defect in the first draft)
- **The defect**: an earlier draft put the value-conditional `ZDOTDIR` removal at the top of
  `inject_with`, but its predicate needs the generated dir path, which today only exists as the
  return of `materialize()` — called *after* both early returns and returning `None` on exactly the
  degrade path this decision's "Before every early return" rule (below) is load-bearing for. Concrete
  failure: parent is a mac-terminal
  tab, `HOME` unset → capture succeeds from `ZDOTDIR` → `materialize()` reads `HOME`
  (`shell_integration.rs:65`) → `None` → early return → the child keeps the parent's stale
  `ZDOTDIR` and `MAC_TERMINAL_USER_ZDOTDIR`. The bug, unfixed, on the path meant to fix it.
- **The decision**: extract `generated_zdotdir_path(get_env) -> Option<PathBuf>` — pure, no I/O,
  `HOME` joined with the support-dir suffix. `materialize` becomes that plus `create_dir_all` and
  the two writes. Both the strip predicate and the writer consume the helper, so the predicate no
  longer depends on I/O having succeeded.
- **The predicate — "is the parent one of our shells?"** — is answered primarily by *name*, not by
  path: an inherited `MAC_TERMINAL_USER_ZDOTDIR` exists only because one of our shells set it, and
  we always set it together with `ZDOTDIR`. The path comparison against `generated_zdotdir_path` is
  the secondary guard, catching a hand-set `ZDOTDIR=<our dir>` with no companion marker. When
  `HOME` is absent the helper returns `None`, the path guard simply does not fire, and the name
  rule still carries the ordinary nested case.
- **Placement**: the agent denylist is name-only and lives in `pty.rs` next to `locale_action`; the
  ZDOTDIR family lives at the top of `inject_with`, because that is where the capture rule and the
  generated-path knowledge already are. Note this is *inside* the `inject` call, hence after `TERM`
  and the locale in wall-clock order — D12's "first" governs the agent denylist only.
- **Before every early return.** The removal must precede both of `inject_with`'s early returns
  (`shell_integration.rs:50-55`, on no-`HOME` or failed materialization), because on the degrade path
  nothing is re-set: an early return that happens after the parent's stale `ZDOTDIR` /
  `MAC_TERMINAL_USER_ZDOTDIR` were left in place ships them straight to the child. This is what makes
  the removal load-bearing rather than redundant with the re-set on the success path.

### D14: No new public API
- **Status**: assumed (auto-resolved from L1)
- **Rationale**: nothing outside `term-core` spawns a long-lived child (D4), so the denylist `const`,
  the strip function and the path helper stay private to their modules and are tested in-module —
  the same shape as `LOCALE_VARS`/`locale_action` and `inject_with`. A `pub` surface would invite
  reuse that has no caller.

### D15: Live verification is defined against a *freshly opened* tab
- **Status**: resolved (from the inversion probe)
- **Rationale**: the likeliest way this ships correct and still looks broken is a stale process — the
  zsh and `claude` in an already-open tab were spawned by the old binary and keep the old environment,
  so the strip cannot retroactively apply to them. Without a stated pass condition, that reads as a
  failed fix. The verification task therefore names the observable: rebuild, relaunch, open a **new**
  tab, `env | grep -E 'CLAUDE|AI_AGENT'` shows configuration variables only, a `.jsonl` dated to now
  appears in `~/.claude/projects/<encoded-cwd>/`, and the panel populates.

### D16: The trace uses the crate's existing logging conventions — no new sink, no escalation
- **Status**: resolved (from the inversion probe)
- **Rationale**: `pty.rs` already logs through `tracing` (`warn_non_utf8_locale`, `pty.rs:79-88`), and
  this line sits one level below at `debug!`. Rejected: escalating to `warn!` when an *unknown*
  agent-ish variable is passed through — it would surface drift without anyone enabling debug logging,
  but "unknown" is decided by a prefix match, so every new benign `CLAUDE_CODE_*` configuration
  variable would raise a warning about nothing, and a warning that cries wolf gets filtered.

### D17: This spec reverses an approved predecessor's Non-goal, deliberately
- **Status**: resolved
- **What it reverses**: `specs/ai-monitor-session-isolation/spec.md:17-19` (approved 2026-08-01)
  declared `CLAUDE_CODE_CHILD_SESSION` inheritance out of scope as "Environmental, not a code defect",
  and `:22` scoped `crates/term-core/**` out of that feature entirely.
- **Rationale**: that classification was correct on the evidence then available — the marker really
  does originate outside this repo. The measured chain in this spec's Goal adds the fact that changes
  it: the app is not a bystander to the inheritance, it is the **propagator**. `Pty::spawn` is a line
  of code in this repository that copies the parent's identity into every tab, and it is ours to
  change. "Environmental" described the marker's origin, not its transmission.
- **What stays true from that spec**: its non-goal of *fixing the panel* still holds — this spec
  changes no panel code (D4), and its strict-isolation behavior is untouched. Only the `term-core`
  scope line is superseded, and only for the environment.

### D18: CI's existing real-spawn path is acknowledged, not extended
- **Status**: resolved
- **Rationale**: `session.rs:360` `live_session()` calls `Pty::spawn` from a `#[test]` at `:376` that
  is **not** `#[ignore]`d, so `cargo test --workspace` already starts a real zsh and will execute both
  the strip and the `debug!` on every CI run. That is a free smoke test of the code path, and it
  constrains the new code: it must not panic, must not write to stdout, and must not depend on any
  variable being present. No new integration gate is added — the deterministic assertions live in the
  unit tests (D6), and this path only proves the code runs.

### D19: D9 amends `left-sidebar-waiting-area`'s approved capture rule — a strict refinement
- **Status**: resolved
- **What it amends**: that spec's D18 (`:132-134`, approved 2026-07-21) fixed the capture as
  "원래 `$ZDOTDIR`(없으면 `$HOME`)를 먼저 캡처" — two tiers, `ZDOTDIR` then `$HOME` — with an
  acceptance criterion at R4.2 (`:249`). D9 makes it three tiers by inserting inherited
  `MAC_TERMINAL_USER_ZDOTDIR` ahead of both.
- **Rationale**: this is a refinement, not a reversal. The new tier fires *only* when the inherited
  `ZDOTDIR` is one of ours — a state that spec never contemplated because it predates any nesting
  case. In every scenario it did contemplate (launched from Finder, from another terminal, from a
  shell with or without a custom `ZDOTDIR`), tier 1 is absent and the behavior is byte-identical to
  its D18. Its R4.2 criterion — "원래 rc를 먼저 source" — is *better* satisfied afterwards, since the
  nested case now finds the user's real rc instead of our generated one.
- **Obligation**: the change must not break `left-sidebar-waiting-area`'s R4.1/R4.2 acceptance
  criteria; the existing `inject_*` tests that encode them stay green unchanged.

## Constraints

- `term-core` gains no dependency on wgpu or winit, and no new crate at all — `env_remove` is already
  in `portable-pty 0.9.0` as locked by `Cargo.lock` (`.claude/rules/dependency-direction.md`).
- No `std::env::set_var` in tests: it is unsound under a threaded harness, which is the stated reason
  `locale_action` takes an oracle (`crates/term-core/src/pty.rs:41-43`). Tests seed a cleared
  `CommandBuilder`, never the process environment.
- The trace prints variable **names** only. No value of any environment variable is ever logged.
- User configuration must survive untouched: `CLAUDE_CONFIG_DIR`, `CLAUDE_CODE_USE_BEDROCK`,
  `ANTHROPIC_*`, a user's real `ZDOTDIR`, and anything else not on the eleven-name list.
- `SHELL` is never added to the denylist — `as_command` re-sets it unconditionally, so the removal
  would not take (`cmdbuilder.rs:529`).
- Behavior when the app is launched from Finder (none of the markers present) must be unchanged —
  `env_remove` on an absent key is a no-op, and a test asserts this path explicitly.
- The new code runs in CI via `session.rs:376` (D18): no panics, no stdout writes, no assumption that
  any variable is present.
- No `#[cfg(target_os = ...)]` branches (`.claude/rules/macos-only.md`).
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace` all stay green (`.github/workflows/ci.yml`).
- `Cargo.lock` is not edited (PreToolUse hook blocks it); no dependency change makes this relevant.

## Known Gaps

- **A tmux server that pre-dates the fix.** It captured its environment when *it* started and hands
  that to every pane it forks; `Pty::spawn` cannot reach across to it. Remedy is operational, not code:
  `tmux kill-server` (or let it exit) after installing the fix.
- **Other agent CLIs are unmeasured** (D10). Cursor, Codex and Gemini CLI may export equivalent
  identity markers; none has been observed leaking here, so none is on the list.
- **Processes already running in open tabs keep the old environment** (D15). The fix applies to tabs
  opened after the rebuild; there is no retroactive path and none is attempted.
- **The diagnosis rests on the measured chain, not on a controlled A/B.** The report's independent
  cross-check — launch the `.app` from Finder, where no markers exist, and confirm the panel works
  today — was offered as a second guard and not adopted; if the panel stays empty after D15's
  verification, that differential is the next diagnostic step, not a code change.
- **This fix unblocks a follow-up it does not build.** `ai-monitor-session-isolation` rejected
  `CLAUDE_CODE_SESSION_ID` as a session-resolution signal *because it is inherited* (`:605-614`).
  Once tabs no longer inherit it, a `CLAUDE_CODE_SESSION_ID` read inside a tab names that tab's own
  session exactly — the "guarded tier" that spec defers becomes viable, and stronger than the timing
  heuristics `tabsession.rs` uses today. That is a separate L2 round, explicitly out of scope here.
- **The `env_clear`-then-reapply guarantee is an implementation detail**, not a published contract
  (`as_command` is `pub(crate)`). A portable-pty upgrade should re-verify it; the unit tests assert
  the builder's state, not the child's, so they would not catch a regression in that layer.

## Requirements

### R0: A tab's environment carries the tab's own identity (D1, D7)

#### R0.1: No inherited agent session identity reaches the child
- **Given**: the app process itself carries `CLAUDE_CODE_CHILD_SESSION=1`, `CLAUDE_PID=2499`,
  `CLAUDE_CODE_SESSION_ID=02a3919c-…` and `CLAUDE_CODE_MESSAGING_SOCKET=/tmp/cc-socks/2499.sock`
- **When**: `Pty::spawn` builds the child environment
- **Then**: none of the eleven names in the denylist is present in the `CommandBuilder`'s env map

#### R0.2: No inherited mac-terminal shell-integration state reaches the child
- **Given**: the app was launched from a mac-terminal tab, so it inherits `ZDOTDIR` pointing at our
  generated dir and a `MAC_TERMINAL_USER_ZDOTDIR` set by the outer shell
- **When**: `Pty::spawn` builds the child environment
- **Then**: `MAC_TERMINAL_USER_ZDOTDIR` holds the *user's real* ZDOTDIR (not our generated dir), and
  `ZDOTDIR` holds our generated dir because we set it, not because it was inherited

### R1: The agent denylist is removed by exact name, first (D1, D3, D5, D10, D12)

#### R1.1: All eleven names are removed when present
- **Given**: a cleared `CommandBuilder` seeded with all eleven names at arbitrary values
- **When**: the strip runs
- **Then**: `get_env` returns `None` for every one of `AI_AGENT`, `CLAUDECODE`,
  `CLAUDE_CODE_CHILD_SESSION`, `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_CODE_EXECPATH`,
  `CLAUDE_CODE_MESSAGING_SOCKET`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_SSE_PORT`,
  `CLAUDE_CODE_VERSION`, `CLAUDE_EFFORT`, `CLAUDE_PID`

#### R1.2: Matching is by exact name, never by prefix
- **Given**: a cleared `CommandBuilder` seeded with `CLAUDECODE_FUTURE=x` and `AI_AGENT_ROLE=y`
  alongside `CLAUDECODE=1` and `AI_AGENT=claude`
- **When**: the strip runs
- **Then**: `CLAUDECODE` and `AI_AGENT` are gone; `CLAUDECODE_FUTURE` and `AI_AGENT_ROLE` are
  untouched at their original values

#### R1.3: Removing an absent variable is a no-op
- **Given**: a cleared `CommandBuilder` with none of the eleven names set (the Finder-launch case)
- **When**: the strip runs
- **Then**: it completes without error and the builder's env map is unchanged

#### R1.4: The strip precedes everything `Pty::spawn` sets
- **Given**: a parent environment carrying the eleven markers
- **When**: `Pty::spawn` completes its environment setup
- **Then**: `TERM=xterm-256color`, the locale decision's injection, `ZDOTDIR` and
  `MAC_TERMINAL_USER_ZDOTDIR` all hold the values *we* set, none having been removed by the strip

#### R1.5: The denylist and its review trigger are documented at the definition
- **Given**: a contributor reading the `const` declaration
- **When**: they look for how the list is maintained
- **Then**: the doc comment states each entry's reason, that the list is extended on measurement
  rather than speculation, and the re-verification procedure to run on a Claude Code major bump

#### R1.6: (child boundary) The spawned shell reports none of the markers
- **Given**: an app process carrying the eleven markers
- **When**: a real zsh is spawned through `Pty::spawn` and asked to print its environment
- **Then**: none of the eleven names appears in the child's output

### R2: User configuration survives untouched (D1, D10)

#### R2.1: Claude configuration variables are not removed
- **Given**: a cleared `CommandBuilder` seeded with `CLAUDE_CONFIG_DIR=/x`,
  `CLAUDE_CODE_USE_BEDROCK=1` and `ANTHROPIC_API_KEY=sk-…`
- **When**: the strip runs
- **Then**: all three are still present at their original values

#### R2.2: A user's real `ZDOTDIR` survives
- **Given**: an inherited `ZDOTDIR=/Users/tester/.config/zsh` that is not our generated dir, and no
  inherited `MAC_TERMINAL_USER_ZDOTDIR`
- **When**: `inject_with` runs
- **Then**: `MAC_TERMINAL_USER_ZDOTDIR` is set to `/Users/tester/.config/zsh`

#### R2.3: `SHELL` is not on the denylist
- **Given**: the denylist `const`
- **When**: its contents are asserted in a test
- **Then**: `SHELL` is absent from it, and the doc comment records why adding it would not take
  (`as_command` re-sets it unconditionally)

### R3: The shell-integration family is re-derived, not inherited (D7, D9, D13, D19)

#### R3.1: `MAC_TERMINAL_USER_ZDOTDIR` is removed unconditionally
- **Given**: a cleared `CommandBuilder` seeded with `MAC_TERMINAL_USER_ZDOTDIR=/stale`
- **When**: `inject_with` runs and reaches any exit path, success or degrade
- **Then**: the builder never carries `/stale` — either the value we re-set, or nothing

#### R3.2: The nested case captures from our own marker (tier 1)
- **Given**: an inherited `MAC_TERMINAL_USER_ZDOTDIR=/Users/tester/.config/zsh` and an inherited
  `ZDOTDIR` equal to our generated dir
- **When**: `inject_with` runs
- **Then**: the child's `MAC_TERMINAL_USER_ZDOTDIR` is `/Users/tester/.config/zsh`, not the
  generated dir and not `$HOME`

#### R3.3: A hand-set `ZDOTDIR` pointing at our dir is caught by the path guard (tier 2)
- **Given**: an inherited `ZDOTDIR` equal to `generated_zdotdir_path()` with **no** companion
  `MAC_TERMINAL_USER_ZDOTDIR`, and `HOME=/Users/tester`
- **When**: `inject_with` runs
- **Then**: that `ZDOTDIR` is not used as the capture source; `MAC_TERMINAL_USER_ZDOTDIR` falls
  through to `/Users/tester`

#### R3.4: The non-nested path is byte-identical to today (tier 3)
- **Given**: no inherited `MAC_TERMINAL_USER_ZDOTDIR`, and either a foreign `ZDOTDIR` or none
- **When**: `inject_with` runs
- **Then**: the capture result equals what the existing two-tier rule produced — `ZDOTDIR` if set,
  else `$HOME` — and the existing `inject_*` tests pass unchanged

#### R3.5: A missing `HOME` disables the path guard without disabling the name rule
- **Given**: `HOME` unset, an inherited `MAC_TERMINAL_USER_ZDOTDIR=/Users/tester/.config/zsh`, and
  an inherited `ZDOTDIR` equal to our generated dir
- **When**: `inject_with` runs and `generated_zdotdir_path()` returns `None`
- **Then**: the tier-1 name rule still supplies the capture, and neither marker is left at its
  inherited value on any exit path

#### R3.6: `generated_zdotdir_path` is pure
- **Given**: a stub oracle supplying only `HOME=/Users/tester`
- **When**: `generated_zdotdir_path` is called
- **Then**: it returns
  `/Users/tester/Library/Application Support/mac-terminal/shell-integration` without creating any
  directory or writing any file, and `materialize` produces the same path

#### R3.7: (child boundary) A nested tab chains the user's real rc
- **Given**: an app launched from a mac-terminal tab, with the user's real config in
  `~/.config/zsh`
- **When**: a tab's zsh sources our generated `.zshrc`
- **Then**: it sources `~/.config/zsh/.zshrc` rather than sourcing our generated `.zshrc` again

### R4: The trace makes drift observable without leaking values (D2, D11, D16)

#### R4.1: The stripped set is captured before removal
- **Given**: a builder seeded with three of the eleven names
- **When**: the strip loop runs
- **Then**: it returns exactly those three names, even though `get_env` for them returns `None`
  afterwards

#### R4.2: The pass-through survey reads the final builder
- **Given**: a fully built child environment containing `CLAUDE_CONFIG_DIR` and an unknown
  `CLAUDE_CODE_PARENT_RUN_ID`
- **When**: the survey runs after the whole environment is assembled
- **Then**: it reports both names, because both are heading into the child

#### R4.3: Only names are emitted, never values
- **Given**: `ANTHROPIC_API_KEY=sk-secret` and `CLAUDE_CODE_MESSAGING_SOCKET=/tmp/cc-socks/2499.sock`
  in the environment
- **When**: the trace line is produced
- **Then**: it contains the variable names and neither `sk-secret` nor the socket path

#### R4.4: The survey filter is prefix `CLAUDE`/`ANTHROPIC` plus exact `AI_AGENT`
- **Given**: an environment with `CLAUDE_X`, `ANTHROPIC_Y`, `AI_AGENT`, `AI_AGENT_ROLE` and `PATH`
- **When**: the survey runs
- **Then**: it reports `CLAUDE_X`, `ANTHROPIC_Y` and `AI_AGENT`, and reports neither `AI_AGENT_ROLE`
  nor `PATH`

#### R4.5: Nothing is emitted when there is nothing to report
- **Given**: a Finder-launched app whose environment has no agent-ish variable at all
- **When**: `Pty::spawn` runs
- **Then**: no trace line is produced

#### R4.6: The shell-integration family appears in neither half
- **Given**: a nested launch where `ZDOTDIR` and `MAC_TERMINAL_USER_ZDOTDIR` were both stripped and
  re-set
- **When**: the trace line is produced
- **Then**: neither name appears in the stripped set nor in the survey, and no function signature
  was changed to make them appear

### R5: The behavior is unconditional and safe in CI (D8, D18)

#### R5.1: There is no opt-out
- **Given**: the shipped configuration surface
- **When**: a user searches it for a way to restore agent-session inheritance
- **Then**: no such key exists, and no code path skips the strip

#### R5.2: The CI real-spawn path stays green
- **Given**: `cargo test --workspace` on a clean macOS runner, where `session.rs:376` spawns a real
  zsh through `Pty::spawn`
- **When**: the new strip and trace execute on that path
- **Then**: the test passes — no panic, no stdout write, and no dependence on any variable being
  present

#### R5.3: The trace is inert without a subscriber
- **Given**: a binary with no `tracing` subscriber installed
- **When**: `Pty::spawn` runs
- **Then**: nothing is written to any stream

### R6: The change is headless-testable and adds no surface (D3, D6, D14, dependency rule)

#### R6.1: Tests never touch the process environment
- **Given**: the new tests
- **When**: they are read or run under a threaded harness
- **Then**: they contain no `std::env::set_var`, seed a cleared `CommandBuilder` or a stub oracle,
  and produce identical results inside and outside a Claude session

#### R6.2: No new public API
- **Given**: `term_core`'s public surface before and after
- **When**: they are compared
- **Then**: they are identical — the denylist `const`, the strip function and
  `generated_zdotdir_path` are all private to their modules

#### R6.3: No new dependency, and the crate stays headless
- **Given**: `crates/term-core/Cargo.toml` and `Cargo.lock`
- **When**: the change lands
- **Then**: both are unmodified, and `term-core` still depends on neither wgpu nor winit

### R7: The fix is verifiable on a live tab (D15)

#### R7.1: A freshly opened tab shows configuration only
- **Given**: the app rebuilt and relaunched from inside a Claude session
- **When**: a **new** tab is opened and `env | grep -E 'CLAUDE|AI_AGENT|ANTHROPIC'` is run in it
- **Then**: the output contains configuration variables only, and none of the eleven markers

#### R7.2: The tab's own session writes a log and the panel reads it
- **Given**: the freshly opened tab from R7.1
- **When**: `claude` is started in it and a prompt is sent
- **Then**: a new `.jsonl` dated to now appears in `~/.claude/projects/<encoded-cwd>/`, and the
  ai-monitor panel populates from it instead of showing the empty state

#### R7.3: A pre-existing tab is documented as out of reach, not as a failure
- **Given**: a tab that was already open before the rebuild
- **When**: its environment is inspected and still shows the markers
- **Then**: this is recorded as expected — the strip applies to shells spawned after the rebuild —
  so a stale process is not mistaken for a failed fix

### R8: Both predecessor specs are reconciled in writing (D17, D19)

#### R8.1: `left-sidebar-waiting-area`'s acceptance criteria stay green
- **Given**: that spec's R4.1 (ZDOTDIR injection registers the OSC 133 hooks) and R4.2 (the injected
  zshrc chains the user's rc first, add-only)
- **When**: the three-tier capture of D9 replaces its two-tier rule
- **Then**: the existing tests encoding those criteria pass unchanged, with no test edited to
  accommodate the new tier

#### R8.2: `ai-monitor-session-isolation`'s panel behavior is untouched
- **Given**: that spec's strict per-tab isolation and its explained empty states
- **When**: this change lands
- **Then**: no file under its ownership is modified — `dashboard.rs`, `tabsession.rs` and the
  `ai-monitor` crate are byte-unchanged — and the reversal of its Non-goal is recorded in D17 rather
  than left implicit

## Tasks

### T1: Extract the child-environment assembly into a testable seam [infra]
- **Fulfills**: R6.1, R6.2, R6.3
- **Depends on**: (none)
- **Files**: `crates/term-core/src/pty.rs`
- **Why first**: `Pty::spawn` currently interleaves `openpty` with environment assembly, so nothing
  about the assembled environment can be asserted without spawning a real zsh. Pull the assembly
  (`CommandBuilder::new` + `TERM` + the locale decision + `shell_integration::inject`) into a private
  function returning the `CommandBuilder`; `Pty::spawn` calls it and hands the result to
  `spawn_command`. Pure refactor — no behavior change, no public API change, existing tests unchanged.
  This is the seam R1.4, R4.2 and R4.5 are asserted through.

### T2: Strip the agent session-identity denylist [vertical]
- **Fulfills**: R0.1, R1.1, R1.2, R1.3, R1.4, R1.5, R1.6, R2.1, R2.3, R5.1, R5.2
- **Depends on**: T1
- **Files**: `crates/term-core/src/pty.rs`
- **Scope**: the documented eleven-name `const` (D1) with its per-entry reasons and review trigger
  (D10); a strip function that loops `env_remove` and returns the names it actually found (D3, D11);
  its call placed first in the assembly (D12). Tests: the R1/R2 table over a cleared `CommandBuilder`,
  including the exact-name and absent-variable cases, plus the `SHELL`-not-on-the-list assertion. The
  `#[ignore]`d child-boundary check (R1.6) goes in `pty_smoke.rs`.

### T3: Re-derive the shell-integration markers instead of inheriting them [vertical]
- **Fulfills**: R0.2, R2.2, R3.1, R3.2, R3.3, R3.4, R3.5, R3.6, R3.7, R8.1
- **Depends on**: (none) — **parallel with T1 and T2**: disjoint file, and no signature crosses
  between them (D14/R4.6 keep `inject_with` returning `()`)
- **Files**: `crates/term-core/src/shell_integration.rs`
- **Scope**: split `generated_zdotdir_path` out of `materialize` as a pure, I/O-free helper over the
  env oracle (D13); remove both markers at the top of `inject_with`, ahead of every early return;
  replace the two-tier capture with D9's three tiers. Tests: the R3 table, including the `HOME`-unset
  case that the first draft of this spec got wrong, and a re-run of the existing `inject_*` tests
  **unedited** to prove R8.1/R3.4.

### T4: Emit the drift trace [vertical]
- **Fulfills**: R4.1, R4.2, R4.3, R4.4, R4.5, R4.6, R5.3
- **Depends on**: T1, T2 — same file as both, and it consumes the name list T2's strip returns
- **Files**: `crates/term-core/src/pty.rs`
- **Scope**: the agent-ish survey over the **final** builder via `iter_full_env_as_str` (prefix
  `CLAUDE`/`ANTHROPIC`, exact `AI_AGENT`), and one `tracing::debug!` naming both sets — names only,
  emitted only when at least one set is non-empty (D11, D16). Tests assert the *composed* line's
  contents from a seeded builder, including that no value appears in it.

### T5: Verify on a live tab [vertical]
- **Fulfills**: R7.1, R7.2, R7.3
- **Depends on**: T2, T3, T4
- **Files**: (none — this is a run, not an edit)
- **Scope**: `cargo build`, relaunch, open a **freshly created** tab, and walk D15's observable:
  `env | grep -E 'CLAUDE|AI_AGENT|ANTHROPIC'` shows configuration only; `claude` in that tab produces
  a `.jsonl` dated to now under `~/.claude/projects/<encoded-cwd>/`; the panel populates. Record that
  a pre-existing tab still shows the markers and that this is expected, so the next reader does not
  read a stale process as a failed fix.

### T6: Annotate both predecessor specs with a superseded-by pointer [docs]
- **Fulfills**: R8.2
- **Depends on**: T3
- **Files**: `specs/ai-monitor-session-isolation/spec.md`, `specs/left-sidebar-waiting-area/spec.md`
- **Scope**: both are **approved** documents and their text stays intact — this adds a pointer only.
  On `ai-monitor-session-isolation`'s Non-goal (`:17-19`), note that `pty-session-env-isolation` D17
  reclassifies the inheritance as a code defect and fixes it, and that its `CLAUDE_CODE_SESSION_ID`
  Known Gap (`:605-614`) rests on a premise that no longer holds. On `left-sidebar-waiting-area`'s
  D18 (`:132-134`), note that D9 extends the capture to three tiers. Also confirm R8.2's negative:
  `dashboard.rs`, `tabsession.rs` and the `ai-monitor` crate are untouched by the diff.

**DAG**

```
T1 ──┬── T2 ──── T4 ──┐
     │                ├── T5
T3 ──┴────────────────┘
 └── T6
```

T1 ∥ T3 and T2 ∥ T3 (different files, no shared interface). T4 serializes behind T2 because both
edit `pty.rs` and T4 consumes T2's return value.

## External Dependencies

### Pre-work
- **Re-measure the eleven names against the locally installed Claude Code** before hardcoding them
  (D10's rule is "extend on measurement"): run `claude` in a tab and
  `env | grep -E 'CLAUDE|AI_AGENT|ANTHROPIC'`. The list in D1 came from a measured chain; a different
  local version may carry a name it does not have.

### Post-work
- **Rebuild and relaunch** — T5's verification is meaningless against the old binary (R7.3).
- **`tmux kill-server`** if a tmux server predating the fix is still running; it hands its own
  captured environment to every pane it forks and `Pty::spawn` cannot reach it (Known Gaps).
- **Consider the newly unblocked follow-up**: with inheritance removed, a guarded
  `CLAUDE_CODE_SESSION_ID` read names a tab's own session exactly — the tier
  `ai-monitor-session-isolation` deferred. Its own `/specify` round, not this one.
