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

use std::collections::{HashMap, VecDeque};

use serde_json::Value;

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
        "write_to_file" | "replace_file_content" => m(
            ToolKind::File(FileOp::Write),
            "CODE_ACTION",
            Some("TargetFile"),
        ),
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
}

/// A tracked task. `status` is whatever string the task tool carried — the
/// vocabulary is not enumerated here, so an unknown value passes through intact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskEntry {
    pub subject: String,
    pub status: String,
    pub created_ms: Option<u64>,
    pub updated_ms: Option<u64>,
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
    // time (D3) — the app derives uptime/idle from these
    pub first_event_ms: Option<u64>,
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
    first_event_ms: Option<u64>,
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
    pub fn restarted(&self) -> Self {
        Self::with_format(self.format).with_log_session_id(self.log_session_id.clone())
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

    // ---- Claude Code (D1-D12) -------------------------------------------------

    fn apply_claude(&mut self, v: &Value) {
        // Time (D4): only a parseable timestamp moves the clock. A bookkeeping line
        // (`mode`, `permission-mode`, ...) carries none and must not reset idle.
        let ts = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_ts);
        if let Some(t) = ts {
            self.first_event_ms.get_or_insert(t);
            self.last_event_ms = Some(t);
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
        match v.get("type").and_then(Value::as_str) {
            Some("tool_use") => self.on_tool_use(v, ts),
            Some("tool_result") => self.on_tool_result(v, ts),
            _ => {}
        }
        match v.pointer("/message/content") {
            Some(Value::Array(items)) => {
                for it in items {
                    match it.get("type").and_then(Value::as_str) {
                        Some("tool_use") => self.on_tool_use(it, ts),
                        Some("tool_result") => self.on_tool_result(it, ts),
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
            Some(Value::String(s)) => self.scan_command_name(s, ts),
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
            self.first_event_ms.get_or_insert(t);
            self.last_event_ms = Some(t);
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
            "PLANNER_RESPONSE" => match v.get("tool_calls") {
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
            },
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

        self.ag_pending.push_back(AgCall {
            name: name.to_string(),
            kind,
            expects,
            start_ms: ts,
            path,
            summary: ag_arg_str(args, "toolSummary"),
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
                let (name, description) = self.ag_subagent_def.clone().unwrap_or_default();
                // AG5 — the status the panel shows is the invocation step's own.
                // `RUNNING` is the format's only non-`DONE` status and it was measured
                // exclusively on `RUN_COMMAND`, so this arm reading it is defensive:
                // an invocation still in flight must not be frozen at a duration it
                // never took (`agent_elapsed` keeps climbing while `end_ms` is `None`).
                let completed = v.get("status").and_then(Value::as_str) == Some("DONE");
                push_capped(
                    &mut self.subagents,
                    Subagent {
                        // The child's own conversation id, which AG5 retains as the
                        // join key to its `brain/<id>/` transcript — a transcript this
                        // fold deliberately does not tail, because each child has its
                        // own `USER_INPUT` steps and interleaving them would multiply
                        // the parent's turn counts (R2.6).
                        id: content.and_then(ag_conversation_id).unwrap_or_default(),
                        subagent_type: name,
                        description,
                        // AG5 — the invocation's `Subagents[]` carries a model, but it
                        // is a JSON array inside a string that truncates (2 of 6
                        // measured), and it names nothing the card needs.
                        model: None,
                        start_ms: call.start_ms,
                        // Unlike the Claude path, the entry is opened at the RESULT
                        // step rather than at the dispatch: the conversation id exists
                        // only there, so an in-flight invocation has no identity to
                        // show yet (G7).
                        end_ms: completed.then_some(ts).flatten(),
                        completed,
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

        // An `Agent` opens a NOT-completed entry in the ring immediately, so an
        // in-flight delegation is visible (D9/R6.1).
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

    fn on_tool_result(&mut self, it: &Value, ts: Option<u64>) {
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
                if let Some(a) = self.subagents.iter_mut().find(|a| a.id == tid) {
                    a.completed = true;
                    a.end_ms = ts;
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

// ---- helpers ------------------------------------------------------------------

/// The Antigravity step types that report a completed tool call — the image of
/// [`ag_tool`]'s middle column. A step type outside this set that also claims no
/// pending call is simply a step that folds nothing (`CHECKPOINT`,
/// `CONVERSATION_HISTORY`, `SYSTEM_MESSAGE`, `ERROR_MESSAGE`), which is why the orphan
/// counter is gated on membership: counting those would make `orphan_tool_results`
/// read as a parser fault on every well-formed transcript.
const AG_RESULT_STEPS: &[&str] = &[
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
    /// The sentence every command result carries, in both the typed and the untyped shape.
    const COMMAND_RESULT: &str = "The command exited with code";
    if step_type == "GENERIC" && content.is_some_and(|c| c.contains(COMMAND_RESULT)) {
        return "RUN_COMMAND";
    }
    step_type
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
