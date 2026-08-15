# Spec: card-live-cwd

## Meta
- **Created**: 2026-08-13
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-08-13
- **L2 review**: the L2-reviewer subagent ran **three** passes — NEEDS_FIX → NEEDS_FIX → PASS.
  Round 1 found three blockers, two of which were errors of fact in this spec's own
  reasoning, not wording. (1) D1 rejected the resync alternative by blaming
  `resolve_log_in`'s mtime flap for a spin loop; the flap cannot drive it, because ranking is
  memoised behind `RESOLVE_TTL_TICKS = 20` and short-circuits inside the TTL
  (`activity.rs:50`, `:400-404`), so two derives in one tick agree and the differ brakes the
  cycle. The rejection now rests on `Cmd::SetActive` deriving in band past `should_refresh`
  with three unmemoised process reads. (2) D5's mechanism **never fired**: both
  `refresh_waiting_band` call sites gate on the waiting *set*, and a cwd-only publish moves
  no item — so Confirmed Goal 3 was contradicted by Known Gap 4 rather than qualified by it.
  (3) The fallback *site* was unstated, and the renderer-side reading would have blanked line
  2's repo name for a user-titled tab in the pre-publish window. Round 2 found one blocker:
  D5's replacement trigger (unconditional call + change-only guard) was a strict superset of
  the `overlay_changed` gate it rejected as wasteful, and its early return sat behind an
  O(tabs) `String` clone on the app's hottest event; it became a generation gate. Round 3
  passed with three non-blocking items, all folded in — the most important being that the
  gate must **join** the existing `waiting_changed` guard rather than replace it, or
  screen-derived `Waiting` edges silently stop repainting the band.

## Goal

The session card does not change when I move to another directory. `cd` into another
project and the card keeps naming the directory the tab was opened in — while the git
branch drawn beside it on the same card updates correctly.

## Non-goals

- **The cwd derivation itself.** `Deriver::tab` already re-resolves the cwd every refresh
  (`crates/app/src/tabmeta/worker.rs:107-116`), `publish_if_changed` already treats a
  cwd-only change as publishable (`worker.rs:1016-1064` pins exactly that), and the wake
  already reaches a redraw. Nothing on the write side is in scope.
- **The label precedence chain.** `resolve_line1_tier`'s ordering — user > AI-task >
  program-OSC > cwd (`crates/app/src/sidebar_model.rs:93-110`) — is unchanged. This spec
  changes *what the cwd tier reads*, never which tier wins.
- **What the waiting band shows.** The band names a tab by its folder today; whether it
  should instead show the resolved label is a separate design question. Only the staleness
  is in scope.
- **OSC 7 / shell-reported cwd.** The shipped integration emits OSC 133 only
  (`crates/term-core/assets/shell-integration/zshrc.zsh`) and OSC 7 is deliberately ignored
  (`crates/term-core/src/parser.rs:376-377`). Adding a second cwd source would violate
  `tmux-pane-cwd` D5's one-owner rule.
- **Anything in `term-core` or the renderer's layout.** This is an `app`-side read-path fix
  plus one field on the existing `RowMeta` seam.

## Confirmed Goal

Every surface that names a tab's directory reads the **live published cwd**
(`TabMeta::cwd`) rather than a copy taken at the last tab action, so that:

1. A bare `cd` in a tab updates that tab's card within one worker tick (500 ms active,
   2 s background) — with no tab switch, no rename, and no OSC 0/2 title from the shell.
2. The folder name and the git branch on one card always describe **the same directory at
   the same instant** — the property `tmux-pane-cwd` D5 claims and does not currently hold.
3. All four folder-name consumers agree: card line 1's cwd tier, line 2's repo name, the
   waiting-band row, and the rename prefill.
4. A tab whose metadata has not been published yet still shows the assembly-time folder
   name rather than going blank.

## Research

### The defect chain — one frozen string, four consumers

- `SidebarRow.title` is the only folder-name carrier the renderer sees
  (`crates/renderer/src/sidebar_row.rs:55`). Its doc calls it "its title"; `Tabs::titles`'s
  doc calls it "the live cwd basename" (`crates/app/src/tabs.rs:167-172`).
- It is built in exactly **one** place: `Tabs::titles()` (`crates/app/src/tabs.rs:195-200`),
  from `s.cwd(meta.get(s.id))` → `file_name()` → `Tab {n}` fallback.
- `Tabs::titles()` has exactly **one** caller: `sync_tab_model()`
  (`crates/app/src/terminal_app.rs:943`).
- `sync_tab_model()` has exactly **five** callers: first assembly at startup
  (`terminal_app.rs:447`), an `AppRequest` batch that changed something — in practice a user
  title being set (`:779`), the card menu's *Reset to auto* (`:914`), `after_roster_change`
  — tab open / close / switch (`:1718`), and a program's OSC 0/2 title arriving (`:2145`).
- **A `cd` is none of those five.** So the row title is frozen at whatever was resolved on
  the last tab action; for a never-renamed tab under a shell that sets no title, that is the
  directory the tab was opened in, for the life of the tab.

### Everything else on the card is already live — the repaint is not the problem

- The worker re-resolves the cwd on every refresh and publishes it on `TabMeta::cwd`
  (`crates/app/src/tabmeta/worker.rs:107-116`; the field at `crates/app/src/tabmeta/mod.rs:57-63`).
- `publish_if_changed` bumps the generation and wakes on a **cwd-only** change — pinned by
  `a_cwd_only_change_publishes_and_wakes` (`worker.rs:1016-1064`), whose comment says the
  wake exists precisely so consumers learn the pane moved.
- The wake lands on `UserEvent::OverlayPending` (`terminal_app.rs:2258-2272`).
  `TabMetaPump::poll` returns `true` whenever the generation moved
  (`tabmeta/mod.rs:316-321`), so `overlays.poll_all()` is `true` and `inner.redraw()` runs.
- On that frame `refresh_meta` re-pulls because the generation moved
  (`sidebar_row.rs:1104-1111`) → `SidebarMetaSource::fill` (`sidebar_model.rs:42-70`)
  rebuilds `branch`, `mark`, `skin` and `line1`. **The fresh `TabMeta` — `cwd` included — is
  in hand at `sidebar_model.rs:48`.**
- But `fill` passes `&row.title` as the cwd tier of the label chain
  (`sidebar_model.rs:56-61`, the argument at `:60`). That one argument is the whole defect:
  the live value is present and unread.

### Where the stale string is drawn

- **Line 1 fallback** — `card_line1_text(&meta.line1, rows[i].title.as_str())`
  (`sidebar_row.rs:1375`). On a plain card `meta.line1` *is* the row title, resolved from it
  one layer up, so both paths carry the same stale text.
- **Line 2's repo name** on a custom-labelled card — `truncate_to_width(&rows[i].title, …)`
  (`sidebar_row.rs:1473`), drawn only when `meta.line1_is_custom` (`:1470-1488`).
- **Waiting-band row** — `build_waiting_rows` clones `r.title` from the same rows
  (`terminal_app.rs:968-983`); drawn at `sidebar_row.rs:1261`.
- **Rename prefill** — `displayed_label` (`terminal_app.rs:843-871`) is the one consumer
  already correct: it syncs the snapshot itself (`:846-848`) and re-derives the basename
  from `session.cwd(...)` (`:852-857`).

### The invariant this breaks

- `tmux-pane-cwd` **D5** ("One cwd, one owner") claims: *"the title, the git branch, the
  agent badge, the task label, the overlay's log directory and the rename prefill are all
  guaranteed to describe the same directory at the same instant"*
  (`specs/tmux-pane-cwd/spec.md`, D5 rationale). True of the *value* — there is one
  derivation — but not of the *read*: five of six consumers read it live and the title reads
  a copy taken at the last tab action.
- `tmux-pane-cwd` **Known Gaps** records the residue as *"A briefly-wrong tab title survives
  outside the overlay … the sidebar row and rename prefill still show the tmux client's
  directory basename, **self-correcting on the next tick**"*. For the sidebar row that
  self-correction does not exist — nothing re-runs `Tabs::titles()` on a tick. The gap is
  therefore permanent, not one tick wide, and it fires on a plain `cd` with no tmux at all.
- Observable consequence: `cd` between two repos and the card shows **old folder + new
  branch** on the same line-2 — because `branch` is pulled live from the same `TabMeta` the
  title ignores.

### Why it can look intermittent

- If a user's zsh emits an OSC 0/2 title each prompt (oh-my-zsh, powerlevel10k, starship),
  `Session::drain` flags `title_dirty` (`crates/app/src/session.rs:133-137`) and
  `terminal_app.rs:2145` calls `sync_tab_model()` — refreshing the rows as a **side effect**
  of a title the card may not even show.
- The shipped integration emits OSC 133 only
  (`crates/term-core/assets/shell-integration/zshrc.zsh:23,35`) and OSC 7 (cwd) is
  deliberately ignored (`crates/term-core/src/parser.rs:376-377`), so a plain zsh never
  triggers that path. Two users on the same build can therefore see different behaviour.

### Change-surface facts

- `RowMeta` is constructed **literally in one place** (`sidebar_model.rs:62`); every other
  instance comes from `Default` (`sidebar_row.rs:1117`, `:1294`, `:1673`, `:1703`), so
  adding a field is a two-site edit inside the renderer (struct + `Default`).
- `RowMeta`'s own doc already argues that live data belongs on this channel rather than on
  `SidebarRow`: *"it refreshes live off the metadata generation — which is why the app
  resolves it in its `MetaSource::fill` and not when it assembles the rows"*
  (`sidebar_row.rs:116-122`). The precedent for this fix is the `line1` field itself.
- `refresh_meta` gates the pull on `gen != self.meta_gen || rows_rev != self.rows_rev_seen`
  (`sidebar_row.rs:1104-1111`), so per-fill work is paid at the publish rate (≤2 Hz), not
  per frame — the budget `line1.to_string()` already occupies.
- `published_or` (`session.rs:301-309`) returns the published cwd without evaluating its
  fallback closure, so reading `meta.cwd` in `fill` costs **zero syscalls** — the same
  laziness R6.1 measures for the keystroke path (`session.rs:396-416`).
- The basename derivation is duplicated today: `tabs.rs:195-200` and
  `terminal_app.rs:852-857`, with the band re-using the first. There is no shared helper, so
  a fix that adds a third site invites drift.
- **Borrow order at the fill site** (L2 review finding — not an existing pattern):
  `resolve_line1_tier<'a>` returns `&'a str` borrowed from its `cwd` argument
  (`sidebar_model.rs:93-98`). Today that argument is `&row.title`, borrowed from a slice
  independent of `out`; with a locally-owned folder name the `RowMeta` literal must evaluate
  `line1.to_string()` **before** moving that `String` into the struct.
- Rules in force: `app` + `renderer` only, `term-core` untouched
  (`.claude/rules/dependency-direction.md`); the renderer must not name app types, which is
  why the value must cross as a plain `String` on `RowMeta`
  (`.claude/rules/no-gui-crates.md`, `sidebar_row.rs:103`).
- Build/test/lint: `cargo build`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings` (`CLAUDE.md`).

## Decisions

### D1: The live folder name rides the per-frame `MetaSource` pull, as a new `RowMeta` field
- **Status**: resolved
- **Rationale**: the cwd became live data when `tmux-pane-cwd` published it on `TabMeta`;
  live data already has a channel, and `RowMeta`'s own doc says so — *"it refreshes live off
  the metadata generation — which is why the app resolves it in its `MetaSource::fill` and
  not when it assembles the rows"* (`sidebar_row.rs:116-122`). The task label set that
  precedent for exactly this reason, one bug earlier. So `fill` resolves the folder name
  from `meta.cwd` and carries it to the renderer on `RowMeta`; no new trigger, no new worker
  traffic, and the pull is already gated on the generation.
  **The rejected alternative — rebuilding the rows whenever the published cwd changes —
  stated at its strongest.** `SidebarRow.title` is not "the card's folder name"; it is the
  app's one per-tab identity string, and three of its four readers never pass through `fill`
  at all: line 1's fallback (`sidebar_row.rs:1375`), line 2's repo name (`:1473`), and
  `build_waiting_rows` (`terminal_app.rs:968-983`). So D1 cannot fix this at one site — it
  needs D3, D5 and D7 beside it, and it takes the number of folder-name *carriers* from one
  to three. A resync keeps that count at one and makes Confirmed Goal 3 true **by
  construction** rather than by four coordinated edits. The historical reason the rows are
  assembly-time-only has also expired: `Tabs::titles`'s doc justifies its cadence by "its
  per-tab `proc_pidinfo` is not on the render path (SD4)" (`tabs.rs:170-172`), but since
  `tmux-pane-cwd` that read goes through `published_or` and costs **zero syscalls** on a
  published value (`session.rs:301-309`, measured at `:396-416`). The app-side cost of a
  resync is now approximately nothing, and this decision does not rest on it.

  **Why it is rejected anyway — on the worker cost, not the app cost.** `Tabs::titles()` also
  sends `set_roster` + `set_active` (`tabs.rs:178-190`), and `Cmd::SetActive` derives the
  active tab **in band and unconditionally**, bypassing `should_refresh`
  (`worker.rs:281-299`). Its three **raw process reads carry no tick memo** — `agent_of`,
  `cwd_of` and `start_ms_of` are bare function pointers (`worker.rs:70-72`, `:86-88`), unlike
  `git_memo` / `tmux_memo` / `act_memo`, and `cwd_of` runs unconditionally on every
  `Deriver::tab` (`worker.rs:108-116`). So resync-on-publish roughly doubles the worker's
  process walks and puts a full derive on a wake-driven path with no rate limiter, braked
  only by the differ. D1 buys the same correctness for no worker traffic at all.

  **Correcting an earlier draft of this rationale** (L2 review finding), which claimed
  `resolve_log_in`'s mtime flapping would spin that cycle. It would not: ranking is memoised
  behind `RESOLVE_TTL_TICKS = 20` and short-circuits inside the TTL (`activity.rs:50`,
  `:400-404`), so two derives in one tick return the same path and the differ stops the cycle
  after one extra derive. The flap is real; it cannot drive this loop. The argument above
  replaces it.

  **The multiplied-carrier objection, answered.** What multiplies is *readers of one live
  value*, not copies of it. `TabMeta::cwd` stays the single source (Constraints), D7 gives the
  derivation a single definition, and every other live field already has several readers —
  `branch` is read by the card, the task label by the card and the rename prefill. The defect
  being fixed is not "many readers"; it is **a copy no event could refresh**. D1 does not add
  a second copy — it demotes the existing one (D2) to a bounded-staleness fallback that is
  never read once a publish exists.

  **Rejected: an OSC 7 cwd report from the shell** — a second cwd owner, against
  `tmux-pane-cwd` D5's one-owner rule, and it would only work for shells that emit it.

### D2: `SidebarRow.title` is demoted to a documented, tested pre-publish fallback, applied in `fill`
- **Status**: resolved
- **Rationale**: it stays the only answer in two real cases — the window before a new tab's
  first publish, and any renderer path with no `MetaSource` installed, which resizes to
  `RowMeta::default()` instead (`sidebar_row.rs:1113-1119`). Keeping it, documenting it as a
  fallback, and pinning *published-beats-assembly-time* with a unit test costs one test and
  leaves every no-source path drawing a name.
  **The fallback is applied in `fill`, not in the renderer** (L2 review finding — the spec
  previously pinned the ordering and left the site open, and the two readings diverge in a
  reachable case). `fill` resolves `cwd_title` as *published basename, else `row.title`*, so
  on any real fill the field is non-empty and `""` means **`RowMeta::default()` only**. Had
  the fallback lived renderer-side instead, a tab with a user title set (so
  `line1_is_custom`, the one condition under which line 2 draws the folder name,
  `sidebar_row.rs:1470-1488`) would render a **blank** repo name during the ≤500 ms
  not-yet-published window — the blank card this decision exists to refuse.
  **Two docs are demoted, not one.** `tabs.rs:167-172` ("the live cwd basename", plus an SD4
  cost claim that `published_or` has since made false) and the field's own doc at
  `sidebar_row.rs:53-54` ("The cwd basename — the bottom of the label chain, and line 2's
  repo/folder name when line 1 is custom"), which is the one a renderer-side reader hits and
  asserts liveness the more strongly of the two.
  **Rejected: making it unreachable from the card path** — having the app always supply the
  string is the strongest guarantee, since the stale value then cannot be drawn at all. But a
  card drawn before the first `fill`, and every renderer-only test, would lose its folder
  name entirely; the guarantee is not worth a blank card.

### D3: One resolved value feeds line 1's cwd tier and line 2's repo name
- **Status**: resolved (Tier-2 implication of D1)
- **Rationale**: line 1 falls back to the cwd tier (`sidebar_model.rs:102-109`) and line 2
  draws the repo name on a custom card (`sidebar_row.rs:1470-1488`). If those read different
  sources, one card could name two different folders — the same class of inconsistency as
  today's old-folder-plus-new-branch, reintroduced one line down. So `fill` resolves the
  folder name **once** and hands the same value to `resolve_line1_tier`'s `cwd` argument and
  to `RowMeta::cwd_title`.
  **Rejected: letting the renderer resolve line 2's name from its own source** — the renderer
  deliberately holds no part of the label policy (`sidebar_row.rs:383-389`, sticky-tab-title
  D7), and splitting the source is what created this bug in the first place.

### D4: The folder name stays the basename — colliding names are not disambiguated
- **Status**: resolved (Inversion Probe)
- **Rationale**: `~/work/api/src` → `~/work/web/src` satisfies every requirement here and
  still looks like nothing changed, because both render as `src`. Accepted: the basename is
  what every folder-name surface has always shown, and they must keep agreeing with each
  other and with the `Tab {n}` fallback.
  **Rejected: disambiguating when two open tabs' folder names collide** — it fixes the
  invisible-change case, but it needs a parent-path truncation policy on line 1, on line 2's
  repo budget (`repo_line2_budget`, `sidebar_row.rs:459`) and in the band, and it changes
  what a *correct* card looks like. That is a feature, not the repair of a stale read.

### D5: The waiting band takes the live cwd at rebuild time — and a metadata publish is a rebuild
- **Status**: resolved (rationale and trigger corrected after L2 review)
- **Rationale**: `build_waiting_rows` (`terminal_app.rs:968-983`) already runs with
  `self.tabmeta_snapshot` freshly synced by `feed_agent_states` (`:1016-1034`), so resolving
  the folder name there costs one lookup and no I/O. Its fallback is the row's own title,
  which it already has in hand (`:972-975`) — the same rule as D2, so band and card degrade
  identically.
  **Correcting the first draft**, which claimed `refresh_waiting_band` "rebuilds the band
  right after" every sync. It does not. Both of its call sites are gated on the waiting
  **set** changing (`terminal_app.rs:2180-2182`, `:2266-2268`), and a cwd-only publish moves
  no waiting item — so under that draft the band would never rebuild on the one event this
  spec exists to handle, and Confirmed Goal 3 would have been *contradicted* by Known Gap 4
  rather than qualified by it.
  **So the trigger is corrected, gated on the metadata generation the app already holds.**
  `Running` remembers the generation its band was last built from; on `OverlayPending`, after
  `feed_agent_states` has synced the snapshot, the band rebuilds when the snapshot's current
  generation differs from it — **in addition to, never instead of, the existing
  `waiting_changed` guard** (`terminal_app.rs:2266-2268`). That distinction is load-bearing:
  `waiting_changed` comes from `push_agent_states()`, which re-derives from the snapshot *and
  each tab's screen*, so it can go true on a wake where the generation did not move (a
  timing-driven `Waiting` edge). Replacing the guard rather than joining it would stop the
  band repainting for exactly those transitions — a T4 regression, silently. The two are
  merged into **one** condition (`waiting_changed || band_gen != snapshot_gen`) that updates
  the stored generation on either arm, so a publish that also flips a tab to `Waiting` does
  not rebuild twice.
  That comparison is **level-triggered against a value, not an
  event**, so `displayed_label`'s own sync cannot consume it; it is **narrower** than
  `overlay_changed`, so an ai-monitor tail wake never touches the band; and it short-circuits
  **before** `refresh_waiting_band`'s `m.rows.clone()` (`terminal_app.rs:992`), which would
  otherwise allocate O(tabs) `String`s on the app's hottest event. Rebuilding at the publish
  rate (≤2 Hz) also lands the unconditional `rows_rev` bump `set_waiting` performs
  (`sidebar_row.rs:576-580`) on a frame that is re-filling anyway.
  This needs one seam addition: `TabMetaSnapshot::generation` is private
  (`tabmeta/mod.rs:258-264`) and gains a reader. Named here because it is a contract change,
  however small.
  **The gate depends on a property worth recording:** *generation moved* ⟹ `overlay_changed`
  ⟹ `redraw()` (`TabMetaPump::poll` reads the same atomic, `tabmeta/mod.rs:315-320`;
  the redraw at `terminal_app.rs:2269-2271`). That is what carries a rebuilt band to the
  screen. It holds today and this decision leans on it; a later refinement that narrows the
  redraw condition would make the band update invisibly.
  **Rejected: calling `refresh_waiting_band` unconditionally behind a change-only equality
  guard inside it** — this decision's own first correction, and wrong for the very reason its
  neighbouring rejection gave. The guard can only return early *after* the `m.rows.clone()`
  above, so the "harmless" no-op is paid on every `OverlayPending` wake while an agent
  streams; and it changes the contract of a function two other paths call
  (`terminal_app.rs:2180-2182`, `:2266-2268`) to save work inside an event that is already
  re-filling.
  **Rejected: gating on `overlay_changed`** — `true` for *any* overlay, so the band would
  rebuild at the ai-monitor's tail rate; the generation gate is strictly narrower for the same
  wiring and the same line count.
  **Rejected: edge-triggering on `TabMetaHandle::sync`'s `bool`** — exactly the trap
  `tmux-pane-cwd` D6 documents: `displayed_label` syncs the same snapshot
  (`terminal_app.rs:846-848`), so a Cmd+Shift+* keystroke can consume the generation edge
  first and the rebuild is silently lost. A stored generation cannot lose it.
  **Rejected: a second renderer pull channel for the band** — fully live, but it duplicates
  the `MetaSource` machinery for a surface that exists only while something is waiting, and
  the band's rows are app-built by design (D6/D9 of `left-sidebar-waiting-area`).
  **Rejected: leaving the band on the assembly-time name** — it would contradict the scope
  confirmed at L0 and leave two folder names on screen disagreeing.

### D6: `RowMeta::cwd_title` is carried on every fill, never conditionally
- **Status**: resolved (derived)
- **Rationale**: line 2 draws the folder name only on a custom card, so a conditional fill is
  superficially tempting. It would require the app to know `show_branch` — renderer geometry
  (`sidebar_row.rs:242`) — and which tier won, and the tier changes as labels come and go, so
  the condition would have to be re-evaluated exactly as often as the value itself. Always
  carrying it is one `String` per row per publish, inside the budget `line1.to_string()`
  already occupies (`sidebar_model.rs:67`).
  **The absent sentinel is `""`, matching its sibling.** `line1`'s doc already reads *"Empty
  for a `default()` row; the layer falls back to the cwd title then"* (`sidebar_row.rs:122`),
  and `resolve_line1_tier` already treats empty as absent at every tier
  (`sidebar_model.rs:99-106`). `cwd_title` inherits that convention verbatim, and the
  renderer's line-2 site (`sidebar_row.rs:1473`) reads `cwd_title` when non-empty, else
  `rows[i].title`.
  **That else-branch is unreachable by construction, and is kept deliberately.** Reaching it
  needs an empty `cwd_title`, which under D2's fill-site fallback means a `RowMeta::default()`
  row — and `default()` sets `line1_is_custom: false` (`sidebar_row.rs:138`), while line 2
  draws the folder name only under `line1_is_custom` (`:1471`). It is defence-in-depth
  against a future change to either half, so it must be **commented as unreachable and not
  given a test**; a test asserting it would be asserting a state the type cannot produce.
  **Rejected: `Option<String>` or a conditional fill** — same allocation in the common case,
  plus a second way for line 1 and line 2 to disagree, which D3 exists to prevent, plus a
  sentinel convention differing from the field beside it.

### D7: One shared basename helper serves every folder-name surface
- **Status**: resolved (derived from I2)
- **Rationale**: the basename derivation is already duplicated at `tabs.rs:195-200` and
  `terminal_app.rs:852-857`; D1 and D5 would add a third and a fourth site. A single app-side
  helper — returning `Option<String>` so each caller keeps its own fallback (`Tab {n}` for a
  row, empty for the rename prefill, the row title for the card) — gives "what the folder is
  called" exactly one definition, which is the property this whole spec is restoring one
  level up. It lives in `sidebar_model.rs` — the module whose stated job is joining tab
  metadata to what the card shows, and already the home of the sibling policy function
  `resolve_line1_tier` — not in `tabs.rs`, whose copy is the one being demoted.
  **Rejected: inlining the derivation at each site** — it is three lines, which is precisely
  why it drifted; the divergence this bug is made of started as a copy that was correct.
  **Rejected: a new `folder.rs` module** — one function, whose only callers are the sidebar
  surfaces `sidebar_model.rs` already serves.

## Constraints

- `TabMeta::cwd` stays the **single owner** of a tab's directory (`tmux-pane-cwd` D5). No
  second source may be introduced — not OSC 7, not a per-surface syscall.
- **No new per-frame work.** `fill` must stay gated on the metadata generation
  (`sidebar_row.rs:1104-1111`); the G1 idle-CPU property (`docs/architecture.md`) must not
  regress.
- **No new publish trigger and no new worker command traffic** — the
  publish → resync → derive → publish cycle D1 rejects must not be reintroduced by any later
  refinement.
- **Zero extra syscalls on the read path.** The card reads the published cwd only; the
  laziness `published_or` guarantees (`session.rs:301-309`, measured by R6.1) must hold.
- The label precedence chain user > AI-task > program-OSC > cwd is unchanged
  (`sidebar_model.rs:93-110`).
- `term-core` is untouched and the dependency direction `common ← term-core ← renderer ← app`
  holds (`.claude/rules/dependency-direction.md`).
- The renderer names no app type — the folder name crosses as a plain `String`
  (`.claude/rules/no-gui-crates.md`, `sidebar_row.rs:103`).
- macOS ARM64 only; no `#[cfg(target_os)]` branches (`.claude/rules/macos-only.md`).
- No `unwrap()` outside tests; `cargo clippy --workspace --all-targets -- -D warnings` clean
  (`CLAUDE.md`).

## Known Gaps

- **If the tabmeta worker never installed, the fix silently no-ops.** `global()` is `None`
  when `install` never ran (`tabmeta/mod.rs`), so nothing is ever published, every row falls
  through to the assembly-time title, and cards freeze exactly as they do today. Inherited
  verbatim from `tmux-pane-cwd`'s own recorded gap — recorded here, not closed.
- **Two directories with the same basename are indistinguishable** (D4, accepted).
  `~/a/src` → `~/b/src` is a correct render that looks like nothing happened.
- **The card lags a `cd` by up to one worker tick** — 500 ms active, 2 s background
  (`worker.rs:27-31`). The cadence is inherited, not chosen, and unmeasured for this use
  (`docs/architecture.md` G1).
- **Band and card refresh on the same wake but in different phases** (D5). Both are driven by
  the metadata generation on one `OverlayPending`: the band app-side before `redraw()`, the
  card renderer-side during the frame. The residual is a one-frame skew if a publish lands
  between the two phases — not the indefinite divergence the first draft of this gap
  described. Accepted; closing it would mean a second renderer pull channel (D5, rejected).
- **The band rebuilds on every metadata publish, not only on a cwd change** (D5). The gate is
  the generation, which moves for a branch, task or agent-state change too, so the band pays
  a rows clone at up to 2 Hz while any tab's metadata is moving. Bounded by the same rate
  `fill` already runs at, and the alternative — a per-field comparison — would mean the app
  re-deriving what the differ already decided.
- **`tmux-pane-cwd`'s Known Gaps text is wrong and should be amended when this ships.** It
  records the stale sidebar title as *"self-correcting on the next tick"*; nothing re-runs
  `Tabs::titles()` on a tick, so the gap is permanent and fires without tmux at all. Left
  uncorrected, the next reader re-derives the same wrong conclusion.
- L2 unresolved/provisional sweep: **none** — every checkpoint resolved. Composite 1.00 as
  approved; it sat at ≈0.94 between the two L2-review passes, when D5's trigger did not
  follow from its own stated criterion. Nothing carried forward as a gap.

## Requirements

### R0: A `cd` changes the card

#### R0.1: A `cd` renames a plain card within one refresh
- **Given**: a tab with no user title, no agent task and no program title, whose card reads `terminal`
- **When**: the user runs `cd ~/other-repo` and one worker refresh elapses (≤500 ms active, ≤2 s background)
- **Then**: the card's line 1 reads `other-repo`, with no tab switch, no rename and no OSC 0/2 involved

#### R0.2: The folder name and the branch never describe different directories
- **Given**: a tab in repo A on branch `main`, whose card shows `A` and `main`
- **When**: the user cds into repo B, which is on branch `dev`, and the next publish is drawn
- **Then**: the card shows `B` and `dev` on the same frame — never `A` with `dev`

#### R0.3: A renamed tab's line 2 follows the shell too
- **Given**: a tab the user renamed `Frontend`, so line 1 is the user title and line 2 shows the repo name beside the branch
- **When**: the user cds to another repo
- **Then**: line 1 still reads `Frontend` and line 2's repo name is the new folder — the case that is *permanently* stale today, since a user title suppresses `title_dirty` entirely (`session.rs:136`)

### R1: The app resolves one live folder name per row on the metadata pull (D1, D3, D6)

#### R1.1: The fill resolves the folder name from the published cwd
- **Given**: a row for tab 7 and a snapshot in which tab 7's `TabMeta.cwd` is `/Users/x/web`
- **When**: `MetaSource::fill` runs for that row
- **Then**: the resolved folder name is `web`, whatever `SidebarRow.title` holds

#### R1.2: One value feeds line 1's cwd tier and line 2's repo name
- **Given**: a row whose live folder name is `web` and whose user title is `Frontend`
- **When**: the fill runs
- **Then**: `resolve_line1_tier` receives `web` as its cwd argument **and** `RowMeta.cwd_title` is `web` — the same resolved string, not two derivations

#### R1.3: The folder name is carried even when line 2 will not draw it
- **Given**: a row whose line 1 is not custom, so line 2 renders the branch alone
- **When**: the fill runs
- **Then**: `RowMeta.cwd_title` is still populated with the live folder name

#### R1.4: The renderer draws the pulled name on line 2
- **Given**: a `RowMeta` with `line1_is_custom: true` and `cwd_title: "web"`, joined to a `SidebarRow` whose `title` is `api`
- **When**: the card's line 2 is laid out
- **Then**: the repo name drawn is `web`, and `SidebarRow.title` is not read on that path

#### R1.5: The renderer's empty-name branch is marked unreachable and left untested
- **Given**: `RowMeta::default()` sets `line1_is_custom: false` (`sidebar_row.rs:138`) while line 2 draws the folder name only under that flag (`:1471`)
- **When**: the line-2 fallback to `rows[i].title` is written
- **Then**: it carries a comment stating it is unreachable under D2's fill-site fallback, and **no test asserts it** — a test there would assert a state the type cannot produce

### R2: Fallback semantics — the demoted field (D2)

#### R2.1: An unpolled tab shows today's name, never a blank
- **Given**: a tab opened moments ago, for which `snap.get(id)` is still `None`
- **When**: the fill runs for its row
- **Then**: the card shows `SidebarRow.title` — exactly what it shows today — on both lines

#### R2.2: A published cwd beats the assembly-time title
- **Given**: a row whose `title` is `api` and whose published cwd is `/Users/x/web`
- **When**: the fill runs
- **Then**: the card shows `web`, and a unit test pins this precedence so the demoted field cannot silently win again

#### R2.3: An unresolvable cwd keeps the `Tab {n}` fallback
- **Given**: a tab whose published cwd is `None` and whose row title is `Tab 3`
- **When**: the fill runs
- **Then**: the card shows `Tab 3`, unchanged from today

#### R2.4: Both now-false docs are corrected
- **Given**: `tabs.rs:167-172` calls the value "the live cwd basename" and justifies its cadence with an SD4 `proc_pidinfo` cost, and `sidebar_row.rs:53-54` describes the field as the card's folder name
- **When**: the change lands
- **Then**: both describe it as the assembly-time fallback, and the SD4 cost claim is removed or marked spent (`published_or` made it free)

### R3: The waiting band takes the same live name (D5)

#### R3.1: A band row shows the live folder name
- **Given**: a tab present in the waiting band whose card now reads `web` after a `cd`
- **When**: the next metadata publish is handled
- **Then**: the band row reads `web` as well, so the two zones never name different folders for one tab

#### R3.2: The generation gate is additive and merged into one condition
- **Given**: the `OverlayPending` arm's existing `if waiting_changed { refresh_waiting_band() }` (`terminal_app.rs:2266-2268`)
- **When**: the generation gate is added
- **Then**: the condition becomes `waiting_changed || band_gen != snapshot_gen` and updates `band_gen` on either arm — so a screen-derived `Waiting` edge with no generation move **still** repaints the band, and a publish that also flips a tab to `Waiting` rebuilds it exactly once

#### R3.3: A band row for an unpolled tab falls back to the row title
- **Given**: a band row whose tab has no published cwd
- **When**: `build_waiting_rows` runs
- **Then**: the row shows the `SidebarRow.title` it already holds (`terminal_app.rs:972-975`) — the same degrade rule as the card

#### R3.4: The snapshot's generation becomes readable
- **Given**: `TabMetaSnapshot::generation` is a private field with no reader (`tabmeta/mod.rs:258-264`)
- **When**: the gate needs to compare it
- **Then**: a reader is added on `TabMetaSnapshot` and no other field of the snapshot is exposed

### R4: One definition of "the folder name" (D7)

#### R4.1: A single helper serves every folder-name surface
- **Given**: the basename derivation is duplicated at `tabs.rs:195-200` and `terminal_app.rs:852-857`
- **When**: the change lands
- **Then**: one helper in `sidebar_model.rs` is the only place deriving it, called by the row assembly, the card fill, the waiting band and the rename prefill

#### R4.2: Each caller keeps its own fallback
- **Given**: the helper returns `Option<String>` and takes `Option<&Path>`
- **When**: each of the four callers uses it
- **Then**: the row falls back to `Tab {n}`, the rename prefill to empty, the card and the band to the row title — no fallback is baked into the helper, since the four differ

### R5: Cost and non-regression (Constraints, D4)

#### R5.1: The join performs no I/O
- **Given**: `probe::reset_all()` and a `TabMeta` carrying a published cwd
- **When**: the row-meta join runs
- **Then**: `probe::PROC_CWD` reads 0 — the pull consumes the published value and never re-resolves it

#### R5.2: The metadata path sends no worker command
- **Given**: the `OverlayPending` path now rebuilds the band
- **When**: a publish is handled
- **Then**: no `Cmd` reaches the worker — neither `set_roster` nor `set_active` is called — so publish → resync → derive → publish cannot close

#### R5.3: Colliding basenames render identically, by decision
- **Given**: two tabs whose cwds are `~/a/src` and `~/b/src`
- **When**: their cards are drawn
- **Then**: both read `src`; this is D4's accepted behaviour and is recorded as a Known Gap, not treated as a defect

#### R5.4: The label precedence chain is untouched
- **Given**: a tab carrying a user title, an AI task label and a program title at once
- **When**: the fill runs
- **Then**: line 1 shows the user title exactly as before, and the existing precedence tests in `sidebar_model.rs` pass unmodified

## Tasks

### T1: One folder-name helper, and migrate the two existing derivations onto it [infra]
- **Fulfills**: R4.1, R4.2, R2.4 (the `tabs.rs` half)
- **Depends on**: (none)
- **Touches**: `crates/app/src/sidebar_model.rs` (new helper), `crates/app/src/tabs.rs`,
  `crates/app/src/terminal_app.rs` (`displayed_label` only)
- Shared infrastructure, so a horizontal split is licensed: the helper has no single UI
  counterpart — all four surfaces call it. Doing it first is what stops T2 and T3 each
  inventing a third and fourth derivation, which is the drift D7 exists to prevent.
- The `tabs.rs` doc correction rides here because this task already edits that file;
  splitting a pure-doc task out would serialize T2 behind it for no benefit.

### T2: The card follows the shell [vertical]
- **Fulfills**: R0.1, R0.2, R0.3, R1.1, R1.2, R1.3, R1.4, R1.5, R2.1, R2.2, R2.3,
  R2.4 (the `sidebar_row.rs` half), R5.1, R5.3, R5.4
- **Depends on**: T1
- **Touches**: `crates/renderer/src/sidebar_row.rs`, `crates/app/src/sidebar_model.rs`
- One task, not two, **because the seam is the point**: `RowMeta::cwd_title`'s producer
  (`fill`) and consumer (line 2's draw) sit on opposite sides of the app→renderer boundary,
  and splitting them is exactly the horizontal split that risks the two halves disagreeing —
  the bug being fixed, reintroduced in the fix.
- Extracting the per-row join as a free function taking `(&SidebarRow, Option<&TabMeta>)` is
  what makes R1.*, R2.* and R5.1 unit-testable at all: `TabMetaSnapshot`'s fields are private
  to its module, so a test can reach that shape and not `fill`'s.

### T3: The waiting band follows the shell [vertical]
- **Fulfills**: R3.1, R3.2, R3.3, R3.4, R5.2
- **Depends on**: T1 — **parallel with T2**
- **Touches**: `crates/app/src/tabmeta/mod.rs` (the generation reader),
  `crates/app/src/terminal_app.rs` (`band_gen`, the merged gate, `build_waiting_rows`)
- Parallel with T2 under all three conditions: no file overlap (T2 is `sidebar_row.rs` +
  `sidebar_model.rs`, T3 is `terminal_app.rs` + `tabmeta/mod.rs`), no interface dependency
  (T3 consumes T1's helper, never T2's `cwd_title`), and no shared model — the band's rows
  and the card's `RowMeta` are separate types with separate publication paths.
- R3.2's merge is the whole risk of this task: the gate must **join** the existing
  `waiting_changed` guard, never replace it, or screen-derived `Waiting` edges stop
  repainting the band.

## External Dependencies

### Pre-work
- (none) — no new crates, no config keys, no schema. Every seam this touches already exists.

### Post-work
- **Amend `specs/tmux-pane-cwd/spec.md`'s Known Gaps**: it records the stale sidebar title as
  *"self-correcting on the next tick"*, which is false and is what let this survive. Correct
  it to point at this spec, or the next reader re-derives the same wrong conclusion.
- **Eyeball it with `/run-term`**: `cd` between two repos in a live tab and watch one card.
  No automated test covers the on-screen result — the unit tests pin the join, not the
  frame — and the Known Gaps' silent-no-op case (worker never installed) is only visible here.
