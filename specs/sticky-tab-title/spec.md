# Spec: sticky-tab-title

## Meta
- **Created**: 2026-07-27
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-07-28

## Goal
A user-set tab title sticks until the session is killed or the user changes it
again, overriding the live AI-task label from PR #13 (`feat: tab identity — live
per-tab AI-task label + manual OSC-2 override`). The override path already
exists, so the work is likely making it sticky/precedence-correct rather than
net-new.

## Non-goals
- No persistence across app restarts — "sticky" means for the life of the session, not on disk.
- No macOS window-title change; this is the sidebar card label only.
- No bulk/other-tab renaming — a title set targets exactly one tab.
- No new shell integration or `$SHELL` dispatch (`zsh-local-only`).

## Confirmed Goal
A title the **user** sets on a tab persists for the life of that session: it
survives shell/program-emitted title escapes (zsh precmd, vim, ssh, tmux), cwd
changes, and AI-task label updates. It is replaced only when the user sets it
again, cleared only by an explicit user clear, and discarded when the session
dies. Precedence on the sidebar card becomes: **user-set > AI-task > program/cwd
title**.

Done when: with a user title set on a tab, `cd`, an AI task starting/finishing,
and a program emitting its own OSC 2 all leave the card label unchanged; setting
a new user title replaces it; an explicit clear reverts to the auto label;
closing the tab discards it.

## Research

### The title path already exists, end to end
- Parser accepts OSC 0 / OSC 2, drops invalid UTF-8, forwards the string
  (`crates/term-core/src/parser.rs:362-377`).
- Grid stores it as a **transient one-shot** — a newer title overwrites an
  unconsumed one; absent from `GridSnapshot` (`crates/term-core/src/grid.rs:246-251`,
  `:1006-1016`).
- `Session::drain` takes it after each advance, stores it on the session, and sets
  `title_dirty` only on a real change (`crates/app/src/session.rs:108-116`).
- The event loop drains `take_title_dirty`, resyncs the sidebar model, and redraws —
  even for a background tab (`crates/app/src/terminal_app.rs:1059-1093`).
- `Tabs::titles` promotes a non-empty session title to `SidebarRow.label`
  (`crates/app/src/tabs.rs:161-179`).
- The renderer picks `manual.or(task).unwrap_or(cwd_title)`
  (`crates/renderer/src/sidebar_row.rs:295-303`).

**So the precedence order is already user > task > cwd** (`sidebar_row.rs:303`), and
storage already dies with the session (`title: String` on `Session`, dropped on tab
close). The ordering is not the defect.

### What actually breaks stickiness
- **OSC 0/2 is a shared channel, not a user channel.** zsh precmd/oh-my-zsh, vim,
  ssh, tmux and less all emit OSC 2. Every one of them overwrites `Session.title`
  (`session.rs:111-115`), so a user-set title is clobbered by the next prompt
  redraw — and, worse, a program-set title *masquerades* as the highest-precedence
  "manual label", outranking the AI-task label it was meant to defer to.
- **There is no in-app way for a user to set a title.** The only channel is typing
  an escape sequence into the shell, which is byte-identical to program output.
  `Session::set_title` exists but is `#[allow(dead_code)]`
  (`crates/app/src/session.rs:162-167`).
- **Empty title currently means "clear"** (`grid.rs:1013`, `session.rs:109`) — a
  program emitting `ESC]2;ST` would wipe a user title.

### Seams available for a user-only channel
- **Overlay seam**: `Overlay { priority, visible, on_key, layer, poll }`
  (`crates/app/src/seam.rs`), precedent `AiMonitorOverlay` — a Cmd+Shift+M modal that
  consumes keys before the shell and draws through `DrawLayer`
  (`crates/app/src/overlay.rs:44-120`). This is the established pattern for a
  key-capturing, self-drawn prompt with no GUI crate (`no-gui-crates`).
- **`AppCtx` is deliberately narrow** — `writer`, `tab_count`, `active`,
  `active_cwd`, `bracketed_paste` (`crates/app/src/seam.rs:46-66`). It has **no**
  title mutator, so an overlay-based rename needs one added.
- **Chord precedence** in `terminal_app.rs:965-1022`: visible overlays first, then
  registered handlers, then tab chords, then `encode_key`. Existing Cmd chords:
  Cmd+C (`copy.rs:23`), Cmd+V (`clipboard.rs:83`), Cmd+Shift+M (`overlay.rs:46`),
  Cmd+T/W/number (`input.rs:27`).
- **Sidebar click** already hit-tests rows to a `tab_id`
  (`terminal_app.rs:622`, `:1194-1284`) — an alternative entry point (double-click
  a card) if a mouse affordance is wanted.

### Constraints from the harness
- `term-core` must stay GPU/window-free; the renderer consumes `SidebarRow`, not a
  live `Grid` (`.claude/rules/dependency-direction.md`, `sidebar_row.rs:143`).
- No GUI crates — any rename prompt is drawn by hand as a `DrawLayer`
  (`.claude/rules/no-gui-crates.md`).
- Config is TOML+serde in `common` (`crates/common/src/config.rs:19-73`, with a
  `SidebarConfig`) — an available home for a "honor program titles" toggle.
- Tests: `term-core` has `tests/` with insta snapshots; **`crates/app` has no
  `tests/` dir** — app-level coverage is inline `#[cfg(test)]` over pure functions
  (`input.rs:423+`, `copy.rs:102+`). New logic should be shaped as pure functions to
  stay testable headlessly (`.claude/rules/dependency-direction.md`, D10).

## Decisions

### D1: The user channel is an in-app rename prompt (Cmd+Shift+R), not an escape sequence
- **Status**: resolved
- **Rationale**: Stickiness is impossible while the only channel (OSC 0/2) is one any
  program can write. A key chord captured by the app is a channel no program can forge.
  Built on the existing `Overlay` seam (`seam.rs`, precedent `AiMonitorOverlay`'s
  Cmd+Shift+M at `overlay.rs:46`), which already consumes keys before the shell and
  draws through `DrawLayer` — no GUI crate (`no-gui-crates`).
  **Rejected**: a private OSC number (e.g. `ESC]777;`) — cheapest, but still typed into
  the shell, still forgeable by a rogue program, and requires the user to remember a
  `printf`. **Rejected**: double-click-to-rename as the *only* entry — needs the same
  text-edit machinery anyway and is not keyboard-reachable.

### D2: A sidebar hamburger/settings button is a second entry point to the same rename action
- **Status**: resolved — superseded in scope by **D4**
- **Rationale**: User-requested. Discoverability: a chord alone is invisible to anyone
  who hasn't read the docs. Reuses the sidebar's existing ordered press path
  (`sidebar_press`, dispatched at `terminal_app.rs:1197`) and the hand-drawn card
  pipeline (`sidebar_row.rs`). The button opens the same rename state D1 defines — one
  implementation, two entries. D4 fixes what the menu contains.

### D3: Once a user title exists, program OSC 0/2 is recorded but never displayed for that tab
- **Status**: resolved
- **Rationale**: Strictest reading of "sticky" — vim/ssh/zsh-precmd titles cannot
  outrank the user, nor can they masquerade as the manual label and outrank the AI-task
  label (today's actual defect, `session.rs:111`). Recorded-but-hidden rather than
  dropped because the program channel must keep working for the tabs that still depend
  on it — a tab with **no user title and no AI task**, where the program title is the
  label (it still outranks the cwd basename, D5). Under D5/D12 it is never the fallback
  on clear when a task label exists; that narrower claim is what the recording buys.
  **Rejected**: dropping program titles outright — a cleared tab with no agent would
  fall all the way to the cwd basename, losing a working signal. **Rejected**:
  "program title wins while the program runs, then reverts" (iTerm2-ish) — explicitly
  not sticky, and needs run-state tracking the terminal doesn't have.

### D4: The hamburger is a per-tab card menu with Rename · Reset to auto · Close
- **Status**: resolved
- **Rationale**: Supersedes D2's open scope. Per-card (not one global sidebar menu) so
  the menu acts on the tab you clicked, giving rename a target-any-tab path the chord
  doesn't have (the chord targets the active tab). `Reset to auto` appears only when a
  user title is set — it is the discoverable form of D5's clear. `Close` is included
  because it is the other obvious per-tab action and is keyboard-only today (Cmd+W).
  `Close` on the **last** remaining tab exits the app, exactly as Cmd+W does
  (`close_active` returning `false`, `tabs.rs:88-97`) — no confirmation, because adding
  one here would make the mouse path stricter than the keyboard path for the same action.
  **Rejected**: greying out `Close` on the last tab — safer against a misclick, but
  makes the menu's behavior depend on tab count for no other reason. **Rejected**:
  cutting `Close` to keep the spec title-only — the user asked for it.
  **Rejected**: a single global sidebar hamburger — acts on the active tab only, so it
  adds no capability over the chord, and pulls in `SidebarConfig` settings scope this
  spec does not want.

### D5: Clearing reverts to the live auto label; a program OSC title now ranks BELOW the AI-task label
- **Status**: resolved
- **Rationale**: An empty rename (or `Reset to auto`) clears the user title and the card
  falls to the live auto label — AI-task if one exists, else cwd basename. The recorded
  program title is retained but permanently subordinate. This makes the full chain
  **user > AI-task > program OSC 0/2 > cwd**, which corrects PR #13's real defect: today
  a program's OSC 2 lands in the same slot as a manual label and outranks the AI-task
  label (`session.rs:111` → `tabs.rs:171`). **Rejected**: restoring the last OSC 2 on
  clear (PR #13's exact behavior) — reintroduces the masquerade the moment a title is
  cleared. **Rejected**: refusing empty / no clear path at all — a mis-typed title would
  be unfixable back to auto.

### D6: IME commits must be routed to a visible capturing overlay before the PTY
- **Status**: resolved (was `assumed`; the L2 reviewer disproved the assumption)
- **Rationale**: The first draft assumed `KeyInput.text` (`seam.rs:38-42`) was enough.
  It is not. Verified in-tree: `terminal_app.rs::on_ime` handles `Ime::Commit` by
  writing the composed bytes **straight to a session's PTY** (`terminal_app.rs:358`,
  `:364-430`) without consulting any overlay, and the composing gate (`suppress_key`,
  `input.rs:350`, applied at `terminal_app.rs:1336`) withholds `KeyboardInput` *before*
  overlay dispatch (`:965`). So with the rename field open, every
  committed Hangul syllable would be injected into zsh while the field received nothing
  — a direct violation of D8's "the field consumes keys before the shell". Therefore the
  IME path gains the same precedence rule the key path already has: when an overlay is
  visible and capturing text, `Ime::Commit` (and `Ime::Preedit`, for the in-field
  composition display) goes to the overlay, not to `composing_tab`'s PTY. This is a real
  work item, not a gap. **Rejected**: shipping with IME broken in the field and noting it
  — Hangul input is a first-class, already-supported case in this terminal.

### D7: The app resolves the 4-tier chain; `term-core` is untouched and the renderer stays dumb
- **Status**: resolved
- **Rationale**: `Tabs::titles` collapses user / AI-task / program-OSC / cwd into the
  single `SidebarRow` label the renderer draws. term-core keeps emitting raw OSC titles
  via its existing transient one-shot and learns nothing about "user" vs "program" —
  which keeps `dependency-direction` intact and means zero parser/grid churn (no insta
  snapshot rewrites). Precedence becomes one pure function in `app`, unit-testable
  inline like `input.rs`/`copy.rs` (L1: app has no `tests/` dir).
  **Rejected**: growing `SidebarRow` to four source fields and resolving in
  `sidebar_row.rs` — puts policy in the draw layer and widens the renderer contract.
  **Rejected**: a first-class title model in term-core — snapshot-testable, but term-core
  would have to *retain* titles it currently treats as transient, and it would need the
  notion of a user, which is app-level.

### D8: The rename field is modal over the keyboard; a tab switch cancels it, a dead session closes it
- **Status**: resolved
- **Rationale**: While open, the field consumes keys before the shell (the `Overlay`
  seam's existing `Handled::Consumed` contract, `overlay.rs:105`). PTY output keeps
  advancing grids and redrawing behind it — no buffering, no freeze. Any tab chord or
  sidebar click **cancels** the edit and then performs the switch; if the target session
  exits mid-edit the field closes and discards the text. The invariant: **nothing
  half-typed is ever committed**. **Rejected**: committing on switch — silently names a
  tab "Fron". **Rejected**: blocking switches until Enter/Esc — most predictable but
  makes a stray chord feel like a hang.

### D8a: One modal surface at a time, targeted by `TabId`; a chord dismisses the menu AND acts
- **Status**: resolved (resolves the D8 × D10 × D4 conflicts the L2 reviewer found)
- **Rationale**: Three ambiguities the first draft left open, settled together:
  1. **Stacking**: choosing `Rename` closes the menu and opens the field in the same
     event — the two surfaces are never open at once, so keyboard ownership is never
     shared. `Esc` in the field returns to nothing, not to the menu. Keyboard ownership
     needs one more thing to actually hold: `OverlayHost::dispatch` offers keys to
     **hidden** overlays too, by design, so an overlay can consume its own opening chord
     (`seam.rs:125-145`). The rename field therefore registers at a priority **above**
     `AiMonitorOverlay`'s `100` (`overlay.rs`) and, while visible, consumes *every* key
     it does not itself act on — otherwise Cmd+Shift+M would open the monitor on top of
     an open rename field.
  2. **Chord semantics are uniform** (D10 said "dismiss without acting", D8 said "cancel
     then switch" — for the same keystroke). The rule is D8's: whichever surface is
     open, a tab chord **dismisses it without committing, then performs the switch**.
     D10's phrasing is corrected to match. A chord is never swallowed.
  3. **Target ≠ active**: both surfaces carry the `TabId` they act on (D9), so renaming
     a background tab is well-defined. D8's "a tab switch cancels" applies to *any*
     switch, whether or not the destination is the surface's target — the user's
     attention has moved, so the edit is abandoned. If the surface's target session dies
     while it is open, the surface closes and discards.

### D9: `Session` gets two distinct title slots; app-side mutation is by `TabId`, not by active index
- **Status**: assumed (auto-resolved from L1 + D4/D7)
- **Rationale**: D3/D5/D7 require the user title and the last program title to coexist,
  so `Session.title` splits into `user_title: Option<String>` and
  `program_title: String` (today's OSC sink, `session.rs:111`). D4's per-card menu can
  target a **non-active** tab, so the mutators are keyed by `TabId` — `Tabs` currently
  exposes only `close_active` (`tabs.rs:88-97`), so a `close(TabId)` sibling is needed
  alongside `set_user_title(TabId, …)`. `AppCtx` (`seam.rs:46-66`) has no title mutator
  and no tab-id access, so the overlay seam is extended narrowly with exactly those two
  operations — not with a general `&mut Tabs`, which would blow the seam's deliberate
  narrowness. **`close(TabId)` must also fix `Tabs.active`**: today only `close_active`
  runs `active_after_close` (`tabs.rs:97`, `:209`), so closing a *non-active* tab needs
  its own index rule — the active tab keeps its identity, which means its index shifts
  down by one when the closed tab sat before it. **When the target *is* the active tab**
  it behaves exactly like today's `close_active` (`active_after_close`, `tabs.rs:209`),
  and it keeps that function's return contract: `false` when the last tab was closed, so
  the caller exits the app (D4's mouse path and Cmd+W then agree). Marked assumed: the
  exact seam shape is the implementer's call; the index invariant and the return contract
  are not.

### D13: The seam gains an app→overlay command direction; without it the mouse entry point is unbuildable
- **Status**: resolved (gap found by the L2 reviewer)
- **Rationale**: The seam is one-way. `OverlayHost` exposes `register`,
  `dispatch(KeyInput)`, `layers`, `poll_all`, `notify_active_changed`
  (`seam.rs:222-267`) — the app can hand overlays *keys* and *active-tab changes*, and
  D9 adds overlay→app mutators, but there is **no way for the app to command a specific
  overlay**. D8a.1 needs exactly that: the menu's `Rename` item is a mouse press handled
  in `terminal_app.rs`'s `SidebarAction` arm (`:1203`), and it must open the rename field
  targeted at a `TabId` in the same event; D8's "a sidebar click cancels the edit" needs
  the same direction. So `OverlayHost` gains a narrow broadcast — an `OverlayCommand`
  enum (`BeginRename(TabId)`, `Cancel`) and a `fn on_command(&mut self, …) -> Handled`
  on `Overlay` with a no-op default. It is the structural twin of the existing
  `notify_active_changed` fan-out, so it adds a direction, not a new mechanism.
  **Rejected**: owning the rename field directly in `terminal_app` instead of through
  the seam — avoids the seam change, but every feature since ai-monitor has gone through
  SD4 precisely so `terminal_app.rs` is not edited per feature. **Rejected**: a
  downcast/handle to reach one concrete overlay — defeats the trait boundary.

### D10: Menu geometry joins the renderer's ONE ordered hit-test; the app keeps performing no geometry
- **Status**: resolved (was `assumed`; the L2 reviewer showed the first phrasing was
  unimplementable)
- **Rationale**: The first draft said the app should hit-test the popup "before the
  existing sidebar row hit-test". That cannot be built: the app performs **no geometry**
  (SD2). The split, precisely — and note the first revision named the wrong function:
  - **Geometry lives in `renderer::sidebar_layout::hit_test`**
    (`sidebar_layout.rs:408-445`), the ONE ordered test. It gains two
    `SidebarRegion` variants — the per-card hamburger hit-box and, when a menu is open,
    its item rects — resolved **ahead of** step 4's `Card(i)`, in the same style as the
    waiting band's step 3.5.
  - **`sidebar_press` is app-side** (`input.rs:75`) and stays a pure region→action
    mapper: it maps the new regions to new `SidebarAction` variants. Index→`TabId`
    resolution is app-side too (precedent: `waiting_row_tab_index`,
    `terminal_app.rs:622`), so the renderer keeps speaking in row indices only.
  - **Open-menu state travels as layout input.** `hit_test` is pure over
    `SidebarLayoutInput` (`sidebar_layout.rs:104-120`), which carries no menu state, so
    the struct gains one field — `open_menu: Option<usize>` (the row whose menu is
    open) — pushed by the app exactly as `waiting_count` already is. It must carry the
    open menu's **item count** alongside the row index: D4 makes `Reset to auto`
    conditional on a user title existing, so a menu has 2 or 3 items, and `hit_test`
    cannot derive that from anything else it holds. Exact field shape is an R-level
    detail; that the count rides in the layout input is not. This is not policy
    in the renderer: the **app decides** whether a menu is open; the renderer is *told*,
    so it can lay the popup out and hit-test it. The menu's `DrawLayer` sorts above the
    sidebar layer.
  - Dismissal follows D8a: an outside click, `Esc`, or a tab chord dismisses without
    acting on a menu item.

  **Rejected**: an app-side geometry pre-test — violates SD2 and forks hit-testing into
  two places that must agree on layout. **Rejected**: letting the renderer *own* the
  menu's open/closed state (decide it, toggle it) — that is policy, and D7 keeps policy
  in the app; passing it down as layout input is the opposite arrangement.

### D11: Naming means naming — a named tab shows no live task text, and the card layout does not change
- **Status**: resolved (inversion probe)
- **Rationale**: The accepted trade: naming a tab is a statement about *identity*, and
  the AI-task label is what you get when you have not named it. Line 2 keeps repo/folder
  + git branch as-is (`sidebar_row.rs` D9), so this spec touches no layout. The activity
  signal is not fully lost — the card's existing agent-state mark still renders
  independently of the label. **Rejected**: moving the live task label to line 2 on named
  tabs — keeps both signals but adds a second precedence chain and a layout rule to a
  spec whose point is one chain. **Rejected**: nothing (accepting silent signal loss
  without stating it).

### D12: The program-OSC demotion is unconditional, not gated on the tab ever having been named
- **Status**: resolved (implication probe)
- **Rationale**: Confirms D5 applies to every tab, including tabs that never carried a
  user title. A program-set title is a weak ambient signal; the AI-task label is the
  stronger one; only an explicit user title beats it. This is a deliberate, accepted
  behavior change vs. PR #13 for un-named tabs that have both a zsh title and an AI task.
  **Rejected**: demoting only on tabs that have (or had) a user title — preserves PR #13
  for un-named tabs, but makes precedence depend on hidden history, so two identical-
  looking tabs would rank their labels differently.
- **Steelman addressed (L2 reviewer)**: the strongest case against D12 is that the
  criterion should be *freshness*, not *source* — the task label is **not** live
  (`sidebar_row.rs:295-297` documents that an idle agent keeps its last label), so an
  hour-dead task label would outrank the title vim is writing right now, making the
  stalest label the most privileged. Answered, not dodged: **the persisted task label is
  deliberate identity, not a liveness claim** — "this is the tab where I did X" is
  exactly the signal that distinguishes two shells in one repo, and it is *supposed* to
  survive the task ending (PR #13 D3). Liveness is carried by a different channel that
  this spec does not touch: the card's agent-state mark. **Rejected**: an idle TTL after
  which the program title wins — freshness-correct, but introduces a time-dependent
  label that changes with no user action and a new unanswerable constant (what N is).

## Constraints
- `term-core` must not learn about "user" vs "program" titles, and must not gain a
  retained title model — it keeps its transient one-shot (`dependency-direction`, D7).
- No GUI/widget crate for the rename field or the card menu — both are hand-drawn
  `DrawLayer`s (`no-gui-crates`).
- The renderer must not gain the precedence policy; it receives one resolved label (D7).
- The app performs no sidebar geometry — `renderer::sidebar_layout::hit_test` stays the
  ONE ordered hit-test; the app only maps regions to actions (SD2, D10).
- New features extend the SD4 seam rather than editing `terminal_app.rs` per feature
  (D13).
- Nothing half-typed is ever committed as a title (D8).
- Never more than one modal surface open at a time (D8a).
- A user title never survives its session — no disk persistence (L0 non-goal).
- No new polling thread. Note that a rename does **not** travel the PTY wake path:
  `title_dirty` is set inside `Session::drain` (`session.rs:108-116`), which an
  app-initiated rename never enters, so the rename/menu paths must call the tab-model
  resync + `request_redraw` directly (the same functions `terminal_app.rs:1074-1093`
  calls, not the same trigger).
- macOS/zsh only; no shell-integration script is required for the feature to work
  (`macos-only`, `zsh-local-only`).

## Known Gaps
- **L2 assumed: Data Model — seam shape (D9).** The exact `AppCtx` extension
  (`set_user_title(TabId, …)` / `close(TabId)`) is the implementer's call; its
  narrowness, the active-index invariant, and the `false`-on-last-tab return contract
  are not.
- **Open: IME preedit rendering (D6).** Routing `Ime::Commit` to the overlay is decided.
  Whether the in-field *preedit* (uncommitted Hangul jamo) is drawn inside the rename
  field, or only committed syllables appear, is not — the field would read correctly
  either way, but composition would be invisible until commit in the cheap version.
- **Open: menu affordance visibility.** Whether the hamburger is always drawn on every
  card or only on the hovered/active card is unspecified — hover state exists
  (`refresh_hover`, `terminal_app.rs:1169`), so both are reachable.
- **Resolved since first draft** (kept for traceability): menu z-order/hit-test → D10;
  last-tab close → D4; IME commit routing → D6.

### Found during verification (post-implementation, human-accepted)

- **R0.3's "or the shell exiting" and R2.5's dead-session clause are not implemented, and
  were never implementable here.** There is no child-exit detection anywhere in
  `crates/app` (no `try_wait` / `waitpid` / `.wait()`); a session's reader thread simply
  breaks on EOF (`session.rs:82`) and the tab stays in the roster. So "the field closes and
  discards when the target session dies" can only fire via an **explicit** close — Cmd+W or
  the menu's `Close` — both of which do broadcast `OverlayCommand::Cancel`. The tab-close
  half of both requirements is implemented and tested; the shell-exit half is **open**.
  Accepted as a documented gap rather than fixed: adding child-exit detection is new scope
  beyond T1–T6 and would need its own spec pass. Pre-existing — not a regression from this
  feature.

- **Amendment to R1.5's deviation note.** The implementation deviates from R1.5 in *two*
  ways, not one. (a) The chain resolves in `sidebar_model::fill` rather than
  `Tabs::titles` — necessary, since the AI-task label arrives on the renderer's per-frame
  `MetaSource` pull and resolving at row-assembly time would freeze the auto label and
  break R4.2. (b) `SidebarRow` **did** gain a `program` field, which R1.5's "the renderer
  gains no new source field" separately forbids — forced, because `SidebarMetaSource` holds
  only a `TabMetaHandle` and has no route to `Tabs`, so the program title can only reach the
  resolver by riding the row (symmetric with `label`, which already travelled that way).
  D7's *intent* is still satisfied more fully than the letter of R1.5: the renderer lost its
  precedence branch entirely.

## Requirements

### R0: A user-set tab title survives everything except the user and the session ending

#### R0.1: A named tab keeps its name across cwd, agent, and program title changes
- **Given**: Tab 2 has the user title `Frontend`, is in `~/proj`, has a running Claude task, and is about to run vim
- **When**: The user `cd ~/other`, the agent's task label changes, and vim emits `ESC]2;vim - main.rs BEL`
- **Then**: Tab 2's card line 1 reads `Frontend` throughout — no flicker to any other value at any point

#### R0.2: Only the user replaces a user title
- **Given**: Tab 2 has the user title `Frontend`
- **When**: The user renames it to `API`
- **Then**: The card reads `API`, and no subsequent program OSC 0/2 on that tab changes it

#### R0.3: A user title dies with its session
- **Given**: Tab 2 has the user title `Frontend`
- **When**: The tab is closed (menu `Close`, Cmd+W, or the shell exiting)
- **Then**: The title is gone with the `Session`; a newly created tab starts with no user title and shows its auto label

---

### R1: The label precedence chain (D3, D5, D7, D9, D12)

#### R1.1: `Session` holds the user title and the program title as distinct slots (producer)
- **Given**: `Session` today has a single `title: String` fed by the OSC drain (`session.rs:111`)
- **When**: The split lands
- **Then**: `Session` exposes `user_title: Option<String>` and `program_title: String`; the OSC 0/2 drain in `Session::drain` writes **only** `program_title` and never touches `user_title`

#### R1.2: The chain resolves user > AI-task > program-OSC > cwd (producer)
- **Given**: A tab with user title `Frontend`, task label `refactor the parser`, program title `vim - main.rs`, cwd `~/proj`
- **When**: The label is resolved
- **Then**: `Frontend`; removing the user title yields `refactor the parser`; removing the task label too yields `vim - main.rs`; removing that yields `proj`

#### R1.3: A program title is stored but not shown while a user title exists (producer)
- **Given**: Tab 2 has user title `Frontend`
- **When**: The shell emits `ESC]2;~/other BEL`
- **Then**: `program_title` becomes `~/other`, the resolved label stays `Frontend`, and no redraw is requested for a label that did not change

#### R1.4: Titles are mutated by `TabId`, never by active index (producer)
- **Given**: 3 tabs open, tab 1 active
- **When**: The user title of the tab with `TabId(3)` is set
- **Then**: Only that tab's label changes; the active tab and every other tab are untouched, and the result is identical whether or not the target is active

#### R1.5: The renderer receives one resolved label and keeps no precedence policy (consumer/boundary)
- **Given**: `Tabs::titles` resolves the chain per R1.2
- **When**: The `SidebarRow` model is pushed to the renderer
- **Then**: The row carries the already-resolved line-1 string; `sidebar_row.rs` gains no new source field and no new precedence branch, and `term-core` is unchanged (no parser/grid edits, no insta snapshot churn)

#### R1.6: Precedence is a pure, unit-tested function (producer)
- **Given**: The app crate has no `tests/` dir; pure fns are tested inline (`input.rs`, `copy.rs`)
- **When**: The chain is implemented
- **Then**: It is a free function over the four optional sources with inline `#[cfg(test)]` cases covering all 4 tiers, empty-string sources, and the all-absent case

---

### R2: The keyboard rename path (D1, D6, D8, D8a)

#### R2.1: Cmd+Shift+R opens a rename field for the active tab
- **Given**: Tab 2 is active with no rename field open
- **When**: The user presses Cmd+Shift+R
- **Then**: A rename field opens targeting tab 2, pre-filled with its current user title (empty if none), and the chord never reaches zsh

#### R2.2: Enter commits the typed name as the user title
- **Given**: The rename field is open on tab 2 with `Frontend` typed
- **When**: The user presses Enter
- **Then**: Tab 2's `user_title` becomes `Frontend`, the field closes, and the sidebar redraws immediately — including when tab 2 is not the active tab

#### R2.3: An empty commit clears the user title back to the auto label
- **Given**: Tab 2 has user title `Frontend` and the field is open with the text deleted (empty or whitespace-only)
- **When**: The user presses Enter
- **Then**: `user_title` becomes `None` and the card falls to the live auto label — the AI-task label if one exists, else the program title, else cwd (R1.2)

#### R2.4: Esc cancels without committing
- **Given**: The rename field is open on tab 2 with `Fron` typed, tab 2's title being `Frontend`
- **When**: The user presses Esc
- **Then**: The field closes, `user_title` is still `Frontend`, and nothing was written to any PTY

#### R2.5: A tab switch or a dead session discards an in-progress edit
- **Given**: The rename field is open on tab 2 with `Fron` typed
- **When**: The user presses Cmd+3 (or clicks another card), **or** tab 2's shell exits
- **Then**: The field closes discarding `Fron`, no title is committed, and — for the switch case — the switch still happens (the chord is not swallowed)

#### R2.6: IME commits route to the open field, not to the shell (producer/boundary)
- **Given**: The rename field is open and a Hangul IME is composing (`Ime::Commit` currently writes straight to a session PTY, `terminal_app.rs:364-430`)
- **When**: The user commits `프론트`
- **Then**: The text lands in the rename field, **zero bytes are written to any PTY**, and the composing gate (`suppress_key`, `input.rs:350`) does not swallow the field's input

#### R2.7: The open field owns the keyboard above every other overlay
- **Given**: The rename field is open and `AiMonitorOverlay` is registered at priority 100
- **When**: The user presses Cmd+Shift+M
- **Then**: The ai-monitor does **not** open; the field consumes the key, and no key it does not act on falls through to the shell

#### R2.8: PTY output keeps flowing behind the open field
- **Given**: The rename field is open on tab 2 and a build is spewing output on tab 1
- **When**: Output arrives on both tabs
- **Then**: Grids keep advancing and the active tab keeps redrawing normally — no buffering, no freeze, and the field stays open with its text intact

---

### R3: The mouse path — per-card hamburger menu (D2, D4, D10, D13)

#### R3.1: The renderer's ordered hit-test resolves the hamburger and menu items (consumer/boundary)
- **Given**: `sidebar_layout::hit_test` orders collapsed → outside → header → waiting band → cards (`sidebar_layout.rs:408-445`)
- **When**: The new regions are added
- **Then**: The per-card hamburger hit-box and, when a menu is open, its item rects resolve **before** step 4's `Card(i)` — a press on a menu item never returns `Card`

#### R3.2: Open-menu state reaches the layout as input, not as renderer-owned state (boundary)
- **Given**: `SidebarLayoutInput` carries no menu state (`sidebar_layout.rs:104-120`)
- **When**: The app opens a menu on row 2 with 3 items
- **Then**: The app pushes the open row **and its item count** in `SidebarLayoutInput` (as it already pushes `waiting_count`), and `hit_test` stays a pure function of its inputs

#### R3.3: `sidebar_press` maps the new regions to actions without geometry (producer)
- **Given**: `sidebar_press` is app-side and does no rect math (`input.rs:75`)
- **When**: A new region comes back from `hit_test`
- **Then**: It maps to a new `SidebarAction` variant; the app resolves the row index to a `TabId` via the `SidebarRow.tab_id` join (precedent `waiting_row_tab_index`, `terminal_app.rs:622`), and no coordinate arithmetic appears in the app crate

#### R3.4: The app can command an overlay (boundary)
- **Given**: `OverlayHost` has no app→overlay path (`seam.rs:222-267`)
- **When**: The user picks `Rename` from the menu
- **Then**: The app broadcasts an `OverlayCommand::BeginRename(TabId)` through a new `Overlay::on_command` (no-op default, mirroring `on_active_changed`), and the rename field opens targeting that tab — including when it is not the active tab

#### R3.5: Choosing a menu item closes the menu first — never two modals at once
- **Given**: The menu is open on tab 3
- **When**: The user clicks `Rename`
- **Then**: The menu closes and the rename field opens in the same event; at no point are both open, and Esc in the field returns to neither

#### R3.6: The menu's three items act on the clicked tab
- **Given**: The menu is open on tab 3 while tab 1 is active
- **When**: The user clicks `Rename` / `Reset to auto` / `Close`
- **Then**: Respectively — the field opens on tab 3; tab 3's `user_title` clears to `None`; tab 3 closes. `Reset to auto` is present only when tab 3 has a user title (so the menu has 2 or 3 items, R3.2)

#### R3.7: Closing a non-active tab keeps the active tab's identity
- **Given**: 3 tabs, tab 3 active, menu open on tab 1
- **When**: The user clicks `Close`
- **Then**: Tab 1 is gone, the still-active tab is the same session as before (its index shifts from 2 to 1), and closing the **last** remaining tab exits the app exactly as Cmd+W does (`false` return, `tabs.rs:88-97`)

#### R3.8: The menu dismisses on any outside input without acting
- **Given**: The menu is open on tab 3
- **When**: The user clicks outside it, presses Esc, or presses a tab chord
- **Then**: The menu closes, no item fires — and for the chord, the switch still happens (uniform with R2.5)

---

### R4: Redraw and lifetime plumbing (D9, constraints)

#### R4.1: An app-initiated title change redraws without the PTY wake path
- **Given**: `title_dirty` is set only inside `Session::drain`, on PTY output (`session.rs:108-116`)
- **When**: A rename commits or a reset clears a title
- **Then**: The path calls the tab-model resync + `request_redraw` directly and the new label appears at once — no reliance on the next PTY byte, and no new polling thread

#### R4.2: A cleared title re-follows the live auto label afterwards
- **Given**: Tab 2's user title was just cleared and its agent is still working
- **When**: The AI-task label subsequently changes
- **Then**: The card follows it live again, exactly as an never-named tab does

---

### R5: No regressions (D11, constraints)

#### R5.1: Card layout and the agent-state mark are unchanged
- **Given**: A tab is named `Frontend`
- **When**: Its card draws
- **Then**: Line 2 still shows repo/folder + git branch, card height is unchanged, and the agent-state mark still reflects running/idle/waiting — the identity/liveness split D11 accepts

#### R5.2: An un-named tab with no agent behaves as it does today
- **Given**: A plain shell tab, no user title, no AI task, zsh emitting OSC 2
- **When**: The card draws
- **Then**: It shows the program title, falling back to the cwd basename when there is none — the one un-named case D12 does not change

#### R5.3: The existing chords and clicks still work
- **Given**: No rename field or menu is open
- **When**: The user uses Cmd+C, Cmd+V, Cmd+T/W, Cmd+number, Cmd+Shift+M, card clicks, waiting-row clicks, and the collapse/restore buttons
- **Then**: All behave exactly as before — the new overlay and the new hit-test regions are inert when closed

## Tasks

### T1: Split the title model and make precedence a pure function [infra]
- **Fulfills**: R1.1, R1.2, R1.3, R1.5, R1.6, R4.2, R5.2
- **Depends on**: (none) — *parallel with T2*
- **Touches**: `crates/app/src/session.rs`, `crates/app/src/tabs.rs`
- Splits `Session.title` into `user_title` / `program_title`, points the OSC drain at
  `program_title` only, and lands the 4-tier chain as a free function with inline tests.
  No UI: the resolved string flows through the existing `SidebarRow.label`, so the
  precedence fix is observable on its own (an un-named tab with an agent stops showing
  zsh's title).

### T2: Menu geometry and the menu draw layer, renderer-side [infra]
- **Fulfills**: R3.1, R3.2 (renderer half)
- **Depends on**: (none) — *parallel with T1* (disjoint crates, no shared file)
- **Touches**: `crates/renderer/src/sidebar_layout.rs`, a new `crates/renderer/src/` menu
  layer module
- Adds the hamburger and menu-item `SidebarRegion` variants ordered ahead of `Card(i)`,
  extends `SidebarLayoutInput` with the open row + item count, and draws the popup above
  the sidebar layer. `hit_test` stays pure, so this is covered by the renderer's existing
  unit-test style.

### T3: Extend the seam in both directions [infra]
- **Fulfills**: R1.4, R3.4, R3.7
- **Depends on**: T1 (shares `tabs.rs`/`session.rs`, and needs the title slots to exist)
- **Touches**: `crates/app/src/seam.rs`, `crates/app/src/tabs.rs`,
  `crates/app/src/terminal_app.rs`
- Overlay→app: `AppCtx::set_user_title(TabId, …)` and `close(TabId)`, plus
  `Tabs::close(TabId)` preserving `active_after_close` semantics and the
  `false`-on-last-tab return. App→overlay: `OverlayCommand` + `Overlay::on_command`
  (no-op default) and the `OverlayHost` broadcast.

### T4: The keyboard rename path, end to end [vertical]
- **Fulfills**: R0.1, R0.2, R0.3, R2.1, R2.2, R2.3, R2.4, R2.5, R2.7, R2.8, R4.1, R5.1, R5.3
- **Depends on**: T3
- **Touches**: new `crates/app/src/rename.rs`, new renderer field layer,
  `crates/app/src/main.rs` (registration), `crates/app/src/terminal_app.rs`
- The rename overlay: Cmd+Shift+R, the field's state machine (type / Enter commits /
  empty commits a clear / Esc cancels / switch cancels / dead session closes), priority
  above ai-monitor with consume-all-keys while visible, and the direct resync +
  `request_redraw` that does **not** ride the PTY wake path. This is the first slice that
  delivers the goal — sticky titles work by keyboard alone after it.

### T5: Route IME commits to a capturing overlay [vertical]
- **Fulfills**: R2.6
- **Depends on**: T4 (needs a field to route to; also shares `terminal_app.rs`)
- **Touches**: `crates/app/src/terminal_app.rs` (`on_ime`), `crates/app/src/input.rs`
  (`suppress_key`), `crates/app/src/seam.rs`
- Makes `Ime::Commit`/`Ime::Preedit` consult the visible capturing overlay before
  `composing_tab`'s PTY, and stops the composing gate from swallowing the field's input.
  Fixes a real defect, not just a new-feature gap.

### T6: The hamburger menu, end to end [vertical]
- **Fulfills**: R3.3, R3.5, R3.6, R3.8, R5.1, R5.3 (and completes R3.1/R3.2's app half)
- **Depends on**: T2, T3, T4, **T5** — the T5 edge is a file-overlap serialization, not a
  logical one: both edit `crates/app/src/terminal_app.rs`, so they cannot run concurrently
- **Touches**: `crates/app/src/input.rs` (`sidebar_press`),
  `crates/app/src/terminal_app.rs` (`SidebarAction` arms, open-menu state)
- Maps the new regions to new `SidebarAction` variants, resolves row index → `TabId`
  app-side, owns the open/closed state and pushes it as layout input, and wires the three
  items — `Rename` (menu closes, field opens via T3's command), `Reset to auto`,
  `Close` — plus dismiss-without-acting.

**DAG**: `(T1 ∥ T2) → T3 → T4 → T5 → T6`. The chain is largely forced by
`terminal_app.rs` being the app's hub file; T1/T2 are the only genuinely independent
pair, and no false dependencies were added.

## External Dependencies

### Pre-work
- (none) — no new crates. Everything is drawn by hand on the existing render stack
  (`no-gui-crates`), and the toolchain is pinned.

### Post-work
- **Manual Hangul IME verification (R2.6).** IME composition cannot be exercised
  headlessly or by `cargo test`; it needs a human at the keyboard with a Korean input
  source, via `/run-term`. This is the one acceptance criterion no automated gate can
  close.
- **Manual eyeball of the menu popup (R3.1/R3.5).** Hit-test correctness is unit-testable;
  that the popup lands where it looks like it lands is not.

