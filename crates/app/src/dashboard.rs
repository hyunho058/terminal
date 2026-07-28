//! ai-monitor dashboard layout — the app-side presentation policy (SD4/D21).
//!
//! `crates/ai-monitor` accumulates raw numbers; everything about how they *look*
//! lives here: which sections exist, their order, truncation, the per-model
//! context-window table, and the placeholder rules. The renderer is untouched
//! (SD3) — the output is a plain `Vec<String>` drawn as glyphs.
//!
//! Two constraints shape every decision in this module:
//!
//! * **The app is width-blind.** `OverlayModel` carries no width, `AppCtx` no
//!   columns, and the `Overlay` trait has no resize hook — the renderer computes a
//!   column budget at draw time and clips. So sections are **open-right** (a rule
//!   that simply ends early when clipped) rather than closed boxes, whose missing
//!   corner would be a conspicuous tell. Even a width channel would not fix it:
//!   nothing recomposes the overlay on resize, so any learned width goes stale at
//!   the first window drag.
//! * **Pure.** [`compose`] takes a snapshot and an injected `now_ms` and reads no
//!   clock, no filesystem, no globals — which is what makes the `insta` snapshots
//!   byte-stable.

use ai_monitor::telemetry::{FileOp, TaskEntry, TelemetrySnapshot};

/// Nominal rule length. At the default 900x540 logical window with `size_pt: 14.0`
/// on a 2x display, `cw = round(14*2*0.6) = 17` and the full-width panel budgets
/// `floor((1800 - 2*10)/17) = 104` columns — so a rule terminates almost exactly at
/// the panel edge by default, runs short on a wider window, and is clipped mid-rule
/// on a narrower one. That is the least conspicuous direction to be wrong in: a rule
/// is self-similar, so a clipped one still reads as a rule.
pub const RULE_COLS: usize = 104;

/// Width of the left label gutter. A fixed *left* gutter needs no width knowledge,
/// unlike a right-aligned column.
const LABEL_COLS: usize = 8;

/// The placeholder for every unknown value (U+2014).
pub const DASH: &str = "—";

// ---- section policy (B1/B2) ---------------------------------------------------
//
// Row caps deliberately EXCEED the default window's ~14-line budget. Sizing to 14
// would show one row per section on the large windows where this dashboard is
// actually read, and the app cannot size adaptively — it is height-blind for the
// same freshness reason it is width-blind. What makes the overflow safe is the
// order: the renderer keeps the LAST `max_lines` (`overlay_layer.rs:268`), so the
// least disposable content is composed last. Tasks clip first, the summary never.
//
// Populated: 5 rules + 19 rows + the 4-line summary = 28 lines. Empty: 5 inline
// rules + 4 = 9 (B3 puts the empty state ON the rule rather than spending a row).
const TASKS_ROWS: usize = 4;
const SKILLS_ROWS: usize = 3;
const TOOLS_ROWS: usize = 4;
const FILES_ROWS: usize = 4;
const AGENTS_ROWS: usize = 4;

/// Outcome, for finished tool and file rows. Two-valued: the pending-tool map is
/// excluded from the snapshot, so no in-flight tool reaches this layer (B4).
const OK: &str = "✓";
const FAIL: &str = "✗";

/// Liveness, for agent rows — a *different axis* from outcome. The fold records no
/// `is_error` for agents, so `✓`/`✗` would assert a result the data does not carry;
/// an agent enters the ring the moment its call is folded and is completed later.
///
/// All four glyphs sit outside the procedurally-drawn `U+2500..=U+259F` range and so
/// depend on font coverage — which is why each pair is one constant, replaceable
/// with `+`/`-` in a line if the configured font disappoints.
const LIVE: &str = "●";
const DONE: &str = "◯";

/// Column budget for a file row's path (B7), derived term by term against
/// `RULE_COLS = 104`: `2 indent, 5 operation label, 1, 1 glyph, 1, 8 duration, 1
/// separator` is 19 of chrome, leaving 85. Pinned by
/// `the_path_budget_equals_its_own_derivation`, so the number cannot drift away from
/// the row it describes — two revisions of the spec stated a budget that disagreed
/// with its own subtraction.
const PATH_COLS: usize = 85;

/// Per-model context windows (SD4 — app-side, so a new model needs no `ai-monitor`
/// change). Seeded from the model ids that actually appear in the logs, measured
/// over `message.model` across every file under `~/.claude/projects`:
/// `claude-opus-4-8` (24,693), `claude-opus-5` (4,020),
/// `claude-haiku-4-5-20251001` (123), `claude-fable-5` (61), `<synthetic>` (28).
///
/// Longest-prefix, then default — one rule, no second mechanism. Note there is no
/// suffix parsing: model ids carry no bracketed context-window marker in any of the
/// 28,925 logged values, so a `[1m]`-style rule would be unreachable code.
const MODEL_WINDOWS: &[(&str, u64)] = &[
    ("claude-opus-", 200_000),
    ("claude-sonnet-", 200_000),
    ("claude-haiku-", 200_000),
    ("claude-fable-", 200_000),
];

/// Everything the composer needs that the snapshot does not carry. Injected rather
/// than read, which is what keeps [`compose`] pure and the snapshots byte-stable.
#[derive(Clone, Copy, Debug, Default)]
pub struct Env {
    /// Wall clock, epoch millis.
    pub now_ms: u64,
    /// This window's own start time. A log whose last event precedes it stopped
    /// being written before the window existed, so it cannot be this window's
    /// session — see [`compose`]. Zero disables the rule.
    pub floor_ms: u64,
    /// `[ai_monitor] context_window` from the TOML config, overriding the table.
    pub context_window: Option<u64>,
}

/// The window for `model`, or `None` when no prefix matches.
///
/// `None` is load-bearing: the caller omits the percentage entirely rather than
/// assuming a denominator. A confident percentage against a guessed window is a
/// false statement about how much room the session has left, and `<synthetic>`
/// matches nothing.
pub fn context_window(model: &str) -> Option<u64> {
    MODEL_WINDOWS
        .iter()
        .filter(|(p, _)| model.starts_with(p))
        .max_by_key(|(p, _)| p.len())
        .map(|(_, w)| *w)
}

/// Compose the dashboard for a snapshot at `now_ms`.
///
/// Pure: the clock is a parameter, not a call.
///
/// `None` — no log resolved for this tab — renders the *same* frame as a snapshot
/// with nothing folded into it yet (F3a's empty example). The two are the same
/// statement to a reader ("nothing known"), and rendering nothing at all would flash
/// the panel empty on every open.
///
/// Section order is Tasks → Skills → Other Tools → File Activity → Agents →
/// summary, and it is a degradation order, not a reading order: the panel keeps the
/// tail, so what is composed last is what survives a short window. File Activity
/// sits below Other Tools deliberately — on the file-heavy session that motivates
/// the "Other Tools" name, the reverse order would spend a guaranteed-visible slot
/// on an empty section while decapitating the populated one above it.
pub fn compose(snap: Option<&TelemetrySnapshot>, env: &Env) -> Vec<String> {
    let empty = TelemetrySnapshot::default();
    // Which log a tab owns is not knowable — a live `claude` holds no open fd on its
    // own log, so the resolver can only offer "whatever wrote here most recently".
    // One thing IS knowable: a log that stopped being written before this window
    // existed cannot belong to a session started in it. Refusing that case is a
    // fact, not a staleness heuristic — it never hides a session that has been
    // active at any point since launch, including one idle for hours, which is
    // exactly what an idle timeout would get wrong.
    let detached = snap.and_then(|t| Detached::of(t, env));
    let t = match snap {
        Some(t) if detached.is_none() => t,
        _ => &empty,
    };
    let mut lines = Vec::with_capacity(28);
    lines.extend(tasks(t));
    lines.extend(skills(t, env.now_ms));
    lines.extend(other_tools(t));
    lines.extend(file_activity(t));
    lines.extend(agents(t, env.now_ms));
    lines.extend(summary(t, env, detached));
    lines
}

/// Why a resolved log is not being shown as this window's session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detached {
    /// Its last event predates the window. Carries that time, so the row can say
    /// how stale the log it declined actually is.
    StaleSince(u64),
    /// It has content but no parseable timestamp, so it cannot be shown to have
    /// been written since the window opened. An ownership check has to fail
    /// **closed**: "no clock" is not evidence of freshness, and treating it as
    /// such is how a foreign session's spend and context render as this tab's.
    Undated,
}

impl Detached {
    fn of(t: &TelemetrySnapshot, env: &Env) -> Option<Detached> {
        if env.floor_ms == 0 {
            return None; // rule disabled
        }
        match t.last_event_ms {
            Some(last) if last < env.floor_ms => Some(Detached::StaleSince(last)),
            Some(_) => None,
            // An entirely empty snapshot is the ordinary "nothing folded yet" state,
            // not a log we are refusing — it has nothing to misattribute.
            None if *t != TelemetrySnapshot::default() => Some(Detached::Undated),
            None => None,
        }
    }
}

/// Tasks — newest-*updated* first (B6c).
///
/// The store is a keyed map, which has no order, so the sort is not a nicety: a
/// `HashMap` iteration would make the pinned snapshots flaky run to run, Rust's
/// hasher being randomly seeded per process. The key is `updated_ms` because it
/// surfaces the task that just changed and because the crate already evicts by it —
/// display order and eviction order agree instead of fighting. Ties fall through to
/// `created_ms` then the id, so the order is total.
fn tasks(t: &TelemetrySnapshot) -> Vec<String> {
    let mut ordered: Vec<&(String, TaskEntry)> = t.tasks.iter().collect();
    ordered.sort_by(|a, b| {
        b.1.updated_ms
            .cmp(&a.1.updated_ms)
            .then_with(|| b.1.created_ms.cmp(&a.1.created_ms))
            .then_with(|| a.0.cmp(&b.0))
    });
    let rows = ordered
        .iter()
        .take(TASKS_ROWS)
        .map(|(_, e)| fit_row("  ", &e.subject, &format!(" · {}{}", e.status, task_dur(e))))
        .collect();
    section("Tasks", "(no tasks)", t.tasks.len(), rows)
}

/// A task's duration, or nothing.
///
/// Only `completed` is treated as terminal — every other string, known or not. The
/// measured vocabulary is exactly `completed` and `in_progress` (plus `active` at
/// creation); `failed` does not occur. Closing the rule on one value rather than
/// enumerating a set is what keeps an unknown status from being read as finished,
/// and `updated_ms − created_ms` is only a duration when the last update *is* the
/// completion — otherwise it measures time-since-last-change (B6a).
fn task_dur(e: &TaskEntry) -> String {
    if e.status != "completed" {
        return String::new();
    }
    match (e.created_ms, e.updated_ms) {
        (Some(c), Some(u)) if u >= c => format!(" · {}", dur_short(u - c)),
        _ => String::new(),
    }
}

/// Skills — name and a relative age (B7a).
///
/// No duration: a `Skill` tool_use→result pair measures ~50ms of instruction-load
/// latency, not the skill's execution, so a duration column here would be a
/// precisely-formatted irrelevance. The age is relative rather than a wall clock
/// because the injected `now_ms` is a bare epoch value and `std` alone cannot
/// resolve a local timezone — a UTC clock on a local-session dashboard is worse
/// than no clock.
fn skills(t: &TelemetrySnapshot, now_ms: u64) -> Vec<String> {
    let rows = t
        .skills
        .iter()
        .take(SKILLS_ROWS)
        .map(|s| {
            let when = match s.start_ms {
                Some(ms) => format!(" · {} ago", age(now_ms.saturating_sub(ms))),
                None => String::new(),
            };
            fit_row("  ", &s.name, &when)
        })
        .collect();
    section("Skills", "(no skills yet)", t.skills.len(), rows)
}

/// Other Tools — named for what the store actually holds (B7b).
///
/// The ring receives only tools that classify as *plain*: `Read|Write|Edit|Grep|Glob`
/// go to file activity, `Agent`/`Skill`/`Task*` to their own stores. "Recent Tools"
/// over a section that structurally excludes the five most common tools would read
/// as a bug on a file-heavy session — empty here, full below.
fn other_tools(t: &TelemetrySnapshot) -> Vec<String> {
    let rows = t
        .tools
        .iter()
        .take(TOOLS_ROWS)
        .map(|c| {
            fit_row(
                &format!("  {} ", mark(c.ok)),
                &c.name,
                &format!(" {}", opt_dur(c.duration_ms)),
            )
        })
        .collect();
    section("Other Tools", "(no other tool calls)", t.tools.len(), rows)
}

/// File Activity — operation, outcome, target, duration.
///
/// Spacing here is single-column and deliberate: it is the layout B7's path budget
/// is derived against, so widening a separator silently invalidates `PATH_COLS`.
fn file_activity(t: &TelemetrySnapshot) -> Vec<String> {
    let rows = t
        .files
        .iter()
        .take(FILES_ROWS)
        .map(|f| {
            // Through `fit_row` like every other section, so the path yields when the
            // duration is wider than the 8 columns B7 reserves for it. `dur` can
            // legitimately emit 9 (`70000h00m`) from a hostile timestamp pair, and a
            // row that overruns its own rule is the one thing this layout must not do.
            fit_row(
                &format!("  {:<5} {} ", op_label(f.op), mark(f.ok)),
                &file_target(f.op, &f.path),
                &format!(" {}", opt_dur(f.duration_ms)),
            )
        })
        .collect();
    section("File Activity", "(no file activity)", t.files.len(), rows)
}

/// Agents — a flat list, because there is no tree.
///
/// Real Claude Code JSONL carries no parent linkage (nested agents write to a
/// `subagents/` sibling directory that is never read), so `├─`/`└─` connectors would
/// draw a hierarchy that was never observed. The heading is "Agents" rather than
/// "Active Agents" because completed entries persist: on an idle session nothing new
/// pushes them out, and an "active" heading whose empty state becomes unreachable
/// after the first agent ever runs is simply false (B5/B6).
fn agents(t: &TelemetrySnapshot, now_ms: u64) -> Vec<String> {
    let rows = t
        .subagents
        .iter()
        .take(AGENTS_ROWS)
        .map(|a| {
            // Frozen on completion. A finished agent whose timer keeps climbing is
            // the single most misleading thing a status dashboard can show.
            let elapsed = match (a.start_ms, a.completed, a.end_ms) {
                (Some(s), true, Some(e)) if e >= s => format!(" · {}", dur(e - s)),
                (Some(s), false, _) => format!(" · {}", dur(now_ms.saturating_sub(s))),
                _ => String::new(),
            };
            let head = format!(
                "  {} {} · ",
                if a.completed { DONE } else { LIVE },
                a.subagent_type.as_deref().unwrap_or(DASH)
            );
            let tail = format!(
                "{}{elapsed}",
                a.model
                    .as_deref()
                    .map(|m| format!(" · {m}"))
                    .unwrap_or_default()
            );
            fit_row(&head, a.description.as_deref().unwrap_or(DASH), &tail)
        })
        .collect();
    section("Agents", "(no agents yet)", t.subagents.len(), rows)
}

/// A section: its rule, then its rows — or a single rule carrying the empty state.
///
/// An empty section is one line, not two: at a 14-line budget an all-empty dashboard
/// must not spend ten of them saying nothing. A missing section and an empty one are
/// different facts, so it is never simply omitted — the layout would jump as
/// sections appeared, and a reader could not tell "no data" from "not built".
fn section(label: &str, empty_state: &str, total: usize, rows: Vec<String>) -> Vec<String> {
    if total == 0 || rows.is_empty() {
        return vec![rule_empty(label, empty_state)];
    }
    let hidden = total.saturating_sub(rows.len());
    let mut out = Vec::with_capacity(rows.len() + 1);
    out.push(if hidden > 0 {
        rule_with(label, Some(&format!("+{hidden} more")))
    } else {
        rule(label)
    });
    out.extend(rows);
    out
}

/// The summary block: rule, session/model/context, spend, activity.
///
/// `overlay-dashboard-boxes` composes this LAST, beneath its sections — the panel
/// keeps the *tail* of the output, so a summary placed first is the first thing
/// clipped on a short window.
/// `detached` carries the last-event time of a log the floor rule rejected, so the
/// session row can say *why* it is showing nothing rather than looking merely empty.
pub fn summary(t: &TelemetrySnapshot, env: &Env, detached: Option<Detached>) -> Vec<String> {
    let mut lines = Vec::with_capacity(4);
    lines.push(rule("ai-monitor"));

    // session id · model · context%
    let session = match detached {
        // Naming the gap is the point: "empty" alone would read as "nothing has
        // happened", when in fact a log was found and deliberately not believed.
        Some(Detached::StaleSince(last)) => format!(
            "(no active session — newest log idle {})",
            dur(env.now_ms.saturating_sub(last))
        ),
        Some(Detached::Undated) => "(no active session — newest log is undated)".to_string(),
        None => {
            let mut ident = vec![
                t.session_id
                    .as_deref()
                    .map(|s| s.chars().take(8).collect::<String>())
                    .unwrap_or_else(|| DASH.to_string()),
                t.model.clone().unwrap_or_else(|| DASH.to_string()),
            ];
            // Omitted entirely — label and all — when the model is unmatched or the
            // gauge is zero. A fresh session's occupancy is unknown, not 0%.
            if let Some(pct) = context_label(t, env) {
                ident.push(pct);
            }
            ident.join(" · ")
        }
    };
    lines.push(row("session", &session, None));

    // Cumulative COST counters, never presented as occupancy: cache-read is
    // re-billed every turn, so these run ~16x the live context on a real session.
    lines.push(row(
        "spend",
        &format!(
            "in {} · out {} · cache {}",
            tok(t.total_input),
            tok(t.total_output),
            tok(t.total_cache_read.saturating_add(t.total_cache_creation))
        ),
        Some("billed"),
    ));

    // Both values freeze between snapshots — nothing recomposes the overlay on a
    // timer — so they are framed as a reading, not a live counter.
    let activity = match (t.first_event_ms, t.last_event_ms) {
        (Some(f), Some(l)) => format!(
            "up {} · idle {}",
            dur(env.now_ms.saturating_sub(f)),
            dur(env.now_ms.saturating_sub(l))
        ),
        _ => DASH.to_string(),
    };
    lines.push(row("activity", &activity, Some("as of last update")));
    lines
}

/// The rendered `context N%` field, or `None` when occupancy cannot be stated.
///
/// Above 100% it reads `>100%` rather than the number. A percentage over 100 is
/// self-evidently impossible and reads as a broken dashboard; what it actually means
/// is that the *window* is wrong — a 1M-context session logs the same bare model id
/// as a 200k one — so the clamp reports the one thing that is certain (the assumed
/// window is too small) instead of a figure that cannot be true. `[ai_monitor]
/// context_window` is how a user resolves it.
pub fn context_label(t: &TelemetrySnapshot, env: &Env) -> Option<String> {
    let pct = context_pct(t, env)?;
    Some(if pct > 100 {
        "context >100%".to_string()
    } else {
        format!("context {pct}%")
    })
}

/// Context occupancy as a whole percentage, or `None` when it cannot be stated.
pub fn context_pct(t: &TelemetrySnapshot, env: &Env) -> Option<u64> {
    if t.context_used == 0 {
        return None;
    }
    // The configured window wins over the table: the table can only guess from a
    // model id that does not record the window.
    let window = match env.context_window {
        Some(w) => w,
        None => context_window(t.model.as_deref()?)?,
    };
    if window == 0 {
        return None;
    }
    // Round-half-up without floats, then floor at 1: `0%` is reserved for "unknown"
    // (F7/R6.2 omit the label entirely there), so a displayed percentage always
    // means real, non-zero occupancy. Rounding a live gauge down to the one value
    // that elsewhere means "no reading" is the one ambiguity worth spending a
    // fraction of a percent to avoid.
    let pct = (t.context_used.saturating_mul(200) / window).div_ceil(2);
    Some(pct.max(1))
}

/// `── label ─────…` to `RULE_COLS`. Open on the right: no closing glyph to lose.
pub fn rule(label: &str) -> String {
    rule_with(label, None)
}

/// A rule carrying an optional marker next to its label (e.g. `+7 more`), which is
/// where a marker must live: at the tail it would be clipped on exactly the narrow
/// windows that make truncation likely.
pub fn rule_with(label: &str, marker: Option<&str>) -> String {
    let head = match marker {
        Some(m) => format!("── {label} {m} "),
        None => format!("── {label} "),
    };
    let used = head.chars().count();
    format!("{head}{}", "─".repeat(RULE_COLS.saturating_sub(used)))
}

/// A rule carrying its section's empty state: `── Skills ── (no skills yet) ───…`.
///
/// The state sits **next to the label**, not at the rule's tail, for the reason B2a
/// gives for `+N more`: at a 700-logical-px window the budget is 81 columns, so
/// anything past column 81 is clipped away — and a clipped empty state leaves a bare
/// rule, which says nothing at all. Adjacent to the label it survives any width.
fn rule_empty(label: &str, state: &str) -> String {
    let head = format!("── {label} ── {state} ");
    format!(
        "{head}{}",
        "─".repeat(RULE_COLS.saturating_sub(head.chars().count()))
    )
}

/// A labelled row with an optional right-hand qualifier, one space after the value.
/// Never padded to a column — aligning would be the fixed-width assumption the
/// open-right design rejects, and it would break the moment a value changed length.
fn row(label: &str, value: &str, qualifier: Option<&str>) -> String {
    let mut s = format!(" {:<w$} {value}", label, w = LABEL_COLS);
    if let Some(q) = qualifier {
        s.push_str(&format!(" ({q})"));
    }
    s
}

/// `prefix + variable + suffix`, truncating **only** the variable part so the row
/// lands within `RULE_COLS`.
///
/// The budget is measured from the chrome actually composed, never from a stated
/// per-row constant — a number written down beside a row is a number that drifts
/// away from it. `PATH_COLS` is the one exception, and it is pinned by a test
/// asserting it equals this same subtraction.
/// The chrome itself is not always bounded — a task's `status` and an agent's
/// `subagent_type`/`model` are raw strings the crate passes through verbatim — so the
/// composed row is clipped once more at the end. Without that backstop a 150-column
/// status would leave the variable part zero-width *and* still overrun the rule.
fn fit_row(prefix: &str, variable: &str, suffix: &str) -> String {
    let chrome = prefix.chars().count() + suffix.chars().count();
    let budget = RULE_COLS.saturating_sub(chrome);
    let row = format!("{prefix}{}{suffix}", truncate_right(variable, budget));
    truncate_right(&row, RULE_COLS)
}

/// Keep the head, mark the loss with `…` (U+2026).
fn truncate_right(s: &str, cols: usize) -> String {
    if s.chars().count() <= cols {
        return s.to_string();
    }
    match cols {
        0 => String::new(),
        n => s.chars().take(n - 1).chain(['…']).collect(),
    }
}

/// Keep the tail. For a path that is the filename and its extension — the part
/// worth reading — which is why paths truncate in the opposite direction to text.
fn truncate_left(s: &str, cols: usize) -> String {
    let len = s.chars().count();
    if len <= cols {
        return s.to_string();
    }
    match cols {
        0 => String::new(),
        n => std::iter::once('…')
            .chain(s.chars().skip(len - (n - 1)))
            .collect(),
    }
}

/// Outcome glyph for a finished entry.
fn mark(ok: bool) -> &'static str {
    if ok {
        OK
    } else {
        FAIL
    }
}

/// The 5-column operation label the path budget is derived against.
fn op_label(op: FileOp) -> &'static str {
    match op {
        FileOp::Read => "READ",
        FileOp::Write => "WRITE",
        FileOp::Grep => "GREP",
        FileOp::List => "LIST",
        FileOp::Other => "OTHER",
    }
}

/// What a file row shows for its target.
///
/// `Grep`/`Glob` capture a **pattern**, not a path (the crate reads `pattern` as
/// well as `file_path`/`path`). Path-shortening a regex is meaningless and actively
/// mangles one containing `/`, so patterns keep their head and truncate right. The
/// TS reference has this bug; it is deliberately not inherited.
fn file_target(op: FileOp, s: &str) -> String {
    match op {
        FileOp::Grep | FileOp::List => truncate_right(s, PATH_COLS),
        _ => short_path(s),
    }
}

/// `parent/basename`, prefixed `…/` when ancestors were dropped (B7).
///
/// A full path spends the whole row on `/Users/…/IdeaProjects/…` and tells the
/// reader nothing; a bare basename cannot distinguish two `mod.rs`. One parent is
/// the cheapest thing that disambiguates.
fn short_path(path: &str) -> String {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let base = segs.last().copied().unwrap_or(path);
    // The root counts as an ancestor: without this, `/etc/hosts` shortens to
    // `etc/hosts`, which reads as a relative path — a different file.
    let root = path.starts_with('/');
    let candidate = match segs.len() {
        0 | 1 if root => format!("…/{base}"),
        0 | 1 => base.to_string(),
        2 if !root => format!("{}/{}", segs[0], base),
        n => format!("…/{}/{}", segs[n - 2], base),
    };
    if candidate.chars().count() <= PATH_COLS {
        return candidate;
    }
    // Drop the parent before mangling the name; only then truncate the name itself,
    // from the left, so the extension survives.
    truncate_left(base, PATH_COLS)
}

/// A duration, or `—` when the crate could not derive one (a missing timestamp or a
/// negative interval). Never `0ms`, which would read as a real sub-millisecond call.
fn opt_dur(ms: Option<u64>) -> String {
    match ms {
        Some(ms) => dur_short(ms),
        None => DASH.to_string(),
    }
}

/// `450ms` / `1.2s` / `2m03s` — sub-second precision matters for a tool call, where
/// the whole point of the column is telling a fast call from a slow one.
fn dur_short(ms: u64) -> String {
    // Round to a tenth FIRST, then pick the unit. Picking the unit first is how
    // `59_990ms` comes out as `60.0s` — the same defect class as choosing `num()`'s
    // magnitude before `{:.1}` rounds it.
    let tenths = ms.saturating_add(50) / 100;
    if tenths < 10 {
        format!("{ms}ms")
    } else if tenths < 600 {
        format!("{}.{}s", tenths / 10, tenths % 10)
    } else {
        dur(tenths.saturating_mul(100))
    }
}

/// `12s` / `4m` / `2h` — a coarse age, for "when did this happen" rather than
/// "how long did it take".
fn age(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        format!("{s}s")
    } else if s < 3_600 {
        format!("{}m", s / 60)
    } else {
        format!("{}h", s / 3_600)
    }
}

/// A billed counter, or `—` when it is zero.
///
/// Zero and "not yet known" are indistinguishable in this data, and a real session
/// never bills zero input or zero output tokens — so the placeholder is the honest
/// reading (F7 rejects showing `0` for an unknown). The only case that reaches zero
/// legitimately is a session with nothing folded, which is exactly the empty state.
fn tok(n: u64) -> String {
    if n == 0 {
        DASH.to_string()
    } else {
        num(n)
    }
}

/// `45` / `19.3K` / `1.9M` — one decimal above each threshold, bare below 1000.
/// Thousands separators are locale-dependent and wider.
pub fn num(n: u64) -> String {
    // The threshold is the value that *rounds* to 1.0M, not 1M itself: `{:.1}`
    // rounds after the branch is chosen, so a plain `>= 1_000_000` prints
    // `1000.0K` for the 50-value band below it — a band a cumulative cache
    // counter passes straight through.
    if n >= 999_950 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// `12s` / `2m03s` / `1h05m`.
pub fn dur(ms: u64) -> String {
    let s = ms / 1000;
    let (m, h) = (s / 60, s / 3600);
    if h > 0 {
        format!("{h}h{:02}m", m % 60)
    } else if m > 0 {
        format!("{m}m{:02}s", s % 60)
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    /// A default environment at `now`: no floor (nothing is ever detached) and no
    /// configured window, so every pre-existing expectation keeps its meaning.
    fn env(now: u64) -> Env {
        Env {
            now_ms: now,
            floor_ms: 0,
            context_window: None,
        }
    }

    use super::*;
    use ai_monitor::telemetry::{CompletedTool, FileActivity, SkillEntry, Subagent};

    /// Every store filled **past** its cap, so each section shows both its rows and
    /// its `+N more` marker, at a fixed `now_ms`. Hand-built: no live log, no clock.
    fn full() -> (TelemetrySnapshot, u64) {
        let (mut t, now) = populated();
        // 6 tasks against a cap of 4, deliberately out of order and with a tie on
        // `updated_ms` so the tie-break is exercised rather than assumed.
        t.tasks = vec![
            task(
                "t-3",
                "Wire the overlay seam",
                "completed",
                now - 900_000,
                now - 600_000,
            ),
            task(
                "t-1",
                "Fold usage per message id",
                "in_progress",
                now - 300_000,
                now - 60_000,
            ),
            task(
                "t-5",
                "Audit the dedupe keying",
                "blocked",
                now - 800_000,
                now - 120_000,
            ),
            task(
                "t-2",
                "Pin the worked examples",
                "completed",
                now - 700_000,
                now - 60_000,
            ),
            task(
                "t-6",
                "Measure the model census",
                "in_progress",
                now - 500_000,
                now - 400_000,
            ),
            task(
                "t-4",
                "Draft the partition manifest",
                "completed",
                now - 950_000,
                now - 930_000,
            ),
        ];
        // 4 skills against a cap of 3.
        t.skills = ["harness-ops:loop", "insta-review", "run-term", "specify"]
            .iter()
            .enumerate()
            .map(|(i, n)| SkillEntry {
                name: (*n).to_string(),
                start_ms: Some(now - (i as u64 + 1) * 240_000),
            })
            .collect();
        // 11 plain tools against a cap of 4 — R2.2's exact "+7 more" case.
        t.tools = (0..11)
            .map(|i| CompletedTool {
                name: ["Bash", "WebFetch", "TodoWrite", "Monitor"][i % 4].to_string(),
                ok: i != 1,
                duration_ms: match i {
                    2 => None,
                    _ => Some(120 + i as u64 * 640),
                },
                start_ms: Some(now - (i as u64 + 1) * 30_000),
            })
            .collect();
        // 5 file activities against a cap of 4, covering a path, a pattern and a
        // failed write.
        t.files = vec![
            file(
                FileOp::Read,
                "/Users/x/proj/crates/renderer/atlas.rs",
                true,
                Some(430),
            ),
            file(FileOp::Grep, r"src/.*/mod\.rs", true, Some(1_240)),
            file(
                FileOp::Write,
                "/Users/x/proj/crates/app/src/dashboard.rs",
                false,
                Some(62_000),
            ),
            file(FileOp::List, "**/*.toml", true, None),
            file(FileOp::Read, "Cargo.toml", true, Some(12)),
        ];
        // 5 subagents against a cap of 4: one live, the rest finished.
        t.subagents = vec![
            Subagent {
                id: "a1".into(),
                subagent_type: Some("general-purpose".into()),
                description: Some("Gate 3 checker".into()),
                model: Some("opus".into()),
                start_ms: Some(now - 90_000),
                end_ms: None,
                completed: false,
            },
            agent_done(
                "a2",
                "Explore",
                "map the overlay seam",
                now - 400_000,
                now - 370_000,
            ),
            agent_done(
                "a3",
                "general-purpose",
                "census message.model",
                now - 900_000,
                now - 880_000,
            ),
            agent_done(
                "a4",
                "Plan",
                "sequence the build order",
                now - 1_200_000,
                now - 1_150_000,
            ),
            agent_done(
                "a5",
                "Explore",
                "find the panel budget",
                now - 1_800_000,
                now - 1_790_000,
            ),
        ];
        (t, now)
    }

    fn task(
        id: &str,
        subject: &str,
        status: &str,
        created: u64,
        updated: u64,
    ) -> (String, TaskEntry) {
        (
            id.to_string(),
            TaskEntry {
                subject: subject.to_string(),
                status: status.to_string(),
                created_ms: Some(created),
                updated_ms: Some(updated),
            },
        )
    }

    fn file(op: FileOp, path: &str, ok: bool, duration_ms: Option<u64>) -> FileActivity {
        FileActivity {
            op,
            path: path.to_string(),
            ok,
            duration_ms,
            start_ms: None,
        }
    }

    fn agent_done(id: &str, kind: &str, desc: &str, start: u64, end: u64) -> Subagent {
        Subagent {
            id: id.to_string(),
            subagent_type: Some(kind.to_string()),
            description: Some(desc.to_string()),
            model: Some("sonnet".into()),
            start_ms: Some(start),
            end_ms: Some(end),
            completed: true,
        }
    }

    /// A populated snapshot: claude-opus-5, 117,066 of a 200k window, 45m12s uptime,
    /// 2m03s since the last event, at a fixed `now_ms`.
    fn populated() -> (TelemetrySnapshot, u64) {
        let now = 1_785_226_000_000;
        let t = TelemetrySnapshot {
            session_id: Some("5e59b629-d34a-4ab6".into()),
            model: Some("claude-opus-5".into()),
            total_input: 45,
            total_output: 19_348,
            total_cache_read: 1_900_000,
            total_cache_creation: 12_000,
            context_used: 117_066,
            first_event_ms: Some(now - (45 * 60 + 12) * 1000),
            last_event_ms: Some(now - (2 * 60 + 3) * 1000),
            ..TelemetrySnapshot::default()
        };
        (t, now)
    }

    #[test]
    fn populated_summary_matches_its_pinned_form() {
        let (t, now) = populated();
        insta::assert_snapshot!(summary(&t, &env(now), None).join("\n"));
    }

    #[test]
    fn empty_snapshot_renders_the_frame_with_placeholders() {
        let t = TelemetrySnapshot::default();
        insta::assert_snapshot!(summary(&t, &env(1_785_226_000_000), None).join("\n"));
    }

    #[test]
    fn an_absent_snapshot_renders_the_same_frame_as_an_empty_one() {
        // R6.1 — "no log resolved" and "nothing folded yet" are one statement to a
        // reader. Never a blank panel.
        let now = 1_785_226_000_000;
        assert_eq!(
            compose(None, &env(now)),
            compose(Some(&TelemetrySnapshot::default()), &env(now))
        );
        let lines = compose(None, &env(now));
        assert_eq!(
            lines.len(),
            9,
            "five inline empty rules + the summary block"
        );
        assert_eq!(
            &lines[5..],
            &summary(&TelemetrySnapshot::default(), &env(now), None)[..]
        );
    }

    #[test]
    fn every_rule_is_exactly_rule_cols_wide() {
        let (t, now) = full();
        for line in compose(Some(&t), &env(now))
            .iter()
            .chain(compose(None, &env(now)).iter())
            .filter(|l| l.starts_with("──"))
        {
            assert_eq!(line.chars().count(), RULE_COLS, "{line}");
        }
        assert_eq!(
            rule_with("Other Tools", Some("+7 more")).chars().count(),
            RULE_COLS
        );
    }

    #[test]
    fn a_rule_never_carries_a_closing_glyph() {
        // Open-right: clipping a rule must leave a valid rule, never an orphaned corner.
        let r = rule("ai-monitor");
        assert!(r.ends_with('─'));
        assert!(!r.contains('╗') && !r.contains('┐') && !r.contains('│'));
    }

    #[test]
    fn context_window_is_longest_prefix_then_default() {
        assert_eq!(context_window("claude-opus-5"), Some(200_000));
        assert_eq!(context_window("claude-opus-4-8"), Some(200_000));
        assert_eq!(context_window("claude-haiku-4-5-20251001"), Some(200_000));
        assert_eq!(context_window("claude-fable-5"), Some(200_000));
        // The one observed value matching nothing — and the reason `None` exists.
        assert_eq!(context_window("<synthetic>"), None);
        assert_eq!(context_window("opus"), None);
    }

    #[test]
    fn an_unmatched_model_omits_the_percentage_entirely() {
        let (mut t, now) = populated();
        t.model = Some("<synthetic>".into());
        let s = summary(&t, &env(now), None).join("\n");
        assert!(!s.contains("context"), "no label, not 0% and not —%:\n{s}");
        assert!(s.contains("<synthetic>"), "the model is still named");
    }

    #[test]
    fn a_zero_gauge_omits_the_percentage() {
        let (mut t, now) = populated();
        t.context_used = 0;
        assert!(!summary(&t, &env(now), None).join("\n").contains("context"));
    }

    #[test]
    fn context_percent_rounds_to_the_nearest_whole() {
        let (mut t, _) = populated();
        assert_eq!(context_pct(&t, &env(0)), Some(59), "117066/200000 = 58.5%");
        t.context_used = 100_000;
        assert_eq!(context_pct(&t, &env(0)), Some(50));
        t.context_used = 1;
        assert_eq!(
            context_pct(&t, &env(0)),
            Some(1),
            "a non-zero gauge never reads 0%"
        );
    }

    #[test]
    fn spend_is_labelled_billed_and_is_never_a_percentage() {
        let (t, now) = populated();
        let s = summary(&t, &env(now), None).join("\n");
        assert!(s.contains("(billed)"));
        // cache = read + creation, summed because both are billed
        assert!(s.contains("cache 1.9M"), "{s}");
        // the ONLY percentage is the context gauge
        assert_eq!(s.matches('%').count(), 1);
    }

    #[test]
    fn activity_carries_the_staleness_qualifier() {
        let (t, now) = populated();
        let s = summary(&t, &env(now), None).join("\n");
        assert!(s.contains("up 45m12s · idle 2m03s"));
        assert!(s.contains("(as of last update)"), "not a live counter");
    }

    #[test]
    fn missing_event_times_render_a_dash_not_an_epoch() {
        let (mut t, now) = populated();
        t.first_event_ms = None;
        t.last_event_ms = None;
        let s = summary(&t, &env(now), None).join("\n");
        assert!(s.contains(&format!("activity {DASH}")) || s.contains(DASH));
        assert!(!s.contains("up "), "no epoch-derived duration:\n{s}");
    }

    #[test]
    fn the_composer_is_pure() {
        // Same inputs -> byte-identical output. This is what makes the snapshots
        // stable, and it only holds because the clock is a parameter.
        let (t, now) = populated();
        assert_eq!(summary(&t, &env(now), None), summary(&t, &env(now), None));
    }

    #[test]
    fn number_and_duration_formatting() {
        assert_eq!(num(45), "45");
        assert_eq!(num(999), "999");
        assert_eq!(num(19_348), "19.3K");
        assert_eq!(num(1_912_000), "1.9M");
        // the rounding band at the magnitude boundary — never "1000.0K"
        assert_eq!(num(999_949), "999.9K");
        assert_eq!(num(999_950), "1.0M");
        assert_eq!(num(1_000_000), "1.0M");
        assert_eq!(dur(12_000), "12s");
        assert_eq!(dur((2 * 60 + 3) * 1000), "2m03s");
        assert_eq!(dur((65 * 60) * 1000), "1h05m");
        assert_eq!(dur_short(430), "430ms");
        assert_eq!(dur_short(1_240), "1.2s");
        // the same magnitude-boundary trap, one unit down — never "60.0s"
        assert_eq!(dur_short(59_949), "59.9s");
        assert_eq!(dur_short(59_950), "1m00s");
        assert_eq!(dur_short(62_000), "1m02s");
        assert_eq!(age(12_000), "12s");
        assert_eq!(age(240_000), "4m");
        assert_eq!(age(7_200_000), "2h");
    }

    // ---- sections ------------------------------------------------------------

    #[test]
    fn the_populated_dashboard_matches_its_pinned_form() {
        let (t, now) = full();
        insta::assert_snapshot!(compose(Some(&t), &env(now)).join("\n"));
    }

    #[test]
    fn the_empty_dashboard_matches_its_pinned_form() {
        insta::assert_snapshot!(compose(None, &env(1_785_226_000_000)).join("\n"));
    }

    // R8.1 — one snapshot per section, each store holding more than its cap, so a
    // change to a cap or to a row's shape shows up as a reviewable diff. One test per
    // section rather than five assertions in one: `insta` stops a test at its first
    // mismatch, so a single test would hide every section after the one that moved.

    #[test]
    fn tasks_at_their_truncation_boundary_are_pinned() {
        let (t, _) = full();
        insta::assert_snapshot!("boundary_tasks", tasks(&t).join("\n"));
    }

    #[test]
    fn skills_at_their_truncation_boundary_are_pinned() {
        let (t, now) = full();
        insta::assert_snapshot!("boundary_skills", skills(&t, now).join("\n"));
    }

    #[test]
    fn other_tools_at_their_truncation_boundary_are_pinned() {
        let (t, _) = full();
        insta::assert_snapshot!("boundary_other_tools", other_tools(&t).join("\n"));
    }

    #[test]
    fn file_activity_at_its_truncation_boundary_is_pinned() {
        let (t, _) = full();
        insta::assert_snapshot!("boundary_file_activity", file_activity(&t).join("\n"));
    }

    #[test]
    fn agents_at_their_truncation_boundary_are_pinned() {
        let (t, now) = full();
        insta::assert_snapshot!("boundary_agents", agents(&t, now).join("\n"));
    }

    #[test]
    fn the_composed_height_is_28_populated_and_9_empty() {
        // B1/B2 — the two numbers the caps were chosen against. If a cap moves, this
        // is the assertion that says so.
        let (t, now) = full();
        assert_eq!(compose(Some(&t), &env(now)).len(), 28);
        assert_eq!(compose(None, &env(now)).len(), 9);
    }

    #[test]
    fn the_summary_block_is_last_and_survives_a_14_line_panel() {
        // R1.2 — the renderer keeps the LAST max_lines, so "composed last" is what
        // makes the summary the content that never gets clipped.
        let (t, now) = full();
        let lines = compose(Some(&t), &env(now));
        let tail: Vec<&String> = lines.iter().rev().take(14).collect();
        for expected in ["ai-monitor", "session ", "spend ", "activity "] {
            assert!(
                tail.iter().any(|l| l.contains(expected)),
                "{expected} clipped at 14 lines"
            );
        }
        assert_eq!(&lines[lines.len() - 4..], &summary(&t, &env(now), None)[..]);
    }

    #[test]
    fn the_section_order_is_fixed() {
        let (t, now) = full();
        let rules: Vec<&String> = compose(Some(&t), &env(now))
            .iter()
            .filter(|l| l.starts_with("──"))
            .cloned()
            .collect::<Vec<_>>()
            .leak()
            .iter()
            .collect();
        let labels: Vec<&str> = rules
            .iter()
            .map(|r| r.split(' ').nth(1).unwrap_or(""))
            .collect();
        assert_eq!(
            labels,
            vec!["Tasks", "Skills", "Other", "File", "Agents", "ai-monitor"]
        );
    }

    #[test]
    fn each_section_renders_at_most_its_cap() {
        // R2.1 — every store in `full()` holds more than its cap.
        let (t, now) = full();
        assert_eq!(tasks(&t).len(), TASKS_ROWS + 1, "rows + rule");
        assert_eq!(skills(&t, now).len(), SKILLS_ROWS + 1);
        assert_eq!(other_tools(&t).len(), TOOLS_ROWS + 1);
        assert_eq!(file_activity(&t).len(), FILES_ROWS + 1);
        assert_eq!(agents(&t, now).len(), AGENTS_ROWS + 1);
    }

    #[test]
    fn truncation_is_stated_next_to_the_label_and_never_lengthens_the_rule() {
        // R2.2 — 11 tools against a cap of 4.
        let (t, _) = full();
        let r = &other_tools(&t)[0];
        assert!(r.starts_with("── Other Tools +7 more ─"), "{r}");
        assert_eq!(r.chars().count(), RULE_COLS);
        // At a 700-logical-px window the budget is 81 columns; a tail marker would
        // be clipped away on exactly the narrow windows where truncation is likely.
        assert!(r.chars().take(81).collect::<String>().contains("+7 more"));
    }

    #[test]
    fn a_section_at_exactly_its_cap_carries_no_marker() {
        // R2.3
        let (mut t, _) = full();
        t.tools.truncate(TOOLS_ROWS);
        assert!(!other_tools(&t)[0].contains("more"));
        assert_eq!(other_tools(&t)[0].chars().count(), RULE_COLS);
    }

    #[test]
    fn an_empty_section_is_one_line_with_its_own_wording() {
        // R3.1/R3.2 — all five reachable, inline on the rule, one line each.
        let now = 1_785_226_000_000;
        let e = TelemetrySnapshot::default();
        for (lines, wording) in [
            (tasks(&e), "(no tasks)"),
            (skills(&e, now), "(no skills yet)"),
            (other_tools(&e), "(no other tool calls)"),
            (file_activity(&e), "(no file activity)"),
            (agents(&e, now), "(no agents yet)"),
        ] {
            assert_eq!(lines.len(), 1, "inline on the rule, never a second row");
            assert!(lines[0].contains(wording), "{}", lines[0]);
            assert_eq!(lines[0].chars().count(), RULE_COLS);
            // Adjacent to the label, so a narrow panel cannot clip away the only
            // thing the section had to say.
            assert!(lines[0]
                .chars()
                .take(81)
                .collect::<String>()
                .contains(wording));
        }
    }

    #[test]
    fn the_two_marker_pairs_are_never_conflated() {
        // R4.1/R4.2 — outcome is ✓/✗; liveness is ●/◯. The fold records no agent
        // outcome, so ✓ on an agent row would assert something the data lacks.
        let (t, now) = full();
        let tool_rows = other_tools(&t)[1..].join("\n");
        assert!(tool_rows.contains(OK) && tool_rows.contains(FAIL));
        assert!(!tool_rows.contains(LIVE) && !tool_rows.contains(DONE));

        let agent_rows = agents(&t, now)[1..].join("\n");
        assert!(agent_rows.contains(LIVE) && agent_rows.contains(DONE));
        assert!(!agent_rows.contains(OK) && !agent_rows.contains(FAIL));
    }

    #[test]
    fn agents_are_flat_with_no_tree_connectors() {
        // R5.1 — there is no parent linkage in the data, so a connector would draw a
        // hierarchy that was never observed.
        let (t, now) = full();
        for row in &agents(&t, now)[1..] {
            assert!(!row.contains('├') && !row.contains('└'), "{row}");
            assert!(row.starts_with("  ●") || row.starts_with("  ◯"), "{row}");
        }
    }

    #[test]
    fn a_live_agent_elapsed_grows_and_a_completed_one_is_frozen() {
        // R5.2
        let (t, now) = full();
        let rows = agents(&t, now);
        assert!(rows[1].contains("1m30s"), "live, from now_ms: {}", rows[1]);
        assert!(
            rows[2].contains("30s"),
            "completed, from its end: {}",
            rows[2]
        );
        let later = agents(&t, now + 600_000);
        assert!(later[1].contains("11m30s"), "live grew: {}", later[1]);
        assert!(later[2].contains("30s"), "completed did NOT: {}", later[2]);
    }

    #[test]
    fn agent_rows_carry_type_description_and_model() {
        // R5.4
        let (t, now) = full();
        let row = &agents(&t, now)[1];
        assert!(row.contains("general-purpose") && row.contains("Gate 3 checker"));
        assert!(row.contains("opus"), "{row}");
    }

    #[test]
    fn tasks_sort_by_updated_desc_deterministically() {
        // R6.1 — the store is an unordered map; without a total order the pinned
        // snapshots would be flaky run to run (the hasher is seeded per process).
        let (t, _) = full();
        let rows = tasks(&t);
        let subjects: Vec<&str> = rows[1..]
            .iter()
            .map(|r| r.trim_start().split(" · ").next().unwrap_or(""))
            .collect();
        // t-1 and t-2 tie on `updated_ms`; t-1 has the later `created_ms` and wins.
        assert_eq!(
            subjects,
            vec![
                "Fold usage per message id",
                "Pin the worked examples",
                "Audit the dedupe keying",
                "Measure the model census"
            ]
        );
        // Order does not depend on the input order of an unordered store.
        let mut shuffled = t.clone();
        shuffled.tasks.reverse();
        assert_eq!(tasks(&shuffled), rows);
    }

    #[test]
    fn task_status_is_the_raw_string_and_only_completed_shows_a_duration() {
        // R6.2/R6.3 — `blocked` is unobserved in the corpus and must still pass
        // through intact rather than being glyph-substituted or treated as terminal.
        let (t, _) = full();
        let rows = tasks(&t).join("\n");
        assert!(rows.contains("· in_progress"), "{rows}");
        assert!(rows.contains("· blocked"), "{rows}");
        assert!(rows.contains("· completed · 10m40s"), "{rows}");
        // the in_progress row carries no duration after its status
        let ip = tasks(&t)[1].clone();
        assert!(ip.ends_with("in_progress"), "{ip}");
    }

    #[test]
    fn skills_show_a_relative_age_and_no_duration() {
        // R7.1 — a Skill pair measures dispatch latency, not execution.
        let (t, now) = full();
        let row = &skills(&t, now)[1];
        assert!(
            row.contains("harness-ops:loop") && row.contains("4m ago"),
            "{row}"
        );
        assert!(!row.contains("ms") && !row.contains(".0s"), "{row}");
    }

    #[test]
    fn a_missing_duration_renders_the_placeholder() {
        // R7.2
        let (t, _) = full();
        assert!(other_tools(&t)[1..].iter().any(|r| r.ends_with(DASH)));
        assert!(file_activity(&t)[1..].iter().any(|r| r.ends_with(DASH)));
    }

    #[test]
    fn paths_keep_the_filename_and_one_parent() {
        // R7.3
        assert_eq!(
            short_path("/Users/x/proj/crates/renderer/atlas.rs"),
            "…/renderer/atlas.rs"
        );
        assert_eq!(short_path("crates/app/dashboard.rs"), "…/app/dashboard.rs");
        assert_eq!(short_path("Cargo.toml"), "Cargo.toml");
        assert_eq!(short_path("src/main.rs"), "src/main.rs");
        // The root is an ancestor: without the marker these read as relative paths,
        // which name different files.
        assert_eq!(short_path("/etc/hosts"), "…/etc/hosts");
        assert_eq!(short_path("/Cargo.toml"), "…/Cargo.toml");
    }

    #[test]
    fn no_row_overruns_the_rule_on_hostile_telemetry() {
        // The chrome around the variable field is raw passthrough data — a task's
        // status, an agent's type and model — so it is not bounded by construction.
        let now = 1_785_226_000_000;
        let mut t = TelemetrySnapshot {
            tasks: vec![task("t", &"s".repeat(300), &"x".repeat(300), 0, 0)],
            subagents: vec![Subagent {
                id: "a".into(),
                subagent_type: Some("y".repeat(300)),
                description: Some("d".repeat(300)),
                model: Some("m".repeat(300)),
                start_ms: Some(0),
                end_ms: None,
                completed: false,
            }],
            ..TelemetrySnapshot::default()
        };
        // A timestamp pair spanning the whole epoch: `dur` emits 9 columns here,
        // one more than the file row's derivation reserves.
        t.files = vec![file(FileOp::Read, &"p".repeat(400), true, Some(u64::MAX))];
        t.tools = vec![CompletedTool {
            name: "n".repeat(300),
            ok: true,
            duration_ms: Some(u64::MAX),
            start_ms: None,
        }];
        t.skills = vec![SkillEntry {
            name: "k".repeat(300),
            start_ms: Some(0),
        }];
        for line in compose(Some(&t), &env(now)) {
            assert!(line.chars().count() <= RULE_COLS, "{line}");
        }
    }

    #[test]
    fn an_over_long_basename_truncates_from_the_left() {
        // R7.4 — left, so the extension survives; right-truncation would clip the
        // one part of a path worth reading.
        let long = format!("/a/b/{}.rs", "x".repeat(120));
        let out = short_path(&long);
        assert_eq!(out.chars().count(), PATH_COLS);
        assert!(out.starts_with('…') && out.ends_with(".rs"), "{out}");
    }

    #[test]
    fn grep_and_glob_render_their_pattern_not_a_path() {
        // R7.5 — path-shortening a regex mangles one containing `/`.
        assert_eq!(
            file_target(FileOp::Grep, r"src/.*/mod\.rs"),
            r"src/.*/mod\.rs"
        );
        assert_eq!(file_target(FileOp::List, "**/*.toml"), "**/*.toml");
        // ... and the head is what survives truncation, since that is a pattern's
        // discriminating part.
        let long = "a/".repeat(80);
        let out = file_target(FileOp::Grep, &long);
        assert_eq!(out.chars().count(), PATH_COLS);
        assert!(out.starts_with("a/a/") && out.ends_with('…'), "{out}");
    }

    #[test]
    fn the_path_budget_equals_its_own_derivation() {
        // B7 term by term: 2 indent + 5 op + 1 + 1 glyph + 1 + 8 duration + 1 = 19.
        // A stated number that disagrees with the row it describes is the defect this
        // pins; two earlier revisions of the spec got it wrong in exactly that way.
        const CHROME: usize = 2 + 5 + 1 + 1 + 1 + 8 + 1;
        assert_eq!(PATH_COLS, RULE_COLS - CHROME);
        // and the composed row honours it: op label is 5 wide, indent is 2
        let (t, _) = full();
        for row in &file_activity(&t)[1..] {
            assert!(row.starts_with("  "), "{row}");
            assert!(row.chars().count() <= RULE_COLS, "{row}");
        }
    }

    #[test]
    fn no_row_overruns_the_rule() {
        let (t, now) = full();
        for line in compose(Some(&t), &env(now)) {
            assert!(line.chars().count() <= RULE_COLS, "{line}");
        }
    }

    // ---- session attachment (the floor rule) ---------------------------------

    #[test]
    fn a_log_that_stopped_before_this_window_existed_is_not_shown_as_the_session() {
        // The reported defect: a fresh window attached to an unrelated session's log
        // and rendered `up 114h37m` as if it were the new tab's. The log's last event
        // predates the window, so it cannot be this window's session.
        let (t, now) = full();
        let launched = now - 30 * 60 * 1000; // window opened 30m ago
        let stale = Env {
            now_ms: now,
            floor_ms: launched,
            // last_event_ms is 2m03s before `now` in `populated()`, i.e. AFTER launch,
            // so move it to before the launch to trigger the rule.
            ..env(now)
        };
        let mut t = t;
        t.last_event_ms = Some(launched - 110 * 60 * 1000); // idle 1h50m, pre-launch
        let out = compose(Some(&t), &stale).join("\n");

        assert!(out.contains("(no active session"), "{out}");
        assert!(out.contains("idle 2h20m"), "says how stale it is:\n{out}");
        // and none of the foreign session's data leaks through
        assert!(!out.contains("claude-opus-5"), "{out}");
        assert!(
            !out.contains("up "),
            "no uptime from a session we disowned:\n{out}"
        );
        assert!(!out.contains("19.3K"), "no spend either:\n{out}");
        assert!(out.contains("(no tasks)") && out.contains("(no agents yet)"));
    }

    #[test]
    fn a_log_active_since_the_window_opened_is_shown_even_when_long_idle() {
        // The case a plain idle-timeout would break: my own session, untouched for
        // hours, but it wrote *after* this window started — so it is mine.
        let (mut t, now) = populated();
        let launched = now - 6 * 3_600_000;
        t.last_event_ms = Some(launched + 1_000);
        t.first_event_ms = Some(launched + 500);
        let out = compose(
            Some(&t),
            &Env {
                now_ms: now,
                floor_ms: launched,
                context_window: None,
            },
        )
        .join("\n");
        assert!(out.contains("claude-opus-5"), "still attached:\n{out}");
        assert!(!out.contains("no active session"), "{out}");
        assert!(
            out.contains("idle 5h59m"),
            "idle is reported, not acted on:\n{out}"
        );
    }

    #[test]
    fn an_absent_log_is_detached_without_claiming_staleness() {
        // Nothing resolved at all: there is no "newest log" to report an age for, so
        // the frame renders its plain empty state rather than inventing one.
        let out = compose(
            None,
            &Env {
                now_ms: 1_000,
                floor_ms: 500,
                context_window: None,
            },
        )
        .join("\n");
        assert!(!out.contains("no active session"), "{out}");
        assert!(out.contains(" session  — · —"), "{out}");
    }

    #[test]
    fn a_log_with_no_timestamps_fails_closed() {
        // F2 — "no clock" is not evidence of freshness. A snapshot with content but
        // no parseable event time cannot be shown to have been written since the
        // window opened, so its spend and context must not render as this tab's.
        let (mut t, now) = populated();
        t.first_event_ms = None;
        t.last_event_ms = None;
        let out = compose(
            Some(&t),
            &Env {
                now_ms: now,
                floor_ms: now - 1000,
                context_window: None,
            },
        )
        .join("\n");
        assert!(
            out.contains("(no active session — newest log is undated)"),
            "{out}"
        );
        assert!(
            !out.contains("claude-opus-5") && !out.contains("19.3K"),
            "{out}"
        );

        // But an entirely empty snapshot is the ordinary "nothing folded yet" state
        // — there is nothing to misattribute, so it is not a refusal.
        let out = compose(
            Some(&TelemetrySnapshot::default()),
            &Env {
                now_ms: now,
                floor_ms: now - 1000,
                context_window: None,
            },
        )
        .join("\n");
        assert!(!out.contains("no active session"), "{out}");
    }

    #[test]
    fn a_detached_row_stays_within_the_rule() {
        // F4 — the detached string is built outside `fit_row`, so its width is
        // asserted rather than assumed.
        let (mut t, now) = populated();
        t.last_event_ms = Some(0);
        for line in compose(
            Some(&t),
            &Env {
                now_ms: now,
                floor_ms: now,
                context_window: None,
            },
        ) {
            assert!(line.chars().count() <= RULE_COLS, "{line}");
        }
    }

    // ---- context window ------------------------------------------------------

    #[test]
    fn an_occupancy_over_the_window_reads_greater_than_100_not_a_number() {
        // The measured case: 256,551 tokens against an assumed 200k window rendered
        // `context 128%`. A percentage over 100 is impossible on its face; what it
        // really means is the window is wrong.
        let (mut t, now) = populated();
        t.context_used = 256_551;
        let out = compose(Some(&t), &env(now)).join("\n");
        assert!(out.contains("context >100%"), "{out}");
        assert!(!out.contains("128"), "never the impossible number:\n{out}");
    }

    #[test]
    fn the_configured_window_overrides_the_model_table() {
        // Which is the fix for the case above: the same 256,551 tokens against the
        // 1M window the session actually had.
        let (mut t, now) = populated();
        t.context_used = 256_551;
        let out = compose(
            Some(&t),
            &Env {
                now_ms: now,
                floor_ms: 0,
                context_window: Some(1_000_000),
            },
        )
        .join("\n");
        assert!(out.contains("context 26%"), "{out}");
    }

    #[test]
    fn the_configured_window_applies_to_an_unmatched_model_too() {
        // `<synthetic>` matches no prefix, so the table yields nothing — but an
        // explicit window is knowledge, not a guess, and does apply.
        let (mut t, now) = populated();
        t.model = Some("<synthetic>".into());
        assert_eq!(
            context_pct(
                &t,
                &Env {
                    now_ms: now,
                    floor_ms: 0,
                    context_window: Some(200_000)
                }
            ),
            Some(59)
        );
        assert_eq!(
            context_pct(&t, &env(now)),
            None,
            "without it, still omitted"
        );
    }

    #[test]
    fn the_whole_dashboard_is_pure() {
        // R8.2 — same inputs, byte-identical output, twice.
        let (t, now) = full();
        assert_eq!(compose(Some(&t), &env(now)), compose(Some(&t), &env(now)));
    }
}
