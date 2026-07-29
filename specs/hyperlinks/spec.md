# Spec: hyperlinks

## Meta
- **Created**: 2026-07-29
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-07-29

## Goal
Add clickable hyperlinks: when the terminal shows a URL (e.g. the pull-request
link `gh pr create` prints), clicking it opens that site in the default browser.

## Non-goals
- No built-in/embedded browser — the URL is handed to macOS and that is the end
  of our responsibility.
- No link editing, no link list/picker/palette UI, no "copy link address" menu.
- No new remote/network behavior in the terminal itself (`zsh-local-only`).
- Not a general mouse-reporting feature (forwarding button presses to TUI apps is
  a separate concern; see Research).

## Confirmed Goal
Detect URLs in the visible grid, show a hover affordance when the pointer is over
one, and open the URL in the user's default browser on a click gesture that
cannot be confused with starting a text selection. Done when a line containing
`https://github.com/owner/repo/pull/123` can be clicked to land on that page, and
normal drag-selection never fires a browser tab.

## Research

### Where a click is handled today
- Left press/release live in one `window_event` match; a press first runs the
  sidebar's single ordered `hit_test`, and only a press that resolves to **no**
  sidebar action and lands on a real grid cell arms a drag
  (`crates/app/src/terminal_app.rs:1701-1784`). Every grid press also runs
  `press_dismisses_modals`, which closes the card menu and cancels an in-flight
  rename (`crates/app/src/terminal_app.rs:1716-1728`).
- Release with `!drag_moved` (pointer never left the anchor cell) is exactly the
  "plain click" case — today it just clears the selection
  (`crates/app/src/terminal_app.rs:1786-1809`). This is the natural insertion
  point for "open the link under the cursor".
- Pixel→cell mapping already exists and already accounts for the sidebar/left and
  top insets: `TerminalApp::cursor_cell` → `pixel_to_cell`
  (`crates/app/src/terminal_app.rs:1244-1254`).
- `ModifiersChanged` keeps live modifier state in `inner.mods`
  (`crates/app/src/terminal_app.rs:1672-1674`), so a ⌘-gated click is available
  with no new plumbing.
- `CursorMoved` already fires per motion and calls `refresh_hover()` (currently a
  *sidebar*-only hover) (`crates/app/src/terminal_app.rs:1676-1679`); a grid-side
  hover would hook the same event.
- `CursorLeft` clears hover (`crates/app/src/terminal_app.rs:1699`).
- The app keeps the snapshot that is actually **on screen** in
  `Running::last_snapshot` (`crates/app/src/terminal_app.rs:195`), re-cloned only
  when `snapshot_dirty` is set. `redraw()` sets that flag; the self-perpetuating
  continuous `window.request_redraw()` deliberately does not
  (`crates/app/src/terminal_app.rs:360-371`). Reading `last_snapshot` is free;
  calling `terminal.snapshot()` (`crates/term-core/src/lib.rs:61`) clones the whole
  `cols × rows` cell vector.

### Text the click must resolve against
- `GridSnapshot` is the renderer/app-facing contract: `cell(col,row)`, `cells()`,
  `to_text()`, `selection_text()`, plus `view_offset`/`at_bottom`
  (`crates/term-core/src/snapshot.rs:12-186`). Rows are composed from history +
  live for the current `view_offset`, so a URL scan over the snapshot works while
  scrolled back too.
- Row-text reconstruction rules already exist and must be reused for URL scanning:
  double-width continuation cells are skipped, and a row is treated as
  soft-wrapped into the next iff its last column is non-blank/non-continuation
  (`GridSnapshot::selection_text` + `row_soft_wraps`,
  `crates/term-core/src/snapshot.rs:127-183`). A URL wrapped across the right edge
  is therefore a real, already-modeled case.
- `Cell` is `{ch, fg, bg, attrs}` (`crates/term-core/src/grid.rs:92-97`) with
  `CellAttrs` a packed `u8`, 5 bits used (`crates/term-core/src/grid.rs:63-88`). There is **no** per-cell id field — an
  OSC 8 explicit-hyperlink implementation would have to widen `Cell` (paid across
  the whole scrollback), whereas plain-text detection needs no per-cell state.
- `Selection` is a viewport `(col,row)` anchor/focus pair with `normalized()` and
  a linear-span `contains()` (`crates/term-core/src/selection.rs:11-54`) — the
  shape a "link span" can copy.

### Rendering a hover affordance
- `DrawLayer { z_order, record }` is the extension seam
  (`crates/renderer/src/lib.rs:148-153`). Layers are registered **once at startup**
  via `Renderer::add_layer` (`crates/renderer/src/lib.rs:491`, called from
  `crates/app/src/terminal_app.rs:264-286`); each registered layer's `record()` then
  runs every frame. `OverlayHost::layers()` (`crates/app/src/seam.rs:338`) has no
  production caller — it is exercised only by a unit test (`seam.rs:623`). The
  overlay seam's contribution is one `o.layer()` box drained at startup
  (`crates/app/src/terminal_app.rs:286`).
- `selection_layer` is the closest precedent for the *drawing*: a per-cell
  translucent quad layer built by a pure, GPU-free `selection_instances` function,
  z=4 between glyphs and cursor (`crates/renderer/src/selection_layer.rs:1-60`).
  But it is stateless-by-construction — it reads its span out of
  `FrameCtx.snapshot.selection`. `SelectionPresenter`
  (`crates/app/src/copy.rs:76-95`) and `ImagePresenter`
  (`crates/app/src/clipboard.rs:160-179`) work for the same reason.
- For state that is **not** on the snapshot, the in-tree pattern is
  `PreeditLayer::new() → (layer, handle)`: a shared `Rc<RefCell<…>>` model the app
  writes and the layer reads, constructed and seeded in `terminal_app.rs` before
  `add_layer` (`crates/app/src/terminal_app.rs:270-276`) precisely because
  `FrameCtx` carries no such data. Hover-link state needs this shape, not
  `SelectionPresenter`'s.
- **Caching hazard**: the renderer skips re-shaping when the new snapshot `==` the
  cached `last_snapshot` (`crates/renderer/src/lib.rs:740-772`, the idle-CPU fix).
  Hover state that lives outside the snapshot must therefore drive its own layer
  (which re-records every frame) and must force a redraw itself — mirroring how
  `preedit`/`cursor` layers already sit outside the snapshot cache.
- `CellAttrs::UNDERLINE` already exists and renders
  (`crates/term-core/src/grid.rs:70`), but it is a *cell content* attribute owned
  by the VT parser — hover is transient app state, not grid state.

### Opening the URL on macOS
- `objc2` / `objc2-app-kit` are already dependencies of `app`
  (`crates/app/Cargo.toml`), and the codebase already calls AppKit directly:
  `NSPasteboard::generalPasteboard()` for copy (`crates/app/src/copy.rs:29-36`)
  and for image paste (`crates/app/src/clipboard.rs:17-24`). `NSWorkspace openURL:`
  is the same shape of call — no new crate, and the `no-gui-crates` rule is
  untouched.
- The existing precedent also shows the testability pattern: the side effect goes
  behind an injectable sink (`CopyHandler::sink`, `crates/app/src/copy.rs:38-54`)
  so the decision logic is unit-testable without performing the real effect.

### Constraints found in the tree
- No `regex` crate anywhere in the workspace (`Cargo.toml`,
  `crates/*/Cargo.toml`); `term-core` deps are deliberately small (`vte`,
  `portable-pty`, `unicode-width`, `image`, `base64`). URL detection either
  hand-rolls a scanner or introduces the first regex dependency.
- OSC dispatch exists and is deliberately narrow: OSC 0/2 (title) and OSC 133
  (shell integration) only; **everything else is explicitly ignored**
  (`crates/term-core/src/parser.rs:362-398`). OSC 8 would be an additive branch
  there, plus per-cell link identity (see `Cell` above).
- Mouse *protocol* state is already parsed and tracked in the grid
  (`?9/?1000/?1002/?1003` → `MouseProtocol`, `crates/term-core/src/grid.rs:261-967`),
  but is **not** plumbed into the app's mouse handling — the app currently reports
  wheel only, never button presses. So a link click cannot today collide with a
  TUI app's own click handling, but the mode flag is available if we want to
  defer to it.
- `term-core` must stay GPU/window-free (`dependency-direction` rule); any URL
  scanning that lives in `term-core` must be a pure function over the snapshot.
- Build/test/lint gates: `cargo build`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`; `insta` snapshots for
  grid/parser changes (`CLAUDE.md`).

## Decisions

### D1: ⌘-click opens a link; plain click and drag keep their current meaning
- **Status**: resolved
- **Rationale**: The open gesture must be impossible to confuse with starting a
  selection, which a plain click already means (`terminal_app.rs:1788-1800`).
  ⌘-click is the macOS-wide convention (Terminal.app, iTerm2, VS Code). Rejected:
  *plain click opens* (every click on link text would launch a browser, including
  one meant to place or clear a selection — the exact false-fire the goal rules
  out); *double-click opens* (double-click on words is widely expected to mean
  word-select, and the sidebar already binds double-click to rename).

### D2: Only `http://` and `https://` are ever detected — a strict scheme allow-list
- **Status**: resolved
- **Rationale**: Covers the motivating PR-link case with the smallest blast
  radius. A scheme outside the list is not merely blocked at open time — it is
  **never detected as a link**, so nothing off-list can reach the OS. Rejected:
  `file://` (turns build output into a launcher for arbitrary local paths, e.g.
  `file:///Users/me/.ssh/id_rsa`); `mailto:`/`ssh://`/`vscode://` (hands hostile
  output to whatever app claims the scheme); bare `www.` (no scheme means guessing,
  and it multiplies false positives in ordinary prose).

### D3: The authority must be ASCII and must carry no userinfo
- **Status**: resolved (from the Inversion Probe + L2-reviewer rounds)
- **Definition (so the rule is testable)**: the **authority** is the characters after
  `://` up to — exclusive — the first `/`, `?`, or `#`, or the end of the run,
  whichever comes first.
- **The rule**: a candidate run is **rejected outright** (yields no link at all) if
  (a) its authority contains `@`, or (b) the run was terminated by a non-ASCII or
  C0/DEL byte *while still inside the authority*.
- **Rationale**: The terminal renders whatever bytes a command emits, so hostile
  output can paint `https://gıthub.com/…` (dotless ı) that reads as GitHub and
  passes any scheme check. Note that D23's truncation alone is **not** a defense
  here — it would end the run at `ı` and leave `https://g` as a perfectly clickable
  link. Clause (b) is what turns that truncation into a rejection, so a confusable
  host yields nothing rather than a plausible fragment. ASCII is not enough either:
  `https://github.com@evil.com/pull/1` is pure ASCII and reads as GitHub in a
  terminal font — clause (a) rejects it, while `https://example.com/a@b` keeps
  working because its `@` falls after the authority ended. Rejected:
  punycode-normalize and compare (needs an IDNA dependency and a confusables table
  in a terminal, not a browser); allowing userinfo and trusting the browser's
  address bar to reveal the real host (too late — the tab is already open);
  legitimate `user@host` URLs are vanishingly rare in shell output, so the cost of
  rejecting them is near zero.

### D4: Trailing punctuation is trimmed, with a matched-opener exception
- **Status**: resolved
- **Rationale**: Shell and prose output routinely writes `(see https://…/pull/12).`
  Trailing `.` `,` `;` `:` `!` `?` `"` `'` are stripped unconditionally; trailing
  `)` `]` `}` `>` are stripped **unless** the matching opener occurs earlier in the
  URL — so `…/pull/12` opens while `…/wiki/Foo_(bar)` keeps its paren. Trimming
  **iterates** until no trailing char qualifies, so `…/pull/12).` loses both. This
  is the rule terminals have converged on. Rejected: *stop at
  whitespace only* (simplest, but `…/pull/12).` is opened verbatim and 404s — the
  single most common real-world case).

### D5: A URL that soft-wraps across the right edge is rejoined and clickable
- **Status**: resolved
- **Rationale**: Narrow windows wrap long PR URLs constantly; a URL you can see is
  a URL you should be able to click. Reuses the heuristic already shipped for
  copy: a row continues into the next iff its last column holds a non-blank,
  non-continuation cell (`snapshot.rs:174-183`). Both halves highlight and both are
  clickable. Rejected: *per-row detection only* (safer against stitching unrelated
  text, but silently kills the feature at exactly the window widths where it is
  most needed).
  The heuristic's known limits (a full-width hard line, a CJK margin wrap) are
  **not** merely cosmetic here as they are for copy: a mis-stitch would fabricate a
  URL out of two unrelated lines. What makes that acceptable is D20 — the stitched
  span is exactly what gets underlined, so a mis-stitch is *visible before the
  click*, never a hidden target. This is why spoof-by-stitching is tolerable where
  spoof-by-OSC-8 (D6) is not: OSC 8's target is unrenderable by construction.

### D6: OSC 8 explicit hyperlinks are out of scope
- **Status**: resolved
- **Rationale**: `git`, `gh`, and CI output print raw URLs, so plain-text detection
  covers the goal. OSC 8 would require per-cell link identity — `Cell` is
  `{ch, fg, bg, attrs:u8}` with no id field (`grid.rs:63-97`), so it would have to
  widen and pay that across the entire scrollback — and it introduces a
  displayed-text ≠ target spoofing surface that D2/D3 exist to avoid. Consistent
  with the parser, which deliberately ignores every OSC but 0/2/133
  (`parser.rs:376-378`). Rejected: parsing OSC 8 now (cost and risk far exceed the
  motivating use case; revisitable as its own spec).

### D7: The affordance appears only while ⌘ is held
- **Status**: resolved
- **Rationale**: The underline must be honest — it should appear exactly when a
  click would open, not while a click would only select. ⌘ down + pointer over a
  link → that link's cells underline/tint; ⌘ up → it vanishes. Requires recomputing
  hover on `ModifiersChanged` (`terminal_app.rs:1672-1674`), not just on
  `CursorMoved`. Rejected: *always underline on hover* (more discoverable, but it
  advertises a click that would not open); *no visual change at all* (cheapest, but
  nothing shows where the link ends or that one was detected).

### D8: Links work anywhere on the visible screen, including scrolled-back history
- **Status**: resolved
- **Rationale**: Scrolling back to find an earlier PR link is the likeliest real
  use. `GridSnapshot` already composes its rows from history + live for the current
  `view_offset` (`snapshot.rs:36-60`), so detection over the snapshot works
  unchanged while scrolled. **Deliberately diverges** from `selection` and `images`,
  which are live-bottom-only (`snapshot.rs:62-72`) — those are live-viewport
  concepts; a link is just text that is on screen.

### D9: ⌘+drag still selects; opening fires only on a press+release that never moved
- **Status**: resolved
- **Rationale**: Reuses the `drag_moved` test that already separates click from
  drag (`terminal_app.rs:1788-1795`), so ⌘ adds a gesture without removing one.
  Rejected: *⌘ suppresses selection entirely* (cleaner mode separation, but it
  makes ⌘+drag a dead gesture and would need a second code path for the
  drag-anchor arming).

### D10: A failed open is logged and otherwise silent
- **Status**: resolved
- **Rationale**: Matches the project's graceful-degradation rule (shell integration
  degrades silently, `shell_integration.rs:10-12`). A `tracing` warn, no screen
  change, nothing written to the PTY. Rejected: *visible flash/red link* (invents
  new render state and a timing concern for a rare, non-destructive failure).

### D11: The scan is lazy and app-side over `last_snapshot`; `GridSnapshot` gains no link field
- **Status**: resolved
- **Rationale**: Idle CPU is a known past regression here — the renderer skips
  re-shaping when the snapshot compares equal (`lib.rs:740-772`), and a `links`
  field would join that equality and be recomputed on every PTY burst. Instead the
  app scans **`Running::last_snapshot`** — the snapshot actually on screen
  (`terminal_app.rs:195`), so the scan is free of any clone and can never hit-test
  rows the user is not looking at. `terminal.snapshot()` is explicitly *not* used:
  it clones the whole cell vector per call. When `last_snapshot` is `None` (first
  frame) there is nothing on screen to hit-test and the scan yields no links.
- **Steelman (the real argument against, and the answer)**: the strongest case for
  carrying links on the snapshot is not performance, it is *consistency* — the
  highlight and the click target would provably derive from one immutable value,
  and recompute would already be gated by `snapshot_dirty`. Two concrete costs of
  going lazy follow, and both are answered explicitly rather than waved off:
  1. **TOCTOU.** Spans found at ⌘-down go stale when a PTY burst scrolls the
     viewport, since that raises neither `CursorMoved` nor `ModifiersChanged`. →
     D24 adds a fresh-snapshot edge trigger so the highlight cannot go stale, and
     D19 opens only what is lit at the instant of release. (Note: *re-resolving at
     release independently of the highlight* is explicitly **rejected** by D19 — it
     would reintroduce opened ≠ underlined.)
  2. **Motion rate.** Pointer motion fires at display rate, so a ⌘-held scan can run
     *more* often than the snapshot changes — inverting D11's own perf premise. →
     D21 bounds each scan to the hovered row plus its wrap neighbors, not the full
     grid, and gates the scan on ⌘-down **and** the pointer being over a grid cell.
- **Rejected**: *carry links on the snapshot* (one shared source of truth, but every
  snapshot pays a full-grid scan for a feature used seconds at a time, and the
  field joins snapshot equality — reopening exactly the idle-CPU debt that was
  already paid down once).

### D12: Detection is a pure, GPU-free function over `GridSnapshot`, living in `term-core`
- **Status**: assumed (L1 auto-resolved; signature fixed by D25)
- **Rationale**: `dependency-direction` requires the engine to stay headless-
  testable; a pure function of a snapshot is unit-testable with no window, no GPU,
  and no PTY — the same shape as `selection_text`. The exact signature is **not**
  the whole-grid `snapshot → Vec<LinkSpan>` this decision first assumed; D25 fixes it
  as a point resolver, which is what D21's bounded scan and D20's same-path
  invariant both require. Rejected: putting detection in `app` (it would be testable
  only through the event loop).

### D13: A link is represented as a viewport cell span, shaped like `Selection`
- **Status**: assumed (L1 auto-resolved)
- **Rationale**: `Selection` already models a linear `(col,row)` span with
  `normalized()` and a `contains()` membership test that never panics on inverted
  or edge coordinates (`selection.rs:11-54`) — exactly what a possibly-multi-row
  link needs for hit-testing and for highlighting. Reuse the shape rather than
  inventing a second span type.

### D14: The URL is opened via AppKit (`NSWorkspace`) behind an injectable sink
- **Status**: assumed (L1 auto-resolved)
- **Rationale**: `objc2` / `objc2-app-kit` are already `app` dependencies and the
  codebase already calls AppKit directly for the pasteboard (`copy.rs:29-36`). The
  side effect goes behind a `Box<dyn Fn(&str)>` sink exactly like `CopyHandler`
  (`copy.rs:38-54`), so the resolve/open decision is unit-testable without opening a
  browser during `cargo test`. Rejected: shelling out to `/usr/bin/open` (a process
  spawn per click, and no typed failure result); any new crate (`no-gui-crates`,
  and AppKit is already in the tree).

### D15: The affordance is a `DrawLayer` + shared handle, on the `PreeditLayer` pattern
- **Status**: resolved (corrected in the L2-reviewer round)
- **Rationale**: The *drawing* copies `selection_layer` — a per-cell quad layer
  built by a pure, GPU-free instance function (`selection_layer.rs:1-60`). The
  *state plumbing* must not: `SelectionLayer` works only because its span rides on
  `FrameCtx.snapshot`, and D11 keeps hover state off the snapshot. Layers are
  registered once at startup (`terminal_app.rs:286`), so a stateless
  `Overlay::layer()` box has no path to app-side state at all. The correct in-tree
  precedent is therefore `PreeditLayer::new() → (layer, handle)` with an
  `Rc<RefCell<…>>` model the app writes and the layer reads
  (`terminal_app.rs:270-276`). The model is plain data — the highlighted spans plus
  the underline color — so it stays unit-testable without a GPU.
- **Cost admitted**: this *does* mean `terminal_app.rs` is edited (the handle is
  constructed and held there, next to `CursorLayer`/`PreeditLayer`) — the earlier
  "registered without touching `terminal_app.rs`" claim was false, and is dropped.
  `terminal_app.rs` owns pointer events, so it has to hold the handle regardless.
- **Rejected**: a passthrough `Overlay` presenter à la `SelectionPresenter`
  (structurally appealing, but there is no state channel — it would work only if
  links rode on the snapshot, i.e. only by reversing D11).

### D16: URL matching is a hand-rolled scanner — no `regex` dependency
- **Status**: assumed (L1 auto-resolved)
- **Rationale**: With D2 (two literal scheme prefixes), D3 (ASCII host) and D4
  (character-class trimming), the match is a linear scan over a row's chars — a
  regex buys nothing and would be the workspace's first, added to a tree that
  deliberately carries five small `term-core` deps. Rejected: `regex` /
  `linkify` (dependency weight and compile time for a ~60-line scanner).

### D17: Mouse-reporting apps do not suppress ⌘-click
- **Status**: resolved
- **Rationale**: ⌘-click is a terminal-level chord and is never forwarded to the
  child. The app does not forward button presses at all today (only wheel), so the
  gesture cannot collide with vim/lazygit; and links staying live inside a
  full-screen TUI is the more useful behavior. The grid already tracks
  `MouseProtocol` (`grid.rs:261-967`) if a future button-forwarding feature needs to
  revisit this. Rejected: *suppress while reporting is on* (reserves the gesture for
  a feature that does not exist, at the cost of dead links inside every TUI).

### D18: No always-on hint and no pointer-cursor change; ⌘ is the convention
- **Status**: resolved
- **Rationale**: The user explicitly accepted that nothing on screen advertises
  clickable URLs until ⌘ is pressed — ⌘-click-to-open is a habit users already
  bring from Terminal.app/iTerm2/VS Code. Rejected: swapping to the pointing-hand
  cursor while ⌘ is held (one `winit` call, but still invisible before ⌘ and adds a
  cursor-state machine to restore correctly on every exit path).

### D19: One rule governs both the highlight and the open — "open what is lit"
- **Status**: resolved (unified in L2-reviewer round 2)
- **Rationale**: The first draft of this decision ("re-resolve the URL at release")
  contradicted D20 ("what is underlined is what opens"): between press and release,
  output can move the text, so a re-resolve could open something that was never
  underlined. Both intents survive under a single rule:
  1. The **highlight is always current** — recomputed on every event that can change
     what is under the pointer (D24), and cleared the instant ⌘ goes up (D7).
  2. On release, the terminal opens **iff a currently-highlighted span is under the
     pointer**, and it opens exactly that span's text.
  With (1) holding, "re-resolve at release" and "open what is lit" are the same
  operation, which is why this is one decision rather than two competing ones. The
  user's stated intent — *never open a URL you were not pointing at* — is preserved
  literally: if output scrolled the link away, the highlight moved with it, and a
  release over nothing lit opens nothing.
- **Rejected**: *open the span captured at ⌘-down* (output moves under the pointer
  with no pointer or modifier event to invalidate it — the original TOCTOU);
  *re-resolve at release independent of the highlight* (reintroduces the
  opened ≠ underlined case D20 exists to forbid).

### D20: What is underlined is exactly what opens — a testable invariant
- **Status**: resolved (reversing the Inversion Probe's earlier "leave it a gap")
- **Rationale**: Two claims, both asserted in tests:
  - **Same-path**: the highlight and the opened string come from one function — the
    same match, the same D4 trim, the same D5 rejoin. There is no second code path
    that could drift.
  - **Same-instant**: per D19 the thing opened *is* the lit span, so the property is
    not merely "equal by construction at some instant" — it holds at the moment of
    the open.
  This is what makes the residual spoof surfaces safe: a mis-stitched wrap (D5) or
  an over-eager trim (D4) becomes something the user *sees highlighted before
  clicking*, never a hidden target. Rejected: leaving it documented-only (a later
  change to D4/D5 could diverge the two with nothing failing).

### D21: Each scan is bounded to the hovered row and its soft-wrapped run
- **Status**: assumed (derived in the L2-reviewer round)
- **Rationale**: Pointer motion fires at display rate, so a full-grid scan per
  motion event could run more often than the snapshot itself changes. Only the row
  under the pointer can contain the hovered link; D5 extends that to the contiguous
  soft-wrapped run it belongs to. Scans are edge-triggered, not level-triggered
  (D24), and a scan is skipped entirely when `cursor_cell()` yields `None` (pointer
  off the grid).
- **Honest limit**: this does **not** mean the app's other ⌘ chords (Cmd+C/T/W/V/1-9)
  never scan. `cursor_cell()` returns `Some` for any resting pointer over the grid,
  so a Cmd chord typed with the pointer over the grid does cost one ⌘-down-edge
  scan of one row run — and if that row holds a URL it also lights up, which under
  D7 is the correct and intended behavior (⌘ is down; a ⌘-click there *would* open).
  The bound that matters is one row run per edge, not zero scans.

### D22: ⌘ must be held at press **and** at release
- **Status**: resolved (extended in L2-reviewer round 2)
- **Rationale**: "⌘-click" is otherwise undefined in time. Requiring ⌘ at **press**
  stops a ⌘ pressed *after* an ordinary click from opening a browser — the reading
  that matches D1 ("cannot be confused with starting a selection"). Requiring it
  again at **release** falls out of D19: ⌘-up clears the highlight (D7), and D19
  opens only what is lit, so releasing ⌘ mid-gesture cancels the open. Without this,
  the user would watch the underline vanish and a browser open anyway.
  Everything else about the click is unchanged: `press_dismisses_modals` still runs
  (so an opening click also dismisses an open card menu / rename field, exactly as
  a plain grid press does today, `terminal_app.rs:1716-1728`), and the `!drag_moved`
  release still clears the selection.

### D23: A URL run is ASCII-only and ends at whitespace, C0/DEL, or any non-ASCII byte
- **Status**: assumed (derived in the L2-reviewer round)
- **Rationale**: D16's hand-rolled scanner *is* the character class, so it has to be
  stated or L3 cannot write a scan test. Terminating on any non-ASCII char makes D17
  (links stay live inside TUIs) behave: a URL abutting a `│` panel border in lazygit
  ends cleanly at the border instead of swallowing it. Terminating on C0/DEL keeps
  stray control bytes out of an opened URL. **Truncation is not by itself a defense**
  — see D3, which rejects the candidate outright when the cut lands inside the
  authority, so `https://gıthub.com/…` yields no link rather than the clickable
  fragment `https://g`.

### D24: The highlight is edge-triggered by five events, including a new snapshot
- **Status**: resolved (added in L2-reviewer round 2; fifth trigger added in round 3)
- **Rationale**: D19 requires the highlight to be current; that is only true if every
  event which can change what is under the pointer, or what ⌘ is doing, recomputes
  it. There are five:
  1. **⌘-down edge** (`ModifiersChanged`) — the affordance appears.
  2. **⌘-up edge** (`ModifiersChanged`) — the highlight clears; no scan.
  3. **Pointer enters a different cell** (`CursorMoved`, keyed on the *cell* like the
     existing drag-jitter guard) — plus `CursorLeft`, which clears.
  4. **A fresh snapshot was taken while ⌘ is held** — the trigger the first revision
     named but never hooked. It lives in the `RedrawRequested` arm, on the branch
     that already re-clones under `snapshot_dirty` (`terminal_app.rs:1895-1897`), not
     on every frame: the loop redraws continuously for cursor blink
     (`terminal_app.rs:1918-1920`), and an idle frame re-presents the cached snapshot
     and must not scan.
  5. **Focus loss** (`WindowEvent::Focused(false)`, `terminal_app.rs:1859`) — treated
     as a ⌘-up edge: the highlight clears and the link gesture disarms. Without this
     the feature can violate its own Constraint. macOS delivers `flagsChanged` only
     to the focused window, so a ⌘-Tab away while ⌘ is held leaves `inner.mods` stuck
     at `SUPER` with the highlight lit — and on return, a genuinely **plain** click
     would satisfy both D22 (⌘ at press) and D19 (a lit span under the pointer) and
     open a browser. The tree already documents this exact quirk class at
     `Focused(false)` ("a Cmd-Tab away fires no `CursorLeft`").
  Trigger 4 is the explicit carve-out to the "no per-snapshot scan" constraint: it is
  bounded by ⌘ being held (seconds at a time, user-initiated) **and** by D21's one-row
  bound, so the streaming-output cost the constraint protects is untouched.
- **Rejected**: leaving trigger 4 out (the highlight silently goes stale under output
  and, via D19, the click then opens nothing when the user can plainly see a lit
  link — reopening the TOCTOU from the other side).

### D25: `term_core::links` exposes one point resolver, not a whole-grid scan
- **Status**: resolved (reconciled in L2-reviewer round 2)
- **Rationale**: D12 first said `snapshot → Vec<LinkSpan>` while D21 bounded scanning
  to the hovered row — two incompatible signatures. The bounded one wins, and one
  function serves both callers (which is also how D20's same-path claim is
  mechanically guaranteed):
  `pub fn link_at(snap: &GridSnapshot, col: u16, row: u16) -> Option<LinkSpan>`
  — pure, GPU-free, `None` when the cell is not inside a link. `LinkSpan` carries the
  cell span (D13 shape) **and** the resolved URL string, so the highlight and the
  open read the same value. No whole-grid `links()` is specified; nothing needs it.
- **Mechanical follow-on**: `GridSnapshot::row_soft_wraps` is currently private
  (`snapshot.rs:174`); a sibling `links` module in `term-core` needs it `pub(crate)`.

## Constraints

- `term-core` must not gain a GPU/window dependency; detection is a pure function
  over `GridSnapshot` (`dependency-direction` rule).
- No GUI/widget crate may be added; the affordance is drawn on the cell grid
  (`no-gui-crates` rule).
- No new `#[cfg(target_os = …)]` branches — macOS ARM64 only (`macos-only` rule).
- Nothing about links may write to the PTY or open a network connection from the
  terminal process (`zsh-local-only`); the URL is handed to the OS and that is all.
- The existing plain click, drag-select, and Cmd+C copy behavior must be
  byte-for-byte unchanged when ⌘ is not held.
- Link handling must run **after** the sidebar's single ordered `hit_test` and only
  on a grid-region fallthrough — a ⌘-click on a tab card, the card menu, or the
  rename field keeps its current meaning (`terminal_app.rs:1701-1784`).
- Idle CPU must not regress: **no per-frame scan**, and no scan at all while ⌘ is
  not held. Scans are edge-triggered only (D24's four events), never
  level-triggered. The one snapshot-driven trigger (D24.4) is explicitly permitted
  and is bounded twice over — by ⌘ being held and by D21's single row run — so
  streaming output with ⌘ up costs nothing.
- The renderer's snapshot-equality cache must stay valid (`lib.rs:740-772`): a
  ⌘-driven highlight change must go through the bare `window.request_redraw()` and
  must **not** set `snapshot_dirty` (`terminal_app.rs:360-371`) — otherwise every ⌘
  tap re-clones the grid and invalidates the cache this constraint exists to protect.
- No `unwrap()` outside tests; per-crate `thiserror` enums (`CLAUDE.md`).
- `cargo clippy --workspace --all-targets -- -D warnings` must stay clean.

## Known Gaps

- No numeric per-scan performance budget is stated. D21 bounds the scan
  structurally (hovered row + its wrap run, gated on ⌘ + a real grid cell), which
  is the substantive guard; a millisecond figure is not committed, consistent with
  `docs/domain.md` invariant 4 treating perf numbers as goals until locked.
- OSC 8 (D6) and non-http schemes (D2) are deferred, each revisitable as its own
  spec.
- Homoglyphs *within the path or query* are not analyzed — only the authority is
  ASCII-restricted and userinfo-rejected (D3). A confusable path segment can still
  render misleadingly; D20 means it is at least visible before the click.
- D5 inherits the `row_soft_wraps` heuristic's known limits (a full-width hard
  line, a CJK margin wrap). D20 makes a mis-stitch visible rather than hidden, but
  it does not prevent one.
- Whether a link should be openable from the keyboard (no pointer) is not
  considered; this feature is pointer-only.

### Carried to L3 by the L2 reviewer (requirements, not open decisions)

- **Pointer-inside tracking**: `last_cursor` is written only by `CursorMoved`, and
  there is no `CursorEntered` handler, so after `CursorLeft` `cursor_cell()` still
  returns a stale `Some`. L3 must add an explicit `pointer_inside` flag (false on
  `CursorLeft`, true on `CursorMoved`) and gate both the scan and the open on it —
  otherwise a ⌘-down edge can light a link under an absent pointer.
- **Non-empty authority**: D3 defines the authority but does not require it to be
  non-empty; `https://` and `https:///path` must not become links.
- **One span, not many**: pin D15's handle model to `Option<LinkSpan>` to match D25.
- **The visual is unspecified**: whether D7's affordance is an underline, a tint, or
  both, and where its color comes from (theme config vs a constant like
  `SELECTION_RGBA`). D20's invariant is over the opened *string*, so this is free to
  settle at L3 — but a test needs one answer.
- **Assert the surprising case**: if output scrolls a *different* link under a
  stationary pointer between press and release, D19 opens the second link. That is
  the specified consequence of rejecting capture-at-⌘-down; L3 should assert it
  rather than leave it inferred.

## Requirements

### R0: ⌘-clicking a URL on screen opens it in the default browser

#### R0.1: A printed PR link opens on ⌘-click
- **Given**: `gh pr create` has printed `https://github.com/o/r/pull/123` on screen
- **When**: The user holds ⌘, points at any character of that URL, and clicks
  without moving the pointer
- **Then**: The default browser opens `https://github.com/o/r/pull/123`, nothing is
  written to the PTY, and the grid content is unchanged

#### R0.2: Ordinary selection never opens anything
- **Given**: A line containing a URL is on screen
- **When**: The user drags across it with no modifier, then releases
- **Then**: The text is selected exactly as it is today and no browser is invoked

---

### R1: `term_core::links::link_at` resolves a cell to a URL (D2, D3, D4, D5, D12, D13, D16, D23, D25)

#### R1.1: A cell inside an http/https URL resolves to that URL
- **Given**: A `GridSnapshot` whose row 0 reads `see https://example.com/a now`
- **When**: `link_at(&snap, 8, 0)` is called (a column inside the URL)
- **Then**: It returns `Some(LinkSpan)` whose `url` is `https://example.com/a` and
  whose span covers exactly columns 4..=24 of row 0

#### R1.2: A cell outside any URL resolves to nothing
- **Given**: The same snapshot
- **When**: `link_at(&snap, 1, 0)` is called (inside the word `see`)
- **Then**: It returns `None`

#### R1.3: A non-allow-listed scheme is not a link
- **Given**: A row reading `file:///Users/me/.ssh/id_rsa mailto:a@b.com ftp://h/x`
- **When**: `link_at` is called at any column of that row
- **Then**: It returns `None` for every column

#### R1.4: A URL run ends at whitespace, C0/DEL, or any non-ASCII byte
- **Given**: A row reading `https://example.com/a│b` (a `│` box-drawing char, as
  lazygit would paint beside a URL)
- **When**: `link_at` is called at a column inside the URL
- **Then**: The resolved `url` is `https://example.com/a` — the `│` is excluded and
  is not part of the span

#### R1.5: Trailing punctuation is trimmed, iteratively
- **Given**: A row reading `(see https://example.com/a/12).`
- **When**: `link_at` is called inside the URL
- **Then**: The resolved `url` is `https://example.com/a/12` — both `)` and `.` are
  removed, and the span excludes them

#### R1.6: A closing bracket with a matching opener inside the URL is kept
- **Given**: A row reading `https://en.wikipedia.org/wiki/Foo_(bar)`
- **When**: `link_at` is called inside the URL
- **Then**: The resolved `url` retains its trailing `)` — `https://en.wikipedia.org/wiki/Foo_(bar)`

#### R1.7: An authority containing userinfo is rejected outright
- **Given**: A row reading `https://github.com@evil.com/pull/1`
- **When**: `link_at` is called at any column of that row
- **Then**: It returns `None` — no link, not a truncated one

#### R1.8: A `@` after the authority is legal
- **Given**: A row reading `https://example.com/a@b`
- **When**: `link_at` is called inside the URL
- **Then**: It returns `Some` with `url` == `https://example.com/a@b`

#### R1.9: A non-ASCII byte inside the authority rejects the whole candidate
- **Given**: A row reading `https://gıthub.com/pull/1` (U+0131 dotless ı)
- **When**: `link_at` is called at any column of that row
- **Then**: It returns `None` — specifically **not** the truncated `https://g`

#### R1.10: A non-ASCII byte after the authority merely ends the run
- **Given**: A row reading `https://example.com/aı`
- **When**: `link_at` is called inside the URL
- **Then**: It returns `Some` with `url` == `https://example.com/a`

#### R1.11: An empty authority is rejected
- **Given**: A row reading `https:// and https:///p`
- **When**: `link_at` is called at any column of that row
- **Then**: It returns `None` for every column

#### R1.12: A soft-wrapped URL is rejoined into one link
- **Given**: A 20-column snapshot where row 0 ends `https://example.co` filling the
  last column and row 1 begins `m/pull/9 done`
- **When**: `link_at` is called at a column in row 0 inside the URL
- **Then**: The resolved `url` is `https://example.com/pull/9` and the span covers
  the tail of row 0 **and** the head of row 1

#### R1.13: Either half of a wrapped URL resolves to the same link
- **Given**: The same snapshot
- **When**: `link_at` is called at a column in row 1 inside the URL's continuation
- **Then**: It returns a `LinkSpan` equal to the one returned for row 0

#### R1.14: A hard-wrapped row does not stitch
- **Given**: A snapshot where row 0's last column is blank (no soft wrap) and row 1
  begins with text
- **When**: `link_at` is called at a column near the end of row 0
- **Then**: Resolution stops at the end of row 0 — row 1's characters never join the URL

#### R1.15: Double-width continuation cells never enter a URL
- **Given**: A row containing `https://example.com/한글` where `한` occupies two cells
- **When**: `link_at` is called inside the URL
- **Then**: The resolved `url` is `https://example.com/` and contains no
  `Cell::CONTINUATION` sentinel character

#### R1.16: Resolution works on a history-composed snapshot
- **Given**: A `GridSnapshot` taken with `view_offset > 0`, whose visible rows come
  from scrollback and contain a URL
- **When**: `link_at` is called at a column inside that URL
- **Then**: It resolves identically to the live-bottom case (D8)

#### R1.17: The resolver is headless and pure
- **Given**: A unit test that constructs a `GridSnapshot` directly, with no window,
  no GPU, and no PTY
- **When**: The test calls `link_at` and calls it a second time with the same inputs
- **Then**: It compiles and passes in `term-core`'s test suite, and both calls return
  equal values with no observable side effect

---

### R2: The highlight is edge-triggered and always current (D7, D11, D21, D24)

#### R2.1: Pressing ⌘ over a link lights it
- **Given**: The pointer rests over a URL on the grid and no modifier is held
- **When**: The user presses and holds ⌘
- **Then**: That URL's span is written to the link handle and a redraw is requested

#### R2.2: Releasing ⌘ clears the highlight
- **Given**: A link is currently lit with ⌘ held
- **When**: The user releases ⌘
- **Then**: The handle's span becomes `None` and no scan is performed

#### R2.3: Moving to a different cell with ⌘ held recomputes
- **Given**: ⌘ is held and link A is lit
- **When**: The pointer moves to a cell belonging to link B
- **Then**: The handle holds link B's span; moving to a non-link cell sets it to `None`

#### R2.4: Motion inside the same cell recomputes nothing
- **Given**: ⌘ is held and a link is lit
- **When**: The pointer moves by a few pixels without leaving its cell
- **Then**: No scan runs and the handle is untouched (keyed on the cell, like the
  existing drag-jitter guard)

#### R2.5: A fresh snapshot while ⌘ is held recomputes the highlight
- **Given**: ⌘ is held, a link is lit, and the pointer does not move
- **When**: PTY output arrives and a new snapshot is taken (the `snapshot_dirty`
  branch of `RedrawRequested`)
- **Then**: The highlight is recomputed against the new snapshot — following the link
  if it scrolled, becoming `None` if nothing is under the pointer any more

#### R2.6: An idle frame never scans
- **Given**: ⌘ is held, a link is lit, and no output arrives
- **When**: The continuous redraw loop produces frames that re-present the cached
  snapshot
- **Then**: Zero scans run and the handle is untouched

#### R2.7: Losing window focus disarms the gesture
- **Given**: ⌘ is held and a link is lit
- **When**: The window loses focus (⌘-Tab away — no ⌘-up event is delivered)
- **Then**: The highlight is cleared and the gesture is disarmed, so a later plain
  click on that cell opens nothing

#### R2.8: The pointer outside the window never lights a link
- **Given**: The pointer has left the window (`CursorLeft`) while stale `last_cursor`
  coordinates still point at a URL cell
- **When**: The user presses ⌘
- **Then**: No scan runs and nothing is lit — an explicit `pointer_inside` flag, not
  `cursor_cell()`, gates this

#### R2.9: A scan touches only the hovered row's soft-wrapped run
- **Given**: A 100-row grid with URLs on many rows and ⌘ held over row 40
- **When**: A scan is triggered
- **Then**: Only row 40 and the rows in its contiguous soft-wrapped run are examined

#### R2.10: No scan ever runs with ⌘ up
- **Given**: ⌘ is not held
- **When**: The pointer moves anywhere over the grid and output streams continuously
- **Then**: Zero scans run

---

### R3: The renderer draws the affordance from a shared handle (D15, D25)

#### R3.1: A lit span renders as an underline
- **Given**: The link handle holds a span covering columns 4..=24 of row 0
- **When**: The layer records a frame
- **Then**: It emits one underline quad per covered cell, using the **same geometry
  constants as the grid's `UNDERLINE` pass** (`thickness = max(ch*0.06, 1.0)`,
  `y = row*ch + ascent + max(ch*0.08, 1.0)`) and each cell's own foreground color.
  No background tint is drawn.

#### R3.2: An empty handle draws nothing
- **Given**: The link handle holds `None`
- **When**: The layer records a frame
- **Then**: It emits zero instances and issues no draw call

#### R3.3: A wrapped span underlines both rows
- **Given**: The handle holds a span that starts in row 0 and ends in row 1
- **When**: The layer records a frame
- **Then**: Both the row-0 tail and the row-1 head are underlined, and no cell
  between the two ends is skipped

#### R3.4: The instance builder is a pure GPU-free function
- **Given**: A span and cell metrics passed to the instance-building function
- **When**: A unit test calls it with no GPU device
- **Then**: It returns the expected instance list, testable exactly like
  `selection_instances`

#### R3.5: The handle model holds at most one span
- **Given**: The handle type
- **When**: Its model is inspected
- **Then**: It stores `Option<LinkSpan>` — a single span, matching what `link_at`
  returns

#### R3.6: A highlight change does not invalidate the snapshot cache
- **Given**: ⌘ goes down over a link
- **When**: The redraw for the new highlight is requested
- **Then**: `snapshot_dirty` is **not** set (the bare `window.request_redraw()` is
  used), so the next frame re-presents the cached snapshot

---

### R4: The click gesture opens exactly what is lit (D1, D9, D10, D14, D17, D19, D20, D22)

#### R4.1: ⌘-click on a lit link opens it
- **Given**: ⌘ is held and a link is lit under the pointer
- **When**: The user presses and releases the left button without leaving the cell
- **Then**: The sink is called exactly once with the lit span's `url`

#### R4.2: The opened string is byte-identical to the lit cells
- **Given**: Any lit span (including a trimmed and/or wrap-rejoined one)
- **When**: The open fires
- **Then**: The string passed to the sink equals the `url` of the span currently in
  the handle — asserted directly, not by re-deriving it

#### R4.3: Releasing ⌘ before the button cancels the open
- **Given**: ⌘ was held at press and a link was lit
- **When**: The user releases ⌘ first, then releases the button
- **Then**: The highlight is already cleared and the sink is never called

#### R4.4: Pressing ⌘ after the button press does not open
- **Given**: The user presses the left button over a URL with no modifier
- **When**: They then press ⌘ and release the button without moving
- **Then**: The sink is never called; the release behaves exactly as today

#### R4.5: ⌘+drag selects and opens nothing
- **Given**: ⌘ is held over a URL
- **When**: The user presses, moves the pointer to a different cell, and releases
- **Then**: A selection exists over the dragged range and the sink is never called

#### R4.6: A plain click on a URL behaves exactly as today
- **Given**: No modifier is held and a selection exists
- **When**: The user clicks on a URL without moving
- **Then**: The selection is cleared, nothing is lit, and the sink is never called

#### R4.7: A release over nothing lit opens nothing
- **Given**: ⌘ is held and the pointer is over ordinary text
- **When**: The user clicks
- **Then**: The sink is never called

#### R4.8: Output that moves a different link under a stationary pointer opens the second
- **Given**: ⌘ is held, link A is lit, the button is pressed and held
- **When**: Output scrolls the view so that link B is now under the stationary
  pointer (R2.5 relights it), and the button is then released
- **Then**: The sink is called with link B's URL — the specified consequence of
  opening what is lit rather than what was captured at press

#### R4.9: The open goes through an injectable sink
- **Given**: A handler constructed with a recording sink in a unit test
- **When**: An opening ⌘-click is dispatched
- **Then**: The recorder captures the URL and no `NSWorkspace` call is made, so
  `cargo test` never launches a browser

#### R4.10: A failed open is logged and otherwise silent
- **Given**: The production sink and a URL macOS declines to open
- **When**: The open fires
- **Then**: A `tracing` warning is emitted, the grid and highlight are unchanged, and
  nothing is written to the PTY

#### R4.11: Links stay live inside a mouse-reporting TUI
- **Given**: The child app has enabled mouse reporting (`?1000`/`?1002`/`?1003`) and
  its screen shows a URL
- **When**: The user ⌘-clicks that URL
- **Then**: The sink is called and no mouse-report bytes are written to the PTY

#### R4.12: A ⌘-click over the sidebar keeps its current meaning
- **Given**: The pointer is over a tab card, the card menu, or the rename field
- **When**: The user ⌘-clicks
- **Then**: The existing ordered `hit_test` action runs (switch / menu / rename) and
  no link is resolved or opened

#### R4.13: An opening click still dismisses modals
- **Given**: The card menu is open and a rename field is active
- **When**: An opening ⌘-click lands on the grid
- **Then**: `press_dismisses_modals` runs exactly as for a plain grid press — the
  menu closes and the rename cancels — in addition to the open

---

### R5: Constraints hold (D11, D12, D16, and the Constraints section)

#### R5.1: Streaming output with ⌘ up costs nothing
- **Given**: A command producing continuous output and ⌘ not held
- **When**: The terminal runs for a sustained period
- **Then**: Zero link scans occur and the renderer's snapshot-equality cache
  behaves exactly as before this feature

#### R5.2: `term-core` stays headless
- **Given**: The workspace after the change
- **When**: `term-core`'s manifest and imports are inspected
- **Then**: There is no `wgpu`, `winit`, or other GPU/window dependency, and the new
  `links` module imports none

#### R5.3: No new third-party dependency enters the workspace
- **Given**: The workspace `Cargo.toml` and each crate manifest after the change
- **When**: Dependencies are diffed against `main`
- **Then**: No crate has been added — in particular no `regex` — and `Cargo.lock` is
  changed only by cargo

#### R5.4: The lint and test gates stay green
- **Given**: The completed change
- **When**: `cargo test --workspace` and
  `cargo clippy --workspace --all-targets -- -D warnings` run
- **Then**: Both pass, and the change adds no `unwrap()` outside `#[cfg(test)]`

#### R5.5: Existing selection, copy, and scroll behavior is unchanged
- **Given**: The existing `selection-input`, `copy-clipboard`, and `terminal-scroll`
  test suites
- **When**: They run against the change
- **Then**: All pass unmodified, and no existing insta snapshot requires acceptance

---

### R6: Explicit non-behaviors (D2, D6, D18)

#### R6.1: An OSC 8 sequence produces no link and no damage
- **Given**: The PTY emits `ESC ] 8 ; ; https://evil.com ESC \ PR #123 ESC ] 8 ; ; ESC \`
- **When**: The parser processes it
- **Then**: The escape is ignored as before (no crash, no grid mutation), the visible
  text `PR #123` is **not** clickable, and OSC 0/2/133 handling is unaffected

#### R6.2: Hovering without ⌘ shows nothing
- **Given**: The pointer rests over a URL with no modifier held
- **When**: The user waits
- **Then**: No underline, no tint, no cursor-shape change — the screen is identical
  to today

#### R6.3: `file://` and `mailto:` are not clickable
- **Given**: A row containing `file:///Users/me/.ssh/id_rsa` and `mailto:a@b.com`
- **When**: The user ⌘-hovers and ⌘-clicks each
- **Then**: Nothing is lit and the sink is never called

## Tasks

### T1: `term_core::links` — the resolver and its type [infra]
- **Fulfills**: R1, R6.1, R5.2
- **Depends on**: (none)
- **Surface**: new `crates/term-core/src/links.rs` (`LinkSpan`, `link_at`);
  `crates/term-core/src/lib.rs` (module + re-export);
  `crates/term-core/src/snapshot.rs` (`row_soft_wraps` → `pub(crate)`);
  new tests in `crates/term-core/tests/`.
- **Notes**: Horizontal by necessity — this is shared infrastructure with no UI
  counterpart, and it is the type both later layers consume. R6.1 belongs here
  because "OSC 8 is still ignored" is a parser-level assertion. The 17 R1 sub-reqs
  are the acceptance criteria; every one is a table-testable pure-function case.

### T2: `renderer::link_layer` — draw the affordance from a shared handle [infra]
- **Fulfills**: R3
- **Depends on**: T1 (consumes `LinkSpan`)
- **Surface**: new `crates/renderer/src/link_layer.rs` (layer + `LinkHandle`);
  `crates/renderer/src/lib.rs` (module export, z-order const).
- **Notes**: Modeled on `selection_layer` for the quad work and on
  `preedit_layer` for the `Rc<RefCell<…>>` handle. The instance builder is a pure
  function (R3.4) so the whole task is testable without a GPU. Nothing is visible
  yet — the handle has no writer until T3.

### T3: ⌘-hover lights a link [vertical]
- **Fulfills**: R2
- **Depends on**: T1, T2
- **Surface**: new `crates/app/src/hyperlink.rs` (the trigger/gate logic as pure
  functions); `crates/app/src/terminal_app.rs` (construct + hold the handle beside
  `CursorLayer`/`PreeditLayer`; hook `ModifiersChanged`, `CursorMoved`, `CursorLeft`,
  `Focused(false)`, and the `snapshot_dirty` branch of `RedrawRequested`; add the
  `pointer_inside` flag).
- **Notes**: First user-visible slice — hold ⌘ over a URL and it underlines. Ends
  at the affordance: no click handling yet, so nothing can open. Keep the trigger
  predicates (which event, which gate, which bound) in `hyperlink.rs` as pure
  functions over plain inputs so R2.4/R2.6/R2.8/R2.10 are unit-testable without an
  event loop.

### T4: ⌘-click opens what is lit [vertical]
- **Fulfills**: R0, R4, R6.2, R6.3
- **Depends on**: T3
- **Surface**: `crates/app/src/hyperlink.rs` (the open decision + the injectable
  `NSWorkspace` sink); `crates/app/src/terminal_app.rs` (the press/release arms).
- **Notes**: Serialized after T3 rather than parallel because both write
  `terminal_app.rs` and `hyperlink.rs`. Completes the feature end-to-end. The open
  decision — "⌘ at press **and** release, a lit span under the pointer, open exactly
  its `url`" — must be a pure function over plain inputs (R4.9), so R4.1–R4.8 are
  testable with a recording sink and no browser is ever launched by `cargo test`.

### T5: Constraint + regression sweep [infra]
- **Fulfills**: R5
- **Depends on**: T4
- **Surface**: no new feature code — manifest/dependency diff, the existing
  `selection-input` / `copy-clipboard` / `terminal-scroll` suites, clippy, and an
  idle-scan assertion.
- **Notes**: R5.1 (zero scans with ⌘ up) is the one that must be *asserted*, not
  eyeballed — the idle-CPU debt this feature could reintroduce was a real past
  regression. R5.3 also guards the `Cargo.lock` PreToolUse hook: no dependency is
  added, so the lock file must change only by cargo, if at all.

## External Dependencies

### Pre-work
- (none) — no new crates, no accounts, no services. `objc2-app-kit 0.3.2` already
  exposes `NSWorkspace` with the features currently enabled.

### Post-work
- Eyeball the render via `/run-term`: the underline's vertical position, thickness,
  and color at the actual DPI are the one part of R3.1 that unit tests cannot judge.
  Check a wrapped URL (R3.3) and a link in scrolled-back history (R1.16) while
  you are there.

