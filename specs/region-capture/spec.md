# Spec: region-capture

## Meta
- **Created**: 2026-09-21
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-09-21
- **L2 review**: 3 rounds (NEEDS_FIX, NEEDS_FIX, PASS) — D10 was decided three times; the first two
  designs were blocking-defective and both failures are recorded in D10 itself.

## Goal
Add a capture function to the terminal: one key chord runs an interactive region
screen capture, and the resulting image is handed straight to the CLI AI agent
running in the active tab — skipping the user's current "run a capture app → copy →
paste into the agent" round trip.

## Non-goals
- Full-screen or single-window capture modes — region selection only.
- Capturing the terminal's own grid (as text or as pixels).
- Video or GIF capture.
- Rendering the captured image inline in the terminal as confirmation — path only,
  matching today's Cmd+V image-paste behaviour (L0 decision).
- A retention/cleanup policy for captured files. They accumulate exactly as Cmd+V
  pastes already do in `~/.cache/mac-terminal/pasted/`; pruning is a separate spec
  if it ever bites (L0 decision).
- Any change to the existing Cmd+V clipboard paste path, which keeps working as-is.

## Confirmed Goal
A key chord in the active tab launches macOS interactive **region** capture. On a
completed selection the PNG is written under the existing image-paste cache
directory and its **absolute path + a space** is typed into the **originating** tab's PTY
(the tab that was active when the chord was pressed — see D6, which may no longer be the active
tab seconds later), so
`claude`/`gemini` receive a real file path with no clipboard involvement. On a
cancelled capture nothing is typed. The capture must not block the winit event loop
(a stalled loop blanks the window — C6 / winit#2640), and the feature must account
for macOS Screen Recording (TCC) permission being required and possibly denied.

Done when: with `claude` running in a tab, pressing the chord, dragging a region and
releasing sends the image to the agent; pressing Esc mid-capture leaves the prompt
untouched; and the window keeps drawing throughout.

## Research

### The half that already exists (reuse, don't rebuild)
- Image-paste **path mode** already does the whole back half: write PNG → type
  absolute path + space into the active tab's PTY (`crates/app/src/clipboard.rs:74-76`).
- Its rationale is this feature's rationale, stated verbatim in the earlier spec:
  "so `claude`/`gemini` on the CLI receive a real file path"
  (`specs/image-paste/spec.md:79-81`, D4).
- Cache dir helper `pasted_dir()` → `~/.cache/mac-terminal/pasted/`, `create_dir_all`
  on demand, `None` when `$HOME` is unset (`clipboard.rs:88-96`).
- Collision-free naming `unique_name(secs, seq)` → `paste-<ts>-<seq>.png`, a
  per-process `AtomicU64` so same-second writes never collide (`clipboard.rs:100-102`).
- Never-overwrite guard: `path.exists()` → bail (`clipboard.rs:70-71`, C4 of image-paste).

### The seam it must register through
- Two registration seams exist, and **which one a feature picks decides what it can do** — the fact
  that decided D10. Handlers are values in `feature_handlers()` (`lib.rs:174-181`, precedents
  `ClipboardHandler` and `CopyHandler`), which **takes no arguments**. Overlays are values in
  `feature_overlays(proxy, arm)` (`lib.rs:155`), which is handed an `EventLoopProxy` — so only an
  overlay can wake the loop from a background thread.
- An overlay need not draw or be visible: `ImagePresenter` (`clipboard.rs:166`) is a pure
  passthrough with `visible() -> false`, and `Overlay` carries five defaulted methods
  (`seam.rs:265, 277, 285, 299`, `on_active_changed`), so a new defaulted method disturbs no
  existing implementor.
- The proxy→background→wake shape is already in-crate: `overlay.rs:893-899` clones the proxy into a
  closure moved into `std::thread::spawn`, which sends `UserEvent::OverlayPending`.
- SD4 forbids a *feature* editing `terminal_app.rs` (`specs/image-paste/spec.md:52, 90`); D10 adds
  one generic drain block to the hub rather than feature-specific logic.

### Why it cannot be done inline on the UI thread
- `AppCtx::writer` is `&'a mut dyn Write` (`crates/app/src/seam.rs:82`) — a borrow, neither
  `Send` nor `'static`. No background thread can hold it.
- `InputHandler::on_key` is synchronous, returning `Handled` (`seam.rs:236-238`). It cannot await.
- Blocking it is not merely slow — it blanks the window. C6's continuous redraw exists because
  the compositor drops the presented frame once the run loop sleeps (winit#2640);
  `docs/architecture.md` › Runtime model records the four failed alternatives.
- The codebase already solved "call something that can hang, from the UI thread": `resolved_root_with`
  spawns a thread and `recv_timeout`s a budget (`crates/app/src/tabsession.rs:429-437`).
- A background→loop wake path already exists: `session::Wake = Box<dyn Fn(TabId) + Send + 'static>`
  (`session.rs:35`), and `UserEvent::{PtyOutput, ConfigReloaded, OverlayPending}`
  (`terminal_app.rs:73-82`) with `OverlayPending` sent from a background closure (`main.rs:51-54`).
- Handlers that need state the writer-borrow excludes *record* an `AppRequest` that `terminal_app`
  applies after dispatch (`seam.rs:127-148`) — the established shape for deferred effects.
- **But that shape does not reach background work, and nothing else does either.**
  `Overlay::poll` returns a bare `bool` with no context (`seam.rs:296-300`); `InputHandler` has
  only `on_key` (`seam.rs:235-238`); and `ctx.take_requests()` is called at exactly two
  production sites, both key dispatch (`terminal_app.rs:2076`, `2117`). `poll_all()` likewise has
  one production call site (`terminal_app.rs:2365`) inside the `UserEvent::OverlayPending` arm,
  and a structural test asserts it stays exactly one, in that arm (`overlay.rs:5974-5988`).
  Every producer of that event is a tabmeta or ai-monitor watcher (`main.rs:54`,
  `overlay.rs:667/685/791/896`). `feature_handlers()` takes **no arguments** (`lib.rs:174`),
  unlike `feature_overlays(proxy, arm)` (`lib.rs:155`), so a handler cannot even obtain a proxy.
  **There is today no path from background work to a tab's PTY** — the finding that decided D10.

### The chord — a trap this repo already fell into once
- **macOS takes `Cmd+Shift+3/4/5` before any application sees the event.** The ai-monitor
  section-jump family was originally `Cmd+Shift+<digit>`, and sections 3-5 were simply
  unreachable; it moved to `Cmd+Opt+<digit>` for that reason (`crates/app/src/input.rs:623-627`,
  regression test `input.rs:1652-1660`). The obvious chord for a screenshot feature is exactly
  the one the OS eats.
- Taken today, verified by enumerating every case-folded letter chord in `crates/app/src/`
  (`a c e m r s v`): `Cmd+Shift+M` (ai-monitor), `Cmd+Shift+R` (rename), `Cmd+Shift+S` (sidebar),
  `Cmd+C`/`Cmd+V`, `Cmd+T`/`Cmd+W`, `Cmd+<digit>` (tabs), `Cmd+Opt+1..6` (monitor sections).
- **Two modal traps inside the rename field**, both easy to miss because they are not in any
  chord-toggle list: it matches `Cmd+A` case-folded **without excluding shift**
  (`rename.rs:220-222`), and it consumes *every* `Cmd`/`Ctrl` chord outright
  (`rename.rs:276-278`, its D8a.1). Overlays dispatch before handlers (`terminal_app.rs:2072`
  before `2115`), so a capture chord colliding with the first does the wrong thing, and any
  capture chord at all does nothing while that field is open.
- Letter chords are matched case-folded (`copy.rs:21`, `input.rs:231`); digit chords must be
  matched on **physical key position**, because on a US layout `Cmd+Opt+1` arrives as `"¡"`
  (`input.rs:617-621`).

### The external tool
- The binary is **`/usr/sbin/screencapture`**, not `/usr/bin/screencapture`. The production
  subprocess precedent uses an absolute path (`tabsession.rs:523`); note `presence_fd.rs:364` looks
  like a second one but sits inside `#[cfg(test)]`.
- `-i` = interactive selection; **Escape cancels**. `-s` = "only allow mouse selection mode",
  which locks out window-selection and so enforces the region-only non-goal at the tool level.
- Documented hazard: under `-i`, "the control key will cause the screenshot to go to the
  clipboard" — the user can complete a capture and **no file is written**. Indistinguishable
  from cancel at the filesystem, which is why the completion test must be the file, not the
  exit status.
- The man page documents no exit code for cancel (it opens "not very well documented to date").
  Treat exit status as unverified until measured.

### macOS Screen Recording (TCC) — the unenforceable assumption
- `scripts/bundle-app.sh:4-5` states the bundle is **unsigned, un-notarized, explicitly**:
  "no code signing, no notarization (explicit non-goals). The bundle runs on the machine that
  built it." Bundle id `dev.local.mac-terminal`, Info.plist carries no TCC usage keys.
- TCC grants bind to an app's code identity, so an unsigned binary rebuilt by `cargo build
  --release` can present a changed identity to TCC. Whether the grant survives a rebuild, and
  whether `-i` needs the grant at all (the capture is performed by Apple's own
  `screencapture` process, not by us), are **machine facts this repo cannot assert from source**.
  They belong in the same family as G5's A1/A2 and G6 in `docs/architecture.md`: named
  assumptions about external behaviour, to be measured rather than believed.

### Verification surface
- App-level tests live in `crates/app/tests/{vertical_slice.rs,wiring.rs}`; handler unit tests
  sit in-module (`clipboard.rs` `mod tests`, `copy.rs`, `input.rs`).
- Coverage is judged by `cargo mutants -f <file> -j 4`, not test count (CLAUDE.md).

## Decisions

> **Complexity: Complex (5 signals)** — revised up from Medium during L2 review. External resource
> (`screencapture` behind a TCC gate), async resource (completion arrives seconds later off the UI
> thread), multi-stage flow (idle → capturing → completed / cancelled / Ctrl-diverted),
> **child-process lifecycle** (spawn / deadline / kill / orphan, D7), and a **cross-cutting seam
> change** (D10 adds a defaulted `Overlay` method and one drain site).
> *An earlier draft of this note claimed the D10 reversal "removed two signals". It did not — the
> seam change is still here, and the shared-writer form it briefly chose would have added a
> `Session` field-type change plus two call-site rewrites. The count is 5 either way; the reason is
> different.*

### D1: The chord is `Cmd+Shift+G`, consumed and never forwarded to the shell
- **Status**: resolved
- **Rationale**: "G" for Grab, the name of Apple's original screenshot utility. Verified free: the
  case-folded letter chords taken across `crates/app/src/` are exactly `a c e m r s v`
  (`clipboard.rs:83`, `copy.rs:23`, `input.rs:235`, `overlay.rs:429`, `rename.rs:91/206/209/221`).
  **Rejected — `Cmd+Shift+4`**: macOS takes `Cmd+Shift+3/4/5` before any application sees the
  event. This repo already paid for that lesson — the ai-monitor section jumps were
  `Cmd+Shift+<digit>` and 3-5 were unreachable, which is why they moved to `Cmd+Opt+<digit>`
  (`input.rs:623-627`, regression test `input.rs:1652-1660`). Reclaiming it needs the system
  shortcut disabled globally, breaking screenshots in every other app to gain it in one.
  **Rejected — `Cmd+Shift+A`** (an earlier draft's choice, caught in review): `rename.rs:220-222`
  matches `Cmd+A` case-folded **without excluding shift**, and overlays dispatch before the default
  path (`terminal_app.rs:2072`, `:2115`), so with a rename field open it would re-select the tab
  name instead of capturing.
  **Rejected — narrowing rename's `Cmd+A` with a `!shift_key()` guard**: arguably fixes a latent
  bug, but changes a shipped feature's input handling and widens this spec's blast radius.
- **Known modal limit, stated rather than hidden**: while a tab-rename field is open it consumes
  *every* `Cmd`/`Ctrl` chord (`rename.rs:276-278`, its D8a.1), so `Cmd+Shift+G` does nothing there
  — exactly as `Cmd+V` and `Cmd+C` already do nothing there.

### D2: Region-only and error-reporting are enforced by the tool: `screencapture -i -s -d <path>`
- **Status**: resolved
- **Rationale**: `-s` = "only allow mouse selection mode", making the region-only non-goal a
  property of the invocation rather than a convention the code must police — window selection
  cannot be reached even by pressing Space. `-d` = "display errors to the user graphically", so a
  missing Screen Recording grant is explained by Apple's own dialog, naming the right permission,
  with no UI code and no bytes on the PTY. The binary is at **`/usr/sbin/screencapture`** (verified
  present; `/usr/bin/screencapture` verified absent), invoked by absolute path like the crate's
  production subprocess precedent (`tabsession.rs:523`).
  **Rejected — silent no-op on failure**: a first-run permission denial would be indistinguishable
  from a broken chord, with nothing to diagnose.
  **Rejected — typing `# capture failed: …`**: puts non-user bytes on the PTY for a reason the user
  did not originate.

### D3: Completion is judged by the **file**, never by exit status
- **Status**: resolved
- **Rationale**: The man page documents no exit code for cancellation and opens by admitting the
  tool "is not very well documented to date". Worse, it documents a case where the user *completes*
  a capture and no file appears: under `-i`, "the control key will cause the screenshot to go to
  the clipboard". Cancel and Ctrl-divert are indistinguishable at the process level but identical
  at the filesystem — in both, no file was written and nothing should be typed. Testing
  `path.exists() && len > 0` collapses all three outcomes into one correct rule and depends on
  nothing undocumented.
  **Rejected — branching on `ExitStatus`**: would encode an unverified assumption about an external
  tool, exactly the failure mode `docs/architecture.md` G6 records for the `/context` title read.

### D4: Reuse the image-paste cache directory, guard and naming scheme — with a `capture-` prefix
- **Status**: resolved
- **Rationale**: `pasted_dir()` (`clipboard.rs:88-96`) and the never-overwrite `path.exists()`
  guard (`clipboard.rs:70-71`) are already correct and already tested; duplicating them would
  create two answers to one question. Naming is `capture-<secs>-<seq>.png` against the same
  per-process `AtomicU64` discipline as `unique_name` (`clipboard.rs:100-102`) — the distinct
  prefix keeps provenance readable in a directory that also holds `paste-*` files, at zero cost.
  **Rejected — writing into the active tab's cwd**: guarantees the agent can read it, but litters
  the project directory, risks committed PNGs, and makes capture behave differently from Cmd+V.

### D5: The capture runs on a watcher thread that the UI thread never waits on, and the spawn is injectable
- **Status**: resolved
- **Rationale**: `screencapture -i` blocks for as long as the human takes to drag a rectangle.
  Blocking `on_key` is not merely slow — C6's continuous redraw exists because the compositor drops
  the presented frame once the run loop sleeps (winit#2640), so a multi-second stall blanks the
  window, and `docs/architecture.md` records four targeted alternatives that already failed.
- **The in-crate precedent transfers only halfway, and the half that must not transfer is named**:
  `resolved_root_with` (`tabsession.rs:429-437`) spawns a thread and then calls
  `rx.recv_timeout(RESOLVE_BUDGET)` **on the calling thread** (`:436`). Copied literally with D7's
  budget that would freeze the UI for five minutes and violate C4. Here `on_key` spawns and returns
  immediately; the *watcher* thread owns the deadline. `std::process::Child` has no `wait_timeout`
  on stable, so the watcher enforces D7 by `try_wait` polling.
- **The other half of that precedent is the part to copy**: `resolved_root_with` is "generic over
  the resolver so the budget is testable without actually hanging the suite" (`tabsession.rs:427`).
  `screencapture -i` waits for a human and can never run under `cargo test`, so the spawn must be
  injectable the same way — otherwise D5, D7 and D12 have no testable requirement at all and
  `cargo mutants` reports the whole module as survivors.

### D6: Delivery targets the `TabId` captured at press time
- **Status**: assumed (derived from an existing repo convention, not user-chosen)
- **Rationale**: The capture was initiated for one tab's agent, and the roster can change between
  press and completion. `seam.rs:121-123` already rules on this shape: every request "names a
  `TabId`, never an index: … an index would silently mean a different tab if the roster shifted
  between the press and the apply." `session::Wake` is likewise already `Fn(TabId)` (`session.rs:35`).
- **Observable when the tab is gone** (the review's point — "the bytes are dropped" names nothing a
  test can assert): `Tabs::by_id_mut` (`tabs.rs:68`) returns `None`, the drain does nothing, and the
  requirement is that **no other tab receives the path, no panic occurs, and no error surfaces**.
  The PNG remains on disk.

### D7: Single-flight with a 5-minute budget; the child is killed on expiry; the flag clears on every outcome
- **Status**: resolved
- **Rationale**: One capture at a time — `screencapture` owns the whole screen and a second
  crosshair is incoherent. The budget is **5 minutes, and that number is load-bearing in the
  opposite direction from the crate's existing one**: `RESOLVE_BUDGET` is ~250 ms for a `ps` walk,
  and copying that constant here would abort every real capture mid-drag. This timeout is a
  wedge-recovery device, not a UX deadline — far beyond any human selection, still re-arming within
  one work session. The in-flight flag clears on *every* terminal outcome — success, cancel,
  timeout, spawn failure, dead tab — because a flag that leaks on one path disables the chord for
  the rest of the session.
  **Rejected — 30 s**: an interrupted user returns to a silently dead crosshair.
  **Rejected — no timeout**: a child that never exits (a permission dialog left open behind another
  window) disables the chord until the terminal restarts.

### D8: Every failure path returns `Consumed` with nothing written — never `Passthrough`
- **Status**: resolved (**corrected in review — the earlier draft said `Passthrough` and was wrong**)
- **Rationale**: `Passthrough` does not mean "do nothing" for this chord. winit reports `text` for
  Cmd chords — `rename.rs:271-273` states it outright ("without this guard Cmd+V would insert a
  literal `v`") — and `encode_key`'s terminal arm is `_ => text.map(|t| t.as_bytes().to_vec())`
  (`input.rs:894`), which `terminal_app.rs:2149-2151` writes to the shell. So falling through would
  type a literal `g` at the prompt on every failed capture, breaking D1 and C6. The uniformity
  argument with `clipboard.rs`'s degrade-to-`Passthrough` does not carry over, because Cmd+V's
  fall-through is *wanted* there and image-paste has no "never reaches the shell" constraint.
  Spawn failure (a missing or unexecutable `/usr/sbin/screencapture`) therefore consumes the chord
  and does nothing visible — `-d` cannot report it, because nothing spawned.

### D9: The typed bytes are the absolute path plus one space, not bracketed-paste wrapped
- **Status**: assumed (matches the adjacent code path)
- **Rationale**: Exactly what `clipboard.rs:74-76` writes on the image branch. `wrap_paste` is
  applied only to the *text* branch, because bracketed paste exists to stop multi-line pasted text
  executing — a single path token with no newline needs none of it, and wrapping would make the two
  image paths differ for no reason.

### D10: The capture registers as an `Overlay`, and delivers through a drain after `poll_all`
- **Status**: resolved (**this decision was taken three times; the first two were wrong and both
  failures are recorded here, because each was caused by a fact that is easy to miss again**)
- **Shape**: the feature registers in `feature_overlays(proxy, arm)` (`lib.rs:155`) as an invisible
  overlay — the `ImagePresenter` precedent, `visible() -> false`, default `NoLayer`. `on_key` handles
  the chord and spawns the watcher (D5), which sends the **existing** `UserEvent::OverlayPending` on
  completion. A new `Overlay::take_pending_write(&mut self) -> Option<(TabId, Vec<u8>)>` **defaults
  to `None`**, so no other overlay changes; `terminal_app`'s `OverlayPending` arm drains it right
  after `poll_all()` (`terminal_app.rs:2365`) and writes via the already-existing `Tabs::by_id_mut`
  (`tabs.rs:68`) → `Session::write` (`session.rs:176`) → `snap_after_input(id)`
  (`terminal_app.rs:1982`). The structural guard still passes: it asserts `poll_all()` appears
  exactly once and inside that arm (`overlay.rs:5974-5988`), and a drain after it adds no second call.
- **Why not `AppRequest::TypeIntoTab` drained after `poll_all` (attempt 1)**: it had no wake.
  `poll_all()` has one production call site, in the `OverlayPending` arm, and every producer of that
  event is a tabmeta or ai-monitor watcher (`main.rs:54`, `overlay.rs:667/685/791/896`). The path
  would have appeared on the next unrelated background tick — non-deterministically late. The stated
  cause was that `feature_handlers()` takes no arguments (`lib.rs:174`) so a handler cannot obtain a
  proxy. **That is true of the handler seam only**, and registering as an overlay dissolves it —
  which is the fact both earlier attempts missed.
- **Why not a shared `Arc<Mutex<Box<dyn Write + Send>>>` writer (attempt 2)**: it was chosen because
  it appeared to need no `terminal_app.rs` edit. It does. `WriteProbe::new` takes `&'w mut dyn Write`
  (`seam.rs:358-364`) and both dispatch sites pass `&mut self.tabs.active_mut().writer`
  (`terminal_app.rs:2066`, `:2109`); an `Arc<Mutex<…>>` does not implement `Write`, so both must
  change, and no formulation avoids it because a `WriteProbe` cannot borrow a temporary `MutexGuard`.
  With that asymmetry gone the `Mutex` is pure liability: the UI thread would acquire that lock on
  every keystroke and hold the guard across `dispatch`, so a PTY master write blocking on the capture
  thread — stopped foreground program, flow control, full kernel buffer — stalls the UI thread inside
  `lock()`. **That is the exact stall class C4 and winit#2640 exist to prevent**, and it would have
  been introduced by the decision meant to respect them. (Note `portable_pty` bails on a second
  `take_writer` — `portable-pty-0.9.0/src/unix.rs:359` — so `Arc<Mutex<…>>` was the only shared-writer
  shape available; the option was not merely worse, it was worse *and* forced.)
- **Priority `0`, appended after the two presenters** — and D1 silently depends on it. The rename
  field is priority 200 (`crates/app/tests/wiring.rs:83`), and D1's "known modal limit" (rename
  swallows the chord) is only true while capture sits *below* it. Registering a fifth overlay
  changes eight assertions in `wiring.rs` — `:52` (`len() == 4`), `:76`/`:98`/`:123` (`counts()`),
  `:83`/`:130`/`:147` (`priorities()`), `:92`/`:135` (`layers.len()`) — and two of those are
  behavioural rather than cosmetic: `the_registration_list_reads_in_the_order_keys_are_offered`
  (`:158`) requires descending order, and `no_extension_starts_visible` (`:174`) requires
  `visible() == false`. The test file says so itself at `wiring.rs:53-55`: "If a feature was added,
  assert its priority below too."
- **The drain must carry an outcome, not just a success value.** A bare
  `Option<(TabId, Vec<u8>)>` can only express "a capture succeeded", but D7 requires the in-flight
  flag to clear on cancel, timeout, spawn failure **and** dead tab — outcomes that produce no bytes.
  Drained as success-only, those four paths would never tell the UI thread the capture ended and the
  chord would stay disarmed. The value crossing the seam therefore carries the outcome; only the
  success variant carries bytes.
- **How the bytes reach the overlay**: an `mpsc` channel from the watcher thread, which is what this
  seam was built for — `seam.rs:296-297` describes `poll` as draining "background work (e.g. a
  log-tail channel) into the overlay's draw model". `OverlayHost.overlays` is private
  (`seam.rs:426`), so the drain needs one aggregating method beside `poll_all` (`seam.rs:489`).
- **What this costs, stated plainly**: the writer stays single-authored and the "user-originated
  bytes only" invariant stays structurally enforced by `&mut` — but `take_pending_write` does make it
  *possible* for any future overlay to inject bytes at that drain point (A5). `seam.rs:76-81` names a
  different extension for a related problem ("Add an `AppCtx::write_silent()` … not built yet"); that
  one is about suppressing the snap for non-echo writes during dispatch, which is not this problem —
  a capture path *is* user-originated and *should* snap (D13).

### D11: The capture stays in `~/.cache/mac-terminal/pasted/` despite being outside the agent's workspace
- **Status**: resolved
- **Rationale**: The inversion probe's finding — every requirement can pass and the feature still
  fail, because an agent with workspace-scoped file access may refuse or prompt on a path under
  `~/.cache`. Image-paste D4 has carried this exposure since it shipped, but it has never been
  exercised against an agent through this terminal. One directory for every image this terminal
  hands off is worth more than pre-emptively forking the two features on an unmeasured risk — so it
  is accepted as a **named gap to measure first** (A4, L4 T1), not a silent assumption.

### D12: A capture in flight at app exit is abandoned, and the honest consequence is recorded
- **Status**: resolved (**weakened from the earlier draft's "the child is killed on app exit", which
  was unimplementable**)
- **Rationale**: There is no `ApplicationHandler::exiting` impl in this app — the impl at
  `terminal_app.rs:2165` defines only `resumed`, `user_event` and `window_event`, and
  `WindowEvent::CloseRequested => event_loop.exit()` (`:2402`) runs no feature code. The `Child`
  lives on the watcher thread (D5), so the UI thread holds no handle. Claiming a kill would be
  claiming a mechanism that does not exist. **The truth**: the process exits, the watcher thread dies
  with it, and `screencapture` is reparented and keeps its crosshair until the user presses Esc —
  which dismisses it, writing nothing. Adding an `exiting` hook to kill the child is deliberately
  **out of scope**: it is a third `terminal_app.rs` edit to remove one Esc press.
- **Tab close is different and is handled**: the capture is left to finish (killing it would yank a
  crosshair the user is actively dragging) and its bytes are dropped per D6.

### D13: A delivered capture snaps the target tab's viewport, exactly as typed input does
- **Status**: resolved (**corrected in review — the earlier draft claimed output never snaps, which
  is false**)
- **Rationale**: The earlier draft asserted "output does not snap the viewport". It does, for a
  near-bottom viewport: `was_near_bottom = snap_on_output(s.terminal.view_offset())`
  (`terminal_app.rs:2203`), applied at `:2217-2218`, where `snap_on_output` (`:2842`)
  defers to `near_bottom`, which tests `offset <= SNAP_FLOOR` with `SNAP_FLOOR = 3` (`:2829`, `:2854`). The repo states the distinction
  itself at `terminal_app.rs:2152-2153`: the *input* rule "has no `SNAP_FLOOR`, unlike the output
  snap". Since D10 delivers through `snap_after_input(id)` (`:1982`), a capture snaps
  **unconditionally**, like a keystroke — which is right, because the user did originate it by
  pressing the chord and expects to see where the path landed.

## Constraints
- C1: No new crate dependency. `screencapture` is a subprocess, not a library — `no-gui-crates` untouched.
- C2: macOS ARM64 only; no `#[cfg(target_os)]` branches (`macos-only` rule).
- C3: `term-core` and `renderer` are not touched, and neither is `Session`'s shape —
  `Session.writer` stays `Box<dyn Write + Send>` (`session.rs:49`) and `Pty::writer()` stays
  `Box<dyn Write + Send>` (`pty.rs:290`). `common ← term-core ← renderer ← app` is preserved.
- C4: The UI thread must never block on the capture. `on_key` spawns and returns; the deadline lives
  on the watcher thread (D5); and no lock taken by a background thread is ever acquired on the input
  path (D10).
- C5: Two captures must not collide on one filename. **This cannot be enforced the way image-paste
  enforces it**, and the difference is the point: there the *app* performs the write, so
  `path.exists()` (`clipboard.rs:70-71`) is a real guard. Here `screencapture` writes the file and
  will clobber, so a pre-spawn `exists()` check is best-effort with a TOCTOU window. The guarantee
  rests on `unique_name`'s timestamp + per-process `AtomicU64`; the `exists()` check is a cheap
  second line, not the guarantee.
- C6: The chord must never reach the shell — including on every failure path (D8) — and must not
  collide with `Cmd+Shift+M/R/S`, `Cmd+C/V`, `Cmd+T/W`, `Cmd+<digit>`, `Cmd+Opt+1..6`, or rename's
  modal `Cmd+A`/`Ctrl+A`/`Ctrl+E`.
- C7: The Cmd+V image-paste and text-paste paths are behaviourally unchanged; their existing tests
  stay green. `feature_handlers().len()` stays 2 (`crates/app/tests/wiring.rs:57`) because this
  feature registers as an overlay, not a handler — but the overlay-side assertions in that file
  (counts, priorities, layer count, registration order, initial visibility) must all be updated
  deliberately, not silenced. See D10 for the list.
- C8: `cargo clippy --workspace --all-targets -- -D warnings` clean; no `unwrap()` outside tests;
  per-crate `thiserror` errors.

## Known Gaps
- **A1 (TCC durability)**: `scripts/bundle-app.sh:4-5` builds an **unsigned, un-notarized** bundle by
  explicit non-goal. TCC grants bind to code identity, so a Screen Recording grant may not survive
  `cargo build --release`. If the chord one day does nothing after a rebuild, suspect this first.
  Unmeasured — this repo cannot assert it from source.
- **A2 (grant necessity)**: whether `screencapture -i` requires the grant *at all* is unknown — the
  capture is performed by Apple's own signed binary, and TCC attribution to a spawned child's
  responsible process is not documented. Measure before assuming either way.
- **A3 (cancel exit code)**: undocumented. D3 is written so nothing depends on it; if a future change
  starts reading `ExitStatus`, this gap is why it must not.
- **A4 (agent readability) — MEASURED 2026-09-21, and it passes.** A generated PNG was written to
  `~/.cache/mac-terminal/pasted/` and a Claude Code agent whose workspace root is this repo was given
  only the absolute path. It read the image, no refusal. **D11 therefore holds and T1 may hard-code
  the directory.** Residual caveat, deliberately not overclaimed: the measuring session held broad
  tool permissions, so a session with stricter settings may still show a one-time approval prompt on
  first read — a prompt, not a refusal, and not a blocker. This was the inversion finding behind D11
  and the likeliest way for a fully-correct implementation to be useless; it is now closed.
- **A5 (a second byte-source at the drain)**: `take_pending_write` makes it possible for any future
  overlay to put bytes on a tab's PTY. Narrower than the rejected alternatives — the write happens on
  the UI thread at one defined point, through `Session::write`, and `&mut` still proves a single
  author — but the "user-originated bytes only" invariant (`seam.rs:72-82`) now depends partly on
  overlays honouring it.
- **A6 (kill/write race)**: if the D7 kill lands after `screencapture` has written a complete file,
  that file is orphaned and never typed. Accepted: the kill path never types, so the race can produce
  a stray file but never a stray path at the prompt.
- **A7 (delivery is not display)**: the drain writes the path to the PTY, but it appears on screen
  only if the foreground program **echoes** it. Under `read -s`, or a raw-mode program that discards
  unexpected input, the bytes are delivered and nothing is shown. This is the same *kind* of fact as
  A1-A3 — an assumption about an external program this repo cannot enforce — and it bounds how an L3
  requirement may be phrased: "the path reaches the tab's PTY" is testable, "the path appears in the
  grid" is true only under an echoing shell.

## Requirements

> **Phrasing note forced by gap A7**: a `Then` may assert that bytes **reach the tab's PTY**, never
> that they "appear on screen" — display depends on the foreground program echoing, which this repo
> cannot enforce. Every observable below is chosen to be assertable without a human, without a GPU,
> and without a real screen capture (D5's injectable spawn is what makes that possible).

### R0: A region capture reaches the agent in the originating tab

#### R0.1: A completed capture delivers a readable path to the shell
- **Given**: A tab is running an agent, and the capture spawn is stubbed to produce a valid PNG
- **When**: The user presses `Cmd+Shift+G` and the capture completes
- **Then**: The absolute path of an existing, non-empty `.png` under `pasted_dir()`, followed by one
  space, has been written to that tab's PTY

#### R0.2: A cancelled capture leaves the prompt untouched
- **Given**: A tab is running an agent and a capture is in flight
- **When**: The capture ends without writing a file (Esc, or Ctrl-divert to clipboard)
- **Then**: Zero bytes have been written to any tab's PTY

### R1: The chord is recognised, consumed, and never reaches the shell (D1, C6)

#### R1.1: `Cmd+Shift+G` starts a capture and is consumed
- **Given**: No capture is in flight
- **When**: A `KeyInput` with `SUPER|SHIFT` and character `g` is dispatched
- **Then**: The overlay returns `Handled::Consumed` and exactly one capture spawn is requested

#### R1.2: The chord is matched case-folded
- **Given**: No capture is in flight
- **When**: The key arrives as `G` (shift-produced uppercase) with `SUPER|SHIFT`
- **Then**: It is recognised as the chord, exactly as lowercase `g` is

#### R1.3: A bare `g` is not the chord
- **Given**: No capture is in flight
- **When**: A `KeyInput` with no modifiers and character `g` is dispatched
- **Then**: The overlay returns `Handled::Passthrough` and no spawn is requested

#### R1.4: `Cmd+G` without shift is not the chord
- **Given**: No capture is in flight
- **When**: A `KeyInput` with `SUPER` only and character `g` is dispatched
- **Then**: The overlay returns `Handled::Passthrough` and no spawn is requested

#### R1.5: The chord writes nothing to the shell
- **Given**: A tab with a recording PTY writer
- **When**: The chord is dispatched through the full input path
- **Then**: Zero bytes reach that tab's PTY as a result of the keypress itself

#### R1.6: The chord does not collide with any existing chord
- **Given**: The registered overlay and handler sets
- **When**: Each of `Cmd+Shift+M/R/S`, `Cmd+C`, `Cmd+V`, `Cmd+T`, `Cmd+W`, `Cmd+<digit>`,
  `Cmd+Opt+1..6` is dispatched
- **Then**: No capture spawn is requested by any of them

### R2: The invocation is exactly the region-only, self-reporting form (D2)

#### R2.1: The spawned command line is fixed
- **Given**: A capture is requested with target path `P`
- **When**: The spawn is performed
- **Then**: The program is `/usr/sbin/screencapture` and the arguments are exactly
  `["-i", "-s", "-d", P]`, in that order

### R3: Completion is decided by the file, never the exit status (D3)

#### R3.1: A non-empty file is a completed capture
- **Given**: The stubbed capture exits and a non-empty file exists at the target path
- **When**: The outcome is evaluated
- **Then**: The outcome is success and carries that path

#### R3.2: A missing file is not a capture
- **Given**: The stubbed capture exits and no file exists at the target path
- **When**: The outcome is evaluated
- **Then**: The outcome is non-success and carries no bytes

#### R3.3: A zero-length file is not a capture
- **Given**: The stubbed capture exits and a zero-byte file exists at the target path
- **When**: The outcome is evaluated
- **Then**: The outcome is non-success and carries no bytes

#### R3.4: Exit status does not change the verdict
- **Given**: The stubbed capture exits **non-zero** but leaves a valid non-empty file
- **When**: The outcome is evaluated
- **Then**: The outcome is success — the verdict is identical to R3.1

### R4: The captured file lands beside pasted images, uniquely named (D4, D11, C5)

#### R4.1: The target directory is the image-paste cache directory
- **Given**: `$HOME` is set
- **When**: A target path is computed
- **Then**: Its parent is `pasted_dir()` (`~/.cache/mac-terminal/pasted/`), created if absent

#### R4.2: The filename carries the capture prefix and a timestamp
- **Given**: A capture at epoch seconds `S` with sequence `N`
- **When**: The target name is computed
- **Then**: It is exactly `capture-<S>-<N>.png`, distinguishable from `paste-*.png`

#### R4.3: Two captures within one second get distinct names
- **Given**: Two captures computed with the same epoch second
- **When**: Both target names are computed
- **Then**: The names differ, because the per-process sequence advanced

#### R4.4: An occupied target path aborts before spawning
- **Given**: A file already exists at the computed target path
- **When**: A capture is requested
- **Then**: No spawn occurs and the chord is consumed with nothing written

#### R4.5: An unusable cache directory aborts before spawning
- **Given**: `$HOME` is unset, or the cache directory cannot be created
- **When**: The chord is pressed
- **Then**: No spawn occurs, the chord is consumed, and nothing is written to the PTY

### R5: The UI thread never waits, and the spawn is injectable (D5, C4)

#### R5.1: The key handler returns without waiting for the capture
- **Given**: A capture spawn stubbed to block for longer than any test would tolerate
- **When**: The chord is dispatched
- **Then**: `on_key` returns `Consumed` promptly, with the capture still in flight

#### R5.2: The whole capture path is drivable without a real screen capture
- **Given**: A test that injects a fake spawn
- **When**: That fake reports each of success, cancel, timeout and spawn-failure
- **Then**: Every outcome path of R6 and R7 is exercised with no human, no screen and no GPU

### R6: Single-flight, and the chord re-arms after every outcome (D7)

#### R6.1: A second chord while a capture is in flight is ignored
- **Given**: A capture is in flight
- **When**: The chord is dispatched again
- **Then**: No second spawn occurs, and the chord is still consumed

#### R6.2: The chord re-arms after a successful capture
- **Given**: A capture has completed and been delivered
- **When**: The chord is dispatched again
- **Then**: A new spawn occurs

#### R6.3: The chord re-arms after a cancelled capture
- **Given**: A capture ended with no file written
- **When**: The chord is dispatched again
- **Then**: A new spawn occurs

#### R6.4: The budget expires, kills the child, and re-arms
- **Given**: A capture that has run past the 5-minute budget
- **When**: The watcher's deadline fires
- **Then**: The child is killed, no bytes are delivered, and a subsequent chord spawns again

#### R6.5: The chord re-arms after a spawn failure
- **Given**: The spawn itself fails (program missing or not executable)
- **When**: The chord is dispatched again
- **Then**: A new spawn is attempted

#### R6.6: The chord re-arms when the originating tab is gone
- **Given**: A capture succeeded but its originating tab was closed before delivery
- **When**: The chord is dispatched again in another tab
- **Then**: A new spawn occurs

### R7: Delivery crosses the watcher→drain boundary (D10, D6, D9)

#### R7.1: Producer — a successful capture publishes tab-tagged bytes
- **Given**: A capture in a tab with id `T` completes with a valid file
- **When**: The watcher publishes its outcome
- **Then**: The published value names `T` and carries the path bytes

#### R7.2: Producer — a non-success outcome publishes no bytes but still reports
- **Given**: A capture ends by cancel, timeout or spawn failure
- **When**: The watcher publishes its outcome
- **Then**: A non-success outcome is published, carrying no bytes, so the in-flight flag can clear

#### R7.3: Producer — every outcome wakes the loop
- **Given**: A capture reaches any terminal outcome
- **When**: The watcher publishes it
- **Then**: `UserEvent::OverlayPending` is sent through the proxy — no new event variant is introduced

#### R7.4: Consumer — the drain writes to the originating tab
- **Given**: A success outcome naming tab `T` is pending
- **When**: The `OverlayPending` arm runs and drains after `poll_all()`
- **Then**: The bytes are written through `Tabs::by_id_mut(T)` → `Session::write`, and no other tab
  receives them

#### R7.5: The bytes are the absolute path plus exactly one space
- **Given**: A success outcome for path `P`
- **When**: The bytes are built
- **Then**: They are `P` in full, absolute, followed by a single `0x20` and nothing else

#### R7.6: The bytes are not bracketed-paste wrapped
- **Given**: The tab has bracketed-paste mode enabled
- **When**: A capture is delivered
- **Then**: The bytes carry no `ESC[200~` / `ESC[201~` envelope — matching image-paste's image branch

#### R7.7: A closed originating tab drops the bytes silently
- **Given**: A success outcome names a tab that has since been closed
- **When**: The drain runs
- **Then**: No tab receives the bytes, no panic occurs, no error surfaces, and the PNG remains on disk

### R8: Every failure path consumes the chord and writes nothing (D8, C6)

#### R8.1: A spawn failure consumes the chord
- **Given**: The spawn fails
- **When**: The chord is dispatched
- **Then**: The overlay returns `Consumed` — never `Passthrough`

#### R8.2: No failure path emits a literal `g`
- **Given**: A tab with a recording PTY writer
- **When**: Each of spawn failure, unusable cache dir (R4.5) and occupied path (R4.4) occurs
- **Then**: Zero bytes reach the PTY in every case — in particular never `g` or `G`

### R9: A delivered capture snaps the viewport like typed input (D13)

#### R9.1: Delivery snaps a scrolled-back viewport unconditionally
- **Given**: The originating tab's viewport is scrolled back further than `SNAP_FLOOR` (3)
- **When**: A capture is delivered to it
- **Then**: The viewport snaps to the live bottom — the input rule, not the floor-gated output rule

#### R9.2: The snap targets the originating tab, not the active one
- **Given**: The originating tab `T` is scrolled back and a different tab is now active
- **When**: A capture is delivered to `T`
- **Then**: `T`'s viewport snaps and the active tab's viewport is unchanged

### R10: The feature registers as an invisible overlay below rename (D10, C7)

#### R10.1: It is registered in the overlay set at priority 0
- **Given**: The registration lists
- **When**: `feature_overlays(..)` is built
- **Then**: It contains the capture overlay, its priority is `0`, and the priority list stays in
  descending order

#### R10.2: It starts invisible
- **Given**: A freshly constructed capture overlay
- **When**: `visible()` is read
- **Then**: It is `false`, so it never captures keys other than its own chord

#### R10.3: It is not registered as a handler
- **Given**: The registration lists
- **When**: `feature_handlers()` is built
- **Then**: Its length is still 2 — Cmd+V paste and Cmd+C copy

#### R10.4: The new seam method is inert for every other overlay
- **Given**: Each existing overlay (rename, ai-monitor, image presenter, selection presenter)
- **When**: `take_pending_write()` is called on it
- **Then**: It returns `None`, by the trait default, with no change to that overlay's source

### R11: Tab close and app exit have defined, honest behaviour (D12, D6)

#### R11.1: Closing the originating tab does not kill the capture
- **Given**: A capture is in flight for tab `T`
- **When**: `T` is closed while the crosshair is still up
- **Then**: The child is not killed, the capture may complete, and its bytes are dropped per R7.7

#### R11.2: No exit hook is claimed that does not exist
- **Given**: The app's `ApplicationHandler` implementation
- **When**: Its method set is inspected
- **Then**: It defines no `exiting`, and no code attempts to kill a capture child at app exit —
  the documented consequence being that `screencapture` is reparented and its crosshair survives
  until the user presses Esc

## Tasks

> **On parallelism**: only T1 ∥ T2 are genuinely independent — they share no file (`capture.rs`
> versus `seam.rs`/`terminal_app.rs`) and no interface, because the seam carries only
> `(TabId, Vec<u8>)`, both pre-existing types. Everything after that serialises on `capture.rs`.
> The chain is not padded to look wider than it is.

### T1: Path, invocation and outcome model — pure, headless [infra]
- **Fulfills**: R2, R4
- **Depends on**: (none)
- **Files**: `crates/app/src/capture.rs` (new)
- Target-path computation reusing `pasted_dir()` with the `capture-<secs>-<seq>.png` name and the
  per-process `AtomicU64`; the occupied-path and unusable-cache-dir aborts; the fixed argv builder;
  and the outcome type whose success variant carries `(TabId, PathBuf)` while every other variant
  carries none. No threads, no process, no UI — every one of R2 and R4's sub-requirements is a pure
  function test.

### T2: The delivery seam and its drain [infra] — parallel with T1
- **Fulfills**: R7.4, R7.5, R7.6, R7.7, R9, R10.4, R11.2
- **Depends on**: (none)
- **Files**: `crates/app/src/seam.rs`, `crates/app/src/terminal_app.rs`
- `Overlay::take_pending_write(&mut self) -> Option<(TabId, Vec<u8>)>` defaulting to `None`
  (R10.4 — no existing overlay changes), an aggregating `OverlayHost::take_pending_writes()` beside
  `poll_all` since `overlays` is private (`seam.rs:426`), and the drain block in the
  `UserEvent::OverlayPending` arm immediately after `poll_all()` (`terminal_app.rs:2365`), writing
  through `Tabs::by_id_mut` → `Session::write` → `snap_after_input(id)`. Includes the R11.2
  structural assertion that no `exiting` hook exists, and must leave the `poll_all()`-appears-once
  guard (`overlay.rs:5974-5988`) passing.

### T3: Watcher thread, completion verdict and single-flight lifecycle [vertical]
- **Fulfills**: R3, R5, R6, R7.1, R7.2, R7.3
- **Depends on**: T1
- **Files**: `crates/app/src/capture.rs`
- The injectable spawn seam (R5.2 — without it nothing below is testable, because
  `screencapture -i` waits for a human), the watcher thread that owns the deadline so the UI thread
  never does (R5.1), `try_wait` polling to the 5-minute budget with kill-on-expiry, the
  file-not-exit-status verdict including the deliberate R3.4 negative, the in-flight flag clearing
  on all six outcomes, and publication of every outcome over an `mpsc` channel followed by
  `UserEvent::OverlayPending` through a cloned proxy — the `overlay.rs:893-899` shape.

### T4: The chord, the overlay, and the registration [vertical]
- **Fulfills**: R0, R1, R8, R10.1, R10.2, R10.3, R11.1
- **Depends on**: T2, T3
- **Files**: `crates/app/src/capture.rs`, `crates/app/src/lib.rs`, `crates/app/tests/wiring.rs`
- The `Overlay` impl with `visible() -> false` and priority `0`; `Cmd+Shift+G` matched case-folded;
  `Consumed` on **every** path including all three failure paths (R8.2 — `Passthrough` would type a
  literal `g`); `take_pending_write` draining the channel; registration appended in
  `feature_overlays`; and the eight `wiring.rs` assertions updated deliberately — counts,
  priorities, layer count, registration order, initial visibility. Closing this task closes R0.

### T5: Mutation coverage for the capture module [verify]
- **Fulfills**: R0–R11 (coverage gate, not new behaviour)
- **Depends on**: T4
- **Files**: `crates/app/src/capture.rs` (tests)
- `cargo mutants -f crates/app/src/capture.rs -j 4`, then `--list` for the day's total. Record the
  **survivor count**, not a ratio (CLAUDE.md). Treat every survivor as a missing test unless
  provably equivalent, and prefer rewriting a condition that says less than it means over adding an
  `exclude_re`. The at-risk functions are the ones the precedent warns about — a verdict, a budget
  comparison and a flag-clearing path can each be replaced by a constant with the suite still green,
  which is exactly how `tabs.rs` reached 41 survivors.

## External Dependencies

### Pre-work
- **Measure A4 before T1 — it can reopen D11.** Confirm a CLI agent will actually read
  `~/.cache/mac-terminal/pasted/<file>.png`, a path outside its workspace, without refusing. Cheapest
  check: drop a PNG there by hand, paste the absolute path to `claude` in this terminal, see whether
  it reads it. If it refuses, D11 is wrong and the target directory must change before any code
  is written.
- **Measure A1/A2 — whether this feature can run at all on this machine.** Run
  `/usr/sbin/screencapture -i -s -d <tmp>.png` from inside mac-terminal and confirm a file appears.
  That settles A2 (whether the grant is needed when Apple's own binary does the capture) and
  establishes the baseline for A1. Then rebuild with `cargo build --release`, re-run, and see whether
  the grant survives — the unsigned-bundle question from `scripts/bundle-app.sh:4-5`.
- Grant Screen Recording to the app if the above shows it is required.

### Post-work
- Record A1's measured answer in `docs/architecture.md` beside G5's A1/A2 and G6, which is where
  this repo keeps assumptions about external tools it cannot enforce.
- If A1 shows the grant dies on every rebuild, that is a daily-driver friction worth its own spec
  (signing the local bundle) — not a change to this feature.
- Captures accumulate in `~/.cache/mac-terminal/pasted/` with no pruning, exactly as Cmd+V pastes
  already do. Retention stays an explicit non-goal; revisit only if it bites.
