# Spec: agent-classifier-deleted-binary

## Meta
- **Created**: 2026-08-29
- **Type**: dev
- **Status**: approved (L4) — plan only; implementation not started
- **Approved by**: user
- **Approved at**: 2026-08-29
- **Line references**: as of `e99ad7a` (the commit this work was rebased onto).
  References into `tabmeta/{agent,proc,mod}.rs` describe the **pre-fix** file, which
  is the state this spec was written against and the state the bug lives in.
- **Complexity**: Medium (4 signals) — the staged classifier ladder (D1, D2); `unsafe` FFI
  against a partially-specified kernel struct (D1, D4, D5); the global test-instrumentation
  invariants a new counter touches (D5, D8); and real-process test infrastructure — spawn,
  rename, unlink, reap, under CI (D6, D7). The last is a distinct risk domain from the first,
  and it is where this spec's review found most of its defects.

## Goal
When an agent's executable is unlinked from disk while the process is still running —
which `agy`'s auto-updater does on every launch (rename to `<name>.<id>.old`, then delete
it on a later run) — the tab must keep being recognised as running that agent, so the
ai-monitor panel and the sidebar card do not silently go blank mid-session.

## Non-goals
- Opening a `history.jsonl` fallback for a tab whose agent could not be detected.
  `overlay.rs` only enters the Antigravity path when the walk names an `agy` pid, so
  `Probed::NoAgent` is never constructed in production and the AG6 tier-2 path is dead
  code today. Reviving it is a separate design decision — AG6 closed that door on purpose,
  to stop an `/exit`ed older conversation being adopted as the tab's live session — and it
  is explicitly out of scope here.
- Any change to how a *resolved* session is tailed, parsed or rendered. This spec ends at
  "which agent is this tab running"; everything downstream of that answer is untouched.
- Recovering a deleted **`claude`** versioned-install binary
  (`~/.local/share/claude/versions/<ver>`). Claude already has an argv-based floor
  (`tabsession::find_claude` over a `/bin/ps` table) that survives deletion, so the panel
  does not lose a Claude tab this way. The sidebar skin would, but that is a strictly
  smaller, unobserved defect and folding it in would widen the change surface past the
  reported bug.
- Adding any new detectable agent, or changing the agent tables' membership.

## Confirmed Goal
`tabmeta::agent`'s classifier must survive `proc_pidpath` returning nothing. Today
`classify_pid` reads `proc::exe_path` and `classify_with` opens with `let path = exe_path?;`,
so an unlinked executable makes **all four** classification stages unreachable and the
process is reported as "not an agent". The fix is a fallback source for the process's name
that the kernel still holds after the file is gone, applied only when `exe_path` is absent,
with the existing stages, their exactness, and their syscall budget unchanged.

Done when: a live pid whose executable has been unlinked still classifies as its
`AgentKind`; the behaviour is locked by a test that fails on the current code; classification
of a process with a readable `exe_path` is bit-for-bit unchanged; and the per-process syscall
cost does not grow **for any pid whose `exe_path` is readable**.

That last clause is bounded deliberately. An earlier draft said "over non-agent processes",
which D8 makes unsatisfiable: a walk over ordinary processes routinely contains pids that died
between the child listing and the probe (`worker.rs:1374` documents that transience), and those
are exactly the pids that now pay one extra read. "Readable `exe_path`" is what D8 and D10
actually promise and what the existing equality assertions actually test. The bound itself is
D8's, which is an agent-made call — this L2 gate is where a human accepts or rejects it.

## Research

### The defect, in the code
- The classifier's single point of failure is one line: `let path = exe_path?;`
  (`crates/app/src/tabmeta/agent.rs:122`). Every stage — 1 basename, 1a leading
  dot-component, 1b `<agent>/versions`, 2 interpreter argv — lives *below* it, so an absent
  path skips all four and the process reports as "not an agent".
- The only source feeding it is `proc::exe_path` (`crates/app/src/tabmeta/proc.rs:72`), which
  returns `None` whenever `proc_pidpath` returns `<= 0`. `classify_pid` (`agent.rs:211`)
  calls nothing else.
- The name half of the kernel's process record is untouched by this path even though
  `proc.rs` already reads the same struct twice: `tty_dev_of_pid` (`proc.rs:113`) and
  `start_ms_uncounted` (`proc.rs:174`) both pull `PROC_PIDTBSDINFO`. `proc.rs:147` states the
  precedent in words — "adds a field read, not a new syscall surface".

### Measured on this machine (2026-08-29), in two experiments

**Experiment 1 — `proc_pidpath` follows a rename, and dies on unlink.** A child exec'd from
`<tmp>/agy`, then renamed, then deleted:

```
exec'd as  <tmp>/agy         : proc_pidpath rc=150 path=<tmp>/agy
after mv → <tmp>/agy.<id>.old: proc_pidpath rc=174 path=<tmp>/agy.<id>.old
after rm                     : proc_pidpath rc=0   path=(none)
```

The middle row is why stage 1a exists (`specs/antigravity-agy-detection`). The last row is
this bug: `exe_path` is `None` for a running process whose binary was deleted.

**Experiment 2 — the kernel's name is frozen at exec and survives both.** Across the *same*
three steps, `proc_pidinfo(PROC_PIDTBSDINFO)` returned `pbi_name=[agy]` and
`pbi_comm=[agy]` at every step — unchanged by the rename, unchanged by the deletion.

This corrects a wrong reading in an earlier draft of this spec, and the correction matters
for the whole design. A first experiment that exec'd a file *already named* `agy.<id>.old`
reported `pbi_name=[agy.1784614720971820000.old]` and led to "the fallback lands on stage
1a". It does not. In the real incident `agy` is exec'd as `agy` and renamed afterwards, so
the recovered name is **`agy`** and the **exact stage-1 table match** fires. The name is
exec-time state, not a path lookup — which is a *stronger* property than the draft claimed:
it is immune to the rename and the deletion alike.

Buffer sizes, from the declarations the fix will read (`libc-0.2.186`
`unix/bsd/apple/mod.rs:619-620`): `pbi_comm: [c_char; MAXCOMLEN]` (16) and
`pbi_name: [c_char; 32]`. A 32-byte name therefore arrives with **no NUL terminator**.

### `agy` deletes its predecessor — the reported session's timeline
`~/.gemini/antigravity-cli/log/cli-*.log` shows `auto_updater.go:305] Spawned background
update process` on essentially every launch (16 occurrences); `~/.local/bin/agy` was
rewritten 2026-08-27 14:04 and **no `*.old` file remains** in `~/.local/bin`. The reported
session: `agy` up at 14:03:47 with updater pid 53386; conversation `cff60beb…` streaming from
14:04:20; a second `agy` + updater 54980 at 14:42:37; that conversation still writing its
transcript at 16:01. The window in which the running binary was renamed-then-deleted sits
inside the live session, which is why the panel worked and then stopped.

### What the failure reaches
- `overlay::open` asks `tabsession::agent_for` (`overlay.rs:396`) and keeps only a monitored
  kind (`overlay.rs:428`); the Antigravity branch is entered solely via `antigravity_pid`
  (`overlay.rs:1173`). A second consumer, `overlay.rs:820`, calls `agent_for` again **on a
  spawned background thread**. With no kind, the panel falls to the Claude wording
  `(no Claude session in this tab)` (`dashboard.rs:295-324`) — the exact string in the
  reported screenshot, and one only reachable when `is_monitored(tab_agent)` is false.
- The sidebar reads the same detector, so the card drops to `AgentSkin::Generic`
  (`sidebar_model.rs:149`) — visible in the same screenshot as the only plain card among
  four. One cause, two symptoms.
- **There is no safety net.** `Probed::NoAgent` is constructed nowhere in `crates/app`
  (verified: `overlay.rs:518` names it only in a comment); it appears only in
  `crates/ai-monitor/src/antigravity.rs:220,274` and `crates/ai-monitor/tests/antigravity.rs`.
  The `history.jsonl` tier is therefore dead in production. (In this incident it would have
  resolved: `history.jsonl` holds 6 records pairing `/Users/hyunhokim/IdeaProjects/anki` with
  `cff60beb…`, whose transcript is on disk. Reviving it is a Non-goal here.)

### Why Claude tabs did not show this
`tabsession::find_claude` (`tabsession.rs:558`) judges by argv[0] from a `/bin/ps` table
(`is_claude`, `tabsession.rs:585`) and acts as a floor under `tab_agent` (`overlay.rs:436`),
so a deleted Claude binary still yields a panel. Antigravity has only the `proc_pidpath`
route. The asymmetry is the structural reason the bug is agent-specific — and the reason
D1 has to argue against reusing that floor rather than merely noting it.

### Prior art in this repo — the same lesson, twice already
- `specs/antigravity-agy-detection/loop.md` added stage 1a for the updater's in-place rename
  and recorded the lesson verbatim: "detection must not assume `proc_pidpath` basename == the
  command name". Deletion is the next step of that same updater, unaddressed.
- `specs/agent-skin-detection-fix/loop.md` added the `<agent>/versions` signature for the
  same class of problem on the Claude side.

### Constraints the fix inherits
- **Cost instrumentation.** `probe::PROC_EXE` / `PROC_ARGS` (`tabmeta/mod.rs:399-470`,
  `reset_all` at `:459`) are asserted by `agent.rs:333`
  (`sysctl_gate_closed_for_non_interpreters`), `activity.rs:1158`, and
  `worker.rs:485,1034,1341` — of which `worker.rs:1341` is already a **floor** (`>= 1`), not
  an equality, because the live-child count is nondeterministic. `tmux.rs:581`
  (`the_three_new_counters_reset_with_the_rest`) is the repo's guard against a counter that
  was added to the set but forgotten in `reset_all`.
- **Purity convention.** `classify_with` (`agent.rs:118`) takes its impure input as a closure
  so the gate is testable with no syscall (D13(3)); `walk` (`agent.rs:174`) is generic over
  its providers. A new source should arrive the same way.
- **`classify_with` has 8 existing test call sites** (`agent.rs:263,276,292,299,307,317,340,353`)
  plus the one production caller (`:213`). Any change to its arity edits all eight.
- **Test feasibility (measured).** A test can copy a locally built binary to `<tmp>/agy`, run
  it, rename it, and unlink it — an ad-hoc/linker-signed Mach-O keeps running from a deleted
  path. Copying `/bin/sleep` does **not** work (a platform binary's signature does not survive
  the copy; the child is killed before it runs), so the child must be the test binary itself
  (`std::env::current_exe()`). A libtest binary spawned with **no arguments runs the entire
  suite**, so the child invocation must name one test.
- CI runs `cargo test --workspace` on `macos-14` (`.github/workflows/ci.yml:11,35`), so a
  test that spawns a real child runs on the same platform class as the product.

## Decisions

### D1: When `exe_path` is absent, fall back to the kernel's exec-time process name
- **Status**: resolved
- **Decision**: `classify_pid` reads `proc_pidinfo(PROC_PIDTBSDINFO).pbi_name` — via a new
  `proc::name_of_pid` — **only** when `proc::exe_path` returned `None`.
- **Rationale**: the name is the one identity the kernel fixes at `exec` and never revisits.
  Measured across a rename *and* a deletion of a live pid's binary, `pbi_name` stayed `[agy]`
  while `proc_pidpath` first followed the rename and then fell to `rc=0`. So the fallback is
  not merely "something that survives deletion" — it is the pre-rename, pre-deletion name the
  agent tables were written against. It costs one `proc_pidinfo` on a struct `proc.rs` already
  reads twice (`tty_dev_of_pid:113`, `start_ms_uncounted:174`), so it adds a field read, not a
  syscall surface — the phrasing `proc.rs:147` already uses for this exact move.

  **Rejected — generalize `tabsession::is_claude` (`tabsession.rs:585`) to the whole agent
  table.** This is the strongest counter-proposal, and it deserves the space: the repo
  *already* owns an identity source that survives deletion (argv[0] from a `/bin/ps` table),
  it is exactly why Claude tabs never showed this bug, and reusing it would add zero syscalls,
  zero FFI and zero counters — where D1 introduces a *third* mechanism for one question, the
  shape `proc.rs:160-172` records as having already produced two defects. It is rejected on
  one fact: `ProcTable::read()` forks `/bin/ps`, which `tabsession.rs:157` itself calls "far
  too expensive for a per-tick worker" — and the **sidebar skin**, one of the two reported
  symptoms, is produced by exactly that per-tick worker. The argv floor can only ever fix the
  panel half. A fix that repairs one symptom and leaves the other is not this bug's fix.

  **Rejected — argv[0] via `KERN_PROCARGS2`**: it would recover the full path and therefore
  stage 1b as well, but it is precisely the `sysctl` that the D5/R4.4 interpreter gate exists
  to avoid. The objection is **per-call size, not population**: `proc_pidinfo` fills a fixed
  136-byte struct (measured: the call returns `n=136`), while `proc_args`
  (`proc.rs:229-252`) allocates a `KERN_ARGMAX`-sized buffer per call. (Stated this way
  deliberately: D8 accepts the new read partly *because* the affected population is small, so
  "the population is large" cannot also be the reason to reject this one.)

  **Rejected — name-then-argv chain**: the widest recovery, but it buys back only stage 1b,
  whose sole real case (`claude`'s `versions/<ver>`) is a declared Non-goal, at the price of
  two failure paths where one will do.

  **Rejected — `pbi_comm`**: it would in fact work for the observed case (`agy` is 3 chars),
  so this is a margin choice, not a correctness one: `pbi_comm` truncates at `MAXCOMLEN` = 16
  where `pbi_name` gives 32, and a longer future agent name should degrade into "unknown",
  not into a silently different string.

### D2: The fallback substitutes a basename and re-enters the existing ladder — it is not a new ladder
- **Status**: resolved
- **Decision**: the recovered name is used wherever `path.file_name()` is used today.
  Stage 1 (exact table match) is the stage that fires in the observed case, because the
  exec-time name is the un-renamed `agy`. Stage 1a (leading dot-component) still applies and
  is left in place for a process exec'd directly from an already-renamed file. Stage 1b
  (`<agent>/versions`) is skipped — it needs a path, and there is none. Stage 2 stays behind
  the same `is_interpreter` gate, now keyed on the recovered name.
- **Rationale**: the bug is that the ladder is unreachable, not that the ladder is wrong.
  Restoring its input preserves every exactness property those stages were reviewed for and
  keeps this change from re-litigating them.
  The Tier-3 tension this creates is accepted **and must be pinned in both directions**: a
  process with no readable path whose name is `node` now reaches the `sysctl`, where before it
  reached nothing. That is the gate behaving as specified on an input it previously never
  received. The existing gate test (`agent.rs:333`) passes `Some(Path::new(base))` and so does
  not cover the new branch at all, so this decision commits L3 to deriving two assertions
  (there is no Requirements section yet — this is a forward commitment, not a back-reference):
  - `exe_path == None` + recovered name `"rustc"` ⇒ the argv reader is **not** invoked
    (`PROC_ARGS` unchanged) — the gate still closed on the new branch;
  - `exe_path == None` + recovered name `"node"` ⇒ the argv reader **is** invoked — the
    accepted widening, pinned as a fact rather than conceded in prose.
  **Rejected — reconstructing a synthetic path from the name** so stage 1b could run: it
  would fabricate a path component the kernel never reported, which is the exact failure
  `antigravity::conversation_id_of` documents refusing (a scraped id that confidently names
  the wrong thing).

### D3: The seam is a new `classify_with_name`; `classify_with` keeps its arity
- **Status**: resolved
- **Decision**: add `classify_with_name(exe_path, read_name, read_argv)` holding the staged
  logic, and keep `classify_with(exe_path, read_argv)` as a thin wrapper that calls it with
  `|| None`. `classify_pid` calls the three-argument form. Both name sources arrive as
  lazily-invoked closures; `read_name` is called only on the `exe_path == None` branch.
- **Rationale**: two constraints meet here, and an earlier draft violated one of them without
  noticing. Adding a third parameter to `classify_with` would edit all **eight** existing test
  call sites (`agent.rs:263-353`) — the precise "an existing test needed modifying" signal the
  Constraints block calls the sign that the fix has widened. The wrapper keeps every one of
  them byte-identical while the new tests drive the new seam directly. The split is also the
  repo's own idiom for exactly this situation: `start_ms_of_pid` / `start_ms_uncounted`
  (`proc.rs:150,174`) and `conversation_of_pid` / `conversation_of_pid_in`
  (`presence_fd.rs:79,88`).

  **The wrapper carries `#[allow(dead_code)]`.** Once `classify_pid` calls the three-argument
  form, `classify_with`'s only remaining callers are the eight inside `#[cfg(test)] mod tests`;
  `crates/app` has no crate-level `#![allow(dead_code)]` (`main.rs:1-12`), so the non-test
  compilation in `cargo clippy --workspace --all-targets` would emit `dead_code` on a
  `pub(crate)` fn and the `-D warnings` gate would fail. The trap this sets is worth naming: the
  obvious way out at L4 is to point `classify_pid` back at `classify_with`, which dissolves this
  decision entirely.
  `#[allow(dead_code)]` over `#[cfg(test)]`, and the deciding fact is `tmux.rs:232`, which
  carries an intra-doc link `[`super::agent::classify_with`]`. A `cfg(test)` gate would break
  that link in every non-test build; the attribute keeps the item present in all of them. It is
  also the repo's established answer for this exact situation — `presence_fd.rs:78,87` carries
  it on `conversation_of_pid` / `conversation_of_pid_in` for the same reason.
  Closures rather than eager values, for the reason `agent.rs:118` already gives: eager
  evaluation would make the new read unconditional, silently doubling the per-pid cost of
  every walk (the thing D8 exists to prevent) and making it impossible to write the pure test
  that proves the read did *not* happen.

### D4: Decode the name defensively; empty or unreadable is absence, not an error
- **Status**: resolved
- **Decision**: `name_of_pid` reads **at most 32 bytes** of `pbi_name`, truncates at the first
  NUL **if there is one**, decodes with `std::str::from_utf8().ok()`, and returns `None` for a
  short read, a failed call, an invalid-UTF-8 name, or an empty result. `classify_with_name`
  then returns `None` exactly as `classify_with` does today.
- **Rationale**: `libc` declares `pbi_name: [c_char; 32]` (`apple/mod.rs:620`), so a 32-byte
  name arrives **unterminated**; a naive `CStr::from_ptr` would read past the field into
  `pbi_nfiles`. `from_utf8().ok()` is what `proc::exe_path` (`proc.rs:89`) already does.

  **This deliberately diverges from the tree's fixed-C-array idiom, and the divergence is the
  whole point of writing D4 down.** Four sites — `proc.rs:49`, `presence_fd.rs:190`,
  `session.rs:286`, `tabsession.rs:500` — all spell it:

  ```rust
  let end = bytes.iter().position(|&b| b == 0).unwrap_or(0);   // NOT this, here
  ```

  which yields `None` for an unterminated buffer. That is correct for those four, because every
  one of them reads a **path** (`vip_path` is documented as NUL-terminated at
  `presence_fd.rs:184`), and for a path a missing terminator means a corrupt read. For a
  **name** a missing terminator means a maximal 32-byte name — the exact case D4 exists to
  handle. Copying the nearest idiom would therefore produce a `name_of_pid` that returns `None`
  for precisely the input this decision was written for, and neither review nor D6's test would
  catch it (`agy` is 3 characters). The rule here is
  `.unwrap_or(bytes.len())`.

  Absence-on-failure otherwise follows the module's standing contract (`agent.rs:117`: "a
  failed argv read (empty) means 'not an agent'"; R5.4/R5.6: a dead pid's walk simply carries
  on). **Measured, not merely argued**: an unreaped zombie (`ps` state `ZN`) returns `rc=0`
  from `proc_pidinfo` with an empty name, alongside `proc_pidpath` `rc=0` — so the fallback
  cannot resurrect a corpse as a live agent, which is the sharpest attack on D1 and the one
  a reader will try first.

### D5: One new counted primitive, `proc::name_of_pid`, with its own probe counter and reset guard
- **Status**: resolved
- **Decision**: add `proc::name_of_pid(pid) -> Option<String>` beside the two existing
  `PROC_PIDTBSDINFO` readers, `probe::PROC_NAME` to the counter set **and** to `reset_all`
  (`mod.rs:459`), plus a reset guard in the shape of `tmux.rs:581`.
- **Rationale**: `mod.rs:438` states the convention outright — "uncounted I/O is exactly what
  lets a cost test keep passing while the real cost grows" — and `tmux.rs:582` states why the
  reset guard is separate: "a counter missing from `reset_all()` would silently carry state
  between cost tests and make every later assertion meaningless."
  On assertion strength, the honest rule (an earlier draft overclaimed here): `PROC_NAME` is
  **not** private to `tabmeta`. `tabsession::agent_for` (`tabsession.rs:164`) is an outside
  caller and reaches `classify_pid`, with three consumers — `overlay.rs:396`, `overlay.rs:820`
  (a background thread) and `presence_fd.rs:386`. What makes an absolute assertion sound today
  is weaker and must be written down as such: none of those is reached by a default-suite test
  (`presence_fd.rs:386` is inside an `#[ignore]`d manual gate; the overlay paths need a winit
  loop). Any test that asserts on `PROC_NAME` while a live walk is in flight must therefore
  use a floor or a delta, exactly as `PROC_START`'s doc (`proc.rs:155-172`) and
  `worker.rs:1341` already do.

### D6: The regression test reproduces the production sequence — exec as `agy`, rename, unlink
- **Status**: resolved
- **Decision**: a test copies `std::env::current_exe()` to `<tmp>/agy`, spawns it, **waits
  until the child has actually exec'd** — polling `proc::exe_path(child)` until it reads
  `Some(<tmp>/agy)`, with a bounded timeout that fails loudly — then **renames** it to
  `<tmp>/agy.<id>.old`, **unlinks** it, and asserts that `classify_pid(child)` is
  `Some(AgentKind::Antigravity)` while `proc::exe_path(child)` is `None`. It also asserts the
  recovered name is the exact `agy`, so the stage the fallback lands on is pinned. It runs in
  the default suite, not `#[ignore]`d.

  The readiness wait is not defensive padding: `Command::spawn` returns once fork/exec is
  *initiated*, so an `unlink` that wins the race against the child's `execve` leaves the child
  never starting — nondeterministically, and more often on a loaded CI runner than locally.
  (The manual experiment this decision transcribes needed a `sleep` in exactly that position.)
  It also costs nothing, because it doubles as the "before" half of the sequence: the test now
  reads `exe_path == Some(<tmp>/agy)` → rename → `unlink` → `exe_path == None` →
  `classify_pid == Antigravity`, which pins the whole production sequence rather than its end
  state alone.

  **`<tmp>` must be canonicalized once, on our side, before the expected path is built** —
  `let tmp = std::fs::canonicalize(std::env::temp_dir())?;` and compare against
  `tmp.join("agy")`. Without it the readiness poll can never pass and the test fails 100% of
  the time, including on CI: `std::env::temp_dir()` on macOS is `$TMPDIR` =
  `/var/folders/…/T/`, while `proc_pidpath` reports the kernel's fully resolved
  `/private/var/folders/…/T/agy` (measured: spawned from `/var/folders/…/T/agy`, reported as
  `/private/var/…`, `rc=60`). `worker.rs:543` shows `std::env::temp_dir()` is the `<tmp>` this
  suite reaches for, so this is the path L4 will take by default.
  This follows `presence_fd.rs:88-96`, which canonicalizes its own side for this exact reason
  and names the trap as one the tree "documents having shipped once already".
- **Rationale**: the failure is a *syscall returning nothing*, which no pure test can
  reproduce and no reviewer can eyeball — the previous two instalments of this same bug
  (`agent-skin-detection-fix`, `antigravity-agy-detection`) were both closed on a live-process
  probe run by hand, and this defect is exactly what shipped through that gap afterwards.
  The **rename step is not decoration**: without it the test pins "an exec-time name survives
  unlink", which is a weaker and different property than the one that failed in production
  ("a binary exec'd as `agy`, renamed by the updater, then deleted, still classifies").
  Feasibility is measured, not assumed, and CI is `macos-14` running `cargo test --workspace`.
  **Rejected — pure injection plus an `#[ignore]`d manual gate** (`presence_fd.rs`'s shape):
  it cannot fail in CI, which is precisely how this class of bug reached a user twice.

### D7: The child is a named `#[ignore]`d sleeper, serialised and reaped by a guard
- **Status**: resolved
- **Decision**: the spawned copy is invoked as
  `["--exact", "tabmeta::agent::tests::deleted_binary_child_sleeper", "--ignored", "--nocapture"]`
  — the full path spelled out, `tests::` segment included — naming an `#[ignore]`d test whose
  body sleeps a bounded few seconds and returns. The test holds `probe::serial()` across the
  child's **entire** lifetime (spawn → assert → reap), holds the child in a drop-guard that
  `kill`s **and** `wait`s on unwind, and asserts the child is still alive (`try_wait()` is
  `None`) at assert time. The assertion calls `classify_pid(child)` directly, never
  `agent_of_pid(std::process::id())`.
- **Rationale**: three distinct hazards, each fatal on its own.
  1. **Fork bomb.** `current_exe()` is the libtest harness; spawning it with no arguments runs
     the whole suite — including this test, which spawns another copy, recursively. Naming one
     `#[ignore]`d test is the only invocation that does not, and libtest offers no `main` hook
     to intercept before tests run.
  2. **A pre-existing test breaks.** `worker.rs:501`
     (`a_tab_with_no_claude_agent_pays_nothing_for_identity`) walks `std::process::id()`'s live
     children and asserts `meta.agent.is_none()`. A live direct child named `agy` would make it
     fail — the exact D10 violation this spec forbids. That test already holds
     `probe::serial()`, so holding the same lock across the child's lifetime makes the overlap
     unrepresentable rather than unlikely.
  3. **Counter flake.** The child's classification bumps `PROC_EXE` and `PROC_NAME` on a live
     pid. Un-serialised, it could land between another cost test's `reset_all()` and its
     `PROC_EXE == 0` assertion (`worker.rs:485`) — the flake `proc.rs:155-172` documents having
     already been reproduced once at `--test-threads=2`.
  4. **A filter typo fails open.** `--exact` with a name libtest does not have runs zero tests
     and exits 0 immediately; the child then dies at once and the parent classifies a pid that
     no longer exists, failing with a message that points at the classifier instead of at the
     typo. The liveness assertion is what makes a filter typo read as a filter typo. (Checked:
     nothing in the suite or CI asserts a test count or enumerates test names — the
     "N tests green" strings at `worker.rs:536` and `overlay.rs:4459` are prose in comments —
     and `presence_fd.rs`'s manual gate is invoked with its own filter, so a second `#[ignore]`d
     test disturbs neither.)
  Asserting through `classify_pid` rather than `agent_of_pid(self)` keeps the test's subject
  unambiguous: it names the pid under test instead of depending on whatever else is running.

### D8: The extra read happens only where the old code learned nothing, and is bounded
- **Status**: assumed
- **Decision**: accept one additional `proc_pidinfo` per pid **whose `exe_path` is
  unreadable**, and nothing added to the common path. The bound is the walk's existing
  `MAX_NODES = 64` (`agent.rs:27`).
- **Rationale**: the reachable population is small and mostly self-limiting — a binary
  deleted under a running process, a pid that died between the child listing and the probe,
  and a process we may not inspect. In the worst case a walk pays 64 extra fixed-size
  `proc_pidinfo` calls, the same order as the `PROC_EXE` reads it already pays. Existing cost
  assertions keep their present form (equalities where they are equalities, and the floor at
  `worker.rs:1341` where it is already a floor) because a readable `exe_path` never reaches the
  new branch (D1, made lazy by D3).

### D9: A process whose path is permission-blocked becomes classifiable, and that is accepted
- **Status**: assumed
- **Decision**: no special-casing for `EPERM`. If `exe_path` fails because the process is
  not ours to inspect (e.g. an agent launched under `sudo`) and the name is still readable,
  it classifies.
- **Rationale**: it is the same exact-match rule applied to a process the old code could not
  see at all, so it cannot produce a *different* verdict than the name rule already gives —
  only a verdict where there was none. Truncation cannot manufacture a false positive either:
  every table entry is far shorter than the 32-byte `pbi_name` buffer, so a longer name can
  never truncate *into* an exact match.

### D10: A readable `exe_path` changes nothing
- **Status**: resolved
- **Decision**: when `proc_pidpath` succeeds, classification is byte-for-byte what it is
  today — same stages, same order, same syscalls, same verdict.
- **Rationale**: this is the property that makes the change reviewable at all. Every existing
  test in `agent.rs`, `worker.rs`, `activity.rs` and the sidebar suite must pass unmodified;
  if any of them needs editing, the change has widened past its bug. D3's wrapper and D7's
  serialisation are the two places where that property was nearly lost.

## Constraints
- macOS ARM64 only; `objc2`/`libc` syscalls are fine, `#[cfg(target_os)]` branches are not
  (`.claude/rules/macos-only.md`).
- The change stays inside `crates/app` — `proc_pidinfo` cannot move into `ai-monitor` or
  `term-core` (`.claude/rules/dependency-direction.md`; the same reasoning `presence_fd.rs`
  records as AG11).
- No new crate dependencies. `libc` is already a dependency and already exports
  `PROC_PIDTBSDINFO` and `proc_bsdinfo`.
- No `unwrap()` outside tests; per-crate `thiserror` error style (`CLAUDE.md`).
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace` must all pass.
- **No pre-existing test may be modified** — not its assertions, not its inputs, and (by D3)
  not even its call syntax. Editing one to keep it green is the signal that the fix has
  widened.
- Nothing under `~/.gemini` or `~/.claude` is written, and no new process is spawned by the
  product — only by the test, under D7's constraints.

## Known Gaps
- A deleted `claude` versioned-install binary (`~/.local/share/claude/versions/<ver>`) stays
  undetected by the sidebar skin, because stage 1b needs a path the fallback cannot supply
  (D1/D2). The panel is unaffected — `tabsession::find_claude`'s argv floor covers it. Named
  as a Non-goal, recorded here so the asymmetry is not rediscovered as a new bug.
- `Probed::NoAgent` remains unreachable in production, so `ai-monitor`'s `history.jsonl` tier
  stays dead code. Out of scope by the user's decision at L0; worth its own spec.
- This fix does not make detection robust against the *next* thing `agy`'s updater changes.
  It is the third instalment of one lesson (rename → `versions/` → deletion); if the updater
  ever re-execs under a different name, the exec-time name moves with it and D1 still holds,
  but a wrapper script or an interpreter relaunch would not be covered.
- L2 provisional: Error/Edge — the reachable population of "unreadable `exe_path`" processes
  is argued from three named cases (deleted binary, dead pid, foreign process) rather than
  measured over a live session. D8's bound holds regardless of the count.

## Requirements

### R0: A live agent whose executable was deleted is still identified

#### R0.1: The reported failure no longer happens
- **Given**: a live process exec'd from `<tmp>/agy`, whose file was renamed to
  `<tmp>/agy.<id>.old` and then unlinked — the production sequence `agy`'s updater performs
- **When**: `tabmeta::agent::classify_pid(child)` is called
- **Then**: it returns `Some(AgentKind::Antigravity)`, where on the current code it returns
  `None`

#### R0.2: The walk composes with the fallback
- **Given**: a fake process tree whose depth-1 node classifies only through the name fallback
- **When**: `walk(root, children, classify)` runs
- **Then**: it returns `Detected { kind, pid }` for that node, with the same first-match and
  depth ordering it has today

### R1: `proc::name_of_pid` — the kernel-name primitive (D1, D4, D5)

#### R1.1: A live process reports its exec-time name
- **Given**: a live child exec'd from a file named `agy`
- **When**: `proc::name_of_pid(child)` is called
- **Then**: it returns `Some("agy")`

#### R1.2: The name survives both the rename and the deletion
- **Given**: that same child, after its file is renamed to `agy.<id>.old` and then unlinked
- **When**: `proc::name_of_pid(child)` is called at each step
- **Then**: it returns `Some("agy")` at every step — the exec-time name, never the renamed one
  (this is what distinguishes the fallback from `proc_pidpath`, which reports the rename and
  then fails)

#### R1.3: A pid with no live process yields absence
- **Given**: a pid that is dead, unreaped (a zombie), or never existed
- **When**: `proc::name_of_pid(pid)` is called
- **Then**: it returns `None` — a short read from `proc_pidinfo` is absence, and no corpse can
  be reported as a running agent

#### R1.4: A maximal, unterminated 32-byte name decodes in full
- **Given**: a `pbi_name` buffer holding exactly 32 non-NUL bytes (the case `libc`'s
  `[c_char; 32]` allows and the tree's `unwrap_or(0)` idiom would reject)
- **When**: the decode step runs — a pure function over the buffer, so this is testable with
  no process
- **Then**: it returns all 32 characters, not `None`; and for a buffer with an embedded NUL it
  returns the bytes before it; and for an empty or invalid-UTF-8 buffer it returns `None`

#### R1.5: Every name read is counted exactly once
- **Given**: `probe::reset_all()` has just run
- **When**: `proc::name_of_pid` is called `n` times
- **Then**: `probe::PROC_NAME.get() == n`

#### R1.6: The new counter is reset with the rest
- **Given**: `PROC_NAME` has been bumped
- **When**: `probe::reset_all()` runs
- **Then**: `PROC_NAME.get() == 0` — the guard `tmux.rs:581` exists to provide

### R2: The classifier consumes the name through the existing ladder (D1, D2, D3, D9)

#### R2.1: An absent path plus an exact agent name classifies — stage 1
- **Given**: `exe_path` is `None` and the recovered name is `agy`
- **When**: `classify_with_name(None, || Some("agy"), argv)` runs
- **Then**: it returns `Some(AgentKind::Antigravity)`, via the exact stage-1 table match

#### R2.2: Stage 1a still applies to a recovered name
- **Given**: `exe_path` is `None` and the recovered name is `agy.1784614720971820000.old`
  (a process exec'd directly from an already-renamed file)
- **When**: `classify_with_name` runs
- **Then**: it returns `Some(AgentKind::Antigravity)` via the leading dot-component, and
  `agyx.7.old` still returns `None`

#### R2.3: The `sysctl` gate stays closed on the new branch
- **Given**: `exe_path` is `None`, the recovered name is `rustc`, and `probe::reset_all()`
  has just run
- **When**: `classify_with_name` runs
- **Then**: it returns `None` **and** `probe::PROC_ARGS.get() == 0` — the argv reader was
  never invoked

#### R2.4: The accepted widening is pinned, not merely conceded
- **Given**: `exe_path` is `None` and the recovered name is `node`
- **When**: `classify_with_name` runs with an argv closure naming a packaged agent
- **Then**: the argv closure **is** invoked and the agent classifies — the one new input the
  interpreter gate now receives, stated as a fact so a later change cannot silently remove it

#### R2.5: A readable path never triggers the name read
- **Given**: `exe_path` is `Some("/usr/bin/claude")`
- **When**: `classify_with_name` runs with a name closure that records whether it was called
- **Then**: it returns `Some(AgentKind::Claude)` and the name closure was **not** called

#### R2.6: Neither source means not an agent
- **Given**: `exe_path` is `None` and the name closure returns `None`
- **When**: `classify_with_name` runs
- **Then**: it returns `None`, and the argv closure was not called

#### R2.7: The wrapper leaves every existing call site untouched
- **Given**: the eight existing `classify_with(...)` call sites in `agent.rs`'s test module
- **When**: the change lands
- **Then**: all eight are byte-identical to their current text, `classify_with` keeps its
  two-argument signature, and `cargo clippy --workspace --all-targets -- -D warnings` is clean
  (the wrapper carries `#[allow(dead_code)]`, and `tmux.rs:232`'s intra-doc link still resolves)

#### R2.8: The branch does not ask *why* the path was unreadable
- **Given**: two pids whose `exe_path` is `None` for different reasons — one a deleted binary,
  one a process the caller may not inspect
- **When**: each is classified
- **Then**: both take the identical path, with no `EPERM`-specific branch anywhere in the
  code — the verdict depends only on the recovered name

### R3: The regression test reproduces the production sequence (D6, D7)

#### R3.1: The test pins the whole sequence, not its end state
- **Given**: a copy of `std::env::current_exe()` at `<tmp>/agy`, where `<tmp>` is
  `std::fs::canonicalize(std::env::temp_dir())`
- **When**: the test spawns it, waits for readiness, renames it to `<tmp>/agy.<id>.old`,
  and unlinks it
- **Then**: it asserts `exe_path == Some(<tmp>/agy)` before the rename, `exe_path == None`
  after the unlink, `name_of_pid == Some("agy")`, and
  `classify_pid == Some(AgentKind::Antigravity)`

#### R3.2: The test cannot race the child's `exec`
- **Given**: `Command::spawn` has returned, which only means fork/exec was initiated
- **When**: the test polls `proc::exe_path(child)` before renaming
- **Then**: it proceeds only once the path reads `Some(<tmp>/agy)`, and on a bounded timeout it
  fails with a message naming the readiness step rather than the classifier

#### R3.3: The child is exactly one named test, and its death is detectable
- **Given**: the copy is spawned as
  `["--exact", "tabmeta::agent::tests::deleted_binary_child_sleeper", "--ignored", "--nocapture"]`
- **When**: the test reaches its assertions
- **Then**: `try_wait()` reports the child still running — so a filter that matched nothing
  (libtest runs zero tests and exits 0) fails as a filter error, not as a classifier error

#### R3.4: The live child cannot disturb any other test
- **Given**: `worker.rs:501` asserts `meta.agent.is_none()` over the test process's live
  children, and several cost tests assert absolute counter values
- **When**: the deleted-binary test runs, including under `--test-threads=2`
- **Then**: it holds `probe::serial()` across the child's entire lifetime and reaps the child
  with a guard that `kill`s and `wait`s, so no other test can observe the child or its counter
  bumps

### R4: Nothing else changes (D8, D10)

#### R4.1: A readable path costs exactly what it costs today
- **Given**: a walk over processes whose `exe_path` is readable, after `probe::reset_all()`
- **When**: the walk completes
- **Then**: `PROC_NAME.get() == 0`, and `PROC_EXE` / `PROC_ARGS` hold the same values as before
  the change

#### R4.2: The existing suite passes unmodified
- **Given**: the current `agent.rs`, `worker.rs`, `activity.rs`, `tmux.rs` and sidebar tests
- **When**: `cargo test --workspace` runs on the changed tree
- **Then**: every one of them passes with no edit to its assertions, its inputs, or its call
  syntax — the property that bounds this change to its bug

#### R4.3: The gates are green
- **Given**: the changed tree
- **When**: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` and `cargo build --workspace --release` run, as CI runs them
- **Then**: all four succeed

## Tasks

### T1: The kernel-name primitive and its cost counter [infra]
- **Fulfills**: R1
- **Depends on**: (none)
- **Files**: `crates/app/src/tabmeta/proc.rs`, `crates/app/src/tabmeta/mod.rs`
- Add `proc::name_of_pid(pid) -> Option<String>` reading `proc_pidinfo(PROC_PIDTBSDINFO)`
  beside `tty_dev_of_pid` and `start_ms_uncounted`, with the decode split into a **pure
  function over the 32-byte buffer** so R1.4 is testable without a process. Add
  `probe::PROC_NAME` to the counter set and to `reset_all`, plus a **new** reset-guard test in
  `proc.rs`'s own test module — `tmux.rs:581`'s guard is a pre-existing test and must not be
  edited (Constraints).
- Horizontal by nature: a syscall primitive with no consumer yet, which is the one shape the
  vertical-slice rule exempts.

### T2: The classifier consumes the name [vertical]
- **Fulfills**: R0.2, R2
- **Depends on**: T1 — it calls `name_of_pid`, and the two tasks would otherwise disagree
  about the primitive's signature
- **Files**: `crates/app/src/tabmeta/agent.rs`
- Add `classify_with_name(exe_path, read_name, read_argv)` holding the staged logic; keep
  `classify_with(exe_path, read_argv)` as a `#[allow(dead_code)]` wrapper that calls it with
  `|| None`; point `classify_pid` at the three-argument form. Add the pure tests R2.1–R2.6 and
  R2.8 and the walk-composition test R0.2. **No existing test in this file is touched** — that
  is R2.7, and it is the acceptance criterion that tells this task it stayed in scope.

### T3: The live regression test, the cost pin, and the gates [vertical]
- **Fulfills**: R0.1, R3, R4
- **Depends on**: T2 — it asserts the fixed behaviour, and it edits the same file (the file
  overlap alone forces the ordering; parallel workers would conflict in `agent.rs`)
- **Files**: `crates/app/src/tabmeta/agent.rs` (test module only)
- Add the `#[ignore]`d `deleted_binary_child_sleeper`, the drop-guard that kills **and** waits,
  and the spawn → readiness-poll → rename → unlink → assert test, with `<tmp>` canonicalized
  once and `probe::serial()` held across the child's whole lifetime. Add the R4.1 cost pin
  (`PROC_NAME == 0` over a walk of readable-`exe_path` processes). Then run the four CI gates
  (R4.3) and confirm the untouched suite is green (R4.2).
- There is deliberately no fourth "final verify" task: holistic verification against
  `## Requirements` belongs to the execute pipeline, and the per-task gates above are what this
  task owns.

**The DAG is a chain, and that is the honest shape.** Three tasks over two files, each
consuming the previous one's output: T1's signature is T2's input, and T3 asserts T2's
behaviour while editing T2's file. There is no pair here that satisfies all three parallelism
conditions, so no parallel edge is claimed.

## External Dependencies

### Pre-work
- (none) — `libc` already exports `PROC_PIDTBSDINFO` and `proc_bsdinfo`, CI already runs the
  four gates on `macos-14`, and every measurement this spec rests on is already taken.

### Post-work
- **The manual gate this bug earned.** Automated tests prove the mechanism; they cannot prove
  the product. With a real `agy` session running, delete the running binary
  (`rm ~/.local/bin/agy.*.old`, or wait for the updater to do it), then confirm the ai-monitor
  panel still shows that session **and** the sidebar card keeps its Antigravity skin. Both
  symptoms, because both consumers read the same detector.
- Record the lesson beside the two it continues (`specs/antigravity-agy-detection/loop.md`,
  `specs/agent-skin-detection-fix/loop.md`): an agent CLI's identity must not be read from a
  path that its own updater owns — `proc_pidpath` follows a rename and dies on a delete, while
  the exec-time name does neither.
