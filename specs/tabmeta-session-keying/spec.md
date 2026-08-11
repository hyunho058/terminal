# Spec: tabmeta-session-keying

## Meta
- **Created**: 2026-08-10
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-08-10
- **L2 review**: the L2-reviewer subagent ran **three** passes — NEEDS_FIX → NEEDS_FIX → PASS
  (composite ≈0.78 → ≈0.82 → ≈0.95). Round 1 found 11 issues, the most important being that
  this spec's own research had misread the ranker: `resolve_log_in` → `newest_jsonl` ranks by
  **mtime**, not by newest event. Correcting that surfaced a *second* defect mechanism the
  original bug report did not contain — the `prev_label` carry-forward at `activity.rs:200`
  reads from the cwd-keyed entry, so it leaks one session's label onto another session's log
  even after the newer log wins the ranking. Round 2 found 3 blockers (the single-stat budget
  was unachievable with the stated signatures; tier 1b's project directory was undefined and
  its only available source was the one D4 rejects, which would have stranded the very tabs
  tier 1b exists to help; no injection seam for the registry read). Round 3 passed with five
  polish items, all folded in before approval. Two round-1 findings were pushed back on with
  evidence and the reviewer accepted both: `Unknown`-as-falling-edge is established intent
  (`terminal_app.rs:1061-1062`), and sibling-exclusion ranking is a scope question rather
  than a correctness one.

## Goal

The sidebar card's AI-task label is resolved by working directory, not by session, so
every tab in one project renders the same label and the same activity badge — by
construction, not by accident.

`worker.rs:112` fetches the label with `label_of_cwd(&self.act_memo, c)`, keyed on the
tab's cwd. Both memos in `activity.rs:71-95` are `HashMap<PathBuf, _>`; the module doc
says so outright ("Both are shared by cwd, so two tabs in one project share them").
`activity.rs:169` calls `ai_monitor::resolve_log_in()`, which picks **one winner per project
directory**. So two tabs in `~/IdeaProjects/terminal` read the same `.jsonl` and get the
same label, by construction.

**Two distinct mechanisms produce the reported symptom, and only one was in the original
diagnosis.** `resolve_log_in` → `newest_jsonl` (`crates/ai-monitor/src/lib.rs:656-672`)
ranks by **mtime**, *not* by newest event — the last-event ranker is a different function,
`most_recent_session` (`lib.rs:603-616`), which this path never calls. (The doc block at
`lib.rs:164-176` describing last-event ranking is orphaned above `Wanted`, which is what
makes the misreading easy.) Under mtime ranking:

1. **Before the new session writes anything** it has no file, so the older session's log is
   the only candidate and wins. Both cards show the older session's prompt.
2. **After the new session writes its first line** it has the *newest* mtime and wins the
   ranking — yet both cards still show the older prompt. This is the second mechanism: the
   new log has no substantive prompt yet, so `pick_label` returns `None` and
   `activity.rs:200` carries the previous label forward — and `prev_label` is read from the
   memo entry **keyed on the cwd**, i.e. from the *older session's* entry. The carry-forward
   that exists to keep an idle agent's identity (tab-identity D6) leaks one session's label
   onto another session's log.

Both mechanisms are consequences of the same root cause — cwd as the key — and both are
closed by keying on the session.

The same defect hits the busy/waiting/idle mark: `state_of_cwd` (`activity.rs:134`) uses
the identical cwd-keyed resolution, so two tabs in one project share one activity badge.

This is the bug class already fixed for the ai-monitor panel in #24/#25 (tab pid → its own
`claude` process → `~/.claude/sessions/<pid>.json` → an exact session id). The sidebar card
path was never migrated; it is still on the pre-#24 cwd-ranking.

## Non-goals

- The ai-monitor panel's own resolution path (`tabsession::hint_for`,
  `ai_monitor::resolve_owned_session`) — already session-correct, not touched.
- Changing *what* the label says: `pick_label`, `is_substantive` / `is_continuation`, and
  the `MIN_LABEL_CHARS` policy stay exactly as they are.
- Calling `tabsession::hint_for` from the worker — it forks `/bin/ps`, too costly for a
  per-tick derivation.
- Cross-platform or non-Claude agents: only `AgentKind::Claude` has a log source (D7 of
  `tab-metadata`); every other agent stays `Unknown` / no label.
- **Improving the cwd-ranking fallback itself.** Tier 3 (D1) keeps today's `resolve_log_in`
  behaviour verbatim, mtime flapping and foreign-app logs included. The feature's claim is
  that tiers 1/1b/2 stop any *identifiable* tab from reaching it — not that ranking gets
  better. See Known Gaps for the identified follow-up.

## Confirmed Goal

Key the card's task label **and** its activity state on the tab's own Claude session
rather than on its cwd, using the session registry `claude` already writes about itself
(`~/.claude/sessions/<pid>.json`), so that:

1. Two tabs running separate Claude sessions in one project directory render **distinct**
   task labels and **distinct** activity badges.
2. A tab whose registry record is missing or unvalidatable still renders today's
   cwd-resolved label rather than going blank.
3. The waiting band (`waiting.rs::on_agent_states`) — which consumes the per-tab
   `AgentState` — stops raising and clearing one session's question on every tab in the
   project.
4. The added steady-state cost per **Claude** tab per refresh is exactly: one
   `proc_pidinfo(PROC_PIDTBSDINFO)`, one ~400-byte JSON read, and — on a registry miss only —
   one `sysctl` argv read. **No process fork, and no extra directory scan or extra `stat`**
   — resolution takes the one `stat` and hands it to `tail_role`, which stops taking its own
   (D7). Tabs with no Claude agent pay nothing new.

## Research

### The defect path (as reported, verified line by line)

- `crates/app/src/tabmeta/worker.rs:110-115` — the label comes from
  `super::activity::label_of_cwd(&self.act_memo, c)` where `c` is the tab's cwd. The
  comment ("`self.activity` just warmed the role memo for this cwd, so this is a pure memo
  read") records the coupling: `worker` relies on `activity()` having populated the memo
  under the *same cwd key* one statement earlier.
- `crates/app/src/tabmeta/activity.rs:71-95` — `ActivityMemo { path: HashMap<PathBuf, …>,
  role: HashMap<PathBuf, …> }`. Both keyed on cwd. Module doc `activity.rs:10-11`:
  "Both are shared by cwd, so two tabs in one project share them."
- `crates/app/src/tabmeta/activity.rs:134-150` — `state_of_cwd` takes `cwd: &Path` and runs
  the same resolution, so the activity badge shares the defect with the label.
- `crates/app/src/tabmeta/activity.rs:169` → `ai_monitor::resolve_log_in`
  (`crates/ai-monitor/src/lib.rs:137-140`) → `newest_jsonl` (`lib.rs:656-672`): one winner
  per project directory, ranked by **mtime**. Its own doc names the limit: "a **directory**-
  level heuristic with no binding to any process … Callers must treat the result as 'the
  session that most recently wrote here', not 'this tab's session'." Measured now:
  `~/.claude/projects/-Users-hyunhokim-IdeaProjects-terminal/` holds **56** `*.jsonl` files,
  all competing for that single slot. Because the rank is mtime, two *live* sessions in one
  project make this winner **flap** between logs, not settle.
- `crates/app/src/tabmeta/activity.rs:200` — `let prev_label = memo.role.get(cwd)…` — the
  carry-forward reads the previous label from the **cwd** entry, so it survives a change of
  log. That is the second defect mechanism described in the Goal.
- `crates/app/src/tabmeta/agent.rs:152-184` — `walk` and `agent_of_pid` visit the agent's
  pid and then **discard it**, returning only `Option<AgentKind>`. The pid the fix needs is
  already in hand at `agent.rs:167`.

### Blast radius beyond the label

- `crates/app/src/sidebar_model.rs:55-62` — `task` feeds `resolve_line1_tier`
  (precedence user > AI-task > program-OSC > cwd), so a wrong label displaces the correct
  cwd title on the card's first line.
- `crates/app/src/terminal_app.rs:861` — the same `task` field is read again there.
- `crates/app/src/waiting.rs:144-152` — `on_agent_states(&[(TabId, AgentState)], …)` drives
  the sidebar waiting band off the per-tab `AgentState`. Because that state is cwd-shared,
  one session's `Waiting` rising edge currently raises the band for every tab in the
  project, and the falling edge clears them all. Confirmed goal #3.
- `crates/app/src/waiting.rs:167-172` — `was_waiting && !is_waiting` ⇒ `slot.waiting = None`.
  So **any** transition out of `Waiting`, including into `Unknown`, drops a pending band
  item. `crates/app/src/terminal_app.rs:1061-1062` shows this is *established intent*, not an
  accident: "A tab with no live agent maps to `Unknown` (not `Waiting`), so an agent that
  finished … is seen as leaving `Waiting` — a falling edge." Load-bearing for D10.
- `crates/app/src/sidebar_model.rs:132-142` — `mark_for` maps **both** `Unknown` and `Busy`
  to `RowMark::Agent`, so "agent present, activity unknown" is not visually distinct from
  "working". Pre-existing (R6.6); bounds what D1's tier 2 can communicate.

### Facts that constrain the implementation (verified, not assumed)

- **The panel does not re-read the start time per wake.** `overlay.rs:968` validates against
  `prev.process_start_ms`, cached in `OpenedOn` at open (`overlay.rs:1006-1009`). So the
  **14 µs** figure at `overlay.rs:333` measures `registered_session` *alone*; a caller that
  must also establish the process start pays a `proc_pidinfo` on top.
- **`mod proc;` is private** (`crates/app/src/tabmeta/mod.rs:20`), while `tmux` is
  `pub(crate)` with a comment explaining why (`mod.rs:21-24`). Anything `tabsession` consumes
  from `proc` needs the same treatment.
- **`tabsession`'s own tests call `start_ms` directly without `probe::serial()`**
  (`tabsession.rs:726, 733`), and `tabsession::ProcTable` shells out to `/bin/ps`. This is
  the exact hazard already documented at `worker.rs:566-576` — a process-global counter
  bumped from tests that do not hold the serial guard.
- **`tabmeta::proc::proc_args` already reads argv via `sysctl`** (`proc.rs:162`) and
  `agent.rs:191` already calls it. An argv (`--resume <id>`) identity tier is therefore
  available inside `tabmeta` without the `/bin/ps` fork that `hint_for` pays.
- **`tabsession::hint_for` has a cwd gate that the tabmeta walk does not.**
  `choose_claude` (`tabsession.rs:194-217`) admits a pane's `claude` only on a positive
  `cwd_of(pane) == cwd_of(shell)` match; `worker.rs:84-93` re-roots with no such gate. The
  two can therefore select different processes for the same tab.

### The session-identity source (already built, already proven)

- `crates/ai-monitor/src/lib.rs:87-101` — `registered_session_in(sessions_root, pid)` /
  `registered_session(pid)` read `~/.claude/sessions/<pid>.json` into
  `Registered { session_id, cwd, started_at_ms }`. Any missing field / bad JSON / unreadable
  file yields `None`.
- `crates/ai-monitor/src/lib.rs:110-126` — `REGISTRY_DRIFT_MS = 60_000` and
  `validated_session_id(reg, process_start_ms)`; the `checked_sub` is the stale-file test
  (a record written *before* the process started cannot describe it). Needed because
  `~/.claude/sessions/` files outlive their processes — measured 5 files for 4 live
  processes.
- `crates/app/src/tabsession.rs:113-131` — `registered_id` / `registered_id_with` is the
  panel's use of exactly this pair, with the read injected for testability.
- `crates/app/src/tabsession.rs:435-460` — `start_ms(pid)` via
  `proc_pidinfo(PROC_PIDTBSDINFO)`. **Module-private today**, and the only implementation in
  the tree. `crates/app/src/tabmeta/proc.rs:113-145` already calls the same
  `PROC_PIDTBSDINFO` for `tty_dev_of_pid`, so `tabmeta` can host it without a new syscall
  surface. `libc = "0.2.186"` is already an `app` dependency (`crates/app/Cargo.toml:17`).
- `crates/app/src/overlay.rs:953-976` — `session_rotated_with` re-reads the registry for the
  attached pid **on every wake**, and `overlay.rs:333` records the measured cost:
  **14 µs** per read. That is the cost evidence for doing this per tick instead of memoising
  it.

### Live verification of the mechanism (2026-08-10, this machine)

`~/.claude/sessions/` held 4 records; mapping each `sessionId` to
`~/.claude/projects/<encode_cwd(cwd)>/<sessionId>.jsonl`:

| pid | session | log exists | project |
|-----|---------|-----------|---------|
| 2499 | `02a3919c…` | yes | `terminal` |
| 7748 | `26fa908b…` | yes | `terminal-fix-session-card-label` |
| 8573 | `71c62a68…` | yes | `harness-ops-fix-antigravity-cli-skills` |
| 11395 | `064c3591…` | **no** | `terminal-fix-ai-monitor-no-session` |

What this pins down: **the pid → session id → log path chain resolves directly, with no
ranking.** Three of four records name a log that exists, at the path this feature computes.

The 4th row is the "**named but not yet written**" state — a validating record whose log did
not exist, 95 s after that process started.

> **Correction (2026-08-11, after #28).** This row was originally read as evidence that the
> unwritten window is *long* — "not a sub-second window that can be waved away" — and D1's
> rationale leant on the 95 s figure. That reading is now superseded and the figure is
> **withdrawn**.
>
> `#28 fix(pty): give each tab its own agent session identity, not the launcher's` found the
> real cause: `CommandBuilder::new` seeds from `std::env::vars_os()`, so an app launched from
> inside a Claude session leaked `CLAUDE_CODE_CHILD_SESSION=1` / `CLAUDE_PID` into every tab.
> The tab's `claude` read the inherited marker, called itself a nested child session, and
> **suppressed its transcript entirely**. Pid 11395 was in that state: it was never going to
> write a log, at 95 s or at any later time. So the row measures the env-leak bug, not the
> latency of a healthy session.
>
> **What survives, and why the tier is unchanged.** The window itself is real and is not in
> dispute — a session cannot have a log before it writes its first event, and the registry
> record appears 1.1–2.2 s after process start (measured independently, `lib.rs:329-336`).
> What is no longer evidenced is any claim about *how long* the window lasts. Tier 2's rule
> never depended on the duration: it turns on the *authority* of the name, not on how long the
> file has been missing. A one-second window and a ninety-second one take the same answer,
> because ranking during either returns a sibling's log.
>
> Two consequences worth stating rather than leaving implicit. Post-#28 the window should be
> short in the ordinary case, so tier 2 will be **rarer** than this section originally implied.
> And a tab whose `claude` genuinely never writes — one the user starts and never types into —
> still sits in tier 2 indefinitely; that case is unaffected by #28 and remains the reason the
> tier must be terminal-safe rather than time-bounded.

### Precedent for what to do when the named log is absent

`crates/ai-monitor/src/lib.rs:313-327` — tier 1 of `resolve_owned_session_with`: a known
session id whose file does not exist returns `None`, deliberately, and the comment states
why: "A session id here is *authoritative* … if its log does not exist yet, the honest
answer is 'not written yet', NOT a timing guess. The fall-through this used to do is what
let a tab with a perfectly known session id adopt a sibling's log."

### Existing behaviour the fix must preserve

- `activity.rs:152-179` — `RESOLVE_TTL_TICKS = 20` (≈10 s) memo on the expensive `read_dir`.
- `activity.rs:186-229` — the role memo revalidates on `(mtime, len)`: unchanged log ⇒ one
  `stat`, no read (R6.7). The wider `recent_prompts` scan (`PROMPT_SCAN_CAP` = 1 MiB,
  `crates/ai-monitor/src/activity.rs:102`) runs only on a changed log.
- `activity.rs:199-205` — label carry-forward (tab-identity D6): a fresh tail with no
  substantive prompt keeps the previous label.
- `worker.rs:36-38` — `should_refresh`: active tab every 500 ms tick, background tabs every
  4th tick, striped by slot.
- `worker.rs:260-279` — `publish_if_changed`: no wake unless something changed (guards the
  G1 idle-CPU property).

### Tests that will have to change

- `activity.rs:423-813` — every test drives `state_of_cwd(&mut memo, &root, cwd, …)` and
  reads back with `label_of_cwd(&memo, cwd)`. The cost tests
  (`unchanged_log_is_not_reread`, `log_path_memo_expires`) assert against the `LOG_RESOLVE`
  / `LOG_STAT` / `LOG_READ` probe counters (`crates/app/src/tabmeta/mod.rs:364-383`).
- `worker.rs:396-424`, `:499-538`, `:616-651` — cost/toggle tests that call `d.tab(&tp, …)`
  and assert probe counts; a new registry read must be counted or these become dishonest
  (the `LOG_READ_BYTES` comment at `activity.rs:207-214` sets the honesty bar).
- `crates/ai-monitor/src/lib.rs:1505-1513` — `resolve_log_in`'s own test; the function stays
  (it is the fallback) and is otherwise called only from `activity.rs:169`.

### Project constraints that bound the design

- `.claude/rules/dependency-direction.md` — `common ← term-core ← renderer ← app`. All of
  this lives in `app` + `ai-monitor`; nothing here touches `term-core`.
- `CLAUDE.md` — no `unwrap()` outside tests; `cargo clippy --workspace --all-targets -D
  warnings` is the gate.
- `crates/app/src/tabmeta/mod.rs:340-412` — the `probe` module is `#[cfg(test)]` counters
  plus a `serial()` mutex; new I/O gets a new counter there.

## Decisions

### D1: Resolution ladder = named-session-first, with a *scoped* fallback
- **Status**: resolved
- **Decision**: four tiers. Which one applies is decided by *why* identity is missing.

  | Tier | Condition | Candidate | On a missing file |
  |------|-----------|-----------|-------------------|
  | **1** | registry record for the tab's `claude` pid passes `validated_session_id` | `encode_cwd(`**`reg.cwd`**`)/<session_id>.jsonl` | → tier 2 |
  | **1b** | no valid record, but the pid's argv carries `--resume <id>` | `encode_cwd(`**`tab cwd`**`)/<id>.jsonl` | → **tier 3** |
  | **2** | tier 1 named a candidate and the file does not exist | — | **no log**: `task = None`, state `Unknown`. Never rank. |
  | **3** | nothing could be named, or tier 1b's guess missed | today's `resolve_log_in` cwd ranking, unchanged | — |

  **Only tier 1 earns tier 2, and the difference is where the *directory* comes from.** Tier
  1 gets both halves of the path from the same authoritative record, so a missing file proves
  "this session has not written yet". Tier 1b has an authoritative **id** but must guess the
  **directory** from the tab's live cwd — the very source D4 rejects as drift-prone — so a
  missing file proves nothing: it is equally consistent with "resumed from a different
  directory, transcript lives under the origin's project dir". Routing a tier-1b miss into
  tier 2 would strand exactly the tabs tier 1b exists to help in a permanent no-label
  `Unknown`, strictly worse than today. It falls to tier 3 instead, which is where an
  unidentifiable tab belongs.

  The tier-2 test is **not** a separate `is_file`: the candidate path is handed to the
  existing `stat_log` (`activity.rs:285-291`), whose `None` *is* the miss signal, and whose
  `(mtime, len)` is carried forward into `tail_role` rather than re-taken (D7). One `stat`,
  same as today.
- **Rationale**: a named session is *authoritative* — `claude` named it, either in its own
  registry record or in its argv — so "not written yet" is the honest answer where a timing
  guess is not. Same reasoning and nearly the same shape as `ai-monitor`'s tier 1
  (`crates/ai-monitor/src/lib.rs:313-327`), whose comment records that the fall-through this
  avoids "is what let a tab with a perfectly known session id adopt a sibling's log".

  **The rule turns on authority, not on duration** — worth stating precisely, because an
  earlier draft argued it from a measured 95 s unwritten window that #28 has since re-explained
  as an env-leak artefact (see the correction under Research → Live verification). Nothing here
  rests on that figure. Ranking during the window returns a neighbour's log whether the window
  is one second or ninety, and the standing measurement that *does* matter is the directory:
  56 competing logs in one project. A shorter window makes tier 2 rarer; it does not make
  ranking inside it any less wrong.

  **Tier 1b is not decoration.** Without it the card and the panel disagree *permanently* on
  `claude --resume <id>` whose registry record is absent: the panel names the session from
  argv (`tabsession.rs:102`, `table.resume_id`) while the card would rank the directory. The
  usual objection — "that costs a `/bin/ps` fork" — does not apply inside `tabmeta`:
  `proc::proc_args` reads argv by `sysctl` and `agent.rs:191` already calls it. Gated on a
  tier-1 miss, so it costs nothing in the common case.

  **Tier 2's exits, stated because nothing measures its duration.** It ends when the log
  appears (→ tier 1, within one refresh, D3), when the session rotates (`/clear` → a new id,
  also tier 1), or when the process exits (→ `agent: None`, no label). It does **not** end for
  a `claude` the user never types into — that session genuinely never writes a log, and tier 2
  is then *terminal and correct*: agent-present mark, cwd title, no task. That is strictly
  better than today, where such a tab displays a neighbour's task. This "never writes" case is
  the one that survives #28 untouched, and it is why the tier is designed to be safe when
  terminal rather than bounded by a timeout.

  **Rejected — "never rank"** (drop tier 3): strictest isolation, but a card goes blank on any
  registry hiccup, and the registry is another program's undocumented on-disk detail that
  `Registered`'s own doc says "must never be load-bearing beyond 'use it when it is there'"
  — measured 1 of 5 live processes with no record after 1h17m (`lib.rs:334-341`).

  **Rejected — "always rank on any failure"**: keeps the reported bug alive in exactly the
  window it was reported in.

  **Rejected — rank-with-sibling-exclusion in tier 2** (rank the directory but drop every log
  claimed by another live `claude`). This is the strongest objection to tier 2 and it is not
  a timing guess, so D1's "a guess is not honest" argument does not answer it; the machinery
  exists (`ai_monitor::Rival` / `claim_of`, `lib.rs:531-566`) and under D5+D3 every sibling
  tab's validated session id is already in hand. It is rejected on **scope and shape**, not
  on principle: (i) it would put cross-tab state into a strictly per-tab derivation — the set
  would have to come from the *previous* pass, since `Deriver::pass` derives tabs in roster
  order, so it is one tick stale by construction; (ii) it does not address the *foreign*-app
  logs (Warp, iTerm) that share the directory, so tier 3 stays unsafe either way; and
  (iii) tier 2 is reached only for a session that has been named and has written nothing —
  for which "no task yet" is the accurate answer, not a degraded one. Recorded in Known Gaps
  as the identified follow-up if tier 3 is ever hardened.

  **On tier 2's legibility** (the other half of that objection): `mark_for`
  (`sidebar_model.rs:132-142`) maps `Unknown` to the agent-present mark, so the card does say
  "an agent is running here"; what it cannot currently say is "…and it has not started yet",
  because `Unknown` and `Busy` share a mark. That is pre-existing (R6.6) and out of scope
  here; noted in Known Gaps rather than silently inherited.

### D2: Memo key = the resolved log path, not the cwd
- **Status**: resolved
- **Decision**: `ActivityMemo.role` becomes keyed on the resolved `.jsonl` path. Two tabs on
  different sessions can no longer collide; two tabs genuinely on one session (both attached
  to the same tmux pane, or one session resumed twice) share one entry, which is correct
  caching rather than the defect. `MEMO_MAX` / `MEMO_TTL_TICKS` / the `(mtime, len)`
  revalidation are untouched.
- **Rationale**: the key type stays `PathBuf`, so this is the smallest change that removes
  the collision, and it keys on the thing actually being cached (a file's tail) rather than a
  proxy for it. The label carry-forward then lands where it belongs: it follows the *session*,
  so a `/clear` rotation starts a fresh key with no inherited label — today's cwd key would
  leak the pre-`/clear` task label into the new session.
  **Rejected — session-id string**: equivalent isolation, but tier 3 has no session id, so it
  would need a second key space or a synthetic key for the fallback.
  **Rejected — `TabId`**: guarantees isolation but re-reads and re-derives the same log once
  per tab when tabs share a session, and ties entry lifetime to tab close rather than the
  existing reclaim-by-disuse.

### D3: Identity is re-derived every refresh — nothing about it is memoised
- **Status**: resolved
- **Decision**: for a tab with a detected `AgentKind::Claude`, every refresh performs, in
  order: `proc::start_ms_of_pid(claude_pid)` →
  `ai_monitor::registered_session_in(sessions_root, claude_pid)` → `validated_session_id`,
  then on a miss `proc::proc_args(claude_pid)` for `--resume`. No TTL, no memo, on **any** of
  it. Active tab = 2 Hz, background = 0.5 Hz (`worker.rs:36-38`, unchanged).
  (The `_in` variant, not `registered_session` — D7 threads `sessions_root` so the tiers stay
  hermetically testable.)
- **Cost, stated honestly** — per Claude tab per refresh:

  | Call | When | Cost |
  |------|------|------|
  | `proc_pidinfo(PROC_PIDTBSDINFO)` | always | one syscall, same class as the `cwd_of_pid` / `exe_path` the tab already pays |
  | `registered_session_in` (open+read+parse ~400 B) | always | **14 µs** measured (`overlay.rs:333`) |
  | `proc_args` (`sysctl`) | tier-1 miss only | one syscall; zero in the common case |
  | `stat` of the candidate | always | **not new** — it is `tail_role`'s existing `stat_log` |

  At 20 Claude tabs that is ≈10 registry reads/s ≈ 140 µs/s plus ≈20 syscalls/s.
- **Rationale**: this is the *rotation signal*. `/clear` keeps the pid and swaps the session
  id; the tier-2 → tier-1 transition is the same event from the other side. Memoising it
  means the card paints a finished session for the length of the TTL. The panel re-reads the
  registry per wake for exactly this reason (#27, "follow the session across `/clear`").

  **Why `start_ms` is re-read rather than cached per pid** — and the correction that forces
  the question. The panel does *not* re-read it: `session_rotated_with` validates against
  `prev.process_start_ms` cached in `OpenedOn` at open (`overlay.rs:968, 1006-1009`). It can
  do that because it captured the value while holding the process identity it is asking
  about. The card has no equivalent anchor — `agent::walk` re-discovers the pid each refresh,
  and a cached start time keyed on a pid is precisely what a *recycled* pid defeats, which is
  the sole reason `validated_session_id` exists (`lib.rs:106-110`). Caching the one input
  that makes the validation meaningful would leave the validation as ceremony.

  **Rejected — reuse `RESOLVE_TTL_TICKS` (≈10 s)**: that TTL amortises a `read_dir`, three
  orders of magnitude more expensive; borrowing it here buys ~14 µs and costs 10 s of
  staleness on every `/clear`.
  **Rejected — a new short TTL (~2 s)**: a tuning knob with no measurement behind it.

### D4: The log path is built from the *record's* cwd, not the tab's live cwd
- **Status**: resolved
- **Scope note**: this decision governs **tier 1 only**. Tier 1b has no record, so it has no
  `reg.cwd` to use; it falls back to the tab's live cwd and, precisely because that source is
  drift-prone, a tier-1b miss routes to tier 3 rather than tier 2 (see D1's table).
- **Decision**: tier 1 resolves to
  `projects_root / encode_cwd(reg.cwd) / format!("{}.jsonl", reg.session_id)`. The git branch,
  the card title and `TabMeta.cwd` keep using the tab's live cwd — unchanged.
- **Rationale**: `Registered.cwd` is the directory `claude` registered under, so it names the
  directory `claude` actually writes into; the tab's live cwd is a proxy that drifts
  (`Ctrl+Z` → `cd` → `fg`, or a re-rooted tmux pane). The field is already on `Registered`
  (`crates/ai-monitor/src/lib.rs:56-64`) so this costs nothing.
  **Rejected — the tab's live cwd**: after a drift the path names a directory the session
  never wrote to, tier 1 misses, and the card silently drops to "no label" while the session
  is alive and logging.
  **Rejected — require `reg.cwd == tab_cwd`**: that gate exists in
  `tabsession::choose_claude` because *there* the `claude` was found through a tmux pane and
  might belong to another project. Here the `claude` is already known to be a descendant of
  this tab, so the gate would reject correct identities and fall back to ranking.
  **The display consequence, accepted explicitly.** When the two diverge, the card's line 1
  can carry a task from project B while its branch and cwd title describe project A — and
  `resolve_line1_tier` (`sidebar_model.rs:55-62`) lets the task *displace* the title. This
  weakens the invariant `worker.rs:88-92` states for the rebound root ("branch and agent
  never describe different processes"). It is accepted because the alternative is worse: the
  agent genuinely *is* working in project B, so reporting its task is correct, whereas
  suppressing the label would hide a live agent's work to preserve a cosmetic pairing. Listed
  in Known Gaps.

### D5: The walk returns the agent's pid alongside its kind
- **Status**: assumed (auto-resolved from L1)
- **Rationale**: `agent.rs:162-176` already has the pid in hand at the moment it classifies;
  `agent_of_pid` discards it. Returning it is the whole reason the cheap route is available —
  it avoids `tabsession::hint_for`, which forks `/bin/ps` and is far too costly for a
  per-tick worker. `walk`'s generic-over-`children`/`classify` shape is preserved so the
  depth/node/short-circuit tests keep working against fake trees.

### D6: `start_ms` moves to `tabmeta::proc`; `tabsession` delegates to it
- **Status**: resolved
- **Decision**: `start_ms` becomes `tabmeta::proc::start_ms_of_pid`; `tabsession::start_ms`
  becomes a one-line delegation, preserving its current signature and semantics. Two
  consequences that are part of the decision, not incidental to it:
  1. **`mod proc;` becomes `pub(crate) mod proc;`** (`mod.rs:20`), with the same style of
     comment `tmux` already carries (`mod.rs:21-24`) naming `tabsession` as the consumer.
  2. **`start_ms_of_pid` gets a `PROC_START` probe counter (D9), and the worker cost tests
     assert a *floor* on it, never an equality.** `tabsession`'s own tests call `start_ms`
     directly without `probe::serial()` (`tabsession.rs:726, 733`) and its `ProcTable` shells
     out to `/bin/ps`, so a shared process-global counter can be bumped concurrently. This is
     the identical hazard `worker.rs:566-576` already documents for `PROC_EXE`, and it takes
     the identical remedy — the deterministic shape is pinned against a fake tree instead.
- **Rationale**: `tabsession.rs:435-460` is the only `PROC_PIDTBSDINFO` start-time reader in
  the tree, and it is module-private. `tabmeta/proc.rs:113-145` already calls
  `PROC_PIDTBSDINFO` for `tty_dev_of_pid`, so hosting it there adds no new syscall surface.
  Copying it would put two copies of a validation input in the tree — the exact shape this
  codebase names as having "already produced two defects in this feature"
  (`crates/ai-monitor/src/lib.rs:116-118`). One rule, one function.

### D7: Resolution is threaded explicitly; the two entry points take a log, not a cwd
- **Status**: resolved
- **Decision**: `activity.rs` exposes three functions in place of two, and the worker calls
  them in this order so the borrows do not overlap:

  ```
  // start_ms is OPTIONAL: `start_ms_of_pid` returns None for a dead pid or a future
  // start time. Tier 1 needs it (validation is meaningless without it); tier 1b needs
  // only the pid and its argv. So a None start must fall to tier 1b, not discard the pid.
  struct ClaudeId { pid: u32, start_ms: Option<u64> } // D5's walk + D6's reader

  /// A resolved log AND the stat that proved it exists — so nobody stats it twice.
  struct Found { log: PathBuf, mtime: SystemTime, len: u64 }

  enum Resolution {
      Found(Found),        // tier 1, 1b, or 3
      NamedButUnwritten,   // tier 2 ONLY (tier 1's id + tier 1's dir)
      None,                // no agent, no candidate, empty project dir
  }

  // 1. resolve — &mut for tier 3's cwd-keyed `path` memo. `sessions_root` is threaded
  //    exactly like `projects_root` so the tiers are hermetically testable.
  fn resolve_log(memo: &mut ActivityMemo, projects_root: &Path, sessions_root: &Path,
                 cwd: &Path, claude: Option<ClaudeId>, tick: u64) -> Resolution;
  // 2. state — &mut for the log-keyed `role` memo; takes the stat, never re-takes it
  fn state_of_found(memo: &mut ActivityMemo, f: &Found, tick: u64, now: SystemTime) -> AgentState;
  // 3. label — pure memo read, &, after the role memo is warm
  fn label_of_log(memo: &ActivityMemo, log: &Path) -> Option<String>;
  ```

  Two consequences that are part of the decision:
  - **`tail_role` takes `(mtime, len)` as parameters** instead of calling `stat_log` itself.
    Resolution has to stat the candidate anyway to tell tier 1's "unwritten" from tier 1b's
    "wrong guess", so the stat must travel with the path or D9's `LOG_STAT: 1` and the
    Constraint forbidding a second stat are unachievable.
  - **`sessions_root: &Path` is a parameter, not `$HOME`.** `resolve_log` calls
    `ai_monitor::registered_session_in(sessions_root, pid)`, not `registered_session(pid)`.
    `Deriver` holds `sessions_root: Option<PathBuf>` beside its existing `projects_root`.
  - **`ai_monitor::claude_sessions_dir()` is added**, beside its `claude_projects_dir()` twin
    (`lib.rs:129-132`). Today the sessions root is built inline inside `registered_session`
    (`lib.rs:99-100`), so `Deriver` would otherwise construct `$HOME/.claude/sessions` a
    second time — the duplicated-derivation shape D6 exists to avoid.
- **Rationale**: today `worker.rs:110-115` gets the label by calling `label_of_cwd` with the
  same cwd `activity()` used one statement earlier — an undeclared coupling through a shared
  map key, held together by a comment. With D2 the key is no longer derivable from the cwd, so
  the coupling must become a value in a variable. Naming the signatures here (rather than
  leaving "thread it through" to the implementer) is what makes the borrow ordering, the
  single-stat budget, and the tier-2/tier-3 distinction reviewable at L3 instead of
  discovered at compile time.

  **On the `sessions_root` parameter specifically**: `registered_session` reads `$HOME`
  (`lib.rs:98-101`), and `activity.rs`'s entire test suite is hermetic temp-dir based
  (`activity.rs:407-421`), so tiers 1/1b/2 would be untestable if the read went through
  `$HOME`. `registered_session_in` exists for exactly this — its doc says "Split from
  [`registered_session`] so it is testable without a real `$HOME`" (`lib.rs:83-87`) — and it
  is the same seam `projects_root` already uses one line above. Rejected the alternative of
  injecting a `read: impl Fn(u32) -> Option<Registered>` closure (the `_with` idiom of
  `registered_id_with` / `session_rotated_with` / `resolve_owned_session_with`): those inject
  because their callers have no root to thread, whereas this function is already
  root-threaded, and adding a generic parameter to a hot per-tick function to re-solve a
  solved problem is the worse of two sanctioned patterns.

### D8: `ActivityMemo` keeps two key spaces, deliberately different
- **Status**: assumed (implied by D1 + D2)
- **Rationale**: `role` is keyed on the resolved log path (D2). `path` — the memo that
  amortises the expensive `read_dir` — stays keyed on cwd, because it now serves **tier 3
  only**, where there is no session id and the cwd is all there is. Two maps with two key
  meanings is worth one sentence of doc rather than one artificial shared key.

### D9: Two new probe counters, named, with the new steady-state budget stated
- **Status**: resolved
- **Decision**: add exactly two counters to `mod.rs`'s `probe` module —
  **`PROC_START`** (bumped by `proc::start_ms_of_pid`) and **`SESSION_REG`** (bumped by the
  registry read). `proc_args` already has `PROC_ARGS`; the candidate `stat` already has
  `LOG_STAT`. The revised steady-state budget the cost tests assert, for **one warm Claude
  tab in tier 1 whose log has not changed**:

  | Counter | Before | After |
  |---------|--------|-------|
  | `LOG_STAT` | 1 | 1 (unchanged — the candidate stat *is* the revalidation stat) |
  | `LOG_READ` | 0 | 0 |
  | `LOG_RESOLVE` | 0 | **0** (tier 1 never calls `read_dir`; it is now tier-3-only) |
  | `SESSION_REG` | — | 1 |
  | `PROC_START` | — | ≥1 (floor, per D6(2)) |
  | `PROC_ARGS` | k | k (tier-1 hit ⇒ tier 1b's gate stays closed) |

  `k` is **not** zero in general and the row must not claim it is: `agent::classify_pid`
  already calls `proc_args` for interpreter-launched agents, and `node
  …/@anthropic-ai/claude-code/cli.js` — the npm install `agent.rs:224-226` treats as the
  canonical case — is exactly that. So `k = 0` for a native `claude` install and `k ≥ 1` for
  an npm one, *before and after*. What this feature must not do is raise `k`, which is what
  gating tier 1b on a tier-1 miss guarantees.
- **Rationale**: the existing cost tests assert exact syscall counts and
  `activity.rs:207-214` sets the honesty bar explicitly ("Honest wrapper accounting").
  Uncounted I/O would let `unchanged_log_is_not_reread` and
  `a_non_tmux_tab_costs_exactly_the_gate_and_spawns_nothing` keep passing while the per-tick
  cost grew. Note the one genuine *saving*: a tier-1 tab stops calling `read_dir` entirely,
  so `LOG_RESOLVE` drops to 0 where today it fires every `RESOLVE_TTL_TICKS`.

### D10: Tier 2 is `Unknown`; publication stays change-only; the waiting-band edge is intended
- **Status**: resolved
- **Decision**: three separate, individually testable claims.
  1. A tier-2 candidate yields `AgentState::Unknown` and `task: None`.
  2. **`publish_if_changed` must not fire on an unchanged tab.** The property is not "the
     environment is steady" — it is that *nothing D3 reads reaches `TabMeta`*. The registry
     read's outputs (`session_id`, `start_ms`) are consumed entirely inside resolution; only
     the resolved log's *derived* `(agent, task)` is published. So a tab whose session and log
     are unchanged publishes an identical `TabMeta` no matter how often identity is
     re-derived. Testable by counting wakes across N ticks with a static fixture, not by
     assuming a quiet machine.
  3. Entering tier 2 from `Waiting` **does** clear that tab's pending waiting-band item
     (`waiting.rs:167-172`). This is correct, not collateral: the transitions *this feature
     creates* are a `/clear` (the awaited turn is over) and a fresh session start (nothing was
     pending). It is also the *established* reading of `Unknown` — `terminal_app.rs:1061-1062`
     already treats "no live agent → `Unknown`" as a deliberate falling edge. Transitions this
     feature does **not** create — an externally deleted or pruned transcript — can drop a
     band item while its question is still on screen; recorded in Known Gaps rather than
     claimed away by an exhaustive-sounding enumeration.
- **Rationale**: `mod.rs:77-82` fixes what `Unknown` means ("asserts presence only … never
  'ready'"), which is exactly a named session that has not written yet. Claim 2 was
  originally justified by an assumption about the environment; restating it as a property of
  the data flow is what makes it a requirement instead of a hope.

### D11: Sibling order in the walk is made deterministic; depth already decides the rest
- **Status**: resolved
- **Decision**: `proc::children_of` returns its pids **sorted ascending**, so `walk`'s
  traversal is fully deterministic. Depth still decides across levels; the sort only fixes
  the order *within* a level.
- **Rationale**: BFS already gives the right answer across depths — a `claude` the user
  launched (depth 1–2) is classified before one a tool call spawned (depth 3). What it did
  **not** give is a defined answer for two `claude` processes at the *same* depth (one
  suspended with `Ctrl+Z`, another started in the same shell): `proc_listchildpids`
  (`proc.rs:60-69`) returns unsorted kernel order, so the winner — and therefore *which
  session id the card shows* — could differ between runs. That was tolerable when the walk
  only answered "is an agent present"; it is not once the walk picks an identity. Sorting is
  one line, costs nothing at these sizes, and converts nondeterminism into a stated,
  testable rule. Which of two same-depth siblings *ought* to win is a separate question and
  is listed in Known Gaps.
  **Second consumer, considered and accepted**: `tmux::tmux_client_of_with` (`tmux.rs:83-89`)
  also iterates `children_of` and returns its first match, so the sort reorders it too. That
  is an improvement of the same kind — a tab with two tmux clients under one shell currently
  picks one in kernel order — and it is named here so the change is not discovered as an
  unrelated side effect during review.

## Constraints

- `term-core` is not touched; the change lives entirely in `app` + `ai-monitor`
  (`.claude/rules/dependency-direction.md`).
- No new crate dependencies. `libc` and `serde_json` are already present.
- **No `/bin/ps` fork on the worker tick** — `tabsession::hint_for` stays out of `tabmeta`.
- No `unwrap()` outside tests; `cargo clippy --workspace --all-targets -- -D warnings` must
  stay clean (`CLAUDE.md`).
- `TabMeta`'s published shape does not change — no new fields, so no renderer/snapshot churn.
- **No new `stat` per refresh.** The tier-1/1b existence test must reuse `tail_role`'s
  existing `stat_log`; a separate `is_file` would double the per-tab stat count and is
  forbidden by D9's budget table.
- `mod proc;` becomes `pub(crate)` (D6) — the only visibility change; no other `tabmeta`
  internals are exposed.
- `publish_if_changed` must remain silent on an unchanged pass; an unconditional wake would
  reintroduce the G1 idle-CPU debt.
- `ai-monitor` stays UI/GPU-free and read-only on the logs.
- The ai-monitor panel's resolution path (`tabsession`, `resolve_owned_session`,
  `session_rotated_with`) keeps its current behaviour; `D6`'s move of `start_ms` must be a
  pure delegation with no semantic change.
- The label *content* policy (`pick_label`, `is_substantive`, `is_continuation`,
  `MIN_LABEL_CHARS`) is untouched.

## Known Gaps

- **Tier 3 is unchanged, and it is still wrong in two ways.** It ranks by mtime
  (`newest_jsonl`), so with two live sessions in one project it does not merely pick one — it
  **flaps** between them as each writes. And it can adopt a *foreign* application's log
  (Warp, iTerm, VS Code write into the same directory). Both are pre-existing; this feature
  deliberately does not touch tier 3, because tiers 1/1b/2 remove every case in which a tab
  that *can* be identified reaches it. Tier 3 is now the fallback for genuinely
  unidentifiable tabs only: the pre-registry window (measured 1.1–2.2 s) with no `--resume`
  in argv, and `claude` builds too old to write `~/.claude/sessions/<pid>.json`.
- **The identified follow-up for tier 3** is rank-with-sibling-exclusion (D1's rejected
  alternative): the ingredients — `ai_monitor::Rival` / `claim_of` (`lib.rs:531-566`) plus
  every sibling's validated session id, already read by D3 — are in place. It needs cross-tab
  state in `Deriver` and would be one pass stale, so it is its own feature, not a rider here.
- **Tier 2 is not visually distinct from "working".** `mark_for` maps both `Unknown` and
  `Busy` to `RowMark::Agent` (`sidebar_model.rs:132-142`), so "an agent is here but has not
  started" reads as "an agent is here and busy". Pre-existing (R6.6) and out of scope; if it
  becomes a complaint, the fix is a fourth `RowMark`, not a change to D1.
- **Two `claude` processes at the same depth in one tab**: D11 makes the choice deterministic
  (lowest pid) but does not claim it is the *right* one. A suspended `Ctrl+Z` session with a
  lower pid will out-rank the foreground one the user just started. No evidence yet that this
  occurs in practice; revisit if it does, with start-time as the natural tie-break since D3
  already reads it.
- **A `claude` deeper than `MAX_DEPTH = 3` from the tab root is not detected at all**
  (`agent.rs:23`). Pre-existing; such a tab has no agent, hence no label, before and after.
- **The card and the panel can still select different `claude` processes for one tab** —
  a *structural* divergence, not the sub-tick timing difference an earlier draft of this
  section claimed. `tabsession::choose_claude` gates a tmux pane's `claude` on a positive
  cwd match (`tabsession.rs:194-217`); `worker.rs:84-93` re-roots with no such gate. For a
  pane working in a different project the two pick different processes, hence different
  sessions, and they do not converge. Tier 1b closes the *other* divergence the panel had
  (argv `--resume` with no registry record); this one remains and belongs to whichever
  feature next unifies the two identity paths.
- **An externally deleted or pruned transcript drops that tab's waiting-band item.** If the
  resolved `.jsonl` disappears while an agent is genuinely blocked on `AskUserQuestion`, the
  tab falls to `Unknown`, `waiting.rs:167-172` reads a falling edge, and the band item is
  cleared while the question is still on screen. This is *not* a transition this feature
  creates (D10 claim 3 enumerates only `/clear` and session start), and the same falling-edge
  rule already governs an agent that exits; recorded so the enumeration is not mistaken for
  an exhaustive one.
- **`TabMeta.cwd` and `TabMeta.task` can describe different projects** after a shell `cd`
  that outlives its `claude` (D4). Accepted there; recorded here so it is not rediscovered
  as a bug.
- **A tab whose `cwd_of_pid` fails renders `Unknown` with no task, even when tier 1 could
  have named its log.** D4 made tier 1 independent of the tab's live cwd — both halves of the
  path come from the registry record — but `worker.rs`'s `Deriver::activity` still gates the
  whole activity path on `Some(cwd)`, because `resolve_log`'s signature (D7) takes
  `cwd: &Path` unconditionally for tiers 1b and 3. Found during verification and **not**
  fixed here: changing that signature is a change to an approved D7 seam, which this feature's
  loop contract classes as escalate-not-autofix. Exposure is narrow — `cwd_of_pid` failing on
  a pid the agent walk just succeeded on essentially means the process is exiting mid-
  derivation, at which point the agent is about to disappear anyway.

## Requirements

Every requirement names the decisions it discharges. Tests are hermetic — hand-rolled temp
dirs (`activity.rs:407-421`) and fake process trees (`agent.rs:337-340`) — because the
`app` crate has no GPU and the worker must stay headlessly testable.

### R0: Two tabs in one project render independent identity (Confirmed Goal 1–3)

#### R0.1: Two sessions in one directory yield two different labels
- **Given**: a projects root containing `<enc-cwd>/A.jsonl` (substantive prompt "refactor the
  auth module") and `<enc-cwd>/B.jsonl` (substantive prompt "fix the flaky test"), and a
  sessions root with records naming session A for pid 101 and session B for pid 202, both
  validating against their pids' start times
- **When**: a derivation pass runs for two tabs in that same cwd, one resolving to pid 101 and
  the other to pid 202
- **Then**: tab 1's `TabMeta.task` is `Some("refactor the auth module")` and tab 2's is
  `Some("fix the flaky test")` — the two are not equal

#### R0.2: Two sessions in one directory yield two independent activity states
- **Given**: the same two-session fixture, where A's newest record is an assistant `end_turn`
  and B's newest record is an assistant `tool_use`
- **When**: the same derivation pass runs
- **Then**: tab 1's state is `Waiting` and tab 2's is `Busy`

#### R0.3: The reported regression cannot recur — a written-but-promptless new session does not inherit its neighbour's label
- **Given**: log A carries a substantive prompt and has an older mtime; log B (a different
  session) is newer by mtime but contains only an assistant line and no substantive prompt;
  both are named by validating registry records for two different pids
- **When**: the tab owning session B is derived, after a prior pass warmed the memo for the tab
  owning session A
- **Then**: tab B's `task` is `None` (not A's prompt), because the carry-forward is looked up
  under B's log key, which has no prior label

### R1: The walk yields a usable, deterministic Claude identity (D5, D6, D11)

#### R1.1: The walk reports the matching pid alongside the kind
- **Given**: a fake process tree where pid 1's child pid 2 classifies as `Claude`
- **When**: `walk(1, …)` runs
- **Then**: it returns both `AgentKind::Claude` and pid `2`

#### R1.2: Sibling order is deterministic
- **Given**: a fake tree whose root reports children in the order `[7, 3, 9]`, all classifying
  as `Claude`
- **When**: the walk runs twice with the provider returning that same unsorted order
- **Then**: both runs select pid `3` (the lowest), and the selection is stable across runs

#### R1.3: Depth still outranks pid order
- **Given**: a fake tree where a depth-2 node with pid `2` and a depth-1 node with pid `50`
  both classify as `Claude`
- **When**: the walk runs
- **Then**: pid `50` is selected — the sort orders *within* a level and never across levels

#### R1.4: `start_ms_of_pid` lives in `tabmeta::proc` and `tabsession` delegates to it
- **Given**: the running test process
- **When**: `proc::start_ms_of_pid(std::process::id())` and `tabsession`'s start-time reader
  are both called
- **Then**: both return the same `Some(ms)`, and a dead pid (`u32::MAX`) returns `None` from
  both — one implementation, two call sites

#### R1.5: A start time that cannot be read does not discard the pid
- **Given**: a detected Claude pid whose start time reads `None`
- **When**: identity is derived
- **Then**: `ClaudeId.start_ms` is `None`, tier 1 is skipped (validation is impossible) and
  tier 1b is still attempted with the pid's argv

#### R1.6: The tmux client scan inherits the same determinism
- **Given**: a fake tree whose root has two tmux-classified children reported unsorted
- **When**: `tmux_client_of_with` runs
- **Then**: it selects the lower pid, stably across runs

### R2: The four-tier resolution ladder (D1, D4)

#### R2.1: Tier 1 resolves by name, from the record's own cwd
- **Given**: a sessions root with `<pid>.json` naming session `S` with `cwd = /proj/a`, a
  start time within `REGISTRY_DRIFT_MS` of the process, and
  `<projects_root>/<enc(/proj/a)>/S.jsonl` on disk
- **When**: `resolve_log` runs for that pid with a *tab* cwd of `/proj/b`
- **Then**: it returns `Found` pointing at `<enc(/proj/a)>/S.jsonl` — the record's cwd, not
  the tab's

#### R2.2: Tier 1 never consults the directory
- **Given**: the tier-1 fixture above, plus five other `*.jsonl` files in the same project
  directory, one with a much newer mtime
- **When**: `resolve_log` runs
- **Then**: the named log is returned and `LOG_RESOLVE` is `0` — no `read_dir` occurred

#### R2.3: A stale record is rejected and does not reach tier 1
- **Given**: a `<pid>.json` whose `startedAt` precedes the process start (a recycled pid), and
  a log matching its session id on disk
- **When**: `resolve_log` runs
- **Then**: `validated_session_id` rejects it, tier 1 does not fire, and resolution proceeds to
  tier 1b / tier 3

#### R2.4: Tier 1b names the session from argv when no record validates
- **Given**: no `<pid>.json`, argv `["claude", "--resume", "S"]` for that pid, and
  `<projects_root>/<enc(tab cwd)>/S.jsonl` on disk
- **When**: `resolve_log` runs
- **Then**: it returns `Found` pointing at that log, and `LOG_RESOLVE` is `0`

#### R2.5: Tier 1b is gated on a tier-1 miss
- **Given**: a validating record for the pid (tier 1 hits)
- **When**: `resolve_log` runs
- **Then**: `PROC_ARGS` is not incremented by resolution — the argv read never fires

#### R2.6: Tier 2 — a tier-1 name whose log is absent yields nothing, never a ranked guess
- **Given**: a validating record naming session `S`, `S.jsonl` **absent**, and two other
  `*.jsonl` files present in the same project directory
- **When**: `resolve_log` runs
- **Then**: it returns `NamedButUnwritten`; the derived state is `Unknown`, `task` is `None`,
  and `LOG_RESOLVE` is `0` — neither neighbour is adopted

#### R2.7: Tier 1b's miss routes to tier 3, not tier 2
- **Given**: no validating record, argv naming session `S`, `<enc(tab cwd)>/S.jsonl`
  **absent**, and one other `*.jsonl` present in that directory
- **When**: `resolve_log` runs
- **Then**: it returns `Found` pointing at the ranked neighbour — a guessed directory's miss
  proves nothing, so the tab is treated as unidentifiable rather than stranded

#### R2.8: Tier 3 is byte-for-byte today's behaviour
- **Given**: no record, no `--resume` in argv, and a project directory with several logs
- **When**: `resolve_log` runs
- **Then**: the result equals `ai_monitor::resolve_log_in(projects_root, tab_cwd)`, and the
  `RESOLVE_TTL_TICKS` memo still suppresses the `read_dir` on a second call within the TTL

#### R2.9: No agent, or no resolvable anything, degrades without panic
- **Given**: a tab whose walk finds no Claude, and separately a tab whose project directory
  does not exist
- **When**: derivation runs
- **Then**: the first yields `agent: None, task: None`; the second yields `Unknown` with
  `task: None`; neither panics

### R3: Memo keying follows the session (D2, D8)

#### R3.1: The role memo is keyed on the log path
- **Given**: two logs in one project directory, each warmed by a derivation pass
- **When**: `label_of_log` is called for each path
- **Then**: each returns its own log's label; neither returns the other's

#### R3.2: Two tabs on one session share a single memo entry
- **Given**: two tabs whose resolution yields the identical log path, and a pass that warms the
  memo for the first
- **When**: the second tab is derived in the same tick
- **Then**: `LOG_READ` is `0` for the second — the entry is shared, not duplicated

#### R3.3: Carry-forward does not cross sessions
- **Given**: a memo warmed with log A carrying label "implement the parser", and log B (a
  different session) whose tail has no substantive prompt
- **When**: a tab resolving to log B is derived
- **Then**: `label_of_log(memo, B)` is `None` — A's label is not carried onto B

#### R3.4: Carry-forward still works within one session
- **Given**: log A warmed with label "fix the flaky test"
- **When**: a continuation-only turn ("go") is appended to A and the tab is re-derived
- **Then**: `label_of_log(memo, A)` is still `Some("fix the flaky test")` — tab-identity D6 is
  preserved unchanged

#### R3.5: The `path` memo stays cwd-keyed and serves tier 3 only
- **Given**: a tier-1 tab and a tier-3 tab
- **When**: both are derived twice
- **Then**: the tier-3 tab's second pass reuses its cwd-keyed entry (`LOG_RESOLVE` `0` within
  the TTL) while the tier-1 tab never creates one

#### R3.6: Reclaim and eviction still bound the memos
- **Given**: a memo with entries untouched for `MEMO_TTL_TICKS`, and separately more than
  `MEMO_MAX` distinct log keys
- **When**: `reclaim` runs
- **Then**: stale entries are dropped and the map is capped at `MEMO_MAX`, oldest-touched first
  — unchanged by the key change

### R4: Identity is re-derived every refresh, so rotation is followed (D3)

#### R4.1: `/clear` is followed within one refresh
- **Given**: a tab in tier 1 on session A with label "old task", where the registry record for
  the same pid is then rewritten to name session B whose log carries "new task"
- **When**: the very next refresh of that tab runs
- **Then**: `task` is `Some("new task")` — no TTL is waited out

#### R4.2: Tier 2 is promoted to tier 1 on the first write
- **Given**: a tab in tier 2 (record names `S`, `S.jsonl` absent)
- **When**: `S.jsonl` appears with a substantive prompt and the next refresh runs
- **Then**: the tab resolves `Found`, and `task` becomes that prompt

#### R4.3: Nothing about identity is memoised
- **Given**: a warm tier-1 tab
- **When**: two consecutive refreshes run
- **Then**: `SESSION_REG` increments on **both** — exactly once per refresh, never skipped

### R5: The per-refresh cost is bounded and honestly counted (D3, D7, D9)

#### R5.1: A warm tier-1 tab performs exactly one stat and no read
- **Given**: a tier-1 tab whose log is unchanged since the previous pass
- **When**: it is refreshed
- **Then**: `LOG_STAT == 1`, `LOG_READ == 0`, `LOG_RESOLVE == 0`, `SESSION_REG == 1`

#### R5.2: Resolution's stat is the only stat — `tail_role` does not re-take it
- **Given**: the same warm tier-1 tab
- **When**: it is refreshed
- **Then**: `LOG_STAT` is `1`, not `2` — `tail_role` consumes the `(mtime, len)` carried on
  `Found`

#### R5.3: The argv read does not fire on a tier-1 hit
- **Given**: a warm tier-1 tab, with `PROC_ARGS` sampled before the pass
- **When**: it is refreshed
- **Then**: `PROC_ARGS` is unchanged by resolution (any nonzero baseline belongs to
  `agent::classify_pid` for an interpreter-launched install, before and after)

#### R5.4: `PROC_START` is asserted as a floor
- **Given**: a warm tier-1 tab
- **When**: it is refreshed under `probe::serial()`
- **Then**: `PROC_START >= 1`, asserted as a floor and never an equality, because
  `tabsession`'s tests bump the same counter without holding the guard

#### R5.5: A tab with no Claude agent pays nothing new
- **Given**: a tab whose walk finds no agent
- **When**: it is refreshed
- **Then**: `SESSION_REG == 0` and `PROC_START == 0` — the identity path is behind the
  Claude gate

#### R5.6: An empty roster still touches no source
- **Given**: an empty roster
- **When**: a pass runs
- **Then**: every probe counter, including the two new ones, reads `0`

### R6: Consumers see per-session truth, and idle stays idle (D10, Confirmed Goal 3)

#### R6.1: A `Waiting` edge raises the band for only the tab whose session waits
- **Given**: two tabs in one project, session A `Waiting` and session B `Busy`, neither active
- **When**: `on_agent_states` receives the derived states
- **Then**: exactly one waiting-band item is raised, for tab A

#### R6.2: Answering one session's question does not clear the other's
- **Given**: the state above with tab A's item raised, and A's log then advancing to a
  `toolUseResult` line
- **When**: the next pass derives and feeds `on_agent_states`
- **Then**: A's item clears and B is unaffected

#### R6.3: Tier 2 yields `Unknown`, which reads as a falling edge
- **Given**: a tab whose session was `Waiting`, whose session then rotates so the tab enters
  tier 2
- **When**: the next pass derives and feeds `on_agent_states`
- **Then**: the state is `Unknown` and the tab's pending item is cleared — the intended
  behaviour, pinned so a future change cannot alter it silently

#### R6.4: An unchanged tab publishes nothing, however often identity is re-derived
- **Given**: a static fixture — unchanged registry record, unchanged log — and a wake counter
- **When**: ten consecutive passes run
- **Then**: the wake counter increments at most once (the first publish) and the generation
  does not advance thereafter

#### R6.5: `TabMeta`'s shape is unchanged
- **Given**: the `TabMeta` struct after the change
- **When**: its fields are enumerated
- **Then**: they are exactly `branch`, `agent`, `cwd`, `task` — no field added, removed or
  retyped, so no renderer or snapshot churn

### R7: Seams stay testable and unduplicated (D6, D7)

#### R7.1: `resolve_log` is hermetic — no `$HOME`
- **Given**: temp `projects_root` and temp `sessions_root`, and a `$HOME` that points at
  neither
- **When**: the full ladder is exercised
- **Then**: every tier resolves against the temp roots, proving the read goes through
  `registered_session_in` rather than `registered_session`

#### R7.2: `ai_monitor::claude_sessions_dir()` exists beside its projects twin
- **Given**: `HOME=/tmp/x`
- **When**: `claude_sessions_dir()` is called
- **Then**: it returns `Some(/tmp/x/.claude/sessions)`, and `None` when `HOME` is unset —
  matching `claude_projects_dir`'s contract, so the path is derived in one place

#### R7.3: `Deriver` holds both roots
- **Given**: a constructed `Deriver`
- **When**: it is inspected
- **Then**: it holds `projects_root` and `sessions_root`, both resolved once at construction,
  and constructing it still performs no I/O

#### R7.4: `proc` is exposed no wider than needed
- **Given**: the module tree after the change
- **When**: `tabmeta/mod.rs` is read
- **Then**: `proc` is `pub(crate)` with a comment naming `tabsession` as the consumer, and no
  other `tabmeta` submodule's visibility changed

#### R7.5: The panel's identity path is unchanged
- **Given**: the ai-monitor panel's existing test suite (`tabsession`, `overlay`)
- **When**: the full workspace test run executes
- **Then**: every pre-existing panel test passes unmodified — D6 is a pure delegation

### R8: The pre-existing quality bars still hold (Constraints)

#### R8.1: The workspace builds clean
- **Given**: the completed change
- **When**: `cargo clippy --workspace --all-targets -- -D warnings` runs
- **Then**: it exits zero, and `cargo test --workspace` is green

#### R8.2: No `unwrap()` outside tests, and no panic on any degraded input
- **Given**: a dead pid, an unreadable registry file, malformed JSON in `<pid>.json`, a
  0-byte log, and a truncated final JSONL line
- **When**: each is derived
- **Then**: each yields a degraded-but-valid `TabMeta` and none panics

#### R8.3: The dependency direction is untouched
- **Given**: the changed crates
- **When**: their manifests and imports are inspected
- **Then**: `term-core` gains nothing, `ai-monitor` remains UI/GPU-free and read-only on logs,
  and no new crate dependency is added

## Tasks

`R8.1` (clippy/test green) and `R8.3` (dependency direction) are obligations of **every**
task, not a task of their own — the worker runs the build gates automatically and the
direction is a review check. They are not listed per-task below.

### T1: Foundations — shared primitives both sides need [infra]
- **Fulfills**: R1.2, R1.4, R1.6, R7.2, R7.4, R7.5
- **Depends on**: (none)
- **Files**: `crates/ai-monitor/src/lib.rs`, `crates/app/src/tabmeta/proc.rs`,
  `crates/app/src/tabmeta/mod.rs`, `crates/app/src/tabsession.rs`
- **Scope**: add `ai_monitor::claude_sessions_dir()` beside `claude_projects_dir`; move
  `start_ms` to `proc::start_ms_of_pid` and reduce `tabsession::start_ms` to a delegation;
  make `mod proc` `pub(crate)` with the `tmux`-style comment; add the `PROC_START` and
  `SESSION_REG` probe counters (and register both in `reset_all`); define the shared
  `ClaudeId { pid: u32, start_ms: Option<u64> }` in `tabmeta/mod.rs`.
- **Note on where the sort lands (D11)**: sort in **`parse_childpids`**
  (`proc.rs:215-225`), not in `walk`. That is the single point every child listing passes
  through, so `children_of` and therefore *both* consumers — `agent::walk` and
  `tmux::tmux_client_of_with` — inherit it, and the rule is provable by a pure unit test on
  `parse_childpids` rather than an awkward one against a live process. R1.2 and R1.6 are
  discharged there; R1.3 (depth beats pid order) stays a `walk` test in T2, because the
  fake-tree provider deliberately bypasses `proc`.

### T2: The walk yields an identity, not just a kind [vertical]
- **Fulfills**: R1.1, R1.3, R1.5
- **Depends on**: T1 — *parallel with T3*
- **Files**: `crates/app/src/tabmeta/agent.rs`
- **Scope**: `walk` and `agent_of_pid` return the matching pid alongside `AgentKind`, keeping
  `walk` generic over its children/classify closures so every existing depth, node-budget and
  short-circuit test still runs against fake trees.

### T3: The resolution ladder and session-keyed memos [vertical]
- **Fulfills**: R2.1–R2.9, R3.1–R3.6, R5.1, R5.2, R7.1, R8.2 (resolution half)
- **Depends on**: T1 — *parallel with T2*
- **Files**: `crates/app/src/tabmeta/activity.rs`
- **Scope**: introduce `Found { log, mtime, len }` and `Resolution`; implement
  `resolve_log(memo, projects_root, sessions_root, cwd, claude, tick)` with tiers 1/1b/2/3 and
  the asymmetric miss-routing; re-key the `role` memo on the log path while `path` stays
  cwd-keyed for tier 3; change `tail_role` to accept `(mtime, len)` instead of taking its own
  stat; rename `state_of_cwd`/`label_of_cwd` to `state_of_found`/`label_of_log`. Rewrite the
  module doc — `activity.rs:10-11` currently states the defect ("Both are shared by cwd, so
  two tabs in one project share them") as the design.
- **Note**: this is where the existing cwd-sharing tests become session-sharing tests. The
  fixture helper `write_log` (`activity.rs:415-421`) is reusable as-is; a sibling
  `write_registry` helper is needed for the temp `sessions_root`.

### T4: Wire the worker and re-baseline the cost tests [vertical]
- **Fulfills**: R0.1–R0.3, R4.1–R4.3, R5.3–R5.6, R6.4, R6.5, R7.3, R8.2 (worker half)
- **Depends on**: T2, T3
- **Files**: `crates/app/src/tabmeta/worker.rs`
- **Scope**: `Deriver` holds `sessions_root` beside `projects_root`; `Deriver::tab` builds
  `ClaudeId` from T2's pid plus `start_ms_of_pid`, calls `resolve_log` **once**, and threads
  the `Found` to both the state read and the label read — deleting the cwd-key coupling at
  `worker.rs:110-115`. Tier 2 maps to `Unknown` + `task: None`. Re-baseline every probe
  assertion in this file against D9's budget table, using a floor for `PROC_START` (D6(2)).
- **Note**: R0.1–R0.3 are the regression tests for the reported bug and belong here, because
  only a full `pass()` over a two-tab roster exercises the defect end-to-end.

### T5: Per-session waiting-band attribution [vertical]
- **Fulfills**: R6.1, R6.2, R6.3
- **Depends on**: T4
- **Files**: `crates/app/src/waiting.rs` (tests only)
- **Scope**: no production change is expected here — `waiting.rs` already keys on `TabId` and
  simply consumed a cwd-shared state. This task proves that with T4's per-session states the
  band raises for one tab only, clears independently, and that the tier-2 `Unknown` falling
  edge behaves as D10 claim 3 specifies. If a production change *is* required, that is a
  finding worth surfacing rather than absorbing — it would mean the waiting band had a second
  defect the cwd-sharing was masking.

## External Dependencies

### Pre-work
- (none) — every input is already on disk (`~/.claude/sessions/`, `~/.claude/projects/`) and
  every API used is already public or made so in T1.

### Post-work
- **`/run-term` two-tab visual check**: open two tabs in one project, start a separate Claude
  session in each, and confirm the cards show two different labels and two independent
  activity marks. This is the one acceptance criterion the hermetic tests structurally cannot
  cover — they use temp roots and fake trees, so the real `proc_pidinfo` → real
  `~/.claude/sessions` → real log chain is only exercised by running it.
- **`/insta-review` is not needed**: no `term-core` grid or `GridSnapshot` behaviour changes,
  so no insta snapshots move.
