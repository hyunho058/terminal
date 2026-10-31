//! Stateful session telemetry — the fold behind the ai-monitor status dashboard.
//!
//! [`SessionTelemetry::apply`] folds ONE agent JSONL line into accumulated state;
//! [`SessionTelemetry::snapshot`] yields an owned [`TelemetrySnapshot`] the watcher
//! thread sends over its channel (SD5) and the app formats (SD4).
//!
//! Two log dialects fold into this one state: Claude Code's, and Antigravity CLI's
//! `transcript.jsonl` (`specs/ai-monitor-antigravity`). Which one a `SessionTelemetry`
//! reads is chosen at CONSTRUCTION and never sniffed per line (AG9) — the attaching
//! code already walked the process tree and knows which agent it found, and both
//! dialects carry a `type` key, so a sniff would put a guess where a fact is available
//! and misroute silently, three layers from where the symptom shows up.
//!
//! Four properties are load-bearing and every change must preserve them:
//!
//! * **Pure over its input** (D3) — `apply` takes only `&mut self` + `&str`. No
//!   clock, no filesystem. Uptime/idle are NOT fields: the crate exposes
//!   `first_event_ms`/`last_event_ms` and the app subtracts its own `now`. This is
//!   what makes the fixture tests deterministic.
//! * **Usage is folded once per assistant message** (D1) — Claude Code splits one
//!   API response across N JSONL lines (one per content block) that each replicate
//!   an identical `message.usage`. Summing per line over-reports by ~2.3x on real
//!   data. Usage is keyed on `message.id`, with `requestId` as a retry
//!   discriminator: a new `requestId` under a known id is a genuine retry and folds
//!   again; a repeat is a replica and does not. (Antigravity records no token usage
//!   anywhere in its format — AG4 — so for that dialect the four token counters and
//!   both context gauges stay 0, deliberately and permanently.)
//! * **Never panics** — every arithmetic path saturates and every field read has a
//!   typed fallback, so no line, however malformed or hostile, can abort the
//!   process the overlay lives in.
//! * **Bounded** (D6) — every accumulating structure has a cap. This state lives for
//!   the whole life of a long-running terminal.
//!
//! No presentation policy lives here (SD4): no percentages, no formatted strings,
//! no per-model context-window table, no display thresholds.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::Value;

use crate::activity;

// ---- caps (D6) ----------------------------------------------------------------
// Display rings: ~3x the app's display counts, so the app can show more without a
// crate change. Evicting one only drops a row nobody was going to see.
const TOOLS_CAP: usize = 32;
const SKILLS_CAP: usize = 16;
const FILES_CAP: usize = 32;
const SUBAGENTS_CAP: usize = 32;
/// Tasks are a keyed map, not a ring: `TaskUpdate` mutates an entry created
/// earlier by `taskId`, which a newest-first ring cannot represent.
const TASKS_CAP: usize = 64;
/// A `tool_use` whose result never arrives must not leak.
const PENDING_CAP: usize = 64;
/// The dedupe set is a DIFFERENT kind of bound from the rings: evicting a ring
/// entry loses a display row, evicting a dedupe key silently double-counts tokens.
/// Sized against measured contiguity — replicas of one `message.id` are adjacent
/// among usage-bearing lines, widest observed run 4, so 64 leaves ~16x headroom.
const DEDUPE_CAP: usize = 64;
/// Retries recorded per message id. The dedupe map bounds the number of KEYS; this
/// bounds each key's value, so neither dimension of that structure can grow without
/// limit. 8 is ~2x the widest observed content-block split.
const RETRY_IDS_CAP: usize = 8;
/// Distinct names the cumulative tool tally keeps (D12). The key is a string the
/// LOG supplies and MCP tool names are effectively unbounded, so an uncapped map is
/// a slow leak on a long session. 64 mirrors `DEDUPE_CAP` rather than inventing a
/// new magnitude.
///
/// On overflow the smallest **incumbents** are merged into `tool_calls_other` —
/// never the arriving name. Evicting (or merging) the smallest entry overall is the
/// obvious implementation and is wrong: once 64 names sit at count >= 2, every
/// genuinely new tool enters at 1, is instantly the smallest, and is dropped
/// forever no matter how heavily it is later used.
const TALLY_CAP: usize = 64;
/// Merging runs in ONE batch down to this watermark, so the O(n log n) scan is
/// amortised over the next 16 new names instead of running on every one.
const TALLY_WATERMARK: usize = 48;
/// Digest rings (`session-overview-ui` D19): the newest turns and events kept.
const TURNS_CAP: usize = 64;
const EVENTS_CAP: usize = 64;
/// Background work tracked per turn. A turn that starts more than this many pieces of
/// background work stops tracking the excess, so the turn can end before they report.
const TURN_BG_CAP: usize = 64;
/// A tool summary's length in chars, `…` included (D11).
const SUMMARY_CHARS: usize = 80;
/// A subagent report preview: its first non-blank lines, and their total length in
/// chars, `…` included (D19).
const PREVIEW_LINES: usize = 4;
const PREVIEW_CHARS: usize = 400;

// ---- value types --------------------------------------------------------------

/// Which agent's JSONL dialect a [`SessionTelemetry`] folds (AG9).
///
/// Chosen once, by the caller that attached the watcher — it knows the `AgentKind` it
/// found under the tab's shell, so nothing here has to guess. The two dialects share
/// nothing but the file extension: Claude's lines are `assistant`/`user` records with
/// `message.usage` and `tool_use`/`tool_result` blocks; Antigravity's are numbered
/// `step_index` records with a `tool_calls` dispatch and a separate typed result step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogFormat {
    /// Claude Code's session JSONL — the crate's original and default subject.
    #[default]
    Claude,
    /// Antigravity CLI's `brain/<id>/.system_generated/logs/transcript.jsonl`.
    Antigravity,
}

/// What a file-touching tool did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOp {
    Read,
    Write,
    Grep,
    List,
    Other,
}

/// Which store a completed tool belongs in (D7). Classification is mechanism — the
/// crate must branch on the name anyway; labels and icons are the app's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    Agent,
    Skill,
    File(FileOp),
    Task,
    /// Anything unrecognised. Deliberately NOT `Skill` — the TS reference's
    /// `classifyTool` falls through to `'skill'`, which buckets `Bash` as a skill.
    Plain,
}

/// Classify a tool by name (D7).
pub fn classify(name: &str) -> ToolKind {
    match name {
        "Agent" => ToolKind::Agent,
        "Skill" => ToolKind::Skill,
        "Read" => ToolKind::File(FileOp::Read),
        "Write" | "Edit" => ToolKind::File(FileOp::Write),
        "Grep" => ToolKind::File(FileOp::Grep),
        "Glob" => ToolKind::File(FileOp::List),
        "TaskCreate" | "TaskUpdate" | "TaskOutput" => ToolKind::Task,
        _ => ToolKind::Plain,
    }
}

/// The AG2 mapping, read in the direction the fold needs it: a **dispatched** tool
/// name to the kind it folds as, the **result step type** that will report it, and the
/// arg key holding its path (`None` where the tool touches no file).
///
/// Two spellings exist per call — the `tool_calls` entry on a `PLANNER_RESPONSE` and a
/// typed result step — and this table is what pairs them. It is transcribed from the
/// measured corpus, so note what is NOT in it: there is no `WRITE_FILE` step type;
/// writes land as `CODE_ACTION`. `manage_task`'s result is a `GENERIC`, the same step
/// type four other tools report through, which is why the pairing scans for the oldest
/// pending call *expecting this type* rather than assuming the previous step.
///
/// `None` for a name the table does not know. Such a call is deliberately left
/// uncorrelated rather than matched to the next unclaimed result step: mis-attributing
/// a real call to the wrong name is worse than not counting it, and the miss is visible
/// in `orphan_tool_results` where a silent mis-attribution would not be (AG6's
/// "show nothing rather than adopt whatever else happens to be there", applied to a
/// tool name instead of a session).
fn ag_tool(name: &str) -> Option<(ToolKind, &'static str, Option<&'static str>)> {
    let m = |k, step, path| Some((k, step, path));
    match name {
        "view_file" => m(
            ToolKind::File(FileOp::Read),
            "VIEW_FILE",
            Some("AbsolutePath"),
        ),
        "read_file" => m(ToolKind::File(FileOp::Read), "GENERIC", Some("Path")),
        "write_to_file" | "replace_file_content" => m(
            ToolKind::File(FileOp::Write),
            "CODE_ACTION",
            Some("TargetFile"),
        ),
        "write_file" | "edit_file" => m(ToolKind::File(FileOp::Write), "GENERIC", Some("Path")),
        "grep_search" => m(
            ToolKind::File(FileOp::Grep),
            "GREP_SEARCH",
            Some("SearchPath"),
        ),
        "list_dir" => m(
            ToolKind::File(FileOp::List),
            "LIST_DIRECTORY",
            Some("DirectoryPath"),
        ),
        "find_by_name" => m(
            ToolKind::File(FileOp::List),
            "FIND",
            Some("SearchDirectory"),
        ),
        "run_command" => m(ToolKind::Plain, "RUN_COMMAND", None),
        "invoke_subagent" => m(ToolKind::Agent, "INVOKE_SUBAGENT", None),
        "ask_question" => m(ToolKind::Plain, "ASK_QUESTION", None),
        "define_subagent" | "manage_subagents" | "send_message" | "schedule" => {
            m(ToolKind::Plain, "GENERIC", None)
        }
        "manage_task" => m(ToolKind::Task, "GENERIC", None),
        _ => None,
    }
}

/// A finished plain tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletedTool {
    pub name: String,
    pub ok: bool,
    /// `None` when either timestamp was missing or the interval was negative —
    /// never zero, which would read as a real sub-millisecond measurement (D5).
    pub duration_ms: Option<u64>,
    pub start_ms: Option<u64>,
}

/// A skill invocation. Carries NO duration: a `Skill` tool_use -> tool_result pair
/// measures ~50ms of instruction-load latency, not the skill's execution (D8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillEntry {
    pub name: String,
    pub start_ms: Option<u64>,
}

/// A finished file-touching tool call. `path` holds a real path for
/// `Read`/`Write`/`Edit` and a *pattern* for `Grep`/`Glob` — the app must not
/// basename-shorten a pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileActivity {
    pub op: FileOp,
    pub path: String,
    pub ok: bool,
    pub duration_ms: Option<u64>,
    pub start_ms: Option<u64>,
}

/// A delegated `Agent` call. Flat — there is no parent linkage in real Claude Code
/// JSONL (D9), so the app renders a list, not a tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subagent {
    pub id: String,
    pub subagent_type: Option<String>,
    pub description: Option<String>,
    pub model: Option<String>,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub completed: bool,
    /// The agent's OWN id, read from `toolUseResult.agentId`. Distinct from `id`,
    /// which is the `tool_use_id` of the call that launched it: a task-notification
    /// names *this* one, so it is the only key that can end a background agent at its
    /// real time. Captured from EVERY `Agent` result, not just async launches — a
    /// synchronously finished agent can still be resumed in the background later
    /// (D5/R3).
    pub agent_id: Option<String>,
    /// Whether the end was a success. Written wherever an ending is *observed* — by the
    /// notification branch, and by the `Agent` result itself when that result is an
    /// answer rather than a launch receipt. An in-flight agent and a never-notified
    /// agent keep the initial value, which is why that value must be `true`:
    /// initialising `false` would paint both as failures (D13/R7.1).
    ///
    /// The "notification branch only" rule this field shipped with was narrower than the
    /// facts. A synchronous `Agent` result carries `is_error`, so its success is *known*
    /// at fold time and was already being folded into `failed_tool_calls` — withholding
    /// it here left the card and the counter disagreeing about one result.
    pub ended_ok: bool,
    /// The first [`PREVIEW_LINES`] non-blank lines (at most [`PREVIEW_CHARS`] chars) of
    /// the agent's final report: the synchronous `Agent` result's text, or the `<result>`
    /// of a background agent's task notification (`session-overview-ui` D19). Folded only
    /// by a digest accumulator ([`SessionTelemetry::with_digest`]); `None` otherwise.
    pub report_preview: Option<String>,
}

/// One main-agent turn (`session-overview-ui` D19): it starts at a human prompt and ends
/// at the first moment the assistant has ended its turn (`end_turn`) with no background
/// work started since that prompt still pending. `end_ms` is `None` while either is
/// outstanding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Turn {
    pub start_ms: u64,
    pub end_ms: Option<u64>,
}

/// What a digest [`Event`] records (`session-overview-ui` D13).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// A [`Turn`] ended.
    TurnFinished,
    /// A subagent was launched (`Agent` / `Task` call).
    TaskStart { description: String },
    /// A subagent finished successfully.
    TaskDone { description: String },
    /// A subagent finished with a failure (an errored result, or a notification whose
    /// status is present and not `completed`).
    TaskFailed { description: String },
    /// A tool call other than `Agent` / `Task` finished.
    Tool {
        name: String,
        summary: Option<String>,
    },
}

/// A timestamped digest event. `at_ms` is when it happened: a turn's end, a subagent's
/// launch or end, a tool call's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub at_ms: u64,
    pub kind: EventKind,
}

/// A finished tool as the digest reports it: `(name, summary, start_ms)` (D11).
pub type ToolDigest = (String, Option<String>, Option<u64>);

/// A tracked task. `status` is whatever string the task tool carried — the
/// vocabulary is not enumerated here, so an unknown value passes through intact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskEntry {
    pub subject: String,
    pub status: String,
    pub created_ms: Option<u64>,
    pub updated_ms: Option<u64>,
}

/// What a background task's completion notification says (D12).
///
/// Not part of `TelemetrySnapshot` — it is a message the fold consumes, not state the
/// app reads. Public only so the parse can be asserted directly: its guard against
/// false positives is a set of NEGATIVE behaviours, and those are far more honestly
/// tested on a return value than on the absence of a downstream mutation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskNotification {
    /// The agent's own id, and the join key — this equals `toolUseResult.agentId`,
    /// never the `tool_use_id` of the call that woke it.
    pub task_id: String,
    /// `completed`, `failed` or `killed` in every observed log. Not enumerated here,
    /// so a value invented upstream passes through intact.
    pub status: Option<String>,
}

/// A context window READ out of a `/context` command's stdout, paired with the model
/// that was running when it was read (`ai-monitor-context-window` D12).
///
/// One struct rather than two snapshot fields, because the pair must move ATOMICALLY:
/// the app refuses a reading whose model no longer matches (D9), so a window updated
/// without its model — or the reverse — is a reading attributed to the wrong model.
/// Two independent fields put that mismatch one coding slip away; this puts it beyond
/// construction, since the pair is written whole and replaced whole.
///
/// It does NOT make "a window with no model" unrepresentable, and must not try to:
/// `model_at_read: None` is a state D9 explicitly requires, for a log that has not
/// named a model yet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContextWindowReading {
    /// The window `/context` printed, in tokens.
    ///
    /// A ROUNDED READING, not a measurement: the source prints `700k`, so this is
    /// 700,000 ± up to half a notation unit. Cross-checked as exactly 700,000 across
    /// the corpus (`33000/700000 = 4.714%` displays as `4.7%`), but consistency is not
    /// proof.
    ///
    /// **The error bound is a property of the NOTATION, not a constant.** Half a unit of
    /// `700k` is ±500 — 0.07%, invisible in a percentage, which is the case every reading
    /// observed so far takes. Half a unit of `1.0M` is ±50,000 (5%), and of a bare `1M`
    /// ±500,000 (50%) — and both clear [`WINDOW_FLOOR`], so both would be believed.
    ///
    /// No ANCHOR has yielded `M` notation yet — all 12 anchor readings in the corpus are
    /// `700k`. Do not read that as "`M` is not in these records": `1M` appears in every
    /// one of them, in the model banner `Opus 5 (1M context)` and the id
    /// `claude-opus-5[1m]` (12 occurrences), and it is exactly the string D1a and the
    /// header grammar exist to refuse. The day an anchor DOES print `M`, this bound is
    /// the thing to revisit, not the parse.
    pub tokens: u64,
    /// The model folded at the moment of the read, or `None` if the log had not named
    /// one yet. Compared — as a whole string, never by tier — against the live model
    /// before the reading is used.
    pub model_at_read: Option<String>,
}

/// The versioned contract the watcher sends and the app formats (D11).
///
/// Deliberately omits the pending-tool map and the dedupe set — internal
/// bookkeeping the UI never reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TelemetrySnapshot {
    // identity
    pub session_id: Option<String>,
    /// The model the session is running **now** — the last real `message.model` folded,
    /// not the first. The two differ whenever the user ran `/model`, and the field's
    /// whole purpose is to answer "what is running", so first-wins would be a lie about
    /// the live session. Sentinel values (`<synthetic>`) are never folded here.
    pub model: Option<String>,
    // tokens: four cumulative COST counters (D2) ...
    pub total_input: u64,
    pub total_output: u64,
    pub total_cache_read: u64,
    pub total_cache_creation: u64,
    // ... and a latest-value occupancy gauge, which is a different quantity
    pub context_used: u64,
    /// The largest `context_used` this session has ever reported — monotone, never reset.
    ///
    /// The gauge above answers "how full is the window now"; this answers "how big has the
    /// window been PROVEN to be". They diverge whenever occupancy falls (a `/compact`), and
    /// the app needs the second to pick a denominator: a window cannot shrink mid-session,
    /// so evidence of its size must not expire with the reading that produced it.
    pub context_high_water: u64,
    /// The window READ from a `/context` record, when this session has one — the only
    /// place in a log where the window is stated rather than inferred.
    ///
    /// `None` is the ordinary case (37 of 42 logs in the measured corpus never ran
    /// `/context`), and it means exactly what it meant before this field existed: the
    /// app falls back to its model table and its high-water promotion. Last-wins across
    /// a session, on the same grounds as `model` — `/model` can change the window
    /// mid-session, so the newest reading is the true one.
    pub context_window_reading: Option<ContextWindowReading>,
    // time (D3) — the app derives uptime/idle from these
    /// The timestamp of the first line that carried one (first-wins).
    pub first_event_ms: Option<u64>,
    /// The LATEST timestamp any line carried, not the last line's: a line stamped earlier
    /// than this never lowers it. Lines are written out of timestamp order, so
    /// last-wins used to drag the session into the past (see `observe_ts`).
    pub last_event_ms: Option<u64>,
    // activity, newest-first
    pub tools: Vec<CompletedTool>,
    pub skills: Vec<SkillEntry>,
    pub files: Vec<FileActivity>,
    pub subagents: Vec<Subagent>,
    /// `(taskId, entry)`, in a deterministic id order. Display order is the app's.
    pub tasks: Vec<(String, TaskEntry)>,
    // conversation turns (AG3/AG8) — u64 counters, additive to the versioned
    // contract, defaulting to 0 so an unpopulated pair reads as "not measured".
    /// Prompts the human sent. Antigravity's `USER_INPUT` steps; the Claude fold
    /// leaves this 0 until someone specifies what a turn means there.
    pub turns_user: u64,
    /// Replies the agent made. Antigravity's `PLANNER_RESPONSE` steps carrying **no**
    /// `tool_calls` — the tool-bearing ones are dispatch, not replies. Measured 54
    /// user inputs against 61 tool-less planner responses (1.13, which is what a turn
    /// pair looks like) and 772 tool-bearing ones; counting every planner step would
    /// report a 15:1 assistant:user ratio, a different quantity wearing the word
    /// "turn" (AG3).
    pub turns_assistant: u64,
    // cumulative call counters (D3) — a DIFFERENT quantity from the rings above.
    // Those are bounded display windows: on a 3-hour session everything older than
    // the last `TOOLS_CAP` calls is gone, so counting a ring would print a number
    // that LOOKS cumulative and is not.
    /// `(tool name, calls)` over the whole session, in a deterministic NAME order.
    /// Ordering by count would be a ranking, and ranking is presentation (SD4).
    /// Bounded at `TALLY_CAP` names; the overflow lands in `tool_calls_other`.
    pub tool_calls: Vec<(String, u64)>,
    /// Calls merged out of `tool_calls` when it overflowed its name cap. It names
    /// nothing and never drains, so it belongs in no ranking and in no bar
    /// denominator (D12) — but it is what keeps the tally's sum honest.
    pub tool_calls_other: u64,
    /// Every call folded, as its OWN scalar — deliberately not a sum over
    /// `tool_calls`, which is bounded, so the headline number is immune to the map's
    /// cap. It equals `tool_calls`'s sum plus `tool_calls_other` by construction.
    pub total_tool_calls: u64,
    /// Calls whose `tool_result` carried `is_error: true`, folded at the one place
    /// `ok` is derived. Kept apart from the `parse_errors` family below: those count
    /// PARSER faults, which is a different fact — displaying one under the other's
    /// label would be a lie about what failed.
    pub failed_tool_calls: u64,
    // diagnostics — kept apart so a mis-sized tail window (benign) is never
    // conflated with a changed result format (a real regression)
    pub parse_errors: u64,
    pub usage_conflicts: u64,
    pub orphan_tool_results: u64,
    pub orphan_task_updates: u64,
    pub unparsed_task_creates: u64,
    pub duplicate_tool_ids: u64,
    /// `/context` rendered and NEITHER anchor yielded a usable window (D10).
    ///
    /// Without it this failure is unobservable: both anchors can break at once and the
    /// panel just goes back to guessing, silently, with no test able to assert that the
    /// format moved. Since the gate is the panel's own title, a non-zero value is a
    /// definite, actionable statement — "the panel rendered and we could not read it" —
    /// rather than a count of slash commands. What it does NOT say is WHY: a drifted
    /// format, a truncated record and a sub-floor value are one number here.
    pub context_window_parse_failures: u64,
    /// Both anchors yielded a window and they DISAGREED (the labelled line is adopted).
    ///
    /// Kept apart from the failures above on this crate's existing rule: those count a
    /// read that produced nothing, this counts two sources of the same fact
    /// contradicting each other. Filing one under the other's name would be a lie about
    /// what went wrong — the same reason `failed_tool_calls` is not a `parse_error`.
    pub context_window_anchor_conflicts: u64,

    // digest (`session-overview-ui` D11, D19) — folded only by an accumulator built
    // `with_digest`; empty / `None` otherwise.
    /// The newest finished tool call, any name.
    pub last_tool: Option<ToolDigest>,
    /// The newest finished tool call whose name is neither `Agent` nor `Task`.
    pub last_work_tool: Option<ToolDigest>,
    /// The latest human prompt's timestamp (`activity`'s G8 definition of one).
    pub last_prompt_ms: Option<u64>,
    /// The newest [`TURNS_CAP`] turns, newest first.
    pub turns: Vec<Turn>,
    /// The newest [`EVENTS_CAP`] events, newest first.
    pub events: Vec<Event>,
}

/// A `tool_use` awaiting its `tool_result`, holding the per-kind facts D7 extracts.
#[derive(Clone, Debug, Default)]
struct Pending {
    name: String,
    kind: Option<ToolKind>,
    start_ms: Option<u64>,
    path: Option<String>,
    skill: Option<String>,
    subagent_type: Option<String>,
    description: Option<String>,
    model: Option<String>,
    subject: Option<String>,
    task_id: Option<String>,
    status: Option<String>,
    task_description: Option<String>,
    /// D11's one-line summary of the input. Digest only.
    summary: Option<String>,
}

type UsageTuple = (u64, u64, u64, u64);

/// An Antigravity tool call dispatched by a `PLANNER_RESPONSE`, awaiting the typed
/// result step that reports it (AG2).
///
/// The dispatch is where the `args` live — measured, `tool_calls` occurs on
/// `PLANNER_RESPONSE` steps and nowhere else (772 of 772) — and the result step is
/// where `status` and the completion time live. Neither record alone can produce a
/// `FileActivity`, so one has to wait for the other, exactly as a Claude `tool_use`
/// waits for its `tool_result`. What differs is the join: Claude pairs on a
/// `tool_use_id`, and Antigravity's calls carry no id at all, so the pairing is by
/// expected result-step type in dispatch order (see [`ag_tool`]).
#[derive(Clone, Debug)]
struct AgCall {
    /// The name the LOG supplied, which is what the tally is keyed on (AG2) —
    /// `run_command`, never rewritten to `Bash`, or the panel would print a Claude
    /// vocabulary over a session that never used one.
    name: String,
    kind: ToolKind,
    /// The result step type that will report this call, or `None` for a name
    /// [`ag_tool`] does not know — which can therefore never be claimed by a result.
    expects: Option<&'static str>,
    start_ms: Option<u64>,
    path: Option<String>,
    /// `toolSummary`, present on all 773 measured calls: a short human-written label
    /// and the natural display string, so nothing is synthesised from the tool name.
    summary: Option<String>,
    subagent_type: Option<String>,
    description: Option<String>,
}

/// The accumulator. Fold lines in with [`apply`](SessionTelemetry::apply); read the
/// result with [`snapshot`](SessionTelemetry::snapshot).
#[derive(Debug, Default)]
pub struct SessionTelemetry {
    /// Fixed at construction (AG9). `apply` dispatches on it and never re-decides.
    format: LogFormat,
    session_id: Option<String>,
    /// The session id the LOG PATH names, when the caller knew it at construction —
    /// Antigravity's case, where the conversation id is the `brain/<id>/` directory and
    /// appears nowhere in the file. Deliberately a second field rather than a pre-set
    /// `session_id`: this one is a fact about *which log this is*, so it must survive the
    /// truncation reset that clears every folded number.
    log_session_id: Option<String>,
    model: Option<String>,
    total_input: u64,
    total_output: u64,
    total_cache_read: u64,
    total_cache_creation: u64,
    context_used: u64,
    context_high_water: u64,
    /// Last `/context` reading (D5 last-wins). Reset with everything else by
    /// `restarted()`: a file that shrank is most likely a different session, and a
    /// window read from the old one would outlive its evidence.
    context_window_reading: Option<ContextWindowReading>,
    /// First-wins; written only by `observe_ts`.
    first_event_ms: Option<u64>,
    /// The latest timestamp seen, never lowered by an earlier-stamped line; written only
    /// by `observe_ts`.
    last_event_ms: Option<u64>,

    tools: VecDeque<CompletedTool>,
    skills: VecDeque<SkillEntry>,
    files: VecDeque<FileActivity>,
    subagents: VecDeque<Subagent>,
    tasks: HashMap<String, TaskEntry>,

    // cumulative counters (D3/D12) — independent of the rings above
    turns_user: u64,
    turns_assistant: u64,
    tool_calls: HashMap<String, u64>,
    tool_calls_other: u64,
    total_tool_calls: u64,
    failed_tool_calls: u64,

    pending: HashMap<String, Pending>,
    pending_order: VecDeque<String>,
    /// Antigravity's pending queue (AG2). A `VecDeque`, not the id-keyed map above,
    /// because its calls have no ids — and bounded by the same `PENDING_CAP`, since a
    /// dispatch whose result never arrives (an interrupted async `run_command`, or one
    /// still in flight at EOF) must not leak either.
    ag_pending: VecDeque<AgCall>,
    /// The most recent `define_subagent` (`name`, `description`), which is the only
    /// human-readable identity an `invoke_subagent` can be given (AG5).
    ag_subagent_def: Option<(Option<String>, Option<String>)>,
    seen_usage: HashMap<String, (UsageTuple, Vec<String>)>,
    seen_order: VecDeque<String>,

    parse_errors: u64,
    usage_conflicts: u64,
    orphan_tool_results: u64,
    orphan_task_updates: u64,
    unparsed_task_creates: u64,
    duplicate_tool_ids: u64,
    context_window_parse_failures: u64,
    context_window_anchor_conflicts: u64,

    // digest (`session-overview-ui` D19). Untouched unless `digest` is set.
    digest: bool,
    last_tool: Option<ToolDigest>,
    last_work_tool: Option<ToolDigest>,
    last_prompt_ms: Option<u64>,
    /// Newest first; only the front can be open.
    turns: VecDeque<Turn>,
    events: VecDeque<Event>,
    /// Background work started since the last human prompt and not yet reported back,
    /// by the ids `activity` keys G8's Working rule on.
    turn_bg: HashSet<String>,
    /// The assistant has ended its turn (`end_turn`) since the last prompt or the last
    /// background completion that resumed it.
    turn_ended: bool,
}

impl SessionTelemetry {
    /// A Claude Code accumulator — the crate's original subject, and what every
    /// existing call site means by "a session".
    pub fn new() -> Self {
        Self::default()
    }

    /// An accumulator for `format` (AG9). The caller has already identified the agent
    /// it attached to, so the dialect is a fact here, not a guess.
    pub fn with_format(format: LogFormat) -> Self {
        Self {
            format,
            ..Self::default()
        }
    }

    /// Tell the accumulator the session id its log's PATH names, for a format that does not
    /// write one into the file (Antigravity — see `antigravity::conversation_id_of`).
    ///
    /// `None` is a legitimate argument and means only "the caller could not name it": the
    /// panel then shows a dash, which is the honest reading of an unresolved tab.
    pub fn with_log_session_id(mut self, session_id: Option<String>) -> Self {
        self.log_session_id = session_id;
        self
    }

    /// A fresh accumulator for the SAME log — used when the file is truncated under the tail.
    ///
    /// Keeps exactly the two things that are properties of the log rather than of its
    /// contents: the dialect and the path-derived id. Everything folded is dropped, which is
    /// the entire point of the reset. `SessionTelemetry::new()` was used here before and
    /// silently reset the dialect to Claude, contradicting [`format`](Self::format)'s own
    /// "fixed for its whole life" — an Antigravity transcript that shrank would have kept
    /// tailing in the wrong dialect for the rest of its life.
    ///
    /// **The `subagents` ring is dropped too, and that has a known cost.** A live background
    /// agent loses its card here, and the `<task-notification>` that would have ended it later
    /// finds no entry to join and is discarded. Since agents began staying live for their whole
    /// run, this window is an agent's lifetime rather than the ~83ms it used to be. Carrying
    /// live entries across the reset was considered and refused: a file that shrank is most
    /// likely not the same session any more, so importing its agents manufactures the permanent
    /// LIVE ghost the conservative-failure rule (D2) exists to refuse — and does it on every
    /// truncation, not the rare one. See `specs/agent-elapsed-sidechain/spec.md` § Known Gaps.
    pub fn restarted(&self) -> Self {
        let fresh = Self::with_format(self.format).with_log_session_id(self.log_session_id.clone());
        if self.digest {
            fresh.with_digest()
        } else {
            fresh
        }
    }

    /// Also fold the session digest (`session-overview-ui` D19): per finished tool a
    /// one-line summary (D11), `last_tool`, `last_work_tool`, `last_prompt_ms`, `turns`,
    /// `events` and each subagent's `report_preview`.
    ///
    /// Opt-in because only the session overview reads it. Without it none of that work is
    /// done, those fields stay empty, and every other snapshot field is the same as with
    /// it. Claude Code logs only: an Antigravity accumulator folds no digest either way.
    pub fn with_digest(mut self) -> Self {
        self.digest = true;
        self
    }

    /// The dialect this accumulator folds. Fixed for its whole life.
    pub fn format(&self) -> LogFormat {
        self.format
    }

    /// Fold one JSONL line. Never panics; a line that is not valid JSON increments
    /// `parse_errors` and returns (D10 — the existing `parse_line` contract).
    pub fn apply(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            self.parse_errors += 1;
            return;
        };
        match self.format {
            LogFormat::Claude => self.apply_claude(&v),
            LogFormat::Antigravity => self.apply_antigravity(&v),
        }
    }

    /// Fold one parseable timestamp into the session clock, for both dialects
    /// (ai-monitor-absorbed-notifications D5).
    ///
    /// `last_event_ms` is the LATEST timestamp seen, not the last one folded. Lines are
    /// not written in timestamp order: an absorbed task-notification is appended hours
    /// after the stamp it carries, an `assistant` message is stamped when its stream
    /// began, and Antigravity writes a `PLANNER_RESPONSE` after the tool steps it
    /// dispatched. Under last-wins each of those dragged the clock into the past, and
    /// with it the pill, the IDLE tile and the footer. Every large regression measured
    /// had a genuine event as the value it undid, and both resolvers
    /// (`last_event_in_tail`, `last_step_in_tail`) already took the max.
    ///
    /// The price, accepted by name: a line stamped in the FUTURE now holds the clock
    /// until real activity passes it, where last-wins would have self-corrected on the
    /// next line. The fold reads no clock, so it cannot tell "stamped ahead" from "just
    /// happened" and cannot bound this; a bound would belong to the app (spec G1).
    ///
    /// `first_event_ms` stays first-wins: moving it to the minimum is a separate
    /// question with no single right answer for a resumed log (spec G2).
    fn observe_ts(&mut self, t: u64) {
        self.first_event_ms.get_or_insert(t);
        self.last_event_ms = self.last_event_ms.max(Some(t));
    }

    // ---- Claude Code (D1-D12) -------------------------------------------------

    fn apply_claude(&mut self, v: &Value) {
        // Time (D4): only a parseable timestamp moves the clock. A bookkeeping line
        // (`mode`, `permission-mode`, ...) carries none and must not reset idle.
        let ts = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_ts);
        if let Some(t) = ts {
            self.observe_ts(t);
        }
        if self.digest {
            self.digest_turn(v, ts);
        }

        if self.session_id.is_none() {
            self.session_id = str_at(v, "sessionId")
                .or_else(|| str_at(v, "session_id"))
                .map(str::to_string);
        }
        // Identity, but NOT the rule the `session_id` fold above uses. A session id is
        // fixed for the life of a file, so first-wins is correct there. A model is not:
        // `/model` switches it mid-session and every later assistant line carries the
        // new id, so the panel must show the LAST one — first-wins pins the panel to
        // whatever the session happened to start with (measured: a real log ran
        // `claude-fable-5`, switched, then logged two further `claude-opus-5` turns, and the
        // panel still read `claude-fable-5`).
        //
        // Skipping sentinels is what makes last-wins safe. Only a real id may
        // overwrite; a sentinel neither overwrites nor blocks, so the first real fold
        // still populates the field and a trailing `<synthetic>` cannot blank it.
        if let Some(m) = v
            .pointer("/message/model")
            .and_then(Value::as_str)
            .or_else(|| v.get("model").and_then(Value::as_str))
            .filter(|m| is_real_model(m))
        {
            self.model = Some(m.to_string());
        }

        self.fold_usage(v);

        // Tool blocks arrive both as top-level events and nested in message.content
        // (the form real logs use). Handle both (D5).
        // `on_tool_result` needs BOTH halves of the record: the content block carries
        // the result's text, while `toolUseResult` — which says whether an agent was
        // merely launched, and what its own id is — is a sibling of `message` at the
        // top level. In the top-level form the two are the same value (D10/R1.6).
        match v.get("type").and_then(Value::as_str) {
            Some("tool_use") => self.on_tool_use(v, ts),
            Some("tool_result") => self.on_tool_result(v, v, ts),
            // Routed on the TAG, acted on by the parse — the same rule the
            // task-notification branch follows (D12/R4.3), and for a sharper reason
            // here. The string `Auto-compact window` lives in FIVE record types, and the
            // four that are prose outnumber the authoritative `system` records by an
            // order of magnitude. A whole-line grep reads a quotation as a measurement;
            // dispatching on `type` cannot.
            //
            // The RATIO is the durable fact; the absolute counts are not, and are given
            // with a date for that reason. Every conversation about this feature adds
            // more prose: 18/10/1/1 against 6 on 2026-09-11, already 58/57/4/4 against 6
            // on 2026-09-12. A count written here as bare present tense is wrong within a
            // day — the spec makes the same point about its own tables.
            Some("system") => self.on_system(v),
            // The second carrier of a task-notification: one that arrives mid-turn is
            // absorbed into the running turn and written as a `queued_command`
            // attachment, never as a bare-string `user` record. Dispatched here on the
            // record's `type`, like `system` above, because an attachment has no
            // `message` for the content match below to reach.
            //
            // `ts` is the record's own timestamp, not the clock. The attachment is
            // appended when the turn absorbs it (hours later, at the extreme) but keeps
            // the stamp of the notification's arrival, so it is the agent's real end
            // even though it is older than the line before it (ai-monitor-absorbed-
            // notifications D3/D6).
            Some("attachment") => self.on_attachment(v, ts),
            _ => {}
        }
        // Only a conversation record's content is an event. Claude Code 2.1.302 began
        // writing `api-request-blob` records: a copy of the outgoing API request, with no
        // timestamp, whose `message.content` repeats earlier assistant turns block for
        // block. Scanned, every `Agent` call it repeats opened a second ring entry with no
        // start and no agent id, which nothing could ever complete, so a finished agent
        // stayed on the panel as a permanent live ghost (measured: 11 real agents folded
        // to 19 cards). Allowed by name rather than refusing the blob by name, so the next
        // record type that carries a transcript copy is ignored by default rather than
        // folded. On 2026-10-18 the whole local corpus held `message.content` on exactly
        // three types: `assistant`, `user` and `api-request-blob`.
        if !matches!(
            v.get("type").and_then(Value::as_str),
            Some("assistant" | "user")
        ) {
            return;
        }
        match v.pointer("/message/content") {
            Some(Value::Array(items)) => {
                for it in items {
                    match it.get("type").and_then(Value::as_str) {
                        Some("tool_use") => self.on_tool_use(it, ts),
                        Some("tool_result") => self.on_tool_result(it, v, ts),
                        Some("text") => {
                            if let Some(t) = it.get("text").and_then(Value::as_str) {
                                self.scan_command_name(t, ts);
                            }
                        }
                        _ => {}
                    }
                }
            }
            // Both content forms occur in the wild; the TS reference checks only
            // this one and would miss the array form above.
            Some(Value::String(s)) => {
                // A task-notification delivered between turns lands here, as a bare
                // string. It is one of the two signals a background agent ends by; the
                // other is the `queued_command` attachment a notification becomes when
                // it arrives mid-turn (`on_attachment`). It gets its own branch and
                // stops: routing it on to `scan_command_name` would let a command
                // wrapper quoted in its free-text body register a phantom skill.
                //
                // Deliberately NOT folded into `scan_command_name` — the array arm
                // above feeds arbitrary assistant and user prose into that same
                // function, so a parse living there could be reached by anything that
                // merely mentions a notification (D12/R4.3/R4.4).
                // Routed on the TAG, acted on by the parse. Gating the return on parse
                // SUCCESS was the bug: a truncated or malformed notification — one whose
                // `<task-id>` is missing or empty — fell through to `scan_command_name`
                // and let a `<command-name>` quoted in its body register the very phantom
                // skill the paragraph above forbids. A string that announces itself as a
                // notification is never arbitrary prose, whether or not we could read it,
                // so it leaves here either way (R4.4/R4.4a).
                if s.trim_start().starts_with(TASK_NOTIFICATION_TAG) {
                    if let Some(n) = parse_task_notification(s) {
                        self.on_task_notification(&n, ts);
                        if self.digest {
                            self.digest_notification(&n, Some(s), ts);
                        }
                    }
                    return;
                }
                self.scan_command_name(s, ts)
            }
            _ => {}
        }
    }

    // ---- Antigravity CLI (AG2, AG3, AG4, AG5) ---------------------------------

    /// Fold one `transcript.jsonl` step.
    ///
    /// The step envelope is also the format check (R2.8). `step_index` and `type` are
    /// present on all 1,761 measured steps and on no Claude line, so a well-formed
    /// Claude line arriving here is counted a parse error rather than folded — a line
    /// from the wrong format is a bug upstream, not input to interpret, and half-folding
    /// it would produce plausible-looking telemetry for a session that never ran.
    ///
    /// Gating on those two rather than on all five universal keys is deliberate:
    /// `status` / `source` / `created_at` add nothing to the discrimination (no Claude
    /// line carries `step_index` either way) and would turn a whole transcript into
    /// parse errors if Antigravity ever made one of them optional (G6 — the corpus is
    /// one machine over three weeks, and the CLI is a moving target).
    fn apply_antigravity(&mut self, v: &Value) {
        let Some(step_type) = v.get("type").and_then(Value::as_str) else {
            self.parse_errors += 1;
            return;
        };
        if v.get("step_index").is_none() {
            self.parse_errors += 1;
            return;
        }

        // Time (R3.1): `created_at` is RFC 3339 with a `Z` suffix and no fractional
        // part. NEVER the `Created At:` / `Completed At:` lines inside `content` —
        // those are a LOCAL-offset clock, and mixing the two would silently shift
        // every duration by the machine's UTC offset.
        let ts = v
            .get("created_at")
            .and_then(Value::as_str)
            .and_then(parse_ts);
        if let Some(t) = ts {
            self.observe_ts(t);
        }

        let content = v.get("content").and_then(Value::as_str);

        match step_type {
            "USER_INPUT" => {
                self.turns_user = self.turns_user.saturating_add(1);
                // AG4 — the model is prose inside this envelope, not a field, and only
                // a USER_INPUT step is authoritative about it: one measured
                // `grep_search` result echoed a settings sentence back, and scraping
                // every step would adopt a session's own log output as its model.
                if let Some(m) = content.and_then(scrape_model_selection) {
                    self.model = Some(m);
                }
                // Skills, from the same envelope and for the same reason (G10, closed).
                // Antigravity has no skill TOOL — which is why an audit of tool names
                // concluded, wrongly, that no skill signal existed. The signal is a
                // `<SKILL>` block injected into the user's own turn, and it names the
                // skill in parentheses. Scoped to `USER_INPUT` exactly as the model
                // scrape is, and for the identical reason: a `grep_search` result that
                // quoted one of these back would otherwise be folded as an invocation.
                if let Some(name) = content.and_then(scrape_skill_invocation) {
                    push_capped(
                        &mut self.skills,
                        SkillEntry { name, start_ms: ts },
                        SKILLS_CAP,
                    );
                }
            }
            // AG3 — a turn is a REPLY. The tool-bearing planner steps are dispatch,
            // and they are the overwhelming majority (772 against 61), so counting
            // them here is not a small over-count: it reports a different quantity.
            "PLANNER_RESPONSE" => {
                let inp = v.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                let out = v.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
                let cr = v
                    .get("cache_read_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                if inp > 0 || out > 0 || cr > 0 {
                    self.total_input = self.total_input.saturating_add(inp);
                    self.total_output = self.total_output.saturating_add(out);
                    self.total_cache_read = self.total_cache_read.saturating_add(cr);
                    let used = inp.saturating_add(cr);
                    self.context_used = used;
                    self.context_high_water = self.context_high_water.max(used);
                }

                match v.get("tool_calls") {
                    None => self.turns_assistant = self.turns_assistant.saturating_add(1),
                    // An empty array dispatched nothing, so it is a reply like any other.
                    Some(Value::Array(calls)) if calls.is_empty() => {
                        self.turns_assistant = self.turns_assistant.saturating_add(1)
                    }
                    Some(Value::Array(calls)) => {
                        for c in calls {
                            self.ag_dispatch(c, ts);
                        }
                    }
                    // Present but not an array: structurally unexpected (R2.7). Counting a
                    // turn here would be a guess about which half of the step is intact.
                    Some(_) => self.parse_errors += 1,
                }
            }
            "SYSTEM_MESSAGE" => {
                if let Some(c) = content {
                    for id in ag_completed_senders(c) {
                        if let Some(a) = self
                            .subagents
                            .iter_mut()
                            .find(|a| a.id == id || a.agent_id.as_deref() == Some(id))
                        {
                            a.completed = true;
                            if let Some(new_ts) = ts {
                                a.end_ms = Some(a.end_ms.map_or(new_ts, |old| old.max(new_ts)));
                            }
                        }
                    }
                }
            }
            _ => self.ag_result(step_type, v, content, ts),
        }
    }

    /// Record one dispatched `tool_calls` entry. Deliberately counts NOTHING: every
    /// call is written twice (773 dispatch entries against 754 result steps in the
    /// corpus), and tallying both roughly doubles every number the panel shows. This
    /// is the single largest correctness trap in the format (AG2).
    fn ag_dispatch(&mut self, call: &Value, ts: Option<u64>) {
        let Some(name) = call.get("name").and_then(Value::as_str) else {
            self.parse_errors += 1;
            return;
        };
        let args = call.get("args");
        let (kind, expects, path_key) = match ag_tool(name) {
            Some((k, step, path)) => (k, Some(step), path),
            None => (ToolKind::Plain, None, None),
        };

        let path = path_key.and_then(|k| {
            let got = ag_arg_str(args, k);
            // R2.7 names exactly this case: an arg that does not parse as its expected
            // inner type. The call still folds — a missing path costs one FILE ACTIVITY
            // row, and dropping the call would cost the tally a real invocation.
            if got.is_none() {
                self.parse_errors += 1;
            }
            got
        });

        // AG5 — `invoke_subagent`'s args carry only `Subagents[].Model` and `.Prompt`;
        // there is no role, type or name on the invocation. The human-readable identity
        // comes from the `define_subagent` that precedes it, and since the invocation
        // carries no key to join on, "precedes" is the whole of the rule. Kept, not
        // consumed: for any invocation, the preceding definition is the most recent one.
        if name == "define_subagent" {
            self.ag_subagent_def =
                Some((ag_arg_str(args, "name"), ag_arg_str(args, "description")));
        }

        let (subagent_type, description) = if name == "invoke_subagent" {
            let t = ag_arg_str(args, "TypeName");
            let r = ag_arg_str(args, "Role");
            if t.is_some() || r.is_some() {
                (t, r)
            } else {
                self.ag_subagent_def.clone().unwrap_or_default()
            }
        } else {
            (None, None)
        };

        self.ag_pending.push_back(AgCall {
            name: name.to_string(),
            kind,
            expects,
            start_ms: ts,
            path,
            summary: ag_arg_str(args, "toolSummary"),
            subagent_type,
            description,
        });
        while self.ag_pending.len() > PENDING_CAP {
            self.ag_pending.pop_front();
        }
    }

    /// Fold one typed result step — the single counting event for a tool call (AG2).
    fn ag_result(&mut self, step_type: &str, v: &Value, content: Option<&str>, ts: Option<u64>) {
        // What a step reports is not always what it is typed as — see `ag_reported_type`.
        let step_type = ag_reported_type(step_type, content);
        // Steps that report no tool call fold nothing: `CHECKPOINT`,
        // `CONVERSATION_HISTORY`, `SYSTEM_MESSAGE`, and — for now — `ERROR_MESSAGE`.
        // G4 leaves the whole error family (`exit_code`, the `error` key,
        // `ERROR_MESSAGE`) unmapped in this pass, so `failed_tool_calls` stays 0 for
        // Antigravity and the dispatch an `ERROR_MESSAGE` answers simply ages out of
        // `ag_pending`. That is a declared gap, not an oversight.
        let Some(idx) = self
            .ag_pending
            .iter()
            .position(|c| c.expects == Some(step_type))
        else {
            // Reached both by an unmapped step type and by a genuine orphan — the
            // watcher starts from a bounded tail, so results whose dispatch sits above
            // the window always occur. Only count the second: an `ERROR_MESSAGE` in a
            // session with no pending call is not a missing dispatch.
            if AG_RESULT_STEPS.contains(&step_type) {
                self.orphan_tool_results += 1;
            }
            return;
        };
        let Some(call) = self.ag_pending.remove(idx) else {
            return;
        };

        // The counters fold HERE, at the result step and at the one place per call,
        // for the same reason the Claude path folds them in `on_tool_result`: one
        // completed call, one increment, whatever ring it lands in or none.
        //
        // `ok` is unconditionally true under G4 — nothing in this pass reads
        // `exit_code`, so nothing may claim a call failed.
        self.tally_call(&call.name, true);
        let duration_ms = match (call.start_ms, ts) {
            (Some(a), Some(b)) if b >= a => Some(b - a),
            _ => None,
        };

        // TASKS, from `content` rather than from `call.kind` (G5, closed 2026-08-16).
        //
        // The original reading — "the transcript never records a task model" — was wrong,
        // and wrong in a way worth naming: it was inferred from `manage_task`'s ARGS, which
        // carry only an id and an action, without reading the result bodies where the
        // subject and the status actually live. A user asking why the card was empty is
        // what prompted the re-read.
        if let Some(c) = content {
            if let Some((id, subject)) = ag_task_created(c) {
                self.tasks.insert(
                    id,
                    TaskEntry {
                        subject,
                        // Born running: the announcement exists precisely because the
                        // command outlived its synchronous wait.
                        status: "in_progress".to_string(),
                        created_ms: ts.or(call.start_ms),
                        updated_ms: ts.or(call.start_ms),
                    },
                );
                self.evict_tasks();
            } else if let Some((id, status)) = ag_task_status(c) {
                // Only ever UPDATES an entry it already knows. A status report whose
                // creation announcement sits above the watcher's bounded tail would
                // otherwise create a subject-less row, and a task card with no subject is
                // worse than no card: `orphan_task_updates` records it instead, which is
                // the same accounting the Claude path uses for the same situation.
                match self.tasks.get_mut(&id) {
                    Some(entry) => {
                        entry.status = status;
                        entry.updated_ms = ts.or(entry.updated_ms);
                    }
                    None => self.orphan_task_updates += 1,
                }
            }
        }

        match call.kind {
            ToolKind::File(op) => push_capped(
                &mut self.files,
                FileActivity {
                    op,
                    // Same fallback as the Claude path: a call whose path could not be
                    // unwrapped still names the tool that made it.
                    path: call.path.clone().unwrap_or_else(|| call.name.clone()),
                    ok: true,
                    duration_ms,
                    start_ms: call.start_ms,
                },
                FILES_CAP,
            ),
            ToolKind::Agent => {
                let (name, description) =
                    if call.subagent_type.is_some() || call.description.is_some() {
                        (call.subagent_type.clone(), call.description.clone())
                    } else {
                        self.ag_subagent_def.clone().unwrap_or_default()
                    };
                let is_spawn_receipt = content.is_some_and(|c| {
                    c.contains("Created the following subagent:")
                        || c.contains("Created the following subagents:")
                });
                let completed = if is_spawn_receipt {
                    false
                } else {
                    v.get("status").and_then(Value::as_str) == Some("DONE")
                };
                push_capped(
                    &mut self.subagents,
                    Subagent {
                        id: content.and_then(ag_conversation_id).unwrap_or_default(),
                        subagent_type: name,
                        description,
                        model: None,
                        start_ms: call.start_ms,
                        end_ms: completed.then_some(ts).flatten(),
                        completed,
                        agent_id: None,
                        ended_ok: true,
                        report_preview: None,
                    },
                    SUBAGENTS_CAP,
                )
            }
            // The TASKS bookkeeping happens above, off `content`, because the two halves
            // of a task's life arrive on DIFFERENT tools: it is born on a `run_command`
            // whose async wait expired, and it is updated by `manage_task`. Neither arm
            // here could see both. `Skill` is unreachable — no Antigravity tool maps to
            // it — and shares this arm rather than inventing a placeholder entry.
            ToolKind::Task | ToolKind::Skill => {}
            ToolKind::Plain => push_capped(
                &mut self.tools,
                CompletedTool {
                    // `toolSummary`, not the tool name: it is on every call, it is
                    // written for a human, and RECENT CALLS is a display ring. The
                    // NAME still keys the tally, so the two never disagree about what
                    // ran (AG2).
                    name: call.summary.clone().unwrap_or_else(|| call.name.clone()),
                    ok: true,
                    duration_ms,
                    start_ms: call.start_ms,
                },
                TOOLS_CAP,
            ),
        }
    }

    /// An owned, borrow-free view — `Clone + Send + Sync`, so the watcher can send
    /// it (wrapped in an `Arc`) across the channel.
    pub fn snapshot(&self) -> TelemetrySnapshot {
        let mut tasks: Vec<(String, TaskEntry)> = self
            .tasks
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        // A HashMap has no order; emit a deterministic one so snapshot tests are
        // stable. WHICH order to display is the app's policy, not ours.
        tasks.sort_by_key(|t| task_id_key(&t.0));

        // Same reason, same rule: a HashMap has no order, so emit one. By NAME,
        // not by count — sorting by count is a ranking, and which tools to rank
        // and show is the app's policy (SD4).
        let mut tool_calls: Vec<(String, u64)> = self
            .tool_calls
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        tool_calls.sort_by(|a, b| a.0.cmp(&b.0));

        TelemetrySnapshot {
            // Folded first, path second — and `or_else` fires ONLY when the fold found
            // nothing, which is not a tiebreak but a partition: Claude writes `sessionId`
            // into every line and seeds no path id, Antigravity writes none and is the only
            // format seeded. So neither format can reach the other's branch, and no log can
            // have its own recorded id overridden by an inference from its path.
            session_id: self
                .session_id
                .clone()
                .or_else(|| self.log_session_id.clone()),
            model: self.model.clone(),
            total_input: self.total_input,
            total_output: self.total_output,
            total_cache_read: self.total_cache_read,
            total_cache_creation: self.total_cache_creation,
            context_used: self.context_used,
            context_high_water: self.context_high_water,
            context_window_reading: self.context_window_reading.clone(),
            first_event_ms: self.first_event_ms,
            last_event_ms: self.last_event_ms,
            tools: self.tools.iter().cloned().collect(),
            skills: self.skills.iter().cloned().collect(),
            files: self.files.iter().cloned().collect(),
            subagents: self.subagents.iter().cloned().collect(),
            tasks,
            turns_user: self.turns_user,
            turns_assistant: self.turns_assistant,
            tool_calls,
            tool_calls_other: self.tool_calls_other,
            total_tool_calls: self.total_tool_calls,
            failed_tool_calls: self.failed_tool_calls,
            parse_errors: self.parse_errors,
            usage_conflicts: self.usage_conflicts,
            orphan_tool_results: self.orphan_tool_results,
            orphan_task_updates: self.orphan_task_updates,
            unparsed_task_creates: self.unparsed_task_creates,
            duplicate_tool_ids: self.duplicate_tool_ids,
            context_window_parse_failures: self.context_window_parse_failures,
            context_window_anchor_conflicts: self.context_window_anchor_conflicts,
            last_tool: self.last_tool.clone(),
            last_work_tool: self.last_work_tool.clone(),
            last_prompt_ms: self.last_prompt_ms,
            turns: self.turns.iter().copied().collect(),
            events: self.events.iter().cloned().collect(),
        }
    }

    // ---- usage (D1, D2) -------------------------------------------------------

    fn fold_usage(&mut self, v: &Value) {
        let Some(u) = v
            .pointer("/message/usage")
            .or_else(|| v.get("usage"))
            .and_then(Value::as_object)
        else {
            return;
        };
        let vals: UsageTuple = (
            u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
            u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
            u.get("cache_read_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            u.get("cache_creation_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );

        // Identity (D1): key on `message.id`, tracking which `requestId`s have been
        // folded under it. A retried request re-emits the SAME `message.id` under a
        // NEW `requestId` with genuinely new billing, so it must fold again; a
        // replica repeats both and must not.
        //
        // Keying on a concatenated `"id|requestId"` (an earlier shape) split on
        // mixed presence: replicas of one message that disagree about whether
        // `requestId` is present produced two different keys and double-counted.
        // Keying on the id alone and treating `requestId` as a retry-discriminator
        // collapses that case correctly.
        let mid = v.pointer("/message/id").and_then(Value::as_str);
        let rid = v.get("requestId").and_then(Value::as_str);
        let key = match (mid, rid) {
            (Some(m), _) => Some(m.to_string()),
            (None, Some(r)) => Some(r.to_string()),
            // No identity at all: fold as a distinct message rather than dropping
            // it. This degrades to per-line counting (the 2.3x bug), and is
            // unreachable on all observed data (0 of 5,715 usage-bearing lines).
            (None, None) => v.get("uuid").and_then(Value::as_str).map(str::to_string),
        };

        if let Some(k) = &key {
            if let Some(entry) = self.seen_usage.get_mut(k) {
                let unseen_rid = matches!(rid, Some(r) if !entry.1.iter().any(|seen| seen == r));
                let differs = entry.0 != vals;
                // A retry is an unseen `requestId` on a key that ALREADY had one.
                // The "already had one" half is what closes the mixed-presence seam
                // in both directions: when a message's first line carried no
                // `requestId`, a later line's id is the first ever seen for that
                // message, not evidence of a second request — so it is adopted as
                // the key's id rather than treated as a retry. Two genuinely
                // different requests both carry ids, so a real retry is unaffected
                // even when it re-bills an identical amount (R1.2).
                let is_retry = unseen_rid && !entry.1.is_empty();
                if !is_retry {
                    if unseen_rid {
                        push_capped_id(&mut entry.1, rid);
                    }
                    // A replica whose numbers differ means the byte-identical
                    // assumption broke. Count it rather than corrupt the totals.
                    if differs {
                        self.usage_conflicts += 1;
                    }
                    return;
                }
                push_capped_id(&mut entry.1, rid);
                // Rebase onto the retry's numbers, so the retry's OWN replicas
                // compare against it. Without this, every later block-line of a
                // split retry is measured against the first request and reported as
                // a conflict — false-flagging exactly the case this branch admits.
                entry.0 = vals;
            } else {
                let rids = rid.map(|r| vec![r.to_string()]).unwrap_or_default();
                self.seen_usage.insert(k.clone(), (vals, rids));
                self.seen_order.push_back(k.clone());
                while self.seen_order.len() > DEDUPE_CAP {
                    if let Some(old) = self.seen_order.pop_front() {
                        self.seen_usage.remove(&old);
                    }
                }
            }
        }

        let (inp, out, cr, cc) = vals;
        // Saturating throughout: the crate's hard constraint is that NO input can
        // panic, and a hostile or corrupt `output_tokens: u64::MAX` would overflow
        // a plain `+=`. Saturation is the right failure mode here — a pinned total
        // is visibly wrong, an aborted process loses the whole overlay.
        self.total_input = self.total_input.saturating_add(inp);
        self.total_output = self.total_output.saturating_add(out);
        self.total_cache_read = self.total_cache_read.saturating_add(cr);
        self.total_cache_creation = self.total_cache_creation.saturating_add(cc);
        // Occupancy is a LATEST-VALUE gauge, not a sum (D2): summing is meaningless
        // because cache-read is re-billed in full every turn. Set unconditionally —
        // a guard on `> 0` would leave a stale gauge standing when the newest usage
        // genuinely reports zero context.
        let used = inp.saturating_add(cr).saturating_add(cc);
        self.context_used = used;
        // ... but the HIGH-WATER is a monotone session fact, and it is a different
        // question from the gauge. `context_used` answers "how full is the window now";
        // this answers "how big has the window been proven to be". A context window does
        // not shrink mid-session, so evidence of its size cannot expire — whereas the
        // gauge legitimately falls (a `/compact` drops it by an order of magnitude).
        //
        // Without this the app's window deduction is amnesiac: measured on real logs, a
        // `/compact` took one session from 517,389 to 53,268, and the panel re-derived a
        // 200k window from the smaller number — reporting 27% of a window the session had
        // already proven it was not on. Keeping the maximum is what makes the deduction a
        // property of the session rather than of the newest line.
        self.context_high_water = self.context_high_water.max(used);
    }

    // ---- context window (`ai-monitor-context-window`) --------------------------

    /// Fold a `type == "system"` record, which is where a slash command's stdout lands
    /// — and so the only place the context window is ever STATED rather than guessed.
    ///
    /// Two gates, and they are one gate used twice (D1a). `subtype` alone is not enough:
    /// most `local_command` records are not `/context` at all — 16 of 22 on 2026-09-11,
    /// 33 of 39 on 2026-09-12, a proportion that holds while the absolutes climb — and
    /// one of them reads ``Kept model as `Opus 5 (1M context) (default)` ``, a string
    /// from which a permissive read takes `1M` and calls it a window. So the panel's own
    /// title has to be there too. The cost is named rather than hidden: if that title
    /// ever changes, the read and its diagnostics die TOGETHER and the panel falls back
    /// to today's behaviour with nothing to say about it (G-i).
    fn on_system(&mut self, v: &Value) {
        if str_at(v, "subtype") != Some("local_command") {
            return;
        }
        let Some(raw) = str_at(v, "content") else {
            return;
        };
        if !raw.contains(CONTEXT_PANEL_TITLE) {
            return;
        }
        let stripped = strip_sgr(raw);
        let (dedicated, header) = read_window_anchors(&stripped);
        // Two anchors and two counters, each with exactly one trigger. One anchor
        // failing alone is the redundancy WORKING and is counted nowhere.
        if let (Some(a), Some(b)) = (dedicated, header) {
            if a != b {
                self.context_window_anchor_conflicts += 1;
            }
        }
        let Some(tokens) = dedicated.or(header) else {
            self.context_window_parse_failures += 1;
            return;
        };
        // Plain assignment, so a second `/context` replaces the first (D5). The rule
        // follows `model`, not `session_id`: an id is fixed for the life of a file, but
        // a window tracks the model tier and `/model` can move it mid-session, so the
        // newest reading is the true one.
        //
        // The pair is written in ONE statement, which is the whole reason it is one
        // struct: `model_at_read` is read back beside `tokens` when the app decides
        // whether to believe this, and a window carrying the wrong model's name is a
        // reading that looks valid and is not.
        self.context_window_reading = Some(ContextWindowReading {
            tokens,
            model_at_read: self.model.clone(),
        });
    }

    // ---- tool correlation (D5, D7, D8, D9) ------------------------------------

    fn on_tool_use(&mut self, it: &Value, ts: Option<u64>) {
        let Some(name) = it.get("name").and_then(Value::as_str) else {
            return;
        };
        let Some(id) = it
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| it.get("tool_use_id").and_then(Value::as_str))
        else {
            return;
        };
        let kind = classify(name);
        let input = it.get("input");
        let mut p = Pending {
            name: name.to_string(),
            kind: Some(kind),
            start_ms: ts,
            ..Pending::default()
        };
        match kind {
            ToolKind::File(_) => p.path = extract_path(name, input),
            ToolKind::Skill => p.skill = input_str(input, "skill"),
            ToolKind::Agent => {
                p.subagent_type = input_str(input, "subagent_type");
                p.description =
                    input_str(input, "description").or_else(|| input_str(input, "prompt"));
                p.model = input_str(input, "model");
            }
            ToolKind::Task => {
                p.subject = input_str(input, "subject");
                p.task_id = input_str(input, "taskId").or_else(|| input_str(input, "task_id"));
                p.status = input_str(input, "status");
                p.task_description = input_str(input, "description");
            }
            ToolKind::Plain => {}
        }
        if self.digest {
            p.summary = tool_summary(name, input);
            if kind == ToolKind::Agent {
                if let Some(t) = ts {
                    self.push_event(
                        t,
                        EventKind::TaskStart {
                            description: agent_label(&p.description, &p.subagent_type),
                        },
                    );
                }
            }
        }

        // An `Agent` opens a NOT-completed entry in the ring immediately, so an
        // in-flight delegation is visible (`telemetry-model` D9/R6.1). The two
        // lifecycle fields open honest as well: no agent id has been observed yet, and
        // nothing has failed (`agent-elapsed-sidechain` R6.1/R7.1).
        if kind == ToolKind::Agent {
            push_capped(
                &mut self.subagents,
                Subagent {
                    id: id.to_string(),
                    subagent_type: p.subagent_type.clone(),
                    description: p.description.clone(),
                    model: p.model.clone(),
                    start_ms: ts,
                    end_ms: None,
                    completed: false,
                    agent_id: None,
                    ended_ok: true,
                    report_preview: None,
                },
                SUBAGENTS_CAP,
            );
        }

        if self.pending.contains_key(id) {
            // Last-wins: attributing the result to a stale start time would be worse.
            self.duplicate_tool_ids += 1;
            self.pending_order.retain(|k| k != id);
        }
        self.pending.insert(id.to_string(), p);
        self.pending_order.push_back(id.to_string());
        while self.pending_order.len() > PENDING_CAP {
            if let Some(old) = self.pending_order.pop_front() {
                self.pending.remove(&old);
            }
        }
    }

    /// `it` is the `tool_result` content block; `line` is the whole record it arrived
    /// on. They differ in the nested form real logs use, and only `line` carries
    /// `toolUseResult` (D10).
    fn on_tool_result(&mut self, it: &Value, line: &Value, ts: Option<u64>) {
        let Some(tid) = it
            .get("tool_use_id")
            .and_then(Value::as_str)
            .or_else(|| it.get("id").and_then(Value::as_str))
        else {
            return;
        };
        let Some(p) = self.pending.remove(tid) else {
            // Guaranteed, not hypothetical: the watcher starts from a bounded tail,
            // so results whose `tool_use` sits above the window always occur.
            self.orphan_tool_results += 1;
            return;
        };
        self.pending_order.retain(|k| k != tid);

        // Only an explicit `true` is a failure — successes routinely omit the field.
        let ok = !it.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        // The cumulative counters (D3) fold HERE: at the single point `ok` is
        // derived and BEFORE the per-kind branch below. Every completed call is
        // therefore counted exactly once whatever ring it lands in (or none, as an
        // `Agent` does), and the failure count can never disagree with the `ok` the
        // rings carry — a second `is_error` read elsewhere would be a second source
        // of truth for one fact.
        self.tally_call(&p.name, ok);
        let duration_ms = match (p.start_ms, ts) {
            // Clamp a negative interval to None, not zero: zero would render as a
            // real measurement.
            (Some(a), Some(b)) if b >= a => Some(b - a),
            _ => None,
        };
        if self.digest {
            self.digest_tool(&p, ts);
        }

        match p.kind.unwrap_or(ToolKind::Plain) {
            ToolKind::Skill => push_capped(
                &mut self.skills,
                SkillEntry {
                    name: p.skill.clone().unwrap_or_else(|| "(skill)".to_string()),
                    start_ms: p.start_ms,
                },
                SKILLS_CAP,
            ),
            ToolKind::File(op) => push_capped(
                &mut self.files,
                FileActivity {
                    op,
                    path: p.path.clone().unwrap_or_else(|| p.name.clone()),
                    ok,
                    duration_ms,
                    start_ms: p.start_ms,
                },
                FILES_CAP,
            ),
            ToolKind::Agent => {
                // Read from EVERY `Agent` result, not just async launches. A
                // synchronously finished agent can be resumed in the background much
                // later, and without its id captured here that notification has nothing
                // to join against — that case is where most of this feature's reach
                // comes from (D5/R3).
                let agent_id = line
                    .pointer("/toolUseResult/agentId")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                // An async dispatch answers with a RECEIPT, not an answer. Treating it
                // as a completion is the whole defect: the agent runs for minutes while
                // its card reads DONE at 0s (D3/R2).
                let launched = is_async_launch(line, it);
                if let Some(a) = self.subagents.iter_mut().find(|a| a.id == tid) {
                    // Only ever ADD an id, never clear one — a guard for an invariant two
                    // other mechanisms currently enforce, kept because it is the one that
                    // states the intent locally.
                    //
                    // No test pins it, and that is deliberate rather than an omission: the
                    // clearing it prevents is unreachable today. A repeat `tool_result` for
                    // one `tool_use_id` never arrives here at all — `pending.remove(tid)`
                    // consumed the entry, so the second is rejected as an orphan above —
                    // and a duplicate `tool_use` pushes a FRESH ring entry to the front,
                    // which this `find` then matches instead of the one holding the id.
                    // Removing the guard was measured to fail nothing. Should either of
                    // those change, this line is what keeps a captured id from being
                    // erased and the notification join from being severed.
                    //
                    // R3.3 is unaffected: an entry that never saw an id keeps `None`,
                    // because there is nothing to preserve.
                    if agent_id.is_some() {
                        a.agent_id = agent_id;
                    }
                    if !launched {
                        a.completed = true;
                        a.end_ms = ts;
                        // The SAME `ok` the counters above folded. Omitting it here was
                        // the defect: `tally_call` had already charged this call to
                        // `failed_tool_calls` while the card kept the `true` it opened
                        // with, so the panel's own two readings of one result disagreed
                        // — the aggregate said a call failed and the agent it failed on
                        // rendered in the success colour.
                        //
                        // Inside the gate because a receipt reports on the DISPATCH, not
                        // on the run it started — but that placement is a statement of
                        // meaning, not a guard, and no test pins it: hoisting this line
                        // above `if !launched` was measured to change no rendered output.
                        // A receipt carries no `is_error`, so it would write the `true`
                        // the entry already holds; and an errored receipt would leave
                        // `completed: false`, which the accent reads as live whatever
                        // `ended_ok` says. Stated here rather than defended by a test
                        // that would only look like one.
                        a.ended_ok = ok;
                    }
                }
                if self.digest && !launched {
                    self.digest_agent_end(tid, ts, ok, &result_text(it));
                }
            }
            ToolKind::Task => self.on_task_result(&p, it, ts, ok),
            // A `Task` result never reaches the completed-tools ring; everything
            // else does.
            ToolKind::Plain => push_capped(
                &mut self.tools,
                CompletedTool {
                    name: p.name.clone(),
                    ok,
                    duration_ms,
                    start_ms: p.start_ms,
                },
                TOOLS_CAP,
            ),
        }
    }

    /// Read a `queued_command` attachment as a task-notification
    /// (ai-monitor-absorbed-notifications D1/D2).
    ///
    /// Gated on `attachment.type` and on nothing else the record says about itself.
    /// `commandMode` is deliberately NOT read: every observed value is
    /// `task-notification`, so it discriminates nothing today, and a rename upstream
    /// would silently bring back the permanent LIVE ghosts this path exists to end.
    /// A silent format shift is how those ghosts arose in the first place.
    ///
    /// `rendered` / `renderedInHumanTurn` are never read. They carry the same block, but
    /// wrapped inside a `<system-reminder>`, so they would only ever fail the starts-with
    /// gate; `hook_success` and `prompt_snapshot` records quote the tag the same way.
    ///
    /// No outer starts-with test, unlike the bare-string arm. That arm needs one only to
    /// return before `scan_command_name` when the parse fails; nothing here falls through
    /// to prose, so `parse_task_notification`'s own gate is the whole check, and a
    /// second copy could be deleted with no observable change.
    fn on_attachment(&mut self, v: &Value, ts: Option<u64>) {
        if v.pointer("/attachment/type").and_then(Value::as_str) != Some(QUEUED_COMMAND) {
            return;
        }
        if let Some(n) = v
            .pointer("/attachment/prompt")
            .and_then(Value::as_str)
            .and_then(parse_task_notification)
        {
            self.on_task_notification(&n, ts);
            if self.digest {
                self.digest_notification(&n, prompt_raw(v), ts);
            }
        }
    }

    /// End the agent a task-notification names (D4/D6/D11).
    ///
    /// Joined on `task-id` against the agent's OWN id, never on the notification's
    /// `tool-use-id`: that names the call that woke the agent — a `SendMessage`, a
    /// `Bash`, sometimes the original `Agent` — and it is distinct on every
    /// notification, so keying on it makes a re-notified agent unrepresentable.
    fn on_task_notification(&mut self, n: &TaskNotification, ts: Option<u64>) {
        let Some(a) = self
            .subagents
            .iter_mut()
            .find(|a| a.agent_id.as_deref() == Some(n.task_id.as_str()))
        else {
            // Ignored, and deliberately UNCOUNTED (D8). The residue is real but
            // unactionable: background `Bash` tasks, which are not subagents and have
            // no card, and agents already aged out of the ring. A counter would blend
            // those two into one number nobody can act on, and carrying it to the app
            // would widen the versioned contract to say so.
            return;
        };
        // Unconditional and last-wins, even on an entry already marked complete. If
        // both launch witnesses missed, the fallback ended it at ~83ms and this
        // correction is the entire point; if the agent was resumed, this reports
        // cumulative time ALIVE rather than the length of the final leg. `start_ms` is
        // never rewritten — an agent has one start.
        a.completed = true;
        // Guarded, unlike the two writes around it. `ts` is `Option`, and a `None` is
        // the absence of a reading rather than a reading of zero: writing it would leave
        // `completed: true` with no end, which `agent_elapsed` renders as an EMPTY value
        // — and, on an agent that already had a good end, it would erase a true duration
        // in favour of a blank. That is strictly worse than the `0s` this feature exists
        // to remove, so last-wins (D6) applies to values, not to their absence.
        //
        // `completed` and `ended_ok` stay unconditional on purpose: whatever the
        // timestamp, the log did say this agent stopped, and an entry must never be left
        // permanently un-completable by a line that reported its ending.
        if ts.is_some() {
            a.end_ms = ts;
        }
        // Reduced to a boolean here rather than carried as a string: `failed` and
        // `killed` are the only non-`completed` values any log has shown, nothing
        // renders the difference, and a value invented upstream lands on the safe side
        // of `!= "completed"` automatically. Last-wins with `end_ms`, so an agent that
        // failed and was then resumed to success goes back to reading as a success
        // (D13/R7).
        //
        // An ABSENT `<status>` is a third case, and it is not the same as an unknown
        // value. `parse_task_notification` requires only `<task-id>`, so a notification
        // with no status tag parses and arrives here with `None` — and a plain
        // `== Some("completed")` reads that as a failure, painting an agent Rose for the
        // sole reason that the tag could not be read. That inverts D2: a missing field is
        // the absence of a reading, never a reading of "it failed". `is_none_or` keeps
        // the failure side reserved for a status that is present and explicitly not
        // `completed`, which is the same rule `is_error` follows where `ok` is derived.
        //
        // No count is quoted here on purpose. The logs this was measured against are a
        // rolling ~30-day window, so any tally goes stale on its own; the vocabulary,
        // not its frequency, is what this line depends on.
        a.ended_ok = n.status.as_deref().is_none_or(|s| s == "completed");
    }

    /// Task state changes (D9). `TaskCreate.input` carries no id — the assigned id
    /// appears only in the result prose, so create-input and create-result are
    /// paired by `tool_use_id`.
    fn on_task_result(&mut self, p: &Pending, it: &Value, ts: Option<u64>, ok: bool) {
        match p.name.as_str() {
            "TaskCreate" => {
                let content = result_text(it);
                match parse_task_created(&content) {
                    Some((id, subject_from_result)) => {
                        let subject = p.subject.clone().unwrap_or(subject_from_result);
                        self.tasks.insert(
                            id,
                            TaskEntry {
                                subject,
                                status: "active".to_string(),
                                created_ms: ts.or(p.start_ms),
                                updated_ms: ts.or(p.start_ms),
                            },
                        );
                        self.evict_tasks();
                    }
                    // The one format-fragile step: fail soft and make it visible.
                    None => self.unparsed_task_creates += 1,
                }
            }
            "TaskUpdate" => {
                // A failed update must not move a task's status.
                if !ok {
                    return;
                }
                let Some(id) = p.task_id.clone() else {
                    self.orphan_task_updates += 1;
                    return;
                };
                let Some(entry) = self.tasks.get_mut(&id) else {
                    self.orphan_task_updates += 1;
                    return;
                };
                if let Some(s) = &p.status {
                    entry.status = s.clone();
                }
                // `description` is the only text an update carries; it rewords the
                // task. `addBlockedBy` is deliberately ignored — dependency wiring
                // the dashboard does not show.
                if let Some(d) = &p.task_description {
                    entry.subject = d.clone();
                }
                entry.updated_ms = ts.or(entry.updated_ms);
            }
            // TaskOutput is a READ of a task's output — classified Task so it never
            // reaches the tools ring, but it folds nothing.
            _ => {}
        }
    }

    fn evict_tasks(&mut self) {
        while self.tasks.len() > TASKS_CAP {
            // Evict least-recently-UPDATED, not oldest-created: updates cluster
            // late, so oldest-created would drop the most active task.
            let victim = self
                .tasks
                .iter()
                .min_by_key(|(id, t)| (t.updated_ms.unwrap_or(0), (*id).clone()))
                .map(|(id, _)| id.clone());
            match victim {
                Some(id) => {
                    self.tasks.remove(&id);
                }
                None => break,
            }
        }
    }

    // ---- cumulative counters (D3, D12) ----------------------------------------

    /// Fold one completed call into the counters the rings cannot answer for: how
    /// many calls this session made, how many of them failed, and how they split by
    /// name. Saturating throughout, like every other accumulator here — no input
    /// may panic the fold.
    fn tally_call(&mut self, name: &str, ok: bool) {
        self.total_tool_calls = self.total_tool_calls.saturating_add(1);
        if !ok {
            self.failed_tool_calls = self.failed_tool_calls.saturating_add(1);
        }
        if let Some(c) = self.tool_calls.get_mut(name) {
            *c = c.saturating_add(1);
            return;
        }
        // A name the map does not hold. Merge FIRST and insert AFTER, so the
        // arriving name is never among the merge candidates: it would enter at 1
        // and be the smallest entry the instant it existed. That ordering is the
        // whole of D12's anti-starvation rule — swapping the two lines reproduces
        // exactly the bug the rule exists to avoid.
        if self.tool_calls.len() >= TALLY_CAP {
            self.merge_smallest_incumbents();
        }
        self.tool_calls.insert(name.to_string(), 1);
    }

    /// Merge the smallest incumbents into the `other` bucket, in one batch down to
    /// `TALLY_WATERMARK`. The sum is conserved — what leaves the map is added to the
    /// bucket — so the map's sum plus `other` still equals `total_tool_calls`, which
    /// is what lets the app show a per-name breakdown and a total that agree.
    fn merge_smallest_incumbents(&mut self) {
        let excess = self.tool_calls.len().saturating_sub(TALLY_WATERMARK);
        if excess == 0 {
            return;
        }
        // `(count, name)` so the sort is smallest-first with the NAME as the
        // tie-break: a HashMap has no order, so equal counts would otherwise merge
        // a different set on every run of the same input.
        let mut ranked: Vec<(u64, String)> = self
            .tool_calls
            .iter()
            .map(|(k, v)| (*v, k.clone()))
            .collect();
        ranked.sort();
        for (count, name) in ranked.into_iter().take(excess) {
            self.tool_calls.remove(&name);
            self.tool_calls_other = self.tool_calls_other.saturating_add(count);
        }
    }

    /// A `<command-name>/foo</command-name>` wrapper is a slash-command invocation.
    /// No dedupe window against the `Skill` tool_use: the two signals come from
    /// different actors (a human typing `/foo` vs the model invoking a skill), so
    /// one invocation produces one signal.
    fn scan_command_name(&mut self, s: &str, ts: Option<u64>) {
        let Some(raw) = between(s, "<command-name>", "</command-name>") else {
            return;
        };
        let name = raw.trim().trim_start_matches('/').trim();
        if name.is_empty() {
            return;
        }
        push_capped(
            &mut self.skills,
            SkillEntry {
                name: name.to_string(),
                start_ms: ts,
            },
            SKILLS_CAP,
        );
    }
}

// ---- session digest (`session-overview-ui` D11, D13, D19) ------------------------
//
// Reached only when `digest` is set, so an ai-monitor accumulator does none of this.
impl SessionTelemetry {
    /// Advance the turn machine by one Claude record (D19).
    ///
    /// The turn boundaries are `activity`'s G8 predicates, not a restatement of them: a
    /// human prompt opens a turn and forgets older background work, the start records
    /// add work, the completion records clear it, and the turn ends at the first point
    /// where the assistant has ended its turn with nothing pending. A completion of work
    /// this turn started RESUMES the turn — the assistant answers the notification — so
    /// it then needs a fresh `end_turn`.
    fn digest_turn(&mut self, v: &Value, ts: Option<u64>) {
        if activity::is_human_prompt(v) {
            self.turn_bg.clear();
            self.turn_ended = false;
            let Some(t) = ts else {
                return;
            };
            self.last_prompt_ms = self.last_prompt_ms.max(Some(t));
            // A prompt typed while the previous turn never ended (an interrupt, or work
            // that never reported) closes it here: one turn per prompt, never two open.
            // No `TurnFinished`: the assistant did not finish it.
            if let Some(open) = self.turns.front_mut().filter(|t| t.end_ms.is_none()) {
                open.end_ms = Some(t.max(open.start_ms));
            }
            push_capped(
                &mut self.turns,
                Turn {
                    start_ms: t,
                    end_ms: None,
                },
                TURNS_CAP,
            );
            return;
        }
        if let Some(id) = activity::completed_id(v) {
            if self.turn_bg.remove(id) {
                self.turn_ended = false;
            }
        }
        for id in activity::started_ids(v) {
            if self.turn_bg.len() < TURN_BG_CAP {
                self.turn_bg.insert(id.to_string());
            }
        }
        if str_at(v, "type") == Some("assistant") && activity::is_end_turn(v) {
            self.turn_ended = true;
        }
        if !self.turn_ended || !self.turn_bg.is_empty() {
            return;
        }
        let fallback = self.last_event_ms;
        if let Some(open) = self.turns.front_mut().filter(|t| t.end_ms.is_none()) {
            let end = ts.or(fallback).unwrap_or(open.start_ms).max(open.start_ms);
            open.end_ms = Some(end);
            self.push_event(end, EventKind::TurnFinished);
        }
    }

    /// `last_tool`, `last_work_tool` and the `Tool` event for one finished call (D11).
    fn digest_tool(&mut self, p: &Pending, ts: Option<u64>) {
        let entry: ToolDigest = (p.name.clone(), p.summary.clone(), p.start_ms);
        if !is_agent_tool(&p.name) {
            self.last_work_tool = Some(entry.clone());
            if let Some(t) = ts.or(p.start_ms) {
                self.push_event(
                    t,
                    EventKind::Tool {
                        name: p.name.clone(),
                        summary: p.summary.clone(),
                    },
                );
            }
        }
        self.last_tool = Some(entry);
    }

    /// A synchronous `Agent` answer: its report preview and its end event.
    fn digest_agent_end(&mut self, tool_use_id: &str, ts: Option<u64>, ok: bool, text: &str) {
        let Some(a) = self.subagents.iter_mut().find(|a| a.id == tool_use_id) else {
            return;
        };
        if let Some(preview) = report_preview(text) {
            a.report_preview = Some(preview);
        }
        let description = agent_label(&a.description, &a.subagent_type);
        if let Some(t) = ts {
            self.push_event(t, end_event(ok, description));
        }
    }

    /// A background agent's task notification: the `<result>` preview and the end event.
    /// Joined exactly as [`on_task_notification`](Self::on_task_notification) joins.
    fn digest_notification(&mut self, n: &TaskNotification, raw: Option<&str>, ts: Option<u64>) {
        let Some(a) = self
            .subagents
            .iter_mut()
            .find(|a| a.agent_id.as_deref() == Some(n.task_id.as_str()))
        else {
            return;
        };
        if let Some(preview) = raw
            .and_then(|r| between(r, "<result>", "</result>"))
            .and_then(report_preview)
        {
            a.report_preview = Some(preview);
        }
        let ok = n.status.as_deref().is_none_or(|s| s == "completed");
        let description = agent_label(&a.description, &a.subagent_type);
        if let Some(t) = ts {
            self.push_event(t, end_event(ok, description));
        }
    }

    fn push_event(&mut self, at_ms: u64, kind: EventKind) {
        push_capped(&mut self.events, Event { at_ms, kind }, EVENTS_CAP);
    }
}

fn end_event(ok: bool, description: String) -> EventKind {
    if ok {
        EventKind::TaskDone { description }
    } else {
        EventKind::TaskFailed { description }
    }
}

/// `Agent`, and `Task` (its earlier name) — the calls D4 does not count as work.
fn is_agent_tool(name: &str) -> bool {
    matches!(name, "Agent" | "Task")
}

/// What an event calls a subagent: its description, else its type, else `subagent`.
fn agent_label(description: &Option<String>, subagent_type: &Option<String>) -> String {
    description
        .as_deref()
        .or(subagent_type.as_deref())
        .unwrap_or("subagent")
        .to_string()
}

/// D11's one-line summary of a tool call's input, or `None` for a tool it does not name.
fn tool_summary(name: &str, input: Option<&Value>) -> Option<String> {
    let field = |k: &str| input.and_then(|i| i.get(k)).and_then(Value::as_str);
    let raw = match name {
        "Bash" => field("command")?.to_string(),
        "Read" | "Edit" | "Write" | "NotebookEdit" => {
            let path = field("file_path").or_else(|| field("notebook_path"))?;
            basename(path).to_string()
        }
        "Grep" | "Glob" => field("pattern")?.to_string(),
        "Agent" | "Task" => field("description")?.to_string(),
        "WebFetch" => url_host(field("url")?)?.to_string(),
        "WebSearch" => field("query")?.to_string(),
        "Skill" => field("skill")?.to_string(),
        _ => return None,
    };
    one_line(&raw)
}

/// The first non-blank line, whitespace collapsed, at most [`SUMMARY_CHARS`] chars.
fn one_line(s: &str) -> Option<String> {
    let line = s.lines().find(|l| !l.trim().is_empty())?;
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(truncate_chars(&collapsed, SUMMARY_CHARS))
}

/// The last path component; a trailing `/` is ignored.
fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// `https://user@docs.rs:443/x?y` -> `docs.rs`. `None` when nothing host-like remains.
fn url_host(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host.split(':').next()?;
    (!host.is_empty()).then_some(host)
}

/// The first [`PREVIEW_LINES`] non-blank lines of a report, trimmed and joined with
/// `\n`, at most [`PREVIEW_CHARS`] chars. `None` for a report with no text.
fn report_preview(text: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(PREVIEW_LINES)
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(truncate_chars(&lines.join("\n"), PREVIEW_CHARS))
}

/// `s` itself when it has at most `max` chars, else its first `max - 1` and `…`.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The raw `prompt` of a `queued_command` attachment.
fn prompt_raw(v: &Value) -> Option<&str> {
    v.pointer("/attachment/prompt").and_then(Value::as_str)
}

// ---- helpers ------------------------------------------------------------------

/// The Antigravity step types that report a completed tool call — the image of
/// [`ag_tool`]'s middle column. A step type outside this set that also claims no
/// pending call is simply a step that folds nothing (`CHECKPOINT`,
/// `CONVERSATION_HISTORY`, `SYSTEM_MESSAGE`, `ERROR_MESSAGE`), which is why the orphan
/// counter is gated on membership: counting those would make `orphan_tool_results`
/// read as a parser fault on every well-formed transcript.
///
/// Also the closed set the sidebar's tail reader classifies as a tool result
/// (`agent-card-status` D1, `antigravity::tail_state`) — one list, so the panel and the
/// card cannot disagree about which steps are results.
pub(crate) const AG_RESULT_STEPS: &[&str] = &[
    "VIEW_FILE",
    "CODE_ACTION",
    "GREP_SEARCH",
    "LIST_DIRECTORY",
    "FIND",
    "RUN_COMMAND",
    "INVOKE_SUBAGENT",
    "GENERIC",
    "ASK_QUESTION",
];

/// Unwrap one Antigravity `args` value (R2.3).
///
/// Every value in that map is a `String` whose CONTENTS are a JSON document — measured,
/// 3,470 of 3,470 values are strings, and 2,757 of those wrap a quoted string. So a
/// path is reached by parsing the value's contents once; reading the value directly
/// renders `"/a/b.rs"` with its literal quote characters intact, which is what a naive
/// read produces on every path the panel shows.
///
/// `None` when the key is absent, when the value is not a string, or when its contents
/// are not a JSON *string*. That last case is not an error condition in general — it is
/// how the format spells a non-string arg (`EndLine` is `"100"`, `Overwrite` is
/// `"true"`, `questions` is `"[{…}]"`) — but no path, summary or name is ever shaped
/// that way, so for the keys this fold unwraps it is a genuine fault. It is also how a
/// truncated arg reads: `transcript.jsonl` cuts long values and appends a
/// `<truncated N bytes>` marker, breaking the inner document (42 of 360 `CommandLine`
/// values in the corpus). None of the keys unwrapped here were ever among them.
fn ag_arg_str(args: Option<&Value>, key: &str) -> Option<String> {
    let raw = args.and_then(|a| a.get(key)).and_then(Value::as_str)?;
    serde_json::from_str::<String>(raw).ok()
}

/// The child conversation id an `INVOKE_SUBAGENT` step names in its `content` (AG5).
///
/// Scanned, not parsed: the body is pretty-printed JSON embedded in prose
/// (`Created the following subagents:\n{ "conversationId": "…", "logAbsoluteUri": … }`),
/// so the whole field is not a JSON document and `serde_json` has nothing to take.
/// The background-task announcement inside a `RUN_COMMAND` result (G5, closed).
///
/// `run_command` with a `WaitMsBeforeAsync` that expires does not return output — it returns
/// this, and it is the ONLY place a task's description is ever written:
///
/// ```text
/// Tool is running as a background task with task id: <conv>/task-39
/// Task Description: cargo test --workspace
/// ```
///
/// Returns `(id, description)`. The id keeps its `<conv>/` prefix exactly as
/// `manage_task`'s `TaskId` spells it, so the two sides key the same map without either
/// having to normalise.
fn ag_task_created(content: &str) -> Option<(String, String)> {
    let id = ag_line_after(content, "background task with task id:")?;
    let desc = ag_line_after(content, "Task Description:")?;
    Some((id, desc))
}

/// A `manage_task` status report inside a `GENERIC` result (G5, closed).
///
/// ```text
/// Task: <conv>/task-39
/// Status: RUNNING
/// ```
///
/// Returns `(id, status)` with the status translated into the vocabulary the TASKS card
/// already colours — `RUNNING` -> `in_progress` (amber), `DONE` -> `completed` (green).
/// Translating here rather than in the app keeps the mapping beside the format it comes
/// from; an unrecognised status passes through verbatim and renders faint, which is the
/// honest rendering of a state this build does not know.
fn ag_task_status(content: &str) -> Option<(String, String)> {
    let id = ag_line_after(content, "Task:")?;
    let raw = ag_line_after(content, "Status:")?;
    let status = match raw.as_str() {
        "RUNNING" => "in_progress".to_string(),
        "DONE" => "completed".to_string(),
        other => other.to_ascii_lowercase(),
    };
    Some((id, status))
}

/// The remainder of the first line containing `key`, trimmed.
///
/// Line-scoped deliberately: these bodies also carry a `Log:` path and a `YOU MUST` banner,
/// and a scan that ran past the newline would swallow them.
fn ag_line_after(content: &str, key: &str) -> Option<String> {
    let v = content
        .lines()
        .find_map(|l| l.split_once(key).map(|(_, rest)| rest.trim()))?;
    (!v.is_empty()).then(|| v.to_string())
}

/// The result type a step actually REPORTS, which since CLI 1.1.15 is not always the type it
/// is labelled with.
///
/// `run_command` results arrived as `RUN_COMMAND` steps carrying an `exit_code` key for the
/// whole measured corpus — 352 of them — until the CLI updated itself mid-session on
/// 2026-08-18 (binary mtime 23:55, session `e73dca15` at 23:55:32). Under 1.1.15 the same
/// results arrive typed `GENERIC` with no `exit_code`, and the exit status only in prose.
/// `GENERIC` is the type four *other* tools report through, so the pairing found no pending
/// call expecting it and orphaned both calls: the panel showed a session that had run two
/// commands as having made no tool calls at all.
///
/// The discriminator is the result body, not the version, so this holds whichever shape the
/// CLI emits — and it is measured, not guessed: the marker below appears in **all 352**
/// `RUN_COMMAND` results and in **none** of the 68 `GENERIC` steps belonging to `manage_task`,
/// `send_message`, `define_subagent`, `manage_subagents` or `schedule`. Zero false positives
/// on the corpus, which is what makes this safe to apply before the pairing rather than as a
/// fallback after it: a `manage_task` result cannot reach this branch.
///
/// Strictly a widening. A step this does not rewrite pairs exactly as it did before, so the
/// only calls it can newly claim are ones that were being dropped.
fn ag_reported_type<'a>(step_type: &'a str, content: Option<&str>) -> &'a str {
    const COMMAND_RESULT_LEGACY: &str = "The command exited with code";
    const COMMAND_RESULT_147: &str = "Command exited with code";
    const COMMAND_RESULT_ASYNC: &str = "Tool is running as a background task";
    const SUBAGENT_HEADER_PLURAL: &str = "Created the following subagents:";
    const SUBAGENT_HEADER_SINGULAR: &str = "Created the following subagent:";

    if step_type == "GENERIC" {
        if let Some(c) = content {
            if c.contains(COMMAND_RESULT_147)
                || c.contains(COMMAND_RESULT_LEGACY)
                || c.contains(COMMAND_RESULT_ASYNC)
            {
                return "RUN_COMMAND";
            }
            if c.contains(SUBAGENT_HEADER_SINGULAR) || c.contains(SUBAGENT_HEADER_PLURAL) {
                return "INVOKE_SUBAGENT";
            }
        }
    }
    step_type
}

fn ag_completed_senders(content: &str) -> Vec<&str> {
    const NOTICE_HEADER: &str = "[Message] timestamp=";
    content
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix(NOTICE_HEADER)?;
            rest.split_whitespace()
                .find_map(|kv| kv.strip_prefix("sender="))
        })
        .collect()
}

fn ag_conversation_id(content: &str) -> Option<String> {
    let after = content.split("\"conversationId\"").nth(1)?;
    let after = after.trim_start().strip_prefix(':')?;
    let id = between(after, "\"", "\"")?;
    (!id.is_empty()).then(|| id.to_string())
}

/// The skill named by a `<SKILL>` block injected into a `USER_INPUT` (G10).
///
/// Antigravity does not invoke a skill through a tool — it splices the skill's text into the
/// user's turn, prefixed with a sentence that names it:
///
/// ```text
/// <SKILL>The user has explicitly invoked the (worktree) skill. You must strictly follow ...
/// ```
///
/// So the name is the parenthesised word in that sentence, and it is the ONLY place the
/// invocation is recorded — the body that follows is the skill's whole markdown, which is
/// why this stops at the name rather than carrying any of it.
///
/// One skill per turn: the marker is emitted for an explicit `/skill` invocation, and a turn
/// has one. A malformed block (marker present, no parenthesised name) yields `None` and folds
/// nothing rather than inventing an unnamed row.
fn scrape_skill_invocation(content: &str) -> Option<String> {
    const MARKER: &str = "<SKILL>";
    let rest = content.split_once(MARKER)?.1;
    // Bounded to the sentence that introduces the skill, so a parenthesis anywhere in the
    // skill's own markdown body cannot be mistaken for its name.
    let head = rest.get(..rest.find("skill.").map(|i| i + 6).unwrap_or(0))?;
    let name = head.split_once('(')?.1.split_once(')')?.0.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The model named by a `Model Selection` settings change (AG4/R3.2).
///
/// Antigravity records no `model` key anywhere; the only statement it makes about the
/// running model is a sentence inside a `USER_INPUT`'s `<USER_SETTINGS_CHANGE>`
/// envelope: ``The user changed setting `Model Selection` from None to Gemini 3.6 Flash
/// (High). No need to comment…``. This is prose-scraping in a crate that otherwise
/// reads typed fields, and it is the only source there is.
///
/// The effort tier stays (`(High)` is what the user selected and what the CLI reports
/// about itself); the sentence's terminating period does not. Finding that period is
/// the whole subtlety: the VALUE contains periods, so cutting at the first `.` yields
/// `Gemini 3`. The terminator is the first `.` followed by whitespace or nothing —
/// version dots are always followed by a digit.
///
/// `None` when the sentence is absent, which is the common case (21 of 54 measured
/// `USER_INPUT` steps carry no settings block at all). The field then stays `None`, and
/// the panel renders its existing unknown-model form rather than a guessed default
/// (R3.4).
fn scrape_model_selection(content: &str) -> Option<String> {
    const MARKER: &str = "changed setting `Model Selection` from ";
    let after = content.split(MARKER).nth(1)?;
    // `<old>` first, then `<new>`: the observed old values (`None`, a previous model
    // name) never contain " to ", so the first separator is the right one.
    let line = after.split(" to ").nth(1)?.split('\n').next()?;
    let mut end = line.len();
    for (i, c) in line.char_indices() {
        if c == '.' && line[i + 1..].chars().next().is_none_or(char::is_whitespace) {
            end = i;
            break;
        }
    }
    let value = line.get(..end)?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

// ---- context window: reading `/context` stdout (spec ai-monitor-context-window) ----
//
// The window is the DENOMINATOR of the CONTEXT tile, and it appears in exactly one
// place in a log: the stdout of the `/context` command. No field anywhere carries it —
// `dashboard.rs` already records that every `[1m]` in a real log is prose inside a
// message, never a field, and a sweep for a numeric `200000|700000|1000000` field over
// the whole corpus returns nothing. So the app has been GUESSING it from the model id,
// and guessed wrong: a session at 74,677 used showed 11% under `/context` (74.7k/700k)
// and 37% on the panel (74.7k/200k). The numerator already agrees to the token; only
// the denominator is wrong. These helpers read the real one.

/// Smallest value that may be believed as a context window (D7).
///
/// The header anchor's neighbourhood contains a percentage (`74.7k/700k tokens (11%)`),
/// so a loose match could pick `11` as the denominator. Real windows are >= 1,000 and
/// percentages are 0-100, which this cuts cleanly between. The number itself is
/// assumed, not measured — no observed window comes anywhere near it.
const WINDOW_FLOOR: u64 = 1_000;

/// Strip CSI-SGR sequences (`ESC [ … m`) from `s`, keeping every other byte.
///
/// Written here rather than pulled in: the workspace has no ANSI stripper, and
/// `ai-monitor` depends on `serde_json` + `notify` and nothing else. One screenful of
/// code does not justify a third.
///
/// **SGR only, and that is measured, not assumed.** Across the `local_command` stdout
/// of every `/context` record in the corpus there are 572 escape sequences and the
/// final byte is `m` on all 572 — no OSC, no cursor motion. A CSI that is not SGR is
/// therefore left VERBATIM: stripping it would be unreachable code dressed as
/// robustness, and the name of this function would stop being true.
///
/// **Not written against one sample.** Two SGR encodings coexist in the corpus —
/// truecolor `ESC[38;2;153;153;153m` in most logs and 256-colour `ESC[38;5;246m` in
/// one — so a `38;2`-shaped matcher would silently read half the logs. This matches the
/// CSI shape instead: `ESC [`, parameter bytes, final byte `m`.
///
/// **Driven by the escapes, not by a cursor.** An earlier version walked the string
/// with a hand-advanced index; mutation testing reported five mutants that turned an
/// increment into a no-op and HUNG the suite, plus two that made a scan of bytes no
/// input ever contains unobservable. Both classes were properties of the cursor rather
/// than of the job: `match_indices` supplies the only positions that matter, and the
/// parameter run is COUNTED rather than walked, so there is no index left to advance
/// wrongly. ECMA-48's intermediate bytes (`0x20..=0x2f`) are not scanned at all —
/// nothing in the corpus has one, an SGR with an intermediate byte is not SGR by that
/// standard anyway, and code that only a fabricated input can reach is the
/// robustness-shaped unreachable branch the paragraph above refuses.
fn strip_sgr(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    // Start of the run not yet copied. An `ESC` can never fall inside a sequence
    // already removed — neither a parameter byte nor `m` can be 0x1b — so this only
    // ever moves forward, and every slice below is `copied <= start` with both ends on
    // an ASCII byte, hence on a char boundary.
    let mut copied = 0usize;
    for (start, _) in s.match_indices('\u{1b}') {
        if bytes.get(start + 1) != Some(&b'[') {
            continue;
        }
        let params = s[start + 2..]
            .bytes()
            .take_while(|b| (0x30..=0x3f).contains(b))
            .count();
        let end = start + 2 + params;
        if bytes.get(end) != Some(&b'm') {
            continue;
        }
        out.push_str(&s[copied..start]);
        copied = end + 1;
    }
    out.push_str(&s[copied..]);
    out
}

/// Parse one human-facing token count — `700k`, `74.7k`, `1.0M`, `700000`.
///
/// The whole of `s` (trimmed) must be the number; a caller hands in an exact slice it
/// has already anchored, so trailing prose is a parse failure rather than something to
/// skip over. The bare-integer case is not an invented notation — it is the natural
/// boundary of the same grammar `<digits>[.<digits>][k|K|m|M]`.
///
/// Integer arithmetic throughout, and `checked_*` rather than saturating: a value that
/// overflowed to `u64::MAX` would sail past [`WINDOW_FLOOR`] and become a denominator,
/// which is the one failure mode worse than reading nothing.
fn parse_token_count(s: &str) -> Option<u64> {
    let s = s.trim();
    let (digits, mult) = match s.as_bytes().last() {
        Some(b'k' | b'K') => (&s[..s.len() - 1], 1_000u64),
        Some(b'm' | b'M') => (&s[..s.len() - 1], 1_000_000u64),
        _ => (s, 1u64),
    };
    // `split_once` reports a trailing `.` as an EMPTY fractional part, and "every byte
    // of nothing is a digit" is vacuously true — so `700.k` would parse as 700,000
    // unless the optionality is spelt out here. The grammar's `[.<digits>]` means the
    // dot and at least one digit together, or neither.
    let (int_part, frac_part) = match digits.split_once('.') {
        Some((_, "")) => return None,
        Some((a, b)) => (a, b),
        None => (digits, ""),
    };
    if int_part.is_empty() || !int_part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !frac_part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // `74.7` * 1_000 is computed as (747 * 1_000 + 5) / 10 — one scaled integer, then a
    // round-half-up divide. No floats, and `74.7k` lands on exactly 74,700.
    let scale = 10u64.checked_pow(u32::try_from(frac_part.len()).ok()?)?;
    let int_val: u64 = int_part.parse().ok()?;
    let frac_val: u64 = if frac_part.is_empty() {
        0
    } else {
        frac_part.parse().ok()?
    };
    let scaled = int_val.checked_mul(scale)?.checked_add(frac_val)?;
    scaled
        .checked_mul(mult)?
        .checked_add(scale / 2)?
        .checked_div(scale)
}

/// A token count that is allowed to be believed as a window: the grammar, then the
/// floor (D7). Both failures collapse to `None` on purpose — the caller counts them
/// under one diagnostic because both mean the same thing, "this anchor yielded nothing
/// usable".
fn parse_window_tokens(s: &str) -> Option<u64> {
    parse_token_count(s).filter(|n| *n >= WINDOW_FLOOR)
}

/// The panel title every `/context` render opens with, and the gate both the read and
/// its diagnostics hang on (D1a).
///
/// Checked against the RAW content, before [`strip_sgr`] — which is not merely an
/// optimisation for the records it rejects. The two orderings are distinguishable: a
/// title interrupted by an SGR sequence would pass a post-strip gate and fail this one,
/// and passing it is the wrong answer, since this string is what says "the panel
/// rendered", not "something mentions context".
const CONTEXT_PANEL_TITLE: &str = "Context Usage";

/// Label of the authoritative anchor — a line of its own carrying nothing else.
const WINDOW_LABEL: &str = "Auto-compact window:";

/// Both anchors of a `/context` stdout, as `(dedicated line, header denominator)`.
///
/// Two, because the caller wants drift tolerance out of a redundancy that ACTUALLY
/// EXISTS in the source rather than out of guesses about notations nobody has seen:
/// the same window is printed twice per render, once labelled and once as the header's
/// denominator. Either may survive the other's format changing.
///
/// Takes text that has already been through [`strip_sgr`]; both anchors have SGR
/// sequences sitting inside them in the raw bytes — the label and its digits are
/// separated by two of them, which is why the obvious regex over raw content fails.
fn read_window_anchors(stripped: &str) -> (Option<u64>, Option<u64>) {
    (dedicated_anchor(stripped), header_anchor(stripped))
}

/// `Auto-compact window: 700k tokens` — the labelled line.
fn dedicated_anchor(stripped: &str) -> Option<u64> {
    let after = stripped.split(WINDOW_LABEL).nth(1)?;
    // The value is the first word after the label. Taking a word rather than the rest
    // of the line is what makes the trailing ` tokens` a non-event.
    parse_window_tokens(after.split_whitespace().next()?)
}

/// `74.7k/700k tokens (11%)` — the header's DENOMINATOR.
///
/// Pinned as a grammar and not as an example, because an example cannot say what it
/// excludes. The slash and the literal ` tokens (` are both required, and together they
/// rule out the two things in the same stdout that a looser match would swallow: the
/// per-category lines (`System prompt: 4.7k tokens (0.7%)` — no slash) and the model
/// banner (`Opus 5 (1M context)` — neither), the second of which would otherwise hand
/// back 1,000,000 as a window.
fn header_anchor(stripped: &str) -> Option<u64> {
    const TAIL: &str = " tokens (";
    stripped
        .match_indices(TAIL)
        .find_map(|(idx, _)| header_denominator(stripped.get(..idx)?))
}

/// The `<num>/<denom>` that must sit immediately left of a ` tokens (`.
///
/// **The numerator check buys none of the exclusions named above** — measure before
/// crediting it, as two earlier versions of this comment did not. The category lines and
/// the model banner are refused by the slash and the literal ` tokens (`; the
/// `.claude/agents 534 tokens (…)` case is refused by the DENOMINATOR parse, one
/// statement earlier, because `agents 534` is not a number. What the check actually buys
/// is the narrower case where a path's right-hand component IS one: `logs/700000 tokens (`
/// reads as a 700,000-token window without it. Both halves are pinned by the test named
/// `the_numerator_check_buys_one_case_and_not_the_others`.
///
/// **It is also the quietest way to lose this anchor**, and that is the real cost: the
/// header now depends on the NUMERATOR's notation too. A numerator
/// this grammar does not know (a thousands separator, `217,737`; some future unit)
/// returns `None` here while the labelled line still reads fine, so D2's redundancy
/// silently halves and D10's "one anchor failing counts nothing" rule fires no counter,
/// by design. That is the right trade while the labelled line is healthy and the wrong
/// one the day it is not, and no session in the corpus can currently detect the
/// difference — every observed numerator is `<digits>[.<digits>]k`. If the two anchors
/// ever need to fail independently *and* visibly, this is the line that has to change.
fn header_denominator(before: &str) -> Option<u64> {
    let (numerator, denominator) = before.rsplit_once('/')?;
    let window = parse_window_tokens(denominator)?;
    // The numerator is only ever checked, never used: its job is to prove the slash was
    // a `used/total` separator and not, say, a path. The line begins with block glyphs,
    // so "the word before the slash" — not "everything before it" — is the numerator.
    let used = numerator.rsplit(char::is_whitespace).next()?;
    parse_token_count(used).map(|_| window)
}

/// Whether a `message.model` value names a model that actually ran.
///
/// `<synthetic>` is a REAL value of that field, not a hypothetical: Claude Code writes
/// it on records it fabricates locally — API-error and session-limit notices, which is
/// why they land at the TAIL of a log (52 across the measured corpus, against 37,450
/// genuine ids). It is a sentinel, not a model, and under the last-wins rule it
/// would blank the panel's model the moment one landed at the tail of a log.
///
/// Matched by SHAPE, not by the one literal: `<...>` is how this log marks "not a real
/// value", so a future `<none>`/`<unknown>` must be refused on the same grounds. A
/// genuine id is a bare slug (`claude-opus-5`) and can never take this form, so the
/// shape test cannot reject one. Empty is refused for the same reason — it names
/// nothing, and `Some("")` on the panel is indistinguishable from a bug.
///
/// This is a mechanism, not presentation (SD4): the crate must decide what to fold, and
/// "is this a model id" is a property of the datum. Which of the folded ids to SHOW,
/// and how, stays the app's.
fn is_real_model(m: &str) -> bool {
    let sentinel = m.is_empty() || (m.starts_with('<') && m.ends_with('>'));
    !sentinel
}

/// Record a `requestId` against a message key, bounded (D6): the dedupe map caps the
/// number of KEYS, this caps each key's value, so neither dimension grows without limit.
fn push_capped_id(ids: &mut Vec<String>, rid: Option<&str>) {
    if let Some(r) = rid {
        ids.push(r.to_string());
        while ids.len() > RETRY_IDS_CAP {
            ids.remove(0);
        }
    }
}

fn push_capped<T>(ring: &mut VecDeque<T>, item: T, cap: usize) {
    ring.push_front(item);
    while ring.len() > cap {
        ring.pop_back();
    }
}

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn input_str(input: Option<&Value>, key: &str) -> Option<String> {
    input
        .and_then(|i| i.get(key))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The file path a file-tool acted on. `Grep`/`Glob` carry a *pattern* here, not a
/// path — the app must not basename-shorten it.
fn extract_path(tool: &str, input: Option<&Value>) -> Option<String> {
    for key in [
        "file_path",
        "path",
        "filePath",
        "pattern",
        "directory",
        "dir",
    ] {
        if let Some(s) = input_str(input, key) {
            return Some(s);
        }
    }
    Some(tool.to_string())
}

/// The prose an async dispatch answers with. The fallback witness in `is_async_launch`.
const ASYNC_LAUNCH_PROSE: &str = "Async agent launched successfully.";

/// `toolUseResult.status` on a launch receipt — the same-object backstop for `isAsync`.
const ASYNC_LAUNCH_STATUS: &str = "async_launched";

/// Whether an `Agent` result is a launch RECEIPT rather than an answer (D1).
///
/// Three readings in TWO failure domains, consulted in order of authority rather than
/// OR-ed together. When `toolUseResult` exists it decides alone — first on `isAsync`, then
/// on `status` as a same-object backstop; the prose is read only when the object is absent
/// entirely.
///
/// The two domains are what matters: `toolUseResult` is an undocumented on-disk detail of
/// another program, and the prose drifts with wording. `status` does NOT add a domain —
/// it shares an object with `isAsync` and dies in the same edit, which is why D1 refused it
/// as the second *witness*. It is here for the narrower job of surviving a rename of one
/// key while the object lives, a drift that would otherwise take detection to zero.
///
/// The key is tested for PRESENCE-and-true, never compared against `false`: no observed
/// log carries `isAsync: false`. A synchronous completion simply omits the field, so a
/// `== false` test would classify every one of them as a launch by accident of absence.
///
/// **Why the ordering, and what it costs.** OR-ing the two let a genuine synchronous answer
/// that merely OPENED with the receipt sentence be read as a launch, and since no
/// notification describes a sync completion, that agent would never complete — a permanent
/// LIVE ghost, which the scoping paragraph below calls the failure D2 exists to refuse.
/// Measured 2026-08-24: every result the prose alone identifies carries no `toolUseResult`,
/// and every genuine sync answer carries one — so the gate loses no detection and makes the
/// false positive unreachable rather than merely rare. The cost is named: if upstream renames
/// `isAsync` while still writing `toolUseResult`, the prose no longer rescues those results
/// and the panel degrades to the original defect. That is the direction D2 chose. (As-of
/// readings of a rolling ~30-day window; the structural split, not the counts, is the claim.)
///
/// Call this ONLY from the `ToolKind::Agent` arm. The scoping is load-bearing rather
/// than tidy: a shell command that greps for the prose echoes it back into the log as a
/// `Bash` result, and a prose false positive on a real agent would strand it as a
/// permanent LIVE ghost — the failure the conservative fallback (D2) exists to refuse.
fn is_async_launch(line: &Value, it: &Value) -> bool {
    match line.get("toolUseResult") {
        // A structured object is present, so it is the authority: it either says
        // `isAsync: true` or this is not a launch. The prose is not consulted, which is
        // what makes a false positive unreachable rather than merely unlikely.
        //
        // An explicit `null` is deliberately NOT authority. `line.get` returns
        // `Some(Value::Null)` for it, which would otherwise silence the prose on the
        // strength of a field carrying no information — the same mistake as reading an
        // absent `<status>` as a failure.
        Some(tur) if !tur.is_null() => {
            tur.get("isAsync").and_then(Value::as_bool).unwrap_or(false)
                // A second reading INSIDE the object: the one that survives a rename of
                // the key above. Measured 2026-08-25 — it marks 63 of 63 launches and 0
                // of 113 synchronous answers, the same separation `isAsync` gives, so an
                // `isAsync` rename no longer drops detection to nothing.
                //
                // D1 rejected this field as the *second witness* because it shares an
                // object with the first and dies in the same edit. That reasoning stands,
                // and is exactly why it does not replace the prose: it is a same-object
                // BACKSTOP, while the prose remains the independent domain covering the
                // object disappearing entirely.
                || tur.get("status").and_then(Value::as_str) == Some(ASYNC_LAUNCH_STATUS)
        }
        // No structured object — absent, or present-but-null. The only case where the
        // prose has to carry the decision alone, and the only case where it can.
        _ => result_text_starts_with(it, ASYNC_LAUNCH_PROSE),
    }
}

/// Whether [`result_text`] would begin with `prefix`, without building the whole string.
///
/// Intended to be exactly equivalent to `result_text(it).trim_start().starts_with(prefix)`,
/// including the single-space join between `text` blocks.
///
/// **That equivalence is argued, not pinned.** `result_text` is private and this crate keeps
/// its tests in `tests/`, so nothing compares the two functions directly; the coverage that
/// exists (`the_prose_witness_reads_array_content_as_a_single_space_joined_whole`) drives them
/// through the fold and pins the join separator, not the whole equivalence. Said here rather
/// than claimed away, because "equivalent" is the entire justification for having both.
///
/// It exists because the string it avoids building can be enormous: a synchronous agent's
/// `tool_result` content is its complete final report, tens of kilobytes, and the old form
/// cloned all of it on every `Agent` result to inspect ~33 leading bytes.
fn result_text_starts_with(it: &Value, prefix: &str) -> bool {
    // Accumulate only far enough to decide. Once the trimmed head is at least as long as
    // the prefix, no later block can change the bytes being compared — while the head is
    // still all whitespace it can, which is why the test is on the TRIMMED length.
    let mut head = String::new();
    match it.get("content") {
        Some(Value::String(s)) => return s.trim_start().starts_with(prefix),
        Some(Value::Array(items)) => {
            for t in items
                .iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
            {
                if !head.is_empty() {
                    head.push(' ');
                }
                head.push_str(t);
                if head.trim_start().len() >= prefix.len() {
                    break;
                }
            }
        }
        _ => return false,
    }
    head.trim_start().starts_with(prefix)
}

/// A `tool_result`'s content, which is either a bare string or an array of blocks.
fn result_text(it: &Value) -> String {
    match it.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// `Task #12 created successfully: <subject>` -> `("12", "<subject>")`.
fn parse_task_created(s: &str) -> Option<(String, String)> {
    let rest = s.split("Task #").nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let after = &rest[digits.len()..];
    let marker = " created successfully";
    if !after.starts_with(marker) {
        return None;
    }
    let subject = after[marker.len()..]
        .trim_start_matches(':')
        .trim()
        .to_string();
    Some((digits, subject))
}

/// The opening tag of a background task's completion notification.
const TASK_NOTIFICATION_TAG: &str = "<task-notification>";

/// The `attachment.type` of a notification absorbed mid-turn. The only attachment kind
/// that carries a `prompt` field at all (114/114 measured 2026-09-28).
const QUEUED_COMMAND: &str = "queued_command";

/// Read a bare-string `message.content` as a task-notification, or `None` (D12).
///
/// Gated on STARTS-WITH, never `contains`. The notification block is quoted by things
/// that are not notifications — a `/compact` continuation summary recaps one inside its
/// own prose — and a `contains` test would let that summary end a live agent at the
/// summary's timestamp. Measured 2026-08-19 over the logs then on disk: of the
/// bare-string records carrying the tag, all but one begin with it, and that one is
/// exactly such a summary. (Those logs are a rolling ~30-day window, so read the
/// proportion as an as-of reading; the shape it describes is what this gate rests on.)
///
/// Every tag is read at its FIRST occurrence, which `between` already does. The field
/// order upstream emits — `task-id`, `tool-use-id`, `output-file`, `status`, `summary`,
/// `note`, `result`, `usage` — puts both fields read here ahead of the free-text
/// `<result>`, which is the only place an inner block could be quoted.
pub fn parse_task_notification(s: &str) -> Option<TaskNotification> {
    if !s.trim_start().starts_with(TASK_NOTIFICATION_TAG) {
        return None;
    }
    let task_id = between(s, "<task-id>", "</task-id>")?.trim();
    if task_id.is_empty() {
        return None;
    }
    Some(TaskNotification {
        task_id: task_id.to_string(),
        status: between(s, "<status>", "</status>").map(|v| v.trim().to_string()),
    })
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = s.find(open)? + open.len();
    let end = s[start..].find(close)? + start;
    Some(&s[start..end])
}

/// Sort key that orders `"2"` before `"10"` where ids are numeric, and falls back
/// to the raw string otherwise. Only needs to be deterministic.
fn task_id_key(id: &str) -> (u64, String) {
    (id.parse::<u64>().unwrap_or(u64::MAX), id.to_string())
}

/// Parse Claude Code's fixed RFC 3339 form (`2026-07-28T07:17:00.461Z`) to epoch
/// milliseconds, using `std` only (SD6 — no date crate for one known shape).
/// Returns `None` on anything that does not match, so a bad timestamp leaves the
/// clock untouched rather than defaulting to "now".
///
/// Antigravity's `created_at` (`2026-08-14T03:48:57Z`) is the same shape with the
/// fractional part omitted, and needs no branch: the optional-millis arm below already
/// reads a non-`.` at index 19 as zero milliseconds. Verified against the whole corpus
/// — all 1,761 `created_at` values are exactly 20 bytes, `Z`-suffixed, none fractional.
pub(crate) fn parse_ts(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let mo: u32 = s.get(5..7)?.parse().ok()?;
    let d: u32 = s.get(8..10)?.parse().ok()?;
    let h: u64 = s.get(11..13)?.parse().ok()?;
    let mi: u64 = s.get(14..16)?.parse().ok()?;
    let sec: u64 = s.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let millis: u64 = match s.get(19..20) {
        Some(".") => {
            let frac: String = s[20..].chars().take_while(char::is_ascii_digit).collect();
            let mut ms = frac.clone();
            ms.truncate(3);
            while ms.len() < 3 {
                ms.push('0');
            }
            ms.parse().ok()?
        }
        _ => 0,
    };
    let days = days_from_civil(y, mo, d);
    let secs = days * 86_400 + (h * 3600 + mi * 60 + sec) as i64;
    if secs < 0 {
        return None;
    }
    Some(secs as u64 * 1000 + millis)
}

/// Days since the Unix epoch for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    // The private half of `specs/ai-monitor-context-window`. The crate's other tests
    // are integration tests over fixture JSONL, which cannot reach a private free
    // function — and these three ARE the parse, so leaving them to be exercised only
    // through a whole folded log would test them by implication rather than directly.

    #[test]
    fn strip_sgr_removes_both_observed_encodings() {
        // R2.7 — truecolor and 256-colour coexist in the corpus. Text between them
        // survives intact; this is the case a `38;2`-shaped matcher gets half right.
        let raw = "\u{1b}[38;2;153;153;153mtrue\u{1b}[39m/\u{1b}[38;5;246mindexed\u{1b}[39m";
        assert_eq!(strip_sgr(raw), "true/indexed");
    }

    #[test]
    fn strip_sgr_clears_the_label_number_boundary() {
        // R2.1 — the reason a naive `Auto-compact window: (\d+)` regex fails: two SGR
        // sequences sit BETWEEN the label and the digits.
        let raw = "\u{1b}[1mAuto-compact window: \u{1b}[22m\u{1b}[38;5;246m700k tokens\u{1b}[39m";
        assert_eq!(strip_sgr(raw), "Auto-compact window: 700k tokens");
    }

    #[test]
    fn strip_sgr_keeps_non_sgr_and_malformed_sequences() {
        // Measured: 572/572 escapes in the corpus end in `m`. A CSI that is not SGR is
        // left verbatim — the function strips SGR, and says so. A bare `ESC[` with no
        // final byte must not eat the rest of the string either.
        assert_eq!(strip_sgr("a\u{1b}[2Jb"), "a\u{1b}[2Jb");
        assert_eq!(strip_sgr("a\u{1b}[38;5b"), "a\u{1b}[38;5b");
        assert_eq!(strip_sgr("a\u{1b}b"), "a\u{1b}b");
        assert_eq!(strip_sgr("a\u{1b}["), "a\u{1b}[");
    }

    #[test]
    fn strip_sgr_is_utf8_safe_around_multibyte_text() {
        // The panel's own stdout is full of these — `⛶ ⛁ ⛝` between colour changes.
        let raw = "\u{1b}[38;5;246m⛶ ⛁\u{1b}[39m ⛝";
        assert_eq!(strip_sgr(raw), "⛶ ⛁ ⛝");
        assert_eq!(strip_sgr("한글"), "한글");
    }

    #[test]
    fn strip_sgr_needs_the_escape_and_not_just_the_brackets() {
        // The corpus string that makes this a real case rather than a hypothetical:
        // `/context` prints the model id as `claude-opus-5[1m]`, which is `[`, a
        // parameter byte and `m` with NO escape in front. `dashboard.rs` documents the
        // same trap from the other side — every `[1m]` in a log is prose, never a
        // field. A stripper keyed on the brackets eats the id.
        assert_eq!(strip_sgr("claude-opus-5[1m]"), "claude-opus-5[1m]");
        assert_eq!(strip_sgr("a[0mb"), "a[0mb");
        assert_eq!(strip_sgr("[38;2;153;153;153m"), "[38;2;153;153;153m");
    }

    #[test]
    fn strip_sgr_passes_through_text_without_escapes() {
        assert_eq!(
            strip_sgr("Auto-compact window: 700k tokens"),
            "Auto-compact window: 700k tokens"
        );
        assert_eq!(strip_sgr(""), "");
    }

    #[test]
    fn parse_token_count_reads_the_observed_notations() {
        // R2.5 — `700k`, `74.7k`, `1.0M`, and the bare integer at the grammar's edge.
        assert_eq!(parse_token_count("700k"), Some(700_000));
        assert_eq!(parse_token_count("74.7k"), Some(74_700));
        assert_eq!(parse_token_count("1.0M"), Some(1_000_000));
        assert_eq!(parse_token_count("700000"), Some(700_000));
        assert_eq!(parse_token_count("200K"), Some(200_000));
        assert_eq!(parse_token_count("1m"), Some(1_000_000));
        assert_eq!(parse_token_count(" 700k "), Some(700_000));
    }

    #[test]
    fn parse_token_count_rounds_half_up_without_floats() {
        // `591.8k` is a real value from the corpus (`Free space`). The scaled-integer
        // divide must not truncate a half away.
        assert_eq!(parse_token_count("591.8k"), Some(591_800));
        assert_eq!(parse_token_count("0.0005M"), Some(500));
        assert_eq!(parse_token_count("700.5"), Some(701));
        assert_eq!(parse_token_count("1.2345k"), Some(1_235));
    }

    #[test]
    fn parse_token_count_rejects_what_is_not_a_number() {
        for bad in [
            "",
            "k",
            ".5k",
            "abc",
            "7 0 0k",
            "700kk",
            "70-0",
            "700k tokens",
            "1M)",
            "+700k",
            "-700",
            "7.0.0k",
            "700.k",
        ] {
            assert_eq!(parse_token_count(bad), None, "accepted {bad:?}");
        }
    }

    #[test]
    fn parse_token_count_refuses_to_overflow_into_a_denominator() {
        // Saturating here would hand `u64::MAX` to the tile as a window — a number that
        // passes the floor and renders 1% forever. `None` is the only safe answer.
        assert_eq!(parse_token_count("99999999999999999999M"), None);
        assert_eq!(parse_token_count(&format!("{}M", u64::MAX)), None);
    }

    // ---- the gate, and the fold behind it (R1, R3) ---------------------------

    /// One `/context` stdout, wrapped the way the real record wraps it.
    fn context_stdout(body: &str) -> String {
        format!("<local-command-stdout> \u{1b}[1mContext Usage\u{1b}[22m\n{body}</local-command-stdout>")
    }

    fn line(v: serde_json::Value) -> String {
        v.to_string()
    }

    fn folded(lines: &[String]) -> TelemetrySnapshot {
        let mut t = SessionTelemetry::new();
        for l in lines {
            t.apply(l);
        }
        t.snapshot()
    }

    fn assistant_turn(model: &str) -> String {
        line(serde_json::json!({
            "type": "assistant",
            "message": { "model": model, "content": [] },
        }))
    }

    fn context_record(body: &str) -> String {
        line(serde_json::json!({
            "type": "system",
            "subtype": "local_command",
            "level": "info",
            "content": context_stdout(body),
        }))
    }

    #[test]
    fn a_context_record_fills_the_reading() {
        // R1.1
        let snap = folded(&[context_record(&format!("{HEADER}\n{DEDICATED}\n"))]);
        assert_eq!(
            snap.context_window_reading,
            Some(ContextWindowReading {
                tokens: 700_000,
                model_at_read: None,
            })
        );
    }

    #[test]
    fn prose_quoting_the_label_is_not_a_reading() {
        // R1.2 — the trap this whole gate exists for. In the corpus the four prose
        // types carrying this string outnumber the authoritative one by an order of
        // magnitude — 5:1 on 2026-09-11, 23:1 a day later, climbing as every discussion
        // of this feature adds more prose. The ratio is the fact; the figure is a date.
        for ty in ["assistant", "user", "attachment", "queue-operation"] {
            let quoted = line(serde_json::json!({
                "type": ty,
                "subtype": "local_command",
                "content": context_stdout("Auto-compact window: 999k tokens\n"),
            }));
            assert_eq!(
                folded(&[quoted]).context_window_reading,
                None,
                "{ty} was read as authoritative"
            );
        }
    }

    #[test]
    fn a_local_command_that_is_not_context_is_not_a_reading() {
        // R1.3 — verbatim from the corpus, and the exact string that reads as a
        // 1,000,000-token window if either the title gate or the header grammar is
        // missing. Both are removed here in turn to show each one alone refuses it.
        let body = "<local-command-stdout>Kept model as `Opus 5 (1M context) (default)`</local-command-stdout>";
        let record = line(serde_json::json!({
            "type": "system",
            "subtype": "local_command",
            "content": body,
        }));
        assert_eq!(folded(&[record]).context_window_reading, None);
        assert_eq!(read_window_anchors(body), (None, None));
    }

    #[test]
    fn the_title_gate_is_checked_before_the_escapes_are_stripped() {
        // R1.4 — the two orderings are DISTINGUISHABLE, which is what makes this a
        // test and not a comment. A title interrupted by an SGR sequence survives
        // stripping and would pass a post-strip gate; on the raw bytes it does not
        // match, and not matching is the right answer — the unbroken title is what
        // says "the panel rendered this".
        let split_title = format!(
            "<local-command-stdout> \u{1b}[1mContext\u{1b}[0m Usage\u{1b}[22m\n{DEDICATED}\n</local-command-stdout>"
        );
        assert!(strip_sgr(&split_title).contains(CONTEXT_PANEL_TITLE));
        let record = line(serde_json::json!({
            "type": "system",
            "subtype": "local_command",
            "content": split_title,
        }));
        assert_eq!(folded(&[record]).context_window_reading, None);
    }

    #[test]
    fn a_system_record_of_another_subtype_is_not_a_reading() {
        // `turn_duration` and `stop_hook_summary` are 360 of the corpus's 389 `system`
        // records; the dispatch reaches every one of them.
        let record = line(serde_json::json!({
            "type": "system",
            "subtype": "turn_duration",
            "content": context_stdout(&format!("{DEDICATED}\n")),
        }));
        assert_eq!(folded(&[record]).context_window_reading, None);
    }

    #[test]
    fn a_system_record_with_no_string_content_is_not_a_reading() {
        for content in [
            serde_json::json!(null),
            serde_json::json!([]),
            serde_json::json!(7),
        ] {
            let record = line(serde_json::json!({
                "type": "system",
                "subtype": "local_command",
                "content": content,
            }));
            assert_eq!(folded(&[record]).context_window_reading, None);
        }
    }

    #[test]
    fn the_last_context_in_a_session_wins() {
        // R3.1 — the `model` rule, not the `session_id` rule: `/model` can move the
        // window mid-session, so the newest reading is the true one.
        let first = context_record("Auto-compact window: 200k tokens\n");
        let second = context_record("Auto-compact window: 700k tokens\n");
        let snap = folded(&[first, second]);
        assert_eq!(snap.context_window_reading.map(|r| r.tokens), Some(700_000));
    }

    #[test]
    fn the_window_and_the_model_are_stored_as_one_pair() {
        // R3.2 — the model in force at the moment of the read, not at end of file.
        let snap = folded(&[
            assistant_turn("claude-opus-5"),
            context_record(&format!("{DEDICATED}\n")),
        ]);
        assert_eq!(
            snap.context_window_reading,
            Some(ContextWindowReading {
                tokens: 700_000,
                model_at_read: Some("claude-opus-5".to_string()),
            })
        );

        // A later `/model` moves `model` but cannot move a reading already taken —
        // that divergence is the fact D9 consumes.
        let snap = folded(&[
            assistant_turn("claude-opus-5"),
            context_record(&format!("{DEDICATED}\n")),
            assistant_turn("claude-haiku-4-5"),
        ]);
        assert_eq!(snap.model.as_deref(), Some("claude-haiku-4-5"));
        assert_eq!(
            snap.context_window_reading.and_then(|r| r.model_at_read),
            Some("claude-opus-5".to_string())
        );
    }

    #[test]
    fn a_conflict_between_the_anchors_adopts_the_labelled_line() {
        // R2.4 — the dedicated line is authoritative; the header only corroborates.
        let body = format!(
            "\u{1b}[38;2;153;153;153m\u{2036}   74.7k/1.0M tokens (7%)\u{1b}[39m\n{DEDICATED}\n"
        );
        let snap = folded(&[context_record(&body)]);
        assert_eq!(snap.context_window_reading.map(|r| r.tokens), Some(700_000));
    }

    #[test]
    fn either_anchor_alone_still_yields_a_reading() {
        // R6.2's other half: the redundancy is there to be used, not just counted.
        let header_only = folded(&[context_record(&format!("{HEADER}\n{CATEGORIES}"))]);
        assert_eq!(
            header_only.context_window_reading.map(|r| r.tokens),
            Some(700_000)
        );
        let dedicated_only = folded(&[context_record(&format!("{CATEGORIES}\n{DEDICATED}\n"))]);
        assert_eq!(
            dedicated_only.context_window_reading.map(|r| r.tokens),
            Some(700_000)
        );
    }

    #[test]
    fn a_context_record_that_reads_nothing_leaves_the_field_alone() {
        // The fallback that keeps every non-reading session byte-identical to today.
        let snap = folded(&[context_record("the panel rendered but the format moved\n")]);
        assert_eq!(snap.context_window_reading, None);

        // And it does not erase a reading already taken.
        let snap = folded(&[
            context_record(&format!("{DEDICATED}\n")),
            context_record("the panel rendered but the format moved\n"),
        ]);
        assert_eq!(snap.context_window_reading.map(|r| r.tokens), Some(700_000));
    }

    // ---- diagnostics (R6) ----------------------------------------------------

    #[test]
    fn both_anchors_failing_counts_once_as_a_parse_failure() {
        // R6.1 — and only under the failures counter: nothing here disagreed.
        let snap = folded(&[context_record("the panel rendered but the format moved\n")]);
        assert_eq!(snap.context_window_parse_failures, 1);
        assert_eq!(snap.context_window_anchor_conflicts, 0);
        assert_eq!(snap.context_window_reading, None);
    }

    #[test]
    fn a_sub_floor_value_counts_as_a_parse_failure() {
        // D10's table folds "read nothing" and "read something impossible" into one
        // counter, because to the panel they are the same event.
        let snap = folded(&[context_record("Auto-compact window: 11 tokens\n")]);
        assert_eq!(snap.context_window_parse_failures, 1);
        assert_eq!(snap.context_window_reading, None);
    }

    #[test]
    fn one_anchor_failing_counts_nothing() {
        // R6.2 — this is the redundancy doing its job, not a fault. Counting it would
        // make the failure counter a measure of format variety.
        for body in [
            format!("{HEADER}\n{CATEGORIES}"),
            format!("{CATEGORIES}\n{DEDICATED}\n"),
        ] {
            let snap = folded(&[context_record(&body)]);
            assert_eq!(snap.context_window_parse_failures, 0);
            assert_eq!(snap.context_window_anchor_conflicts, 0);
            assert_eq!(snap.context_window_reading.map(|r| r.tokens), Some(700_000));
        }
    }

    #[test]
    fn disagreeing_anchors_count_as_a_conflict_and_not_as_a_failure() {
        // R2.4 / R6 — a reading still lands, so nothing "failed to parse"; what
        // happened is that two sources of one fact disagreed.
        let body = format!(
            "\u{1b}[38;2;153;153;153m\u{2036}   74.7k/1.0M tokens (7%)\u{1b}[39m\n{DEDICATED}\n"
        );
        let snap = folded(&[context_record(&body)]);
        assert_eq!(snap.context_window_anchor_conflicts, 1);
        assert_eq!(snap.context_window_parse_failures, 0);
        assert_eq!(snap.context_window_reading.map(|r| r.tokens), Some(700_000));
    }

    #[test]
    fn agreeing_anchors_count_nothing() {
        let snap = folded(&[context_record(&format!("{HEADER}\n{DEDICATED}\n"))]);
        assert_eq!(snap.context_window_anchor_conflicts, 0);
        assert_eq!(snap.context_window_parse_failures, 0);
    }

    #[test]
    fn records_outside_the_gate_count_nothing() {
        // R6.3 — the reason the gate is the panel title and not `subtype` alone. In
        // the corpus 16 of 22 `local_command` records are not `/context`; under a
        // `subtype`-only gate the failure counter would rise every time the user typed
        // ANY slash command, including once per successful `/context` (its own echo
        // record), at a signal-to-noise ratio of 1:16.
        let noise = [
            line(serde_json::json!({
                "type": "system", "subtype": "local_command",
                "content": "<command-name>/context</command-name>",
            })),
            line(serde_json::json!({
                "type": "system", "subtype": "local_command", "content": "",
            })),
            line(serde_json::json!({
                "type": "system", "subtype": "local_command",
                "content": "<local-command-stdout>Kept model as `Opus 5 (1M context)`</local-command-stdout>",
            })),
            line(serde_json::json!({
                "type": "system", "subtype": "turn_duration", "content": "12s",
            })),
            line(serde_json::json!({
                "type": "assistant",
                "message": { "model": "claude-opus-5", "content": [] },
            })),
        ];
        let snap = folded(&noise);
        assert_eq!(snap.context_window_parse_failures, 0);
        assert_eq!(snap.context_window_anchor_conflicts, 0);
    }

    #[test]
    fn a_successful_read_next_to_the_noise_still_counts_nothing() {
        // The baseline is a CONSTANT zero, not a function of slash-command usage —
        // which is what makes `> 0` actionable.
        let mut lines = vec![line(serde_json::json!({
            "type": "system", "subtype": "local_command",
            "content": "<command-name>/context</command-name>",
        }))];
        lines.push(context_record(&format!("{HEADER}\n{DEDICATED}\n")));
        let snap = folded(&lines);
        assert_eq!(snap.context_window_parse_failures, 0);
        assert_eq!(snap.context_window_reading.map(|r| r.tokens), Some(700_000));
    }

    // ---- anchors (R2.1-R2.4) ------------------------------------------------

    /// The header line as it really appears — block glyphs, then the gauge.
    const HEADER: &str = "\u{1b}[38;2;153;153;153m\u{2036}   74.7k/700k tokens (11%)\u{1b}[39m";
    /// The labelled line as it really appears — two SGR sequences between the label
    /// and the digits, which is exactly what a naive regex trips on.
    const DEDICATED: &str =
        "\u{1b}[1mAuto-compact window: \u{1b}[22m\u{1b}[38;2;153;153;153m700k tokens\u{1b}[39m";
    /// The per-category block, verbatim from a real record. Nothing here is a window.
    const CATEGORIES: &str = concat!(
        "  \u{1b}[1mEstimated usage by category\u{1b}[22m\n",
        "  System prompt: 4.7k tokens (0.7%)\n",
        "  System tools: 22k tokens (3.1%)\n",
        "  Custom agents: 534 tokens (0.1%)\n",
        "  Messages: 31.8k tokens (4.5%)\n",
        "  Free space: 591.8k (84.5%)\n",
        "  Autocompact buffer: 33k tokens (4.7%)\n",
    );

    #[test]
    fn dedicated_anchor_reads_across_the_sgr_between_label_and_digits() {
        // R2.1
        let (dedicated, _) = read_window_anchors(&strip_sgr(DEDICATED));
        assert_eq!(dedicated, Some(700_000));
    }

    #[test]
    fn header_anchor_reads_the_denominator_not_the_numerator_or_percentage() {
        // R2.2 — three numbers on that line and only the middle one is the window.
        let (_, header) = read_window_anchors(&strip_sgr(HEADER));
        assert_eq!(header, Some(700_000));
    }

    #[test]
    fn header_grammar_excludes_the_category_lines() {
        // R2.3 — every one of these has ` tokens (` and none has the slash. `Free
        // space` has the slash-less form AND no ` tokens (`; it must miss on both.
        let (dedicated, header) = read_window_anchors(&strip_sgr(CATEGORIES));
        assert_eq!((dedicated, header), (None, None));
    }

    #[test]
    fn header_grammar_excludes_the_model_banner() {
        // The concrete near-miss D1a was written for: `1M` inside prose. Without the
        // slash and the ` tokens (` this reads as a 1,000,000-token window.
        let banner = "  Opus 5 (1M context)\n  claude-opus-5[1m]\n";
        assert_eq!(read_window_anchors(banner), (None, None));
    }

    #[test]
    fn header_ignores_a_slash_that_is_not_a_gauge() {
        // Three ways a `X ... tokens (` line can fail to be a gauge: a path's slash, a
        // missing `(`, a missing slash. Each is refused by a DIFFERENT clause, which is
        // why the next test exists — for two revisions this comment credited all three
        // to the numerator check, and none of them is.
        assert_eq!(header_anchor("see .claude/agents 534 tokens (0.1%)"), None);
        assert_eq!(header_anchor("74.7k/700k tokens 11%"), None);
        assert_eq!(header_anchor("74.7k 700k tokens (11%)"), None);
    }

    #[test]
    fn the_numerator_check_buys_one_case_and_not_the_others() {
        // Which clause does which work — asserted, because prose about it was wrong
        // twice. The path case never reaches the numerator check: `parse_window_tokens`
        // rejects `agents 534` one line earlier.
        assert_eq!(parse_window_tokens("agents 534"), None);

        // The case the numerator check alone refuses: a path whose right-hand component
        // IS a plausible window. Delete the check and this reads as 700,000.
        assert_eq!(parse_window_tokens("700000"), Some(700_000));
        assert_eq!(parse_token_count("logs"), None);
        assert_eq!(header_anchor("logs/700000 tokens (5%)"), None);
    }

    #[test]
    fn header_refuses_a_denominator_under_the_floor() {
        // R2.6 at the anchor: the shape is right, the number is not a window.
        assert_eq!(header_anchor("3/11 tokens ("), None);
    }

    #[test]
    fn both_anchors_read_the_same_full_stdout() {
        let content = format!("{HEADER}\n{CATEGORIES}\n{DEDICATED}\n");
        assert_eq!(
            read_window_anchors(&strip_sgr(&content)),
            (Some(700_000), Some(700_000))
        );
    }

    #[test]
    fn the_anchors_are_independent_so_either_survives_the_other() {
        // The whole point of keeping two (D2): a format change on one side still
        // leaves a reading. A pre-filter on the label would have destroyed exactly
        // this property, which is why there is none.
        let header_only = format!("{HEADER}\n{CATEGORIES}");
        assert_eq!(
            read_window_anchors(&strip_sgr(&header_only)),
            (None, Some(700_000))
        );
        let dedicated_only = format!("{CATEGORIES}\n{DEDICATED}");
        assert_eq!(
            read_window_anchors(&strip_sgr(&dedicated_only)),
            (Some(700_000), None)
        );
    }

    #[test]
    fn parse_window_tokens_applies_the_floor() {
        // R2.6 — `11` is the percentage from the header's own line, and the one value a
        // loose match is most likely to pick up.
        assert_eq!(parse_window_tokens("11"), None);
        assert_eq!(parse_window_tokens("999"), None);
        assert_eq!(parse_window_tokens("1000"), Some(1_000));
        assert_eq!(parse_window_tokens("700k"), Some(700_000));
        assert_eq!(parse_window_tokens("nonsense"), None);
    }
}
