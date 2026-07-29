# loop.md — Loop Contract: overlay-session-attach

> Round 2. Round 1's contract (re-resolve + floor + clamp) shipped as `83f4518` and
> introduced the regression this round fixes. Its Lessons are preserved below.

## Goal
The overlay must attach to the tab's *actually active* session and, once attached, **never lose the data it has accumulated** for as long as the panel stays open.

## The defects (measured, 2026-07-28, from the reported screenshot)
A tab running a live Claude Code session showed every section empty and `session (no active session — newest log idle 19m39s)`. Two independent causes:

**A — the ranking key is wrong.** `newest_jsonl` ranks candidates by file **mtime**, but mtime advances without a new event. Measured in `~/.claude/projects/-Users-hyunhokim-IdeaProjects-terminal`:

| file | mtime | last event |
|------|-------|------------|
| `539f86bf` | 12:48:58 | 19:33:02Z (12:33) |
| `c422a74e` | 12:44:29 | **19:44:29Z (12:44)** |

mtime picks `539f86bf`; the live session is `c422a74e`. mtime measures "the file was written to"; only the last **event** measures "this session is active". 16 of 94 lines in one file carry no timestamp at all, so a write that advances mtime routinely fails to advance the last event.

**B — round 1's floor rule blanks accumulated data.** `compose` re-evaluated `last_event_ms < floor_ms` on *every* call and swapped the whole snapshot for `&empty`. So a panel that was showing a session emptied the moment its held snapshot aged past the window start. This is the regression the user reported: "기존 내용이 없어졌는데".

## Approach (human-approved 2026-07-28, round 2)
1. **Rank by last event, not mtime.** Pick the candidate whose newest parseable `timestamp` is greatest, via a bounded tail read per file (the `activity::tail_state` precedent — at most `TAIL_CAP` bytes each), over at most the 16 most-recently-touched logs. `mtime >= last_event` always holds, so the mtime shortlist cannot exclude a plausible winner.
2. **Attachment is decided ONCE and latches.** The floor (this window's start) decides *whether to attach*, not what to display. The decision lives in exactly one place — `overlay.rs` — and is sticky: `attached = attached || snapshot.last_event_ms >= started_ms`. Round 1's bug was the same predicate implemented in two places with opposite polarity (Lesson L1); this round it exists once.
3. **Once attached, nothing is ever blanked.** `dashboard::compose` takes `attached: bool` and renders what it is given. It no longer computes staleness, no longer swaps in an empty snapshot, and cannot un-attach. Data persists until the panel is closed or the tab changes.

## Scope & non-goals
- In: `crates/ai-monitor/src/lib.rs`, `crates/app/src/overlay.rs`, `crates/app/src/dashboard.rs`
- Out: `terminal_app.rs`, `seam.rs`, `settings.rs`, `crates/renderer/**`, `crates/common/**` (round 1's config work is done and stays).
- Accepted consequence of the approved persistence rule: if the user ends the session and starts a new one in the same tab, the panel keeps showing the previous session's numbers until the panel is closed and reopened.

---

## Gate 1 — Pass/Fail

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| test failures | `== 0` | `cargo test --workspace` |
| ranking (A) | given the measured pair — one file newer by mtime, another newer by last event — the resolver picks the **last-event** winner | named test |
| bounded I/O | resolution reads `<= TAIL_CAP` bytes per candidate and examines `<= 16` candidates | named test over a dir of 40 logs |
| persistence (B) | once attached, a later compose with an arbitrarily old `last_event_ms` still renders the full data — never the empty state | named test |
| no un-attach | attachment is set in exactly **one** place and is monotonic | named test + `grep -c` on the predicate |
| attach still gated | a window whose only log went quiet before it opened does **not** attach | named test (round 1's case, must not regress) |
| snapshots | the 9 `.snap` files unchanged | `git status --porcelain crates/app/src/snapshots/` |

## Gate 3 — Qualitative
- **The predicate exists once (Lesson L1)** — "has this log written since the window opened" is implemented in a single place; no second site can disagree with it.
- **Data preservation is structural, not incidental** — it must not be possible for `compose` to blank an attached session, whatever the snapshot contains.
- **Policy/mechanism split (SD4)** — the floor and the wording stay app-side; ranking and bounded I/O in `ai-monitor`.
- **Cost is bounded and stated** — resolution is O(candidates) bounded tail reads, and the bound is a named constant, not a hope.
- **No panic path** — unreadable/truncated/missing logs degrade.

---

## Autonomy Boundary
**✅ Auto-fix:** anything within the three declared files.
**🛑 STOP — escalate:**
- Editing `terminal_app.rs`, `seam.rs`, `settings.rs`, `crates/renderer/**`.
- Adding any crate dependency.
- Any write to a session log or under `~/.claude`.
- Reading a whole log file during resolution (the bounded tail is the point).
- Any change that lets an attached session's data be cleared while the panel is open — the approved requirement.

## Exit criteria
Every gate passes and the Evidence Report is emitted.

---

## Lessons  (self-learning — carried across runs of THIS contract)

<!-- Recorded 2026-07-28 after a 3-iteration run. `source: human-approved`. -->

### L1 — two fixes to the same fail-open must agree on their polarity
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: When a rule is enforced in two places (here: the watcher's latch and the display's floor), state the shared predicate once in words, then check each site against that sentence. Better: implement it once.
- note: the display layer was taught to REFUSE an undated log in the same iteration the watcher was taught to LATCH on one (`is_none_or` where `is_some_and` was meant). Round 2 removes the duplication entirely rather than keeping two sites in sync.

### L2 — a "last seen" field is not a "maximum" field
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: Before comparing an accumulated timestamp against a threshold, confirm how it is written. `SessionTelemetry` assigns `last_event_ms` unconditionally, so it can move BACKWARD. Any latch keyed on it must be monotonic on its own.
- note: a single backdated timestamp would otherwise un-latch a live session mid-flight and discard everything accumulated.

### L3 — a fallback that walks up a path must have a stated stopping point
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: Any "nearest existing ancestor" walk needs an explicit boundary and a test asserting it declines rather than escaping.
- note: it climbed to `$HOME` and watched it recursively. The fix climbs exactly one level, to the projects root.

### L4 — the exact-binding idea was refuted by one command; run it first
- cause: other
- source: human-approved
- form: context
- status: active
- check: —
- note: `lsof -c claude | grep .jsonl` returns 0 matches — Claude Code opens, appends, closes. Thirty seconds of measurement removed a whole design branch.

### L5 — the panel was already showing the evidence it needed
- cause: other
- source: human-approved
- form: context
- status: active
- check: —
- note: the misattributed session rendered `idle 1h50m` on a window seconds old. When a display can compute a fact that contradicts its own framing, the fact should gate the framing.

### L6 — mtime answers "was this file written", not "is this session active"
- cause: gate-fail
- source: auto-unattended
- form: gate3-item
- status: active
- check: Any "most recent session" ranking must key on the newest parseable event *inside* the file, never on the filesystem mtime. Assert with a fixture where the two orders disagree.
- note: measured — `539f86bf` had the newest mtime (12:48) and a 15-minute-old newest event (12:33), while the genuinely live session `c422a74e` was second by mtime. 16 of 94 lines in one real log carry no timestamp, so writes that bump mtime without advancing the last event are routine, not exotic.

### L7 — a display filter is not an attachment filter
- cause: gate-fail
- source: auto-unattended
- form: gate3-item
- status: active
- check: A rule that decides *which source to trust* must run when the source is chosen, and its result must be stored. If it is re-evaluated inside the renderer, it will eventually re-decide against data already on screen and erase it.
- note: round 1 put the floor rule inside `compose`, so every repaint re-litigated the attachment and blanked a live panel the moment the held snapshot aged past the window start. The user's requirement — data preserved from session start to session end — is only expressible as a latched attachment decision, never as a per-frame predicate.

### L8 — a flag shared across threads is a check-then-act, not a latch
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: When one thread publishes a decision another thread reads, ask what happens to work *already in flight* when the flag flips. If the answer matters, the reader must be able to **reject the result**, not merely to decline starting new work. No memory ordering closes this — it is not a visibility bug.
- note: moving the ownership rule into a single `Arc<AtomicBool>` correctly removed the duplicate predicate, but the watcher loads it at the top of its callback, so a callback that began unattached could still finish a re-resolution — resetting the accumulator and sending a rival session's data — a moment after the app attached. `attached` then accepted it, since it was neither empty nor stale. Closed by also refusing a snapshot whose `session_id` differs from the attached one: declining to switch and refusing what a switch produced are two different guarantees, and the requirement needed both.

### L9 — a test that re-implements the rule in its own body tests nothing
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: A test must call the production path. If the code under test is unreachable from a test (needs a window, a proxy, a GPU), extract the rule into a plain struct or function rather than restating it in the assertions.
- note: `attachment_is_monotonic_within_an_open` wrote `attached = attached || proves_ownership(...)` in the test body and asserted monotonicity of *that* expression — true by construction, and it would have passed had production been rewritten as a bare `store()`. The rule now lives in `Attachment::admit`, which the test drives directly. Both guards are mutation-checked: deleting either makes exactly its own test fail.
