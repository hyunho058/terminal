# Spec: ai-monitor-idle-timestamps

## Meta
- **Created**: 2026-09-17
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-09-17
- **Amended**: 2026-09-17 — review round 4, four fixes applied at their sites (D11, D18,
  R0.3/R0.4, R2.4, T3); see §"Review round 4". The approval stands: none of the four changes
  a decision, and two of them remove a gate that would have made a decision inert.
- **Amended**: 2026-09-17 — post-PR review (PR #41), three fixes at their sites. Two are
  defects: D18's third bypass is **closed** (`Busy` now cancels a pending deadline as well as
  deferring the arm, R2.11) and a re-point no longer carries the previous session's deadline
  into the new one (R2.12). The third is coverage: `TabMetaHandle::set_panel_deadline` was
  never mutation-tested, survived `replace ... with ()`, and the guard written to prevent
  exactly that could not see methods at all. See §"Post-PR review" and `loop.md` L6/L7.
- **Scope of approval**: the plan only, *as written*. The user's standing instruction for
  `/specify` is to stop at an approved spec. **Implementation followed as a separate, explicit
  decision** and shipped in PR #41; this line previously read "nothing here has been
  implemented", which was true when written and false by the time the spec was merged.

## Goal
Let the ai-monitor panel's time-derived values stop going stale while nothing is appending
to the session log, **without** putting the whole window on a repaint timer.

Two scoped changes:

- **Option 1 (no NEW wakes — see D13; "free" was the wrong word).** On a tabmeta-driven
  `UserEvent::OverlayPending`, `Overlay::poll` (`crates/app/src/overlay.rs:1938`) returns
  `false` because the `LogWatcher` had nothing, so `refresh_model()` is skipped — but
  `feed_agent_states()` can return `true` and the window redraws anyway
  (`crates/app/src/terminal_app.rs:2360`). The panel declines a frame it is already paying
  for. Rebuild the panel model on wakes that already cause a redraw.

- **Option 3 (the pill).** `dashboard::status` (`crates/app/src/dashboard.rs:253`) flips
  CONNECTED→IDLE at `IDLE_AFTER_MS = 300_000`, computed against `env.now_ms`. With no wake
  at the five-minute mark the pill keeps reading CONNECTED for a session that has gone
  quiet — the one genuinely misleading value on the panel. **How** it becomes truthful is
  an open question decided at L2, not assumed here.

## Non-goals
- **Option 2 — a coarse heartbeat riding the tabmeta worker's 500 ms tick** to keep the
  second-granular values (UPTIME/IDLE tiles, live agent elapsed) live. Explicitly deferred
  by the user. Not specified, not designed, not left as a TODO in the code.
- **Un-freezing the other four time-derived values**: the UPTIME/IDLE tiles
  (`dashboard.rs:388`), the task `updated Nm ago` detail (`:687`), live agent elapsed
  (`:747`), and the detached `StaleSince` note (`:299`). These stay as they are. They may
  move *incidentally* when a rebuild happens for another reason; that is a side effect, not
  a requirement, and nothing is built to make it happen.
  **The premise this bullet originally rested on was wrong, and the correction matters** —
  see Research §"What the footer actually says". The first draft justified freezing these
  four by saying they are "readings qualified by the footer's absolute `Last update
  HH:MM:SS`", quoting `RP-D20`. But `footer` (`dashboard.rs:494-499`) formats
  `t.last_event_ms`, **not** `env.now_ms` — it states when the *log* last moved, not when
  the frame was computed, so it qualifies nothing by itself. What rescues the four values is
  a different and stronger fact: the IDLE tile is `dur(now_ms − last_event_ms)`
  (`dashboard.rs:387-396`) and the footer is `clock(last_event_ms)`, so **footer + IDLE
  recovers the instant the frame was computed** — exactly while IDLE is under an hour, and to
  ±59 s above it, where `dur()` switches to `{h}h{:02}m`. (When `last_event_ms` is `None`
  both fields render `DASH` and the pair recovers nothing; the panel then has no time claim
  to be inconsistent with.) The panel therefore states its own "now" implicitly, across two
  fields, and every other reading is consistent with it. That is why the four may stay
  frozen, and it survives D3's deadline rebuild for three of the four — see D3's steelman
  and the LIVE-card gap.
- **Any repaint cadence for the window as a whole.** `ControlFlow::Wait`
  (`docs/architecture.md` C6) is not reopened. Whatever Option 3 resolves to, it must not
  cost a frame while the panel is closed.
- **Changing what the panel's `LogWatcher` tails, resolves, or attaches to.** The
  ai-monitor crate is untouched; this is an app-side display concern.

## Confirmed Goal
Two properties, graded separately, because they fail for different reasons and one of them
is a consistency property rather than a freshness one:

1. **The panel's model is never staler than the frame on screen.** Wherever the app already
   decides to redraw the window while the panel is visible, the panel's model was rebuilt
   first. This is Option 1's honest success criterion. On a genuinely quiet machine it
   changes nothing visible — it recycles frames rather than creating them — so grading it on
   *freshness* would misread it as a failure.

2. **The pill never asserts CONNECTED for a session that has been quiet past its threshold.**
   Option 3. The L0 gate admitted a changed *display* — a re-framing pairing the pill with an
   absolute time, or a third state — alongside a bounded deadline. **L2 resolved this to the
   deadline (D3) and the licence went unused (D4)**: the pill keeps CONNECTED / IDLE /
   NO SESSION exactly as today. What is not admissible is leaving a value on screen that is
   actively false rather than merely frozen.

Done when both hold, `cargo test --workspace` and `cargo clippy --workspace --all-targets
-- -D warnings` are green, and the reversal is **written down at all three of its sites**
(D16) rather than routed around.

### Scope note carried from L0 — **superseded by L1 research**
The note written at L0 said the two options interact: that tabmeta's free state-change wakes
at `BUSY_WINDOW = 10s` / `STALE_LIMIT = 100s` already bracket the quiet-session case, so
Option 1 would shrink what Option 3 must carry from "fire at exactly 300 s" to "do not be
wrong for longer than X".

**That is wrong, and L1 disproved it.** Those two windows gate only the *recency fallback*
in `state_from` (`crates/app/src/tabmeta/activity.rs:66`); the precise `AssistantEndTurn` /
`AssistantAsksUser` signals return `Waiting` immediately and never decay. In the common quiet
case the tab settles on `Waiting` and the worker emits **zero** further wakes — so Option 1
gives the pill **no** coverage at all at the 300 s mark.

The two options are therefore **independent**, and Option 3 must be decided on its own
merits. Recorded rather than deleted because the superseded assumption is the one a reader
would otherwise re-derive from the two constants, exactly as this spec's author did.

## Research

### The gap Option 1 closes
- `UserEvent::OverlayPending` has **three** producers across **five** call sites, verified by
  `grep -rn "send_event(UserEvent::OverlayPending)" crates/app/src/`:
  1. the panel's own `LogWatcher` wake — `crates/app/src/overlay.rs:577`, `:595`, `:701`;
  2. the tabmeta worker — `crates/app/src/main.rs:46`;
  3. **`refresh_rivals`' detached probe thread** — `crates/app/src/overlay.rs:806`, the
     `ping` that announces "a `claude` appeared". This one is easy to miss (the first draft
     of this spec did, while quoting `overlay.rs:714`, whose comment names it) and it
     matters: it fires from an unrelated thread on a path unrelated to the panel's content,
     so it is a wake D2 will now rebuild on. Its rate is bounded by
     `rival_refresh_needed` plus the one-in-flight guard (`overlay.rs:785`), and it stops
     entirely once the panel attaches (R6.3).

  All five land in one arm (`crates/app/src/terminal_app.rs:2333`).
- In that arm, `overlays.poll_all()` → `Overlay::poll` (`crates/app/src/overlay.rs:1938`)
  drains the watcher and returns `false` when it carried nothing — so `refresh_model()`
  (`overlay.rs:1947`) is skipped. But `feed_agent_states()` can independently return `true`,
  and the arm redraws on `overlay_changed || waiting_changed`
  (`crates/app/src/terminal_app.rs:2360`). **A frame is drawn with a model nobody rebuilt.**
- `refresh_model` is reached from only four places: `open` (`overlay.rs:708`), `close`
  (`:849`), `toggle_fold` (`:910`), `poll` (`:1947`). A tabmeta wake that does not re-point
  reaches none of them — `notify_active_changed` no-ops behind `Repoint::changed`.

### The decisive finding — the "free wakes" are far rarer than assumed
- `tabmeta::activity::state_from` (`crates/app/src/tabmeta/activity.rs:66`) returns
  `AgentState::Waiting` **immediately** on the precise `AssistantEndTurn` /
  `AssistantAsksUser` signals, and that classification **does not decay with time**.
- `BUSY_WINDOW` (10 s) and `STALE_LIMIT` (100 s) (`activity.rs:44,47`) gate only the
  **recency fallback** — the branch taken when there is *no* robust `last` signal.
- Therefore, in the common quiet case (Claude finished its turn cleanly), the tab settles on
  `Waiting` and **never changes again**. `publish_if_changed`
  (`crates/app/src/tabmeta/worker.rs:360`) then never fires, so the worker emits **zero**
  further wakes. `TabMeta`'s other fields (`branch`, `cwd`, `task`, agent pid —
  `tabmeta/mod.rs:59`) are likewise static on a quiet machine.
- `waiting_changed` is change-gated too: `waiting::on_agent_states`
  (`crates/app/src/waiting.rs:142`) returns whether the waiting *items* moved.
- **Consequence for L2:** Option 1's benefit is bounded to wakes caused by something *else*
  changing (another tab's agent flipping, a branch/cwd change, a waiting-band edge). On a
  fully idle machine it delivers nothing, and it gives Option 3 **no** free coverage at the
  300 s mark. The pill must be decided on its own merits.

### What Option 1 costs (and what it does not)
- Stage one's cache key is a structural `MonitorModel` `PartialEq`
  (`crates/renderer/src/monitor_panel.rs:658-664`; `MonitorModel` derives `PartialEq` at
  `crates/renderer/src/monitor_model.rs:360`). The model is a tree of **already-formatted
  strings**, so a rebuilt model is *equal* — a free cache hit — unless a formatted string
  actually changed.
- `age()` (`crates/app/src/dashboard.rs:1168`) is minute-granular. `dur()` (`:1210`) is
  second-granular **only below one hour** — above that it formats `{h}h{:02}m` and is
  minute-granular too, and `tiles()` renders a constant `DASH` when the timestamp is `None`
  (`:387-390`). So the cache-miss rate is **not** uniform: a session under an hour old
  misses on any rebuild ≥1 s apart (the UPTIME and IDLE tiles both tick), while a session
  over an hour old misses only when a *minute* rolls over, and a session with no parseable
  timestamps never misses on the tiles at all. The honest cost is therefore one
  `dashboard::model` per wake plus a stage-one re-shape **on the subset of wakes where a
  formatted string actually changed** — commonly most of them for a young session, few for
  an old one. D13 accepts this; the first draft overstated it as always.

### What the footer actually says (corrects a premise inherited from `RP-D20`)
- `footer` (`crates/app/src/dashboard.rs:494-499`) is
  `format!("Last update {}", clock(t.last_event_ms, env.utc_offset_secs))` — a function of
  **`last_event_ms`**, not `env.now_ms`. `RP-D20` (`specs/ai-monitor-right-panel/spec.md:481`)
  describes it as "the one timestamp the whole frame was computed at", and the in-code
  comment at `dashboard.rs:383-385` repeats that. **Both are wrong about the shipped code.**
- The reason nobody noticed: before this feature, the last model rebuild was *caused by* the
  last append, so `now_ms ≈ last_event_ms` and the description was accidentally true of
  every frame that could be observed. D3's deadline is the first thing that rebuilds the
  model at a `now_ms` far from `last_event_ms`, which is what exposes it.
- The panel is **not** left unqualified, though, and this is the load-bearing correction:
  the IDLE tile is `dur(now_ms − last_event_ms)` (`:387-396`) and the footer is
  `clock(last_event_ms)`, so **footer + IDLE = the instant the frame was computed**. The
  panel does state its own "now" — implicitly, across two fields. Every frozen reading stays
  consistent with that pair, before and after D3.
- `RP-D9` is at `specs/ai-monitor-right-panel/spec.md:303` and `RP-D20` at `:481` (earlier
  drafts of this spec cited `:305` and `:487`, which land inside their rationales).
- G1's renderer-side guarantee is unaffected: the idle tests
  (`crates/renderer/src/monitor_panel.rs:992`, `:1451`) drive frames with an **unchanged
  model** pushed once. Option 1 adds no frames — it makes frames the app already draws carry
  a fresher model — so those assertions stay green by construction.

### Why the *broad* hook for Option 1 is wrong
- There is a single `redraw()` seam (`crates/app/src/terminal_app.rs:514`) with 24 call
  sites, which looks like the tidier hook. It is not: `redraw()` fires on **every
  `CursorMoved` of a sidebar resize drag**, which `crates/renderer/src/monitor_panel.rs:384`
  names as the exact load the stage-one cache was built to absorb. Rebuilding the model
  there would change it every frame (second-granular tiles) and re-shape the whole document
  per drag frame — reintroducing the R7.3 regression. The hook must be the narrow one.

### Prior art this spec may have to amend
- `specs/ai-monitor-right-panel/spec.md:202` **D2** — rejected a ~2 Hz repaint timer because
  it "breaks G1 and would require rewriting that guarantee in `docs/architecture.md`".
- Same spec, **D9** (`:303`) — the footer went absolute precisely because a relative age
  "freezes and becomes actively false"; **D20** (`:481`) — the footer's absolute timestamp is
  stated as qualifying *every* value on the panel. (That `RP-D20` claim is **wrong about the
  shipped code** — see Research §"What the footer actually says".)
- `crates/app/src/terminal_app.rs:3135` **R7.4** pins D2 at the source level: bans a second
  `set_control_flow`, bans `ControlFlow::Poll` / `WaitUntil` in `main.rs` /
  `terminal_app.rs` / `overlay.rs`, and bans `Duration` / `Instant` / `thread::sleep` /
  `recv_timeout` / `set_timeout` from `overlay.rs`'s production half.
- **Stale citation to fix if D2 is amended:** D2's rationale cites
  `overlay_layer.rs:674-724` for the G1 guarantee. **That file no longer exists** — the
  guarantee now lives in `crates/renderer/src/monitor_panel.rs:992` and `:1451`. Anyone
  following D2's evidence today lands on nothing.
- `overlay.rs:714` ("The cadence, corrected") documents that `OverlayPending` fires per
  append burst while writing and not at all when idle — and `:728` records that a debounce
  clock was *removed* from this module once already.

## Decisions

> **Numbering.** `D<n>` below are **this spec's** decisions. The ai-monitor right-panel
> spec's decisions are referred to as **`RP-D<n>`** throughout, because this spec amends
> `RP-D2` and an unqualified "D2" would be ambiguous in exactly the place it matters most.

### D1: Option 1 hooks the **narrow** site — the `OverlayPending` arm, not the `redraw()` seam
- **Status**: assumed (auto-resolved from L1)
- **Rationale**: The single `redraw()` seam (`crates/app/src/terminal_app.rs:514`, 24 call
  sites) is the tidier-looking hook and is **wrong**: it fires on every `CursorMoved` of a
  sidebar resize drag, which `crates/renderer/src/monitor_panel.rs:384` names as the exact
  load stage one's cache was built to absorb. Because the UPTIME/IDLE tiles tick every second below an
  hour (`dur()`, `dashboard.rs:1210-1220`), a rebuild there would differ on most frames of a
  drag and re-shape the whole document — reintroducing the R7.3 regression.
  **Rejected**: the `redraw()` seam (above); rebuilding inside the renderer (it would put
  wall-clock time behind the `GridSnapshot`/model contract, which `dependency-direction`
  and RP-D20 both place on the app side).

### D2: Rebuild the model on every `OverlayPending`; **redraw iff it differs**
- **Status**: resolved (**revised at round 2** — see the superseded form below)
- **Rationale**: The model is the authority on whether anything the user can see has moved,
  because it is a tree of already-formatted strings and it is *already* stage one's cache
  key (`crates/renderer/src/monitor_panel.rs:658`). So the diff that decides the redraw is a
  comparison the renderer would otherwise pay for one frame later. Rebuilding
  unconditionally and letting the diff gate the frame means a model-only change — which is
  exactly what D3's deadline produces — earns its frame with **no special case at all**, and
  any future time-derived value inherits the same treatment.
  **Rejected**: (a) the superseded gate below; (b) having `poll()` also return "my deadline
  passed", folding into the existing `overlay_changed` signal — it works and preserves the
  gate, but leaves two mechanisms (a gate *and* a deadline) where one suffices, and the
  consistency property of D1 still needs its own path; (c) a distinct
  `UserEvent::PanelDeadline` variant (`terminal_app.rs:73`) — most explicit at the type
  level, but it adds a variant for one edge and its arm must still resolve which tab the
  deadline belongs to, which D8 has to answer anyway.
- **Superseded form (round 1):** *"The rebuild runs only where a frame is actually going to
  be drawn"* — rejected because Tier-3 detection showed it **defeats D3**: a deadline wake
  changes only the model, so `overlay_changed` and `waiting_changed` are both `false`, the
  arm draws nothing, and the pill never flips. Recorded because the gate is the intuitive
  design and would be re-proposed on sight.
- **Cost, stated honestly:** one `dashboard::model` per `OverlayPending`. On a hot append
  burst this is **not** new — `poll()` already rebuilds there. On a tabmeta wake it is one
  extra build. It is *not* free and this spec does not claim it is; see D9 for the
  short-circuit that bounds what a no-change rebuild costs beyond the build itself.
- **The one gate that survives the revision: `self.visible`.** "Every `OverlayPending`" means
  every one *while the panel is open*. A closed panel rebuilds nothing, which the Constraints
  require and which the round-1 revision must not be read as overturning — the gate D2
  rejected was a gate on *whether a frame is coming*, not on whether the panel exists. Stated
  explicitly because D2's rationale argues against gating in general, and an L3 writer would
  otherwise have to guess which gate was meant to die.

### D6: The one wake is produced by the **tabmeta worker's existing 500 ms tick**
- **Status**: resolved
- **Rationale**: The worker thread (`crates/app/src/tabmeta/worker.rs:28`) already loops
  every 500 ms and already owns an `EventLoopProxy` wake; it simply stays silent unless a
  `TabMeta` changed (`publish_if_changed`, `:360`). Giving it a registered deadline to check
  on a tick it is already running costs **zero new threads and zero new timers**, leaves
  `ControlFlow::Wait` (`docs/architecture.md` C6) untouched, and — decisively — lets R7.4's
  ban on `Duration`/`Instant`/`thread::sleep` **inside `overlay.rs`** stay fully intact, so
  the amendment D5 has to make is far narrower than it first appeared. Granularity is
  ±500 ms against a 300 000 ms threshold (0.17 %), which is below the resolution of the
  thing being reported.
  **This is not the deferred Option 2.** Option 2 was a *heartbeat* — a wake every tick to
  keep second-granular values live. This is **one edge**: the worker emits nothing on the
  other 599 ticks of a five-minute wait.
  **Rejected**: (a) a one-shot `thread::spawn` + `sleep` in `overlay.rs` — self-contained,
  but it needs exactly the tokens R7.4 bans from that module (the widest amendment of the
  three) and it needs a cancellation story, since a panel closed or re-pointed mid-sleep
  must not have a stale wake flip a live session to IDLE; (b) `ControlFlow::WaitUntil` — no
  thread, but window-wide, must be re-set every loop iteration (which defeats R7.4's
  single-setter count), and it is the precise mechanism C6 and R7.4 name and ban.

### D7: Registration reaches the worker through the existing `Cmd` channel, behind an **injected callback**
- **Status**: resolved (**expanded** — the first form named the channel but not the route to
  it, and an L3 writer would have had to invent two things)
- **Rationale**: Two halves, and the second is the one that was missing.
  **(a) The message.** The worker already takes commands over `Sender<Cmd>`
  (`crates/app/src/tabmeta/mod.rs:192` — `SetRoster` / `SetActive` / `SetEnabled`), with a
  public setter per variant (`:230`, `:237`, `:243`). A deadline is the same shape of fact,
  so it becomes one more variant plus one more setter, `TabMetaHandle::set_panel_deadline`,
  rather than a new channel, a shared atomic or a lock.
  **(b) The route.** `Cmd` is `pub(crate)`, `TabMetaHandle.tx` is private, and
  `AiMonitorOverlay` holds **no** `TabMetaHandle` and no `Sender` (verified:
  `grep -c 'TabMetaHandle\|tabmeta::global' crates/app/src/overlay.rs` → 0). So the overlay
  needs a route, and the three candidates are **not** equivalent:
  - `tabmeta::global()` (`mod.rs:389`) — **rejected.** Its own doc says it "Returns `None`
    before `install` (e.g. in tests, which never touch the global)", so the entire arming
    path would be unreachable under `cargo test`, colliding head-on with the Constraint that
    every touched file survive `cargo mutants`.
  - threading a `TabMetaHandle` through `AiMonitorOverlay::with_waker` and `feature_overlays`
    — workable, but `docs/architecture.md` singles that registration set out as "a value a
    test can assert", so changing its signature is an architectural decision, and it couples
    the overlay to the whole handle when it needs one verb.
  - **chosen: a narrow injected callback**, exactly the move `Session::spawn` already makes
    with `session::Wake` — the architecture doc describes that as "the same move the renderer
    makes with `raw-window-handle`, one layer up: it keeps the whole input→shell→grid path
    constructible without an event loop". The overlay takes an `ArmDeadline` callback at
    construction; production wires it to `handle.set_panel_deadline`; a test passes a closure
    that records the arming calls. The overlay then needs no `TabMetaHandle`, no global, and
    no knowledge that a worker exists.
  **Rejected**: an `AtomicU64` on `TabMetaShared` (lock-free and tempting, but it gives the
  worker no way to distinguish "disarmed" from "armed at 0" without a sentinel, and it
  bypasses the command ordering the `Cmd` channel already guarantees).

### D3: The pill gets **one bounded, one-shot deadline** — not a cadence
- **Status**: resolved
- **Rationale**: L1 established that on the common quiet path (`AssistantEndTurn` →
  `AgentState::Waiting`, which never decays) the app receives **zero** further wakes, so no
  existing signal can flip the pill at the 300 s mark. A single edge — armed at
  `dashboard::idle_deadline_ms(last_event_ms)` (D15) while the panel is visible, fired once, then disarmed —
  delivers the truthfulness property at a cost of exactly **one wake per quiet session,
  only while the panel is open**. That is categorically different from the ~2 Hz repaint
  cadence `RP-D2` rejected, and the difference is what D5 must write down.
  **Rejected**: (a) re-framing the pill to state `LAST LINE 16:21:18` — truthful and
  clock-free, but it duplicates the footer's timestamp, which `RP-D20` already established
  as the panel's one absolute time, and it destroys the at-a-glance liveness badge `RP-D5`
  designed; (b) a third state derived from `LastMessage` (e.g. `TURN ENDED`) — also
  clock-free and it reuses `ai_monitor::activity::tail_state`, but it discards the
  five-minute threshold concept entirely and answers "what was the last line" rather than
  "is this session live", which is a different question from the one the pill asks.
- **Steelman against this decision, and the answer.**
  *An earlier draft used a weaker counterargument — "the pill is decorative, the footer says
  it one line below, and this codebase has removed a clock twice" — and answered it by
  appeal to `RP-D5`'s at-a-glance intent. That version is kept in mind but is not the real
  objection: it can be settled without revisiting a single decision in this spec, which is
  the tell that it is the convenient one.*

  **The real objection.** D3 does not deliver "one wake that fixes the pill". It delivers
  **one manufactured wake that advances every time-derived value on the panel by five
  minutes, once, and then lets them freeze again** — including the four the Non-goals
  explicitly disclaim. The deadline wake triggers a full `dashboard::model` rebuild (D2) and,
  per D13, a stage-one re-shape; that rebuild recomputes UPTIME, IDLE, `updated Nm ago` and
  live-agent elapsed against a `now_ms` five minutes newer. The redraw gate passes *because*
  those disclaimed tiles moved. Option 1's incidental movement at least rides on wakes caused
  by real external change; D3's wake is manufactured for the pill alone and drags four
  out-of-scope values with it. "Not a requirement" is not the same as "not this decision's
  consequence" — and the two cures are Option 2 (deferred by the user) and coarsening the
  tiles (rejected by D13 as scope creep), so D3 would be creating a defect whose only
  remedies this spec has already ruled out.

  **The answer, which required checking rather than asserting.** The premise that the panel
  becomes *incoherent* is false, and the correction is Research §"What the footer actually
  says". The IDLE tile is `dur(now_ms − last_event_ms)` and the footer is
  `clock(last_event_ms)` (`dashboard.rs:387-396`, `:494-499`), so **footer + IDLE recovers the
  instant the frame was computed** — exactly below an hour, to ±59 s above it, and not at all
  when `last_event_ms` is `None`. After the deadline fires at T+5m the panel reads
  `Last update 16:21:18` / `IDLE 5m00s` / pill `IDLE` — three fields that reconcile to
  16:26:18 exactly, and a reader can recover the frame's `now` from any two of them. The
  disclaimed values move **to more accurate values**, once. Three of the four stay mutually
  consistent (UPTIME's `first_event_ms` appears nowhere else on the panel; the task rows are
  `age()`-coarse but never contradictory; the detached `StaleSince` note is numerically the
  *same quantity* as the IDLE tile). **The fourth does not** — see the LIVE-card gap in
  Known Gaps, which is the one place the reviewer's objection survives in narrowed form. So D3 does not trade a coherent frozen panel for an incoherent fresher one; it
  trades a coherent frozen panel for a coherent *less stale* one, and the residual freeze
  after T+5m is exactly the deferred Option 2's territory, unchanged in kind from today.
  What the objection does correctly establish is that the Non-goals' original justification
  was wrong (it cited a qualifier that does not exist), which is why that bullet and
  `RP-D20`'s description are corrected rather than quietly relied on.

### D4: The pill keeps its existing CONNECTED / IDLE / NO SESSION vocabulary
- **Status**: resolved
- **Rationale**: Follows from D3. The L0 gate explicitly opened the display to change if
  that bought a clock-free route; D3 declined those routes on their own merits, so the
  licence goes unused and the pill's rendering, colours and `PanelStatus` variants are
  untouched. Recorded rather than left implicit so nobody re-opens the display question
  while implementing a timing change.
  **Rejected**: adding a variant "because we are in here anyway" — it would change
  `PanelStatus`, its colour mapping and its tests for no requirement.

### D5: `RP-D2` and the R7.4 test are amended **in place**, with the reasoning rewritten
- **Status**: resolved
- **Rationale**: D3 reverses part of an approved decision. The project's standing rule is
  that a reversal is argued at its site rather than routed around (see the reversals already
  recorded in `docs/architecture.md` G4 for the panel consuming keys and drawing a chord
  hint). Concretely: `specs/ai-monitor-right-panel/spec.md:202` gains the distinction
  between a *cadence* and a *bounded edge*, and the R7.4 scan list
  (`crates/app/src/terminal_app.rs:3135`) is edited to match. Two things ride along:
  (1) **the dead citation is fixed** — `RP-D2` cites `overlay_layer.rs:674-724` for the G1
  guarantee and that file no longer exists; the guarantee now lives at
  `crates/renderer/src/monitor_panel.rs:992` and `:1451`, so anyone following `RP-D2`'s
  evidence today lands on nothing; (2) **the test must still pin something** — see D12, and D16 for the two further amendment
  sites this decision's first draft missed.
  **Rejected**: editing only the test's scan list and recording the reversal solely here
  (the two specs would then disagree about whether a clock is allowed, and the older,
  more-read one would be the wrong one).

### D8: One deadline slot, replaced on arm and cleared on close — **no generation token**
- **Status**: resolved (**rewritten** — the first form specified a token; it had no transport
  and guarded a hazard that cannot occur)
- **Rationale**: The worker holds **at most one** deadline. Arming overwrites it; `close()`
  and a re-point send `None`. That is the whole protocol.
  **Why the token was dropped, recorded because it looks prudent and is not:**
  1. **It had no transport.** `UserEvent` (`crates/app/src/terminal_app.rs:72-82`) derives
     `Clone, Copy` and `OverlayPending` is a **unit variant carrying no payload**. A token
     "carried by the fired wake" has nothing to travel in. D2's rejected alternative (c)
     closes the `PanelDeadline` variant and D7 closes the shared atomic, so all three
     transports were already ruled out and no fourth was named.
  2. **It guarded an impossible hazard.** The token's stated purpose was stopping a late
     wake from flipping a **live** session to IDLE. Under D10 the pill is a pure function of
     `(TelemetrySnapshot, Env)` recomputed from scratch at rebuild time — a spurious wake
     causes a rebuild that reads the *current* session's `last_event_ms` against the
     *current* `now_ms` and returns CONNECTED. **A late wake cannot produce a wrong value.**
     Its entire cost is one unnecessary `dashboard::model` + `PartialEq`, which D9 already
     short-circuits to nothing when the model is unchanged.
  3. **Its supporting claim was false.** The first form called this "the module's existing
     idiom", citing `refresh_rivals` (`overlay.rs:785`). That is
     `if self.refreshing.swap(true, ..) { return; }` — an in-flight **mutual-exclusion
     flag** ("don't start a second worker"), not a generation token ("ignore a reply
     belonging to an older request"). Different shape, different failure mode. The app's
     actual generation idiom is `band_gen` vs `tabmeta_snapshot.generation()`
     (`terminal_app.rs:2350-2356`), in another module.
  **What the single slot still has to get right** is worker-side replacement, not staleness:
  an arm that arrives while a deadline is pending must **replace** it, so a session that
  resumes and re-quiets does not leave two deadlines racing. That is the real requirement
  and it is what L3 must test.
  **Rejected**: a per-`TabId` deadline map — symmetric with how the worker keys `TabMeta`,
  but only one panel is ever visible, so it would maintain N entries to use at most one, and
  "which tab is the panel on" is the overlay's fact, not the worker's.

### D9: `refresh_model` diffs **first** — the ladder, anchor and badge run only on a change
- **Status**: resolved
- **Rationale**: D2 rebuilds per wake, but `refresh_model` today does much more than build a
  model: it walks `item_ladder` (`crates/renderer/src/monitor_layout.rs:281`), builds a
  `RungKey` per rung (six `String` allocations for headings alone —
  `crates/app/src/overlay.rs:58`), resolves the scroll anchor and counts the new-items badge.
  None of that produces anything when the model turns out unchanged. Gating on the model
  comparison puts the cost where the result is: a no-change wake costs one
  `dashboard::model` plus one `PartialEq`; a real change costs exactly what `refresh_model`
  costs today, not a byte more.
  Correctness is unaffected either way — an equal model yields equal keys, hence `badge = 0`
  and an unmoved anchor — so this is purely about not paying for a computed nothing on every
  wake of a streaming session.
  **Rejected**: (a) splitting `refresh_model` into `compose` + `adopt` so the gate is
  structural rather than an early return — genuinely more robust against a later edit
  slipping past the gate, but more churn in a module this spec is already amending, and the
  early return is directly under test; (b) no short-circuit — least code, and precisely the
  invisible per-wake cost G1 exists to keep out.

### D10: `dashboard.rs`'s **behaviour** is not modified; it gains one pure exported function
- **Status**: resolved (**corrected** — the first form said "not modified", which made this
  decision jointly unsatisfiable with D14; see D15)
- **Rationale**: The pill's rule is right. `dashboard::status`
  (`crates/app/src/dashboard.rs:247-255`) computes the correct answer for any `env.now_ms` it
  is given, including at the boundary. The **only** defect is that on a quiet session nobody
  calls it again.
  **One qualification, added at L2 review round 2, because the first form of this sentence
  was false.** It said "nothing about the rule, the threshold or the formatting is wrong"
  full stop. The threshold's adequacy is *conditional*, and `IDLE_AFTER_MS`'s own doc comment
  (`dashboard.rs:188-192`) states the condition: five minutes is safe because a long
  `cargo build` or deep subagent run "leaves a genuinely working session silent for minutes"
  **and** because "under D2 nothing repaints on a timer to blink it back". D3 removes the
  second leg. The threshold is therefore not unconditionally right — it is right given no
  repaint at the boundary, which is exactly what this feature introduces. **D18 restores the
  condition** rather than re-opening the threshold.
  So no `Env` field, no new `PanelStatus` variant (D4), no change to the value of
  `IDLE_AFTER_MS`, and no change to any output for any input.
  **The correction:** `IDLE_AFTER_MS` is `const IDLE_AFTER_MS: u64 = 300_000;`
  (`dashboard.rs:193`) — **private**, with no `pub` or `pub(crate)`. `overlay.rs` is a
  sibling module and cannot name it. The first form of this decision forbade touching
  `dashboard.rs` at all while D14 required the overlay to arm at
  `last_event_ms + IDLE_AFTER_MS`, leaving the feature's central arithmetic with no legal
  home: exporting the constant broke this decision, and duplicating `300_000` in `overlay.rs`
  broke this decision's own rejected-alternative clause. D15 resolves it by exporting a
  *function* instead of the constant, which keeps the rule in the module that owns it.
  **Rejected**: moving the threshold comparison into the overlay or the worker so the
  "deciding" code sits next to the "waking" code — it would split one rule across two
  modules and make `dashboard::model` no longer a pure function of its `Env`, which is the
  property its whole test suite is built on.

### D15: `dashboard` exports `idle_deadline_ms` — defined as *the first instant at which `status` returns IDLE*
- **Status**: resolved (fixes the D10 ∧ D14 conflict and the boundary off-by-one together)
- **Rationale**: One pure exported function, in the module that owns the rule:
  `pub fn idle_deadline_ms(last_event_ms: u64) -> u64`. The constant stays private; the
  overlay never learns the threshold, only the instant.
  **This also fixes a real off-by-one that would have made the feature inert.**
  `dashboard.rs:253` is `Some(last) if env.now_ms.saturating_sub(last) <= IDLE_AFTER_MS =>
  PanelStatus::Connected` — so IDLE requires **strictly greater than** 300 000. Arming at
  exactly `last + IDLE_AFTER_MS`, as D14 originally said, fires a wake at which the pill is
  **still CONNECTED**; combined with D3's one-shot and D11's "unchanged `last_event_ms`
  re-arms nothing", that burns the single deadline, changes no state, re-arms nothing, and
  leaves the pill reading CONNECTED **for the rest of the session** — precisely the defect
  the feature exists to remove. Improbable in the field thanks to the ±500 ms tick, and
  **certain** in the deterministic worker test D12 requires.
  Defining the function by its postcondition rather than by an expression is what keeps the
  two in step: it returns the smallest `now_ms` for which `status` yields `Idle`, and L3
  requires a test asserting exactly that against `status` itself, so the pair cannot drift.
  This is `CLAUDE.md`'s "the condition says less than it means — prefer rewriting it"
  applied to a boundary rather than to a mutation survivor.
  **Rejected**: (a) exporting `IDLE_AFTER_MS` as `pub(crate)` and doing the arithmetic in
  `overlay.rs` — one line, and it puts a boundary rule in a module that does not own it,
  where the `<=` would have to be re-derived by whoever next reads it; (b) tightening
  `dashboard.rs:253` to `<` so that `last + IDLE_AFTER_MS` becomes exactly right — it changes
  shipped behaviour at one millisecond of the threshold for no requirement, which D10 exists
  to prevent.

### D11: The deadline is re-armed from **every** model rebuild that moves `last_event_ms`
- **Status**: resolved
- **Rationale**: A session that resumes after going quiet must return to CONNECTED and then
  be able to go IDLE *again*. Arming once at `open()` would give each session exactly one
  correct transition and silently none thereafter. Because D2 already rebuilds the model on
  every wake, the arming point is the same place: whenever the rebuild sees a newer
  `last_event_ms` (D14.3 defines "newer" over the `Option`), it re-arms to
  `dashboard::idle_deadline_ms(last_event_ms)` — **not** to `last_event_ms + IDLE_AFTER_MS`,
  which is the off-by-one D15 exists to kill, and there is no token to bump (D8). A rebuild
  that finds `last_event_ms` unchanged re-arms nothing, so a streaming session sends one
  `Cmd` per *burst*, not per line.
- **TWO triggers, not one — and the second is what stops D18 making the feature inert.**
  Added at L2 review round 3. The arming inputs arrive on **different wakes at different
  cadences**: `last_event_ms` comes from the panel's own `LogWatcher` via `Overlay::poll`
  (`overlay.rs:1938`), documented at `overlay.rs:718-719` as firing "per append burst, much
  hotter than 500 ms", while `AgentState` comes from the worker's *published* state via
  `feed_agent_states` (`terminal_app.rs:1077-1099`) on the 500 ms tick, further gated by
  `publish_if_changed`. So on a clean turn ending the watcher wins the race: `end_turn` lands,
  `last_event_ms` moves, D11's trigger fires — and the overlay's `AgentState` is still the
  stale `Busy`, so D18 skips the arm. Up to 500 ms later the worker publishes
  `Busy → Waiting`, D2 rebuilds, but `last_event_ms` is now **unchanged**, so a
  single-trigger D11 arms nothing and **the pill stays CONNECTED forever** — on the common
  quiet path, which L1 named as this feature's primary target.
  The arming rule is therefore: **arm when the rebuild sees a newer `last_event_ms`, OR when
  the tab's `AgentState` leaves `Busy` while `last_event_ms` is already set.** Equivalently:
  D18's gate **defers** the arm rather than dropping it, and the deferred arm executes on the
  `Busy →` transition. This is structurally the same failure D2's
  superseded round-1 form had — a second gate that defeats D3 — and is recorded the same way
  so the single-trigger version is not re-proposed on sight.
- **"…and no deadline is pending" was in the round-3 form of that rule and is now dropped —
  the overlay cannot evaluate it.** Added at round 4, and it is the *third* instance of this
  spec's recurring failure: a gate added to bound a cost that also blocks the path it was
  meant to leave open. D8 established that `UserEvent::OverlayPending` is a **unit variant
  carrying no payload** (`terminal_app.rs:72-82`), so the overlay is never told that its
  deadline fired. A `pending` flag it kept would therefore be set on the first arm and
  **never cleared**, and trigger two would run exactly once per panel-open and never again.
  What actually bounds the arming rate is the **edge**, not a flag: trigger two fires on a
  `Busy → not-Busy` *transition* (R2.4), not on every non-`Busy` wake, so a quiet tab arms
  once and then sends nothing however many wakes pass over it.
  And a redundant arm is **idempotent** rather than merely cheap: the armed instant is always
  `idle_deadline_ms(last_event_ms)` for the *current* `last_event_ms`, which only ever moves
  forward (D14.3), so re-arming writes the value already in the slot and R3.3 makes that a
  replace. There is no state for a second arm to corrupt — which is the same argument D8 used
  to drop the generation token, applied to the other end of the same wire.
  **Rejected**: defining "pending" as `armed_deadline > env.now_ms` — genuinely evaluable, but
  it puts a second reading of the clock at the arming site to save at most one `Cmd` per
  `Busy →` transition, and `Cmd` sends are already bounded to one per burst.
- **Implementation note.** `on_active_changed` (`overlay.rs:1976-1992`) computes
  `self.repoint.changed(...)` (`:1983`) and then dispatches through `tick_action` (`:1992`),
  which no-ops on every wake where the tab did not change — i.e. every wake in the quiet case.
  The incoming `AgentState` must be stored **above** that dispatch, in the same window where
  `self.last_seen_pid` is updated (`:1988-1990`), or it is silently dropped exactly when it is
  needed.
  **But not in the same *shape*:** that update is `if agent_pid.is_some() { … }`, the
  no-information rule `seam.rs:305-311` states for the pid. The state is stored
  **unconditionally**, because for it an absent value is the meaningful one — D18 argues that
  divergence at its site, and mirroring the pid's guard here is the single edit that would
  silently reintroduce the permanent suppression D18 rejects.
  **Rejected**: arming only at `open()` (one transition per open, then wrong forever);
  re-arming on every rebuild regardless of whether `last_event_ms` moved (a `Cmd` per wake
  on a streaming session, for an identical value).

### D12: R7.4's **scan list needs no edit** — the amendment is prose, a citation fix, and one new test
- **Status**: resolved
- **Rationale**: Checked rather than assumed, and the result is much narrower than D5
  anticipated. R7.4 scans `main.rs`, `terminal_app.rs` and `overlay.rs`
  (`crates/app/src/terminal_app.rs:3135`). D6 puts the clock in
  `crates/app/src/tabmeta/worker.rs` — not scanned, and already holding a `Duration` for its
  own `TICK`. The overlay passes a plain `u64` instant obtained from
  `dashboard::idle_deadline_ms` (D15) through an injected callback (D7) — no `Duration`, no
  `Instant`, no `sleep` — so `overlay.rs` stays clean of every banned token, and the ban keeps
  its full force. `ControlFlow::Wait` and the single-setter count are untouched by D6.
  What the amendment **does** consist of: (1) `RP-D2`'s prose gains the cadence-vs-bounded-
  edge distinction and the fixed G1 citation (D5); (2) a **new** test pins the property
  R7.4's source scan structurally cannot see — that an armed deadline yields **exactly one**
  wake however many ticks pass over it. That is the invariant that would silently become a
  heartbeat, and a token scan cannot detect it, which is precisely why it needs a behavioural
  test on the worker's tick rather than another banned string.
  **Rejected**: deleting tokens from the scan list "to make room" for the feature — it would
  leave the test passing while protecting less, the exact failure mode `CLAUDE.md` names for
  `exclude_re` entries in the mutation-testing guidance.

### D13: The per-wake re-shape is **accepted and named**, not engineered around
- **Status**: resolved (from the Inversion Probe)
- **Rationale**: The UPTIME/IDLE tiles are second-granular **only below one hour** — `dur()`
  formats `{h}h{:02}m` above that (`dashboard.rs:1210-1220`) and `tiles()` renders a constant
  `DASH` for a `None` timestamp (`:387-390`). So for a young session almost any two wakes a
  second apart produce a different model and D9's gate fires only on sub-second bursts; for a
  session over an hour old the gate holds until a minute rolls over. The re-shape rate is
  therefore session-age-dependent, not uniform, and the worst case is the young-session one. This is the honest cost of D2 and it
  is accepted: **no new wakes are created**, the panel must be open, and absorbing a
  re-shape is what the two-stage cache exists for. G1's renderer-side guarantee is untouched
  either way, because its tests drive frames with an *unchanged* model
  (`crates/renderer/src/monitor_panel.rs:992`, `:1451`) and this adds no frames.
  **The framing correction that belongs in the code comment:** "free refresh" was never
  free. The accurate claim is **"no new wakes"**, which is a different and weaker statement,
  and writing the weaker one down is what stops a later reader from trusting the stronger one.
  **Rejected**: (a) coarsening the two tiles to `age()` granularity so the gate actually
  bites — defensible, and arguably *more* honest given `RP-D20` calls them readings rather
  than counters, but it changes the display of two values this spec's Non-goals put out of
  scope, and scope creep into a deferred area is how the deferral stops meaning anything;
  (b) a second, hand-maintained "semantic" equality used for the redraw gate while
  `PartialEq` stays the cache key — it keeps both the seconds and the gate, and it is the
  exact failure class `monitor_panel.rs:67` already warns about ("both halves or neither"):
  two equality notions that disagree leave the panel showing a model it never re-shaped,
  with nothing erroring.

### D14: Arming rules at the boundaries
- **Status**: resolved (from the Inversion Probe; no user input needed — one defensible
  answer each)
- **Rationale**: Two edges that a naive arming rule gets wrong, decided here so they are
  requirements rather than discoveries:
  1. **A deadline already in the past is not armed.** Opening the panel at 16:40 onto a
     session last written at 16:21 makes `status()` return IDLE on the spot — the model is
     correct with no wake at all. Arming unconditionally would queue an immediately-due wake
     to recompute a value that is already right.
  2. **The deadline is armed from the same `last_event_ms` the predicate reads** — via
     `dashboard::idle_deadline_ms(last_event_ms)` (D15), never from "now". When a burst lands
     carrying timestamps older than wall-clock now (a batched write), arming from that value
     keeps fire-time and predicate self-consistent: at the instant the wake fires, `status`
     returns `Idle` **by D15's definition**. Arming from "now" would fire late by the batch's
     age, leaving the pill asserting CONNECTED past its own threshold — the very defect being
     fixed. *(The earlier draft stated this invariant as `now_ms - last_event_ms >=
     IDLE_AFTER_MS`, which is wrong at equality against `dashboard.rs:253`'s `<=`; D15
     replaces the arithmetic with the postcondition so the two cannot disagree.)*
  3. **`last_event_ms` is `Option<u64>`** (`crates/ai-monitor/src/telemetry.rs:353`), so
     "newer" has to be defined over the `Option`, not just over the number — the third member
     of exactly the class this decision exists to pre-decide:
     - `None → Some(t)` (a brand-new session's first line): **arm** at
       `idle_deadline_ms(t)`. This is the ordinary first arming.
     - `Some(a) → Some(b)` with `b > a`: **re-arm**. With `b <= a`: leave the pending
       deadline alone (a re-ordered or duplicate record must not push the deadline out).
     - `Some(_) → None` (a re-point to a log with no parseable time): **disarm.** `status`
       already maps a `None` timestamp to IDLE (`dashboard.rs:252-254`), so the model is
       correct with no wake, and a deadline computed from a timestamp that no longer exists
       has nothing to assert.
  4. **No deadline is armed while the snapshot is detached.** `detached` is decided separately
     (`dashboard.rs:540-549`) and `status` returns `NoSession` whenever it is `Some`, never
     `Connected` (`:248-250`) — so the pill cannot be the thing a deadline would correct. Left
     unstated, the overlay would arm for a detached session, the wake would fire, `status`
     would be unchanged, but the tiles and the `StaleSince` note would have moved, so the
     model differs and a redraw happens: one wasted wake plus one wasted stage-one re-shape
     per detached quiet session. Same class as rule 1 — do not arm when the answer is already
     right.

### D16: The **three** amendment sites, not one — `wake()` stops meaning "a `TabMeta` changed"
- **Status**: resolved (**added** — D5 and D12 between them listed one site and missed two)
- **Rationale**: D5's stated principle is that a reversal is argued **at its site**. D6 has
  three sites, and the first draft enumerated only the first:
  1. **`RP-D2`'s prose** (`specs/ai-monitor-right-panel/spec.md:202`) — gains the
     cadence-vs-bounded-edge distinction, plus the dead-citation fix (`overlay_layer.rs`
     no longer exists; G1's guarantee is now at `crates/renderer/src/monitor_panel.rs:992`
     and `:1451`).
  2. **`publish_if_changed`'s doc comment** (`crates/app/src/tabmeta/worker.rs:356-359`).
     Verified: `wake()` has **exactly one call site** in that file (`:379`), inside
     `publish_if_changed`, whose comment says publish "and — crucially — do not `wake` (an
     unconditional wake would force a full grid re-clone at 2 Hz forever, reintroducing the
     G1 idle-CPU debt)". D6 adds a **second** `wake()` site that fires with no generation
     bump, so after this feature `wake()` no longer implies "a `TabMeta` changed". That
     implication is load-bearing elsewhere — `overlay.rs:719-721` reasons about cadence from
     it — so leaving the comment as-is would make the file assert something false about
     itself.
  3. **R7.4's own part-2 comment** (`crates/app/src/terminal_app.rs:3185-3188`): "Its only
     two redraw triggers are `on_key` (a keystroke) and `poll` returning true (a watcher
     update that actually carried a new snapshot)". D2 falsifies this directly — the model
     is now rebuilt on wakes that carried no snapshot at all.
  **Unstated precondition, now stated:** the worker's tick only exists while the roster is
  non-empty. `run` (`crates/app/src/tabmeta/worker.rs:298`) blocks on `rx.recv()` (`:309`,
  guarded by `if roster.is_empty()` at `:307`) with
  **no timeout** when the roster is empty — "zero wakeups, zero syscalls until a roster
  arrives (D4)". D6's "a tick it is already running" is conditional on that. Benign in
  production (a panel is open on a tab, so the roster is non-empty), but it is a precondition
  of the whole mechanism and it belongs in writing rather than in a reader's inference.

### D17: A rebuild whose only delta is a clock-derived string must not move the badge or the anchor
- **Status**: resolved (**added** — the cross-decision tension D9 asserted away)
- **Rationale**: D9 argued that correctness is unaffected because "an equal model yields
  equal keys, hence `badge = 0` and an unmoved anchor". That covers only the **equal**
  branch — and D13 simultaneously declares the *unequal* branch the common one. In the
  unequal branch, `refresh_model` (`crates/app/src/overlay.rs:927-975`) runs `resolve_anchor`,
  re-arms `self.anchor` (`:959-970`), recomputes the new-items badge via `count_new_above`
  (`:972`) and overwrites `self.keys`. Before this spec those ran only when the watcher
  delivered **content**. After D2 they run **on time** — on a wake whose only delta is a tile
  string ticking from `4m03s` to `4m04s`.
  The requirement, which L3 must test rather than assume: *a rebuild in which only
  status/tile/footer strings changed leaves the badge at its previous value and a `Held`
  anchor's index unchanged.* Nothing in the design proves key-set stability under a tile-only
  change — `rung_key` (`overlay.rs:90`) is built per rung from what the rung displays, keyed as
  `RungKey` (`:58-65`), and
  whether the tiles participate in any rung's key is exactly the thing to pin. A silent
  badge that counts "new items" every time a clock ticks would be a worse defect than the one
  being fixed, and it would look like a feature.
  **Fold state is fine by inspection** and is recorded so the next reader does not re-derive
  it: `self.folds` is stamped back onto the fresh model (`overlay.rs:941-946`) independently
  of the model's contents, so a tile-only rebuild cannot disturb it.

---
### D18: No deadline is armed while the tab's agent is **Busy**
- **Status**: resolved (**added at L2 review round 2** — closes the regression D3 would
  otherwise introduce into the scenario `IDLE_AFTER_MS` was sized for)
- **Rationale**: `AgentState::Busy` is documented as "Mid-turn: the agent is actively working
  (a tool_use turn, a tool_result the agent is consuming, a fresh human prompt it is about to
  answer, or a recent append)" (`crates/app/src/tabmeta/mod.rs:127-130`). That is precisely
  the state a long `cargo build` or a deep subagent run puts the tab in: the log is silent
  because the `tool_result` has not been written yet, and `tail_state` classifies the
  outstanding `tool_use` as `LastMessage::AssistantBusy`. Arming a deadline in that state
  produces a **false IDLE** for the remainder of the call — 15 minutes of it for a 20-minute
  build — and D3's one-shot plus D11's re-arm rule mean it cannot correct itself until the
  tool result lands. Today's panel reads CONNECTED throughout, which is **right**, so this
  would be a straight regression introduced by the fix.
  Suppressing on `Busy` costs nothing new: the worker already derives the state for every
  tab on every tick and publishes it in `TabMeta.agent` (`Agent { kind, state }`,
  `tabmeta/mod.rs:99-102`), and the app already reads it on
  every `feed_agent_states`. The overlay is simply not given it yet — `Env.tab_agent` is
  `Option<AgentKind>` (`dashboard.rs:118` — *which* agent), not the state. So the change is one
  more value on the `notify_active_changed` seam the overlay already receives on every wake.
- **That seam is a TRAIT, so `seam.rs` is a touched file.** Added at round 4, because the
  first form said "one more value on the existing seam" as though it were a local edit.
  `on_active_changed` is a defaulted method on `pub trait Overlay`
  (`crates/app/src/seam.rs:242`, the method at `:312`) and the app calls it through
  `OverlayRegistry::notify_active_changed` (`:486`); `AiMonitorOverlay` is one of three
  implementors (`overlay.rs:1815`, `copy.rs:82`, `clipboard.rs:166`). Widening it is a change
  to the overlay contract, not to one overlay. Two consequences an L3 writer must not
  discover: the method keeps its **default empty body**, so the selection and image presenters
  and every test overlay in `seam.rs`'s own test module compile unchanged; and `seam.rs` joins
  the touched-file set for the Constraints' `cargo mutants` requirement (T3).
  This is the same architectural surface D7's rejected alternative (b) declined to widen —
  `docs/architecture.md` calls the registration set "a value a test can assert". D7's
  reasoning still holds: what it rejected was threading a whole `TabMetaHandle` through
  construction to obtain **one verb**, whereas this adds **one value** to a seam whose entire
  purpose is already to carry per-tab facts to overlays, alongside the `cwd`, `shell_pid` and
  `agent_pid` it carries today. Recorded so the two are not read as contradicting each other.
- **The gate is deliberately FAIL-OPEN, and one config toggle opens it.** `Agent` is
  `Option`al on `TabMeta`, and it is `None` in **two** situations the seam cannot tell apart:
  the `claude` process has exited, and `[sidebar] show_agent_status = false`
  (`crates/common/src/config.rs:117`, pushed to the worker at `main.rs:54`, applied at
  `worker.rs:161` where the `agent` field is built only `if self.enabled_agent`). Note the
  asymmetry that makes this reachable at all: `agent_pid` is computed from a walk gated on
  `enabled_agent || is_active` (`worker.rs:146`), so with the toggle off the **active** tab
  still publishes its pid and the panel still attaches and renders — only the *state*
  disappears.
  An absent state therefore **arms** (it is not `Busy`), and that is the correct default
  rather than an oversight. Fail-closed — suppressing whenever the state is absent — would
  hold the pill at CONNECTED forever after every `claude` exit, which is both common and
  precisely the defect this feature exists to remove. Fail-open costs the D18 suppression to
  users who have turned the sidebar's agent marker off, which is a narrow, opted-into
  configuration. The residue is a Known Gap, not a further gate; adding one here would be the
  fourth instance of the pattern the final dimension note names.
  **The state deliberately does NOT inherit the seam's `None`-means-no-information rule, and
  the divergence has to be written down because the two values ride the same call.**
  `seam.rs:305-311` documents that rule for `agent_pid` — "`None` means **no information**,
  never 'a different pid'" — and `overlay.rs:1988` implements it as `if agent_pid.is_some()`.
  Mirroring it for the state would store the last `Some` and never overwrite it, so a tab that
  reads `Busy` and then loses its agent would stay `Busy` in the overlay **forever**, D18 would
  suppress forever, and the pill would stick at CONNECTED — fail-closed by the back door, after
  the paragraph above rejected it at the front. So the rule for the state is the opposite one:
  **absent is a value, and it is "not `Busy`"**, stored unconditionally. `Some(Busy) → None`
  (the agent exited) is therefore a `Busy →` transition that executes D11's trigger two, which
  is the behaviour wanted — an exited agent's session is exactly one that should go IDLE on
  schedule. The two rules differ because the values differ in kind: `agent_pid` is an
  **identity**, where a wrong read causes a re-point storm, and the state is a **predicate**,
  where a latched read causes a permanent suppression.
  The price is that a transient proc-walk failure — the failure `seam.rs:305-311` exists for —
  now reads as one tick of "not `Busy`" and can arm a deadline mid-build, reproducing the
  `show_agent_status` gap's false IDLE from a different cause. It is folded into that same
  Known Gap rather than fixed, because the only clean fix is the same one named there:
  a `TabMeta` state that distinguishes "not tracked" from "no agent". Debouncing it is not
  available — that is a clock in `overlay.rs`, which R7.4 bans and D6 was chosen to avoid.
  **Interaction with D14, stated so the rules compose in one place:** `Busy` joins D14's list
  of conditions under which no deadline is armed (past-due, detached, `last_event_ms` absent).
  The suppression **defers** the arm; D11's second trigger is what executes it on the
  `Busy →` transition. *(A superseded form of this clause said "A tab that transitions
  `Busy → Waiting` is a rebuild that moves `last_event_ms`, so D11 arms it there" — false, and
  it would have made the feature inert on its primary path. See D11's two-trigger clause for
  why the two inputs cannot be assumed to arrive on the same wake.)*
  **This is a TRADE, not a strict improvement, and the first draft wrongly claimed it cost
  nothing.** The `Busy` signals **do not decay** — `state_from` (`activity.rs:72-80`) returns
  `Busy` immediately for `AssistantBusy` / `UserToolResult` / `UserPrompt` with no age check,
  the exact symmetric twin of the `Waiting` non-decay this whole spec pivots on (Research
  §"The decisive finding" reasons about that property for one branch and the first draft of
  this decision gated on the other without noticing it has the identical shape). The age
  clauses at `activity.rs:81-85` are reached **only** when `last` is `None`. So for a turn
  interrupted before `end_turn` is written — Esc or Ctrl-C during a long tool call, with the
  `claude` process still alive so `agent` is `Some` — the tab reads `Busy` indefinitely and
  no deadline is ever armed.
  D18 therefore exchanges *"wrongly IDLE during a long build"* (new, introduced by D3) for
  *"wrongly CONNECTED after an interrupted turn"* (pre-existing, unchanged from today). That
  trade is worth making — a regression the feature introduces is worse than a defect it
  merely fails to fix — but it is a trade and the Known Gaps record its surviving half.
  **Rejected**: (a) accepting the false IDLE as a Known Gap — bounded by the call and
  self-correcting, and a wrong IDLE is arguably gentler than a wrong CONNECTED, but
  `dashboard.rs:190` names this exact scenario as the threshold's reason for existing, and
  the spec would be promising a truthfulness property while making truthfulness worse in the
  named case; (b) suppressing only while `t.subagents` holds a `completed: false` entry —
  the reviewer's proposal, and **partial**: `TelemetrySnapshot.tools` holds only *completed*
  calls (`crates/ai-monitor/src/telemetry.rs:355`), so a long `cargo build` is invisible to
  it and only the subagent half of the threshold's rationale would be covered;
  (c) folding in-flight `tool_use`/`tool_result` pairs into `TelemetrySnapshot` — the most
  precise fix and the most invasive, expanding into the ai-monitor crate that the Non-goals
  exclude and, if `status` read it, reopening D10; the `Busy` signal already answers the
  question with data that exists.

## Constraints
- **`term-core` and `ai-monitor` are not modified.** This is an app-side display concern;
  the log watcher, its resolution and its attachment rules are untouched
  (`.claude/rules/dependency-direction.md`).
- **`dashboard.rs`'s behaviour is not modified** (D10). Its only edit is the addition of one
  pure exported function, `idle_deadline_ms` (D15); no existing output changes for any input,
  and `dashboard::model` stays a pure function of its `Env`.
- **No new dependency.** Nothing here needs one (`.claude/rules/no-gui-crates.md`).
- **`ControlFlow::Wait` stays, set exactly once, in `main.rs`** (`docs/architecture.md` C6).
  D6 was chosen partly because it leaves this untouched.
- **`overlay.rs` stays free of `Duration` / `Instant` / `thread::sleep` / `recv_timeout` /
  `set_timeout`** — R7.4's ban keeps its full force (D12). The overlay never computes a
  threshold at all: it calls `dashboard::idle_deadline_ms` (D15) and passes the result, so it
  needs none of the banned types.
- **Nothing costs anything while the panel is closed** — no arming, no wake, no rebuild.
- **The deferred Option 2 must not appear incidentally.** The worker emits **one** wake per
  armed deadline, not a heartbeat; D12's new test is what pins the difference.
- **macOS ARM64 only**, no `#[cfg(target_os)]` branches (`.claude/rules/macos-only.md`).
- **No `unwrap()` outside tests**; per-crate `Error` enums via `thiserror` (`CLAUDE.md`).
- **Coverage is judged by mutation.** `cargo mutants -f <file> -j 4` on each touched file;
  a survivor is a missing test unless provably equivalent, and an equivalent survivor is
  preferably fixed by tightening the condition rather than by an `exclude_re` entry
  (`CLAUDE.md`).
- **Edits to `.rs` files go through the Edit/Write tools**, not shell-applied patches — the
  `rustfmt` PostToolUse hook only fires on those, and a shell-applied edit fails
  `cargo fmt --check` much later, far from its cause.

## Known Gaps
- **An interrupted turn leaves the pill CONNECTED indefinitely — D18's surviving half.**
  `state_from` (`crates/app/src/tabmeta/activity.rs:72-80`) returns `Busy` for
  `AssistantBusy` / `UserToolResult` / `UserPrompt` with **no age bound**; the 10 s / 100 s
  clauses below it are reached only when `last` is `None`. A turn interrupted before
  `end_turn` is written — Esc or Ctrl-C during a long tool call, `claude` still alive — leaves
  the tab `Busy` forever, so D18 never arms and the pill never flips. **This is today's
  behaviour, unchanged**, which is why it is a gap rather than a blocker: D18 declines to fix
  this case in exchange for not *introducing* a false IDLE during a long build (see D18's
  trade). The fix, if it is ever wanted, belongs to the classifier — a bounded `Busy` for the
  non-decaying signals — not to this feature, and it would want its own spec because
  `left-sidebar-waiting-area` D16 chose the non-decay deliberately.
- **`[sidebar] show_agent_status = false` makes D18's suppression inert — the false IDLE
  during a long build comes back.** With the toggle off, `worker.rs:161` publishes no `Agent`,
  so the overlay sees no state, D18's gate is fail-open (argued there), and a long `cargo
  build` or deep subagent run arms a deadline that fires mid-call. That is exactly the
  regression D18 exists to prevent, restricted to one opted-into configuration. It is a gap
  rather than a blocker because the alternative is worse in a *commoner* case: suppressing on
  an absent state would hold the pill at CONNECTED forever after every `claude` exit, since
  `TabMeta.agent` is `None` for both an exited agent and a disabled marker and the seam cannot
  tell them apart. **A transient proc-walk failure lands in the same gap**: because the state
  is stored unconditionally (D18), one tick of `None` for a genuinely busy tab reads as "not
  `Busy`" and can arm a deadline that fires mid-build. Same false IDLE, different cause, and
  the same remedy. Two narrower fixes exist if it is ever wanted, and both are out of scope
  here: publish the state independently of the display toggle (it is derived on every tick
  regardless — only the *field* is gated, so this costs nothing but a decision about what
  `show_agent_status` means), or give `TabMeta` a state that distinguishes "not tracked" from
  "no agent" — which is the one that also closes the transient-failure half. Recorded
  **beside the per-TAB/per-SESSION gap below** because it is the same
  shape: D18 derives behaviour from a value whose absence is ambiguous.
- ~~**A deadline armed just before the agent goes `Busy` stays pending through the whole
  `Busy` window — D18's third bypass.**~~ **CLOSED at post-PR review.** Recorded here rather
  than deleted, because the reasoning that kept it open for two review rounds is the part
  worth keeping.
  The hole was real: `reconsider_deadline` returned early on `Busy` **without disarming**, so a
  deadline armed on the last append before the worker publishes `Busy` survived into the busy
  window and fired inside it. The shape that reaches it is ordinary — a user prompt whose turn
  opens with a long silent tool call — and the two inputs arrive on different wakes up to one
  500 ms worker tick apart, which is the window in which it happens.
  **Both reasons given for leaving it open were wrong.** The first was that cancelling would
  reopen something worse, "the deferred arm is what D11's second trigger executes, and
  cancelling it would put the primary path back on the single-trigger design that arms
  nothing". That confuses the mechanism: deferral is carried by `armed_from` being `None`, not
  by a pending deadline, and `disarm` sets `armed_from` to `None`. Cancelling loses nothing.
  The second was that it is "narrow, and weaker than it first looks, because `status` is
  recomputed from the *current* `last_event_ms` at fire time — so the wake usually finds
  CONNECTED and the pill does not move". True only when the log moved in the meantime, which
  in the case that matters is exactly what has not happened: a five-minute tool call writes
  nothing, so `last_event_ms` is still the prompt, and the recomputed status is IDLE. The
  mitigation argued from the general case while the bug lives in the specific one.
  `Busy` now disarms. The deferred arm is unchanged and is asserted end to end by
  `overlay::tests::cancelling_on_busy_still_leaves_the_arm_deferred`, which exists because the
  first reason above, had it been true, would have made that assertion impossible.
- **`AgentState` is per-TAB while the panel is per-SESSION, and they can legitimately
  diverge.** `TabMeta.agent` is keyed by `TabId` and resolves one log per tab, while the panel
  is re-pointed to the active tab's *session*. Under tmux one tab can host several panes each
  running `claude`, so the tab may read `Busy` from pane 2's working agent while the panel
  displays pane 1's quiet session — D18 then suppresses and pane 1's pill stays CONNECTED.
  Bounded to concurrent multi-pane `claude` under tmux, and the failure mode is the
  pre-existing "stays CONNECTED" rather than anything new. Recorded **beside `docs/architecture.md`
  G5's A1/A2 as the same kind of fact**, because D18 is the first decision in this repo to
  derive *behaviour* — whether to arm — from a tab-level value while the value on screen is
  session-level, and G5 is precisely the note saying that coupling is not safe to assume.
- **The `LIVE` agent card can contradict the pill, in one narrow case.** `agent_elapsed`
  (`crates/app/src/dashboard.rs:744-750`) freezes a *completed* subagent's timer but grows a
  `completed: false` one against `now_ms`, and `agents()` paints that card amber captioned
  `LIVE`. A subagent whose ending was never observed keeps `completed: false` — a state the
  crate models explicitly (`Subagent::ended_ok`'s doc, `crates/ai-monitor/src/telemetry.rs:239-241`:
  "An in-flight agent **and a never-notified agent** keep the initial value"). So a
  deadline-triggered rebuild can produce one frame reading pill `IDLE` · `Last update
  16:21:18` · IDLE `5m00s` · **`LIVE researcher` amber `12m34s`** — internally reconcilable
  arithmetically (the card's `start_ms + elapsed` still equals footer + IDLE) but asserting
  two things that cannot both be true of one session.
  **D18 narrows this a long way but does not remove it**: while a subagent is genuinely
  running the tab is `Busy` and nothing is armed, so the surviving case is a *stale* card —
  a never-notified subagent left on screen after the session's turn has ended. Before this
  feature the panel was uniformly wrong in the direction of liveness (pill CONNECTED, card
  LIVE, timer climbing), which was stale but coherent; D3 makes the pill honest and leaves
  the stale card contradicting it. Recorded rather than fixed because the fix belongs to the
  card's own completion detection, not to this feature — and because
  `agent_elapsed`'s own doc comment ("A finished agent whose timer keeps climbing is the
  single most misleading thing a status dashboard can show") means the card is already known
  to be wrong in this state, independently of anything here.
- **L2 unresolved: Error/Edge — clock skew between log timestamps and the system clock.**
  `last_event_ms` is read from the session log's own records while `now_ms` is this
  process's wall clock. On one machine these are the same clock, so the gap is theoretical
  today; if a log ever carried a timestamp from another source, a future `last_event_ms`
  would make `saturating_sub` yield `0` and hold the pill at CONNECTED with a deadline
  arbitrarily far out. Behaviour is identical to today's, so this is a pre-existing property
  rather than a regression — recorded so it is not rediscovered as new.
- **The L0 licence to change the pill's display went unused.** The gate explicitly admitted
  re-framings and a third state; D3 declined both on their merits and D4 froze the
  vocabulary. Recorded because the licence is in the L0 section and a reader could otherwise
  take it as still open.

- **`RP-D20` and `dashboard.rs:383-385` describe the footer incorrectly** — both call it the
  timestamp "the whole frame was computed at" when it formats `last_event_ms`. This spec
  corrects the reasoning it depends on but does **not** rewrite `RP-D20` or that comment;
  doing so is a separate, unforced edit to a shipped decision, and the panel's behaviour is
  correct either way (footer + IDLE recovers the frame's `now`). Recorded so the next reader
  of `RP-D20` is not misled by it, and so the fix is a known, scoped follow-up.
- **R7.4's scan is a raw `contains` on non-comment-leading lines.** `production_half`
  (`crates/app/src/terminal_app.rs:3102-3115`) strips only lines whose *first* non-space
  characters are `//`, so a **trailing** comment such as `let x = 1; // Duration of ...` in
  `overlay.rs` would fail the test with a confusing message. D12's claim that the scan list
  needs no edit is unaffected — the reviewer verified it token by token — but an implementer
  adding explanatory comments to `overlay.rs` should know this.


## L2 Review — findings addressed

The L2-reviewer returned **NEEDS_FIX**. Every citation it disputed was re-verified against the
code before acting on it; the record below is what changed, because several of these were
substantive rather than cosmetic.

| Finding | Verified? | Resolution |
|---|---|---|
| F1 — "exactly two producers" undercounts; `overlay.rs:806` is a third | **yes**, 5 sites | Research rewritten; the probe thread is now named |
| F2/F3/F5 — line drift (`poll` at :1938, redraw at :2360, RP-D9 :303, RP-D20 :481) | **yes** | corrected throughout |
| F4 — `dur()` is minute-granular above 1 h | **yes** | Research, D1 **and** D13 corrected (round 2 initially fixed Research only — see review round 2 §C5) |
| F6 — the footer formats `last_event_ms`, not `now_ms` | **yes** | Non-goals' premise corrected; see the nuance below |
| 1 — `IDLE_AFTER_MS` is private, so D10 ∧ D14 were unsatisfiable | **yes** | **D15** — export a function, not the constant |
| 2 — arming at `last + IDLE_AFTER_MS` fires while still CONNECTED | **yes** (`<=` at :253) | **D15** defines the deadline by its postcondition |
| 3/4 — D8's token has no transport and guards an impossible hazard | **yes** (`OverlayPending` is a unit variant) | **D8 rewritten**; token dropped, real requirement named |
| 5 — two amendment sites missed; empty-roster precondition unstated | **yes** (`wake()` has one call site) | **D16** |
| 6 — the overlay has no route to the worker | **yes** (0 references) | **D7 expanded** — injected callback, `session::Wake` idiom |
| 7 — D2 never states the `visible` gate | **yes** | stated in D2 |
| 8 — badge/anchor run on tile-only rebuilds | **yes** | **D17** |
| 9 — "newer `last_event_ms`" undefined over `Option` | **yes** | **D14.3** |
| 11 — the steelman was the convenient one | **yes** | **D3's steelman rewritten** around the real objection |

**One reviewer claim is overstated, and correcting it is what answers its own steelman.**
F6 is factually right — the footer is `clock(last_event_ms)` — but the conclusion drawn from
it ("nothing on the panel states *now*, so the frozen tiles are qualified by nothing", and
therefore D3 leaves the panel *incoherent*) does not follow. The IDLE tile is
`dur(now_ms − last_event_ms)`, so **footer + IDLE recovers the frame's `now` exactly**. After
the deadline fires the panel reads `Last update 16:21:18` / `IDLE 5m00s` / pill `IDLE` —
three fields that reconcile, not three that disagree. D3 therefore trades a coherent frozen
panel for a coherent *less stale* one. The reviewer's underlying point stands and is acted
on: the Non-goals' original justification cited a qualifier that does not exist, and
`RP-D20`'s own description of the footer is wrong about the shipped code.

**Re-scored, per finding 10.** The original 0.96 was not defensible — four Error/Edge items
(2, 3, 8, 9) were edges the spec had answered wrongly or never asked, and Implementation
claimed 1.00 while the central arithmetic had no legal home.

| Dimension | Before | After fixes | Note |
|---|---|---|---|
| Core Behavior (30%) | 1.00 | 1.00 | unchanged |
| Scope (20%) | 1.00 | 1.00 | D16 widened the amendment set without reopening scope |
| Error/Edge (25%) | 0.83 (overstated) | 0.90 | was ≤0.6 as written; D14.3, D15, D17 close it |
| Data Model (15%) | 1.00 | 1.00 | D10 keeps `dashboard::model` pure |
| Implementation (10%) | 1.00 (overstated) | 0.90 | was ≤0.6; D7's route and D15's home fix it |

**Composite: 0.96** — the same number, now earned rather than asserted.

### Review round 2 — the finding that mattered

Round 2 returned NEEDS_FIX again, and one of its findings was the most consequential of the
whole review: **the threshold's adequacy argument has two legs and D3 removed one of them.**
`IDLE_AFTER_MS`'s doc comment (`dashboard.rs:188-192`) justifies five minutes by naming a
long `cargo build` or deep subagent run — then adds "and under D2 nothing repaints on a timer
to blink it back". Verified. So D3 would have manufactured a **false IDLE** in exactly the
scenario the constant was sized for, and pinned it there for the rest of the call. That
inverts Confirmed Goal #2 into its dual: the pill asserting IDLE for a session that is
working. **D18** resolves it by suppressing the arm while the tab's agent is `Busy`.

The reviewer proposed suppressing on `completed: false` subagents instead; that is partial —
`TelemetrySnapshot.tools` holds only completed calls, so a long build is invisible to it.
`AgentState::Busy` covers both halves of the threshold's rationale because it comes from the
same classifier that distinguishes them.

Round 2's other findings: my pushback on F6 was **conceded on three of four values and
refuted on the fourth** — the `LIVE` agent card, now a scoped Known Gap rather than the
whole-panel claim of round 1. The `footer + IDLE` identity is exact only below an hour, and
recovers nothing when `last_event_ms` is `None`; both are now stated. Round-2 text had not
been propagated into Constraints, D1, D11, D12, D13 and the prior-art citations — including,
embarrassingly, a row in the table above claiming an F4 fix that had not been made. All
propagated; that row now says what actually happened. Six citations added in round 2 had
drifted (notably D17's only evidence, `rung_key`, which is at `overlay.rs:90`, not `:58-65`);
all corrected. D14 gained a fourth clause: no arming while detached.

| Dimension | Round 1 | Round 2 | Note |
|---|---|---|---|
| Core Behavior (30%) | 1.00 | 1.00 | D18 is a new decision, not a new gap |
| Scope (20%) | 1.00 | 0.95 | D18 sits adjacent to the deferred Option 2 and is argued as distinct |
| Error/Edge (25%) | 0.90 | 0.95 | the false-IDLE regression was the last unasked edge |
| Data Model (15%) | 1.00 | 1.00 | unchanged |
| Implementation (10%) | 0.90 | 0.90 | unchanged |

**Composite: 0.96**, with the two most dangerous items — an inert feature (D15) and a
truthfulness inversion (D18) — found by review rather than by the user after shipping.

### Review round 3 — D18 itself was wrong twice, in opposite directions

D18 was new in round 2 and therefore unreviewed; round 3 found both of its failure modes, and
both were verified before acting:

1. **Permanently suppressed.** The `Busy` signals **do not decay** (`activity.rs:72-80`) — the
   exact symmetric twin of the `Waiting` non-decay this spec pivots on, which round 2 gated on
   without noticing. An interrupted turn leaves the tab `Busy` forever and the pill CONNECTED
   forever. D18 now names this as a **trade** rather than claiming it "costs nothing new", and
   the surviving case is a Known Gap.
2. **Inert on the primary path.** The two arming inputs arrive on different wakes at different
   cadences — `last_event_ms` via the watcher (hot), `AgentState` via the 500 ms tick — so on a
   clean turn ending the watcher wins, `last_event_ms` moves while the state still reads
   stale `Busy`, the arm is skipped, and the later state change finds `last_event_ms`
   unchanged. Single-trigger D11 would arm **nothing**, on exactly the path L1 named as the
   target. D11 now has **two** triggers and D18's false claim is recorded as superseded, in
   the style D2 already uses for the same class of failure.

The reviewer also recorded, and I agree, that D18's rejection of its own earlier proposal
(suppress on incomplete subagents) was correct: `TelemetrySnapshot.tools` holds only completed
calls, so a long `cargo build` is invisible to that gate.

### Review round 4 — the touched-file set was wrong, and one more gate had defeated itself

Round 4 read the spec against the worktree and confirmed its citations: the five
`OverlayPending` producers, the `<=` at `dashboard.rs:253`, the private `IDLE_AFTER_MS`, the
footer formatting `last_event_ms`, the non-decay of **both** `Busy` and `Waiting`, the single
`wake()` call site, the no-timeout `recv` on an empty roster, and the dead `overlay_layer.rs`
citation in `RP-D2`. Four things needed fixing; none changed a decision.

1. **`seam.rs` was missing from T3.** D18 said "one more value on the seam", but that seam is
   a defaulted method on `pub trait Overlay` (`seam.rs:242`/`:312`) reached through
   `OverlayRegistry::notify_active_changed` (`:486`), with three implementors. Widening it is
   a contract change and puts `seam.rs` in the mutation-coverage set. Recorded in D18 beside
   D7's rejection of the *other* registration-surface change, with the distinction between
   them argued so the two do not read as contradictory.
2. **`[sidebar] show_agent_status = false` disables D18.** `TabMeta.agent` is `None` both when
   the toggle is off (`worker.rs:161`) and when `claude` has exited, and the seam cannot tell
   them apart — so the gate has to fail one way or the other. It fails **open**, because
   fail-closed would hold the pill at CONNECTED forever after every agent exit. The cost is
   D18's suppression going inert under one opted-into setting; that is now a Known Gap rather
   than a fourth gate.
3. **R2.4's "no deadline is pending" was unknowable.** D8 left the fired wake payload-free, so
   the overlay is never told its deadline fired; a `pending` flag would latch on the first arm
   and trigger two would never run again. This is the **third** instance of the pattern below,
   and the first one to reach the Requirements rather than being caught in a decision. Dropped
   in D11 and in R2.4; the **edge** is what bounds the arming rate, now pinned by a new R2.5
   (old R2.5–R2.9 renumbered to R2.6–R2.10).
4. **R0.3 contradicted D3.** "Panel open, nothing appends, five minutes, zero wakes" is false
   for a panel opened onto a session that is quiet but not yet past the threshold — D3 produces
   exactly one wake there, by design. R0.3 now carries the condition that makes it true (the
   session is already past `IDLE_AFTER_MS`, so D14.1 armed nothing), and the property it was
   conflating with is stated separately as **R0.4**: one wake per quiet session, then none.

### Post-PR review

Three findings after the PR was opened, two of them defects that shipped.

1. **D18's third bypass, closed.** `reconsider_deadline` returned early on `Busy` without
   disarming, so a deadline armed in the one-worker-tick window before the `Busy` publish
   survived into the busy window and fired inside it — the false IDLE during a long build that
   D18 exists to prevent. Two arguments had kept it open across two rounds and both were
   wrong; they are dissected at the (struck-through) Known Gaps entry, because the *shape* of
   the error is the reusable part: one confused the mechanism carrying a deferral, the other
   argued a mitigation from the general case while the bug lived in the specific one. Now
   R2.11, with the deferral it was feared to break asserted end to end.

2. **A re-point carried the old session's deadline.** `open()` dropped the session but not
   `armed_from`, and R2.9's `b <= a` guard then read a *newer* tab-A timestamp as covering
   tab B — so B armed nothing and its pill flipped whenever A's unrelated deadline came due.
   Now R2.12. Not caught earlier because every existing arming test drives one session.

3. **The mutation-scope guard was itself unverified, and wrong.** L5's script read the `fn`
   name out of each diff hunk header, which for a method names the `impl` block and so matched
   nothing: every method this feature touched was outside the scope it certified.
   `TabMetaHandle::set_panel_deadline` — the panel's whole route to the worker — survived
   `replace ... with ()` with 1602 tests green, the same failure as the `run` survivor one
   link further along. Fixing the census to body-span overlap exposed two more functions with
   live survivors (`Running::feed_agent_states`, `Running::after_roster_change`), neither
   executable from a test, both now pinned by source scan.

**What this round says about the four before it.** The recurring finding above was "a gate
added to bound a cost also blocks the path it was meant to leave open", and (1) is its fifth
instance — but it is the first found *after* the spec stopped being reviewed and started being
trusted. (3) is the sharper lesson and a different one: the check standing in for expensive
verification had never been run against a case it passed, so it certified six functions it
could not see. A spec's guards deserve the scrutiny its decisions get.

---

**Final dimension note.** Error/Edge absorbed **four** separate "the gate defeats the feature"
findings across four rounds (D2's round-1 form, D18's suppression, D18's ordering, and the
pending-deadline condition). That recurrence is the honest summary of this spec: every gate
added to bound a cost has, on first writing, also blocked the path it was meant to leave open.
The fourth arrived after three rounds of looking for exactly it, which is the argument for
R2.5 and R0.4 existing as separate graded properties rather than as clauses inside their
neighbours — each one fails visibly if the gate returns.

---

## Requirements

### R0: The two graded properties (Confirmed Goal)

#### R0.1: The model is never staler than the wake that reaches the panel
- **Given**: The ai-monitor panel is visible and an `OverlayPending` reaches its arm
- **When**: That arm decides whether to redraw
- **Then**: The `MonitorModel` was rebuilt from the current snapshot and the current clock
  **before** the decision, so no frame drawn out of that arm carries a model nobody rebuilt

> **Narrowed at L2 review round 5, because the first form claimed more than D1 licences.**
> It read "*wherever* the app decides to redraw the window" — but `refresh_model` is reachable
> from four places (`overlay.rs` `open`, `close`, `toggle_fold`, `poll`), and the only
> wake-driven one is `poll`, reached solely from `poll_all()` in the `OverlayPending` arm. A
> redraw caused by PTY output, a keystroke or a resize therefore draws from the model built on
> the *previous* `OverlayPending`. That is not an oversight to fix: **D1 rejects the `redraw()`
> hook on purpose**, because it fires on every `CursorMoved` of a sidebar drag and would
> re-shape the whole document per drag frame. The requirement now states the property D1
> actually delivers. The broader form was never true of the shipped design and no test or
> recorded limit narrowed it, which is what made it an overstated claim rather than a gap.

#### R0.2: The pill does not assert CONNECTED past its threshold for a finished turn
- **Given**: A session whose last log line is an `end_turn`, panel open, no further appends
- **When**: `IDLE_AFTER_MS` elapses
- **Then**: Within one worker tick (≤500 ms) the pill reads IDLE

#### R0.3: No new wakes exist on an idle machine
- **Given**: No tab is running an agent, nothing appends to any log, and the panel is open on a
  session **already past `IDLE_AFTER_MS`** — so D14.1 armed nothing and the pill already reads
  IDLE (or the snapshot is detached and it reads NO SESSION, D14.4)
- **When**: Five minutes pass
- **Then**: The event loop receives zero `OverlayPending` wakes and draws zero frames

#### R0.4: A session that goes quiet under an open panel costs exactly one wake, ever
- **Given**: The panel is open on a session that has just gone quiet with the tab's
  `AgentState` not `Busy`, so exactly one deadline is armed
- **When**: An hour passes with nothing appended and no other tab changing
- **Then**: The event loop receives **exactly one** `OverlayPending` from that deadline and
  draws exactly one frame — the bounded edge of D3, not the cadence `RP-D2` rejected

---

### R1: The model rebuild (D1, D2, D9, D13)

#### R1.1: A tabmeta wake that carries no log data still refreshes the model
- **Given**: Panel visible; `Overlay::poll` finds the `LogWatcher` queue empty; the session started ≥1 s ago so the UPTIME tile has ticked
- **When**: An `OverlayPending` from the tabmeta worker reaches the arm
- **Then**: `dashboard::model` is rebuilt, the new model differs from the published one, it is published, and a redraw is requested

#### R1.2: An unchanged model costs one build and one compare — nothing else
- **Given**: Panel visible; a rebuild produces a model equal to the published one
- **When**: The rebuild completes
- **Then**: `item_ladder`, `rung_key`, `resolve_anchor` and `count_new_above` are **not** run, nothing is published, and no redraw is requested

#### R1.3: A closed panel rebuilds nothing
- **Given**: The panel is not visible
- **When**: Any `OverlayPending` arrives
- **Then**: No `dashboard::model` call is made and no deadline is armed

#### R1.4: The rebuild is not hooked to the `redraw()` seam
- **Given**: A sidebar resize drag is in progress, emitting `redraw()` per `CursorMoved`
- **When**: The drag runs for 60 frames
- **Then**: The number of `dashboard::model` calls is bounded by the `OverlayPending` count, not by the frame count

---

### R2: Arming the deadline — overlay side (D3, D7, D11, D14, D18)

#### R2.1: A newer `last_event_ms` arms the deadline
- **Given**: Panel visible, snapshot attached, tab not `Busy`, `last_event_ms` moves from `Some(a)` to `Some(b)` with `b > a`
- **When**: The model is rebuilt
- **Then**: Exactly one arming call is made carrying `dashboard::idle_deadline_ms(b)`

#### R2.2: An unchanged `last_event_ms` arms nothing
- **Given**: A deadline is pending; a rebuild occurs whose `last_event_ms` is unchanged
- **When**: The rebuild completes
- **Then**: No arming call is made (one `Cmd` per burst, not per line)

#### R2.3: A `Busy` tab defers the arm
- **Given**: Panel visible; the tab's `AgentState` is `Busy`; `last_event_ms` moves to a newer value
- **When**: The model is rebuilt
- **Then**: No arming call is made

#### R2.4: Leaving `Busy` executes the deferred arm — the second trigger
- **Given**: `last_event_ms` is `Some(t)` and the tab's `AgentState` was `Busy` on the previous
  wake
- **When**: A wake reports the tab's `AgentState` as anything other than `Busy` — including
  absent — with `last_event_ms` **unchanged**
- **Then**: Exactly one arming call is made carrying `idle_deadline_ms(t)`

#### R2.5: The second trigger is an EDGE, not a level
- **Given**: R2.4 has just fired, so a deadline is armed and the tab is not `Busy`
- **When**: Further wakes arrive with the tab still not `Busy` and `last_event_ms` unchanged
- **Then**: **No** further arming call is made — what bounds the rate is the `Busy →`
  transition, not a pending flag the overlay has no way to clear (D11)

#### R2.6: The incoming `AgentState` survives the re-point guard
- **Given**: A wake where `Repoint::changed` reports the tab did not change
- **When**: `on_active_changed` runs
- **Then**: The new `AgentState` has been stored before the dispatch short-circuits — **including
  when it is absent**, unlike the `agent_pid` beside it (D11, D18) — and R2.4 can still fire from it

#### R2.7: A past-due deadline is never armed
- **Given**: The panel opens onto a session whose `last_event_ms` is older than `IDLE_AFTER_MS`
- **When**: `open()` builds the first model
- **Then**: The pill reads IDLE immediately and no arming call is made

#### R2.8: A detached snapshot arms nothing
- **Given**: `detached` is `Some(_)`, so `status` returns `NoSession`
- **When**: A rebuild occurs with any `last_event_ms`
- **Then**: No arming call is made

#### R2.9: `Option` transitions follow D14.3
- **Given**: Panel visible, tab not `Busy`
- **When**: `last_event_ms` goes `None → Some(t)` / `Some(a) → Some(b)` with `b <= a` / `Some(_) → None`
- **Then**: Respectively: arm at `idle_deadline_ms(t)` / leave the pending deadline untouched / disarm

#### R2.10: Closing the panel disarms
- **Given**: A deadline is pending
- **When**: `close()` runs
- **Then**: A disarm call is made and no later wake can flip a reopened panel

#### R2.11: `Busy` also CANCELS a deadline already pending
- **Given**: A deadline is pending (armed while the tab was not `Busy`)
- **When**: A wake reports the tab's `AgentState` as `Busy`
- **Then**: A disarm call is made and `armed_from` is cleared — so no wake survives into the
  busy window, and R2.4's deferred arm still executes afterwards from the current
  `last_event_ms`
- **Why it is a requirement and not an optimisation**: the two inputs arrive on different wakes
  up to one worker tick apart, so an append landing just before the `Busy` publish arms a
  deadline the suppression is then too late to stop. Left pending, it fires inside a long tool
  call against an unmoved `last_event_ms` and the pill reads IDLE while the agent works — the
  exact outcome D18 exists to prevent. Added post-PR; see the closed third bypass under Known
  Gaps for the two arguments that wrongly kept it open.

#### R2.12: A re-point drops the previous session's deadline
- **Given**: A deadline is pending for tab A, armed from A's `last_event_ms`
- **When**: The panel re-points to tab B (tab switch, or a `/clear` rotation)
- **Then**: `armed_from` is cleared and a disarm call is made **before** B's first model is
  built, so B arms from its own timestamp
- **Why**: `armed_from` is a timestamp read off A's log, and R2.9's `b <= a` rule compares B's
  against it. B's last event can legitimately be *older* than A's while B is perfectly current,
  in which case the guard reads "already armed from something newer" and B arms nothing — its
  pill then flips whenever A's unrelated deadline happened to be due, up to a full
  `IDLE_AFTER_MS` late. Added post-PR.

---

### R3: Holding and firing the deadline — worker side (D6, D7, D8)

#### R3.1: A passed deadline produces exactly one wake
- **Given**: The worker holds a deadline at `T`, roster non-empty
- **When**: The worker ticks 20 times with `now > T`
- **Then**: `wake()` is called exactly **once** and the slot is cleared — the property distinguishing a bounded edge from a heartbeat

#### R3.2: An unreached deadline produces no wake
- **Given**: The worker holds a deadline at `T`, no `TabMeta` changes
- **When**: The worker ticks repeatedly with `now < T`
- **Then**: `wake()` is not called

#### R3.3: Arming replaces the pending deadline
- **Given**: The worker holds a deadline at `T1`
- **When**: A `Cmd` arrives arming `T2`
- **Then**: The slot holds `T2` alone, and no wake is ever produced for `T1`

#### R3.4: Disarming clears the slot
- **Given**: The worker holds a deadline
- **When**: A disarm `Cmd` arrives
- **Then**: The slot is empty and no tick produces a wake from it

#### R3.5: The deadline does not change the publish path
- **Given**: A tick in which a deadline fires but no `TabMeta` changed
- **When**: The tick completes
- **Then**: No lock is taken, `generation` is not bumped, and nothing is published — only the wake is emitted

---

### R4: `dashboard::idle_deadline_ms` (D10, D15)

#### R4.1: The returned instant is the first at which `status` reports IDLE
- **Given**: Any `last_event_ms = t` on an attached snapshot
- **When**: `status` is evaluated at `now_ms = idle_deadline_ms(t)`
- **Then**: It returns `PanelStatus::Idle` — and at `idle_deadline_ms(t) - 1` it returns `PanelStatus::Connected`

#### R4.2: `IDLE_AFTER_MS` stays private
- **Given**: The workspace after this change
- **When**: `crates/app/src/overlay.rs` is compiled
- **Then**: It names neither `IDLE_AFTER_MS` nor the literal `300_000`

#### R4.3: No existing `dashboard` output changes
- **Given**: The existing `dashboard` test suite
- **When**: It runs against the modified module
- **Then**: Every assertion passes unchanged — the only addition is a new exported function

---

### R5: The pill's display is unchanged (D4)

#### R5.1: `PanelStatus` keeps exactly three variants
- **Given**: The modified `dashboard` module
- **When**: `PanelStatus` is inspected
- **Then**: It has `Connected`, `Idle`, `NoSession` and no others, with their existing labels and colours

---

### R6: View state survives a clock-only rebuild (D17)

#### R6.1: A tile-only change does not move the new-items badge
- **Given**: A panel scrolled to a held anchor with `badge = 0`; a rebuild in which only status/tile/footer strings differ
- **When**: The rebuild publishes
- **Then**: `badge` is still `0`

#### R6.2: A tile-only change does not move a held anchor
- **Given**: A panel with `Anchor::Held` at index `n`
- **When**: A rebuild occurs in which only clock-derived strings differ
- **Then**: The resolved top index is still `n`

#### R6.3: Fold state survives every rebuild
- **Given**: Section 3 is expanded by the user
- **When**: Any number of rebuilds occur
- **Then**: Section 3 is still expanded

---

### R7: The reversal is written down at all three sites (D5, D12, D16)

#### R7.1: `RP-D2` records the cadence-vs-edge distinction and its citation is live
- **Given**: `specs/ai-monitor-right-panel/spec.md` after this change
- **When**: D2's rationale is read
- **Then**: It states what a bounded edge is and why it differs from the rejected cadence, and cites `crates/renderer/src/monitor_panel.rs:992`/`:1451` rather than the deleted `overlay_layer.rs`

#### R7.2: `publish_if_changed`'s doc comment no longer claims `wake()` implies a change
- **Given**: `crates/app/src/tabmeta/worker.rs` after this change
- **When**: The doc comment above `publish_if_changed` is read
- **Then**: It states that a second `wake()` site exists for the panel deadline and that a wake no longer implies a `TabMeta` changed

#### R7.3: R7.4's part-2 comment matches D2
- **Given**: `crates/app/src/terminal_app.rs` after this change
- **When**: The comment listing the panel's redraw triggers is read
- **Then**: It no longer claims the only triggers are a keystroke and a snapshot-carrying `poll`

#### R7.4: The R7.4 test's scan list is unchanged and still passes
- **Given**: The workspace after this change
- **When**: `r7_4_the_loop_still_waits_and_the_panel_schedules_no_wake` runs
- **Then**: It passes with its ban list intact — `ControlFlow::Poll`/`WaitUntil` still banned in all three files, `Duration`/`Instant`/`thread::sleep`/`recv_timeout`/`set_timeout` still banned in `overlay.rs`, and `set_control_flow` still counted once

---

### R8: Cost properties (D13)

#### R8.1: The idle renderer guarantee is untouched
- **Given**: The renderer's existing idle tests, which drive frames with one unchanged model
- **When**: They run after this change
- **Then**: Stage one runs once and stage two runs once across all idle frames, exactly as before

#### R8.2: The comment says "no new wakes", not "free"
- **Given**: The rebuild site in `terminal_app.rs`
- **When**: Its comment is read
- **Then**: It states the cost as one `dashboard::model` per wake plus a re-shape when a formatted string changed, and does not describe the refresh as free

---

## Tasks

> **Slice shape.** "Vertical" here means the producer and consumer of an interface land in
> the same task — the project's own lesson from `docs/architecture.md`, where `Session::spawn`
> taking a `Wake` is what makes the assembled path constructible. T3 is the one that matters:
> the `Cmd` variant, the `TabMetaHandle` setter, the injected callback, the overlay's arming
> rules and the worker's slot are **one** task, because splitting them would leave an
> interface mismatch that no test on either side could catch.

### T1: `dashboard::idle_deadline_ms` + the display-unchanged guard [infra]
- **Fulfills**: R4, R5
- **Depends on**: (none) — **parallel with T2**
- **Touches**: `crates/app/src/dashboard.rs` only
- **Note**: The postcondition test (R4.1) asserts the returned instant against `status`
  *itself*, at `t` and at `t − 1`, which is what stops the function and the `<=` comparator
  from ever drifting apart. R4.2 is a compile-level fact, cheapest asserted as a source scan
  in the same style as R7.4.

### T2: Rebuild the model on every visible-panel wake, redraw on diff [vertical]
- **Fulfills**: R0.1, R0.3, R1, R8
- **Depends on**: (none) — **parallel with T1**
- **Touches**: `crates/app/src/terminal_app.rs` (the `OverlayPending` arm),
  `crates/app/src/overlay.rs` (`refresh_model`'s diff-first short-circuit)
- **Note**: D9's short-circuit (R1.2) must be asserted by *counting* the ladder/anchor work,
  not by asserting the published model — an equal model publishes identically whether or not
  the gate fired, exactly as `monitor_panel.rs:442-446` records for its own cache. R8.2 is a
  comment-content requirement and belongs here so the wording lands with the code it
  describes.

### T3: The deadline, end to end — overlay arming + `Cmd` + worker slot [vertical]
- **Fulfills**: R0.2, R0.4, R2, R3
- **Depends on**: T1 (arms with `idle_deadline_ms`), T2 (a deadline wake produces a frame
  only because T2 rebuilds on it — without T2 this task is inert, which is D11's two-trigger
  finding in task form)
- **Touches**: `crates/app/src/overlay.rs`, `crates/app/src/tabmeta/mod.rs`,
  `crates/app/src/tabmeta/worker.rs`, `crates/app/src/seam.rs` (widening
  `Overlay::on_active_changed` and `OverlayRegistry::notify_active_changed` to carry the tab's
  `AgentState` — a defaulted trait method, so the other two implementors are untouched; see
  D18), `crates/app/src/terminal_app.rs` (reading the state off the snapshot in
  `feed_agent_states` and passing it onto that seam)
- **Note**: R3.1 — *exactly one* wake across 20 ticks past the deadline — is the property no
  source scan can see and the one that keeps this a bounded edge rather than the deferred
  Option 2. R2.4 and R2.6 are the ordering fix; a test must drive the **two inputs on
  separate wakes** (timestamp first, state second) or it will pass against the inert version.
  R2.5 is its twin on the other side: a test that drives only *one* non-`Busy` wake passes
  against a level-triggered arm that sends a `Cmd` on every wake thereafter.

### T4: View state survives a clock-only rebuild [vertical]
- **Fulfills**: R6
- **Depends on**: T3 (serialized on `overlay.rs`, not on logic) — **parallel with T5**
- **Touches**: `crates/app/src/overlay.rs` and its tests
- **Note**: Nothing here is known to be broken — R6 exists because D17 found the design
  *asserts* key-set stability under a tile-only change without proving it. If the assertion
  turns out false, the fix is in `rung_key`, and that is a finding, not a failure.

### T5: Write the reversal down at all three sites [infra]
- **Fulfills**: R7
- **Depends on**: T3 (the second `wake()` site must exist before its doc comment describes
  it) — **parallel with T4**
- **Touches**: `specs/ai-monitor-right-panel/spec.md`, `crates/app/src/tabmeta/worker.rs`
  (doc comment), `crates/app/src/terminal_app.rs` (R7.4's part-2 comment)
- **Note**: R7.4 is a *negative* deliverable — the scan list must come through **unchanged**
  and still pass. If this task finds itself editing the ban list, D12's central claim has
  been falsified and the task should stop and report rather than widen the test.

### Task DAG

```
T1 [infra]     ─┐
                ├─→ T3 [vertical] ─┬─→ T4 [vertical]
T2 [vertical]  ─┘                  └─→ T5 [infra]
```

## External Dependencies

### Pre-work
- (none) — no new crate, no service, no config, no schema. Every input already exists in the
  workspace.

### Post-work
- **`cargo mutants -f <file> -j 4`** on each touched file, per `CLAUDE.md`: a survivor is a
  missing test unless provably equivalent. Expect `overlay.rs` and `worker.rs` to be the
  interesting ones — the arming rules (D14, D18) are a decision table, which is the shape
  that historically left survivors here (`session.rs` 16, `tabs.rs` 41). Record survivor
  **counts**, not ratios. `seam.rs` joins the set (T3) and should be uninteresting — a widened
  signature and a defaulted no-op body — but "should be" is the claim `cargo mutants` exists
  to check, so run it rather than assuming it.
- **`/run-term`** to eyeball the pill actually flipping: open the panel on a finished
  session, wait out the threshold, confirm IDLE — the one behaviour no headless test observes,
  since `cargo test` cannot host a winit event loop (R7.4's own reasoning).
- **Optional follow-up, not part of this spec**: correct `RP-D20`'s and
  `dashboard.rs:383-385`'s description of the footer (see Known Gaps).
