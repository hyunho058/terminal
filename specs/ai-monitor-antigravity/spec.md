# Spec: ai-monitor-antigravity

## Meta
- **Created**: 2026-08-13
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-08-13 (AG4, AG6, AG8, AG10 signed off; all other decisions resolved
  on measured evidence). Supersedes the original 2026-08-13 approval, which was granted
  against decisions and acceptance criteria that named fields the log format and the
  `ai-monitor` API do not have.
- **Decision namespace**: `AG*`, not `D*` — see AG0.
- **One approved decision was tightened after the fact**: AG6 was signed off as "FD probe
  plus mtime fallback, defer the DB tier". Follow-up measurement showed the DB *cannot* reach
  a transcript (disjoint id namespaces) and that an mtime fallback mis-attaches to subagents
  under load, so the ladder became FD → `history.jsonl` → unattached. Strictly safer than
  what was approved, and it drops the SQLite dependency entirely — but it is a change to the
  approved text, recorded here rather than folded in silently.

## Revision log

### 2026-08-13 — schema verification pass
Every schema claim in the first draft was checked against the live corpus on this machine
(51 transcripts, 1,761 steps) and against the `ai-monitor` API. What changed:

| Was | Now | Why |
|-----|-----|-----|
| `D2` mapped `WRITE_FILE` step type | `CODE_ACTION` | `WRITE_FILE` does not exist; writes land as `CODE_ACTION` |
| `D2` mapped both `VIEW_FILE` and `view_file` | one counting event: the **result step** | both spellings exist per call — counting both roughly doubles every tally (773 tool-call entries vs 693 result steps) |
| `R2.2` read `AbsolutePath` as a plain string | a string holding a JSON document, parsed once | all 3,470 arg values are strings; 2,757 wrap a quoted string, 713 a bare scalar or array |
| `R2.4` read `Role` / `TypeName` | `Subagents[].Model` / `.Prompt`, named via `define_subagent` | `Role` and `TypeName` appear nowhere in the corpus |
| `R2.1` counted every `PLANNER_RESPONSE` | only those with **no** `tool_calls` | 833 vs 61; the tool-bearing ones are dispatch steps, not turns |
| `R3.2` read a `Model Selection` field | prose scrape inside `USER_INPUT.content` | the model is not a field; the observed value carries an effort tier — `Gemini 3.6 Flash (High)` |
| `R2.x` asserted `turns_user`, `turns_assistant`, `state`, `tool_calls_file/bash/agent` | AG8 (open) | none of those exist on `TelemetrySnapshot` |
| `D3` sized a context window | AG4 (open) | the format carries **no** token counts at all — the numerator is permanently zero |
| `D6`/`R5.1` adapted an existing panel title | AG10 (open) | there is no title element to adapt; it is new UI, and it churns pinned snapshots |
| `Known Gaps: (none)` | eight named gaps | — |
| `External Dependencies: (none)` | still none, for a different reason (AG6) | the DB the first draft leaned on cannot reach a transcript, so no SQLite is needed |

Unchanged and re-verified: the goal, the non-goals, all eight code citations, `SearchPath` /
`DirectoryPath` / `CommandLine` / `TargetFile` as the path-bearing arg keys, and the decision
not to interleave subagent transcripts.

### 2026-08-13 — implementation pass (T1, T2)
T1 and T2 shipped; both re-measured the live corpus and found five errors in the Research
above, all since corrected in place and each independently re-verified before folding in:

| Was | Is | Consequence |
|-----|-----|-------------|
| result steps carry the call's `args` | `tool_calls` is on `PLANNER_RESPONSE` **only** (772/772); result steps have no args | AG2 needs dispatch↔result **correlation**, not a single-record read. R2.3/R2.4 restated. |
| "773 entries vs 693 result steps" | 754 mapped result steps; 693 silently omitted the 61 `GENERIC` that AG2 maps | the real counting-event total is 750 (754 − 4 unidentifiable dispatches) |
| ``to <new>.`` is the model terminator | the value contains periods; ``to (.*?)\.`` yields `Gemini 3` | terminator is a period **followed by whitespace or end-of-string** |
| `history.jsonl` keys are uniform | `type` on 31 of 43, `conversationId` on 12, both on 9 | neither key may be assumed present |
| a `brain/<id>/` implies a transcript | 54 directories, 51 transcripts | R1.6's unattached state is reachable via the fallback tier, not only at cold start |

Also newly recorded: `truncated_fields` breaks the inner JSON of large args (42/360
`CommandLine`) though none of the unwrapped keys is affected; `created_at` is always 20 bytes
so `parse_ts` needed no change; and `exit_code` is confined to `RUN_COMMAND`.

**Independent confirmation of AG3**: folding all 51 real transcripts through the shipped
parser yields `turns_user=54` and `turns_assistant=61` — exactly the counts this spec derived
the turn definition from, with 0 parse errors across all 1,761 lines.

### 2026-08-13 — sign-off pass (AG4, AG6, AG8, AG10)
Approved with the spec's recommendations. Verifying AG6's consequences before writing them in
overturned a claim made in the review that produced this spec, and the correction is load-
bearing enough to state plainly:

> **Retracted**: "subagent conversations appear in `conversation_summaries` with
> `nesting_depth > 0` and `parent_conversation_id` set, so the fallback must filter on it."
> That was inferred from the table's *columns*, not read from its *rows*. Measured: all 102
> rows have `nesting_depth = 0` and an empty `parent_conversation_id`, the known subagent
> child is absent, and the DB's ids do not intersect `brain/` **at all** (102 rows, 54
> directories, 0 shared). The filter it proposed would have been a no-op guarding a query
> that could never have returned a usable path.

What that changed: the DB tier moved from *deferred* to *rejected*, taking the `rusqlite`
dependency and the read-only-WAL problem with it; the mtime tier was rejected too (18 of 51
transcripts are subagent children, indistinguishable by content, and newest-written exactly
when a fallback runs); and `history.jsonl` was promoted from weak third tier to the only
fallback, on the strength of all 9 of its distinct ids resolving to real `brain/` directories
and none of them being a subagent. R1.3–R1.5 and T1 follow.

## Goal
I'd like to improve the ai-monitor functionality by adding Antigravity CLI information,
similar to how Claude Code does it.

## Non-goals
- Modifying Antigravity CLI binary, internal storage layout, or runtime behavior.
- Breaking or altering Claude Code session monitoring in `ai-monitor`.
- Building UI components outside the existing `ai-monitor` right panel / overlay and
  sidebar dashboard architecture.
- Reconstructing token/cost accounting for Antigravity. The format records none (see
  Research), and inferring it from message sizes would be a fabricated number displayed
  in the same tiles that show measured ones for Claude.

## Confirmed Goal
Extend `crates/ai-monitor` and `crates/app` to detect, resolve, watch, and parse
Antigravity CLI (`agy`) sessions alongside Claude Code (`claude`), streaming real-time
agent telemetry, model status, turns, and tool invocations into the AI monitor panel and
sidebar dashboard.

## Research

### Code the change lands in (re-verified line by line, 2026-08-13)
- `crates/ai-monitor/src/lib.rs:1` — `ai-monitor` provides headless discovery, tailing, and
  parsing of AI agent JSONL logs.
- `crates/ai-monitor/src/lib.rs:87` — `registered_session_in` reads per-pid session
  registration files to pair process IDs with sessions.
- `crates/ai-monitor/src/lib.rs:113` — `looks_like_session_id` performs 36-char lowercase
  UUID validation. Antigravity conversation ids are the same shape, so it is reusable as-is.
- `crates/ai-monitor/src/lib.rs:168` — `resolve_log_in` provides fallback resolution of the
  newest session log in a project directory.
- `crates/ai-monitor/src/telemetry.rs:3` — `SessionTelemetry::apply` folds JSONL line inputs
  into bounded snapshot state with zero presentation logic.
- `crates/ai-monitor/src/telemetry.rs:94` — `classify(name: &str) -> ToolKind` categorizes
  tools into Agent, Skill, File(Read/Write/Grep/List), Task, and Plain. It matches Claude's
  exact tool names; no Antigravity name collides with one.
- `crates/app/src/tabmeta/mod.rs:128` — `AgentKind` already contains `Claude` and
  `Antigravity`.
- `crates/app/src/tabmeta/agent.rs:36` — `classify_exe` and the process table classify `agy`
  and `antigravity` as `AgentKind::Antigravity`, including the auto-updater's `agy.<id>.old`
  rename (`specs/antigravity-agy-detection`).
- `crates/app/src/tabsession.rs:26` — `SessionHint` captures the tab's running agent pid,
  session id, start time, and rival process list.
- `crates/app/src/tabsession.rs:87` — `hint_for(shell_pid)` traverses the process tree under
  the active tab's shell to find the running agent.
- `crates/app/src/overlay.rs:41` — `AiMonitorOverlay` coordinates panel visibility, active
  tab session binding, watcher attachment, and event loop wakes.
- `crates/app/src/dashboard.rs:80` — `MODEL_WINDOWS` defines prefix-based context window
  sizes for models.
- `specs/antigravity-agy-detection/loop.md:18` — the Antigravity CLI executable is `agy`
  (`~/.local/bin/agy`), holding open session locks in `~/.gemini/antigravity-cli/presence/`.

**Neither `tabsession.rs`, `overlay.rs`, nor `dashboard.rs` contains a single reference to
`AgentKind` or `Antigravity`.** The enum exists only in the sidebar's `tabmeta` world. The
monitor half of this feature starts from zero, not from a seam.

### The on-disk format, measured (2026-08-13, this machine)

Corpus: `~/.gemini/antigravity-cli/brain/*/.system_generated/logs/transcript.jsonl` —
**51 files, 1,761 steps, 0 unparseable lines**. The corpus is live: `agy` pid 57079 was
appending during this census, so counts drift by a few between runs.

**Step envelope.** Five keys are present on every one of the 1,761 steps —
`step_index`, `source`, `type`, `status`, `created_at`. The rest are optional:
`content` 958, `tool_calls` 772, `exit_code` 339, `truncated_fields` 289, `thinking` 179,
`error` 6.

- `source` ∈ `MODEL` (1,587), `SYSTEM` (120), `USER_EXPLICIT` (54).
- `status` ∈ `DONE`, `RUNNING`. **`RUNNING` occurs only on `RUN_COMMAND`** (14 of 14) — it
  is the async-command state, not a general "the agent is working" signal.
- `created_at` is RFC3339 with a `Z` suffix (`2026-08-14T03:48:57Z`). Note `content`
  sometimes embeds a *local*-offset timestamp (`Created At: 2026-07-26T09:09:03-07:00`);
  they are different clocks and must not be mixed.

**Step types** (whole corpus):

| type | n | type | n | type | n |
|------|---|------|---|------|---|
| `PLANNER_RESPONSE` | 833 | `CHECKPOINT` | 49 | `ASK_QUESTION` | 12 |
| `RUN_COMMAND` | 353 | `CONVERSATION_HISTORY` | 34 | `FIND` | 12 |
| `VIEW_FILE` | 180 | `SYSTEM_MESSAGE` | 31 | `ERROR_MESSAGE` | 6 |
| `LIST_DIRECTORY` | 75 | `GREP_SEARCH` | 30 | `INVOKE_SUBAGENT` | 6 |
| `GENERIC` | 61 | `CODE_ACTION` | 25 | | |
| `USER_INPUT` | 54 | | | | |

There is **no `WRITE_FILE` type**. Writes surface as `CODE_ACTION` (25 ≈ `write_to_file` 23
+ `replace_file_content` 5). `GENERIC` is the subagent/message lifecycle notice
(`Subagent "skill-portfolio-analyzer" …`, `Message sent to "7640f…"`).

**Tool names** inside `tool_calls` (whole corpus):

| name | n | name | n | name | n |
|------|---|------|---|------|---|
| `run_command` | 360 | `send_message` | 18 | `ask_question` | 12 |
| `view_file` | 184 | `manage_task` | 17 | `find_by_name` | 12 |
| `list_dir` | 76 | `define_subagent` | 13 | `invoke_subagent` | 6 |
| `grep_search` | 32 | `manage_subagents` | 13 | `replace_file_content` | 5 |
| `write_to_file` | 23 | | | `schedule` | 2 |

**Each tool call is recorded twice, and the two records carry different halves of it.**
Once as a `tool_calls` entry on a `PLANNER_RESPONSE` (the dispatch), then again as its own
typed result step carrying `status`, `exit_code`, and a `content` body. Counting both
records roughly doubles every tally — the single largest correctness trap in the format.

The halves do not overlap: **`tool_calls` appears on `PLANNER_RESPONSE` and on no other step
type** (772 of 772 tool-call-bearing steps), so a result step carries **no `args` at all**.
Every path, command line, and `toolSummary` lives on the dispatch; `status` and `exit_code`
live on the result. Reading either record alone loses half the call, so the fold must
**correlate** them (AG2).

Corpus totals: 773 tool-call entries against 754 result steps across the nine step types
AG2 maps. An earlier count of 693 here was wrong — it silently omitted the 61 `GENERIC`
steps, which AG2 does map (754 − 61 = 693). The real counting-event total on this corpus is
**750**: 754 result steps minus 4 whose dispatch cannot be identified.

**Every `args` value is a string holding a JSON document.** Measured over the corpus:
3,470 arg values, **zero** of them a non-string JSON type. Of those, 2,757 are
double-quoted (the arg's real type is string) and 713 are bare JSON scalars or aggregates —
`"true"`, `"5000"`, `"[{\"is_multi_select\":false,…}]"`:

```json
{"name":"view_file","args":{
  "AbsolutePath":"\"/Users/hyunhokim/…/spec.md\"","StartLine":"1","EndLine":"100",
  "toolAction":"\"Viewing spec file\"","toolSummary":"\"View ai-monitor discovery code\""}}
```

So a path is reached by parsing the value's *contents* once — `from_str::<String>` on
`AbsolutePath`, which succeeds because the inner document is a JSON string — never by
reading the value directly, or every recorded path renders with literal quotes. The same
call on `EndLine` (`"100"`) or `Overwrite` (`"true"`) fails, because those inner documents
are a number and a bool; a general unwrap goes through `Value`. This asymmetry is
per-key and stable, not per-call.
Path-bearing keys, all confirmed against samples:

| tool | arg keys (excluding the universal `toolAction` / `toolSummary`) |
|------|-----------------|
| `view_file` | `AbsolutePath`, `StartLine`, `EndLine` |
| `write_to_file` | `TargetFile`, `CodeContent`, `Description`, `Overwrite` |
| `replace_file_content` | `TargetFile`, `TargetContent`, `ReplacementContent`, `StartLine`, `EndLine`, `Instruction`, `AllowMultiple`, `Description` |
| `grep_search` | `SearchPath`, `Query`, `CaseInsensitive`, `MatchPerLine` |
| `list_dir` | `DirectoryPath` |
| `find_by_name` | `SearchDirectory`, `Pattern` |
| `run_command` | `CommandLine`, `Cwd`, `WaitMsBeforeAsync` |
| `invoke_subagent` | `Subagents` (JSON string; entries carry `Model`, `Prompt`) |
| `define_subagent` | `name`, `description`, `system_prompt`, `enable_*_tools` |
| `manage_subagents` | `Action` |
| `manage_task` | `Action`, `TaskId` (`"<conversationId>/task-24"`) |
| `send_message` | `Recipient`, `Message` |
| `ask_question` | `questions` (JSON string) |
| `schedule` | `DurationSeconds`, `Prompt`, `TimerCondition` |

`toolAction` and `toolSummary` are on **every** call — a human-written gerund and a short
label. They are the natural display string for the RECENT CALLS ring; nothing needs to be
synthesized from the tool name.

**`truncated_fields` breaks the inner JSON of large args.** `transcript.jsonl` cuts long
values and appends a `<truncated N bytes>` marker, which leaves the inner document
unparseable: 42 of 360 `CommandLine`, 17 of 23 `CodeContent`, and 7 of 13 `system_prompt`
values fail `from_str::<String>`. This does not change the choice of file — **none of the
keys this fold unwraps was ever truncated** — but a fold that read `CommandLine` would emit
~42 spurious parse errors, and `transcript_full.jsonl` is the file to reach for if command
text is ever needed.

Two smaller confinements, both relevant later: `created_at` is **always exactly 20 bytes**
(1,761/1,761, `Z`-suffixed, never fractional seconds), so the existing `parse_ts` handles it
unchanged; and `exit_code` occurs **only on `RUN_COMMAND`** (339 = 312 zero, 26 one, 1
other), which bounds where G4's error mapping can draw from.

**Turn shape.** `USER_INPUT` = 54; `PLANNER_RESPONSE` **without** `tool_calls` = 61;
`PLANNER_RESPONSE` **with** `tool_calls` = 772. A 54:61 ratio (1.13) is what a turn pair
looks like; 54:833 is not. The tool-bearing planner steps are dispatch, not replies.

`USER_INPUT.content` is a tagged envelope, and the real prompt is extractable:

```
<USER_REQUEST>
How can I see about ai-agent monitor
</USER_REQUEST>
<ADDITIONAL_METADATA>
The current local time is: 2026-07-25T22:02:35-07:00.
</ADDITIONAL_METADATA>
<USER_SETTINGS_CHANGE>
The user changed setting `Model Selection` from None to Gemini 3.6 Flash (High). No need to…
</USER_SETTINGS_CHANGE>
```

**The model is prose, not a field.** `<USER_SETTINGS_CHANGE>` appears in 34 steps, 33 of
them `USER_INPUT`/`USER_EXPLICIT` (the 34th is a `grep_search` result echoing one). Parsed
values across the corpus: `Gemini 3.6 Flash (High)` ×29, `Gemini 3.7 Flash (High)` ×5 — the
reasoning tier is always present in the parens. The pattern is
``changed setting `Model Selection` from <old> to <new>.`` and the terminating period is
part of the sentence, not the value. There is no structured `model` key anywhere.

**The obvious extraction of that value is wrong.** The model name *contains* periods, so a
non-greedy match to the first `.` — ``to (.*?)\.`` — returns `Gemini 3`. The terminator is
the first period **followed by whitespace or end-of-string**; that form returns
`Gemini 3.6 Flash (High)`. Verified against the live sentence both ways.

**No token accounting exists.** Grepping the whole corpus for `usage`, `input_tokens`,
`output_tokens`, `prompt_tokens`, `total_tokens` returns **zero** hits. Nothing in the
format can populate `total_input`, `total_output`, `total_cache_*`, `context_used`, or
`context_high_water`.

**`transcript.jsonl` is append-only and safe to tail by offset.** On the largest sample it
is byte-exact equal to the concatenation of its chunk mirrors:
`chunks/transcript/{00000000,00000001,00000002}.jsonl` = 102400 + 102400 + 14396 = 219196 =
the file's size. `transcript_full.jsonl` is the untruncated twin (same line count); the
`truncated_fields` key on 289 steps records what `transcript.jsonl` shortened. Telemetry
needs counts and paths, not bodies, so the truncated file is the correct target and is a
third the size.

**Presence locks resolve to a pid.** Verified live:

```
$ lsof ~/.gemini/antigravity-cli/presence/4de5fc70-2d8a-44c2-8e25-a6901c93fb86.lock
COMMAND   PID      USER   FD   TYPE  NAME
agy     57079 hyunhokim   86u   REG  …/presence/4de5fc70-….lock
```

The lock basename is the conversation id, and `brain/<same-uuid>/` is its transcript
directory. Stale locks persist after exit — the directory held 30 lock files for far fewer
live sessions — so **presence of a lock file proves nothing; only an open FD does**.

**`conversation_summaries.db` cannot reach a transcript.** Its schema looks ideal —
`conversation_id`, `workspace_uris`, `last_modified_time`, `step_count`, `status`,
`parent_conversation_id`, `nesting_depth` — and its ids are a **disjoint namespace from
`brain/`**. Measured: 102 DB rows, 54 brain directories, **intersection 0**. Every
`nesting_depth` is 0 and every `parent_conversation_id` is empty across all 102 rows, and
the known subagent child `ed14342c-…` is absent entirely. Whatever this table tracks, it is
not the transcripts on disk, so no query against it can name a log path. (Checked both
`mode=ro&immutable=1` and a WAL-replayed copy — 102 rows either way, so the WAL question is
moot here.) This retires the SQLite dependency question with it.

**`history.jsonl` is sparse but sound.** 43 lines. Only `display`, `timestamp`, and
`workspace` are universal: `type` is on 31 records, `conversationId` on 12, and 9 records
carry both — so neither key may be assumed present. Those 12 records name 9 distinct
conversations, **all 9 of which resolve to a real `brain/<id>/` directory**, and every record
carries the `workspace` absolute path. It is the only source on disk that pairs a
conversation with a working directory. (The file is appended live; it grew by one line
during this session's measurements.)

**A `brain/<id>/` directory does not imply a transcript.** 54 directories, 51
`transcript.jsonl` files: three conversations have a brain directory containing only
`scratch/` and `.user_uploaded/`. At least one of them (`9f45b778-…`) is named by a
`history.jsonl` record, so the fallback tier can select a conversation that has a directory
and no log. R1.6's unattached state is therefore reachable through the *fallback*, not only
at cold start — see AG6's note on which absence stops the ladder.

**Subagents get their own conversation, and are indistinguishable from top-level ones.** The
`INVOKE_SUBAGENT` step's `content` names the child directly:

```
Created the following subagents:
{ "conversationId": "ed14342c-…", "logAbsoluteUri": "file:///…/brain/ed14342c-…" }
```

Collecting every such id across the corpus: **18 of the 51 transcripts (35%) are subagent
children**, sitting in `brain/` beside their parents with no marker distinguishing them.
Their step 0 is a `USER_INPUT` / `USER_EXPLICIT` with a `<USER_REQUEST>` envelope, exactly
like a human's — the prompt text is simply the one the parent wrote. `<ADDITIONAL_METADATA>`
is present in 18/18 children **and** 33/33 non-children, so it discriminates nothing. No
content-based test separates them; only a parent's `INVOKE_SUBAGENT` record does.

The two facts compose into the central constraint on fallback resolution: a subagent is
invisible in the DB, indistinguishable in `brain/`, and — because it is running flat out
while its parent waits — it is precisely the **newest-written transcript** at the moment any
mtime-based fallback would run. `history.jsonl` is the one source immune to this: it records
user interactions, and **0 of its 9 conversations are subagents**.

### What the existing `ai-monitor` contract actually offers

`TelemetrySnapshot`'s real fields: `session_id`, `model`, `total_input`, `total_output`,
`total_cache_read`, `total_cache_creation`, `context_used`, `context_high_water`,
`first_event_ms`, `last_event_ms`, `tools`, `skills`, `files`, `subagents`, `tasks`,
`tool_calls: Vec<(String,u64)>`, `tool_calls_other`, `total_tool_calls`,
`failed_tool_calls`, and six `*_errors`/diagnostic counters. There is **no** `turns_user`,
`turns_assistant`, `state`, `tool_calls_file`, `tool_calls_bash`, or `tool_calls_agent` —
no turn counter exists today for Claude either.

`LogWatcher` is Claude-shaped in two load-bearing ways. `Watch { dir, log, want: Wanted,
owned, rivals: RivalHandle, attached }` re-resolves through `resolve_owned_session(&dir,
&want, …)`, which encodes Claude's rival-assignment rule — meaningless when a presence lock
names the conversation exactly. And `touches_project_log` matches an event by **the parent
directory's name**, deliberately, because Claude project directories are uniquely-named
encoded cwds. Every Antigravity conversation's transcript lives in a directory named
`logs`, so that predicate is true for *every* conversation on the machine.

The panel's card labels are `UPTIME`, `IDLE`, `ERRORS`, `CACHE`, `CONTEXT`, `TASKS`,
`ACTIVE AGENTS`, `SKILLS`, `FILE ACTIVITY`, `RECENT CALLS`, `TOOL USAGE`, plus a
`NO SESSION` placeholder. **There is no agent-name title and no turns tile.**
`dashboard::Env` carries `tab_has_claude` (16 call sites), which selects between the two
detached wordings. `MODEL_WINDOWS` matches by `model.starts_with(prefix)` over *model ids*
(`claude-opus-`), and `WINDOW_TIERS` is `[200_000, 1_000_000]` — there is no 2M tier.

### Tests that will have to change
- `crates/ai-monitor/tests/telemetry.rs` + `tests/fixtures/` — the established fixture seam;
  T2 adds Antigravity fixtures beside the Claude ones.
- `crates/ai-monitor/src/lib.rs` — 48 in-file tests, none of which should move.
- `crates/app/src/dashboard.rs` — 63 tests and **9 pinned `insta` snapshots** under
  `crates/app/src/snapshots/`. Any new panel row (AG10) rewrites them; that is a reviewed
  diff, not an incidental one.

## Decisions

### AG0: Decision ids are `AG*`, not `D*`
- **Status**: resolved
- **Rationale**: `telemetry.rs` and `dashboard.rs` already cite bare `(D1)`, `(D3)`, `(D6)`,
  `(D7)`, `(D12)`, `(SD4)`, `(SD5)` meaning `specs/ai-monitor/spec.md`. A second, different
  `D1`–`D6` for the same files would make every future comment ambiguous about which spec it
  cites. The first draft's `D1`–`D6` map to `AG1`, `AG2`, `AG4`, `AG5`, `AG6`, `AG10`.

### AG1: Session attachment is by open FD on a presence lock
- **Status**: resolved
- **Rationale**: the lock basename is the conversation id and `brain/<id>/` is its
  transcript directory, so one FD probe answers the question exactly — no timing heuristic,
  no rival assignment, none of the machinery Claude needs because its logs carry no owner.
  The probe is **open FD**, never file existence: 30 lock files were on disk for a handful of
  live sessions.
- **Alternatives Rejected**: `history.jsonl` alone (misses sessions that ran no slash
  command, and 31 of 43 records carry no conversation id); directory mtimes (background
  writes from subagent conversations make them lie).

### AG2: Event and tool schema mapping
- **Status**: resolved
- **Rationale**: the mapping below is transcribed from the measured corpus, and it counts
  **the result step**, never the `PLANNER_RESPONSE` dispatch that precedes it (AG3). The
  result step is also the only record carrying `status`, `exit_code`, and Created/Completed
  times, so it is where `ok` and `duration_ms` come from for free.

  **Counting the result step is not the same as reading it.** `tool_calls` lives only on the
  dispatch, so the result step has no `args`: the fold **correlates** the pair — dispatch
  supplies name, args, path and `toolSummary`; result supplies `status` and is the single
  counting event. The join is by position, not by id, because Antigravity calls carry none:
  a result claims the **oldest pending dispatch whose expected result-step type matches**.
  Oldest-first rather than nearest-preceding is what an async `run_command` requires, and
  measured, "the immediately preceding dispatch" would be right only 736 of 772 times. A
  dispatch whose tool name is unknown to the table is never matched to a result at all —
  mis-attributing a real call to the wrong name is worse than not counting it, and the miss
  shows up in `orphan_tool_results` where a mis-attribution would be invisible.

  | result step | tool name(s) | `ToolKind` | path from |
  |---|---|---|---|
  | `VIEW_FILE` | `view_file` | `File(Read)` | `AbsolutePath` |
  | `CODE_ACTION` | `write_to_file`, `replace_file_content` | `File(Write)` | `TargetFile` |
  | `GREP_SEARCH` | `grep_search` | `File(Grep)` | `SearchPath` |
  | `LIST_DIRECTORY` | `list_dir` | `File(List)` | `DirectoryPath` |
  | `FIND` | `find_by_name` | `File(List)` | `SearchDirectory` |
  | `RUN_COMMAND` | `run_command` | `Plain` | — (`CommandLine`) |
  | `INVOKE_SUBAGENT` | `invoke_subagent` | `Agent` | — (AG5) |
  | `GENERIC` | `define_subagent`, `manage_subagents`, `send_message`, `schedule` | `Plain` | — |
  | `ASK_QUESTION` | `ask_question` | `Plain` | — |
  | `GENERIC` † | `manage_task` | `Task` | — (`TaskId`) |

  † `manage_task`'s result-step type was **inferred, not measured**. The corpus census
  cannot separate it: `GENERIC` (61 steps) is the shared result type for five different
  tools, and no per-call correlation was run to confirm `manage_task` is among them. It is
  mapped to `GENERIC` so its call is counted; the cost if that is wrong is that its 17
  dispatches contend with the other `GENERIC` reporters for the same result steps. Verifying
  it needs a correlation pass over the live corpus that this feature did not do.

  Every `args` value is unwrapped once from its JSON-string encoding before use. Tools keep
  **their own names** in the `tool_calls` tally: that map is keyed on the name the log
  supplies (`telemetry.rs`), so rewriting `run_command` to `Bash` would print a Claude
  vocabulary over a session that never used one. The display string is `toolSummary`,
  present on every call.
- **Alternatives Rejected**: counting both the dispatch and the result (doubles every
  tally); generic string logging without path extraction (loses FILE ACTIVITY entirely);
  translating names into Claude's vocabulary (see above).

### AG3: A turn is `USER_INPUT` / a tool-less `PLANNER_RESPONSE`
- **Status**: resolved
- **Rationale**: measured 54 `USER_INPUT` against 61 tool-less `PLANNER_RESPONSE` (1.13) and
  772 tool-bearing ones. Counting every planner step would report a 15:1 assistant:user
  ratio — a number that is not wrong by a little, it is a different quantity wearing the
  word "turn".
- **Alternatives Rejected**: `source == MODEL` (1,587 of 1,761 steps); `step_index` deltas
  (counts dispatches).

### AG4: Model name is scraped; no context window is assigned
- **Status**: resolved (approved 2026-08-13)
- **Rationale**: model extraction — parse
  ``changed setting `Model Selection` from <old> to <new>.`` out of `USER_INPUT.content`,
  take `<new>`, strip the terminating period, keep the effort tier (`Gemini 3.6 Flash
  (High)`) because it is what the user selected and what the CLI reports about itself. It is
  prose-scraping in a crate that otherwise reads typed fields, and it degrades to `None`
  cleanly, which is the existing contract for an unknown model.
  The context-window work is **dropped**. The format records no token counts at all, so
  `context_used` stays 0 forever: a `Gemini` row in `MODEL_WINDOWS` would buy a denominator
  under a permanently-zero numerator, and `promoted_window`'s evidence-based correction could
  never fire. `MODEL_WINDOWS` and `WINDOW_TIERS` are therefore **untouched** by this feature —
  no Gemini rows, no 2M tier, no display-name prefixes breaking a table keyed on model ids.
  Instead CONTEXT and CACHE render as explicitly unavailable when the attached agent reports
  no usage: `0%` reads as "nothing used", which is a false statement about a session that has
  run 800 steps, and these cards otherwise only ever show measured numbers. Revisit if
  Antigravity starts logging usage.
- **Alternatives Rejected**: hardcoding 200k (wrong for every Gemini); inferring tokens from
  content byte lengths (a fabricated number in a tile that shows measured ones elsewhere);
  rendering 0% (asserts a measurement that was never taken).

### AG5: Subagents are named from `define_subagent`, joined by conversation id
- **Status**: resolved
- **Rationale**: `invoke_subagent`'s args carry only `Subagents[].Model` and `.Prompt` —
  there is no role or type on the invocation. The human-readable identity comes from the
  `define_subagent` call that precedes it (`name`, `description`), and the
  `INVOKE_SUBAGENT` result step's `content` names the child's `conversationId` /
  `logAbsoluteUri`. So: name from `define_subagent`, status from the invocation's step
  `status`, and the child conversation id retained as the join key. The primary watcher
  keeps tailing the parent transcript only.
- **Alternatives Rejected**: interleaving child transcripts into the main stream (corrupts
  AG3's turn counts, since each child has its own `USER_INPUT`); reading `Role`/`TypeName`
  (they do not exist).

### AG6: Two-tier fallback — open FD, then `history.jsonl`, then nothing
- **Status**: resolved (approved 2026-08-13 as "defer the DB tier"; the measurements below
  turned that deferral into a rejection and replaced the third tier)
- **Rationale**: the ladder is **open FD on a presence lock (AG1) → `history.jsonl` scoped to
  the tab's cwd → unattached**. Both discarded tiers were discarded on evidence, not taste:
  - `conversation_summaries.db` is **rejected outright**, not deferred. Its ids and
    `brain/`'s directory names are disjoint sets — 102 rows, 54 directories, intersection 0 —
    so it cannot name a transcript path at all. This also retires the `rusqlite` dependency
    and the read-only-WAL problem: neither is needed for a table that cannot answer the
    question. That is a strictly better outcome than the approved deferral.
  - **Newest-`brain/*`-by-mtime is rejected.** 18 of 51 transcripts are subagent children,
    they carry no marker distinguishing them from top-level conversations, and a running
    subagent is by construction the most-recently-written transcript on disk while its parent
    sits idle waiting for it. An mtime fallback would therefore mis-attach *specifically* in
    the busiest sessions, and silently: the panel would show real, live, wrong telemetry.
  - `history.jsonl` survives because it records **user interactions**. It is sparse (9
    distinct conversations) but every id resolves to a real `brain/` directory, every record
    carries an absolute `workspace` path, and **none of the 9 is a subagent**. Sparse and
    correct beats dense and wrong for a tier that only runs when the exact answer failed.

  **Amended 2026-08-15 after a live failure.** The ladder above is not two tiers but three
  states, because "no lock to read" and "no agent to ask" have opposite consequences and the
  original `Option<&str>` could not tell them apart. `agy` publishes its presence lock and its
  transcript **~7 seconds after the process starts** (measured: process 00:07:22, both files
  00:07:29). A panel re-pointing inside that window probed nothing, fell through to
  `history.jsonl`, and adopted the conversation whose newest record was the user's own
  `/exit` — proof that session had ended — rendering its 11-minute-old telemetry as the tab's
  live session. `history.jsonl` records a conversation only once it has run a slash command,
  so **a conversation that started seconds ago is structurally absent from it**: with a live
  agent the history tier cannot be right, only wrong. `Probed::LiveButUnnamed` now resolves to
  `None`, and the tier survives solely for `Probed::NoAgent`. Pinned by
  `a_live_agent_never_adopts_an_older_conversation_from_history`, which fails if the
  fall-through is restored.

  When both tiers miss, the panel stays unattached. That is the rule `ai-monitor` already
  applies to Claude — *"a tab with no identifiable session must show nothing rather than
  adopt whatever else happens to be writing there"* (`lib.rs`) — and the subagent measurement
  is what makes it the same rule here rather than a stricter one.

  **A missing transcript stops the ladder; a missing directory skips a record.** These are
  different absences and the tier treats them differently. A `history.jsonl` record whose
  `brain/<id>/` is gone is stale bookkeeping — skip it, try the next record. A record whose
  directory exists but holds no `transcript.jsonl` is almost always the conversation the user
  *just* started, so the honest answer is "not written yet": return unattached rather than
  falling through to an older conversation in the same workspace. Falling through would show
  a finished session's real telemetry while the user sits in a new one — and because the
  watcher does not move once attached, it could stay wrong for the life of the panel. This is
  live, not hypothetical: three brain directories carry no transcript today, and two of the
  author's workspaces resolve to unattached because of it.
- **Alternatives Rejected**: SQLite (cannot reach a transcript); mtime (mis-attaches to
  subagents under load); scanning every transcript for `INVOKE_SUBAGENT` ids to subtract the
  children (unbounded reads on every resolution, to rescue a tier `history.jsonl` already
  covers); copying the DB before reading (a write, elsewhere, plus a stale read).

### AG7: The watcher is made agent-polymorphic before either agent uses it
- **Status**: resolved
- **Rationale**: `Watch`'s re-resolution (`want: Wanted` + `rivals: RivalHandle` →
  `resolve_owned_session`) is Claude's answer to "logs carry no owner", and Antigravity's
  lock answers it exactly — so the Antigravity watch wants *no* re-resolution, not a
  differently-parameterized one. Separately, `touches_project_log` matches by parent
  directory name, and every Antigravity transcript sits in a directory named `logs`, so
  reusing it unchanged wakes the panel on every conversation on the machine. Both are in the
  watcher, both are invisible from the app, and neither was in the first draft's task list.
- **Alternatives Rejected**: a second parallel watcher type (duplicates the tail/offset/
  partial-line handling that took two specs to get right); passing Antigravity through
  `Wanted` (encodes a rival contest that cannot occur).

### AG8: The turn counters are additions to `TelemetrySnapshot`
- **Status**: resolved (approved 2026-08-13)
- **Rationale**: AG3 defines turns, and the snapshot has nowhere to put them. `turns_user`
  and `turns_assistant` are new `u64` fields on a struct the dashboard reads. They are
  additive, they default to 0, and the Claude fold can populate them later or never — but
  they widen the versioned contract, which `docs/architecture.md` says to extend
  deliberately.
  Exactly those two counters are added, and nothing else. **No `state` field**: the format's
  only status signal is `RUNNING` on `RUN_COMMAND` — an async-command flag, not a session
  state — and the `activity.rs` path already answers "is it working". **No
  `tool_calls_file` / `_bash` / `_agent`**: `tool_calls: Vec<(String,u64)>` plus `classify`
  already carry that, and per-kind scalars would be a second, drift-prone spelling of the
  same fold. The Claude fold leaves both counters at 0 until someone specifies what a turn
  means there; a 0 in a field Claude never populates is the same "not measured" the panel
  already handles.
- **Alternatives Rejected**: keeping turns in the app's dashboard model (the fold is
  per-line and stateful — reconstructing it in a pure composer means re-reading the log);
  the four rejected fields above.

### AG9: Format is chosen at attach time, not sniffed per line
- **Status**: resolved
- **Rationale**: the attaching code already knows the `AgentKind` — it walked the process
  tree to find `agy`. Sniffing inside `apply` would put a guess where a fact is available,
  and both formats have a `type` key, so a misroute is silent and looks like a parse bug
  three layers away. `SessionTelemetry` gets an explicit format at construction; `apply`
  dispatches on it.
- **Alternatives Rejected**: per-line sniffing; a separate telemetry type per agent
  (duplicates every cap, ring, and saturating counter in the module).

### AG10: Panel presentation, split into a mechanical half and a snapshot-churning half
- **Status**: resolved (approved 2026-08-13)
- **Rationale**: the first draft framed this as adapting a "Claude Code" title to
  "Antigravity CLI". No such element exists — the panel has cards and a `NO SESSION`
  placeholder, and nothing anywhere names the agent. So this is new UI: an agent-name row,
  and a turns tile that also does not exist. Both rewrite the 9 pinned `insta` snapshots.
  `Env.tab_has_claude` (16 call sites) additionally hardcodes Claude into the detached
  wording an `agy` tab would see.
  The work therefore splits along whether it churns pinned snapshots:
  - **T5, mechanical**: `Env.tab_has_claude` → `Env.tab_agent: Option<AgentKind>`, with the
    detached wording routed through it. This is a bug fix independent of the rest — an `agy`
    tab is told about Claude *today* — and it must not change a single rendered byte for a
    Claude tab, which the existing snapshots then prove.
  - **T6, new UI**: the agent name, the turns tile, and AG4's unavailable CONTEXT/CACHE
    treatment. All three touch composition, so they land together as one reviewed snapshot
    diff rather than three separate re-pins of the same nine files.

  **As built, T6 moved no existing snapshot at all.** Three details made that possible, and
  each is a better answer than the one this decision originally assumed:
  - **The agent name is the session row's `provider` tag, not a new row.** The panel has no
    title element — but it has carried `provider="[Claude]"` beside the session id since it
    was built, which is the same fact R5.1 wanted displayed. A title row would have been new
    chrome saying what an existing row already says. `[Antigravity]` keeps the tag's short
    established shape rather than the spec's literal "Antigravity CLI".
  - **TURNS takes IN·OUT's slot** rather than becoming a seventh tile. The grid is 3x2 and
    stays 3x2: an agent that reports no tokens cannot fill the token trio, so the first of
    those three slots holds the one headline quantity it does measure, in the same
    "two numbers, one dot" idiom.
  - **The new presentation got its OWN pinned snapshot** instead of mutating Claude's. The
    nine existing panels stay byte-identical and the Antigravity panel gets a baseline of its
    own, so neither is ever reviewed as a diff against the other.
- **Alternatives Rejected**: doing both halves in one task (a real regression in the
  mechanical half would arrive inside an expected snapshot diff, which is exactly where it
  would not be noticed); shipping the data path with no presentation at all (an attached
  Antigravity session would be indistinguishable from a Claude one on screen).

### AG11: The FD probe lives in `app`, not `ai-monitor`
- **Status**: resolved
- **Rationale**: enumerating another process's open FDs needs `proc_pidinfo(PROC_PIDLISTFDS)`
  + `proc_pidfdinfo`, or an `lsof` subprocess. `ai-monitor` has neither `libc` nor any
  subprocess use, and acquiring both would trade away exactly the headless purity that makes
  it testable. `app` already does both — `tabsession.rs:263` forks `/bin/ps`, and three call
  sites use `proc_pidinfo`. So `app` resolves pid → conversation id and hands `ai-monitor`
  an id; `ai-monitor` owns paths, tailing, and parsing, as it does for Claude.
- **Alternatives Rejected**: `libc` in `ai-monitor` (breaks the crate's one job); duplicating
  the probe in both (the `proc_pidinfo` copies already in this repo are a standing complaint
  in `tabsession.rs:221`).

## Constraints
- **Read-Only**: zero writes, locks, or modifications under `~/.gemini/antigravity-cli/`.
  Now unconditionally satisfiable: AG6 reads only `presence/*.lock` (an FD probe, no open by
  us), `history.jsonl`, and `transcript.jsonl`. The one artifact whose read was not free —
  the WAL-backed SQLite DB — is out of scope entirely.
- **Dependency Direction**: `crates/ai-monitor` stays pure, headless, and GPU-free. AG11
  keeps process introspection in `app`, and AG6 retires the SQLite question, so the crate's
  manifest stays `serde_json` + `notify`.
- **Total Fault Tolerance**: all JSON parsing and file reads panic-free with typed
  fallbacks. The corpus parsed 1,761/1,761 lines cleanly, which is evidence about the
  format's regularity and not a reason to relax this.
- **Claude Code Parity**: no existing Claude session detection, isolation, or telemetry
  **behavior** changes, and its tests keep passing unmodified. This is deliberately weaker
  than the first draft's "all existing tests untouched": AG8 and AG10 touch shared surfaces,
  and the honest rule is that Claude's *observable behavior* is frozen while pinned
  snapshots may take a reviewed diff.

## Known Gaps

All four sign-off items are closed (AG4, AG6, AG8, AG10, approved 2026-08-13). What remains
are standing limitations, not open decisions.

- **G1 (AG4)** — Antigravity reports no token usage, so CONTEXT and CACHE carry no
  information for these sessions and render as unavailable. This is a limit of the format,
  not of the implementation; it lifts only if Antigravity starts logging usage.
- **G2 (AG6)** — when a tab's `agy` holds no readable presence FD **and** its workspace has
  never appeared in `history.jsonl`, the panel stays unattached. Sparse-and-correct is the
  chosen failure mode; the observed `history.jsonl` covers only 9 conversations across 2
  workspaces, so this state is reachable in practice on a fresh workspace.
- **G3** — `history.jsonl` is assumed never to name a subagent conversation. Measured 0 of 9
  here, and it is a user-interaction log by construction, but it is an inference from one
  machine rather than a documented guarantee.
- **G4** — `exit_code` (339 records), the `error` key (6), and `ERROR_MESSAGE` steps are not
  yet mapped, so the ERRORS tile and `failed_tool_calls` would read 0 for a session that did
  fail. Cheap to add once AG2 lands; deliberately not in R2 to keep the first pass narrow.
- **G5** — `manage_task`'s `TaskId` (`"<conversationId>/task-24"`) suggests a task model the
  transcript never fully records; the TASKS card stays empty for Antigravity.
- **G6** — the corpus is one machine, one user, ~3 weeks. Field names are stable across all
  51 transcripts but Antigravity is a moving target; a version probe is not specified.
- **G7** — subagent *progress* lives in the child conversation, which AG5 deliberately does
  not tail. The Subagents card shows invocation and status, not the child's work.
- **G9 (R0.1)** — **R0.1's scenario was never executed.** Two live tabs, one running
  `claude` and one running `agy`, switching focus and re-pointing the panel, has only ever
  been reasoned about. Its *pieces* are covered — `antigravity_pid` pins the routing over all
  five `AgentKind`s, `presence_fd` pins the FD probe against a descriptor the test process
  really holds, and `tests/antigravity.rs` pins the resolution ladder — but **nothing
  exercises their composition through `overlay::open`**, and no test touches `agent_for` or
  the non-`_in` `conversation_of_pid`. A unit test cannot reach it: it needs two real agent
  processes and a winit event loop. Recorded here rather than only in `loop.md` because a
  reader who greps `R0.1` finds a green test whose subject is a five-line enum match, and
  would otherwise have no way to learn that the requirement's own scenario is unverified.
  Closing this needs a manual `/run-term` session.
- **G8 (AG7)** — *partially closed 2026-08-15.* The **cold-start** half is fixed: the
  `refresh_rivals` worker now re-probes an `agy` tab off-thread and reports the moment a
  conversation becomes nameable, flipping the unarmed branch of `session_rotated` into a full
  re-point — so a tab that resolved nothing during `agy`'s ~7s startup heals itself instead of
  sitting on "has not written a log yet". What remains open is rotation **while attached**:
  `WatchScope::SingleLog` still cannot detect conversation rotation. Claude's
  rotation signal is a sibling `.jsonl` appearing in the same project directory; Antigravity's
  new conversation lands in a *different* `brain/<id>/` the watcher is not watching, and
  watching the brain root instead would mean waking on every conversation on the machine —
  the exact traffic AG7 exists to exclude. Rotation is therefore the app's job, via
  re-resolution on tab switch (T4). A tab whose conversation rotates while the panel sits
  open on it shows the old conversation until the next re-resolution.

## Requirements

### R0: Polymorphic AI session monitoring
The monitor panel follows the agent running in the active tab, for both Claude Code and
Antigravity CLI.

#### R0.1: Dynamic agent-type selection on tab focus
- **Given**: Tab 1 running Claude Code, Tab 2 running Antigravity CLI (`agy`)
- **When**: the user focuses Tab 2 with the panel open (Cmd+Shift+M)
- **Then**: the panel detaches from the Claude watcher, attaches to Tab 2's Antigravity
  transcript, and the accumulated Claude telemetry is not shown against the Antigravity
  session

### R1: Antigravity session discovery and resolution

#### R1.1: Resolve session from an open presence lock
- **Given**: an `agy` process, pid `P`, holding an **open file descriptor** on
  `~/.gemini/antigravity-cli/presence/<UUID>.lock`
- **When**: resolution runs for pid `P`
- **Then**: it yields conversation id `<UUID>` and the path
  `~/.gemini/antigravity-cli/brain/<UUID>/.system_generated/logs/transcript.jsonl`, marked
  owned

#### R1.2: A stale lock file is not a session
- **Given**: `presence/<UUID>.lock` exists on disk with no process holding it open
- **When**: resolution runs
- **Then**: that conversation is not resolved from the lock (it may still be reached by
  fallback), because lock files outlive their sessions

#### R1.3: Fallback resolves by workspace, from `history.jsonl` only (AG6)
- **Given**: an `agy` process in workspace `W` whose presence FD cannot be read, where
  `history.jsonl` names conversation `C` against `workspace: W`, and `brain/C/` exists
- **When**: fallback resolution runs
- **Then**: it returns `C`'s transcript path, marked **not** owned

#### R1.4: A running subagent never wins the fallback (AG6)
- **Given**: the same tab, where a subagent conversation `S` under `W` is the
  most-recently-written transcript in `brain/` and is absent from `history.jsonl`
- **When**: fallback resolution runs
- **Then**: `S` is never returned — resolution yields `C` if `history.jsonl` names one and
  unattached otherwise. Recency across `brain/` is not a tiebreak at any tier, because a
  live subagent is the newest writer precisely while its parent waits

#### R1.5: No identifiable session shows nothing
- **Given**: an `agy` process whose presence FD is unreadable and whose workspace has never
  appeared in `history.jsonl`
- **When**: resolution runs
- **Then**: it returns unattached without error, and the panel shows the waiting placeholder
  rather than adopting any transcript

#### R1.6: Cold start without a transcript
- **Given**: a freshly launched `agy` that has not yet written a transcript
- **When**: resolution runs
- **Then**: it returns unattached without error and the panel shows the waiting placeholder

### R2: Transcript parsing and telemetry folding
Acceptance criteria below name only fields that exist today, except where AG8 is cited.

#### R2.1: Turn folding (AG3, AG8)
- **Given**: a fresh `SessionTelemetry` in Antigravity format
- **When**: a `USER_INPUT` step is applied, then a `PLANNER_RESPONSE` **with** a `tool_calls`
  array, then a `PLANNER_RESPONSE` **without** one
- **Then**: `turns_user` == 1 and `turns_assistant` == 1 — the tool-bearing planner step is a
  dispatch and increments no turn counter

#### R2.2: Each tool call is counted exactly once (AG2)
- **Given**: a fresh `SessionTelemetry`
- **When**: a `PLANNER_RESPONSE` carrying `tool_calls: [{name: "view_file", …}]` is applied,
  followed by its `VIEW_FILE` result step
- **Then**: `total_tool_calls` == 1 and `tool_calls` contains `("view_file", 1)`

#### R2.3: File operations and path extraction (AG2)
- **Given**: a fresh `SessionTelemetry`
- **When**: dispatch/result **pairs** are applied for `view_file`
  (`AbsolutePath: "\"/a/b.rs\""` → `VIEW_FILE`), `write_to_file`
  (`TargetFile: "\"/a/c.rs\""` → `CODE_ACTION`), `grep_search` (`SearchPath: "\"/a\""` →
  `GREP_SEARCH`), and `list_dir` (`DirectoryPath: "\"/a\""` → `LIST_DIRECTORY`) — args on the
  `PLANNER_RESPONSE`, in their real JSON-encoded form, since result steps carry none
- **Then**: `files` holds 4 entries classified `Read`, `Write`, `Grep`, `List`, with paths
  `/a/b.rs`, `/a/c.rs`, `/a`, `/a` — **unquoted**, no embedded `"` characters

#### R2.4: Command execution (AG2)
- **Given**: a fresh `SessionTelemetry`
- **When**: a `PLANNER_RESPONSE` dispatching `run_command` with
  `CommandLine: "\"cargo test\""` and `toolSummary: "\"Run the test suite\""` is applied,
  followed by its `RUN_COMMAND` result step
- **Then**: `tool_calls` contains `("run_command", 1)` — the log's own name, not `Bash` — and
  the entry in `tools` displays the `toolSummary` text

#### R2.5: Subagent invocation (AG5)
- **Given**: a fresh `SessionTelemetry`
- **When**: a `define_subagent` call naming `"code-reviewer"` is applied, then an
  `INVOKE_SUBAGENT` result step whose `content` names `conversationId: "ed14342c-…"`
- **Then**: `subagents` holds one entry labelled `code-reviewer`, carrying the child
  conversation id and the step's `status`

#### R2.6: Turn counts are not multiplied by subagents (AG5)
- **Given**: a session that invoked a subagent
- **When**: the parent transcript is folded
- **Then**: `turns_user` counts only the parent's `USER_INPUT` steps; no child transcript is
  read

#### R2.7: Malformed input (Constraints)
- **Given**: an active `SessionTelemetry`
- **When**: a truncated, non-JSON, or structurally unexpected line is applied — including a
  `tool_calls` entry whose `args` value does not parse as its expected inner type (an
  `AbsolutePath` holding a bare number, a value that is not a string at all)
- **Then**: `apply` returns without panic, every accumulated counter is preserved, and
  `parse_errors` increments

#### R2.8: Format is not sniffed (AG9)
- **Given**: a `SessionTelemetry` constructed in Antigravity format
- **When**: a well-formed **Claude** JSONL line is applied
- **Then**: it is counted as a parse error, not folded — a line from the wrong format is a
  bug upstream, not input to interpret

### R3: Model and timestamps

#### R3.1: Session timing from RFC3339 `created_at`
- **Given**: a fresh `SessionTelemetry`
- **When**: steps with `created_at` `T0` then `T1` are applied
- **Then**: `first_event_ms` is `T0` in epoch millis and `last_event_ms` is `T1`, parsed from
  the `Z`-suffixed step field — never from the local-offset timestamps inside `content`

#### R3.2: Model name scraped from the settings-change envelope (AG4)
- **Given**: a fresh `SessionTelemetry`
- **When**: a `USER_INPUT` step is applied whose `content` contains
  ``<USER_SETTINGS_CHANGE>\nThe user changed setting `Model Selection` from None to Gemini 3.6 Flash (High). No need to comment…``
- **Then**: `model` == `"Gemini 3.6 Flash (High)"` — the effort tier retained, the sentence's
  terminating period excluded

#### R3.3: A later switch wins
- **Given**: telemetry whose `model` is `"Gemini 3.6 Flash (High)"`
- **When**: a later `USER_INPUT` records a change to `Gemini 3.7 Flash (High)`
- **Then**: `model` == `"Gemini 3.7 Flash (High)"`, matching the existing field's
  last-wins contract

#### R3.4: No model recorded
- **Given**: a transcript with no `<USER_SETTINGS_CHANGE>` (21 of 54 `USER_INPUT` steps in
  the corpus carry none)
- **When**: the session is folded
- **Then**: `model` stays `None` and the panel renders its existing unknown-model form —
  no guessed default

### R4: Subagent presentation

#### R4.1: Subagent card population
- **Given**: an Antigravity session that defined and invoked a subagent
- **When**: the dashboard model formats the ACTIVE AGENTS card
- **Then**: it renders a row with the subagent's `define_subagent` name and its invocation
  status

### R5: Panel presentation

#### R5.0: The panel names the agent it is showing (AG10)
- **Given**: an attached Antigravity session
- **When**: the panel composes the session row
- **Then**: the provider tag reads `[Antigravity]`; a Claude session's row is unchanged,
  byte for byte, including its pinned snapshots

#### R5.1: The tab's agent selects the detached wording (AG10)
- **Given**: a tab running `agy` with no resolvable transcript yet
- **When**: the panel is open
- **Then**: the detached wording reflects an Antigravity tab, not a Claude one — i.e.
  `Env` carries the tab's `AgentKind` rather than `tab_has_claude`

#### R5.2: Live tail updates
- **Given**: an open panel attached to an active Antigravity session
- **When**: `agy` appends a step to `transcript.jsonl`
- **Then**: the watcher folds the appended line and wakes the event loop once for the batch

#### R5.3: A sibling conversation does not wake the panel (AG7)
- **Given**: an open panel attached to conversation `A`
- **When**: an unrelated conversation `B` writes to
  `brain/B/.system_generated/logs/transcript.jsonl`
- **Then**: no update is folded and no redraw is requested — the `logs` directory name is
  shared by every conversation and must not be the scoping predicate

#### R5.4: Chunk mirrors are ignored
- **Given**: an open panel attached to conversation `A`
- **When**: `agy` writes `brain/A/.system_generated/logs/chunks/transcript/00000001.jsonl`
- **Then**: nothing is double-folded — the chunk files mirror bytes already tailed from
  `transcript.jsonl`

#### R5.5: Unmeasured usage renders as unavailable, not zero (AG4)
- **Given**: an attached Antigravity session that has folded hundreds of steps, whose
  `total_input`, `context_used`, and `context_high_water` are therefore all 0
- **When**: the panel composes CONTEXT and CACHE
- **Then**: both render as explicitly unavailable for this agent — not `0%`, and not a
  percentage of any assumed window

#### R5.6: The Claude panel is unchanged (Constraints)
- **Given**: a tab running `claude` with an attached session
- **When**: the panel is composed after T5's `Env` change
- **Then**: every existing dashboard snapshot reproduces byte-for-byte — the mechanical half
  of AG10 is required to be invisible on the Claude path

## Tasks

### T1: Antigravity session discovery and log resolution [infra]
- **Fulfills**: R1.1, R1.2, R1.3, R1.4, R1.5, R1.6
- **Depends on**: (none)
- **Description**: FD-probe helper in `app` (AG11) mapping a pid to the presence lock it
  holds open, plus conversation-id → transcript-path resolution in `ai-monitor` and the
  two-tier ladder of AG6 (FD → `history.jsonl` by workspace → unattached). No SQLite, and
  **no recency tiebreak across `brain/`** — R1.4 is the test that pins that absence, and it
  is the one most likely to be "helpfully" reintroduced later. Deterministic tests over mock
  directory fixtures, including the stale-lock and live-subagent cases.

### T2: Transcript parser and telemetry fold [infra]
- **Fulfills**: R2.1–R2.8, R3.1–R3.4
- **Depends on**: (none) — parallel with T1
- **Description**: Antigravity format in `ai-monitor::telemetry` per AG2/AG3/AG9: explicit
  format at construction, result-step counting, one-level `args` unwrapping, tool→`ToolKind`
  mapping, `toolSummary` as the display string, turn counters (AG8), the
  `<USER_SETTINGS_CHANGE>` scrape, and RFC3339 parsing. Fixtures copied from real
  `brain/<uuid>` transcripts into `crates/ai-monitor/tests/fixtures/`, beside the Claude ones.

### T3: Watcher polymorphism [infra]
- **Fulfills**: R5.2, R5.3, R5.4
- **Depends on**: T1
- **Description**: Per AG7 — make the watch's re-resolution and its event-scoping predicate
  agent-specific: Antigravity scopes to one transcript path and does not re-resolve; Claude's
  `Wanted`/`RivalHandle` path is preserved byte-for-byte. Tests for the sibling-conversation
  and chunk-mirror cases, which are silent-wrong rather than loud-wrong if missed.

### T4: Tab session hinting and attachment [vertical]
- **Fulfills**: R0.1
- **Depends on**: T1, T2, T3
- **Description**: Extend `tabsession`/`overlay` to recognize an `agy` process under the tab
  shell (reusing `tabmeta::agent`'s classification, including the `agy.<id>.old` case),
  carry the `AgentKind` on the hint, and bind the matching watcher on tab open and switch.

### T5: Dashboard model — snapshot-neutral half [vertical]
- **Fulfills**: R4.1, R5.1, R5.6
- **Depends on**: T4
- **Description**: Per AG10's mechanical half — subagent rows in ACTIVE AGENTS, and
  `Env.tab_has_claude` → `Env.tab_agent: Option<AgentKind>` across its 16 call sites, routing
  the detached wording through it. **The 9 pinned dashboard snapshots must not move in this
  task** (R5.6); if one does, that is a regression on the Claude path, not a re-pin.

### T6: Panel presentation — new UI [vertical]
- **Fulfills**: R5.5
- **Depends on**: T5
- **Description**: Per AG10's new-UI half, landing as one reviewed snapshot diff: the
  agent-name row, the turns tile fed by AG8's counters, and AG4's unavailable CONTEXT/CACHE
  treatment. Kept separate from T5 so the expected diff here cannot hide an unexpected one
  there.

## External Dependencies

### Pre-work
- (none) — AG4, AG6, AG8, and AG10 were approved 2026-08-13; nothing else gates T1.

### Post-work
- (none)
