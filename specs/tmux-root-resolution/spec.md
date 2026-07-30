# Spec: tmux-root-resolution

## Meta
- **Created**: 2026-07-29
- **Type**: dev
- **Status**: planned
- **Set**: tmux-pane-resolution (`specs/tmux-pane-resolution/partition-manifest.md`)
- **Mode**: batch (partition approved at decompose's single gate)
- **Approved by**: partition gate (`pre-approved-batch: yes`) — per-layer prompts bypassed
- **Approved at**: 2026-07-29
- **L2-reviewer**: PASS on retry 2 (composite 0.89; 8 + 7 findings addressed across two rounds)

## Declared Surface
- feature-id: tmux-root-resolution
- depends_on: []
- declared-surface:
  - crates/app/src/tabmeta/tmux.rs
  - crates/app/src/tabmeta/proc.rs
  - crates/app/src/tabmeta/agent.rs
  - crates/app/src/tabmeta/mod.rs

## Goal
Provide the mechanism that answers "which pid does this tab's metadata derivation start
from" — returning the tmux client's `pane_pid` when the tab's process tree contains an
attached tmux client, and the tab's own shell pid otherwise. Ship it as a self-contained,
headlessly-testable unit with its cost gated and counted, so the two consumers
(`tabmeta/worker.rs`, `tabsession.rs`) each need only a one-line substitution.

## Non-goals
- Wiring the resolver into the tab-card derivation — that is `tabmeta-tmux-reroot`.
- Wiring it into the ai-monitor session hint — that is `tabsession-tmux-reroot`.
- Changing anything downstream of the root pid: the depth-1 agent walk, `cwd_of_pid`, the
  git-branch lookup, the activity lookup and the task label all stay byte-unchanged (SD2).
- Following tmux *panes within a window* (`list-panes`), split layouts, or the pane the
  user's cursor is in. The unit of resolution is the **client's active pane**, which is
  what `#{pane_pid}` on a client row already reports.
- Nested tmux (tmux inside tmux) as an explicitly-designed case. It degrades naturally —
  the outer client's tty matches, yielding the outer pane's pid.
- Resolving a `dev_t` back to a device *name*. `libc::devname` **is** available (see
  Research), but this feature deliberately does not use it — the join runs numerically, in
  the path→rdev direction only (D1).
- Supporting non-default tmux sockets (`tmux -L <name>` / `-S <path>`). Default socket only;
  see D14 and Known Gaps.
- Any change to `term-core` / `renderer`. This is app-crate-local process observation.

## Confirmed Goal
A new `crates/app/src/tabmeta/tmux.rs` exposing a memoized `effective_root(memo, pid, tick)`
and an unmemoized `effective_root_uncached(pid)`, built from five separable pieces — a tmux
**presence gate** over the root pid then its direct children, returning the client it found
(D5); a `tmux list-clients` **spawn** of that client's own binary (D6); a **pure parser**
over its output (D12); an **rdev resolver** with the single injected `stat` seam (D12); and a
**fully pure pane-selector** over pre-resolved device numbers (D7b, D12) — plus
`proc::tty_dev_of_pid` for the tab's side of the join and three probe counters (`TMUX_LIST`,
`PROC_TTY`, `TTY_STAT`) in `tabmeta/mod.rs` that make the cost budget assertable. Done when:
a tab with an attached tmux client resolves to that client's live `pane_pid`; a tab without
one returns its input pid having spawned nothing (`TMUX_LIST == 0`); a tmux tab costs at most
one spawn per tick whatever the outcome (`TMUX_LIST <= 1`, D7e); every failure mode — including
a resolved-but-dead pane (D13) — returns the input pid without panicking; and every branch of
the resolution logic is unit-tested with no tmux process and no live tty.

## Research

### The live tree — the design validated end-to-end against the running machine
- A tab's shell and its tmux client **share a controlling tty**; the client's pane runs on a
  *different* one. Measured: tab shell `73568` (`/bin/zsh`) on `ttys002`; its direct child
  `75071` is `tmux`; `tmux list-clients -F '#{client_tty} #{pane_pid}'` → `/dev/ttys002 74877`;
  pane `74877` is `-zsh` on **`ttys003`**. The join must therefore be on the **client** tty,
  never the pane's.
- **The pane shell is not a descendant of the tab shell at all.** Pane `74877`'s ppid is
  `90662` — the tmux *server*. This is why no increase to `agent.rs`'s `MAX_DEPTH` (currently
  3, `agent.rs:23`) could ever reach the agent through tmux, and why re-rooting is the only
  mechanism that works (SD2's rejected alternative, confirmed empirically).
- **The pane's agent is a direct child of `pane_pid`.** `74877` → `74922 claude` is depth 1,
  so the existing depth-1 walk works unchanged against the new root (SD2's load-bearing
  claim). Confirmed on a second, independent client too: `/dev/ttys004 38200`, pane `38200`
  (`-zsh`, `ttys012`) → direct child `38245 claude`.
- **The tmux client is a direct child of the tab's shell** (`75071`.ppid = `73568`;
  `79066`.ppid = `79049`). This is what makes a *depth-1* presence gate sufficient and cheap.
- `tmux list-clients -F '#{client_tty} #{pane_pid}'` emits exactly two space-separated fields
  per line, one line per attached client — measured output: `/dev/ttys002 74877` and
  `/dev/ttys004 38200`. Multiple clients are the normal case, which is precisely why the
  tty join (rather than "take the first pane") is required: a tmux server is shared across
  terminal apps, so other rows belong to Warp/iTerm sessions.

### The libc surface — two representation facts that constrain the signature
- `proc_bsdinfo.e_tdev` is **`u32`** (`libc-0.2.186/src/unix/bsd/apple/mod.rs:624`), but
  `libc::dev_t` on Apple is **`i32`** (`:15`) and `stat.st_rdev` is `dev_t` (`:319`). The two
  sides of the join are therefore *different Rust types* over the same 32-bit device number —
  the manifest's sketched `-> Option<dev_t>` needs refining (D1).
- Measured rdevs are positive and small-ish (`/dev/ttys000` → `268435456` = `0x10000000`,
  `ttys002` → `268435458`, `ttys004` → `268435460`), so major/minor packing is
  `0x1000_0000 | minor`. Sign is not a practical hazard, but the sentinel below is.
- **`NODEV` is not exposed** by the crate's Apple bindings (only `ENODEV`/`MNT_NODEV`), so
  the "no controlling tty" sentinel (`-1`, i.e. `0xFFFF_FFFF` as `u32`) must be named and
  rejected by this feature itself (D2).
- **`libc::devname` IS exposed on macOS** — at `libc-0.2.186/src/unix/bsd/mod.rs:873`, the
  *parent* of the `apple` module, in an ungated `extern "C"` block (only NetBSD carries a
  `link_name` attribute). Verified by direct grep of the pinned source. **This contradicts the
  originating brief and the manifest's SD1 note**, both of which state it is unavailable; the
  error came from searching only `apple/mod.rs`, where it is indeed absent. A name-direction
  join is therefore *technically possible* (resolve `e_tdev` → `"ttys002"` once per tab and
  string-compare against `client_tty`'s basename, which would eliminate every `stat`). It is
  still rejected, but now on true grounds — see D1.
- `PROC_PIDTBSDINFO` is available (`:3684`), and `tabsession.rs:173-199` already calls
  `proc_pidinfo(PROC_PIDTBSDINFO)` into a `proc_bsdinfo` for `pbi_start_tvsec` — an existing,
  working precedent for the exact syscall this feature adds a second reader of.

### The conventions this feature must match
- `tabmeta/proc.rs` is the home of the raw macOS process primitives; every wrapper returns
  "absent" on any failure and never panics, and each bumps a test-only probe counter as its
  first statement (`proc.rs:26-28`, `:60-62`, `:72-74`, `:95-97`).
- Purity split is the established testing idiom: `proc::parse_childpids` / `parse_procargs`
  are pure and table-tested with no syscall (`proc.rs:144-193`), and `agent::walk` is generic
  over its children-provider *and* its classifier so every bound is tested against hand-built
  fake trees (`agent.rs:149-178`). SD5 is this codebase's existing practice, not a new rule.
- `agent::walk`'s signature is monomorphic in `AgentKind`
  (`classify: impl Fn(u32) -> Option<AgentKind>` → `Option<AgentKind>`, `agent.rs:152-156`), so
  reusing it for tmux detection *would* require generalizing it. D5(b) concludes the gate does
  **not** reuse it, so `agent.rs` is expected to need zero edits; it is in this feature's
  declared surface to **pre-claim** the file, so that if the gate ever does need the generic
  walk, the edit already falls inside this feature's boundary instead of colliding with a
  sibling feature mid-build.
- `agent::walk` also never classifies its root — "The root (the tab's zsh) is not itself
  classified — only its descendants, from depth 1" (`agent.rs:150-151`, and the loop at `:158`).
  This is what makes D5(a)'s depth-0 check genuinely new work rather than a duplicate (D9).
- `agent::classify_exe` is exact + case-insensitive against a closed table (`agent.rs:60-65`);
  tmux basename matching should read the same way.
- Probe counters are declared centrally in `tabmeta/mod.rs`'s `#[cfg(test)] mod probe`
  (`mod.rs:325-388`) — never per-module — with `reset_all()` listing every counter
  (`:361-377`) and a `serial()` mutex that any counter-touching test must hold (`:384-387`),
  because `cargo test` runs in parallel and the counters are process-global statics.
- The per-tick memo shape to mirror is `git::Memo` (`git.rs:35-37`), whose entries carry the
  tick they were last touched and which is reclaimed at the end of every derivation.
- `tabmeta/mod.rs:17-21` declares the sibling modules (`activity`, `agent`, `git`, `proc`,
  `worker`); `tmux` must be added there.
- The app crate already depends on `libc = "0.2.186"` (`crates/app/Cargo.toml:17`) — **no new
  dependency is required**, which matters under the `no-gui-crates` rule and keeps this
  change inside the `macos-only` posture.
- `tabsession.rs:75` establishes the in-repo precedent for shelling out to a fixed absolute
  binary path (`/bin/ps`) and treating a non-zero exit as absent (`:79-82`).

## Decisions

The six `SD<n>` entries below are inherited **verbatim** from the set's partition manifest
(`specs/tmux-pane-resolution/partition-manifest.md`) and apply to every feature in the set.
This feature's local decisions (`D<n>`) follow them.

### SD1: The join key is the tty **device number** — `proc_pidinfo(PROC_PIDTBSDINFO).e_tdev` on the tab's side, `stat(client_tty).st_rdev` on tmux's side
- **Status**: resolved
- **Rationale**: tmux publishes `#{client_tty}` as a path (`/dev/ttys002`) and the tab's shell shares that controlling tty, so the two sides join on the device. Comparing `dev_t` numerically is pure libc, needs no subprocess for the tab's side, and makes no assumption about the `/dev/ttysNNN` name format. **Rejected: `/bin/ps -o tty= -p <pid>`** — simpler and it matches the existing `/bin/ps` pattern at `tabsession.rs:75`, but it costs one subprocess *per tab per tick*, which directly contradicts SD3's cost budget (the active tab refreshes every 500 ms, `worker.rs:27`). Note: `libc::devname` is **not** exposed in the crate's Apple bindings, so `e_tdev` is never resolved back to a name — the comparison runs in the `dev_t` direction only (`stat` the client tty path, compare `st_rdev`).

### SD2: Re-root, do not extend — the only new logic is "which pid do I start from"
- **Status**: resolved
- **Rationale**: a tmux `pane_pid` is the pane's own `-zsh`, and `claude` is its *direct child*. So the existing depth-1 walk (`agent::walk`), `proc::cwd_of_pid`, the git-branch lookup, the activity lookup and the task label all work **unchanged** against the new root. Every consumer changes by exactly one substitution — `tp.pid` → `effective_root(tp.pid)` — and no downstream signature changes beyond the root pid it is handed. **Rejected: teaching the walk to descend *through* tmux** (raising `MAX_DEPTH` / special-casing the server) — the tmux server is not a descendant of the tab's shell at all, so no depth increase can reach the pane; it would only inflate the per-tick node budget.

### SD3: Zero cost without tmux; at most one `tmux list-clients` spawn per tick
- **Status**: resolved
- **Rationale**: an unconditional `tmux list-clients` per tab per tick is not acceptable at the 500 ms active-tab cadence. Two gates: (1) a **presence gate** — run `tmux` only when a descendant of the tab's shell has exe basename `tmux`, so a machine without tmux, or a tab not running it, pays exactly zero; (2) a **per-tick memo** of the client→pane map keyed on tick, in the shape `git::Memo` already establishes, so N tmux tabs cost one spawn per tick, not N. **Cache the map, never the resolved root**: `pane_pid` changes when the user switches tmux windows, and the card following the active pane is the correct behavior.

### SD4: Absent on any failure — never panic, always fall back to today's behavior
- **Status**: resolved
- **Rationale**: this is the contract every wrapper in `tabmeta/proc.rs` already honors (D14). A missing `tmux` binary, a non-zero exit, unparseable output, a dead pid, a `stat` refusal, or no matching tty all resolve to "no re-root" — the derivation proceeds from the tab's own shell pid exactly as it does today. A tmux hop that fails must never be worse than not having looked.

### SD5: Every new piece splits into a pure function plus a thin syscall/spawn wrapper
- **Status**: resolved
- **Rationale**: the whole set must be testable on a machine with no tmux running (and in CI). Mirrors the split this codebase already uses — `proc::parse_childpids` / `parse_procargs` and `agent::walk`'s generic children/classifier. Concretely: a pure parser over the `tmux list-clients` output *string*, and pure root-selection given a `(client_tty, pane_pid)` map plus a `dev_t` — with the subprocess and the syscalls confined to thin wrappers around them.

### SD6: Cost is enforced by probe counters, not by inspection
- **Status**: resolved
- **Rationale**: `tabmeta/mod.rs`'s test-only `probe` module (line 325) is how every existing data source proves its own budget. Two new counters — `TMUX_LIST` and `PROC_TTY` — are declared there and added to `reset_all()`, so SD3's "zero without tmux, one spawn per tick" is an assertable counter delta rather than a claim. Counters are declared centrally so the data-source modules never depend on each other to observe cost (the existing D13(6) rule).

---

### D1: `tty_dev_of_pid(pid) -> Option<u32>` — normalize the join on `u32`, and join by number rather than by name
- **Status**: resolved
- **Rationale**: two parts, both corrected by L1 research.
  **(a) The type.** The manifest sketched `-> Option<dev_t>`, but the two sides of the join are different Rust types over the same 32-bit device number: `proc_bsdinfo.e_tdev` is `u32` (`libc:624`) while `libc::dev_t` on Apple is `i32` (`libc:15`) and `stat.st_rdev` is `dev_t` (`libc:319`). Returning `u32` puts the normalization at exactly one place — the `stat` side casts `st_rdev as u32` — instead of scattering `as` casts through the comparison. **Rejected: returning `dev_t` (`i32`)** to match the manifest's literal wording — it would force the cast onto `e_tdev`, the side authoritative for "which tty is this process on", and invite a sign-extension mistake.
  **(b) The direction — re-justified on true grounds.** The originating brief and SD1 both say `libc::devname` is unavailable, so a name-direction join was never considered. **That premise is false** (`bsd/mod.rs:873` — see Research), so the alternative must be rejected on its merits rather than on availability: `devname` returns a pointer into a **process-global static buffer**, so it is not thread-safe — and this module is called from the `tabmeta` worker thread (F2) *and* from the UI-triggered overlay path (F3), which is exactly a concurrent-use scenario; and on a cache miss it does not fail but **synthesizes a `"#major/minor"` string**, which would silently never match a real `client_tty` basename and produce a wrong-but-plausible negative. The numeric path→rdev direction has neither hazard, needs no `unsafe` string handling, and — after D7 pre-resolves rdevs — costs strictly fewer syscalls than one `devname` per tab would. SD1's *mechanism* (join on the device number) is unchanged and is not relitigated here; only its parenthetical justification was wrong, and that discrepancy is surfaced to the human rather than silently patched into the manifest.

### D2: Name and reject the `NODEV` sentinel locally
- **Status**: resolved
- **Rationale**: a process with no controlling tty reports `e_tdev == NODEV == -1`, i.e. `0xFFFF_FFFF` as `u32`. L1 confirmed **`NODEV` is not exposed** by the crate's Apple bindings (only `ENODEV` and `MNT_NODEV` exist), so this feature declares the constant itself — as a private `const NODEV: u32 = u32::MAX;` in `tabmeta/proc.rs`, beside the `tty_dev_of_pid` that reads it (the same placement `proc.rs` uses for `MAX_CHILDREN`, `:22`) — and treats both it and `0` as "absent". Without this the sentinel would be a *valid-looking* device number that could only ever fail to match — turning a cheap early return into a pointless `stat` of every client tty. **Rejected: comparing against `-1 as u32` inline** — an unnamed magic value at the comparison site is exactly the kind of thing that reads as a bug later.

### D3: One `list-clients` invocation, exit-status-gated, with per-row-tolerant parsing
- **Status**: resolved
- **Rationale**: `tmux list-clients -F '#{client_tty} #{pane_pid}'` emits exactly two space-separated fields per line, one line per attached client (L1 measured). A non-zero exit (no server running, no sessions) yields an empty map per SD4, mirroring the `!out.status.success() → None` handling at `tabsession.rs:79-82`. Parsing is **tolerant per row**: a malformed or short line is skipped individually rather than failing the whole map, because one unparseable client must not blind the tab to a second, valid one. `TMUX_LIST` is bumped in the spawn wrapper as its first statement, before the `Command` runs — so a spawn that fails still counts, which is what makes D7(e)'s `TMUX_LIST <= 1` assertion meaningful rather than vacuous on the failure path. **Rejected: `list-panes`** — it needs a target session and reports every pane, not the one the client is looking at; `#{pane_pid}` on a *client* row is already exactly "the active pane of the client attached to this tty".

### D4: Every failure returns the **input** pid
- **Status**: resolved
- **Rationale**: inherits SD4 and makes it concrete for this module's signature — `effective_root` returns `u32`, never `Option<u32>`, so no caller can accidentally propagate an absence into a pid. The fallback value is the pid the caller passed in, which is precisely today's behavior, so a total tmux failure is indistinguishable from tmux not being installed. **Rejected: `-> Option<u32>`** — it would push an "and if not?" branch into both consumers (F2 and F3), inflating the one-line substitution SD2 promises into a conditional at every call site.

### D5: The presence gate inspects **the root itself, then depth 1** — and returns what it found
- **Status**: resolved
- **Rationale**: four parts.
  **(a) Depth 0 is checked first.** A very common idiom makes tmux the tab's *own* process rather than a child — `[[ -z $TMUX ]] && exec tmux new -A -s main` in `.zshrc`, or any config that runs tmux as the tab command. `exec` replaces the shell's image **at the same pid**, so `tp.pid`'s executable *is* tmux and it has no tmux child at all. A children-only gate (which is what `agent::walk` does — its doc states "the root is not itself classified", `agent.rs:150-151`) would never fire for those users, making the whole feature a silent no-op for them even though the tty join would have worked perfectly (the exec'd tmux still holds the tab's controlling tty). The root check is **one `exe_path` call — cheaper than the depth-1 scan it precedes** — so this costs essentially nothing and closes the larger of the two coverage holes.
  **(b) Then depth 1, and no further.** L1 measured the tmux client as a *direct* child on both live clients (`75071`.ppid = `73568`, `79066`.ppid = `79049`), which is what an interactive `tmux` / `tmux attach` produces. Depth 1 costs one `children_of` plus one `exe_path` per direct child (typically one or two), against a full bounded walk's budget of up to `MAX_NODES = 64` classifications *per tab per tick*. **Rejected: reusing `agent::walk` at `MAX_DEPTH = 3`** — up to 64× the gate cost for every non-tmux tab, to catch a tmux behind a wrapper script (the rarer case, which degrades gracefully; Known Gap). Because no walk is reused, **`agent::walk` needs no generalization and `agent.rs` is expected to require zero edits** — it stays in this feature's declared surface as owned-but-untouched, so a future need for the generic walk is already inside this feature's boundary rather than colliding with a sibling.
  **(c) The gate returns the client it found, not a bool.** Signature: `tmux_client_of(pid) -> Option<TmuxClient>` where `TmuxClient { pid: u32, exe: PathBuf }`. A `bool` would force D6 to re-derive the binary path with a second `exe_path` call and a second `PROC_EXE` bump — falsifying D6's "no new syscall" claim. Returning the pid *and* the already-in-hand path makes D6 free as written, and gives D14's deferred socket work the pid it would need.
  **(d) The match rule is exact, case-insensitive, on the basename only.** `tmux` and `TMUX` match; `tmux-next`, `mytmux` and a bare `tm` do not; `/opt/homebrew/bin/tmux` matches on its file name. This mirrors `agent::classify_exe` (`agent.rs:60-65`) exactly, and is stated as a rule here rather than left as a Research suggestion so an L3 requirement can assert each case. Unlike `classify_exe`, no leading-dot-component or `<name>/versions` handling is needed — tmux ships no in-place auto-updater that renames the running binary, and the near-miss rejections are what keep `tmux-next` from being silently treated as the tab's tmux.

### D6: Spawn the tmux binary **discovered by the presence gate**, not `Command::new("tmux")`
- **Status**: resolved
- **Rationale**: this is the Tier-2 implication check's finding, and it is a correctness issue rather than a preference. `tmux` has no fixed install path (`/opt/homebrew/bin` on Apple Silicon, `/usr/local/bin` on Intel, `/opt/local/bin` under MacPorts), and a macOS `.app` bundle launched from Finder or the Dock inherits a minimal `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`) that contains none of them — so a `PATH`-resolved spawn would work when run via `cargo run` and then silently never resolve in the shipped `.app` (the local-`.app` packaging named in CLAUDE.md's D11). The gate has already identified the running tmux **client process** and returns its binary path in hand as `TmuxClient.exe` (D5c), so the spawn uses that exact absolute path — no additional syscall (the `exe_path` was already spent classifying it) and no `PATH` dependence at all. It is also strictly more correct than any path list: it is by construction the same tmux build the pane is actually running under. **Rejected: `Command::new("tmux")`** (breaks in the shipped `.app`); **rejected: a hardcoded candidate-path list** (unbounded, and still guesses).

### D7: The memo is one wholesale-replaced entry holding **pre-resolved** `(rdev, pane_pid)` rows, valid by tick **and** wall clock
- **Status**: resolved
- **Rationale**: five parts, pinning the type precisely enough to implement and to test.
  **(a) The type.** `tmux::Memo { entry: Option<Entry> }` with `Memo::new()` (mirroring `git::Memo::new()`, `git.rs:56`), where `Entry { tick: u64, fetched_at: Instant, clients: Vec<(u32, u32)> }`. `Vec` — not a map — because D11's "first matching row wins" requires tmux's own output order to be preserved. Note the two shapes the word "map" covers loosely elsewhere are **distinct types**: `parse_clients` yields `Vec<(PathBuf, u32)>` (the `(client_tty, pane_pid)` rows SD3/SD5 call the map), which `resolve_rdevs` converts to the `Vec<(u32, u32)>` stored here. `Memo` is what D10 passes across a module boundary. `TICK` for (d) is `worker.rs:27`'s existing `pub(crate) const` — `use super::worker::TICK` reads it without editing a file outside this feature's declared surface.
  **(b) rdevs are resolved once, at fetch.** The stored rows are `(rdev, pane_pid)`, **not** `(PathBuf, pane_pid)`: each `client_tty` path is `stat`ed once when the map is fetched, so selection afterwards is pure integer comparison. Without this, every tab would `stat` every client tty on every tick — M tabs × N clients filesystem stats per tick, none of it counted (the defect SD6 exists to prevent). With it, the cost is N stats per tick total, counted by `TTY_STAT` (D15).
  **(c) It cannot grow.** Unlike `git::Memo`, which is keyed by cwd and therefore needs TTL expiry, oldest-touched eviction and a `MEMO_MAX` cap (`git.rs:26-31, 70-88`), this holds exactly one entry, replaced wholesale on a miss — so it needs **no reclamation logic at all**. Stated explicitly so an implementer does not copy machinery for a structure with nothing to reclaim.
  **(d) Validity is `tick` AND `fetched_at.elapsed() < TICK`.** The tick alone is not a sound freshness key: `worker.rs` advances `tick_index` **only** on the `RecvTimeoutError::Timeout` branch (`:225`), while `Cmd::SetActive` (`:205`) and `Cmd::SetEnabled` (`:221`) both derive at the *current, unchanged* index, and the loop blocks indefinitely on `rx.recv()` when the roster is empty (`:189-190`). One `tick_index` value can therefore span unbounded wall-clock time, and `SetActive` — the in-band refresh that fires the moment a user switches tabs — is exactly where a stale client→pane map would be most visible. The wall-clock guard bounds staleness at `TICK` (500 ms) while still guaranteeing at most one spawn per tick in the steady state, which is what SD3 actually asks for.
  **(e) EVERY fetch attempt writes an `Entry` — including failures.** A non-zero exit, a missing server, or unparseable output stores `clients: vec![]`, not "no entry". Caching only successes would mean a machine with N tmux tabs and no reachable server (D14's exact scenario, and the no-server case) re-spawns once **per tab** per tick — N spawns, falsifying both D14's budget claim and SD3's headline guarantee. A failure `Entry` carries `fetched_at` like any other, so it suppresses respawn for the rest of the tick. With this negative caching, `TMUX_LIST <= 1` per tick holds regardless of outcome, which is directly assertable. (A non-tmux tab is stopped by the presence gate before any fetch, so `TMUX_LIST == 0` is unaffected.)

### D8: The subprocess call is synchronous and un-timed-out; the blast radius is the metadata worker thread
- **Status**: resolved
- **Rationale**: `Command::output()` blocks, so a wedged tmux server would stall the caller. That caller is the `tabmeta` **worker thread**, which exists precisely so that "no render frame performs filesystem or process I/O" (`tabmeta/mod.rs:1-9`) — so the worst case is tab cards freezing at their last published values, never a dropped frame or a hung UI. This matches the existing accepted risk for `/bin/ps` at `tabsession.rs:75`. **Rejected: a watchdog thread + timeout** — it adds a thread and a kill path to a feature whose entire failure story is "return the input pid", for a hang that has no reported occurrence; recorded as a Known Gap so the tradeoff is revisitable rather than forgotten. (F3's consumer calls this from the overlay-toggle path, not a render frame — that consumer's spec owns that placement.)

### D9: The gate costs `2 + k` syscalls per tab per refresh — one genuinely new, the rest duplicated — accepted, bounded, and counted
- **Status**: resolved
- **Rationale**: the Tier-3 pair-tension check between D5 and SD3, with D5(a) folded in. For a **non**-tmux tab the gate touches the same tree `agent_of_pid` will walk moments later, costing `2 + k` syscalls, where `k` is the direct-child count (typically 1):
  - `exe_path(root)` — D5(a)'s depth-0 check. **Genuinely new, not duplicated**: `agent::walk` provably never classifies the root (`agent.rs:150-151, 158`), so no amount of sharing with the agent walk can eliminate this one. It is the price of covering the `exec tmux` case, and it is one syscall.
  - `children_of(root)` plus `k` × `exe_path(child)` — **duplicated**, since `agent_of_pid` performs exactly these reads on the same tree moments later.
  Against the existing steady-state budget this is small, and it is *measurable* rather than asserted because `PROC_CHILDREN` and `PROC_EXE` already count it (`proc.rs:61, 73`). This sharpens SD3's wording: **"zero cost" means zero `tmux` spawns** (`TMUX_LIST == 0`), not zero syscalls. **Rejected: fusing the gate into the agent walk to share the child list** — it would recover only the duplicated portion (never the root check), and for a tmux tab the two walks are over *different trees* (the tab's, then the pane's), so the fusion helps only the non-tmux case while entangling this feature with `worker.rs`, which belongs to a sibling feature; recorded as a Known Gap for that consumer to weigh.

### D10: Two entry points — memoized for the ticking consumer, unmemoized for the event-driven one
- **Status**: resolved
- **Rationale**: the two consumers run on different cadences. `worker.rs` (F2) ticks every 500 ms and holds per-tick state, so it takes `effective_root(&mut memo, pid, tick)`. `tabsession.rs` (F3)'s `hint_for` is invoked on an overlay toggle and has no tick to key a memo on, so it takes `effective_root_uncached(pid)` — one spawn per toggle, which is a user-initiated event and not on any cadence, so it needs no budget. **Rejected: forcing a tick parameter on F3** — it would mean inventing a fake tick or threading worker state into an unrelated module; **rejected: a global/lazy-static memo** — shared mutable process-global state across two threads for a value whose whole purpose is to be per-tick fresh.

### D11: First matching client row wins
- **Status**: resolved
- **Rationale**: a tmux server is shared across terminal applications — L1 measured two attached clients (`ttys002`, `ttys004`) belonging to different tabs, and Warp/iTerm sessions would appear as further rows. The tty join already discriminates to (normally) exactly one row; "first match wins" makes the degenerate case of two clients reporting the same tty **deterministic** (by tmux's own output order) instead of arbitrary. This mirrors `agent::walk`'s "first match short-circuits" rule (`agent.rs:167`). **Rejected: erroring or returning absent on multiple matches** — it would turn a harmless ambiguity into a lost re-root.

### D12: The impure edges are injectable seams; selection is fully pure
- **Status**: resolved
- **Rationale**: SD5 made concrete for this module, in three pieces.
  `parse_clients(&str) -> Vec<(PathBuf, u32)>` is **fully pure** — the whole `list-clients` output contract (two space-separated fields, per-row tolerance, D3) is table-tested from string literals.
  `resolve_rdevs(rows, rdev_of) -> Vec<(u32, u32)>` takes `rdev_of: impl Fn(&Path) -> Option<u32>`, the one injected seam, so the path→device step is tested against a fake device table with **no live tty**. Rows whose tty cannot be `stat`ed are dropped here (SD4).
  `select_pane(clients: &[(u32, u32)], tty_dev: u32) -> Option<u32>` is then **pure integer comparison** — no injection needed at all, because D7(b) pre-resolved the rdevs. Multi-row (D11), no-match, and sentinel cases are all table-tested.
  Together these mean every branch of the resolution logic is unit-tested **with no tmux installed and no live tty**; the only untested-by-unit-test surface is the thin production wrappers supplying the real `stat` and the real subprocess, each a handful of lines — exactly the split `agent::walk` uses against fake process trees (`agent.rs:337-397`). **Rejected: testing against the developer's live tmux** — the suite would pass or fail based on whether tmux happens to be running, the opposite of the headless-testability invariant the workspace is built around.

### D13: A resolved `pane_pid` must pass a liveness check before it is returned
- **Status**: resolved
- **Rationale**: D4 guarantees that every *failure* returns the input pid — but a successfully-parsed row whose `pane_pid` has since exited is **not** a failure by that definition. It resolves, and then everything downstream (`cwd_of_pid`, the git lookup, the agent walk, the activity lookup) returns absent against a dead pid, so the tab card goes **blank** where today it would still show the tab shell's own branch and agent. That is strictly worse than not having looked, which is precisely what SD4 forbids. So the selected `pane_pid` is accepted only if `proc::cwd_of_pid(pane_pid).is_some()`; otherwise the input pid is returned. Cost is one syscall **on the tmux path only** (a non-tmux tab never reaches this point), already counted by the existing `PROC_CWD` probe, and it does not perturb the existing `worker.rs` cost assertions because those exercise a non-tmux self-pid. **Rejected: trusting the map's freshness** — the window is small (≤ one tick) but the failure is user-visible and the check is one syscall; **rejected: `exe_path` as the liveness probe** — `cwd_of_pid` is the same call the consumer makes next, so using it here keeps the probe and the payload identical rather than testing liveness one way and using it another. Note this does **not** make the consumer's call free: `cwd_of_pid` is an unmemoized raw `proc_pidinfo` that bumps `PROC_CWD` on every call (`proc.rs:26-28`), so a re-rooted tab pays `PROC_CWD == 2` for the tick — once here, once at `worker.rs:69`. That cost is carried explicitly in `tabmeta-tmux-reroot`'s ledger rather than being quietly absorbed.

### D14: Default tmux socket only — a non-default socket falls back rather than failing forever
- **Status**: resolved
- **Rationale**: `tmux list-clients` finds its server via `-L <name>` / `-S <path>`, else `$TMUX_TMPDIR`/`$TMPDIR`/`/tmp`. A user running `tmux -L work` therefore has a server this query cannot see, and the invocation would return non-zero on **every tick, forever**, for a tab that genuinely is running tmux. This feature queries the **default socket only** and, finding no row whose tty matches, returns the input pid (D4) — the tab behaves exactly as it does today. The cost of that permanent miss is bounded to the single memoized spawn per tick SD3 already budgets (not per tab), so the budget is honored even in the pathological case. **Rejected: threading the socket through from the client's argv** (readable via the existing `proc::proc_args` on the pid D5 now returns) — it is implementable, but multiple sockets mean multiple servers with disjoint client lists, which breaks D7's single-entry memo and forces a socket-keyed cache plus reclamation; that is a materially larger design for a minority configuration. Recorded as a Known Gap with the implementation path noted, rather than silently unhandled.

### D15: A third probe counter, `TTY_STAT`, and a named home for the wrapper that bumps it — refining SD6's two
- **Status**: resolved
- **Rationale**: the production `stat` wrapper is `proc::rdev_of_path(path: &Path) -> Option<u32>`, living in `tabmeta/proc.rs` beside `tty_dev_of_pid` (D1) because it is the same kind of thing — a raw macOS syscall wrapper, absent on any failure, bumping its counter as its first statement, exactly like the four already there (`proc.rs:26-28, 60-62, 72-74, 95-97`). It is the function passed as D12's injected `rdev_of` seam in production, and it is what bumps `TTY_STAT`. SD6 sketched exactly two counters (`TMUX_LIST`, `PROC_TTY`), which between them cover the subprocess spawn and the *tab's* side of the join (`tty_dev_of_pid`). They do not cover the **tmux side**: the `stat` of each `client_tty` path. Leaving those uncounted would be exactly the "cost asserted by inspection rather than counted" failure SD6 exists to prevent — and it is not a hypothetical, since without D7(b) it would have been M×N stats per tick. `TTY_STAT` is declared alongside the other two in `tabmeta/mod.rs`'s `probe` module and added to `reset_all()`, making D7(b)'s "N stats per tick, not M×N" an assertable counter delta. This **refines** SD6 by honoring its stated principle ("cost is enforced by probe counters, not by inspection") rather than its illustrative count of two; the discrepancy with the manifest's literal wording is surfaced to the human, not silently absorbed.

## Constraints
- **Dependency direction** (`.claude/rules/dependency-direction.md`): all of this lives in the
  `app` crate. `term-core` must learn nothing about tmux, ttys, or subprocesses.
- **No new dependency.** `libc = "0.2.186"` is already in `crates/app/Cargo.toml:17` and
  supplies every syscall needed; nothing new enters the tree.
- **macOS ARM64 only** (`.claude/rules/macos-only.md`): no `#[cfg(target_os = ...)]` branches,
  no portability shims for a non-Darwin `dev_t`.
- **No `unwrap()` outside tests** (CLAUDE.md) — every syscall and parse degrades to absent.
- **`cargo clippy --workspace --all-targets -- -D warnings` must pass**, and the `rustfmt`
  PostToolUse hook rewrites every edited `*.rs`.
- **The existing cost assertions must keep passing** — in particular
  `worker.rs::steady_state_repo_derivation_does_no_git_read` and `empty_roster_does_no_work`,
  which assert exact counter deltas. Adding counters to `probe::reset_all()` must not
  perturb them.
- **Counter tests must hold `probe::serial()`** (`mod.rs:384-387`); the counters are
  process-global statics and `cargo test` runs in parallel.
- **Read-only observation only** — process listing and `proc_pidinfo`, as `tabsession.rs:16-17`
  states for its own module. No signals, no writes, no tmux *control* commands.

## Known Gaps
- **tmux at depth ≥ 2 is not detected.** D5 covers depth 0 (`exec tmux`) and depth 1 (the
  measured interactive case), but misses e.g. a shell function that forks before `exec`ing
  tmux, or tmux started under a launcher. The tab then behaves exactly as it does today (no
  re-root) — an absent improvement, never a wrong answer. Widen to depth 2 if observed.
- **Non-default tmux sockets are unsupported** (D14). `tmux -L work` / `-S <path>` sessions are
  invisible to the default-socket query, so those tabs never re-root. The implementation path
  is known — the client's argv carries the flag and `proc::proc_args` already reads argv — but
  it requires replacing D7's single-entry memo with a socket-keyed one.
- **`SD1`'s and the brief's `devname` note is factually wrong** (Research, D1). The mechanism
  is unaffected — the numeric join is still the right choice — but the manifest's stated
  reason is not true. Flagged for the human to correct in
  `specs/tmux-pane-resolution/partition-manifest.md`; deliberately **not** silently rewritten.
- **`SD6` names two counters; this feature needs three** (D15). Same disposition as above.
- **No timeout on `list-clients` at *this* layer** (D8). For the `tabmeta` worker consumer a
  wedged tmux server freezes tab-card *updates*, not the UI. That framing covers only the worker
  path: `tabsession-tmux-reroot` establishes that the overlay reaches this resolver on the **winit
  UI thread**, and its own D8 wraps the call in a bounded wait (`RESOLVE_BUDGET`, falling back to
  the tab's shell pid) so the exposure there is a leaked thread rather than a frozen terminal. The
  deferral this decision makes ("that consumer's spec owns that placement") has been discharged.
- **Gate/walk fusion is deferred** (D9). For non-tmux tabs the depth-1 child reads are
  performed twice per refresh. Sharing them requires a decision inside `worker.rs`, which
  belongs to the `tabmeta-tmux-reroot` feature — flagged there rather than resolved here.
- **Nested tmux is untested.** The outer client's tty is the one that matches, so the outer
  pane's pid is returned; this is believed correct but is not a case this feature verifies.

## Requirements

### R0: A tab's derivation root follows its tmux client's active pane

#### R0.1: An attached tmux client re-roots the tab
- **Given**: a tab shell pid whose tree contains an attached tmux client, and that client's row in `tmux list-clients` reports `client_tty` matching the tab shell's controlling tty
- **When**: `effective_root` is called with that pid
- **Then**: it returns the client's `pane_pid`, not the input pid

#### R0.2: A tab with no tmux is untouched and spawns nothing
- **Given**: a tab shell pid with no tmux at depth 0 or 1, and reset probe counters
- **When**: `effective_root` is called with that pid
- **Then**: it returns the input pid unchanged and `TMUX_LIST == 0`

#### R0.3: The re-rooted pid is usable by every existing downstream derivation
- **Given**: a returned `pane_pid` for an attached client
- **When**: `proc::cwd_of_pid` and `agent::agent_of_pid` are called with it
- **Then**: both resolve against the pane's own process tree with no change to their signatures or bounds (SD2 — the pane's agent is a direct child, so the existing depth-1 walk suffices)

### R1: The tab's side of the join — `proc::tty_dev_of_pid` (D1, D2)

#### R1.1: A process with a controlling tty reports its device number
- **Given**: the current test process, running under a tty
- **When**: `tty_dev_of_pid(std::process::id())` is called
- **Then**: it returns `Some(dev)` where `dev` equals `rdev_of_path` of that process's tty, as a `u32`

#### R1.2: A dead pid yields absent, not a panic
- **Given**: pid `0` (which cannot exist)
- **When**: `tty_dev_of_pid(0)` is called
- **Then**: it returns `None` without panicking

#### R1.3: The `NODEV` sentinel is rejected
- **Given**: a process whose `e_tdev` is `NODEV` (`u32::MAX`) because it has no controlling tty
- **When**: `tty_dev_of_pid` reads it
- **Then**: it returns `None` rather than `Some(u32::MAX)`, so no `stat` of any client tty is attempted

#### R1.4: A zero device is rejected
- **Given**: an `e_tdev` value of `0`
- **When**: `tty_dev_of_pid` reads it
- **Then**: it returns `None`

#### R1.5: The counter is bumped once per call
- **Given**: reset probe counters
- **When**: `tty_dev_of_pid` is called once
- **Then**: `PROC_TTY == 1`, whether the call succeeded or returned `None`

### R2: The presence gate — `tmux_client_of` (D5, D9)

#### R2.1: tmux at depth 0 (`exec tmux`) is found
- **Given**: a root pid whose own executable basename is `tmux` (the `exec tmux new -A` idiom) and which has no tmux child
- **When**: the gate runs against a fake tree providing that root's exe path
- **Then**: it returns `Some(TmuxClient { pid: <root>, exe: <that path> })`

#### R2.2: tmux at depth 1 is found
- **Given**: a root whose executable is `zsh` and which has a direct child whose executable basename is `tmux`
- **When**: the gate runs
- **Then**: it returns `Some(TmuxClient)` naming the **child's** pid and exe path

#### R2.3: The root is checked before its children
- **Given**: a root that is itself `tmux` *and* has a `tmux` child
- **When**: the gate runs
- **Then**: it returns the **root's** pid (depth 0 wins), making the result deterministic

#### R2.4: tmux at depth 2 is not found
- **Given**: a root → child → grandchild tree where only the grandchild is `tmux`
- **When**: the gate runs
- **Then**: it returns `None` (the accepted Known Gap), and no fetch is attempted

#### R2.5: Basename matching is exact and case-insensitive
- **Given**: candidate exe paths `/opt/homebrew/bin/tmux`, `TMUX`, `tmux-next`, `mytmux`, and `tm`
- **When**: each is classified by the gate's match rule
- **Then**: the first two match and the last three do not

#### R2.6: The gate costs `2 + k` syscalls for a non-tmux tab
- **Given**: reset probe counters and a non-tmux root with `k` direct children
- **When**: the gate runs once
- **Then**: `PROC_EXE == 1 + k` and `PROC_CHILDREN == 1` (the root check plus one child listing plus one exe read per child)

### R3: Fetching the client→pane rows (D3, D6, D14)

#### R3.1: The spawn uses the client's own binary
- **Given**: a `TmuxClient` whose `exe` is `/opt/homebrew/bin/tmux`
- **When**: the fetch runs
- **Then**: it executes that absolute path with args `list-clients -F '#{client_tty} #{pane_pid}'`, and never resolves `tmux` through `PATH`

#### R3.2: Well-formed output parses to one row per client
- **Given**: the string `"/dev/ttys002 74877\n/dev/ttys004 38200\n"`
- **When**: `parse_clients` is called on it
- **Then**: it returns exactly `[("/dev/ttys002", 74877), ("/dev/ttys004", 38200)]` in that order

#### R3.3: Malformed rows are skipped individually
- **Given**: output containing a valid row, a row with only one field, a row whose pid is not a number, and an empty line
- **When**: `parse_clients` is called
- **Then**: it returns only the valid row — the bad rows are dropped without discarding the good one

#### R3.4: A non-zero exit yields an empty row set
- **Given**: no tmux server reachable on the default socket, so `list-clients` exits non-zero
- **When**: the fetch runs
- **Then**: it yields zero rows rather than an error, and the caller falls back to the input pid

#### R3.5: A non-default socket degrades to no re-root
- **Given**: a tab running `tmux -L work`, whose server the default-socket query cannot see
- **When**: `effective_root` runs for that tab
- **Then**: it returns the input pid, and the cost stays at one spawn for the tick (not one per tab)

### R4: Device resolution and pane selection (D11, D12, D15)

#### R4.1: Each client tty is resolved to a device number once
- **Given**: parsed rows `[("/dev/ttys002", 74877)]` and an injected `rdev_of` mapping that path to `0x1000_0002`
- **When**: `resolve_rdevs` is called
- **Then**: it returns `[(0x1000_0002, 74877)]`

#### R4.2: A tty that cannot be stat-ed is dropped
- **Given**: two rows where the injected `rdev_of` returns `None` for the first
- **When**: `resolve_rdevs` is called
- **Then**: only the second row survives, and no panic occurs

#### R4.3: Selection matches on the device number
- **Given**: resolved rows `[(0x1000_0002, 74877), (0x1000_0004, 38200)]`
- **When**: `select_pane` is called with `tty_dev == 0x1000_0004`
- **Then**: it returns `Some(38200)`

#### R4.4: A device matching no row yields absent
- **Given**: the same rows
- **When**: `select_pane` is called with `tty_dev == 0x1000_0009`
- **Then**: it returns `None`

#### R4.5: Duplicate device rows resolve to the first
- **Given**: rows `[(0x1000_0002, 111), (0x1000_0002, 222)]` in that order
- **When**: `select_pane` is called with `0x1000_0002`
- **Then**: it returns `Some(111)` — deterministic by tmux's output order

#### R4.6: An empty row set yields absent
- **Given**: an empty resolved row set
- **When**: `select_pane` is called with any device
- **Then**: it returns `None`

#### R4.7: `rdev_of_path` reads a real device and counts itself
- **Given**: reset probe counters and the path `/dev/null`
- **When**: `proc::rdev_of_path` is called on it
- **Then**: it returns `Some(_)`, `TTY_STAT == 1`, and a nonexistent path returns `None` while still bumping the counter

### R5: The per-tick memo (D7, D10)

#### R5.1: A second call in the same tick does not respawn
- **Given**: a memo warmed by a fetch at tick 5, with counters reset afterwards
- **When**: `effective_root` is called again at tick 5, within `TICK` of the first
- **Then**: `TMUX_LIST == 0` — the cached rows are reused

#### R5.2: A new tick refetches
- **Given**: a memo warmed at tick 5
- **When**: `effective_root` is called at tick 6
- **Then**: exactly one new fetch occurs (`TMUX_LIST == 1`) and the entry is replaced wholesale

#### R5.3: An entry older than `TICK` is stale even at the same tick index
- **Given**: a memo entry stamped tick 5 whose `fetched_at` is more than `TICK` in the past (the `SetActive`/`SetEnabled` path, which derives without advancing `tick_index`)
- **When**: `effective_root` is called at tick 5
- **Then**: it refetches rather than serving the stale rows

#### R5.4: A failed fetch is cached too
- **Given**: reset counters and a tmux tab whose `list-clients` exits non-zero
- **When**: `effective_root` is called twice within one tick
- **Then**: `TMUX_LIST == 1` — the failure stored `clients: vec![]` and suppressed the second spawn

#### R5.5: The memo holds at most one entry
- **Given**: a memo driven across 100 ticks with varying output
- **When**: its entry count is inspected
- **Then**: it is never more than 1, and no reclamation or eviction code is required

#### R5.6: The map is cached, never the resolved root
- **Given**: a memo warmed at tick 5, after which the user switches tmux windows so `pane_pid` changes
- **When**: `effective_root` is called at tick 6
- **Then**: it returns the **new** pane's pid — the cache never pins a previously resolved root

### R6: The public entry points and the absent-on-failure contract (D4, D10, D13)

#### R6.1: `effective_root` returns a pid, never an `Option`
- **Given**: any input pid and memo
- **When**: `effective_root(&mut memo, pid, tick)` is called
- **Then**: its return type is `u32`, so no consumer must branch on absence

#### R6.2: The unmemoized entry point serves the event-driven consumer
- **Given**: a caller with no tick and no memo (the overlay-toggle path)
- **When**: `effective_root_uncached(pid)` is called
- **Then**: it performs at most one fetch and returns the same value the memoized form would for a cold memo

#### R6.3: A resolved but dead pane falls back to the input pid
- **Given**: a client row whose `pane_pid` names a process that has since exited
- **When**: `effective_root` resolves it
- **Then**: the liveness check (`cwd_of_pid(pane_pid).is_some()`) fails and the **input** pid is returned, so the tab card keeps showing the tab shell's own branch and agent rather than going blank

#### R6.4: Every failure mode returns the input pid without panicking
- **Given**: each of — a dead root pid, an absent tty device, a missing tmux binary, a non-zero exit, unparseable output, an unstat-able client tty, and a no-match device
- **When**: `effective_root` is called for each
- **Then**: it returns the input pid in every case and never panics

#### R6.5: `Memo::new()` is the constructor the consumer calls
- **Given**: a consumer that must own a memo across ticks
- **When**: it calls `tmux::Memo::new()`
- **Then**: it obtains an empty memo, mirroring `git::Memo::new()`, and passes it as `&mut` to `effective_root`

### R7: Cost is counted, not asserted (SD6, D9, D15)

#### R7.1: Three counters exist and reset with the rest
- **Given**: the `probe` module in `tabmeta/mod.rs`
- **When**: `reset_all()` is called
- **Then**: `TMUX_LIST`, `PROC_TTY` and `TTY_STAT` are all zeroed alongside the pre-existing counters

#### R7.2: `TMUX_LIST` counts attempts, not successes
- **Given**: reset counters and a spawn that fails to execute
- **When**: the fetch is attempted
- **Then**: `TMUX_LIST == 1`, because the counter is bumped before the `Command` runs

#### R7.3: Stats are N per tick, not M×N
- **Given**: reset counters, `N` attached clients and `M` tmux tabs derived within one tick
- **When**: the tick completes
- **Then**: `TTY_STAT == N` — the rdevs were resolved once at fetch, not per tab

#### R7.4: The existing cost assertions still hold
- **Given**: the pre-existing `worker.rs` tests `empty_roster_does_no_work` and `steady_state_repo_derivation_does_no_git_read`
- **When**: the suite runs with the new counters registered
- **Then**: both still pass with their original expected deltas

### R8: The module is headlessly testable (SD5, D12)

#### R8.1: Every branch is unit-tested with no tmux installed
- **Given**: a machine with no tmux binary and no tmux server running
- **When**: `cargo test --workspace` runs
- **Then**: every requirement in R2–R5 above passes, because each impure edge is either a pure function or an injected seam

#### R8.2: The module is registered and the workspace stays clean
- **Given**: the new `tabmeta/tmux.rs`
- **When**: the workspace is built and linted
- **Then**: `cargo clippy --workspace --all-targets -- -D warnings` passes and `crates/app/Cargo.toml` gains no new dependency

#### R8.3: The module is reachable by both consumers
- **Given**: the sibling modules are declared privately in `tabmeta/mod.rs:17-21` (`mod activity; mod agent; …`), and `tabsession.rs` is a **crate-root** module (`main.rs:24`), not a child of `tabmeta`
- **When**: `tmux` is declared
- **Then**: it is declared **`pub(crate) mod tmux;`** and `effective_root`, `effective_root_uncached`, `Memo` and `Memo::new` are all `pub(crate)` — otherwise `crate::tabmeta::tmux::effective_root_uncached` is unreachable from `tabsession.rs` and `tabsession-tmux-reroot` cannot compile without editing a file outside its own surface

### Decision → requirement coverage

| Decision | Covered by |
|---|---|
| SD1 (device-number join) | R1.1, R4.1, R4.3 |
| SD2 (re-root, don't extend) | R0.3 |
| SD3 (zero without tmux; ≤1 spawn/tick) | R0.2, R5.1, R5.4, R7.2 |
| SD4 (absent on failure) | R6.4, R6.3 |
| SD5 (pure + thin wrapper) | R8.1 |
| SD6 (counted, not asserted) | R7.1–R7.4 |
| D1 (`-> Option<u32>`, numeric direction) | R1.1, R1.5 |
| D2 (`NODEV` named + rejected) | R1.3, R1.4 |
| D3 (one exit-gated invocation, tolerant rows) | R3.1–R3.4, R7.2 |
| D4 (failure returns input pid) | R6.1, R6.4 |
| D5 (root-then-depth-1 gate, returns client) | R2.1–R2.6 |
| D6 (spawn the discovered binary) | R3.1 |
| D7 (memo shape, wall clock, negative cache) | R5.1–R5.6 |
| D8 (synchronous, un-timed-out) | R3.4 (failure path terminates and is cached) |
| D9 (`2 + k`, counted) | R2.6 |
| D10 (two entry points) | R6.1, R6.2, R6.5 |
| D11 (first match wins) | R4.5 |
| D12 (pure/injectable seams) | R3.2, R3.3, R4.1–R4.6, R8.1 |
| D13 (liveness gate) | R6.3 |
| D14 (default socket only) | R3.5 |
| D15 (`TTY_STAT` + `rdev_of_path`) | R4.7, R7.1, R7.3 |

## Tasks

Vertical slicing note: this feature is a single internal module with no UI, so the
"vertical slice" unit here is **one complete capability plus the tests that prove it** —
each task lands a function set and its `#[cfg(test)] mod tests` cases together, never
implementation in one task and its tests in another. T1 is the one horizontal
infrastructure task (shared counters + module registration), which is why everything
depends on it.

### T1: Register the module and declare the three probe counters [infra]
- **Fulfills**: R7.1, R8.2, R8.3
- **Depends on**: (none)
- Add **`pub(crate) mod tmux;`** to `tabmeta/mod.rs:17-21` (note the visibility: unlike its
  private siblings, this module is consumed from the crate-root `tabsession.rs`, per R8.3);
  create `crates/app/src/tabmeta/tmux.rs` with its
  module doc comment; declare `TMUX_LIST`, `PROC_TTY`, `TTY_STAT` in the `probe` module
  (`mod.rs:325-388`) and add all three to `reset_all()`.

### T2: The two `proc.rs` syscall wrappers [vertical]
- **Fulfills**: R1, R4.7
- **Depends on**: T1
- `tty_dev_of_pid(pid) -> Option<u32>` via `proc_pidinfo(PROC_PIDTBSDINFO)` reading
  `e_tdev`, rejecting the private `const NODEV: u32 = u32::MAX` and `0`; and
  `rdev_of_path(&Path) -> Option<u32>` via `stat`, casting `st_rdev as u32`. Each bumps its
  counter as its first statement. Tests: self-process round-trip, dead pid, both sentinels,
  counter deltas.
- **Parallel with T3** — touches only `proc.rs`, which no task in the `tmux.rs` chain edits.

### T3: The presence gate [vertical]
- **Fulfills**: R2
- **Depends on**: T1
- `tmux_client_of(pid) -> Option<TmuxClient>` with `TmuxClient { pid: u32, exe: PathBuf }`:
  classify the root's own `exe_path` first (depth 0), then each direct child. Exact,
  case-insensitive, basename-only matching. Written generic over its exe-path and children
  providers so R2.1–R2.5 are tested against fake trees with no syscall; the production entry
  point binds them to `proc::exe_path` / `proc::children_of`.

### T4: Fetch and parse the client rows [vertical]
- **Fulfills**: R3, R7.2
- **Depends on**: T1, T3
- `parse_clients(&str) -> Vec<(PathBuf, u32)>` (pure, per-row tolerant) plus the thin spawn
  wrapper that runs `TmuxClient.exe list-clients -F '#{client_tty} #{pane_pid}'`, bumps
  `TMUX_LIST` before executing, and maps a non-zero exit to zero rows. Default socket only
  (D14).

### T5: Resolve devices and select the pane [vertical]
- **Fulfills**: R4 (except R4.7, landed in T2)
- **Depends on**: T2, T4
- `resolve_rdevs(rows, rdev_of) -> Vec<(u32, u32)>` with the injected seam (production binds
  `proc::rdev_of_path`), dropping unstat-able rows; and `select_pane(&[(u32, u32)], u32) ->
  Option<u32>`, pure, first-match-wins.

### T6: The memo and the two public entry points [vertical]
- **Fulfills**: R0, R5, R6
- **Depends on**: T2, T4, T5
- `Memo { entry: Option<Entry> }` / `Memo::new()` / `Entry { tick, fetched_at, clients }`,
  valid on tick match AND `fetched_at.elapsed() < TICK` (`use super::worker::TICK`), writing
  an entry on **every** fetch attempt including failures. Then `effective_root(&mut Memo,
  u32, u64) -> u32` and `effective_root_uncached(u32) -> u32`, wiring gate → memo → fetch →
  resolve → select → **liveness check** (`cwd_of_pid(pane).is_some()`) → pane pid, with the
  input pid returned on every failure path.

### T7: Cost assertions and the non-regression check [vertical]
- **Fulfills**: R7.3, R7.4, R8.1
- **Depends on**: T6
- Counter-delta tests for the full budget — `TMUX_LIST == 0` for a non-tmux tab,
  `TMUX_LIST <= 1` per tick for a tmux tab including the failure path, `TTY_STAT == N` not
  M×N, and the `2 + k` gate cost — each holding `probe::serial()`. Confirm
  `worker.rs::empty_roster_does_no_work` and `steady_state_repo_derivation_does_no_git_read`
  still pass unchanged, and that the whole suite passes with no tmux binary present.

**DAG**: `T1 → {T2 ∥ T3}`, `T3 → T4`, `{T2, T4} → T5`, `{T2, T4, T5} → T6`, `T6 → T7`.
Acyclic. The only concurrency is T2 ∥ T3; T3–T6 all edit `tmux.rs` and are therefore
serialized to avoid conflicting writes.

## External Dependencies

### Pre-work
- (none) — `libc` is already a dependency, and every test is designed to pass with no tmux
  binary and no tmux server present.

### Post-work
- Manual confirmation via `/run-term` with a tab running tmux: the card should show the
  active pane's branch/agent/task and follow a tmux window switch. This exercises the real
  syscalls and the real `list-clients`, which the unit tests deliberately do not.
- **Human decision required on two manifest corrections** this feature surfaced (both in
  Known Gaps, neither silently applied): SD1's `devname` note is factually wrong, and SD6
  names two counters where three are needed.
