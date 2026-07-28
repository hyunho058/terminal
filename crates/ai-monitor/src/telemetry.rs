//! Stateful session telemetry — the fold behind the ai-monitor status dashboard.
//!
//! [`SessionTelemetry::apply`] folds ONE Claude Code JSONL line into accumulated
//! state; [`SessionTelemetry::snapshot`] yields an owned [`TelemetrySnapshot`] the
//! watcher thread sends over its channel (SD5) and the app formats (SD4).
//!
//! Three properties are load-bearing and every change must preserve them:
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
//!   again; a repeat is a replica and does not.
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

// ---- value types --------------------------------------------------------------

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
    pub model: Option<String>,
    // tokens: four cumulative COST counters (D2) ...
    pub total_input: u64,
    pub total_output: u64,
    pub total_cache_read: u64,
    pub total_cache_creation: u64,
    // ... and a latest-value occupancy gauge, which is a different quantity
    pub context_used: u64,
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

/// The accumulator. Fold lines in with [`apply`](SessionTelemetry::apply); read the
/// result with [`snapshot`](SessionTelemetry::snapshot).
#[derive(Debug, Default)]
pub struct SessionTelemetry {
    session_id: Option<String>,
    model: Option<String>,
    total_input: u64,
    total_output: u64,
    total_cache_read: u64,
    total_cache_creation: u64,
    context_used: u64,
    first_event_ms: Option<u64>,
    last_event_ms: Option<u64>,

    tools: VecDeque<CompletedTool>,
    skills: VecDeque<SkillEntry>,
    files: VecDeque<FileActivity>,
    subagents: VecDeque<Subagent>,
    tasks: HashMap<String, TaskEntry>,

    pending: HashMap<String, Pending>,
    pending_order: VecDeque<String>,
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
    pub fn new() -> Self {
        Self::default()
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
            self.session_id = str_at(&v, "sessionId")
                .or_else(|| str_at(&v, "session_id"))
                .map(str::to_string);
        }
        if self.model.is_none() {
            self.model = v
                .pointer("/message/model")
                .and_then(Value::as_str)
                .or_else(|| v.get("model").and_then(Value::as_str))
                .map(str::to_string);
        }

        self.fold_usage(&v);

        // Tool blocks arrive both as top-level events and nested in message.content
        // (the form real logs use). Handle both (D5).
        match v.get("type").and_then(Value::as_str) {
            Some("tool_use") => self.on_tool_use(&v, ts),
            Some("tool_result") => self.on_tool_result(&v, ts),
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

        TelemetrySnapshot {
            session_id: self.session_id.clone(),
            model: self.model.clone(),
            total_input: self.total_input,
            total_output: self.total_output,
            total_cache_read: self.total_cache_read,
            total_cache_creation: self.total_cache_creation,
            context_used: self.context_used,
            first_event_ms: self.first_event_ms,
            last_event_ms: self.last_event_ms,
            tools: self.tools.iter().cloned().collect(),
            skills: self.skills.iter().cloned().collect(),
            files: self.files.iter().cloned().collect(),
            subagents: self.subagents.iter().cloned().collect(),
            tasks,
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
        self.context_used = inp.saturating_add(cr).saturating_add(cc);
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
fn parse_ts(s: &str) -> Option<u64> {
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
