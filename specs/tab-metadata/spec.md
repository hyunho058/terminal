# Spec: tab-metadata

## Meta
- **Created**: 2026-07-20
- **Type**: dev
- **Status**: ready
- **Approved by**: batch mode — the human approved the partition (features, declared surfaces, `depends_on` edges, SD1–SD8) at decompose's single gate; specify's L0/L2/L3/L4 prompts are bypassed per `pre-approved-batch: yes`
- **Approved at**: 2026-07-20
- **Execution**: NOT handed off. Batch mode's deliverable is this file; `decompose` → `build-order` owns execution.
- **Set**: sidebar-redesign (`specs/sidebar-redesign/partition-manifest.md`)
- **Mode**: batch (partition approved at decompose's single human gate)
- **L2 review**: the L2-reviewer subagent ran and returned NEEDS_FIX, naming six open items in one pass — the read half of the seam was never examined; eight cost claims asserted syscall counts / read caps with no observation mechanism; `MAX_CHILDREN` had no reachable test; R1.5 contradicted D9's memo keying; `set_active` shipped two contradictory timings; and background tabs would burst on one tick. All six were turned into checkpoints and addressed (the count rose to 45), as recorded in the scoring-honesty note under the checkpoint table. **The batch run was interrupted by a session rate limit before the final confirming review pass and before this Meta line was backfilled**, so the six fixes are transcriptions of the reviewer's own prescribed remedies, unconfirmed by a further pass. The cross-spec `coherence-audit` at decompose-end is the next independent check.

## Declared Surface
- feature-id: tab-metadata
- depends_on: []
- declared-surface:
  - crates/app/src/tabmeta/**
  - crates/app/src/main.rs
  - crates/ai-monitor/src/**

## Goal
Give every tab two pieces of per-tab metadata that the redesigned sidebar can render: the **git branch** of the tab's live working directory, and **whether an AI agent CLI is running in that tab** — which agent, and whether it is working or waiting. All of it computed on a background thread, cached, and published to the UI thread as plain app-side data, so no render frame ever performs filesystem or process I/O.

## Non-goals
- **Drawing anything.** This feature registers no visible surface and produces no pixels. Its only on-screen effect is via `sidebar-row-redesign`, which consumes the seam this feature publishes.
- **Editing `crates/app/src/tabs.rs`, `terminal_app.rs`, `session.rs`, `seam.rs`, or `input.rs`.** None is in the declared surface. Everything this feature needs from the app is obtained through `main.rs` and the existing `Overlay` seam (D3), or is reimplemented inside `tabmeta/` (D10).
- **Naming, importing, or depending on `renderer::sidebar_theme::AgentSkin`** — or any renderer type at all. This feature owns an app-side agent vocabulary; `sidebar-row-redesign` owns the mapping (D11, and `sidebar-foundations` D13).
- **A config surface.** `crates/common/src/config.rs` belongs to `sidebar-foundations`, and this feature is a `depends_on: []` root that may be built first. The display toggles reach the worker through a seam call, not through `common::config` (D12).
- **Reading the terminal's own scrollback**, or an OSC handshake with the agent, to detect agents (rejected in SD6).
- **A `git` subprocess or a git library** (rejected in SD5).
- **Tracking agents in tabs that are not ours.** Detection starts from *this tab's* zsh pid and walks down; a `claude` running in iTerm is invisible to us, correctly.
- **Rendering an agent's message content.** The existing ai-monitor overlay already does that. This feature reads the same logs for one bit — busy vs. waiting — and never surfaces text.
- **Cross-platform process introspection.** macOS `proc_*` / `sysctl` only (`.claude/rules/macos-only.md`).

## Confirmed Goal
Add a `crates/app/src/tabmeta/` module owning a single background worker thread plus a `Clone`, `Send + Sync` `TabMetaHandle`. The worker is given a roster of `(TabId, zsh pid)` pairs; on a two-rate cadence (active tab fast, background tabs slow) it resolves each tab's live cwd from its pid, derives the git branch by reading `.git/HEAD` directly under a cwd-keyed memo, and derives agent presence/identity by a **bounded** breadth-first walk of the pid's descendants classified by executable basename with an argv fallback for interpreter-launched agents. For a detected Claude agent it additionally derives a busy/idle activity bit from the session JSONL via a new bounded tail-read added to `crates/ai-monitor`. Results are published into a generation-stamped shared cache **only when something actually changed**, and the UI thread is woken through the existing `UserEvent::OverlayPending` path by a headless pump registered from `main.rs` through the existing `Overlay` seam — because `terminal_app.rs` is out of surface and no `UserEvent` variant can be added. Done when a tab in a git repo reports its branch, a tab running an agent reports which agent and its state, a plain zsh tab in a non-repo reports neither, every code path degrades to "absent" on any syscall or I/O failure without panicking, the UI thread performs zero filesystem or process I/O on this feature's behalf, and `cargo build` + `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` are green.

## Research

### `crates/ai-monitor` — what exists, and what it does *not* know
- `crates/ai-monitor/src/lib.rs` is 316 lines. Public API: `AiEvent { role, summary }` (`:16`), `encode_cwd` (`:25`), `claude_projects_dir` (`:33`), `resolve_log_in` (`:41`), `resolve_log` (`:47`), `parse_line` (`:73`), `backfill` (`:125`), `LogWatcher { _watcher, pub rx }` (`:140`), `LogWatcher::spawn(log, start_offset, wake: impl Fn() + Send + 'static) -> Option<LogWatcher>` (`:151`).
- **It has no notion of "is an agent running" and no state model at all** — it reads log lines. Every notion of agent *presence* and *state* in this spec is new.
- `encode_cwd` (`:25-30`) maps **only** `/` and `.` to `-`. It does not handle spaces or other punctuation. So `resolve_log_in` fails for a cwd containing a space; the doc comment says "non-alphanumeric path separators", which is broader than the code. Pre-existing; recorded in Known Gaps.
- `backfill` (`:125-136`) does `std::fs::read(log)` — it reads the **entire** file, then parses **every** line. For a long Claude session that is megabytes of read + JSON parse. It is correct for its own one-shot use at overlay-open, and completely unusable on a poll loop. This is the concrete reason D7 adds a bounded tail read rather than reusing `backfill`.
- `LogWatcher` owns **no thread of its own** — `notify` owns it (`:162-207`), it watches the **parent directory** (`:208`), and dropping the `LogWatcher` stops it.
- `LogWatcher::spawn`'s `wake: impl Fn() + Send + 'static` callback (`:151-155`, doc at `:148-150`: "`wake` runs on the watch thread — keep it cheap (e.g. an `EventLoopProxy::send_event`)") is the in-repo precedent for **the caller owning the wake**. D2 mirrors it.
- `crates/ai-monitor/Cargo.toml` dependencies are **exactly** `serde_json = "1"` and `notify = "8"`. No `common`, no `libc`, no `tracing`. **`Cargo.toml` is NOT in this feature's declared surface** (`crates/ai-monitor/src/**` is), and `Cargo.lock` edits are PreToolUse-hook-blocked — so anything added to `ai-monitor` must build on `std` + `serde_json` alone.
- Only `app` depends on `ai-monitor` (`crates/app/Cargo.toml:15`), and only `crates/app/src/overlay.rs:11` imports it. Adding a module there cannot break another crate.
- Test idiom in that crate: a hand-rolled temp dir (`lib.rs:220-226`) — `std::env::temp_dir()` + `format!("...-{}", std::process::id())` + `remove_dir_all` + `create_dir_all`. **No `tempfile` dev-dependency exists anywhere in the workspace.**

### The app side — what is reachable and what is not
- `crates/app/src/session.rs:20` — `pub struct TabId(pub u64)` derives `Hash` (`:19`), so it is usable as a `HashMap` key. It is `Copy`.
- `crates/app/src/session.rs:108-110` — `Session::cwd()` is `pub`; `crates/app/src/session.rs:142-167` — **`cwd_of_pid` is module-private**, and `session.rs` is **not** in this feature's declared surface. So the `proc_pidinfo` / `PROC_PIDVNODEPATHINFO` call must be reimplemented inside `tabmeta/` (D10).
- `libc = "0.2.186"` is a **direct** dependency of `app` (`crates/app/Cargo.toml:17`). Verified present in that exact version: `proc_listchildpids` (`.../libc-0.2.186/src/unix/bsd/apple/mod.rs:4901`), `proc_pidpath` (`:4923`), `proc_listpids` (`:4897`), `PROC_PIDPATHINFO_MAXSIZE = 4096` (`:3688`), `KERN_PROCARGS2 = 49` (`:3681`). `sysctl` is available via `libc::sysctl`.
- `crates/app/src/main.rs:10-17` holds the `mod` declarations — this feature adds `mod tabmeta;` there. `main()` is `:25-52`.
- **`crates/app/src/main.rs:44` — `app.register_overlay(Box::new(AiMonitorOverlay::new(proxy)))` MOVES `proxy`.** Anything registered afterwards needs a clone taken earlier; `:40` already does `proxy.clone()`.
- `crates/app/src/terminal_app.rs:33-43` — `UserEvent` has exactly three `Copy` variants: `PtyOutput(TabId)`, `ConfigReloaded`, `OverlayPending`. **`terminal_app.rs` is not in this feature's surface — no variant can be added.** The established pattern is a bare signal variant plus a separate `mpsc` channel carrying the payload (`terminal_app.rs:322-332` for `ConfigReloaded`; `overlay.rs:62-64` for `OverlayPending`).
- `crates/app/src/terminal_app.rs:333-338` — `UserEvent::OverlayPending` calls `inner.overlays.poll_all()` and calls `inner.redraw()` (`:336`) if it returns `true`. `OverlayHost::poll_all` (`seam.rs:169-175`) calls `poll()` on **every** registered overlay, visible or not. `OverlayHost::dispatch` (`:154-161`) skips overlays whose `visible()` is `false`. **Together these give a registered-but-never-visible overlay a UI-thread drain hook with zero key interference** — the mechanism D3 uses. Registration itself is `terminal_app.rs:146-150`: every pending overlay's `layer()` goes to `renderer.add_layer(..)` and the overlay into `overlays.register(..)`, and `renderer::lib.rs:717` sorts layers by `z_order()`, so the pump's `i32::MIN` places it below everything.
- `crates/app/src/session.rs:26` — **`Session.pty` is `pub`**, and `Pty::child_pid()` is `pub` (`term-core/src/pty.rs:63`). This is the fact that makes the roster buildable at all: `tabs.rs` can reach `s.pty.child_pid()` without touching `session.rs`, which is out of surface for every feature in the set.
- `crates/app/src/seam.rs:46-56` — `AppCtx` carries `writer`, `tab_count`, `active: usize`, `active_cwd`. **No `TabId`.** `Overlay::on_active_changed(&mut self, cwd: Option<PathBuf>)` (`:109`) is called from `OverlayHost::notify_active_changed` (`:178-182`), itself called from exactly one place — `terminal_app.rs:245-247` — and it carries **only the active tab's cwd**, no id. So the seam cannot deliver a `TabId`-keyed roster; the roster must come from the feature that owns `tabs.rs`.
- `crates/app/src/tabs.rs:138-151` — `Tabs::titles()` calls `s.cwd()` per tab, i.e. one `proc_pidinfo` syscall **per tab per call**. `crates/app/src/tabs.rs:154-156` — `active_cwd()` is one more. Call sites: `titles()` from `sync_tab_model` (`terminal_app.rs:218-223`), which runs at start (`:196`) and after every tab action (`:244`) — **not per frame**; `active_cwd()` from `terminal_app.rs:246` (tab switch) and **twice per keypress** (`:259`, `:272`). SD4's concern is therefore about *compounding* this per-tab syscall cost, not about an existing per-frame cost.
- `crates/app/src/tabs.rs` and `terminal_app.rs` are **not** in this feature's surface. Per the manifest, `sidebar-row-redesign` owns `tabs.rs` and `depends_on` this feature. `Tabs::new` is constructed at `terminal_app.rs:158`, which **no dependent feature owns** — this is the constraint that forces D2's locator.
- Canonical background-thread shape: `session.rs:50-67` — a named `thread::Builder`, an `mpsc::channel`, send-then-`proxy.send_event`, `let _ =` on the send, `break` on hangup. Drain side: `terminal_app.rs:309-340`, including a coalesce-to-latest loop (`:325-327`).
- `crates/term-core/src/pty.rs:63` — `Pty::child_pid()` returns **only** the direct zsh pid; `portable_pty` exposes no descendant or foreground-pgrp API. `Pty` is not `Clone` and is UI-thread-owned, so a worker must be handed the `u32` by value.

### Timing / cadence — there is no tick to hook
- `ControlFlow::Wait` is set **once**, at `main.rs:37`. `WaitUntil` and `Poll` appear **nowhere**; `about_to_wait` and `new_events` are unimplemented.
- Cursor blink does not use a timer — it piggybacks on the continuous redraw loop by sampling `Instant::elapsed()` per frame (`renderer/src/cursor_layer.rs:21,29,211`).
- Therefore **this feature must bring its own cadence** — a worker thread with its own sleep — rather than hooking an existing tick (D4).
- The app redraws **continuously** (`terminal_app.rs:421-430`, the winit#2640 workaround), paused only while occluded (`:429`). So a redraw is nearly free, but a *wake* that forces a re-snapshot (`redraw()` at `:203-212` sets `snapshot_dirty`) is not — which is why D4 publishes only on change.

### Constraints found in project docs
- `.claude/rules/dependency-direction.md` — `common ← term-core ← renderer ← app`. `term-core` is untouched by this feature.
- `.claude/rules/macos-only.md` — target `aarch64-apple-darwin`, use macOS APIs freely, **no `#[cfg(target_os = ...)]` branches**.
- `.claude/rules/zsh-local-only.md` — the PTY child is `/bin/zsh`; nothing here spawns a process or selects a shell.
- `CLAUDE.md` — `thiserror` error enums per crate; **no `unwrap()` outside tests**; gates are `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`.
- `.claude/settings.json` — PostToolUse `rustfmt` on edited `*.rs`; PreToolUse **blocks `Cargo.lock` edits** → no new crate dependency anywhere.
- App-crate test idiom: inline `#[cfg(test)] mod tests` with pure free functions extracted specifically to be testable without spawning a shell (`tabs.rs:4-5,160-185`); hand-rolled trait fakes (`seam.rs:199-255`); and — the direct precedent for testing a raw macOS syscall — `session.rs:169-185`, which calls `cwd_of_pid(std::process::id())` and asserts it equals `std::env::current_dir()`.
- The only process-spawning test in the workspace is `#[ignore]`d (`crates/term-core/tests/pty_smoke.rs:15`). **This feature adds no process-spawning test.**

## L2 Checkpoints

Complexity: **Complex (5 signals)**
- *External resource loading*: three distinct external sources — the kernel process table (`proc_listchildpids` / `proc_pidpath` / `proc_pidinfo` / `sysctl`), the git object store (`.git/HEAD`, possibly indirected through a `.git` **file**), and the Claude JSONL session logs.
- *3+ independent subsystems*: the bounded process walk, the git-HEAD reader, the JSONL activity reader, the worker scheduler/differ, and the seam + pump.
- *State machine / multi-stage flow*: agent state is derived through a multi-stage pipeline (pid → descendants → classification → identity → log resolution → activity), and both memos have their own validate/refresh lifecycle.
- *Async resources*: a background thread, a command channel, a shared generation-stamped cache, and a wake callback into the winit loop.
- *Multi-actor*: the UI thread, the worker thread, the kernel, the agent CLI process (which appears and vanishes without telling us), and the consuming sibling feature.

Brownfield adjustment applied: Core 30% · Scope 20% · Error 25% · Data 15% · Implementation 10%.

### Core Behavior (30%) — 10/10 resolved
- [x] **What "agent state" means, and from which source each part is derived** → D5 (presence + identity from the process walk), D7 (activity from the JSONL), D8 (the three-variant vocabulary). *The two axes are deliberately separate; neither source can answer the other's question.*
- [x] **Refresh cadence, and whether background tabs poll slower than the active one** → D4 (active 500 ms, background 2 s, idle roster blocks with zero wakeups)
- [x] **Publication is change-only, not tick-driven** → D4 *(added by Tier-3 D4 × SD4: an unconditional publish would set `snapshot_dirty` twice a second forever)*
- [x] Where the cadence comes from, given no timer exists anywhere → D4 (own worker thread; L1-verified there is no tick to hook)
- [x] **How the UI thread learns of a change without a new `UserEvent` variant** → D3
- [x] **The exact seam signature `sidebar-row-redesign` calls** → D2
- [x] **How the roster reaches the worker, given `Tabs` is constructed in a file no dependent feature owns** → D2 (the locator) *(added by Tier-1: `tabs.rs`'s owner cannot edit `terminal_app.rs:158`)*
- [x] **How the published snapshot reaches the draw path, given `TabBarModel`'s only struct literal and its `TabModelHandle` are both constructed out of the consumer's surface** → D2 (the consumer's own `main.rs`-registered model object owns the snapshot; the read half needs no locator) *(added on adversarial review: the first draft proved the write path was unreachable without a global and never examined the read path at all, then justified the global as the price of the whole seam rather than of two of its five methods)*
- [x] Git branch derivation, including detached HEAD and `.git`-as-a-file → D9
- [x] **How cwd reaches the worker without adding UI-thread syscalls** → D9/D10 (the roster carries pids, not cwds; the worker resolves cwd itself)

### Scope Boundaries (20%) — 7/7 resolved
- [x] This feature draws nothing and is inert without a consumer → D1, D3
- [x] `tabs.rs` / `terminal_app.rs` / `session.rs` / `seam.rs` are untouched → D2, D3, D10
- [x] **The agent vocabulary is app-side and never names `AgentSkin`** → D11 (the counterpart of `sidebar-foundations` D13)
- [x] **The display toggles are not read from `common::config` here** → D12 *(added by Tier-1: this feature is a `depends_on: []` root and may build before `sidebar-foundations`)*
- [x] Agents outside our own PTY subtree are out of scope → D5 (the walk is rooted at the tab's zsh pid)
- [x] The agent table is **closed** — a process is an agent iff it is in the table → D5
- [x] `ai-monitor`'s existing overlay behaviour is unchanged; only additive API → D7

### Error / Edge Cases (25%) — 14/14 resolved
- [x] **A pid dies mid-walk** → D6 (every syscall returning ≤ 0 means "no children" / "unknown"; the BFS continues with the remaining queue)
- [x] **The walk is unbounded if an agent spawns a large build** → D6 (depth ≤ 3, ≤ 64 visited nodes, ≤ 128 children per level, early exit on first match)
- [x] **pid reuse between roster push and walk** → D6 (bounded to one tick; the walk caches no pid across ticks)
- [x] **`cd` changes the branch with no process change** → D9 (cwd is re-resolved from the live pid every tick, so there is nothing to invalidate)
- [x] **`git checkout` changes the branch with neither a process nor a cwd change** → D9 (the memo validates on `.git/HEAD`'s `(mtime, len)`, not on cwd alone)
- [x] **`git init` in a directory already memoised as not-a-repo** → D9 (negative memo entries expire after 10 s)
- [x] **A JSONL read on a poll loop would re-read the whole file** → D7 (bounded 8 KiB tail; `backfill` explicitly rejected)
- [x] **A node/bun-launched agent has the wrong `proc_pidpath` basename** → D5 (stage-2 argv classification via `KERN_PROCARGS2`), with its own failure mode stated
- [x] **A detected agent for which no activity source exists** → D8 (`AgentState::Unknown`, with a stated consumer obligation)
- [x] **The worker thread panicking would silently kill the feature forever** → D14 (no `unwrap`, no indexing, no panic path; every failure maps to absent metadata)
- [x] **How is a syscall/IO-count claim observed at all?** → D13(6) (`#[cfg(test)]` `probe` counters; six requirements restated against them) *(added on adversarial review: eight cost claims asserted counts or read caps with no observation mechanism — the same defect the `sync()` contract had, unfixed one layer down)*
- [x] **`MAX_CHILDREN` sat below the closure seam, making it the one bound with no reachable test** → D6 (`parse_childpids` split out as a pure function, R5.5 restated against it)
- [x] **Every background tab refreshing on the same tick** → D4's stagger *(added on adversarial review by D4 × SD4: `should_refresh(is_active, tick_index)` had no tab identity, so 20 tabs would burst on one tick every 2 s)*
- [x] **A `"type":"user"` tool-result line as the last line of an abandoned session would read `Busy` forever** → D7's `STALE_LIMIT` bound *(added on adversarial review: `parse_line` takes top-level `type` before `/message/role` at `ai-monitor/src/lib.rs:79-84`, so Claude's tool results carry role `user`)*

### Data Model (15%) — 8/8 resolved
- [x] `TabMeta` shape, and making illegal states unrepresentable → D8 (`Option<Agent>`, not `Option<AgentKind>` + a parallel state field)
- [x] `AgentKind` variants and the exhaustiveness obligation at the mapping site → D11
- [x] `AgentState` variants, and the explicit absence of a `Done` variant → D8
- [x] **The published snapshot type, and its per-frame allocation behaviour** → D2 (`TabMetaSnapshot` is caller-owned and refilled only on a generation change) *(added by Tier-3 D2 × SD4)*
- [x] Branch string representation, including the detached-HEAD form and a length cap → D9
- [x] The roster payload — `(TabId, pid)` only, no cwd → D9/D10
- [x] `TailState` — what `ai-monitor` returns and what the worker (not `ai-monitor`) decides from it → D7
- [x] **Memo key and lifetime — cwd-keyed, so eviction cannot be per-tab** → D9 (`MEMO_TTL_TICKS` disuse reclamation + `MEMO_MAX` cap; `set_roster` evicts nothing) *(added on adversarial review: an earlier R1.5 asserted per-`TabId` memo eviction, which D9 explicitly rejects and which would pull a shared entry out from under a live tab)*

### Implementation (10%) — 6/6 resolved
- [x] Background thread shape: named `thread::Builder`, `mpsc`, `let _ =` send, break on hangup → L1 `session.rs:50-67` (`assumed: true`)
- [x] Raw macOS syscall + struct parse, tested against our own process → L1 `session.rs:142-185` (`assumed: true`)
- [x] Hand-rolled temp dirs; no `tempfile`; no `Cargo.lock` edit → L1 `ai-monitor/src/lib.rs:220-226`, `common/src/watch.rs:94-100` (`assumed: true`)
- [x] **The bounded walk is unit-testable with no syscalls at all** → D13 (the BFS is generic over a children-provider closure)
- [x] Caller-owned `wake` callback rather than an owned proxy → L1 `LogWatcher::spawn` (`ai-monitor/src/lib.rs:151`) (`assumed: true`)
- [x] No new crate dependency anywhere, including in `ai-monitor` → D7 (std + `serde_json` only)

**Composite: 1.00** (45/45 resolved; every dimension 1.00 ≥ 0.60 floor). Unknown/unknowns: 0 pending.

> **Scoring honesty note — and this table has now been wrong twice.** It first read 1.00 before the Tier-1 actor sweep found the roster had no reachable delivery path (→ D2's locator) and before Tier-3 found an unconditional publish would fight SD4 (→ D4's change-only rule). It read 1.00 **again** at 39/39, and an adversarial L2 review then named six more open items in one pass: the read half of the seam was never examined, eight cost claims asserted syscall counts or read caps with no observation mechanism, `MAX_CHILDREN` had no reachable test, R1.5 contradicted D9's memo keying, `set_active` shipped two contradictory timings, and background tabs would burst on one tick. Those are now checkpoints and the count is 45 — but the honest reading is that **the reviewer, not this table, is the gate**, and that a 1.00 means only "no checkpoint I can currently name is open". Three items a table cannot close at all — whether the user's `claude` is a native binary or a node script, whether `KERN_PROCARGS2` is readable under the app's entitlements, and whether 500 ms/2 s feels right — are recorded in Known Gaps rather than scored away.

### Unknown/Unknown Detection

**Tier 1 (Actor).** Actors: the *worker thread* (D1, D4, D14 ✓); the *UI thread* as a reader (D2, D3 ✓); the *kernel* as an unreliable source (D5, D6 ✓); the *agent CLI process*, which appears and vanishes without notice (D5, D8 ✓); the *sibling consumer* `sidebar-row-redesign` (D2, D11, D12 ✓); the *`ai-monitor` overlay*, an existing consumer of the crate this feature edits (initially **0 decisions**) → **+1 Scope checkpoint**: does adding a module to `ai-monitor` change anything for the overlay? → resolved: D7 is purely additive, adds no dependency, and touches no existing function. And the *`Tabs` owner* — the actor that must push the roster (initially 0 decisions) → investigating showed `Tabs::new` is constructed at `terminal_app.rs:158`, a file **no dependent feature owns**, so a constructor-injected handle is impossible → **+1 Core checkpoint** → **D2**'s locator.

**Tier 2 (Implication).**
- D4 (a background worker) → implies a shutdown story → D14 (hangup on the last handle drop, matching `session.rs:58-59`); and implies **what happens when nothing ever pushes a roster** → D4 (block on `recv()`, zero wakeups, feature inert).
- D5 (classify by executable basename) → implies **the npm/node install of Claude Code has the wrong basename**, which would make the flagship agent undetectable → **+1 Error checkpoint** → D5 stage 2.
- D5 stage 2 (`KERN_PROCARGS2`) → implies a new NUL-delimited binary parse and a new failure mode → **+1 Error checkpoint** → D5's stated degradation + D13's self-process test.
- D7 (activity from the JSONL) → implies a **log-resolution** cost (`resolve_log_in` does a `read_dir` + per-entry `metadata`) that is heavier than the tail read itself → **+1 Error/Impl checkpoint** → D7's second memo.
- D7 → implies non-Claude agents have **no** activity source → **+1 Data checkpoint** → D8's `Unknown`.
- D9 (branch from `.git/HEAD`) → implies `.git` may be a **file** (worktrees, submodules) → handled in D9; and implies a **negative** result also needs memoising or every non-repo tab pays a 40-`stat` ancestor walk twice a second → **+1 Error checkpoint** → D9's expiring negative memo.
- D2 (a published snapshot read by the sidebar) → implies a per-frame read path → Tier 3 below.
- D11 (an app-side agent enum) → implies the mapping site must fail to compile when a variant is added, or a new agent silently renders as nothing → D11's exhaustive-match obligation.

**Tier 3 (Pair Tension).** **Runs** — SD4 is an explicit performance/reliability constraint, and D4/D6 are concurrency + resource-budget decisions, so both qualify.
- **D2 × SD4.** A `get(TabId) -> TabMeta` returning owned `String`s, called per tab per frame on a continuously-redrawing loop, allocates forever at idle — the exact class of regression SD4 exists to prevent, and the same trap `sidebar-foundations` D7 caught with its `&mut Vec` sink. → D2 publishes into a **caller-owned** `TabMetaSnapshot` that is refilled **only** when the generation changed; an unchanged frame costs one atomic load and zero allocations.
- **D4 × SD4.** Publishing every tick would call the wake → `poll_all()` → `redraw()` → `snapshot_dirty = true` twice a second forever, forcing a grid re-clone the D7 idle-CPU fix specifically eliminated. → D4 publishes and wakes **only on a real change**.
- **D6 × D5.** A generous walk bound plus a stage-2 `sysctl` per interpreter process multiplies: 64 nodes × one `sysctl` each is a real cost. → D6 caps the walk at 64 nodes *and* breadth-first-with-early-exit (the common case terminates at depth 1 after 1–2 nodes), and D5 gates `sysctl` on the basename already being a known interpreter, so a build tree of `rustc` processes triggers none.
- **D9 × D4.** Re-reading `.git/HEAD` twice a second per tab is I/O the memo exists to avoid, but a memo keyed only on cwd would never notice `git checkout`. → D9's memo validates on `(mtime, len)` of the HEAD file — one `stat`, no read, and it catches checkout.
- **D14 × D1.** A panic in the worker would leave the handle alive but permanently stale, and the UI would show frozen metadata with no error anywhere. → D14 forbids every panic path *and* requires the last-published values to remain visible (stale-but-plausible beats blank), with the staleness bounded by the roster being re-pushed on every tab action.
- **D4 × SD4, second pass** *(missed on the first sweep, found on adversarial review)*. Change-only publication bounds the *average* cost but says nothing about its *shape*: `should_refresh(is_active, tick_index)` had no tab identity, so every background tab shared one phase and a 20-tab window would do 19 tabs of syscalls in a single tick every 2 s. A burst compounds within a tick even when the average is fine. → D4 adds `tab_slot` and stripes the phases.
- **D2 × D4.** `set_active`'s doc said "immediate refresh" while its requirement said "on the next tick" (now R1.7) — under a worker blocked in `recv_timeout(500ms)` those are materially different, and the ambiguity would have shipped as published rustdoc on a cross-feature seam. → D2 fixes it as in-band-before-returning-to-`recv_timeout`, and states that `tick_index` does **not** reset (resetting it would re-bunch the stagger this same review added).
- **D13 × every cost requirement.** D13(2) made the *walk* injectable and stopped there, while six requirements asserted syscall and I/O counts across `git.rs`, the activity path and the worker — none of them reachable by a fake tree, and D13(5) forbids spawning the worker in a test. A performance requirement that no test can fail on is a wish. → D13(6) adds `#[cfg(test)]` `probe` counters and every one of those six Thens is restated against a counter delta.

### Inversion Probe
*"What could make this fail even if every requirement is met correctly?"* — **The feature could be correct and still be permanently blank on the user's machine**, because every one of its three data sources can be unavailable for reasons no test here can see: `claude` may be installed in a form neither classification stage recognises; `KERN_PROCARGS2` may be denied by the app's entitlements once the local `.app` bundle is signed; and `encode_cwd`'s known limitation means a cwd containing a space never resolves a log, so activity silently stays `Unknown`. None of these fails loudly — they all degrade to "no agent", which is indistinguishable from the correct answer for a tab with no agent. Mitigated three ways: D5's classifier is two-stage precisely so the common npm install is covered; D13 requires a self-process test for **each** raw syscall, so an entitlement or ABI failure fails a test rather than silently returning nothing; and D14 requires a `tracing::debug!` at the one point where a *detected* agent yields no activity source, so the blank case is at least diagnosable from a log. Residual: nothing here proves the badge is *right* on a real machine with a real agent — that is a `/run-term` eyeball in `sidebar-row-redesign`, and it is recorded as post-work.

## Decisions

Inherited verbatim from `specs/sidebar-redesign/partition-manifest.md` → `## Shared Decisions`:

### SD1: Adapt the mock's visual language onto the hand-rolled wgpu renderer — never clone its DOM
- **Status**: resolved
- **Rationale**: `Mac Terminal.dc.html` is a CSS/DOM/SVG mock. We reproduce its *visual language* — the `#1e1b18` sidebar on `#24211d`, 13px-radius cards, 12/15px card padding, 3/8px card margins, two-line rows, the `✳`/`◇` mark glyphs, the branch sub-line, the per-agent gradient skins, the 340px default width — as instanced quads and WGSL on the existing renderer. Rejected: adding any GUI/widget crate to get flexbox/SVG for free (violates `.claude/rules/no-gui-crates.md`, the project's reason to exist). Rejected: pixel-perfect cloning — font rasterization, antialiasing and subpixel metrics differ between a browser and our swash atlas, so "matches the mock" means the design tokens are honored, not that a screenshot diff is empty.

### SD2: Sidebar chrome lives in physical-px space, with ONE geometry source of truth shared by draw and hit-test
- **Status**: resolved
- **Rationale**: The mock's paddings, radii and gaps are not cell multiples, so cell-space layout cannot express them. All sidebar layout stays in physical px as pure functions in the renderer, and the app hit-tests by calling those same functions — extending the existing `slot_y`/`tab_at` exact-inverse pair (`crates/renderer/src/tab_bar.rs:98,107`) that `input::tab_target` already delegates to. The sidebar's px width reaches the terminal grid only through `Renderer::set_left_inset` (`crates/renderer/src/lib.rs:508`), which already shifts the grid in the vertex shaders and drives `grid_dims`. Rejected: expressing sidebar layout in cells; rejected: duplicating layout math on the app side (guarantees drift between what is drawn and what is clickable).

### SD3: `term-core` stays GPU-free AND tab-free; tab/git/agent state crosses to the renderer as plain data
- **Status**: resolved
- **Rationale**: `.claude/rules/dependency-direction.md` makes the headless core load-bearing. Tabs are already an `app`-only concept (`crates/app/src/tabs.rs`); git and agent state are likewise app concerns. The renderer keeps receiving them over the existing `Rc<RefCell<TabBarModel>>` side-channel (`tab_bar.rs:30-54`), extended to carry the richer per-tab payload — never on `GridSnapshot`. Rejected: adding tab/git/agent fields to `GridSnapshot` (it is the versioned grid contract and would pull non-terminal state into `term-core`).

### SD4: Per-tab metadata is polled off the UI thread and cached; the render path performs no I/O
- **Status**: resolved
- **Rationale**: `Tabs::titles()` already costs a `proc_pidinfo` syscall per tab per call, and the app runs a continuous redraw loop (`terminal_app.rs:428-430`, the winit#2640 workaround). Adding a filesystem read and a process-tree walk per tab per frame would compound that into a measurable idle-CPU regression — the exact debt G1 records as already paid down once. So a background worker refreshes branch + agent state keyed by `TabId`, and pushes results in via the existing `EventLoopProxy` wake path that PTY output already uses. Rejected: computing branch/agent inline in `sync_tab_model()`.

### SD5: Git branch comes from reading `.git` directly — no git library, no `git` subprocess
- **Status**: resolved
- **Rationale**: We need exactly one string per tab: the current branch. Walking up from the tab's cwd to the first `.git` and reading `HEAD` (`ref: refs/heads/<name>` → branch name; a raw SHA → short detached-HEAD form) is a couple of small file reads. Rejected: `git2` (drags in a libgit2 C build for one field). Rejected: `gix` (large dependency tree for one field). Rejected: shelling out to `git rev-parse` (a process spawn per tab per poll, on a poll loop).

### SD6: AI-agent detection reads the PTY child's descendant processes; `ai-monitor` supplies the Claude-specific detail
- **Status**: resolved
- **Rationale**: `Pty::child_pid()` (`crates/term-core/src/pty.rs:63`) already gives the zsh pid, and the cwd path already uses macOS `libc::proc_pidinfo` (`session.rs:142`) — so a descendant scan via `proc_listchildpids` + `proc_pidpath`, matched against a known-agent table, is the same class of macOS-native call we already depend on (`.claude/rules/macos-only.md` permits this freely). The existing `crates/ai-monitor` JSONL reader supplies the richer Claude Code state where present. Rejected: scraping the terminal's own scrollback for agent banners (fragile, breaks on any CLI release). Rejected: an OSC escape handshake (agent CLIs do not emit one).

### SD7: Sidebar appearance is config-driven, with the mock's values as the defaults
- **Status**: resolved
- **Rationale**: The sidebar's colors, `PAD_X`, `BORDER_W` and the `14.0`/`1.5` cell multipliers are hardcoded in `tab_bar.rs` today, and `common::config` has zero sidebar fields. The mock itself exposes `sidebarWidth` (260–440, default 340), `showGitBranch` and `accent` as knobs, so those are the natural user-facing settings. A new `[sidebar]` section follows its siblings' `#[serde(default, deny_unknown_fields)]` shape and rides the existing live-reload path (`settings::apply`, `settings.rs:23-34`). Rejected: leaving the values hardcoded (the mock's own props argue they are settings).

### SD8: Scope is the SIDEBAR only — the mock's terminal-pane chrome is out, and the search field is deferred
- **Status**: resolved
- **Rationale**: The user's ask is "redesign the left tab", plus git branch and AI agent. The mock's right-hand pane also shows a top-right icon row, a bottom status/footer bar, and an "Use agent" banner — all of which would permanently consume terminal rows from a real PTY grid, and the agent welcome block in the mock is output the agent CLI itself prints (not our chrome). The header's search/filter input is real work of a different kind: a focus-stealing text field must win keystrokes away from PTY routing, which needs its own focus model. Both are deferred to separate future specs rather than smuggled in here. The header's **new-tab** and **hide-sidebar** buttons ARE in scope (they are pure click targets); the filter button is drawn but inert.

---

Feature-local decisions:

### D1: `tabmeta` is one worker thread behind a shared, generation-stamped cache; the UI thread only ever reads
- **Status**: resolved
- **Rationale**: SD4 fixes the shape — poll off the UI thread, cache, push in through the existing wake path. The concrete realisation is a single module `crates/app/src/tabmeta/` with six files, and **exactly one** thread for the whole feature:
  - `mod.rs` — the public types (`TabMeta`, `Agent`, `AgentKind`, `AgentState`, `TabMetaSnapshot`), `TabMetaHandle` (D2), the `TabMetaPump` overlay (D3), and the locator.
  - `worker.rs` — the thread body: the cadence loop, both memos, the differ, the publication (D4).
  - `proc.rs` — the raw macOS calls: `cwd_of_pid`, `children_of`, `exe_path`, `proc_args` (D10, D5).
  - `agent.rs` — the bounded walk and the two-stage classifier (D5, D6).
  - `git.rs` — the `.git` ancestor walk and HEAD parse (D9).
  - `activity.rs` — the busy/idle rule over `ai-monitor`'s tail read, and its two memos (D7).

  A seventh item, `#[cfg(test)] mod probe` (D13(6)), lives in `mod.rs`; its counters are declared in full up front so the three data-source modules never depend on each other to observe their own cost.
  
  **One thread, not one per tab.** A thread per tab would make thread count track user behaviour (twenty tabs = twenty threads), and each would need its own wake and its own share of the memos; the per-tab work is a handful of syscalls, so serialising N tabs in one tick costs less than the context switching of N threads. **One thread, not `notify` file watches.** `LogWatcher` already demonstrates the watch approach and it is the right tool for *tailing one log*; it is the wrong tool here because two of the three signals — a process appearing, and a `cd` — produce no filesystem event at all, so a watch-based design would still need a poll loop for those and would then have two cadences instead of one.
  
  The cache is `Arc<TabMetaShared>` holding an `AtomicU64` generation and a `Mutex<Vec<(TabId, TabMeta)>>`. A `Mutex` rather than an `RwLock` because the UI thread takes it at most once per generation change (D2) and the worker at most once per tick — there is no read contention to optimise away — and rather than a lock-free swap because `Vec<(TabId, TabMeta)>` contains `String`s and a `Mutex` is the boring correct answer. Rejected: `Rc<RefCell<..>>` like `TabModelHandle` (`tab_bar.rs:30-54`) — that pattern is UI-thread-only and this cache crosses a thread boundary. Rejected: sending the whole map down an `mpsc` channel per tick (it would allocate a fresh map per tick even when nothing changed, and D4 already establishes that the common case is "nothing changed").

### D2: The seam is `TabMetaHandle` + a caller-owned `TabMetaSnapshot`, reached through a `OnceLock` locator
- **Status**: resolved
- **Rationale**: This is the contract `sidebar-row-redesign` programs against, so it is stated exactly:

  ```rust
  #[derive(Clone, Copy, PartialEq, Eq, Debug)]
  pub struct TabPid { pub id: TabId, pub pid: u32 }

  #[derive(Clone)]
  pub struct TabMetaHandle { /* Arc<TabMetaShared>, Sender<Cmd> */ }

  impl TabMetaHandle {
      /// Start the worker thread. `wake` is invoked ON THE WORKER THREAD once per
      /// tick in which at least one tab's metadata changed — keep it cheap
      /// (an `EventLoopProxy::send_event`). Returns `None` if the thread cannot
      /// be spawned; the caller then simply has no metadata.
      pub fn spawn(wake: impl Fn() + Send + 'static) -> Option<TabMetaHandle>;

      /// Declare the live tab set. Cheap, and idempotent: pushing an unchanged
      /// roster does not disturb the cadence or invalidate any memo. Call it
      /// wherever the tab set can change (the natural site is alongside
      /// `Tabs::titles()`), never per frame.
      pub fn set_roster(&self, roster: &[TabPid]);

      /// Which tab is focused (it polls at the fast rate, D4). The worker refreshes
      /// that tab IN BAND — on receiving the command, before returning to its
      /// `recv_timeout` — so a switch never waits out a background interval. The
      /// global tick phase is NOT reset (that phase is what staggers background
      /// tabs, D4).
      pub fn set_active(&self, id: Option<TabId>);

      /// Display toggles (D12). Disabling a source stops the worker computing it
      /// and clears that field on the next publish. Both default to `true`.
      pub fn set_enabled(&self, git: bool, agent: bool);

      /// The current publication generation. Monotonic; starts at 0; bumped only
      /// when a publish actually changed something (D4).
      pub fn generation(&self) -> u64;

      /// Refill `out` iff the generation moved since `out` was last filled.
      /// Returns `true` when `out` was rewritten (→ the caller rebuilds its model),
      /// `false` when nothing changed. On `false` this call takes NO lock beyond a
      /// single `Ordering::Acquire` load and performs NO allocation — that is the
      /// property that makes it safe to call every frame.
      pub fn sync(&self, out: &mut TabMetaSnapshot) -> bool;
  }

  #[derive(Default)]
  pub struct TabMetaSnapshot { /* generation: u64, tabs: Vec<(TabId, TabMeta)> */ }
  impl TabMetaSnapshot {
      pub fn get(&self, id: TabId) -> Option<&TabMeta>;
      pub fn iter(&self) -> impl Iterator<Item = (TabId, &TabMeta)>;
      pub fn is_empty(&self) -> bool;
  }
  ```

  **Why the snapshot is caller-owned, and where the caller keeps it.** The sidebar reads metadata per tab, and it draws on a continuously-redrawing loop. A `get(TabId) -> TabMeta` returning owned `String`s would allocate on every tab on every frame at idle — the same trap `sidebar-foundations` D7 caught with its `&mut Vec` sink, one layer up. `sync` refills a `Vec` the caller keeps across frames (`clear()` + `extend`, so the allocation is reused), and only when the generation moved. **The `false` branch is the hot path and it is a single `Ordering::Acquire` load** paired with the worker's `Release` store, so the caller never even contends the mutex on an idle frame. `sync`'s two branches are separately testable — a test asserts that a second `sync` with no intervening publish returns `false` and leaves `out`'s contents and its `Vec::capacity()` bit-identical, which is what makes the "no allocation" claim falsifiable rather than a comment.

  **"A caller that keeps a `TabMetaSnapshot` across frames" is a claim about a call site, so the call site is named — and the seam mechanics are checked, not assumed.** `Overlay` declares exactly four methods (`seam.rs:93-110`): `dispatch`, `poll_all`, `on_active_changed`, and `layer(&self)`. **`layer()` is called from exactly one production site, once, at registration** (`terminal_app.rs:148`, inside the loop at `:146-150`: `renderer.add_layer(o.layer()); overlays.register(o);`), and it hands the renderer a *separately owned* `Box<dyn DrawLayer>`. So the object living in `OverlayHost` gets **no** per-frame callback; the only per-frame hook in the whole app is `DrawLayer::record(&mut self, ctx)` (`renderer/src/lib.rs:141`). The read site is therefore precisely:

  > `sidebar-row-redesign` constructs its sidebar object in `main.rs` (a file it owns), passing a `TabMetaHandle`. Its `Overlay` impl is a near-empty shell — the `ImagePresenter` shape, which is literally a unit struct (`clipboard.rs:115`) whose `layer()` mints an `ImageLayer`. Because `layer(&self)` takes `&self`, the handle is **cloned** into the minted `DrawLayer` (this is why `TabMetaHandle: Clone` is load-bearing, not a convenience). **That `DrawLayer` owns the `TabMetaSnapshot` as a field and calls `sync(&mut self.snap)` at the top of `record(&mut self, ..)`** — the one place with `&mut self` on every frame. `sync` returning `false` is why that is affordable.

  An earlier draft of this paragraph said the `OverlayHost` resident owned the snapshot and synced per frame; that is mechanically impossible for the reason above, and it is exactly the kind of unchecked seam claim this decision exists to prevent. The alternative that does *not* work at all: pushing metadata through `renderer::tab_bar::TabBarModel` would require adding a field to a struct whose only literal is at **`terminal_app.rs:219-222`**, and whose `TabModelHandle` is created at **`terminal_app.rs:134`** (`let (bar, tab_model) = TabBar::new();`) — both in a file `sidebar-row-redesign` does not own. So the read path has exactly one viable site, and it is inside the consumer's own surface.

  **Why a `OnceLock` locator, and — importantly — for which two methods only.** The read half above needs **no** global: `sidebar-row-redesign` owns `main.rs`, so it can be handed a `TabMetaHandle` by value at construction. The global exists for exactly the two methods that must fire from **inside `Tabs`** — `set_roster` and `set_active` — because those must run wherever the tab set or the focus changes, and that is `crates/app/src/tabs.rs`. `Tabs` has no field to hold a handle, because `Tabs::new(first, fg, bg, proxy)` is **constructed at `terminal_app.rs:158`**, and `terminal_app.rs` is in **neither** this feature's surface nor `sidebar-row-redesign`'s (it belongs to `sidebar-interaction`, which comes later and depends on `sidebar-row-redesign`). So constructor injection is impossible for the one type that needs it, and every other candidate fails too: `AppCtx` carries no `TabId` (`seam.rs:46-56`), `on_active_changed` carries only a cwd (`seam.rs:109`), and `seam.rs` itself is out of surface so no `register_service` hook can be added. `set_enabled` is likewise handle-by-value — the consumer calls it from `main.rs` at startup; the live-reload call site belongs to `sidebar-interaction` (D12). Therefore `tabmeta` publishes a locator, scoped to that one need:

  ```rust
  pub fn install(handle: TabMetaHandle);          // called once, from main.rs
  pub fn global() -> Option<&'static TabMetaHandle>;
  ```

  backed by a `std::sync::OnceLock<TabMetaHandle>`. This is a process-global singleton and that is a real smell; three things make it acceptable and one makes it testable. It is acceptable because the binary has exactly one tab set and one event loop; because `common::logging::init()` (`main.rs:26`) already establishes process-global init in this codebase; and because the alternative is not "a cleaner design" but "a cross-feature edit to a file the partition assigns to a third feature", which is precisely the collision an ordered partition exists to prevent. It is **testable** because the global is only a *locator*: `TabMetaHandle` is an ordinary `Clone` value with no dependency on the `OnceLock`, so **every unit test constructs its own handle and never touches `global()`** — the singleton has no reachable effect on any test. It is also removable, and its removal is now a two-call change rather than a redesign, precisely because the other three methods never used it: once `sidebar-interaction` owns `terminal_app.rs` it can inject the handle into `Tabs` properly and delete `install`/`global`. Rejected: threading the handle through `Tabs::new` (an out-of-surface edit to `terminal_app.rs:158`); rejected: a `register_service` seam (`seam.rs` is out of surface); rejected: having the worker discover the roster itself by walking `proc_listchildpids(own_pid)` for zsh children (it finds the pids but cannot map any of them to a `TabId`, which is the whole point of the roster); rejected: keying the cache by pid instead of `TabId` (pids are recycled and change on nothing the user did, while `TabId` is stable and already `Hash` at `session.rs:19`); rejected: routing the *read* half through the global too, for symmetry (it would spread a singleton across the seam to save the consumer one constructor argument in a file it owns).

  **`set_active`'s timing is in-band, and the tick phase does not reset.** `set_active` (like every other command) arrives on the channel the worker is blocked on, so `recv_timeout` returns it immediately. The worker then refreshes **that tab only, before returning to `recv_timeout`** — so a switch to a background tab shows fresh data at once rather than waiting out its 2 s interval. It does **not** reset `tick_index`: the global tick phase is what staggers background tabs (D4), and resetting it on every Cmd+1..9 would re-bunch them. The published rustdoc says exactly this, so the two readings the phrase "immediate refresh" would otherwise allow cannot both ship.

### D3: The wake rides `UserEvent::OverlayPending` via a headless pump registered through the existing `Overlay` seam
- **Status**: resolved
- **Rationale**: The worker must wake the UI thread, and `UserEvent` (`terminal_app.rs:33-43`) has exactly three variants in a file that is **not** in this feature's surface — no variant can be added. The established in-repo pattern for exactly this situation is a **bare signal variant plus a separate channel for the payload**: `ConfigReloaded` carries nothing and the `Config` rides `config_rx` (`terminal_app.rs:322-332`), and `OverlayPending` carries nothing while ai-monitor's events ride `LogWatcher::rx` (`overlay.rs:62-64`). This feature's payload rides the shared cache (D1), so it needs only the signal.

  `OverlayPending`'s handler calls `inner.overlays.poll_all()` and redraws iff it returns `true` (`terminal_app.rs:334-338`), and `OverlayHost::poll_all` calls `poll()` on **every** registered overlay regardless of visibility (`seam.rs:169-175`) while `dispatch` skips overlays whose `visible()` is `false` (`seam.rs:154-161`). So a registered overlay that is permanently invisible gets a UI-thread drain hook and receives **no** keys. That is `TabMetaPump`:

  ```rust
  pub struct TabMetaPump { handle: TabMetaHandle, seen: u64 }
  impl Overlay for TabMetaPump {
      fn priority(&self) -> i32 { i32::MIN }      // never ahead of a real overlay
      fn visible(&self) -> bool { false }          // never receives a key
      fn on_key(&mut self, _: &KeyInput, _: &mut AppCtx) -> Handled { Handled::Passthrough }
      fn layer(&self) -> Box<dyn DrawLayer> { /* z_order = i32::MIN, record() is empty */ }
      fn poll(&mut self) -> bool { let g = self.handle.generation(); let c = g != self.seen; self.seen = g; c }
  }
  ```

  Registered from `main.rs` after the existing two. **Note `main.rs:44` moves `proxy`** into `AiMonitorOverlay::new`, so that line becomes `proxy.clone()` and this feature's `spawn` takes the original — a one-token edit inside the declared surface.

  This is honestly a slight abuse: `Overlay` means "a visible thing that consumes input and draws", and this is a headless service. The costs are exactly two — one `record()` call per frame that returns immediately, and one `poll()` call per `OverlayPending` event that reads an atomic — and both are already paid by `ImagePresenter`, which is likewise a registered `Overlay` that exists for its layer rather than for its keys. The wart is named rather than hidden. Rejected: adding a `UserEvent::TabMetaPending` variant (out of surface, and it would collide with `sidebar-interaction`, which owns `terminal_app.rs`); rejected: adding a `register_service` hook to `seam.rs` (out of surface); rejected: having the worker call `window.request_redraw()` directly (`Window` is not reachable from `main.rs` and the call is not thread-safe to make from an arbitrary thread in winit 0.30's model); rejected: no wake at all, relying on the continuous redraw loop to pick changes up eventually (it would work today by accident, but it breaks the moment the window is occluded — redraws are paused at `terminal_app.rs:429` — and it silently depends on a workaround that `docs/architecture.md` G1 wants removed).

### D4: Two-rate cadence — active 500 ms, background 2 s — with change-only publication and a zero-wakeup idle
- **Status**: resolved
- **Rationale**: L1 established there is **no timer anywhere**: `ControlFlow::Wait` is set once (`main.rs:37`), `WaitUntil`/`Poll` appear nowhere, `about_to_wait` is unimplemented, and even the cursor blink fakes a clock by sampling `Instant::elapsed()` per frame (`cursor_layer.rs:21,29,211`). So the feature brings its own cadence: the worker blocks in `rx.recv_timeout(tick)` on its command channel, which serves as both the sleep and the command inbox.

  **Rates.** `TICK = 500ms`; the active tab is refreshed every tick, background tabs every 4th tick (**2 s**). The active tab's badge is the one the user is looking at while an agent works, so its latency budget is "faster than a human notices a state change" — 500 ms. A background tab's badge is glanceable, not watched; 2 s is invisible there and cuts the background cost by 4×. Both are named constants in one place so they are one edit to retune.

  **Background tabs are staggered, not bunched.** The scheduler is `should_refresh(is_active, tick_index, tab_slot) = is_active || tick_index % 4 == tab_slot % 4`, where `tab_slot` is the tab's index in the roster. A naive `tick_index % 4 == 0` would refresh **every** background tab on the **same** tick, so a 20-tab window would do 19 tabs' worth of syscalls in one burst every 2 s and nothing in between — the same total work, delivered in the shape most likely to be visible as a stutter. Spreading them over the four phases turns that into ~5 tabs per tick, and it costs one extra argument. This is a genuine gap in the first draft: SD4's concern is about *compounding* cost, and a burst compounds within a tick even when the average is fine.

  **The cost this buys, stated concretely.** In the steady state (memos warm) one active-tab refresh is: 1 `proc_pidinfo` (cwd) + 1–2 `proc_listchildpids` + 1–2 `proc_pidpath` + 1 `stat` on `.git/HEAD` + 1 `stat` on the JSONL ≈ **6–10 syscalls and zero file reads**, twice a second. For comparison, the app already spends 1 `proc_pidinfo` per tab in `Tabs::titles()` on every tab action and **two** in `on_key` on every keypress (`terminal_app.rs:259,272`) — so at a normal typing rate the existing code already exceeds this feature's whole steady-state budget. That is the honest frame for SD4: this feature must not *compound* a per-frame cost, and it does not, because it is not on the frame path at all.

  **Publication is change-only.** After computing a tick's results the worker compares each tab's freshly derived `TabMeta` against the last published one. If **nothing** differs it does not take the mutex, does not bump the generation, and **does not call `wake`**. This is not an optimisation, it is a correctness requirement against SD4: `poll_all()` returning `true` calls `Running::redraw()` (`terminal_app.rs:336`), which sets `snapshot_dirty` (`:213`) and forces a full grid re-clone on the next frame — exactly the idle-CPU debt `docs/architecture.md` G1 records as already paid down once. An unconditional publish would reintroduce it at 2 Hz forever.

  **An empty roster costs nothing at all.** With no tabs registered the worker blocks in `rx.recv()` with **no** timeout — zero wakeups, zero syscalls — until a roster arrives. This is also what makes the feature genuinely inert when it ships before its consumer: with nothing calling `set_roster`, the thread exists and sleeps.

  **A tab whose refresh is slow does not stall the others.** Each tab's derivation is independent and failure-isolated (D14); the loop simply moves on. There is no per-tab timeout — a genuinely hung `stat` on a dead network mount would stall the tick, but it would equally stall `Tabs::titles()` today, so it is a pre-existing property of the codebase rather than something this feature introduces. Recorded in Known Gaps.

  Rejected: one rate for all tabs (either the background cost is 4× higher for no benefit, or the active tab is sluggish); rejected: an adaptive/backoff rate keyed on recent change (real complexity, and the failure mode — a badge that gets *slower* to update the longer you leave a tab alone — is exactly backwards); rejected: driving the cadence from the existing continuous redraw loop the way the cursor blink does (that would put the I/O on the UI thread, which SD4 forbids in one sentence); rejected: `std::thread::sleep` plus a separate command channel (`recv_timeout` is one primitive that does both, and it makes `set_active`'s in-band refresh fall out for free).

### D5: Agent presence and identity come from a two-stage classification of the tab's descendant processes
- **Status**: resolved
- **Rationale**: SD6 fixes the mechanism (walk the PTY child's descendants); this decides how a process is *recognised*.

  **Stage 1 — executable basename.** `proc_pidpath(pid)` (verified in `libc-0.2.186` at `apple/mod.rs:4923`, with `PROC_PIDPATHINFO_MAXSIZE = 4096` at `:3688`) yields the full executable path; its file name is compared **case-insensitively** against a closed const table:

  | basename | `AgentKind` |
  |----------|-------------|
  | `claude` | `Claude` |
  | `antigravity` | `Antigravity` |
  | `codex` | `Codex` |
  | `gemini` | `Gemini` |
  | `aider` | `Aider` |

  **Stage 2 — argv, for interpreter processes only.** Stage 1 alone has a specific, fatal hole: a Claude Code installed via npm runs as `node /…/@anthropic-ai/claude-code/cli.js`, so `proc_pidpath` returns `…/node` and the flagship agent is undetectable. So when — and **only** when — a descendant's basename is in a second small table (`node`, `bun`, `deno`, `python3`, `python`), the worker reads that process's argument vector via `sysctl([CTL_KERN, KERN_PROCARGS2, pid])` (`KERN_PROCARGS2 = 49`, verified at `apple/mod.rs:3681`) and scans the arguments for any **path component** matching a known agent name or its packaged form (`claude`, `claude-code`, `antigravity`, `codex`, `gemini`, `aider`). First match wins. Gating on the interpreter table matters: it means a tab where an agent has spawned a hundred `rustc` processes triggers **zero** `sysctl` calls, which is the D6 × D5 tension resolved.

  **Stage 2's own failure mode, stated.** `KERN_PROCARGS2` returns the argv buffer only for processes the caller may inspect; it can fail with `EINVAL` for a same-user process under a hardened runtime, and the local `.app` bundle (`docs/architecture.md` D11) is signed. When the `sysctl` fails, the process is simply **not** an agent — there is no fallback, no retry, and no error surfaced. The consequence is honest and bounded: a node-installed Claude in a signed build would show no badge, exactly as if no agent were running. D13 requires a self-process test (`proc_args(std::process::id())` must contain the test binary's own path), so an outright ABI or entitlement failure fails a test rather than degrading silently on every machine.

  **The table is closed.** A process is an agent **iff** it is in the table. Rejected explicitly: heuristics such as "any long-lived child that is not a known shell utility", which would flag `vim`, `node` running a dev server, or `python` running anything. A closed table means a new agent CLI is invisible until someone adds one line — a cost paid in a one-line edit, against the alternative of false badges the user cannot turn off.

  **Where `ai-monitor` does and does not participate.** SD6 says `ai-monitor` "supplies the Claude-specific detail". This decision draws that line precisely: `ai-monitor` supplies **no** part of *detection* — presence and identity are entirely a process fact. It supplies only *activity* (D7). An earlier draft used a fresh Claude JSONL in the tab's cwd as a **fallback detector** for the interpreter case, and it was rejected: a log's recency is a poor proxy for "the process exists right now", so it would light the badge for a session you closed ten minutes ago and it would need an arbitrary freshness window to be even approximately right — whereas `KERN_PROCARGS2` answers the actual question. Rejected: matching on the process's *name* from `proc_listpids` rather than its path (the name is truncated to 16 bytes on macOS and would collide); rejected: matching the process group or controlling terminal instead of walking children (`portable_pty` exposes neither, per L1).

### D6: The walk is breadth-first, bounded on three axes, and treats every syscall failure as "absent"
- **Status**: resolved
- **Rationale**: The walk starts at the tab's zsh pid (`Pty::child_pid()`, `term-core/src/pty.rs:63`, delivered to the worker as a `u32` in the roster) and descends. Unbounded, it is a denial of service against ourselves: an agent running `cargo build -j16` has hundreds of descendants, and the walk would run twice a second. Three bounds, all named constants:
  - **`MAX_DEPTH = 3`** levels below zsh. `zsh → claude` is depth 1; `zsh → node → claude` and `zsh → npx → claude` are depth 2; a user's shell-function wrapper adds one more. At depth 4 and beyond you are inside the agent's *own* subprocess tree (claude → `bash -c` → `cargo` → `rustc`), where nothing is an agent.
  - **`MAX_NODES = 64`** pids visited per walk, hard stop. Sized so the realistic worst case (an agent plus a build) terminates well inside it while a pathological tree cannot make one tab's tick unbounded.
  - **`MAX_CHILDREN = 128`** per `proc_listchildpids` call — a fixed buffer; if the kernel reports more, the excess is dropped. Truncation is acceptable because we are looking for one agent, not auditing a process tree. **The truncation lives above the syscall, in a pure `parse_childpids(buf: &[i32], returned: c_int) -> Vec<u32>`** (the same split `proc_args`/`parse_procargs` uses), so the cap and the `returned <= 0` cases are table-tested without spawning 128 processes — which D13 and the workspace's `#[ignore]`d-only precedent both forbid. Had the cap stayed inside `children_of` it would have been the one bound in D6 with no reachable test.
  - **Breadth-first with early exit.** The first classified match ends the walk. The overwhelmingly common shape is one zsh with one or two children, so the typical walk visits 1–3 nodes and stops.

  **Dead pids.** `proc_listchildpids` returning ≤ 0, `proc_pidpath` returning ≤ 0, and a failed `sysctl` all mean the same thing: that node contributes nothing. The BFS **continues with the remaining queue** rather than aborting — a process exiting mid-walk is the normal case, not an error — and nothing is logged at `warn` level for it. If the tab's own zsh pid is gone, the result is simply "no agent", and the tab's entry disappears entirely on the next `set_roster` from the consumer.

  **pid reuse, bounded rather than asserted.** A pid delivered in the roster can in principle be recycled before the worker walks it, which would attribute another process's descendants to this tab. Two properties bound the damage. First, **no pid is ever cached across ticks** — the walk restarts from the roster's zsh pid every time and keeps no intermediate pid — so a stale attribution cannot persist; it lasts at most one tick (≤ 2 s). Second, the roster is re-pushed by the consumer on every tab action, so a closed tab's pid leaves the roster promptly rather than lingering. What is *not* claimed: that a mis-attribution cannot happen at all. It can, for one tick, and the visible consequence is a briefly wrong badge on a tab that is being closed. Detecting it properly would need a pid + start-time pair from `proc_pidinfo(PROC_PIDTBSDINFO)`, which is a second syscall per tab per tick to close a one-tick cosmetic window on a tab that is going away — rejected as not worth it, and recorded in Known Gaps so the trade is visible rather than implied.

  Rejected: depth-first (it would descend into a build tree before checking zsh's other direct children, which is where the agent almost always is); rejected: `proc_listpids(PROC_ALL_PIDS)` and reconstructing the whole tree from ppids (one call returning every process on the machine, per tab, per tick, to answer a question about ≤ 3 processes); rejected: unbounded depth with only a node cap (the node cap alone would still let a wide build tree consume the entire budget before the agent's sibling is reached, which breadth-first plus a depth cap avoids).

### D7: Agent *activity* is a separate axis, derived from a bounded JSONL tail read added to `ai-monitor`
- **Status**: resolved
- **Rationale**: Presence answers "is an agent here"; it cannot answer "is it working or waiting for me", which is what distinguishes the mock's `✳` mark from its `◇` ready mark. The only available source for that is the Claude Code session JSONL — which is exactly what `crates/ai-monitor` already reads, and why the manifest put `crates/ai-monitor/src/**` in this feature's surface.

  **`backfill` cannot be reused.** `lib.rs:125-136` does `std::fs::read(log)` — the **entire** file — then `parse_line`s **every** line. On a long session that is megabytes of read plus JSON parse, per tab, per tick. So this feature adds a sibling function rather than calling it:

  ```rust
  // crates/ai-monitor/src/activity.rs
  pub struct TailState { pub role: String, pub mtime: SystemTime, pub len: u64 }
  /// The role of the LAST complete line in `log`, plus the file's mtime and length.
  /// Reads at most the final 8 KiB. `None` if the file is missing, empty, or has no
  /// parseable final line.
  pub fn tail_state(log: &Path) -> Option<TailState>;
  ```

  It `stat`s the file, seeks to `max(0, len - 8192)`, reads forward, takes the last `\n`-terminated line, and hands it to the existing `parse_line` (`lib.rs:73`). Purely additive: no existing function changes, and it uses only `std` + `serde_json`, so **`ai-monitor/Cargo.toml` is untouched** — which matters because that file is not in the declared surface and `Cargo.lock` edits are hook-blocked.

  **The verdict is computed by the worker, not by `ai-monitor`.** `tail_state` is a pure function of the file; the busy/idle rule is app policy and lives in `worker.rs`:

  > `Busy` if the log was appended to within `BUSY_WINDOW = 10s`, **or** the last line's role is `"user"`. Otherwise `Idle`.

  Both clauses are needed and each covers the other's blind spot. Recency alone misses the case where you have just pressed Enter and the assistant has not yet written anything — the last line is your `user` turn and the file may already be seconds old. The role clause alone misses the case where the agent is mid-turn writing tool calls, where the last line's role is `assistant` or a tool event but appends are streaming in. Ten seconds is chosen as comfortably longer than the gap between a working agent's appends and comfortably shorter than a human's thinking pause; it is a named constant.

  **Two memos, because the cheap part is not the expensive part.** `resolve_log_in` (`lib.rs:41`) does a `read_dir` plus a `metadata()` per entry to find the newest `*.jsonl` — for a directory holding many sessions that is far more expensive than the tail read it precedes. So: the **log path** is memoised per cwd and re-resolved every 10 s (a new session creates a new file, so it cannot be cached forever), and the **tail role** is memoised on the log's `(mtime, len)` so an idle log is read exactly once per append rather than once per tick. Steady state for an idle agent is therefore **one `stat`** per tick. Both memos are keyed by **cwd**, share D9's lifetime rule exactly (reclaimed by disuse at `MEMO_TTL_TICKS`, hard-capped at `MEMO_MAX`, never evicted by tab removal), and are therefore shared correctly by two tabs sitting in the same project.

  **The role clause is bounded, and its failure mode is stated.** `parse_line` takes the top-level `type` field before `/message/role` (`lib.rs:79-84`), and Claude Code writes **tool results as `"type":"user"` lines** — so an interrupted or abandoned session whose final line happens to be a `user` tool result would read `Busy` forever if the role clause had no time bound. It therefore fires only while the log is younger than `STALE_LIMIT = 10 × BUSY_WINDOW` (100 s); beyond that the role is ignored and the state is `Idle`. That bound is deliberately generous — it must comfortably exceed the longest plausible gap between submitting a turn and the assistant's first written token — and its residual cost is a badge that reads `Busy` for up to 100 s after an abandoned turn, which is recorded in Known Gaps rather than papered over.

  Rejected: reusing `backfill` (whole-file read, see above); rejected: keeping a `LogWatcher` per tab (`notify` watches the parent directory (`lib.rs:208`), so N tabs in the same project would each install a watch on the same dir — and it still would not answer the "cd" or "process appeared" questions, so the poll loop would exist anyway); rejected: putting `tail_state` in `tabmeta` instead of `ai-monitor` (it would have to duplicate `parse_line` and `resolve_log_in`, and the manifest deliberately gave this feature the `ai-monitor` surface); rejected: deriving activity from CPU time via `proc_pidinfo(PROC_PIDTASKINFO)` (an agent waiting on a network round-trip uses no CPU and would read as idle mid-turn, and it says nothing about *whose* turn it is).

### D8: `TabMeta` makes illegal states unrepresentable; `AgentState` has three variants and **no `Done`**
- **Status**: resolved
- **Rationale**:

  ```rust
  #[derive(Clone, Default, PartialEq, Eq, Debug)]
  pub struct TabMeta { pub branch: Option<String>, pub agent: Option<Agent> }

  #[derive(Clone, Copy, PartialEq, Eq, Debug)]
  pub struct Agent { pub kind: AgentKind, pub state: AgentState }

  #[derive(Clone, Copy, PartialEq, Eq, Debug)]
  pub enum AgentState { Busy, Idle, Unknown }
  ```

  **`Option<Agent>` rather than `Option<AgentKind>` plus a sibling `state` field**, so "no agent but state = Busy" cannot be constructed or accidentally rendered. `Agent` is `Copy`; only `branch` allocates.

  **There is no `Done`.** It is the obvious fourth variant and it is wrong: an agent that has finished *is a process that has exited*, so it is `agent: None` — the badge disappears, which is precisely what "done" looks like. A `Done` state would require keeping a tombstone after the process is gone, which then needs an expiry policy, and the first question it raises ("done with *what*, and since when?") has no answer the process table can give. If a lingering "finished" affordance is ever wanted it is a UI decision about a transition, belongs to the feature that draws it, and would be built from watching `Some(agent) → None` — which the generation-stamped snapshot already makes observable.

  **`Unknown` is a real state, and it carries a consumer obligation.** A detected `Codex`/`Gemini`/`Aider`, or a `Claude` whose log cannot be resolved (no `$HOME`, a cwd `encode_cwd` mangles, a session started elsewhere), is genuinely "an agent is running and we cannot tell what it is doing". Collapsing that into `Busy` would assert knowledge we do not have, and collapsing it into `Idle` would be actively misleading while the agent works. The obligation on the consumer is stated here so it is not left to taste: **`Unknown` must render the "an agent is present" affordance (the mock's `✳` mark and the agent skin), never the "ready/waiting" affordance (`◇`)** — because the one thing `Unknown` does assert is presence. `Busy` and `Unknown` therefore share a mark and differ only in that `Busy` is known; `Idle` is the only state that earns `◇`.

  `TabMeta::default()` is `{ branch: None, agent: None }` — a plain zsh tab in a non-repo, and the correct value for a tab that has not been polled yet, so there is no "unpolled" third state to handle. Rejected: a flattened struct with a `has_agent: bool`; rejected: `AgentKind::None` as a variant (it would force every match site to handle a non-agent case that `Option` already expresses); rejected: adding a `since: Instant` to `Agent` (it would make `TabMeta` unequal on every tick and defeat D4's change detection entirely — a genuinely load-bearing reason, since `PartialEq` on `TabMeta` *is* the change detector).

### D9: Branch comes from a cwd-keyed memo over a bounded `.git` ancestor walk; `cd` needs no invalidation at all
- **Status**: resolved
- **Rationale**: SD5 fixes the mechanism (read `.git` directly). The interesting part is invalidation, and the interesting realisation is that **there is nothing to invalidate for `cd`**: the roster carries a *pid*, not a cwd, and the worker resolves the cwd from that pid on every tick (D10). A `cd` therefore shows up within one tick with no machinery — and, importantly, without adding a single syscall to the UI thread, which is what would have happened had the roster carried cwds computed by `Tabs`.

  **Resolution.** From the cwd, walk parents (≤ `MAX_GIT_ANCESTORS = 40`, stopping at the filesystem root) looking for `.git`. If `.git` is a **directory**, the git dir is that path. If `.git` is a **file** — the worktree and submodule case — read it (capped at 4 KiB), take the `gitdir: <path>` line, and resolve it relative to the containing directory if it is not absolute. Then read `<gitdir>/HEAD`, capped at 4 KiB.

  **Parsing.** `ref: refs/heads/<name>` → `<name>`, verbatim including slashes (`feat/x`). A bare 40-character hex SHA → `@` + its first 7 characters, so a detached HEAD reads as `@a1b2c3d` and is visibly not a branch name. Anything else → `None`. Trailing `\r`/`\n` trimmed. The result is capped at **64 characters** — `HEAD` is a plain user-writable file and a pathological one must not push an unbounded `String` into a struct the UI clones.

  **The memo, and why it is keyed on two things.** A memo keyed only on cwd would never notice `git checkout`, which changes the branch with **neither** a process change **nor** a cwd change — the case the checkpoint table calls out explicitly. So the memo entry is `Repo { cwd, head_path, head_mtime, head_len, branch }`, and a tick with an unchanged cwd revalidates by `stat`ing `head_path` and comparing `(mtime, len)`: unchanged → reuse the cached string with **no read and no ancestor walk**; changed → re-read and re-parse. One `stat` per tab per tick in the steady state.

  **Negative results are memoised too, but they expire.** Without that, every tab outside a repo pays a 40-`stat` ancestor walk twice a second forever. So a `NotARepo { cwd, checked_at }` entry short-circuits the walk, and it is **re-validated every 10 s** — because `git init` in the current directory creates a repo with no process change, no cwd change, and nothing else for the memo to notice. Ten seconds is a deliberately loose bound for an event that happens roughly never; the cost of being wrong is a branch name appearing a few seconds late.

  **Memo lifetime — by disuse, never by tab removal.** Because the key is a cwd and not a `TabId`, two tabs in the same repo **share** one entry, so evicting "a closed tab's entries" would pull the rug from a live tab. Reclamation is therefore by disuse: every entry records the tick at which it was last read, entries untouched for `MEMO_TTL_TICKS` (≈ 60 s) are dropped at the end of a tick, and the map is hard-capped at `MEMO_MAX = 64` entries with the oldest-touched evicted first — so a shell that `cd`s through hundreds of directories cannot grow it without bound. `set_roster` evicts nothing. The same rule governs D7's two memos, which are keyed the same way.

  Rejected: caching the branch per `TabId` instead of per cwd (two tabs in the same repo would each do the work, and the entry would survive a `cd` that should invalidate it); rejected: watching `.git/HEAD` with `notify` (a watch per tab per repo, installed and torn down on every `cd`, to save one `stat`); rejected: reading `.git/refs/heads/<name>` or `packed-refs` (we need the branch *name*, which `HEAD` already gives; the commit SHA is not displayed); rejected: `git rev-parse --abbrev-ref HEAD` (SD5 rejects it — a process spawn per tab per tick).

### D10: `cwd_of_pid` is reimplemented inside `tabmeta`, not reused from `session.rs`
- **Status**: resolved
- **Rationale**: `Session::cwd()` is `pub` (`session.rs:108`) but it needs a `&Session`, which lives on the UI thread inside `Tabs` and is not `Send`. The function that actually does the work, `cwd_of_pid` (`session.rs:142-167`), is **module-private**, and **`session.rs` is not in this feature's declared surface** — so it can be neither called nor made `pub`. It is therefore reimplemented in `tabmeta/proc.rs`: `libc::proc_pidinfo(pid, PROC_PIDVNODEPATHINFO, 0, &mut info, size)`, short-read check, NUL-terminated `pvi_cdir.vip_path`, UTF-8 → `PathBuf`, `None` on any failure.

  This is ~20 duplicated lines and that is a genuine cost, so the reasoning is recorded rather than assumed. The alternative — routing cwd through the roster, i.e. having `Tabs` call `Session::cwd()` per tab and hand the paths to the worker — is worse on three counts, not one: it puts a `proc_pidinfo` **per tab** back on the UI thread at every roster push (the exact compounding SD4 names); it makes `cd` detection depend on how often the consumer happens to push a roster, rather than on this feature's own cadence; and it would require the roster to be re-pushed on a timer, which would mean building a second cadence on the UI thread. Duplicating twenty lines of a stable kernel ABI is the cheaper trade.

  The duplication is made safe rather than merely accepted: `session.rs:169-185` already tests its copy against the current process, and D13 requires **the same test** for this copy (`cwd_of_pid(std::process::id())` must equal `std::env::current_dir()`), so a divergence in either copy fails a test. Rejected: moving `cwd_of_pid` to a shared location (`session.rs` is out of surface, and `common` is UI/GPU-free but this is a macOS process call that does not belong in the config crate); rejected: `std::fs::read_link("/proc/<pid>/cwd")` (Linux-only; there is no `/proc` on macOS).

### D11: The agent vocabulary is app-side, standalone, and never names a renderer type
- **Status**: resolved
- **Rationale**: This is the counterpart of `sidebar-foundations` D13, and the direction constraint is real: this feature is `depends_on: []`, so it may be built and merged **before** `crates/renderer/src/sidebar_theme.rs` exists at all. Referencing `renderer::sidebar_theme::AgentSkin` would not merely be poor layering, it would **not compile**. It is also wrong under SD3: an app-side metadata provider has no business knowing about renderer colour vocabulary.

  So `AgentKind { Claude, Antigravity, Codex, Gemini, Aider }` is defined here, in `crates/app/src/tabmeta/mod.rs`, with no renderer import anywhere in the module. `sidebar-row-redesign` — the only feature that `depends_on` both this and `sidebar-foundations` — owns the `AgentKind → AgentSkin` mapping, where `Claude → Claude`, `Antigravity → Antigravity`, and the remaining three → `Generic` (which `sidebar-foundations` D6 defines as the plain-card treatment, so an unrecognised agent renders as a normal tab).

  **The enum is deliberately not `#[non_exhaustive]`** — that attribute has no effect within a crate, and `sidebar-row-redesign` is in the same crate for the mapping's app-side half. What is wanted instead is the opposite property: the mapping site must use an **exhaustive `match` with no `_` arm**, so that adding a sixth agent to D5's table is a compile error at the one place a human must make a decision. That obligation belongs to the mapping site, so it is stated here as a contract on the consumer rather than enforced by a type. `AgentKind` also carries `pub fn label(self) -> &'static str` for logging and future tooltips, so the identity is not stringly-typed at the seam while still being printable.

  Rejected: a `&'static str` agent id at the seam (it loses exhaustiveness at exactly the site D13 wants it); rejected: putting `AgentKind` in `common` so both crates could share it (a UI concept in the config crate, and it would make this root feature depend on a crate it has no other reason to touch); rejected: emitting `AgentSkin` directly (inverts the dependency direction, breaks SD3, and cannot compile in build order).

### D12: Display toggles reach the worker through `set_enabled`, not through `common::config`
- **Status**: resolved
- **Rationale**: SD7 puts `show_git_branch` and `show_agent_status` in a `[sidebar]` TOML section, and `sidebar-foundations` D1 defines them. But `crates/common/src/config.rs` is **`sidebar-foundations`'s declared surface**, and this feature is a `depends_on: []` root that may be built first — so `common::config::SidebarConfig` may not exist when this code compiles. Referencing it would create an undeclared dependency edge and break the build order the partition establishes.

  So the worker learns the toggles through `TabMetaHandle::set_enabled(git: bool, agent: bool)`, defaulting to `true`/`true`, and `sidebar-row-redesign` — which depends on both features and already reads the config — wires `cfg.sidebar.show_git_branch` / `show_agent_status` to it **at startup, from `main.rs`** — a file it owns. Live reload is deliberately *not* claimed here: `crates/app/src/settings.rs` (whose `apply` is the reload path, `settings.rs:23-34`) belongs to **`sidebar-interaction`** per the manifest, not to `sidebar-row-redesign`. So a mid-session toggle of these two keys takes effect on the next launch until `sidebar-interaction` adds the one `set_enabled` line to `settings::apply`. Saying otherwise would be a seam claim about a file the claimant does not own.

  The toggles are not cosmetic filters at the draw site: disabling a source makes the worker **stop computing it**, and clears that field on the next publish. Turning off `show_agent_status` on a machine where the process walk is expensive or the argv `sysctl` is denied therefore actually removes the cost, which is the only reason a user would reach for the switch. Rejected: reading `common::config` directly (undeclared dependency, may not compile in build order); rejected: ignoring the toggles here and letting the renderer hide the fields (the work would still be done twice a second, so the setting would be a lie); rejected: a `TabMetaConfig` struct at the seam (two `bool`s do not need a type, and a struct would need versioning the moment a third knob appears — which `set_enabled`'s signature makes an honest breaking change instead).

### D13: Every syscall gets a self-process test; every bounded algorithm is tested with no syscall at all
- **Status**: resolved
- **Rationale**: This feature is almost entirely composed of things that are hard to test — kernel calls, another process's existence, a background thread's timing. The repo already contains the answer to the first and the third; the second needs a deliberate design choice.

  1. **Raw syscalls → one self-process test each**, following `session.rs:169-185` exactly, which calls `cwd_of_pid(std::process::id())` and asserts it equals `std::env::current_dir()`. This feature owes four such tests: `cwd_of_pid(own)` equals `current_dir()`; `exe_path(own)` ends in the test binary's file name; `children_of(own)` returns without error (a count of zero is a pass — the point is that the ABI and buffer sizing are right, not that we have children); and `proc_args(own)` contains the test binary's own path. These are the tests that would fail loudly if an entitlement or a libc ABI change broke stage 2 of D5, rather than letting it degrade silently on every user's machine.
  2. **The bounded walk → generic over a children-provider, so it needs no syscall.** `fn walk(root: u32, children: impl Fn(u32) -> Vec<u32>, classify: impl Fn(u32) -> Option<AgentKind>) -> Option<AgentKind>` lets every bound be tested against a hand-built fake tree: depth 4 is not reached; a 200-wide level stops at `MAX_NODES`; a match at depth 1 short-circuits before a sibling subtree is visited; a node returning an empty child list (the dead-pid case) does not abort the remaining queue. This mirrors `tabs.rs:4-5,160-185`, where the index arithmetic is factored into free functions "so it is unit-tested without spawning a shell", and it means the **only** untested-by-fake part of D6 is the three syscalls, which (1) covers.
  3. **Pure parsers → table-driven tests.** `parse_head` (ref line, nested branch name, 40-hex SHA, CRLF, empty, garbage, over-length), `classify_exe` (each table entry, case variations, a near-miss like `claude-foo`), `parse_procargs` (a synthetic `argc`-prefixed NUL-delimited buffer), `state_from(role, mtime, now)` (both `Busy` clauses, and `Idle`).
  4. **Filesystem behaviour → hand-rolled temp dirs**, per `ai-monitor/src/lib.rs:220-226` and `common/src/watch.rs:94-100`. **No `tempfile` dev-dependency is added** — none exists in the workspace and `Cargo.lock` edits are hook-blocked. This covers `.git` as a directory, `.git` as a `gitdir:` file, the ancestor walk finding a repo three levels up, the ancestor cap, and `tail_state` against a synthetic JSONL (including a file larger than the 8 KiB window, to prove the tail is what is read).
  5. **The worker's decision logic → extracted from the thread.** The differ (`did_anything_change`), the tick scheduler (`should_refresh(is_active, tick_index, tab_slot)`) and both memo validators are free functions tested directly. The thread body itself is a loop over them and is not unit-tested; **no test spawns the worker thread and sleeps on a timer**, because a timing-dependent test on a 500 ms cadence is a flake generator and the repo's only process/timing-dependent test is `#[ignore]`d (`term-core/tests/pty_smoke.rs:15`).
  6. **Cost claims are observed, not asserted.** Several requirements state syscall and I/O *counts* ("zero syscalls on an empty roster", "no more than 10 per tick", "`proc_args` is never invoked", "one `stat`, no read"). Written as prose those are unfalsifiable, and a spec that asserts a performance property it cannot observe has stated a wish. So `tabmeta` carries a **`#[cfg(test)] mod probe`** of `AtomicU64` counters — `probe::hit(&probe::CWD)`, compiled to nothing in release — incremented at every raw syscall entry point in `proc.rs` (`CWD`, `CHILDREN`, `EXE`, `ARGS`) and every filesystem entry point in `git.rs` (`GIT_STAT`, `GIT_READ`, `GIT_READ_BYTES`) and the activity path (`LOG_RESOLVE`, `LOG_STAT`, `LOG_READ`, `LOG_READ_BYTES`). Tests then drive one derivation pass over a temp-dir fixture and **assert the counter deltas**, which is what turns R0.2 / R0.3 / R1.8 / R3.6 / R4.4 / R6.7 into real gates; the two `*_BYTES` counters do the same for the two *capped-read* claims (R6.1's 8 KiB tail and R7.10's 4 KiB HEAD cap), which are the same class of unobservable assertion and would otherwise have been left as prose. This is the standard the `Vec::capacity()` assertion sets for `sync`'s no-allocation claim (D2): a cost claim is only a requirement if a test can fail on it.
  **The whole `probe` module and its full counter set are declared in the module scaffold (T1), not by whichever task first uses one** — otherwise `git.rs` and the activity path would each depend on the task that introduced `proc.rs`'s counters, and the three would stop being parallel. Rejected: making every derivation closure-generic the way `walk` is (it would push injectable parameters through `git.rs`, the activity path and the worker for a property a counter measures directly); rejected: measuring wall-clock time (a flake generator, and it would not distinguish "did no I/O" from "did fast I/O", which is the actual claim).

  Rejected: an integration test that spawns a real agent CLI (nothing may be assumed installed, and the repo has no non-`#[ignore]`d process-spawning test); rejected: a `trait ProcSource` with a production impl and a mock (the closure-generic form gets the same testability with no trait, no dyn dispatch, and no second implementation to keep in sync); rejected: testing the cadence by sleeping (flaky by construction).

### D14: Every failure degrades to absent metadata; the worker cannot panic and cannot die
- **Status**: resolved
- **Rationale**: This feature reads three unreliable sources on a background thread whose death would be **completely invisible** — the handle stays alive, `sync` keeps returning `false`, and the sidebar shows the last published values forever with nothing anywhere indicating that metadata stopped updating. That failure mode is worse than being wrong, so the policy is absolute:
  - **No `unwrap()`, `expect()`, or slice indexing on any path** in `tabmeta/` or in `ai-monitor`'s new module, outside `#[cfg(test)]` (CLAUDE.md's rule, load-bearing here). Every fallible step is `Option`/`Result` and every `None` means "this field is absent".
  - **No error type is surfaced.** There is no `TabMetaError`. A tab whose cwd cannot be resolved has `branch: None`; a tab whose walk finds nothing has `agent: None`. The UI cannot distinguish "failed" from "nothing there", and it should not: both mean "draw no badge".
  - **Logging is `debug!`, not `warn!`.** A dead pid, a missing `.git`, a failed `sysctl` are all normal. The one exception, added because the Inversion Probe named it as the feature's most likely silent failure: when an agent **is** detected but no activity source can be resolved for it, log once per `(TabId, AgentKind)` transition at `debug!` — so a user reporting "the badge never says ready" has something to look at. `tracing` is already an `app` dependency (`crates/app/Cargo.toml:24`). Note this logging lives in `tabmeta`, **not** in `ai-monitor`, which has no `tracing` dependency and whose `Cargo.toml` is out of surface.
  - **A failure to spawn the thread is not fatal.** `TabMetaHandle::spawn` returns `Option`; `main.rs` logs at `warn!` (this one *is* a warning — it happens once and it disables a feature) and continues with no handle installed, so `global()` returns `None` and every consumer call is a no-op. `TabMetaPump` is likewise not registered, because it holds a handle by value and cannot exist without one. The terminal starts and works; it just has no badges.
  - **Shutdown is by hangup.** The worker's `recv_timeout` returning `Disconnected` breaks the loop, exactly as the PTY reader breaks on a failed send (`session.rs:58-59`). Dropping the last `TabMetaHandle` therefore ends the thread cleanly.
  - **Stale beats blank.** If a tick fails entirely, the previously published values remain — a branch name that is a second out of date is more useful than a row that flickers empty. Staleness is bounded in practice by the consumer re-pushing the roster on every tab action.

  Rejected: a `thiserror` error enum for this module (CLAUDE.md asks for per-crate error enums, and `app` has none — it uses `anyhow_lite` (`session.rs:129-131`) — and more fundamentally there is no caller who could act on a distinction between "no repo" and "read failed"); rejected: `warn!` on syscall failures (a closed tab produces several per tick and the log would be unusable); rejected: catching panics with `catch_unwind` (it would paper over a real bug and the no-panic rule makes it unnecessary); rejected: restarting the worker on death (it cannot die, and a restart loop would hide whatever caused it).

## Constraints
- **No new crate dependency anywhere.** `libc` (`app/Cargo.toml:17`), `tracing` (`:24`), `serde_json` and `notify` (`ai-monitor/Cargo.toml`) are already present and sufficient. `Cargo.lock` edits are blocked by a PreToolUse hook, and neither `Cargo.toml` is in the declared surface.
- **`crates/ai-monitor/src/**` additions must compile against `std` + `serde_json` only** — that crate has no `common`, no `libc`, no `tracing`, and its `Cargo.toml` cannot be edited.
- **No `#[cfg(target_os = ...)]` branches** (`.claude/rules/macos-only.md`). The `proc_*` and `sysctl` calls are used unconditionally.
- **No `unwrap()`/`expect()` outside `#[cfg(test)]`** (CLAUDE.md), and no panic path in the worker (D14).
- **`term-core` is untouched.** `Pty::child_pid()` is consumed by the consumer feature, not by this one; this feature receives a `u32`.
- **`crates/app/src/tabs.rs`, `terminal_app.rs`, `session.rs`, `seam.rs`, `input.rs`, `settings.rs` are NOT in the declared surface** and must not be edited. The only app file touched outside `tabmeta/` is `main.rs`.
- **No renderer import in `crates/app/src/tabmeta/`** — not `AgentSkin`, not anything (D11). This must remain true even after `sidebar-foundations` lands.
- **The UI thread must perform no filesystem or process I/O on this feature's behalf** (SD4). The only UI-thread work is `TabMetaPump::poll`'s atomic load and `sync`'s conditional refill.
- **No test spawns a process** (the workspace's only such test is `#[ignore]`d) and **no test sleeps on the worker's cadence**.
- **No `tempfile`.** Temp dirs are hand-rolled per `ai-monitor/src/lib.rs:220-226`.
- All three gates green: `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`.

## Known Gaps
- **Whether the user's `claude` is a native binary or a node script is not verifiable from this repo**, and it decides whether D5 stage 1 or stage 2 does the work. Both are implemented, but only stage 1 is testable without an agent installed. First real confirmation is a `/run-term` session with an agent actually running.
- **`KERN_PROCARGS2` may be denied under the signed local `.app` bundle.** D13's self-process test proves the call works for the *test binary*; it does not prove it works from the signed app against a child process. If it is denied, node-installed agents show no badge and there is no fallback (D5 states this). A human should check once the `.app` is built.
- **`encode_cwd` (`ai-monitor/src/lib.rs:25-30`) maps only `/` and `.` to `-`**, while its doc comment claims "non-alphanumeric path separators". A cwd containing a space (or `@`, `+`, …) will not resolve a Claude log, so activity silently stays `Unknown` for such directories. Pre-existing, inside this feature's declared surface, and **deliberately not fixed here** — changing the encoding would change which log the *existing* ai-monitor overlay resolves, which is a behaviour question for a human, not a side effect of a sidebar feature.
- **The 500 ms / 2 s cadence and the 10 s `BUSY_WINDOW` are reasoned, not measured.** They are named constants in one place. Revisit after the first `/run-term` with a real agent; the specific risk is `BUSY_WINDOW` being too short for an agent sitting in a long tool call, which would flicker `Busy → Idle → Busy`.
- **pid reuse can mis-attribute an agent for at most one tick** (≤ 2 s) on a tab that is being closed. D6 bounds it rather than eliminating it; the fix (a pid + start-time pair via `PROC_PIDTBSDINFO`) costs a syscall per tab per tick and was judged not worth it. Revisit if it is ever observed.
- **`.git/HEAD`'s `(mtime, len)` is not a perfect change detector.** Two checkouts within the same filesystem mtime granularity to branch names of equal length would be missed until the next cwd change. Accepted; the observable cost is a stale branch label.
- **A tick can be stalled by a hung `stat` on an unresponsive network mount.** There is no per-tab timeout. This is a pre-existing property — `Tabs::titles()` has the same exposure today on the UI thread — and this feature at least moves it off the UI thread; but it is not *fixed*.
- **The agent table is closed and hand-maintained** (D5). A new agent CLI is invisible until someone adds a row. There is no config-driven extension because the config surface belongs to another feature; adding one later is a `set_enabled`-style seam addition, not a breaking change.
- **`TabMetaPump` uses the `Overlay` trait for a headless service** (D3). It works, it is cheap, and it is named as a wart. If `sidebar-interaction` later adds a proper service hook to `seam.rs` (its surface includes neither `seam.rs` nor this pump, so this would be new scope), the pump should move to it.
- **An abandoned turn can read `Busy` for up to `STALE_LIMIT` (100 s).** D7's role clause fires on a last line whose role is `user`, and Claude Code writes tool results as `"type":"user"` lines (`parse_line` takes top-level `type` first, `ai-monitor/src/lib.rs:79-84`). The `STALE_LIMIT` bound stops it being *forever*, but a session interrupted mid-tool-call still reads `Busy` until the bound expires. Distinguishing a tool result from a genuine user turn would mean inspecting `/message/content` for `tool_result` blocks — a real refinement, deferred because it depends on a JSONL shape that is not versioned and would need re-checking on every Claude Code release.
- **The memo cap (`MEMO_MAX = 64`, `MEMO_TTL_TICKS` ≈ 60 s) is a bound, not a measurement.** It is sized so a normal session never evicts and a pathological `cd` loop cannot grow the map without limit. If a user habitually keeps more than 64 distinct working directories warm, they pay a re-walk. Nothing observes this today.
- **`crates/app/src/main.rs` is in three features' declared surfaces** (this one, `sidebar-row-redesign`, `sidebar-interaction`) — a manifest-level property, not this spec's defect, but this spec's exposure: T1 edits `main.rs`'s `mod` block and its line 44 (`proxy` → `proxy.clone()`), which `sidebar-row-redesign` will then edit again. The ordering in the manifest makes that sequential rather than concurrent, so it is a merge ordering constraint, not a conflict — but a build that ran the two out of order would collide.
- **Nothing in this feature is visually verified.** Its entire output is consumed by `sidebar-row-redesign`. If the seam is subtly wrong — wrong ordering, a `TabId` that never matches — it will surface there, not here.

## Requirements

### R0: The module shape holds, the feature is inert without a consumer, and it costs the UI thread nothing (D1)

#### R0.1: Shipping this feature alone changes nothing observable
- **Given**: the terminal built at the commit before this feature and at the commit after it, with no consumer wiring `set_roster`
- **When**: both are launched and the same tabs are opened
- **Then**: the sidebar, grid and every rendered pixel are identical, and the worker thread exists but is blocked in `recv()` having performed zero syscalls

#### R0.2: An empty roster does no work at all
- **Given**: the derivation pass invoked with an empty roster, with D13(6)'s `probe` counters zeroed
- **When**: it runs
- **Then**: every counter (`CWD`, `CHILDREN`, `EXE`, `ARGS`, `GIT_STAT`, `GIT_READ`, `GIT_READ_BYTES`, `LOG_RESOLVE`, `LOG_STAT`, `LOG_READ`, `LOG_READ_BYTES` — all eleven declared in D13(6)) is still 0, `generation()` is still 0, and `wake` was invoked zero times — and, structurally, the worker in this state is blocked in `recv()` with no timeout, so it does not even wake

#### R0.3: The UI thread performs no I/O for this feature
- **Given**: a `TabMetaHandle` and a filled `TabMetaSnapshot`, with `probe` counters zeroed
- **When**: `TabMetaPump::poll` and `sync` are each called 1000 times with no intervening publish — the per-frame UI-thread work this feature adds
- **Then**: every `probe` counter is still 0, proving the UI-thread path contains no `proc_*` call, no `sysctl` and no file open; the only work is one atomic load per call

#### R0.4: All three gates are green with no new dependency
- **Given**: the completed feature
- **When**: `cargo build`, `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` are run, and `git diff` is inspected
- **Then**: all three exit 0 with no new warnings, no `unwrap()`/`expect()` outside `#[cfg(test)]`, and neither `Cargo.lock` nor any `Cargo.toml` is modified

#### R0.5: The whole feature runs on exactly one thread, whatever the tab count
- **Given**: a terminal with 1 tab and the same terminal with 20 tabs
- **When**: the threads this feature owns are counted
- **Then**: it is exactly **one** in both cases — thread count does not track user behaviour, and no per-tab thread, `notify` watch or timer exists (D1)

#### R0.6: The cache crosses the thread boundary, so it cannot use the UI-thread handle idiom
- **Given**: `TabMetaShared`
- **When**: its type is inspected
- **Then**: it is `Arc<..>` with an `AtomicU64` generation and a `Mutex`-guarded `Vec` — **not** the `Rc<RefCell<..>>` shape `TabModelHandle` uses (`tab_bar.rs:30-54`), which is UI-thread-only and would not compile across the worker boundary — and it is `Send + Sync`, which is what makes `TabMetaHandle: Clone + Send + Sync` possible (D1, D2)

### R1: The seam `sidebar-row-redesign` programs against (D2, D12)

#### R1.1: A fresh snapshot is filled on first sync
- **Given**: a `TabMetaHandle` that has published at least once, and a `TabMetaSnapshot::default()`
- **When**: `handle.sync(&mut snap)` is called
- **Then**: it returns `true`, and `snap.get(id)` yields the published `TabMeta` for every roster tab

#### R1.2: An unchanged generation costs nothing
- **Given**: a `TabMetaSnapshot` just filled by `sync`, with no intervening publish
- **When**: `sync` is called 1000 more times
- **Then**: every call returns `false`, `snap`'s contents are unchanged, and its `Vec::capacity()` is identical to after the first call — proving no reallocation and no refill occurred

#### R1.3: A publish makes the next sync refill
- **Given**: a filled snapshot and a worker publish that changed one tab's branch
- **When**: `sync` is called
- **Then**: it returns `true` and that tab's `TabMeta.branch` is the new value, while the other tabs' values are unchanged

#### R1.4: The roster is idempotent
- **Given**: a running worker with roster `[(TabId(0), 100), (TabId(1), 200)]`
- **When**: `set_roster` is called again with the identical slice
- **Then**: no memo is invalidated, no immediate refresh is forced, and `generation()` does not move

#### R1.5: A removed tab disappears from the snapshot without evicting a shared memo
- **Given**: a roster of three tabs — two of them in the *same* repo, i.e. sharing one cwd-keyed memo entry — then `set_roster` with only two tabs, dropping one of that pair
- **When**: the next publish and `sync` occur
- **Then**: `snap.get(removed_id)` is `None`, and the shared cwd memo entry is **retained** (the surviving tab still reads it), because the memos are keyed by cwd, not by `TabId` (D9, D7)

#### R1.6: Memo entries are reclaimed by disuse, not by tab removal
- **Given**: a memo holding entries for cwds no tab has resolved for many ticks
- **When**: a tick completes
- **Then**: entries not touched within `MEMO_TTL_TICKS` are dropped, and the memo never exceeds `MEMO_MAX = 64` entries (oldest-touched evicted first) — so a session that `cd`s through hundreds of directories cannot grow it without bound

#### R1.7: `set_active` refreshes the newly focused tab in band
- **Given**: `TabId(3)` polling at the 2 s background rate, with the worker blocked in `recv_timeout`
- **When**: `set_active(Some(TabId(3)))` is called 490 ms into a tick
- **Then**: the worker refreshes `TabId(3)` on receiving the command — **before** returning to `recv_timeout`, not on the next tick boundary — it subsequently polls at the 500 ms rate, and `tick_index` is **not** reset, so the background tabs' stagger phase (R3.2) is undisturbed

#### R1.8: Disabling a source stops the work and clears the field
- **Given**: a tab reporting `branch: Some("main")` and `agent: Some(..)`, with `probe` counters zeroed
- **When**: `set_enabled(false, true)` is called and a derivation pass runs
- **Then**: the publish has `branch: None` for every tab, and `probe::GIT_STAT` and `probe::GIT_READ` are both 0 — no `.git` ancestor walk and no HEAD `stat` was performed, so the toggle removes the cost rather than merely hiding the field

#### R1.9: The handle is obtainable without constructor injection, for the two methods that need it
- **Given**: `main.rs` has called `tabmeta::install(handle)`
- **When**: code inside `Tabs` calls `tabmeta::global()` to reach `set_roster` / `set_active`
- **Then**: it receives `Some(&TabMetaHandle)`; and when `install` was never called (spawn failed), it receives `None` and both calls become no-ops

#### R1.10: The read path needs no global, and lands on the one per-frame hook that exists
- **Given**: the consumer's sidebar object, constructed in `main.rs` — a file `sidebar-row-redesign` owns — and handed a `TabMetaHandle`, which it **clones** into the `Box<dyn DrawLayer>` its `layer(&self)` mints
- **When**: that `DrawLayer::record(&mut self, ctx)` runs on a frame and calls `sync(&mut self.snap)`
- **Then**: it compiles and works — `record` is the only per-frame `&mut self` callback in the app (`renderer/src/lib.rs:141`), whereas `layer()` fires once at registration (`terminal_app.rs:146-150`) and `Overlay::poll` only on `OverlayPending` — and neither `sync` nor `set_enabled` touches `global()`, so the locator's blast radius is exactly `set_roster` and `set_active`

#### R1.11: Unit tests never touch the global
- **Given**: this feature's test suite
- **When**: it is searched for `global()` and `install()`
- **Then**: no unit test calls either — every test constructs its own `TabMetaHandle`, so the `OnceLock` has no effect on any test

### R2: The wake path, without a new `UserEvent` variant (D3)

#### R2.1: A publish wakes the loop through `OverlayPending`
- **Given**: a registered `TabMetaPump` and a worker tick that changed something
- **When**: the worker invokes its `wake` callback
- **Then**: `UserEvent::OverlayPending` is sent, `OverlayHost::poll_all` calls `TabMetaPump::poll`, it returns `true`, and `Running::redraw()` runs

#### R2.2: The pump never sees a keypress
- **Given**: a registered `TabMetaPump` and any key event
- **When**: `OverlayHost::dispatch` runs
- **Then**: the pump's `on_key` is never called, because `visible()` is `false` and `dispatch` skips invisible overlays

#### R2.3: The pump's layer draws nothing
- **Given**: the pump's `DrawLayer`
- **When**: `record(ctx)` is called on every frame
- **Then**: it returns immediately having recorded no pass, and its `z_order` places it below every real layer

#### R2.4: `UserEvent` and `terminal_app.rs` are unchanged
- **Given**: the diff for this feature
- **When**: it is inspected
- **Then**: `crates/app/src/terminal_app.rs` does not appear in it, and `UserEvent` still has exactly three variants

#### R2.5: `main.rs`'s existing proxy move still compiles
- **Given**: `main.rs:44`, which moves `proxy` into `AiMonitorOverlay::new`
- **When**: this feature's `TabMetaHandle::spawn` also needs a proxy for its wake closure
- **Then**: the existing line becomes `proxy.clone()` and both registrations compile, with the ai-monitor overlay's behaviour unchanged

### R3: Cadence and change-only publication (D4)

#### R3.1: The active tab refreshes four times more often than a background tab
- **Given**: `should_refresh(is_active, tick_index, tab_slot)` and 8 consecutive ticks
- **When**: it is evaluated for an active tab and for a background tab
- **Then**: the active tab refreshes on all 8 and the background tab on exactly 2 — a 500 ms and a 2 s effective interval at `TICK = 500ms`

#### R3.2: Background tabs are spread across the tick phases
- **Given**: 20 background tabs at roster slots 0..19
- **When**: `should_refresh` is evaluated for each over 4 consecutive ticks
- **Then**: exactly 5 tabs refresh on each tick and every tab refreshes exactly once across the four — no tick refreshes all 20, so the per-tick syscall burst is bounded at a quarter of the tab count

#### R3.3: An unchanged tick publishes nothing
- **Given**: a tick whose freshly derived `TabMeta` equals the last published value for every tab
- **When**: the differ runs
- **Then**: the mutex is not taken, `generation()` does not move, and `wake` is not invoked — so no `redraw()` and no grid re-snapshot occurs

#### R3.4: A single changed field publishes once
- **Given**: five tabs, one of which changed its agent state from `Busy` to `Idle`
- **When**: the tick completes
- **Then**: `generation()` advances by exactly 1 and `wake` is invoked exactly once, carrying all five tabs in the new snapshot

#### R3.5: A dropped roster ends the thread
- **Given**: a running worker
- **When**: the last `TabMetaHandle` is dropped, disconnecting the command channel
- **Then**: `recv_timeout` returns `Disconnected` and the thread's loop breaks and returns, exactly as the PTY reader breaks on hangup

#### R3.6: The steady-state syscall budget is bounded
- **Given**: a temp-dir fixture standing in for an active tab in a git repo with an idle agent log, both memos warm from a prior pass, and `probe` counters zeroed
- **When**: one derivation pass runs
- **Then**: the summed counters are ≤ 10 — `CWD` = 1, `CHILDREN` ≤ 2, `EXE` ≤ 2, `GIT_STAT` = 1, `LOG_STAT` = 1 — and `GIT_READ`, `LOG_READ` and `LOG_RESOLVE` are all **0**, i.e. zero file reads and zero directory scans

### R4: Agent presence and identity (D5)

#### R4.1: A directly-launched agent is identified by executable basename
- **Given**: a zsh whose direct child's executable path ends in `/claude`
- **When**: the walk classifies it
- **Then**: the tab reports `agent: Some(Agent { kind: AgentKind::Claude, .. })`

#### R4.2: Basename matching is case-insensitive and exact
- **Given**: candidate basenames `Claude`, `CLAUDE`, `claude-foo`, `myclaude`, `node`
- **When**: `classify_exe` is applied to each
- **Then**: the first two yield `Claude`, and the last three yield `None` — a near-miss is not an agent

#### R4.3: An interpreter-launched agent is identified from its argv
- **Given**: a descendant whose executable basename is `node` and whose argv contains `/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js`
- **When**: stage 2 runs
- **Then**: it yields `AgentKind::Claude`, because a path component matches a known packaged agent name

#### R4.4: `sysctl` is not called for non-interpreter processes
- **Given**: a classifier pass over the basenames `rustc`, `cc` and `ld`, with `probe::ARGS` zeroed
- **When**: the classifier runs
- **Then**: `probe::ARGS` is still 0 — `proc_args` was never invoked, because none of those basenames is in the interpreter table, which is what keeps a 60-process build tree from costing 60 `sysctl` calls per tick

#### R4.5: A failed argv read means "not an agent"
- **Given**: a `node` descendant whose `KERN_PROCARGS2` `sysctl` fails
- **When**: stage 2 runs
- **Then**: it yields `None`, the walk continues with the remaining queue, and nothing is logged at `warn`

#### R4.6: A plain zsh tab reports no agent
- **Given**: a tab whose zsh has no children, or only children like `vim` and `less`
- **When**: the walk runs
- **Then**: the tab reports `agent: None`

#### R4.7: The raw syscalls work against our own process
- **Given**: the test process
- **When**: `exe_path(std::process::id())`, `children_of(std::process::id())` and `proc_args(std::process::id())` are called
- **Then**: the exe path ends in the test binary's file name, `children_of` returns without error, and the argv contains the test binary's own path — so an entitlement or ABI failure fails here rather than silently on a user's machine

### R5: The walk is bounded, and dead pids are normal (D6)

#### R5.1: The walk stops at `MAX_DEPTH`
- **Given**: a fake process tree with a matching agent at depth 4 below the root
- **When**: `walk` runs with `MAX_DEPTH = 3`
- **Then**: it returns `None` — the depth-4 node is never visited

#### R5.2: The walk stops at `MAX_NODES`
- **Given**: a fake tree whose first level has 200 non-matching children and a match beyond the 64th
- **When**: `walk` runs
- **Then**: it visits at most 64 nodes and returns `None`

#### R5.3: A match short-circuits the walk
- **Given**: a fake tree with a match as the root's first child and a large subtree under its second child
- **When**: `walk` runs
- **Then**: it returns the match having visited no node in the second subtree

#### R5.4: A dead node does not abort the walk
- **Given**: a fake tree where the second of three siblings returns an empty child list (the dead-pid case) and the third is a match
- **When**: `walk` runs
- **Then**: it returns the match — the empty result is skipped, not treated as termination

#### R5.5: Per-level children are capped, provably without spawning 128 processes
- **Given**: `parse_childpids(buf: &[i32], returned: c_int) -> Vec<u32>` — the pure half of `children_of`, split out exactly as `parse_procargs` is — fed a 128-slot buffer together with a `returned` value the kernel would report for 300 children, and separately with `returned` values of `0`, `-1` and one exceeding the buffer
- **When**: each case is evaluated
- **Then**: the over-full case yields exactly 128 pids with the excess dropped and no panic; `0` and `-1` both yield an empty `Vec`; and no test spawns a process, because the truncation logic now lives above the syscall rather than inside it

#### R5.6: A dead root pid yields no agent
- **Given**: a roster entry whose zsh pid has already exited
- **When**: the tick refreshes that tab
- **Then**: it reports `agent: None` and `branch: None`, with nothing logged at `warn` and no panic

### R6: Agent activity from the session JSONL (D7, D8)

#### R6.1: `tail_state` reads only the tail of a large log
- **Given**: a JSONL file of 200 KiB whose last line is a parseable assistant event
- **When**: `ai_monitor::tail_state(&log)` is called
- **Then**: it returns that line's role along with the file's `mtime` and `len`, and it is correct for a file whose first 192 KiB contain lines that would parse differently — proving the *tail*, not the head, was read.
  **Honest limit on the byte assertion**: `probe` is `#[cfg(test)]` inside `crates/app`, and this read happens in the separate `ai-monitor` crate, which returns no byte count. So `probe::LOG_READ_BYTES` is incremented by the app-side wrapper with the value it *computes* (`min(len, 8192)`), which checks the wrapper's arithmetic but cannot observe `ai-monitor`'s actual read. The 8 KiB bound is enforced by construction in `tail_state`'s seek offset and covered functionally by the Then above; it is the one cost claim in this spec that is not independently observed, and that is stated rather than implied.

#### R6.2: A recently-appended log reads as busy
- **Given**: a resolved log whose `mtime` is 2 seconds old and whose last role is `assistant`
- **When**: the busy/idle rule is applied with `BUSY_WINDOW = 10s`
- **Then**: the state is `Busy` — the recency clause fires

#### R6.3: An unanswered user turn reads as busy even when stale
- **Given**: a log whose `mtime` is 60 seconds old and whose last line's role is `user`
- **When**: the rule is applied
- **Then**: the state is `Busy` — the role clause fires, covering a submitted turn the assistant has not yet written to

#### R6.4: A settled assistant turn reads as idle
- **Given**: a log whose `mtime` is 60 seconds old and whose last role is `assistant`
- **When**: the rule is applied
- **Then**: the state is `Idle`, which is the state that earns the sidebar's ready mark

#### R6.5: A detected agent with no activity source is `Unknown`
- **Given**: a tab running `AgentKind::Aider`, and separately a `Claude` tab whose log cannot be resolved
- **When**: activity is derived
- **Then**: both report `AgentState::Unknown`, and a `debug!` is emitted once per `(TabId, AgentKind)` transition so the blank case is diagnosable

#### R6.6: `Unknown` renders as present, never as ready
- **Given**: the contract this feature places on its consumer
- **When**: `AgentState::Unknown` is mapped in `sidebar-row-redesign`
- **Then**: it takes the "agent present" affordance (the `✳` mark and the agent skin), never the `◇` ready affordance — because presence is the one thing `Unknown` asserts

#### R6.7: An unchanged log is not re-read
- **Given**: a temp-dir log whose `(mtime, len)` is unchanged since a prior derivation pass warmed the memo, with `probe` counters zeroed
- **When**: another pass derives activity
- **Then**: `probe::LOG_STAT` is 1 while `probe::LOG_READ` and `probe::LOG_RESOLVE` are both 0 — the cached role was reused with no read and no JSON parse

#### R6.8: The log path is memoised but expires
- **Given**: a tab whose log path was resolved 3 seconds ago, and the same tab 12 seconds later
- **When**: each tick derives activity
- **Then**: the first reuses the memoised path with no `read_dir`, and the second re-runs `resolve_log_in` — so a newly-started session is picked up within 10 seconds

#### R6.9: `ai-monitor`'s existing behaviour and manifest are untouched
- **Given**: the diff to `crates/ai-monitor`
- **When**: it is inspected
- **Then**: `Cargo.toml` is unmodified, no existing function's signature or body changed, the new module uses only `std` + `serde_json`, and the ai-monitor overlay behaves exactly as before

#### R6.10: The role clause is bounded, so an abandoned session does not read `Busy` forever
- **Given**: a log whose last line's role is `user` (which Claude Code also writes for tool results) and whose `mtime` is 200 seconds old — beyond `STALE_LIMIT = 10 × BUSY_WINDOW`
- **When**: the rule is applied
- **Then**: the state is `Idle`, not `Busy` — the role clause is ignored past the bound; and at 90 seconds old the same input still yields `Busy`, so the bound is where it is claimed to be

#### R6.11: There is no `Done` state
- **Given**: a tab whose agent process exits
- **When**: the next tick runs
- **Then**: the tab reports `agent: None` — the badge disappears — and `AgentState` has exactly three variants with no tombstone

### R7: Git branch (D9)

#### R7.1: A tab inside a repo reports its branch
- **Given**: a tab whose cwd is three directories below a repo whose `.git/HEAD` contains `ref: refs/heads/main`
- **When**: the tick derives the branch
- **Then**: it reports `branch: Some("main")`

#### R7.2: A branch name containing slashes survives verbatim
- **Given**: `.git/HEAD` containing `ref: refs/heads/feat/improving-tab-design\n`
- **When**: `parse_head` runs
- **Then**: it yields `feat/improving-tab-design` — nothing after `refs/heads/` is truncated

#### R7.3: A detached HEAD reads as a short SHA
- **Given**: `.git/HEAD` containing a bare 40-character hex SHA
- **When**: `parse_head` runs
- **Then**: it yields `@` plus the first 7 characters, so it is visibly not a branch name

#### R7.4: A `.git` file is followed to its real git dir
- **Given**: a worktree whose `.git` is a **file** containing `gitdir: ../../.git/worktrees/wt1`
- **When**: the resolver runs
- **Then**: it resolves the path relative to the containing directory and reads `HEAD` from there

#### R7.5: A tab outside any repo reports no branch
- **Given**: a tab whose cwd has no `.git` in any ancestor within the cap
- **When**: the tick runs
- **Then**: it reports `branch: None`, and a `NotARepo` memo entry short-circuits the ancestor walk on subsequent ticks

#### R7.6: The ancestor walk is capped
- **Given**: a cwd 60 directories deep with no `.git` anywhere
- **When**: the resolver runs
- **Then**: it examines at most `MAX_GIT_ANCESTORS = 40` directories and returns `None`

#### R7.7: `cd` into another repo changes the branch with no invalidation call
- **Given**: a tab on branch `main` in repo A
- **When**: the user runs `cd ../repoB` where repo B is on `develop`
- **Then**: within one tick the tab reports `develop` — because the cwd is re-resolved from the live pid every tick and the memo's cwd key no longer matches

#### R7.8: `git checkout` changes the branch with no process or cwd change
- **Given**: a tab whose branch memo is warm for cwd C on branch `main`
- **When**: the user runs `git checkout develop`, changing `.git/HEAD`'s `(mtime, len)`
- **Then**: the next tick's `stat` sees the change, re-reads HEAD, and reports `develop`

#### R7.9: `git init` in a memoised non-repo is picked up
- **Given**: a tab whose cwd is memoised as `NotARepo`
- **When**: the user runs `git init` and 10 seconds elapse
- **Then**: the negative entry expires, the ancestor walk re-runs, and the branch appears

#### R7.10: A pathological HEAD cannot inject an unbounded string
- **Given**: a `.git/HEAD` containing a 10 MB single line, and separately one containing binary garbage, with `probe::GIT_READ_BYTES` zeroed
- **When**: the branch resolver runs
- **Then**: `probe::GIT_READ_BYTES` is ≤ 4096 for each, the result is either `None` or a string of at most 64 characters, and nothing panics

### R8: cwd resolution inside `tabmeta` (D10)

#### R8.1: The reimplemented `cwd_of_pid` matches the kernel
- **Given**: the test process
- **When**: `tabmeta::proc::cwd_of_pid(std::process::id())` is called
- **Then**: it equals `std::env::current_dir()` after canonicalisation — the same assertion `session.rs:169-185` makes about its own copy

#### R8.2: A dead or unreadable pid yields `None`
- **Given**: a pid that does not exist
- **When**: `cwd_of_pid` is called
- **Then**: it returns `None` without panicking, and the tab's branch becomes `None`

#### R8.3: The roster carries pids, not paths
- **Given**: the `TabPid` type and `set_roster`'s signature
- **When**: they are inspected
- **Then**: they carry only `TabId` and `u32` — so no `proc_pidinfo` call for this feature happens on the UI thread

#### R8.4: `session.rs` is not modified
- **Given**: the diff for this feature
- **When**: it is inspected
- **Then**: `crates/app/src/session.rs` does not appear in it, and its private `cwd_of_pid` is untouched

### R9: The agent vocabulary is renderer-free (D11)

#### R9.1: No renderer type is named anywhere in `tabmeta`
- **Given**: `crates/app/src/tabmeta/**`
- **When**: it is searched for `renderer::`, `AgentSkin` and `sidebar_theme`
- **Then**: there are no matches — so this feature compiles with `crates/renderer/src/sidebar_theme.rs` absent, as it may be when built first

#### R9.2: `AgentKind` covers both skinned agents plus the generic ones
- **Given**: the `AgentKind` enum
- **When**: its variants are enumerated
- **Then**: they are `Claude`, `Antigravity`, `Codex`, `Gemini` and `Aider` — the first two matching the mock's two skins, the rest destined for `Generic`

#### R9.3: Adding an agent forces a decision at the mapping site
- **Given**: the contract this feature places on `sidebar-row-redesign`
- **When**: a sixth `AgentKind` variant is added
- **Then**: the mapping's exhaustive `match` (no `_` arm) fails to compile, so the new agent cannot silently render as nothing

#### R9.4: The identity is printable without being stringly-typed
- **Given**: `AgentKind::label(self) -> &'static str`
- **When**: each variant is labelled
- **Then**: each yields a stable lowercase name usable in a `debug!` line, while the seam itself still carries the enum

### R10: Failure policy (D14)

#### R10.1: No panic path exists outside tests
- **Given**: `crates/app/src/tabmeta/**` and `crates/ai-monitor/src/activity.rs`
- **When**: they are searched for `unwrap(`, `expect(`, and slice indexing on a fallible length
- **Then**: there are no matches outside `#[cfg(test)]`, and clippy is green with `-D warnings`

#### R10.2: A failed thread spawn is not fatal
- **Given**: a `TabMetaHandle::spawn` that returns `None`
- **When**: `main.rs` handles it
- **Then**: it logs at `warn!` and continues; `install` is never called so `global()` returns `None`; **`TabMetaPump` is not registered either** (it holds a handle by value and cannot be constructed without one); and the terminal starts and runs normally with no badges and no extra overlay

#### R10.3: No error type is surfaced
- **Given**: the module's public API
- **When**: it is inspected
- **Then**: there is no `TabMetaError` and no `Result` in the seam — every failure is expressed as an absent `Option` field

#### R10.4: Routine failures do not spam the log
- **Given**: a tab whose zsh has exited, refreshed for 60 consecutive ticks
- **When**: the log is inspected
- **Then**: no `warn!` was emitted; only `debug!`, and the detected-agent-without-activity-source message appears at most once per `(TabId, AgentKind)` transition

#### R10.5: A failed tick leaves the previous values visible
- **Given**: a tab reporting `branch: Some("main")` whose next tick fails to resolve the cwd transiently
- **When**: the tick completes
- **Then**: the derived value is `None`, the diff publishes it, and the UI shows no branch — but the worker keeps running and the value returns on the next successful tick, with no thread death and no permanent staleness

### R11: Testability (D13)

#### R11.1: Every bound in the walk is tested without a syscall
- **Given**: `walk` generic over a children-provider and a classifier closure
- **When**: R5.1–R5.4 are exercised with hand-built fake trees
- **Then**: all pass with no `proc_*` call made, mirroring `tabs.rs:160-185`'s syscall-free index tests

#### R11.2: Filesystem behaviour is tested with hand-rolled temp dirs
- **Given**: no `tempfile` dev-dependency in the workspace
- **When**: the `.git`, worktree, ancestor-cap and `tail_state` tests run
- **Then**: they build their fixtures with `std::env::temp_dir()` + `std::process::id()` per `ai-monitor/src/lib.rs:220-226`, and no `Cargo.toml` gains a dev-dependency

#### R11.3: No test spawns a process or sleeps on the cadence
- **Given**: this feature's test suite
- **When**: it is inspected
- **Then**: no test spawns a child process and no test sleeps waiting for a worker tick — the differ, scheduler and memo validators are tested as free functions instead

#### R11.4: Pure parsers are table-driven
- **Given**: `parse_head`, `classify_exe`, `parse_procargs`, `parse_childpids` and `state_from`
- **When**: their tests run
- **Then**: each covers its happy path, its malformed input, and its boundary — including a 40-hex SHA, a `claude-foo` near-miss, a synthetic `argc`-prefixed NUL buffer, an over-full child buffer with a `returned` of `0`/`-1`/`300`, and both `Busy` clauses plus the `STALE_LIMIT` edge

#### R11.5: Cost claims are observed by a counter, not asserted in prose
- **Given**: D13(6)'s eleven `#[cfg(test)] mod probe` counters, declared in T1 at every raw syscall and filesystem entry point
- **When**: the eight cost requirements run — R0.2, R0.3, R1.8, R3.6, R4.4 and R6.7 (call counts) plus R6.1 and R7.10 (the 8 KiB and 4 KiB read caps, via `LOG_READ_BYTES` / `GIT_READ_BYTES`)
- **Then**: each asserts a specific counter delta and fails if the implementation performs the I/O it claims to avoid — so **no cost claim in this spec is left as prose**, which is the standard the `Vec::capacity()` assertion in R1.2 sets for `sync`

## Tasks

### T1: Module skeleton, public types, handle/worker scaffold, pump, and `main.rs` wiring [infra]
- **Fulfills**: R0.5, R0.6, R1.1, R1.2, R1.3, R1.4, R1.9, R1.10, R1.11, R2, R9.1, R9.2, R9.4
- **Depends on**: (none)

Creates `crates/app/src/tabmeta/{mod.rs, worker.rs, proc.rs, agent.rs, git.rs, activity.rs}` with the public types (`TabMeta`, `Agent`, `AgentKind`, `AgentState`, `TabPid`, `TabMetaSnapshot`), the full `TabMetaHandle` API from D2, the `OnceLock` locator, `TabMetaPump`, **D13(6)'s complete `#[cfg(test)] mod probe` counter set** (all eleven, not just `proc.rs`'s — declaring them here is what keeps T2/T4/T5 parallel), and a worker loop that already implements the cadence's *shape* (block on `recv()` when the roster is empty, `recv_timeout(TICK)` otherwise) while every derivation returns `TabMeta::default()`. Adds `mod tabmeta;` to `main.rs:10-17`, changes `main.rs:44`'s `proxy` to `proxy.clone()`, spawns the handle, installs it, and registers the pump. This lands the whole seam first so the shape is fixed before any derivation exists — and because R1.2's no-reallocation assertion and R2.x's wiring are testable with an empty derivation. Land R9.1's grep assertion here too: the module must compile with no renderer import. **Not fulfilled here** despite being in R1: R1.5/R1.6 (memo lifetime → T4), R1.7 (`set_active` in-band → T6), R1.8 (`set_enabled` stops the work → T6); nor R9.3, a consumer obligation no task here can verify.

### T2: macOS process primitives [infra]
- **Fulfills**: R8, R4.7
- **Depends on**: T1

Fills `tabmeta/proc.rs`: `cwd_of_pid` (reimplemented per D10), `children_of` (fixed 128-entry `proc_listchildpids` buffer), `exe_path` (`proc_pidpath` with `PROC_PIDPATHINFO_MAXSIZE`), and `proc_args` (`sysctl([CTL_KERN, KERN_PROCARGS2, pid])` plus the `argc`-prefixed NUL-delimited parse), each incrementing its T1-declared `probe` counter. Each gets the self-process test from D13(1) — these four tests are the feature's early-warning system for an ABI or entitlement failure, so land them with the functions, not after. **Two pure halves are split out so their bounds are table-testable without a syscall or a spawned process**: `parse_procargs(buf: &[u8]) -> Vec<OsString>` (R11.4) and `parse_childpids(buf: &[i32], returned: c_int) -> Vec<u32>` (R5.5).

### T3: Bounded descendant walk and the two-stage classifier [infra]
- **Fulfills**: R4, R5
- **Depends on**: T2

Fills `tabmeta/agent.rs` **only** — it does not touch `worker.rs`. The const agent and interpreter tables, `classify_exe`, the stage-2 argv scan, and `walk` — **generic over a children-provider and a classifier closure** (D13(2)) so R5.1–R5.4 are tested against fake trees with no syscall — plus a single `agent_of_pid(pid) -> Option<AgentKind>` entry point that binds `walk` to T2's real syscalls. The `sysctl` gate (R4.4) is the load-bearing detail: it is what keeps a build tree from costing 64 `sysctl` calls per tick.

### T4: Git branch reader and its cwd-keyed memo [infra]
- **Fulfills**: R7, R1.5, R1.6
- **Depends on**: T1  ← parallel with T2/T3/T5

Fills `tabmeta/git.rs` **only**: the capped ancestor walk, `.git`-as-directory and `.git`-as-`gitdir:`-file resolution, `parse_head` (ref line / 40-hex SHA / garbage, with the 4 KiB read cap and 64-char output cap), the memo with its `Repo{(mtime,len)}` revalidation and its expiring `NotARepo` entry, and the disuse-based reclamation (`MEMO_TTL_TICKS`, `MEMO_MAX`) that R1.5/R1.6 assert — which is why those two sub-requirements land here rather than with the rest of R1. Increments T1's `GIT_STAT`/`GIT_READ`/`GIT_READ_BYTES` probes. Its entry point is `branch_of_cwd(&mut Memo, &Path, tick) -> Option<String>`, so it needs nothing from T2/T3 and is fully testable against temp dirs on its own.

### T5: `ai-monitor::tail_state` and the pure activity rule [infra]
- **Fulfills**: R6.1, R6.2, R6.3, R6.4, R6.7, R6.8, R6.9, R6.10, R6.11
- **Depends on**: T1  ← parallel with T2/T3/T4

Adds `crates/ai-monitor/src/activity.rs` + its `mod` line, using **only `std` + `serde_json`** — `Cargo.toml` must not change (R6.9) — implementing the bounded 8 KiB tail read on top of the existing `parse_line`. Fills `tabmeta/activity.rs` with the **pure** rule `state_from(role: &str, mtime: SystemTime, now: SystemTime) -> AgentState` (both `Busy` clauses, the `STALE_LIMIT` bound, `Idle`) and the two cwd-keyed memos, incrementing T1's `LOG_*` probes. Like T4 it exposes one entry point and **does not touch `worker.rs`**. R6.5's once-per-transition `debug!` needs the worker's transition tracking and lands in T6; R6.6 is a consumer obligation (below).

### T6: The worker loop — cadence, differ, publication, failure policy [vertical]
- **Fulfills**: R0.1, R0.2, R0.3, R0.4, R1.7, R1.8, R3, R6.5, R10, R11
- **Depends on**: T3, T4, T5

The **only** task that writes `tabmeta/worker.rs` beyond T1's scaffold, which is what keeps T2–T5 genuinely parallel. Wires T3's `agent_of_pid`, T4's `branch_of_cwd` and T5's activity path into one derivation pass; adds `should_refresh(is_active, tick_index, tab_slot)` with its stagger, the change differ built on `PartialEq for TabMeta`, the conditional publish (mutex + `Release` generation store + `wake`, all skipped when nothing changed), `set_active`'s in-band refresh (no `tick_index` reset), roster diffing, the cost assertions that read T1's `probe` counters, R6.5's once-per-transition `debug!`, and the D14 sweep — no `unwrap`/`expect`, `debug!` not `warn!`, hangup shutdown, stale-beats-blank. `should_refresh`, the differ and both memo validators are free functions so R11.3 holds (no test sleeps on the cadence).

**On R0's placement**: R0 is a whole-feature property, split by what can actually be proved when. R0.5/R0.6 (one thread; an `Arc`/`Mutex` cache rather than the UI-thread `Rc<RefCell<..>>` idiom) are structural facts of the scaffold, so they are **T1**'s. R0.4 (the gates) is run by every task per CLAUDE.md but is only finally true at the end. R0.1/R0.2/R0.3 (inertness, the zero-work empty roster, no UI-thread I/O) are established structurally in T1 yet only *provable* once the derivations exist and the `probe` counters can be shown to stay at zero — so they attach to **T6** as the last task rather than being duplicated across all six. R0 is not a disguised final-verify task; holistic verification belongs to the execute pipeline.

**On R6.6 and R9.3 — obligations, not deliverables.** Both are properties of `sidebar-row-redesign` (that `AgentState::Unknown` maps to the *present* affordance, and that the `AgentKind → AgentSkin` match is exhaustive with no `_` arm). They are stated in this spec because this feature is where the vocabulary is defined and a downstream reader must not have to infer them — but **no task here can verify them**, so they are listed under Post-work and are deliberately not counted toward T1's or T5's completion. Every other sub-requirement is verifiable inside this feature.

**DAG**: T1 → {T2 ∥ T4 ∥ T5}; T2 → T3; {T3, T4, T5} → T6. No cycles. T2/T4/T5 are genuinely parallel — **disjoint files** (`proc.rs` / `git.rs` / `tabmeta/activity.rs` + `ai-monitor/activity.rs`), no shared interface, all three depending only on T1's types, scaffold, empty module files and full `probe` counter set. T3 is serialised behind T2 because it consumes `children_of`/`exe_path`/`proc_args`. Two false-parallelism defects found on adversarial review are fixed here rather than papered over: T3 and T5 both wrote `worker.rs` (all worker wiring is now T6's alone), and T4/T5 asserted `probe` counters that T2 introduced while declared parallel with it (the whole counter set is now T1's, which is also why T1 creates all six module files including empty `activity.rs`).

**Horizontal-split justification**: T2–T5 are all BE-only in vertical-slice terms, which the L4 rule permits for *pure infrastructure* — this feature has no user-visible surface by construction (D11, R0.1), so there is no UI half to pair them with. T6 is the vertical slice in this feature's own terms: it is the task after which a wired consumer would actually observe metadata change. The user-visible slice arrives in `sidebar-row-redesign`.

## External Dependencies

### Pre-work
- (none) — every API this feature calls is already a dependency of the crates it edits, and both `Cargo.toml` files stay untouched.

### Post-work
- **A human must run the terminal with a real agent** (`/run-term`, then start `claude` in a tab) and confirm: which classification stage fires (D5 stage 1 vs. stage 2 — i.e. whether the installed `claude` is a native binary or a node script), that `KERN_PROCARGS2` is not denied under the signed local `.app` bundle, and that the badge tracks `Busy`/`Idle` sensibly with the 10 s `BUSY_WINDOW`. None of these is verifiable from this repo, and all three are recorded in Known Gaps. This blocks nothing in T1–T6, but it should settle before `sidebar-row-redesign` treats a missing badge as its own bug.
- **`sidebar-row-redesign` must call the seam.** Until it calls `set_roster` / `set_active` / `set_enabled` and reads via `sync`, this feature is inert by design (R0.1). The consumer also inherits two obligations stated here: the exhaustive `AgentKind → AgentSkin` match with no `_` arm (R9.3), and mapping `AgentState::Unknown` to the *present* affordance rather than the *ready* one (R6.6).
