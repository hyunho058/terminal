//! ai-monitor panel composition — the app-side presentation policy (SD4/D11).
//!
//! `crates/ai-monitor` accumulates raw numbers; everything about how they *read* lives
//! here: which sections exist, their order, their wordings, the per-model context-window
//! table, and the placeholder rules. What this module does **not** do is measure:
//! [`model`] hands `renderer::monitor_layout` a typed structure and the renderer, which
//! knows the panel's width, decides what fits (R3.1/R3.4).
//!
//! Two constraints shape every decision in it:
//!
//! * **No column count lives here any more.** The module used to assemble fixed
//!   104-column strings — `RULE_COLS`, `fit_row`, a `rule*` family — because the old
//!   full-width overlay carried no width, `AppCtx` no columns and the `Overlay` trait no
//!   resize hook, so a section had to be *open-right* (a rule that simply ends when
//!   clipped) rather than a closed box whose missing corner would be a conspicuous tell.
//!   A **docked** panel has a known width, so the entire compensation is gone (R3.3). Do
//!   not bring one back: a second width rule here and in `monitor_layout` would put the
//!   run rectangles and the glyphs drawn into them permanently out of step.
//! * **Pure.** [`model`] takes a snapshot and an injected [`Env`] and reads no clock, no
//!   filesystem, no timezone and no globals — which is what makes the `insta` snapshots
//!   byte-stable (R3.2). The **UTC offset is injected too** (D19), exactly like `now_ms`:
//!   D7/D9 put a local wall clock on the panel, and resolving it here would render one
//!   thing on the author's machine, another on a UTC CI runner, and a third across a DST
//!   boundary. `overlay.rs` — impure by design, it already reads the config and the
//!   process table — resolves it and passes it in.

use ai_monitor::telemetry::{FileOp, Subagent, TaskEntry, TelemetrySnapshot};
use renderer::monitor_model::{
    Bar, Card, ColorRole, MonitorModel, PanelStatus, Row, RowLabel, Section, SectionBody,
    SessionRow, StatTile,
};

/// The placeholder for every unknown value (U+2014).
pub const DASH: &str = "—";

/// Upper bound, in columns, on any variable-length string that enters the model.
///
/// **Not a layout budget.** The renderer knows the panel's width and truncates to it
/// (`monitor_layout::Painter::text`); this bounds what crosses the seam. A task's
/// `status`, an agent's `subagent_type`/`model` and a tool's `name` are raw strings the
/// crate passes through verbatim, so a hostile 4000-char one would otherwise be cloned
/// into the model and compared field by field on every frame by the R7.3 shaping cache —
/// for a row that can never show more than ~90 columns (the panel's 760px maximum, less
/// padding, at the widest cell the app draws). Deliberately above that, so it clips
/// nothing a panel could have shown.
const TEXT_CAP: usize = 96;

/// Outcome, for finished tool and file rows. Two-valued: the pending-tool map is
/// excluded from the snapshot, so no in-flight tool reaches this layer (B4).
const OK: char = '✓';
const FAIL: char = '✗';

/// Liveness, for agent cards — a *different axis* from outcome. The fold records no
/// `is_error` for agents, so `✓`/`✗` would assert a result the data does not carry;
/// an agent enters the ring the moment its call is folded and is completed later.
///
/// All four glyphs sit outside the procedurally-drawn `U+2500..=U+259F` range and so
/// depend on font coverage — which is why each pair is one constant, replaceable
/// with `+`/`-` in a line if the configured font disappoints.
const LIVE: char = '●';
const DONE: char = '◯';

/// Per-model context windows (SD4 — app-side, so a new model needs no `ai-monitor`
/// change). Seeded from the model ids that actually appear in the logs, measured
/// over `message.model` across every file under `~/.claude/projects`:
/// `claude-opus-4-8` (24,693), `claude-opus-5` (4,020),
/// `claude-haiku-4-5-20251001` (123), `claude-fable-5` (61), `<synthetic>` (28).
///
/// Longest-prefix, then default — one rule, no second mechanism. Note there is no
/// suffix parsing: model ids carry no bracketed context-window marker in any of the
/// 28,925 logged values, so a `[1m]`-style rule would be unreachable code. (Re-verified
/// since: every `[1m]` occurrence in a real log is prose inside a message, never a
/// field — the window genuinely cannot be READ.)
///
/// These are therefore the window a session is **assumed** to have, and the assumption
/// is the small one: the same bare id is logged whether the session opened a 200k or a
/// 1M window, so guessing large would over-report headroom on every ordinary session.
/// What cannot be read can still be DEDUCED — see [`WINDOW_TIERS`] and
/// [`promoted_window`], which correct the assumption from observed usage.
const MODEL_WINDOWS: &[(&str, u64)] = &[
    ("claude-opus-", 200_000),
    ("claude-sonnet-", 200_000),
    ("claude-haiku-", 200_000),
    ("claude-fable-", 200_000),
];

/// The context-window sizes a Claude session can actually have, smallest first.
///
/// Two, because two exist: the standard window and the 1M tier. Ascending order is
/// load-bearing — [`promoted_window`] takes the first that fits, which is the smallest,
/// and a mis-sorted table would over-state the window and under-report occupancy.
const WINDOW_TIERS: &[u64] = &[200_000, 1_000_000];

/// Everything the composer needs that the snapshot does not carry. Injected rather
/// than read, which is what keeps [`model`] pure and the snapshots byte-stable.
#[derive(Clone, Copy, Debug, Default)]
pub struct Env {
    /// Wall clock, epoch millis.
    pub now_ms: u64,
    /// Whether a session has been attached — decided once, in `overlay.rs`, and
    /// latched there. This module does not re-derive it: a freshness predicate
    /// evaluated per frame re-litigates an attachment already on screen and blanks a
    /// live panel. Once true, the accumulated data renders unconditionally.
    pub attached: bool,
    /// `[ai_monitor] context_window` from the TOML config, overriding the table.
    pub context_window: Option<u64>,
    /// Which agent the tab this panel belongs to is running, if any — answered by walking
    /// the tab's process tree in `overlay.rs`, which is why it is injected rather than
    /// derived: [`model`] reads no process table, and a snapshot that does not exist carries
    /// nothing to derive it from. It selects which of the two [`Detached::NoLog`] wordings
    /// the session row shows.
    ///
    /// The KIND, not a bool (AG10). It was `tab_has_claude` while Claude was the only agent
    /// monitored, and a bool cannot express the case that arrived with this feature: a tab
    /// running an agent the monitor does not implement. "Waiting for a log" and "there is
    /// nothing here to watch" are different statements, and only the kind separates them —
    /// see [`session_row`].
    pub tab_agent: Option<crate::tabmeta::AgentKind>,
    /// Seconds to add to a UTC epoch to get local wall-clock time (D19), resolved by
    /// `overlay.rs` from the platform timezone each time the model is rebuilt.
    ///
    /// Injected, never read here, for the reason the module header gives: a timezone
    /// lookup inside a pure composer makes its output machine- and season-dependent, and
    /// the `insta` snapshots that pin composition policy would flake on a UTC runner.
    ///
    /// It is the offset **now**, applied to every timestamp on the frame — a session
    /// spanning a DST transition renders its pre-transition rows an hour off. Accepted
    /// rather than solved: a per-timestamp historical offset needs a full tz database,
    /// which this project does not carry, and the error is bounded at one hour twice a
    /// year against values whose purpose is ordering.
    pub utc_offset_secs: i32,
}

/// The **assumed** window for `model`, or `None` when no prefix matches.
///
/// `None` is load-bearing: the caller omits the percentage entirely rather than
/// assuming a denominator. A confident percentage against a guessed window is a
/// false statement about how much room the session has left, and `<synthetic>`
/// matches nothing.
///
/// `Some` is an assumption, not a reading — the id records no window. Callers that
/// have observed usage should correct it through [`promoted_window`] rather than
/// treat this number as measured.
pub fn context_window(model: &str) -> Option<u64> {
    MODEL_WINDOWS
        .iter()
        .filter(|(p, _)| model.starts_with(p))
        .max_by_key(|(p, _)| p.len())
        .map(|(_, w)| *w)
}

/// Correct an assumed window against what the session has demonstrably used.
///
/// The log records no window, so [`context_window`]'s answer is a guess — and it
/// guesses small (see `MODEL_WINDOWS`). But `used` is a **measurement**: the tokens the
/// API actually accepted in one turn. A turn of 514,930 tokens is proof the window was
/// never 200k, whatever the id says, and against the assumed 200k it renders as a
/// useless `>100%` on a session that is barely half full.
///
/// So: promote to the smallest known tier that fits the measurement. Three properties
/// make this a deduction rather than a second guess, and each is pinned by a test —
///
/// * **Evidence only.** `used <= assumed` returns `assumed` untouched, so a session
///   that never exceeds its assumed window is bit-for-bit unaffected. The model NAME is
///   never consulted here; naming a model that *can* run 1M would be exactly the hunch
///   this must not make.
/// * **Smallest fit.** The first tier that fits, not the largest — 514,930 proves the
///   window is >200k and nothing more, so claiming more headroom than proven would be
///   the original bug pointed the other way.
/// * **No fit, no promotion.** Usage past the largest tier promotes to nothing and
///   keeps `assumed`, so the caller's `>100%` still fires. That state is reachable and
///   still means the right thing — "the window is wrong, and this table cannot say what
///   it is" — which is what `[ai_monitor] context_window` exists to resolve.
fn promoted_window(assumed: u64, used: u64) -> u64 {
    if used <= assumed {
        return assumed;
    }
    WINDOW_TIERS
        .iter()
        .copied()
        .find(|&tier| tier > assumed && tier >= used)
        .unwrap_or(assumed)
}

/// How long a session may go quiet before the pill reads IDLE rather than CONNECTED
/// (D5/R5.2).
///
/// Five minutes, not five seconds: a `tool_use` line is written when a call starts and
/// its result when the call ends, so a long `cargo build` or a deep subagent run leaves
/// a genuinely working session silent for minutes. A threshold shorter than the longest
/// routine gap would blink a live session to IDLE mid-call, and under D2 nothing
/// repaints on a timer to blink it back.
const IDLE_AFTER_MS: u64 = 300_000;

/// Compose the **panel model** for a snapshot at `env.now_ms` (D11/R3.1).
///
/// Structure, never pre-formatted width-bound strings: the renderer knows the panel's
/// width and this module does not. Pure, and by the same means as before — every input
/// is in `env`.
///
/// `None` — no log resolved for this tab — builds the *same* model as a snapshot with
/// nothing folded into it yet. The two are the same statement to a reader ("nothing
/// known"), and rendering nothing at all would flash the panel empty on every open.
///
/// `visible` is left at its default: whether the user is looking is `overlay.rs`'s fact,
/// not the snapshot's, and folding it in here would make the model depend on something
/// no telemetry can supply.
pub fn model(snap: Option<&TelemetrySnapshot>, env: &Env) -> MonitorModel {
    let empty = TelemetrySnapshot::default();
    // Which log a tab owns is not knowable — a live `claude` holds no open fd on its
    // own log, so the resolver can only offer "whichever session here did something
    // most recently". Whether to believe it is decided once, at attach, by the
    // overlay; this function is told the answer and never second-guesses it. That is
    // what makes the data durable: with the check here instead, every repaint could
    // and eventually did erase a panel that was showing a live session.
    let detached = match (snap, env.attached) {
        (_, true) => None,
        (Some(t), false) => Some(Detached::of(t)),
        // No log resolved for this tab at all. Still says so — "nothing known" and
        // "nothing here" should not be two different-looking blank frames.
        (None, false) => Some(Detached::NoLog),
    };
    // Unattached renders the EMPTY snapshot, never the resolved one: a log this tab
    // cannot prove is its own must not have its spend, model or uptime shown as this
    // tab's. The wording below says why the panel is empty.
    let t = match snap {
        Some(t) if env.attached => t,
        _ => &empty,
    };
    MonitorModel {
        visible: false,
        title: "ai-monitor".to_string(),
        status: status(t, env, detached),
        session: session_row(t, detached, env),
        tiles: tiles(t, env),
        sections: sections(t, env),
        footer: footer(t, env),
    }
}

/// The pill's three states (D5/R5.2).
///
/// Detached is `NO SESSION` whichever of the four detached cases it is — the pill is an
/// at-a-glance badge and the four are diagnosed by the session row's note. Attached
/// splits on liveness, and an attached log with **no** parseable event time reads IDLE
/// rather than CONNECTED: absence of a clock is not evidence of liveness, the same rule
/// `overlay.rs::proves_ownership` applies to attachment.
fn status(t: &TelemetrySnapshot, env: &Env, detached: Option<Detached>) -> PanelStatus {
    if detached.is_some() {
        return PanelStatus::NoSession;
    }
    match t.last_event_ms {
        Some(last) if env.now_ms.saturating_sub(last) <= IDLE_AFTER_MS => PanelStatus::Connected,
        _ => PanelStatus::Idle,
    }
}

/// The session identity row, with the detached wording underneath it (R5.3).
///
/// The panel's name is already in the header, so this row spends its columns on the
/// session instead of repeating it. When detached it carries **only** the note: every
/// identity field is left empty, because the fields would otherwise describe the log
/// this tab declined to claim.
fn session_row(t: &TelemetrySnapshot, detached: Option<Detached>, env: &Env) -> SessionRow {
    if let Some(d) = detached {
        return SessionRow {
            name: "session".to_string(),
            note: Some(detached_note(d, env)),
            ..SessionRow::default()
        };
    }
    SessionRow {
        name: "session".to_string(),
        short_id: t
            .session_id
            .as_deref()
            .map(|s| s.chars().take(8).collect::<String>())
            .unwrap_or_else(|| DASH.to_string()),
        provider: provider_tag(env.tab_agent),
        model: t
            .model
            .as_deref()
            .map(clip)
            .unwrap_or_else(|| DASH.to_string()),
        note: None,
    }
}

/// The four detached wordings (R5.3), in one place so the pill's compression cannot
/// quietly become a compression of these too.
///
/// They are the entire diagnostic story for "why is this panel empty", which is the
/// panel's most common confusing state — and "you have no Claude here" and "your Claude
/// has not logged yet" are diagnosed completely differently.
pub fn detached_note(d: Detached, env: &Env) -> String {
    match d {
        // Naming the gap is the point: "empty" alone would read as "nothing has
        // happened", when in fact a log was found and deliberately not believed.
        Detached::StaleSince(last) => format!(
            "(no active session — newest log idle {})",
            dur(env.now_ms.saturating_sub(last))
        ),
        Detached::Undated => "(no active session — newest log is undated)".to_string(),
        // Nothing resolved — which under strict resolution is the ordinary empty state,
        // so it has to say *which* nothing this is. The key is whether the tab runs a
        // `claude` process, deliberately NOT whether its hint was usable: a live
        // `claude` started without `--resume`, or whose start time could not be read,
        // yields an empty hint, and telling the user there is no session in a tab where
        // they can watch `claude` running reads as a bug.
        //
        // MONITORED kinds only, which is the distinction the old bool could not draw. A tab
        // running an agent this monitor does not implement (`Codex`, `Gemini`, `Aider` — the
        // sidebar classifies them, nothing reads their logs) must not be told its session
        // "has not written a log yet": no log is ever going to be resolved for it, and a
        // wording that implies waiting describes a state that will never arrive.
        Detached::NoLog if is_monitored(env.tab_agent) => {
            "(this tab's session has not written a log yet)".to_string()
        }
        // Still names Claude, deliberately, and this is the wording's LAST Claude-specific
        // corner. It is only reachable when the tab runs no monitored agent, so an `agy` tab
        // never sees it — that case takes the neutral branch above (R5.1). Generalising the
        // copy would be a user-visible change to a string two tests pin verbatim, which is
        // T6's reviewed-diff half of AG10, not this task's mechanical one.
        Detached::NoLog => "(no Claude session in this tab)".to_string(),
    }
}

/// The session row's provider tag — which agent this panel is showing (AG10).
///
/// This IS the "agent name" surface the spec asked for. R5.1 was drafted as "the panel title
/// displays 'Antigravity CLI'", written against a title element that does not exist; the
/// panel has no title, but it has carried a provider tag next to the session id since it was
/// built. Adding a title row to say a thing an existing row already says would have been new
/// chrome competing with the design rather than fitting it, so the tag is what varies. The
/// short form matches the tag's established shape (`[Claude]`, not `[Claude Code]`).
///
/// Non-Antigravity keeps `[Claude]` verbatim, which is what leaves the pinned panel
/// snapshots byte-identical.
fn provider_tag(kind: Option<crate::tabmeta::AgentKind>) -> String {
    match kind {
        Some(crate::tabmeta::AgentKind::Antigravity) => "[Antigravity]".to_string(),
        _ => "[Claude]".to_string(),
    }
}

/// Does this agent's log record token usage at all (AG4)?
///
/// Antigravity's does not: grepping the whole measured corpus (51 transcripts, 1,761 steps)
/// for `usage`, `input_tokens`, `output_tokens`, `prompt_tokens` and `total_tokens` returns
/// zero hits. So for an `agy` session `total_input`, `total_cache_*` and `context_used` are
/// not "zero so far", they are **never written** — and the difference is the whole point of
/// this predicate, because the panel renders those two states very differently.
///
/// Exhaustive, for the reason [`is_monitored`] gives.
fn reports_usage(kind: Option<crate::tabmeta::AgentKind>) -> bool {
    use crate::tabmeta::AgentKind::*;
    match kind {
        Some(Antigravity) => false,
        // `None` keeps the Claude shape: an attached session whose process has since exited
        // still has real token totals on screen, and blanking them on process death would
        // delete data the user is reading.
        Some(Claude) | Some(Codex) | Some(Gemini) | Some(Aider) | None => true,
    }
}

/// Does this monitor actually read this agent's logs?
///
/// Exhaustive on purpose — no `_` arm. `AgentKind` is the sidebar's classification vocabulary
/// and is deliberately wider than what the monitor implements, so the day one of the others
/// grows a resolver, this match stops compiling until someone decides which side it belongs
/// on. That is the same guard `sidebar-row-redesign` puts on the skin mapping, for the same
/// reason: a `_` arm would silently answer for a variant nobody considered.
fn is_monitored(kind: Option<crate::tabmeta::AgentKind>) -> bool {
    use crate::tabmeta::AgentKind::*;
    match kind {
        Some(Claude) | Some(Antigravity) => true,
        Some(Codex) | Some(Gemini) | Some(Aider) | None => false,
    }
}

/// The 3x2 stat grid, in the design's order.
///
/// Every value is a **reading**, not a counter (D20): the footer's absolute timestamp
/// says when the whole frame was computed, and UPTIME/IDLE freeze between snapshots
/// because under D2 nothing repaints on a timer.
fn tiles(t: &TelemetrySnapshot, env: &Env) -> Vec<StatTile> {
    let since = |ms: Option<u64>| match ms {
        Some(ms) => dur(env.now_ms.saturating_sub(ms)),
        None => DASH.to_string(),
    };
    // Cumulative and session-wide (D3) — NOT `parse_errors`, which counts parser faults.
    // Showing one under the other's label would be a false statement about what failed.
    let errors = t.failed_tool_calls;
    let mut out = vec![
        StatTile::new("UPTIME", since(t.first_event_ms), ColorRole::TextPrimary),
        StatTile::new("IDLE", since(t.last_event_ms), ColorRole::TextPrimary),
        StatTile::new(
            "ERRORS",
            num(errors),
            // Zero failures is a fact worth stating plainly; a rose zero would read as
            // an alarm about nothing.
            if errors > 0 {
                ColorRole::Rose
            } else {
                ColorRole::TextMuted
            },
        ),
    ];
    // The last three tiles are the token trio, and an agent that reports no tokens cannot
    // fill them (AG4/R5.5). It gets the same three SLOTS — the grid is 3x2 and stays 3x2 —
    // with the one quantity it does measure in the first of them.
    if reports_usage(env.tab_agent) {
        out.push(StatTile::new(
            "IN·OUT",
            format!("{}·{}", tok(t.total_input), tok(t.total_output)),
            ColorRole::TextPrimary,
        ));
        out.push(StatTile::new(
            "CACHE",
            tok(t.total_cache_read.saturating_add(t.total_cache_creation)),
            ColorRole::Teal,
        ));
        out.push(context_tile(t, env));
    } else {
        // Turns, in the slot tokens would have used: the same "two numbers, one dot" idiom
        // as IN·OUT, and the one headline quantity an Antigravity transcript does record
        // (AG3/AG8).
        out.push(StatTile::new(
            "TURNS",
            format!("{}·{}", num(t.turns_user), num(t.turns_assistant)),
            ColorRole::TextPrimary,
        ));
        // Explicitly unavailable, NOT zero.
        //
        // Only the CACHE line is load-bearing: `tok(0)` would print a Teal `0`, which
        // asserts a measurement ("nothing cached") that no line in an Antigravity log
        // entitles the panel to make. CONTEXT here is belt-and-braces and says so
        // honestly — `context_pct` already returns `None` the moment `context_used == 0`
        // (see its first line), so `context_tile` would produce this exact tile anyway.
        // It is written out rather than delegated so the panel's answer for a token-less
        // agent does not silently depend on a zero-check living in another function, and
        // so this branch reads as one decision instead of one decision plus a coincidence.
        // `StatTile::gauge`'s own doc draws the same distinction from the other
        // direction — an unknown window rather than an unknown numerator.
        out.push(StatTile::new("CACHE", DASH, ColorRole::TextMuted));
        out.push(StatTile::new("CONTEXT", DASH, ColorRole::TextMuted));
    }
    out
}

/// The CONTEXT tile and its gauge (R5.4/R5.5).
///
/// `None` from [`context_pct`] emits **no percentage and no gauge** — not `0%`, and not
/// an empty track. An empty track is a claim ("you have used none of your window") that
/// a model matching no `MODEL_WINDOWS` prefix does not entitle the panel to make, and
/// `StatTile::gauge` distinguishes the two precisely so this case can be drawn as
/// nothing.
fn context_tile(t: &TelemetrySnapshot, env: &Env) -> StatTile {
    let Some(pct) = context_pct(t, env) else {
        return StatTile::new("CONTEXT", DASH, ColorRole::TextMuted);
    };
    // `>100%` because a percentage over 100 is self-evidently impossible and reads as a
    // broken dashboard. The gauge clamps to full alongside it.
    //
    // It now means TWO different things, and which one depends on the branch
    // `context_pct` took for the denominator. The tile does not distinguish them —
    // changing what this displays is out of scope — so the distinction is recorded here:
    //
    // * Via a `/context` READING, it is a true statement about the session: usage has
    //   passed the auto-compact threshold the session itself printed. Nothing is wrong
    //   with the denominator; the number really is over budget. This is why a reading is
    //   terminal rather than promoted — promotion would hide a fact behind a guess.
    // * Via the TABLE, it is the old signal: the assumed window is too small, and this
    //   build knows no tier large enough. Reachable only past the largest known tier —
    //   unobserved in the measured corpus (max 666,545), but retained because a larger
    //   window shipping is exactly when it matters. `promoted_window` only promotes to a
    //   window the table KNOWS about, so usage past the largest tier fits nothing and
    //   lands here. `[ai_monitor] context_window` remains how a user resolves it.
    let value = if pct > 100 {
        ">100%".to_string()
    } else {
        format!("{pct}%")
    };
    StatTile::new("CONTEXT", value, ColorRole::Teal).with_gauge(pct as f32 / 100.0)
}

/// The footer: an **absolute** last-update time (D9/D20/R5.10).
///
/// Never a relative age. Under D2 nothing repaints on a timer, so "updated 6s ago"
/// freezes and becomes actively false while Claude spends five minutes composing a
/// reply; `16:21:18` stays correct however long it sits on screen. It also qualifies
/// every other value on the panel — UPTIME and IDLE are readings taken at that moment,
/// and a duration has no absolute form to fix itself with.
fn footer(t: &TelemetrySnapshot, env: &Env) -> String {
    match t.last_event_ms {
        Some(ms) => format!("Last update {}", clock(ms, env.utc_offset_secs)),
        None => format!("Last update {DASH}"),
    }
}

/// `16:21:18` — a UTC epoch in local wall-clock time, on the injected offset (D7/D19).
///
/// Deliberately no date: the panel shows one session, and a session that spans midnight
/// is rarer than the row this would cost. `rem_euclid` rather than `%` so a negative
/// offset west of Greenwich wraps into the previous day instead of producing a negative
/// hour.
pub fn clock(epoch_ms: u64, utc_offset_secs: i32) -> String {
    let local = (epoch_ms / 1000) as i64 + utc_offset_secs as i64;
    let sod = local.rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", sod / 3600, (sod % 3600) / 60, sod % 60)
}

/// Why a resolved log is not being shown as this window's session.
///
/// `StaleSince` and `Undated` describe a log that *was* resolved and then declined —
/// which production no longer produces: strict resolution means a log this tab cannot
/// prove is its own is never resolved in the first place, so "not attached" collapses
/// to `NoLog`. Both variants are reachable only through the `resolve_session` ranked
/// fallback, which is retained under test and has no production caller. They are kept
/// (and worded, and tested) for that contract rather than deleted on the strength of
/// one release's call graph.
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
    /// Nothing resolved at all. Distinct from the two above because it must not
    /// claim anything about a log that does not exist.
    NoLog,
}

impl Detached {
    /// Why the *not yet attached* state is showing. Reporting only — the decision
    /// itself was made in `overlay.rs`; this turns it into words.
    fn of(t: &TelemetrySnapshot) -> Detached {
        // Read from the RESOLVED snapshot, before `model` swaps in the empty one —
        // testing the emptied value here would make every case look like `NoLog`.
        if *t == TelemetrySnapshot::default() {
            return Detached::NoLog;
        }
        match t.last_event_ms {
            Some(last) => Detached::StaleSince(last),
            None => Detached::Undated,
        }
    }
}

// ---- the content sections -----------------------------------------------------

/// The content sections, in the design's order.
///
/// The order is a **priority** order now, not the degradation order the retiring overlay
/// used. That layer kept the *last* `max_lines`, so the least disposable content had to
/// be composed last; D18's allocator instead reserves headings from the top and drops
/// them from the **bottom**, so what is listed last is what a short panel loses. TASKS
/// and ACTIVE AGENTS lead because they answer "what is this session doing right now".
///
/// The five ring-backed sections are **not** capped here. The crate's rings are already
/// bounded (`TOOLS_CAP = 32`, `SKILLS_CAP = 16`, …) and sized at "~3x the app's display
/// counts, so the app can show more without a crate change"; an app-side cap on top of
/// that would decide the panel's height in the one file that cannot see it, and a tall
/// window would show four rows while the allocator had room for eight. TOOL USAGE is the
/// exception, and for a different reason — see [`TOOL_BARS`].
fn sections(t: &TelemetrySnapshot, env: &Env) -> Vec<Section> {
    vec![
        tasks(t, env),
        agents(t, env),
        skills(t, env),
        file_activity(t),
        recent_calls(t),
        tool_usage(t),
    ]
}

/// A populated section, or its empty state.
///
/// The wording rides the **heading** (the renderer draws it there), never a row: an empty
/// state that occupied a row would compete for the single row D18 guarantees a populated
/// neighbour. A missing section and an empty one are also different facts, so a section
/// is never simply omitted — the layout would jump as sections appeared, and a reader
/// could not tell "no data" from "not built".
fn section(label: &str, empty_state: &str, total: usize, body: SectionBody) -> Section {
    Section {
        collapsed: false,
        label: label.to_string(),
        total,
        lost: 0,
        body: if body.is_empty() {
            SectionBody::Empty(empty_state.to_string())
        } else {
            body
        },
    }
}

/// How many entries a ring has dropped: what the session did, minus what is still held.
///
/// Only RECENT CALLS can answer this honestly, and only because the crate keeps
/// `total_tool_calls` as **its own scalar** rather than a sum over the bounded tally — the
/// comment there says exactly why: *"counting a ring would print a number that LOOKS
/// cumulative and is not"*. The other rings have no cumulative counter to compare against,
/// so their loss stays `0`, which reads as "nothing known to be lost" and not as "nothing
/// lost". Inventing a number for them would be the `+N more` mistake again.
fn ring_loss(session_total: u64, retained: usize) -> usize {
    (session_total as usize).saturating_sub(retained)
}

/// TASKS — newest-*updated* first (B6c), one card each.
///
/// The crate hands the tasks over as a `Vec` already sorted by **id**
/// (`telemetry.rs:358`, which is what makes the snapshots byte-stable), so the sort here
/// is not deduplicating a `HashMap` order — it is what turns that arbitrary-but-stable id
/// order into the display order. The key is `updated_ms` because it
/// surfaces the task that just changed and because the crate already evicts by it —
/// display order and eviction order agree instead of fighting. Ties fall through to
/// `created_ms` then the id, so the order is total.
fn tasks(t: &TelemetrySnapshot, env: &Env) -> Section {
    let mut ordered: Vec<&(String, TaskEntry)> = t.tasks.iter().collect();
    ordered.sort_by(|a, b| {
        b.1.updated_ms
            .cmp(&a.1.updated_ms)
            .then_with(|| b.1.created_ms.cmp(&a.1.created_ms))
            .then_with(|| a.0.cmp(&b.0))
    });
    let cards = ordered
        .iter()
        .map(|(_, e)| Card {
            accent: Some(task_role(&e.status)),
            title: clip(&e.subject),
            value: task_dur(e),
            detail: Some(task_detail(e, env)),
        })
        .collect();
    section(
        "TASKS",
        "(no tasks)",
        t.tasks.len(),
        SectionBody::Cards(cards),
    )
}

/// A task's accent: amber while it is moving, green once it is done, faint otherwise.
///
/// Only `completed` is treated as terminal — every other string, known or not. The
/// measured vocabulary is exactly `completed` and `in_progress` (plus `active` at
/// creation); `failed` does not occur. Closing the rule on one value rather than
/// enumerating a set is what keeps an unknown status from being read as finished.
fn task_role(status: &str) -> ColorRole {
    match status {
        "completed" => ColorRole::AccentGreen,
        "in_progress" | "active" => ColorRole::Amber,
        _ => ColorRole::TextFaint,
    }
}

/// A completed task's duration, or nothing.
///
/// `updated_ms − created_ms` is only a duration when the last update *is* the
/// completion — otherwise it measures time-since-last-change (B6a), which the card's
/// detail line reports instead and in those words.
fn task_dur(e: &TaskEntry) -> String {
    if e.status != "completed" {
        return String::new();
    }
    match (e.created_ms, e.updated_ms) {
        (Some(c), Some(u)) if u >= c => dur_short(u - c),
        _ => String::new(),
    }
}

/// `in_progress · updated 4m ago` — the raw status, then how long since it moved.
///
/// The status passes through verbatim; substituting a glyph or normalising the
/// vocabulary would silently reinterpret a value the crate deliberately does not
/// enumerate. The age answers what a status alone leaves open on a long session
/// (`in_progress` *since when*), and it is a reading like every other duration on the
/// panel — frozen between snapshots, qualified by the footer's absolute time (D20).
fn task_detail(e: &TaskEntry, env: &Env) -> String {
    let status = clip(&e.status);
    match e.updated_ms {
        Some(u) => format!(
            "{status} · updated {} ago",
            age(env.now_ms.saturating_sub(u))
        ),
        None => status,
    }
}

/// ACTIVE AGENTS — a flat list, because there is no tree.
///
/// Real Claude Code JSONL carries no parent linkage (nested agents write to a
/// `subagents/` sibling directory that is never read), so `├─`/`└─` connectors would
/// draw a hierarchy that was never observed.
///
/// Completed agents keep their cards under the design's ACTIVE heading: on an idle
/// session nothing new pushes them out, and a section that emptied itself the moment its
/// last agent finished would say "no agents ran" when it means "none are running". The
/// `●`/`◯` mark and the accent are what separate the two.
fn agents(t: &TelemetrySnapshot, env: &Env) -> Section {
    let cards = t
        .subagents
        .iter()
        .map(|a| Card {
            // Three-way, and the failure arm is gated on `completed` for a reason:
            // `ended_ok` starts `true` and is written only where an ending is observed,
            // so an ungated rule would be meaningless for a live agent — whose elapsed
            // is still growing and whose field therefore still holds its opening value.
            // The gate is what keeps Rose off in-flight cards; it is NOT what decides
            // which completions are Rose. A synchronous completion whose result carried
            // `is_error: true` is a real failure and is coloured like one — the fold
            // knows it failed, so the card says so (D13/R7.1/R7.5). The colour rides the
            // accent bar because `Card` has no colour channel for its value; TASKS
            // already derives an accent from a status the same way (`task_role`)
            // (D13/R7.4).
            accent: Some(match (a.completed, a.ended_ok) {
                (false, _) => ColorRole::Amber,
                (true, true) => ColorRole::TextFaint,
                (true, false) => ColorRole::Rose,
            }),
            title: format!(
                "{} {}",
                if a.completed { DONE } else { LIVE },
                clip(a.subagent_type.as_deref().unwrap_or(DASH))
            ),
            value: agent_elapsed(a, env),
            detail: Some(agent_detail(a)),
        })
        .collect();
    section(
        "ACTIVE AGENTS",
        "(no agents yet)",
        t.subagents.len(),
        SectionBody::Cards(cards),
    )
}

/// Frozen on completion. A finished agent whose timer keeps climbing is the single most
/// misleading thing a status dashboard can show.
fn agent_elapsed(a: &Subagent, env: &Env) -> String {
    match (a.start_ms, a.completed, a.end_ms) {
        (Some(s), true, Some(e)) if e >= s => dur(e - s),
        (Some(s), false, _) => dur(env.now_ms.saturating_sub(s)),
        _ => String::new(),
    }
}

fn agent_detail(a: &Subagent) -> String {
    let desc = clip(a.description.as_deref().unwrap_or(DASH));
    match a.model.as_deref() {
        Some(m) => format!("{desc} · {}", clip(m)),
        None => desc,
    }
}

/// SKILLS — name and the local wall-clock time it started, `at 16:21:18` (D7/R5.9).
///
/// No duration: a `Skill` tool_use→result pair measures ~50ms of instruction-load
/// latency, not the skill's execution, so a duration column here would be a
/// precisely-formatted irrelevance.
///
/// **Absolute, not the relative age this section used to show.** The old form was chosen
/// because `std` alone cannot resolve a local timezone and a UTC clock on a local-session
/// dashboard is worse than no clock — but that reasoning assumed `std` was the only
/// option, and D7/D19 removed the constraint by resolving the offset in `overlay.rs` and
/// injecting it. An absolute time is also what makes a skill run correlatable against the
/// terminal's own scrollback, which "12s ago" on a panel that does not repaint is not.
fn skills(t: &TelemetrySnapshot, env: &Env) -> Section {
    let rows = t
        .skills
        .iter()
        .map(|s| Row {
            text: clip(&s.name),
            value: match s.start_ms {
                Some(ms) => format!("at {}", clock(ms, env.utc_offset_secs)),
                None => String::new(),
            },
            value_color: ColorRole::TextMuted,
            ..Row::default()
        })
        .collect();
    section(
        "SKILLS",
        "(no skills yet)",
        t.skills.len(),
        SectionBody::Rows(rows),
    )
}

/// FILE ACTIVITY — verb, outcome, target, duration.
///
/// The verb is a [`RowLabel`], which the renderer draws in a fixed gutter rather than at
/// its own width — that is what makes `READ`/`WRITE` scan as a column — and it carries
/// the design's roles: magenta for a write, teal for a read (R5.6).
fn file_activity(t: &TelemetrySnapshot) -> Section {
    let rows = t
        .files
        .iter()
        .map(|f| Row {
            label: Some(RowLabel {
                text: op_label(f.op).to_string(),
                color: op_role(f.op),
            }),
            glyph: Some(mark(f.ok)),
            text: file_target(f.op, &f.path),
            value: opt_dur(f.duration_ms),
            value_color: value_role(f.ok, f.duration_ms),
        })
        .collect();
    section(
        "FILE ACTIVITY",
        "(no file activity)",
        t.files.len(),
        SectionBody::Rows(rows),
    )
}

/// RECENT CALLS — the completed-tool ring, newest first.
///
/// The ring receives only tools that classify as *plain*: `Read|Write|Edit|Grep|Glob` go
/// to FILE ACTIVITY directly above, `Agent`/`Skill`/`Task*` to their own sections. The
/// design's heading is kept because FILE ACTIVITY sitting immediately above answers the
/// "where are the reads" question the name would otherwise raise — but the **empty state
/// still names the gap**, since an empty RECENT CALLS on a file-heavy session is
/// otherwise indistinguishable from a broken section.
fn recent_calls(t: &TelemetrySnapshot) -> Section {
    let rows = t
        .tools
        .iter()
        .map(|c| Row {
            glyph: Some(mark(c.ok)),
            text: clip(&c.name),
            value: opt_dur(c.duration_ms),
            value_color: value_role(c.ok, c.duration_ms),
            ..Row::default()
        })
        .collect();
    let mut s = section(
        "RECENT CALLS",
        "(no other tool calls)",
        t.tools.len(),
        SectionBody::Rows(rows),
    );
    // D20/D6: this is the section the interview named as the acceptance criterion, and the
    // only one the crate gives a cumulative counter for. On a long session the ring holds
    // the last `TOOLS_CAP` calls and the rest are gone — the panel says so rather than
    // letting "everything is reachable now" quietly become a lie.
    s.lost = ring_loss(t.total_tool_calls, t.tools.len());
    s
}

/// Distinct tools TOOL USAGE ranks (D12).
///
/// The one app-side cap on a section, and it is not a height decision the allocator could
/// make instead: the tally holds up to 64 names, and a bar list long enough to include a
/// tool called twice is a list in which the design's proportions — the point of the
/// section — are unreadable. Nine matches the design.
const TOOL_BARS: usize = 9;

/// TOOL USAGE — the cumulative per-tool tally, ranked and normalised (D12/R5.8).
///
/// The ranking lives here because ranking *is* presentation: the crate emits `tool_calls`
/// in name order and says so, precisely so this decision is not made where the numbers are
/// accumulated. Count descending, then name ascending — a count-only sort would leave
/// ties resolved by whatever order the vector happened to arrive in, and the pinned
/// snapshots would flake the first time two tools drew level.
///
/// `tool_calls_other` is in **neither** the ranking nor the denominator: it accumulates
/// monotonically and never drains, so ranking it would permanently occupy a slot with an
/// aggregate that names nothing, and normalising against it would rescale every real
/// tool's bar toward zero as the session aged.
///
/// The counts are cumulative and the display rings are not — `TOOLS_CAP = 32` means
/// everything older than the last 32 calls is gone from RECENT CALLS above. A tool with a
/// large bar and no recent row is therefore the expected reading, not a contradiction.
fn tool_usage(t: &TelemetrySnapshot) -> Section {
    let mut ranked: Vec<&(String, u64)> = t.tool_calls.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(TOOL_BARS);
    // The highest *named* count is the full-width bar. Zero only when the tally is empty,
    // in which case there are no bars to normalise.
    let top = ranked.first().map(|(_, n)| *n).unwrap_or(0);
    let bars = ranked
        .iter()
        .map(|(name, n)| {
            let fraction = if top > 0 { *n as f32 / top as f32 } else { 0.0 };
            Bar::new(clip(name), *n, fraction)
        })
        .collect();
    section(
        "TOOL USAGE",
        "(no tool calls yet)",
        t.tool_calls.len(),
        SectionBody::Bars(bars),
    )
}

/// The colour a row's trailing value and its outcome mark share.
///
/// They share one role deliberately (`Row::value_color` drives both): a `✗` drawn in the
/// duration's colour would announce a failure in the palette of a fast call. So a failure
/// outranks the magnitude — the question "did this work" is answered before "was it
/// quick" — and only a successful call is coloured by how long it took.
fn value_role(ok: bool, duration_ms: Option<u64>) -> ColorRole {
    if !ok {
        return ColorRole::Rose;
    }
    match duration_ms {
        Some(ms) => dur_role(ms),
        // No derivable duration: the value is the `—` placeholder, and there is nothing
        // to bucket. Colouring it green would read as "fast".
        None => ColorRole::TextMuted,
    }
}

/// A duration's colour, bucketed by magnitude (R5.7): amber from 30s, neutral from 1s,
/// green below.
///
/// Buckets, not a gradient, because the question this column is read for is categorical —
/// *was that slow* — and three steps answer it at a glance where a continuous ramp would
/// have to be decoded against a scale the panel does not draw. The boundaries are the two
/// places the answer changes: a second is where a call stops being instant, and thirty is
/// where it stops being something you waited through and becomes something you noticed.
fn dur_role(ms: u64) -> ColorRole {
    if ms >= 30_000 {
        ColorRole::Amber
    } else if ms >= 1_000 {
        ColorRole::TextMuted
    } else {
        ColorRole::AccentGreen
    }
}

/// The verb's colour role (R5.6). `WRITE` magenta, `READ` teal; the rest stay muted —
/// the design colours the two operations a reader scans for, and colouring all five
/// would spend the distinction it is making.
fn op_role(op: FileOp) -> ColorRole {
    match op {
        FileOp::Write => ColorRole::Magenta,
        FileOp::Read => ColorRole::Teal,
        _ => ColorRole::TextMuted,
    }
}

// ---- formatting helpers -------------------------------------------------------

/// The session's `/context` reading, if it has one and it still applies to the model
/// that is running now (`ai-monitor-context-window` D9).
///
/// A PREDICATE, evaluated here, not a mutation applied when the model changed. The
/// difference is observable: a session that goes opus → haiku → opus gets its reading
/// back, and it should, because the reading is a fact measured on opus and opus is what
/// is running again. Discarding it at the switch would lose it permanently.
///
/// The hole this closes is real and one-directional. Run `/context` on a 1M session,
/// then `/model` down to a 200k one, and last-wins has nothing newer to offer: the 1M
/// reading sets and the panel UNDER-reports — the one direction a tile whose job is to
/// warn "this is filling up" must not fail in.
///
/// Compared as a WHOLE STRING, never by tier. Which id belongs to which tier is exactly
/// the knowledge this spec established a log does not contain, so a tier comparison
/// would smuggle the guess back in. The price is that `claude-opus-5` →
/// `claude-opus-5-20260301` throws away a still-valid reading; it errs toward
/// over-reporting, which is the safe side.
///
/// Four cases, and the fourth is a known tension rather than a tidy rule:
///
/// | at read → now | | |
/// |---|---|---|
/// | `None` → `None` | use | a log that never named a model; the case above exists for it |
/// | `None` → `Some` | use | `None` at read is "no evidence yet", not "a different model" |
/// | `Some(a)` → `Some(b)` | refuse | a genuine switch |
/// | `Some(a)` → `None` | refuse | unreachable today, and NOT free if it ever is: with no model the fallback's `?` fires and the percentage vanishes entirely — which is the very failure the paragraph above exists to prevent. Whoever makes this arm reachable must revisit it together with that one. |
fn usable_window_reading(t: &TelemetrySnapshot) -> Option<u64> {
    let reading = t.context_window_reading.as_ref()?;
    let applies =
        reading.model_at_read.is_none() || reading.model_at_read.as_deref() == t.model.as_deref();
    applies.then_some(reading.tokens)
}

/// Context occupancy as a whole percentage, or `None` when it cannot be stated.
pub fn context_pct(t: &TelemetrySnapshot, env: &Env) -> Option<u64> {
    if t.context_used == 0 {
        return None;
    }
    // The configured window wins over the table AND over the deduction below: the
    // table can only guess from a model id that does not record the window, and a
    // deduction is still an inference. A user who has stated the window has knowledge
    // neither of them has, so nothing may overrule it — which is why this returns
    // before `promoted_window` is ever consulted.
    let window = match env.context_window {
        Some(w) => w,
        // Second: the window the session PRINTED, if it ever ran `/context`. A read
        // beats a guess, and it beats the high-water deduction too — but not the line
        // above, because the doctrine there is about knowledge the panel does not have,
        // and that is still true of a reading.
        //
        // It is also a TERMINAL value: `promoted_window` is not consulted afterwards,
        // and that is the opposite of how it first looked. The two numbers are not on
        // one axis. What `/context` prints is an auto-compact BUDGET (700,000, with the
        // 33k buffer as a slice INSIDE it — `33000/700000` displays as the `4.7%` the
        // panel shows); `WINDOW_TIERS` are raw API windows. Feeding 700,000 in as an
        // assumption would promote it to 1,000,000 the moment usage passed it — a
        // denominator `/context` never displays, produced at exactly the occupancy
        // where this tile matters most, and reached by replacing the one measured truth
        // with the guess this whole path exists to retire.
        None => match usable_window_reading(t) {
            Some(read) => read,
            // Assumed from the id, then corrected by measurement. The correction is inert
            // unless the session has itself proved the assumption wrong.
            //
            // Measured against the session HIGH-WATER, never the current reading. The window
            // is a property of the session; occupancy is a property of the newest turn, and
            // occupancy falls — a `/compact` took a real session from 517,389 to 53,268. Read
            // against `context_used` the deduction is amnesiac: it would re-derive a 200k
            // window from the smaller number and report 27% of a window that session had
            // already proven it was not on. Evidence of a window's size cannot expire with
            // the reading that produced it, because a window does not shrink mid-session.
            None => promoted_window(
                context_window(t.model.as_deref()?)?,
                t.context_high_water.max(t.context_used),
            ),
        },
    };
    if window == 0 {
        return None;
    }
    // Round-half-up without floats, then floor at 1: `0%` is reserved for "unknown"
    // (the tile omits the value and the gauge entirely there), so a displayed
    // percentage always means real, non-zero occupancy. Rounding a live gauge down to
    // the one value that elsewhere means "no reading" is the one ambiguity worth
    // spending a fraction of a percent to avoid.
    let pct = (t.context_used.saturating_mul(200) / window).div_ceil(2);
    Some(pct.max(1))
}

/// Bound a passthrough string before it enters the model. See [`TEXT_CAP`].
fn clip(s: &str) -> String {
    truncate_right(s, TEXT_CAP)
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
fn mark(ok: bool) -> char {
    if ok {
        OK
    } else {
        FAIL
    }
}

/// The operation label. Fixed vocabulary, five values wide at most, because the renderer
/// draws it in a fixed gutter.
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
        FileOp::Grep | FileOp::List => truncate_right(s, TEXT_CAP),
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
    if candidate.chars().count() <= TEXT_CAP {
        return candidate;
    }
    // Drop the parent before mangling the name; only then truncate the name itself,
    // from the left, so the extension survives.
    truncate_left(base, TEXT_CAP)
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
/// reading. The only case that reaches zero legitimately is a session with nothing
/// folded, which is exactly the empty state.
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
    use super::*;
    use ai_monitor::telemetry::{
        CompletedTool, ContextWindowReading, FileActivity, SessionTelemetry, SkillEntry,
    };
    use renderer::monitor_layout::layout;
    use std::fmt::Write as _;

    /// A default environment at `now`: attached (so nothing is detached) and no
    /// configured window, so every pre-existing expectation keeps its meaning.
    /// `tab_agent` is irrelevant while attached — it only selects between the two
    /// `NoLog` wordings — so it takes the same `false` the derived `Default` would.
    fn env(now: u64) -> Env {
        Env {
            now_ms: now,
            attached: true,
            context_window: None,
            tab_agent: None,
            utc_offset_secs: 0,
        }
    }

    /// The unattached environment, at `now`.
    fn unattached(now: u64) -> Env {
        Env {
            attached: false,
            ..env(now)
        }
    }

    /// The unattached, nothing-resolved environment the two `NoLog` wordings are keyed in.
    fn no_session(now: u64, tab_agent: Option<crate::tabmeta::AgentKind>) -> Env {
        Env {
            tab_agent,
            ..unattached(now)
        }
    }

    /// Every store filled **past** the height any panel could draw, at a fixed `now_ms`.
    /// Hand-built: no live log, no clock.
    fn full() -> (TelemetrySnapshot, u64) {
        let (mut t, now) = populated();
        // 6 tasks, deliberately out of order and with a tie on `updated_ms` so the
        // tie-break is exercised rather than assumed.
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
        t.skills = ["harness-ops:loop", "insta-review", "run-term", "specify"]
            .iter()
            .enumerate()
            .map(|(i, n)| SkillEntry {
                name: (*n).to_string(),
                start_ms: Some(now - (i as u64 + 1) * 240_000),
            })
            .collect();
        t.tools = (0..11)
            .map(|i| CompletedTool {
                name: ["Bash", "WebFetch", "TodoWrite", "Monitor"][i % 4].to_string(),
                ok: i != 1,
                duration_ms: match i {
                    2 => None,
                    // One call over R5.7's 30s boundary, so the pinned form carries all
                    // three duration colours and not only the two fast ones.
                    5 => Some(45_000),
                    _ => Some(120 + i as u64 * 640),
                },
                start_ms: Some(now - (i as u64 + 1) * 30_000),
            })
            .collect();
        // A path, a pattern and a failed write — plus one of each **over** `TEXT_CAP`, so
        // the pinned form carries the two truncation directions and a change to either
        // is a snapshot diff rather than a silent re-render (R3.5).
        t.files = vec![
            file(
                FileOp::Read,
                "/Users/x/proj/crates/renderer/atlas.rs",
                true,
                Some(430),
            ),
            file(FileOp::Grep, r"src/.*/mod\.rs", true, Some(1_240)),
            file(
                FileOp::Read,
                &format!("/Users/x/proj/crates/renderer/{}.rs", "glyph".repeat(24)),
                true,
                Some(88),
            ),
            file(FileOp::Grep, &"nested/dir/".repeat(12), true, Some(310)),
            file(
                FileOp::Write,
                "/Users/x/proj/crates/app/src/dashboard.rs",
                false,
                Some(62_000),
            ),
            file(FileOp::List, "**/*.toml", true, None),
            file(FileOp::Read, "Cargo.toml", true, Some(12)),
        ];
        // One live agent, the rest finished.
        t.subagents = vec![
            Subagent {
                id: "a1".into(),
                subagent_type: Some("general-purpose".into()),
                description: Some("Gate 3 checker".into()),
                model: Some("opus".into()),
                start_ms: Some(now - 90_000),
                end_ms: None,
                completed: false,
                agent_id: None,
                ended_ok: true,
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
        // The cumulative tally, in the NAME order the crate emits (D3) and holding more
        // names than TOOL USAGE ranks, with a tie astride the cut so the tie-break is
        // exercised rather than assumed. `WebFetch`/`WebSearch` both hold 3 and both fall
        // outside the top 9. The sum is honest: 247 named + 40 merged = 287 total.
        t.tool_calls = [
            ("Bash", 76),
            ("Edit", 27),
            ("Glob", 9),
            ("Grep", 31),
            ("Monitor", 4),
            ("Read", 58),
            ("Skill", 6),
            ("TodoWrite", 12),
            ("WebFetch", 3),
            ("WebSearch", 3),
            ("Write", 18),
        ]
        .iter()
        .map(|(n, c)| ((*n).to_string(), *c))
        .collect();
        t.tool_calls_other = 40;
        t.total_tool_calls = 287;
        t.failed_tool_calls = 3;
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
            agent_id: None,
            ended_ok: true,
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

    // ---- the projection the snapshots pin ------------------------------------

    /// A deterministic text projection of the model — what the `insta` snapshots pin
    /// (D11/R3.5).
    ///
    /// It renders exactly what this module *decides*: section order, what each section
    /// holds and in which order, truncation, colour roles, placeholders and wordings. It
    /// renders nothing the **renderer** decides — no pixel, no width, no allocator grant
    /// — and that is the line the migration draws. The old snapshots pinned 104-column
    /// geometry, which was only ever the app's business because the app could not see the
    /// width; what is left is composition policy, so a diff here is a diff in a decision.
    fn project(m: &MonitorModel) -> String {
        let mut o = String::new();
        let _ = writeln!(o, "{} [{}]", m.title, m.status.label());
        let s = &m.session;
        let _ = writeln!(
            o,
            "  session name={:?} id={:?} provider={:?} model={:?}",
            s.name, s.short_id, s.provider, s.model
        );
        let _ = writeln!(o, "  note {}", opt(s.note.as_deref()));
        for t in &m.tiles {
            let _ = writeln!(
                o,
                "  tile {:<8} {:>7} {:?} gauge={}",
                t.label,
                t.value,
                t.color,
                match t.gauge {
                    Some(g) => format!("{g:.2}"),
                    None => "none".to_string(),
                }
            );
        }
        for section in &m.sections {
            o.push_str(&project_section(section));
        }
        let _ = writeln!(o, "  footer {:?}", m.footer);
        o
    }

    /// One section of [`project`], also snapshotted on its own so a change to one
    /// section's policy is a one-section diff.
    fn project_section(s: &Section) -> String {
        let mut o = String::new();
        let _ = writeln!(o, "  {} (total {})", s.label, s.total);
        match &s.body {
            SectionBody::Empty(state) => {
                let _ = writeln!(o, "    empty {state:?}");
            }
            SectionBody::Cards(v) => {
                for c in v {
                    let _ = writeln!(
                        o,
                        "    card {:<12} {:?} value={:?} detail={}",
                        role(c.accent),
                        c.title,
                        c.value,
                        opt(c.detail.as_deref())
                    );
                }
            }
            SectionBody::Rows(v) => {
                for r in v {
                    let _ = writeln!(
                        o,
                        "    row  {:<19} {} {:?} value={:?} {:?}",
                        match &r.label {
                            Some(l) => format!("{} {:?}", l.text, l.color),
                            None => "—".to_string(),
                        },
                        r.glyph.unwrap_or('—'),
                        r.text,
                        r.value,
                        r.value_color
                    );
                }
            }
            SectionBody::Bars(v) => {
                for b in v {
                    let _ = writeln!(
                        o,
                        "    bar  {:<12} {:>5} {:.2}",
                        b.name, b.count, b.fraction
                    );
                }
            }
        }
        o
    }

    /// This module's production **code**, with its tests and every comment line removed.
    ///
    /// Two tests below assert that certain names do not appear in it. The comments are
    /// stripped because this file's register is to explain what broke before — the header
    /// names the deleted 104-column machinery precisely so nobody rebuilds it — and a
    /// grep that could not tell an explanation from a call would force the prose to stop
    /// naming what it is warning about.
    fn production_code() -> String {
        include_str!("dashboard.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn opt(s: Option<&str>) -> String {
        match s {
            Some(s) => format!("{s:?}"),
            None => "—".to_string(),
        }
    }

    fn role(r: Option<ColorRole>) -> String {
        match r {
            Some(r) => format!("{r:?}"),
            None => "—".to_string(),
        }
    }

    /// The named tile, or a failure naming what was actually built.
    fn tile_of<'a>(m: &'a MonitorModel, label: &str) -> &'a StatTile {
        m.tiles
            .iter()
            .find(|t| t.label == label)
            .unwrap_or_else(|| {
                let built: Vec<&str> = m.tiles.iter().map(|t| t.label.as_str()).collect();
                panic!("no {label} tile among {built:?}")
            })
    }

    /// The named section, or a failure naming what was actually built.
    fn section_of<'a>(m: &'a MonitorModel, label: &str) -> &'a Section {
        m.sections
            .iter()
            .find(|s| s.label == label)
            .unwrap_or_else(|| {
                let built: Vec<&str> = m.sections.iter().map(|s| s.label.as_str()).collect();
                panic!("no {label} section among {built:?}")
            })
    }

    // ---- the pinned forms (R3.5) ---------------------------------------------

    #[test]
    fn the_populated_panel_matches_its_pinned_form() {
        let (t, now) = full();
        insta::assert_snapshot!(project(&model(Some(&t), &env(now))));
    }

    #[test]
    fn the_empty_panel_matches_its_pinned_form() {
        insta::assert_snapshot!(project(&model(None, &env(1_785_226_000_000))));
    }

    #[test]
    fn the_detached_panel_matches_its_pinned_form() {
        // A resolved log this tab declined: every identity field empty, every section
        // empty, and one line saying which nothing this is.
        let (t, now) = full();
        insta::assert_snapshot!(project(&model(Some(&t), &unattached(now))));
    }

    // One snapshot per section, each store holding more than a short panel can draw, so
    // a change to a section's ordering, shape or wording shows up as a reviewable diff.
    // One test per section rather than six assertions in one: `insta` stops a test at its
    // first mismatch, so a single test would hide every section after the one that moved.

    #[test]
    fn tasks_at_their_truncation_boundary_are_pinned() {
        let (t, now) = full();
        insta::assert_snapshot!("boundary_tasks", project_section(&tasks(&t, &env(now))));
    }

    #[test]
    fn agents_at_their_truncation_boundary_are_pinned() {
        let (t, now) = full();
        insta::assert_snapshot!("boundary_agents", project_section(&agents(&t, &env(now))));
    }

    #[test]
    fn skills_at_their_truncation_boundary_are_pinned() {
        let (t, now) = full();
        insta::assert_snapshot!("boundary_skills", project_section(&skills(&t, &env(now))));
    }

    #[test]
    fn file_activity_at_its_truncation_boundary_is_pinned() {
        let (t, _) = full();
        insta::assert_snapshot!(
            "boundary_file_activity",
            project_section(&file_activity(&t))
        );
    }

    #[test]
    fn recent_calls_at_their_truncation_boundary_are_pinned() {
        let (t, _) = full();
        insta::assert_snapshot!("boundary_recent_calls", project_section(&recent_calls(&t)));
    }

    #[test]
    fn tool_usage_at_its_ranking_boundary_is_pinned() {
        let (t, _) = full();
        insta::assert_snapshot!("boundary_tool_usage", project_section(&tool_usage(&t)));
    }

    // ---- R3.2: purity --------------------------------------------------------

    #[test]
    fn r3_2_the_model_is_pure_and_reaches_for_no_clock_of_its_own() {
        let (t, now) = full();
        let m = model(Some(&t), &env(now));
        assert_eq!(m, model(Some(&t), &env(now)), "same inputs, equal models");
        // ... and not equal by accident: both injected values are genuinely read, so
        // "stable" cannot be passing because the model ignores its `Env`.
        assert_ne!(m, model(Some(&t), &env(now + 60_000)), "now_ms is read");
        let shifted = Env {
            utc_offset_secs: 32_400,
            ..env(now)
        };
        assert_ne!(m, model(Some(&t), &shifted), "utc_offset_secs is read");

        // The purity is structural, not incidental. A clock, a filesystem read or a
        // timezone lookup added here would still pass every assertion above (the two
        // calls are microseconds apart) and would flake the snapshots above on a UTC CI
        // runner or across a DST boundary — the exact failure D19 exists to prevent. So
        // the production half of this file is checked for the calls themselves.
        let production = production_code();
        for forbidden in [
            "SystemTime",
            "Instant::now",
            "std::fs",
            "std::env",
            "unsafe ",
            "static mut",
        ] {
            assert!(
                !production.contains(forbidden),
                "`{forbidden}` in dashboard.rs — the composer must stay a function of its Env (D19)"
            );
        }
    }

    #[test]
    fn an_absent_snapshot_builds_the_same_model_as_an_empty_one() {
        // "No log resolved" and "nothing folded yet" are one statement to a reader.
        // Never a blank panel.
        let now = 1_785_226_000_000;
        assert_eq!(
            model(None, &env(now)),
            model(Some(&TelemetrySnapshot::default()), &env(now))
        );
    }

    // ---- R3.3: the fixed-width machinery is gone ------------------------------

    #[test]
    fn r3_3_no_column_geometry_survives_in_this_module() {
        // The deleted mechanism, named: `RULE_COLS`, `fit_row` and the `rule*` family
        // existed only to compensate for not knowing the panel's width, and a docked
        // panel knows it. A helper that quietly reintroduced one would be invisible in a
        // diff of 2,000 lines, so the names are pinned as absent.
        let production = production_code();
        for gone in ["RULE_COLS", "fit_row", "fn rule", "rule_with", "rule_empty"] {
            assert!(
                !production.contains(gone),
                "`{gone}` is back in dashboard.rs"
            );
        }
    }

    // ---- composition policy ---------------------------------------------------

    #[test]
    fn the_section_order_is_the_designs_order() {
        // D18 drops headings from the BOTTOM, so this list is a priority order: what is
        // last is what a short panel loses.
        let (t, now) = full();
        let m = model(Some(&t), &env(now));
        let labels: Vec<&str> = m.sections.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "TASKS",
                "ACTIVE AGENTS",
                "SKILLS",
                "FILE ACTIVITY",
                "RECENT CALLS",
                "TOOL USAGE"
            ]
        );
    }

    #[test]
    fn every_section_is_present_however_empty_the_session_is() {
        // D10 — a region that vanished with its data would make "not built" and "no data"
        // indistinguishable, and the panel's shape would jump as a session warmed up.
        let (t, now) = full();
        let populated: Vec<String> = model(Some(&t), &env(now))
            .sections
            .iter()
            .map(|s| s.label.clone())
            .collect();
        for e in [
            model(None, &env(now)),
            model(
                None,
                &no_session(now, Some(crate::tabmeta::AgentKind::Claude)),
            ),
            model(Some(&t), &unattached(now)),
        ] {
            let labels: Vec<String> = e.sections.iter().map(|s| s.label.clone()).collect();
            assert_eq!(labels, populated);
        }
    }

    // ---- ai-monitor-panel-scroll T11 / R0.3 / R1.5 / R12.3: what did NOT change ----

    #[test]
    fn r1_5_the_tool_usage_cap_survives_the_feature_that_removed_every_other_limit() {
        // D21. D4 deleted the height allocator because its reason — rationing a fixed band —
        // stopped existing. `TOOL_BARS` looks like the same kind of limit and is not: its
        // reason is *readability*, that a bar list long enough to include a tool called twice
        // stops showing proportions at all. Unlimited height does not touch that argument, so
        // the cap stays and the difference it creates is explicitly NOT a ring loss (C10).
        let (t, now) = full();
        let m = model(Some(&t), &env(now));
        let usage = section_of(&m, "TOOL USAGE");
        assert!(
            usage.body.len() <= TOOL_BARS,
            "the readability cap must still bound the bars: {}",
            usage.body.len()
        );
        assert_eq!(usage.lost, 0, "and its cap is not reported as a loss");
    }

    #[test]
    fn r0_3_nothing_outside_the_rings_is_ever_recovered() {
        // D1 drew the scope line here: the panel makes what is IN MEMORY reachable, and does
        // not resurrect what the rings dropped. The composer has no filesystem, no clock of
        // its own and no second source — so the guarantee is simply that every section's body
        // is bounded by the snapshot it was handed.
        let (mut t, now) = full();
        t.total_tool_calls = 5_000; // a long session; the ring is far behind
        let m = model(Some(&t), &env(now));
        assert_eq!(
            section_of(&m, "RECENT CALLS").body.len(),
            t.tools.len(),
            "RECENT CALLS holds exactly the ring, never more"
        );
        assert_eq!(section_of(&m, "SKILLS").body.len(), t.skills.len());
        assert_eq!(section_of(&m, "FILE ACTIVITY").body.len(), t.files.len());
        // The loss is *reported*, which is D20 — and reporting is not recovering (C8).
        assert!(section_of(&m, "RECENT CALLS").lost > 0);
    }

    #[test]
    fn r12_3_long_text_still_truncates_horizontally_exactly_as_before() {
        // D28 kept the horizontal axis out of scope. A path too long for the panel is still
        // clipped with an ellipsis — this feature added vertical reach, not width.
        let long = "a".repeat(TEXT_CAP * 3);
        let out = clip(&long);
        assert_eq!(out.chars().count(), TEXT_CAP);
        assert!(out.ends_with('…'), "still an ellipsis, not a wrap: {out:?}");
    }

    // ---- ai-monitor-panel-scroll T6 / R9.2-R9.5: what the app knows about ring loss ----

    #[test]
    fn r9_2_recent_calls_reports_what_the_ring_dropped() {
        // D20 needs a session-cumulative count, and the crate keeps exactly one:
        // `total_tool_calls`, deliberately its own scalar rather than a sum over the bounded
        // tally. Without it the panel could not tell "32 calls happened" from "32 are all
        // that is left of 400", and D4's "everything is reachable" would quietly overclaim.
        let (mut t, now) = full();
        t.total_tool_calls = 400;
        let n = t.tools.len();
        assert!(
            n > 0 && (n as u64) < t.total_tool_calls,
            "premise: the ring is behind"
        );
        let m = model(Some(&t), &env(now));
        let s = section_of(&m, "RECENT CALLS");
        assert_eq!(s.total, n, "the mark still counts what is held");
        assert_eq!(s.lost, 400 - n, "and the loss is what is gone");
    }

    #[test]
    fn r9_5_only_recent_calls_claims_a_loss_because_only_it_can_know_one() {
        // C10. TOOL USAGE's `total` (tally names) exceeds its body (`TOOL_BARS` bars) for a
        // completely different reason — a readability cap D21 keeps, with the data still in
        // memory. The other rings have no cumulative counter at all, so `0` there means
        // "not known", not "none". Reporting either as loss re-creates the conflation
        // `+N more` was retired for.
        let (mut t, now) = full();
        t.total_tool_calls = 400;
        let m = model(Some(&t), &env(now));
        for label in [
            "TASKS",
            "ACTIVE AGENTS",
            "SKILLS",
            "FILE ACTIVITY",
            "TOOL USAGE",
        ] {
            assert_eq!(
                section_of(&m, label).lost,
                0,
                "{label} must not claim a loss it cannot measure"
            );
        }
        let usage = section_of(&m, "TOOL USAGE");
        assert!(
            usage.total >= usage.body.len(),
            "premise: the app cap really does hold something back"
        );
    }

    #[test]
    fn r9_3_a_session_shorter_than_the_ring_reports_no_loss() {
        // The everyday case: nothing has been evicted yet, so the heading must look exactly
        // as it does today. `saturating_sub` also covers the transient where the counter and
        // the ring are read a fold apart and the ring momentarily looks longer.
        let (mut t, now) = full();
        t.total_tool_calls = t.tools.len() as u64;
        assert_eq!(
            section_of(&model(Some(&t), &env(now)), "RECENT CALLS").lost,
            0
        );
        t.total_tool_calls = 0;
        assert_eq!(
            section_of(&model(Some(&t), &env(now)), "RECENT CALLS").lost,
            0
        );
    }

    #[test]
    fn r1_1_a_real_model_reaches_the_document_with_every_entry_it_holds() {
        // The end-to-end half of D4. `monitor_layout` proves the document holds everything
        // for a synthetic model; this proves the app hands over a model that actually has
        // the entries — the two halves used to be "the allocator rations rows" and "the app
        // does not cap what it sends", and only the first half changed.
        //
        // The panel height is deliberately varied: under the old allocator the drawn set
        // shrank with the panel, which is the behaviour D4 replaced with "laid out once,
        // reachable by scrolling". A short panel must lay out exactly as much as a tall one.
        let (t, now) = full();
        let mut m = model(Some(&t), &env(now));
        m.visible = true; // `overlay.rs` stamps this after `model` returns
        let cell = [8.0, 16.0];
        let entries: usize = m.sections.iter().map(|s| s.body.len()).sum();
        assert!(entries >= 6, "premise: the fixture populates the sections");

        let mut sizes = Vec::new();
        for h in [260.0f32, 1600.0] {
            let draw = layout(&m, [0.0, 0.0, 430.0, h], 1.0, cell);
            sizes.push(draw.items.len());
            let text = |s: &str| {
                draw.chrome
                    .runs
                    .iter()
                    .chain(draw.content.runs.iter())
                    .any(|r| r.text == s)
            };
            for (label, first) in [
                ("TASKS", "Fold usage per message id"),
                ("ACTIVE AGENTS", "● general-purpose"),
                ("SKILLS", "harness-ops:loop"),
                ("FILE ACTIVITY", "READ"),
                ("RECENT CALLS", "Bash"),
                ("TOOL USAGE", "Bash"),
            ] {
                assert!(text(label), "h={h}: {label} heading missing");
                assert!(text(first), "h={h}: {label} drew no row");
            }
            // R9.1: the marker that used to announce the rationing is gone with it. The
            // test is on the OLD mark's exact shape (`+N more`) rather than on the word,
            // because the fold control added later reads `▸ N more` — same word, opposite
            // meaning. The old one reported rows the allocator had discarded and offered no
            // way to them; the new one is the way to them. Matching the word alone would
            // make this assertion fire on the fix for the thing it is guarding against.
            assert!(
                !draw
                    .chrome
                    .runs
                    .iter()
                    .chain(draw.content.runs.iter())
                    .any(|r| r.text.starts_with('+') && r.text.ends_with("more")),
                "h={h}: `+N more` survived"
            );
        }
        assert_eq!(
            sizes[0], sizes[1],
            "the document is the same at every panel height — that is what D4 bought"
        );
        // One rung per heading, one per entry, and one per section long enough to offer a
        // fold. The fold rung exists in both states — a control that vanished when you
        // opened it would leave no way back — so this count does not depend on `collapsed`.
        let foldable = m.sections.iter().filter(|s| s.foldable()).count();
        assert!(
            foldable > 0,
            "premise: the fixture has a section worth folding"
        );
        assert_eq!(sizes[0], m.sections.len() + entries + foldable);
    }

    #[test]
    fn r13_1_a_shut_fold_hides_rows_that_one_toggle_brings_back() {
        // The fold's whole contract in one place: shutting a section removes rungs from the
        // document, and re-opening restores exactly what was removed. If these two ever
        // disagree, the panel has rows that no interaction can reach — which is the defect
        // `+N more` had and the reason this feature exists.
        let (t, now) = full();
        let mut m = model(Some(&t), &env(now));
        m.visible = true;
        let open = renderer::monitor_layout::item_ladder(&m).len();
        let n = m
            .sections
            .iter()
            .position(|s| s.foldable())
            .expect("the fixture has a foldable section");
        let hidden = m.sections[n].body.len() - renderer::monitor_model::FOLD_THRESHOLD;

        m.sections[n].collapsed = true;
        let shut = renderer::monitor_layout::item_ladder(&m).len();
        assert_eq!(
            open - shut,
            hidden,
            "a shut fold hides exactly what it says"
        );

        m.sections[n].collapsed = false;
        assert_eq!(
            renderer::monitor_layout::item_ladder(&m).len(),
            open,
            "re-opening restores the document it was folded from"
        );
    }

    #[test]
    fn an_empty_section_states_itself_on_its_heading_and_never_as_a_row() {
        // The wording rides the heading (the renderer draws it there): a row would
        // compete for the single row D18 guarantees a populated neighbour.
        let now = 1_785_226_000_000;
        let m = model(None, &env(now));
        for (label, wording) in [
            ("TASKS", "(no tasks)"),
            ("ACTIVE AGENTS", "(no agents yet)"),
            ("SKILLS", "(no skills yet)"),
            ("FILE ACTIVITY", "(no file activity)"),
            ("RECENT CALLS", "(no other tool calls)"),
            ("TOOL USAGE", "(no tool calls yet)"),
        ] {
            let s = section_of(&m, label);
            assert_eq!(s.total, 0);
            assert_eq!(
                s.body,
                SectionBody::Empty(wording.to_string()),
                "{label} lost its empty state"
            );
        }
    }

    #[test]
    fn a_section_reports_the_session_total_beside_what_it_sent() {
        // `total` is what the heading counts and what `+N more` subtracts from (R8.6), and
        // it is the field — not the vector's length — that the renderer reads. For the
        // five ring-backed sections nothing is capped app-side, so the two agree.
        let (t, now) = full();
        let m = model(Some(&t), &env(now));
        for (label, total) in [
            ("TASKS", t.tasks.len()),
            ("ACTIVE AGENTS", t.subagents.len()),
            ("SKILLS", t.skills.len()),
            ("FILE ACTIVITY", t.files.len()),
            ("RECENT CALLS", t.tools.len()),
        ] {
            let s = section_of(&m, label);
            assert_eq!(s.total, total, "{label}");
            assert_eq!(s.body.len(), total, "{label} sends the whole store");
        }
        // TOOL USAGE is the one section that caps (D12), so the two deliberately differ —
        // and `total` still counts every named tool, which is what makes `+2 more` on its
        // heading the truth rather than a restatement of the cap.
        let usage = section_of(&m, "TOOL USAGE");
        assert_eq!(usage.total, t.tool_calls.len(), "11 names in the tally");
        assert_eq!(usage.body.len(), TOOL_BARS);
    }

    #[test]
    fn tasks_sort_by_updated_desc_deterministically() {
        // The store is an unordered map; without a total order the pinned snapshots
        // would be flaky run to run (the hasher is seeded per process).
        let (t, now) = full();
        let titles = |t: &TelemetrySnapshot| match tasks(t, &env(now)).body {
            SectionBody::Cards(v) => v.iter().map(|c| c.title.clone()).collect::<Vec<_>>(),
            body => panic!("tasks are cards, got {body:?}"),
        };
        // t-1 and t-2 tie on `updated_ms`; t-1 has the later `created_ms` and wins.
        assert_eq!(
            titles(&t),
            vec![
                "Fold usage per message id",
                "Pin the worked examples",
                "Audit the dedupe keying",
                "Measure the model census",
                "Wire the overlay seam",
                "Draft the partition manifest",
            ]
        );
        // Order does not depend on the input order of an unordered store.
        let mut shuffled = t.clone();
        shuffled.tasks.reverse();
        assert_eq!(titles(&shuffled), titles(&t));
    }

    #[test]
    fn task_status_is_the_raw_string_and_only_completed_shows_a_duration() {
        // `blocked` is unobserved in the corpus and must still pass through intact
        // rather than being glyph-substituted or treated as terminal.
        let (t, now) = full();
        let cards = cards_of(tasks(&t, &env(now)));
        let detail = |i: usize| cards[i].detail.clone().unwrap_or_default();
        assert!(
            detail(0).starts_with("in_progress · updated 1m ago"),
            "{:?}",
            detail(0)
        );
        assert!(
            detail(2).starts_with("blocked · updated 2m ago"),
            "{:?}",
            detail(2)
        );
        // Only a completed task carries a duration, and it is the completion interval.
        assert_eq!(cards[0].value, "", "in_progress: no duration");
        assert_eq!(cards[1].value, "10m40s", "completed: created → updated");
        assert_eq!(cards[1].accent, Some(ColorRole::AccentGreen));
        assert_eq!(cards[0].accent, Some(ColorRole::Amber));
        assert_eq!(
            cards[2].accent,
            Some(ColorRole::TextFaint),
            "unknown status"
        );
    }

    #[test]
    fn a_live_agent_elapsed_grows_and_a_completed_one_is_frozen() {
        let (t, now) = full();
        let values = |now: u64| match agents(&t, &env(now)).body {
            SectionBody::Cards(v) => v.iter().map(|c| c.value.clone()).collect::<Vec<_>>(),
            body => panic!("agents are cards, got {body:?}"),
        };
        assert_eq!(values(now)[0], "1m30s", "live, measured from now_ms");
        assert_eq!(values(now)[1], "30s", "completed, measured from its end");
        assert_eq!(values(now + 600_000)[0], "11m30s", "the live one grew");
        assert_eq!(values(now + 600_000)[1], "30s", "the completed one did not");
    }

    /// One agent, launched and never ended — the state an async launch now leaves.
    fn launched_agent(start: u64) -> TelemetrySnapshot {
        TelemetrySnapshot {
            subagents: vec![Subagent {
                id: "a1".into(),
                subagent_type: Some("general-purpose".into()),
                description: Some("background sweep".into()),
                model: Some("opus".into()),
                start_ms: Some(start),
                end_ms: None,
                completed: false,
                agent_id: Some("agent-11".into()),
                ended_ok: true,
            }],
            ..TelemetrySnapshot::default()
        }
    }

    #[test]
    fn an_async_launched_agent_renders_live_and_climbing() {
        // R2.3 — the render half of "a launch is not an ending". Before this feature
        // the same log produced `◯ 0s` here, frozen 83ms after dispatch.
        let start = 1_787_097_600_000;
        let t = launched_agent(start);
        let card = |now: u64| match agents(&t, &env(now)).body {
            SectionBody::Cards(v) => v.into_iter().next().expect("one card"),
            body => panic!("agents are cards, got {body:?}"),
        };
        let c = card(start + 90_000);
        assert_eq!(c.value, "1m30s");
        assert!(c.title.starts_with(LIVE), "{}", c.title);
        assert_eq!(c.accent, Some(ColorRole::Amber));
        assert_eq!(card(start + 900_000).value, "15m00s", "it climbs");
    }

    #[test]
    fn a_never_notified_agent_stays_live_unclamped() {
        // R6.4 / R6.5 — roughly one async launch in eight never notifies (measured
        // 2026-08-19; the corpus is a rolling ~30-day window, so the exact ratio drifts
        // and only the magnitude is durable). The panel does not invent an ending for
        // them: clamping to the last event would put up a number the log never
        // asserted, which is the fabrication this feature removes.
        let start = 1_787_097_600_000;
        let t = launched_agent(start);
        let SectionBody::Cards(v) = agents(&t, &env(start + 31 * 3_600_000)).body else {
            panic!("agents are cards");
        };
        assert!(v[0].title.starts_with(LIVE));
        assert_eq!(v[0].accent, Some(ColorRole::Amber));
        assert_eq!(v[0].value, "31h00m", "hours format; u64 cannot overflow");
    }

    /// 2026-08-19T00:00:00.000Z — the launch the three lines below share.
    const AGENT_T0: u64 = 1_787_097_600_000;
    const LAUNCH_USE: &str = r#"{"type":"tool_use","id":"a1","name":"Agent","input":{"subagent_type":"general-purpose","description":"background sweep"},"timestamp":"2026-08-19T00:00:00.000Z"}"#;
    const LAUNCH_RECEIPT: &str = r#"{"type":"tool_result","tool_use_id":"a1","timestamp":"2026-08-19T00:00:00.083Z","content":"(receipt)","toolUseResult":{"isAsync":true,"agentId":"agent-11"}}"#;
    const NOTIFICATION: &str = r#"{"type":"user","timestamp":"2026-08-19T00:24:21.000Z","message":{"content":"<task-notification>\n<task-id>agent-11</task-id>\n<status>completed</status>\n</task-notification>"}}"#;

    /// Fold real JSONL, so these two assert the whole path rather than a hand-built
    /// snapshot that could agree with the renderer while disagreeing with the log.
    fn fold_lines(lines: &[&str]) -> TelemetrySnapshot {
        let mut t = SessionTelemetry::new();
        for l in lines {
            t.apply(l);
        }
        t.snapshot()
    }

    #[test]
    fn a_background_agent_reports_its_real_duration_end_to_end() {
        // R0.1 — the feature in one assertion. The ground truth is an agent that ran
        // 24m21s while this card read 0s.
        let t = fold_lines(&[LAUNCH_USE, LAUNCH_RECEIPT, NOTIFICATION]);
        let SectionBody::Cards(v) = agents(&t, &env(AGENT_T0 + 3_600_000)).body else {
            panic!("agents are cards");
        };
        assert_eq!(v[0].value, "24m21s", "never 0s");
        assert!(v[0].title.starts_with(DONE), "{}", v[0].title);
        assert_eq!(v[0].accent, Some(ColorRole::TextFaint));
    }

    #[test]
    fn a_launched_agent_is_live_until_its_notification_arrives() {
        // R0.2 — the same log one line short. This is the state the defect used to
        // render as a frozen `◯ 0s`, 83ms after dispatch.
        let t = fold_lines(&[LAUNCH_USE, LAUNCH_RECEIPT]);
        let SectionBody::Cards(v) = agents(&t, &env(AGENT_T0 + 300_000)).body else {
            panic!("agents are cards");
        };
        assert_eq!(v[0].value, "5m00s");
        assert!(v[0].title.starts_with(LIVE), "{}", v[0].title);
        assert_eq!(v[0].accent, Some(ColorRole::Amber));
    }

    /// A `<task-notification>` line for `task_id`, at `ts`.
    fn note_line(task_id: &str, status: &str, ts: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"{ts}","message":{{"content":"<task-notification>\n<task-id>{task_id}</task-id>\n<status>{status}</status>\n</task-notification>"}}}}"#
        )
    }

    /// The single card `agents()` produces for a folded transcript, at `now`.
    fn one_card(lines: &[&str], now: u64) -> Card {
        let t = fold_lines(lines);
        match agents(&t, &env(now)).body {
            SectionBody::Cards(v) => {
                assert_eq!(v.len(), 1, "expected exactly one card: {v:?}");
                v.into_iter().next().expect("one card")
            }
            body => panic!("agents are cards, got {body:?}"),
        }
    }

    #[test]
    fn a_renotified_card_reads_cumulative_time_alive() {
        // R5.3's render half — the second leg is 4m long, but the card promises time
        // ALIVE, so it must read 16m and not 4m.
        let first = note_line("agent-11", "completed", "2026-08-19T00:12:00.000Z");
        let second = note_line("agent-11", "completed", "2026-08-19T00:16:00.000Z");
        let c = one_card(
            &[LAUNCH_USE, LAUNCH_RECEIPT, &first, &second],
            AGENT_T0 + 7_200_000,
        );
        assert_eq!(c.value, "16m00s");
        assert!(c.title.starts_with(DONE));
    }

    #[test]
    fn a_fallback_completed_card_is_corrected_by_its_notification() {
        // R5.4's render half — the drift path, seen the way a user sees it. Both states
        // are `◯`; what changes is the number, from a lie to the truth.
        let fallback = r#"{"type":"tool_result","tool_use_id":"a1","timestamp":"2026-08-19T00:00:00.083Z","content":"(an unrecognised receipt)","toolUseResult":{"agentId":"agent-11"}}"#;
        let note = note_line("agent-11", "completed", "2026-08-19T00:24:21.000Z");
        let before = one_card(&[LAUNCH_USE, fallback], AGENT_T0 + 3_600_000);
        assert_eq!(before.value, "0s", "the defect, reproduced");
        assert!(before.title.starts_with(DONE));

        let after = one_card(&[LAUNCH_USE, fallback, &note], AGENT_T0 + 3_600_000);
        assert_eq!(after.value, "24m21s", "and corrected");
        assert!(after.title.starts_with(DONE));
    }

    #[test]
    fn a_sync_agents_card_grows_when_it_is_resumed() {
        // R5.5's render half — the Known Gap made visible: the same card reads `4s` and
        // then, hours later, `2h13m`. Surprising, intended, and asserted so that a
        // reader who finds it surprising can see it was a decision.
        let sync = r#"{"type":"tool_result","tool_use_id":"a1","timestamp":"2026-08-19T00:00:04.000Z","content":"Mapped the seam.","toolUseResult":{"status":"completed","agentId":"agent-11"}}"#;
        let note = note_line("agent-11", "completed", "2026-08-19T02:13:00.000Z");
        let before = one_card(&[LAUNCH_USE, sync], AGENT_T0 + 60_000);
        assert_eq!(before.value, "4s");

        let after = one_card(&[LAUNCH_USE, sync, &note], AGENT_T0 + 10_800_000);
        assert_eq!(after.value, "2h13m");
        assert!(after.title.starts_with(DONE), "the glyph never changed");
    }

    #[test]
    fn a_failed_card_returns_to_faint_when_the_resume_succeeds() {
        // R7.6's render half — `ended_ok` is last-wins like `end_ms`, so Rose is not a
        // one-way door.
        let failed = note_line("agent-11", "failed", "2026-08-19T00:12:00.000Z");
        let fixed = note_line("agent-11", "completed", "2026-08-19T00:16:00.000Z");
        let c1 = one_card(&[LAUNCH_USE, LAUNCH_RECEIPT, &failed], AGENT_T0 + 7_200_000);
        assert_eq!(c1.accent, Some(ColorRole::Rose));

        let c2 = one_card(
            &[LAUNCH_USE, LAUNCH_RECEIPT, &failed, &fixed],
            AGENT_T0 + 7_200_000,
        );
        assert_eq!(c2.accent, Some(ColorRole::TextFaint));
        assert_eq!(c2.value, "16m00s");
    }

    #[test]
    fn a_resumed_agents_card_stays_frozen_through_the_resume_window() {
        // R8.1's render half — the declared gap. The `SendMessage` below is the resume:
        // it is in the transcript, the fold does not inspect it, and the card therefore
        // says DONE while the agent is running again. Folding it is what makes this
        // test discriminate — without it the assertion is just R5.1 restated.
        let note = note_line("agent-11", "completed", "2026-08-19T00:12:00.000Z");
        let resume = r#"{"type":"tool_use","id":"s1","name":"SendMessage","input":{"to":"agent-11","message":"keep going"},"timestamp":"2026-08-19T00:13:00.000Z"}"#;
        let resume_res = r#"{"type":"tool_result","tool_use_id":"s1","timestamp":"2026-08-19T00:13:00.200Z","content":"delivered"}"#;
        let c = one_card(
            &[LAUNCH_USE, LAUNCH_RECEIPT, &note, resume, resume_res],
            AGENT_T0 + 7_200_000,
        );
        assert!(c.title.starts_with(DONE));
        assert_eq!(c.value, "12m00s", "frozen, though two hours have passed");
        assert_eq!(c.accent, Some(ColorRole::TextFaint));
    }

    #[test]
    fn a_failed_end_colours_the_accent_not_the_glyph() {
        // R7.4 — the section's `●`/`◯` vocabulary means live-or-not and nothing else,
        // so the failure rides the accent bar. The elapsed still freezes at the
        // notification: a failed run took as long as it took.
        let failed = r#"{"type":"user","timestamp":"2026-08-19T00:24:21.000Z","message":{"content":"<task-notification>\n<task-id>agent-11</task-id>\n<status>failed</status>\n</task-notification>"}}"#;
        let t = fold_lines(&[LAUNCH_USE, LAUNCH_RECEIPT, failed]);
        let SectionBody::Cards(v) = agents(&t, &env(AGENT_T0 + 3_600_000)).body else {
            panic!("agents are cards");
        };
        assert_eq!(v[0].accent, Some(ColorRole::Rose));
        assert!(v[0].title.starts_with(DONE), "the glyph is unchanged");
        assert_eq!(
            v[0].value, "24m21s",
            "a failed run's duration is still real"
        );
    }

    #[test]
    fn nothing_unobserved_is_ever_rose() {
        // R7.5 — three agents no notification ever described: one in flight, one
        // completed synchronously, one completed by the unrecognised-result fallback.
        //
        // Note precisely what this does and does not discriminate. All three carry
        // `ended_ok == true` — the in-flight one because nothing has observed its end,
        // and the other two because their results carry no `is_error`, which is how a
        // success is written. So an ungated `!ended_ok -> Rose` rule would not paint them
        // either. What is pinned here is the other half of the gate: `completed` alone
        // must never imply Rose. The `ended_ok == false` side is pinned separately below,
        // because the fold cannot reach `completed == false` with `ended_ok == false` at
        // all.
        //
        // "Sync completions are never Rose" is NOT what this asserts, and must not be
        // read that way: a sync completion whose result DID carry `is_error: true` is a
        // real failure and is Rose — see `a_failed_sync_completion_is_rose`.
        let sync_use = r#"{"type":"tool_use","id":"a2","name":"Agent","input":{"subagent_type":"sync"},"timestamp":"2026-08-19T00:00:00.000Z"}"#;
        let sync_res = r#"{"type":"tool_result","tool_use_id":"a2","timestamp":"2026-08-19T00:00:04.000Z","content":"Done.","toolUseResult":{"agentId":"agent-sync"}}"#;
        let fallback_use = r#"{"type":"tool_use","id":"a3","name":"Agent","input":{"subagent_type":"fallback"},"timestamp":"2026-08-19T00:00:00.000Z"}"#;
        let fallback_res = r#"{"type":"tool_result","tool_use_id":"a3","timestamp":"2026-08-19T00:00:09.000Z","content":"(a shape nobody has seen)","toolUseResult":{"someNewKey":true}}"#;
        let t = fold_lines(&[
            LAUNCH_USE,
            LAUNCH_RECEIPT,
            sync_use,
            sync_res,
            fallback_use,
            fallback_res,
        ]);
        let SectionBody::Cards(v) = agents(&t, &env(AGENT_T0 + 300_000)).body else {
            panic!("agents are cards");
        };
        let accent = |kind: &str| {
            v.iter()
                .find(|c| c.title.contains(kind))
                .unwrap_or_else(|| panic!("no card for {kind}: {v:?}"))
                .accent
        };
        assert_eq!(
            accent("general-purpose"),
            Some(ColorRole::Amber),
            "in flight"
        );
        assert_eq!(
            accent("sync"),
            Some(ColorRole::TextFaint),
            "sync completion"
        );
        assert_eq!(accent("fallback"), Some(ColorRole::TextFaint), "fallback");
        assert!(
            v.iter().all(|c| c.accent != Some(ColorRole::Rose)),
            "nothing here failed: {v:?}"
        );
    }

    #[test]
    fn a_failed_sync_completion_is_rose() {
        // R7.5a — the arm R7.5 used to exempt wholesale. A synchronously completed
        // agent whose result carried `is_error: true` ended, and ended badly, so it gets
        // the failure colour like any other bad ending. No notification is involved.
        //
        // Catches a regression to "only a notified failure is ever Rose": with
        // `ended_ok` left at its opening `true`, this card renders TextFaint — visually
        // identical to the clean sync completion in the test above.
        let use_line = r#"{"type":"tool_use","id":"a2","name":"Agent","input":{"subagent_type":"sync-failed"},"timestamp":"2026-08-19T00:00:00.000Z"}"#;
        let res_line = r#"{"type":"tool_result","tool_use_id":"a2","is_error":true,"timestamp":"2026-08-19T00:00:04.000Z","content":"Agent failed: terminal API error","toolUseResult":{"agentId":"agent-sync"}}"#;
        let t = fold_lines(&[use_line, res_line]);
        let SectionBody::Cards(v) = agents(&t, &env(AGENT_T0 + 300_000)).body else {
            panic!("agents are cards");
        };
        assert_eq!(v[0].accent, Some(ColorRole::Rose));
        assert!(v[0].title.starts_with(DONE), "the glyph is unchanged");
        assert_eq!(v[0].value, "4s", "a failed run's duration is still real");
    }

    #[test]
    fn liveness_outranks_ended_ok_in_the_accent() {
        // R7.5's unreachable corner, pinned by hand because the fold cannot produce it:
        // every writer of `ended_ok` — the notification branch and the non-launch `Agent`
        // result — sets `completed` in the same breath, so `(completed: false, ended_ok:
        // false)` never occurs in practice.
        // Without this, `(false, _) => Amber` could be rewritten as
        // `(false, false) => Rose` and the entire suite would still pass.
        //
        // The rule it fixes: an agent that is still running is not a failure, whatever
        // else its fields say. Rose means "it ended, and it ended badly".
        let start = 1_787_097_600_000;
        let mut t = launched_agent(start);
        t.subagents[0].ended_ok = false;
        let SectionBody::Cards(v) = agents(&t, &env(start + 90_000)).body else {
            panic!("agents are cards");
        };
        assert_eq!(
            v[0].accent,
            Some(ColorRole::Amber),
            "still live, not failed"
        );
        assert!(v[0].title.starts_with(LIVE));
    }

    #[test]
    fn agent_cards_carry_type_description_and_model() {
        let (t, now) = full();
        let SectionBody::Cards(cards) = agents(&t, &env(now)).body else {
            panic!("agents are cards");
        };
        assert_eq!(cards[0].title, "● general-purpose");
        assert_eq!(cards[0].detail.as_deref(), Some("Gate 3 checker · opus"));
        assert_eq!(cards[1].title, "◯ Explore");
        // Flat: there is no parent linkage in the data, so a connector would draw a
        // hierarchy that was never observed.
        for c in &cards {
            assert!(!c.title.contains('├') && !c.title.contains('└'), "{c:?}");
        }
    }

    #[test]
    fn the_two_marker_pairs_are_never_conflated() {
        // Outcome is ✓/✗; liveness is ●/◯. The fold records no agent outcome, so ✓ on an
        // agent card would assert something the data lacks.
        let (t, now) = full();
        let m = model(Some(&t), &env(now));
        let calls = project_section(section_of(&m, "RECENT CALLS"));
        assert!(calls.contains(OK) && calls.contains(FAIL), "{calls}");
        assert!(!calls.contains(LIVE) && !calls.contains(DONE), "{calls}");

        let agents = project_section(section_of(&m, "ACTIVE AGENTS"));
        assert!(agents.contains(LIVE) && agents.contains(DONE), "{agents}");
        assert!(!agents.contains(OK) && !agents.contains(FAIL), "{agents}");
    }

    #[test]
    fn skills_show_when_they_ran_and_never_a_duration() {
        // A Skill pair measures dispatch latency, not execution.
        let (t, now) = full();
        let rows = rows_of(skills(&t, &env(now)));
        assert_eq!(rows[0].text, "harness-ops:loop");
        assert!(!rows[0].value.contains("ms"), "{:?}", rows[0].value);
        assert!(rows.iter().all(|r| r.label.is_none() && r.glyph.is_none()));
    }

    #[test]
    fn r5_9_skills_show_a_local_absolute_time() {
        // D7/D19 — the epoch below is 07:21:18 UTC, so +32400 reads 16:21:18. The offset
        // is injected, which is what lets an absolute time be pinned in a snapshot at all.
        let t = TelemetrySnapshot {
            skills: vec![SkillEntry {
                name: "specify".into(),
                start_ms: Some(1_785_223_278_000),
            }],
            ..TelemetrySnapshot::default()
        };
        let kst = Env {
            utc_offset_secs: 32_400,
            ..env(1_785_226_000_000)
        };
        assert_eq!(rows_of(skills(&t, &kst))[0].value, "at 16:21:18");
        assert_eq!(
            rows_of(skills(&t, &env(1_785_226_000_000)))[0].value,
            "at 07:21:18",
            "the same instant, a different zone"
        );
        // Never the relative form this section used to carry: under D2 nothing repaints,
        // so an age freezes and starts lying while an absolute time stays correct.
        assert!(!rows_of(skills(&t, &kst))[0].value.contains("ago"));
        // A skill with no start time states nothing rather than inventing midnight UTC.
        let undated = TelemetrySnapshot {
            skills: vec![SkillEntry {
                name: "specify".into(),
                start_ms: None,
            }],
            ..TelemetrySnapshot::default()
        };
        assert_eq!(rows_of(skills(&undated, &kst))[0].value, "");
    }

    #[test]
    fn r5_7_call_duration_colour_is_bucketed_by_magnitude() {
        // The requirement's three worked values, plus both boundaries and the two cases
        // that are not a magnitude at all.
        let t = TelemetrySnapshot {
            tools: vec![
                tool("Bash", true, Some(32_800)),
                tool("Grep", true, Some(1_800)),
                tool("Read", true, Some(139)),
                tool("Edit", false, Some(139)),
                tool("Glob", true, None),
            ],
            ..TelemetrySnapshot::default()
        };
        let roles: Vec<ColorRole> = rows_of(recent_calls(&t))
            .iter()
            .map(|r| r.value_color)
            .collect();
        assert_eq!(
            roles,
            vec![
                ColorRole::Amber,       // 32.8s — you noticed this one
                ColorRole::TextMuted,   // 1.8s  — ordinary
                ColorRole::AccentGreen, // 139ms — instant
                ColorRole::Rose,        // failed: the outcome outranks the magnitude
                ColorRole::TextMuted,   // no duration: nothing to bucket, never "fast"
            ]
        );
        // The boundaries themselves, since a `>` where a `>=` belongs is invisible in a
        // colour.
        assert_eq!(dur_role(30_000), ColorRole::Amber);
        assert_eq!(dur_role(29_999), ColorRole::TextMuted);
        assert_eq!(dur_role(1_000), ColorRole::TextMuted);
        assert_eq!(dur_role(999), ColorRole::AccentGreen);
        assert_eq!(dur_role(0), ColorRole::AccentGreen);
    }

    #[test]
    fn r5_8_tool_usage_ranks_named_tools_and_normalises_to_the_top_one() {
        // The requirement's worked case: `other` is in neither the list nor the
        // denominator, or `bash` would be a sliver of a bucket that names nothing.
        let t = TelemetrySnapshot {
            tool_calls: vec![("bash".into(), 76), ("read".into(), 27)],
            tool_calls_other: 900,
            total_tool_calls: 1003,
            ..TelemetrySnapshot::default()
        };
        let bars = bars_of(tool_usage(&t));
        assert_eq!(
            bars.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            vec!["bash", "read"]
        );
        assert_eq!(bars[0].fraction, 1.0, "the top named tool is full width");
        assert!(
            (bars[1].fraction - 27.0 / 76.0).abs() < 1e-6,
            "{}",
            bars[1].fraction
        );
        assert_eq!(
            bars[0].count, 76,
            "the count is the tally's, not a proportion"
        );
    }

    #[test]
    fn tool_usage_draws_the_top_nine_named_tools_and_breaks_ties_by_name() {
        let (t, _) = full();
        let bars = bars_of(tool_usage(&t));
        assert_eq!(
            bars.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            vec![
                "Bash",      // 76
                "Read",      // 58
                "Grep",      // 31
                "Edit",      // 27
                "Write",     // 18
                "TodoWrite", // 12
                "Glob",      // 9
                "Skill",     // 6
                "Monitor",   // 4
            ],
            "count descending — the crate emits this list in NAME order"
        );
        // The two tied at 3 fall outside the cut, and `total` still counts them, so the
        // heading can say `+2 more` rather than pretending the tally holds nine names.
        assert_eq!(tool_usage(&t).total, 11);
        // Ranking is stable under the tie: two tools level on count sort by name, so a
        // fixture reordering cannot flip the snapshot.
        let mut tied = t.clone();
        tied.tool_calls = vec![("zeta".into(), 5), ("alpha".into(), 5), ("mid".into(), 5)];
        assert_eq!(
            bars_of(tool_usage(&tied))
                .iter()
                .map(|b| b.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "mid", "zeta"]
        );
    }

    #[test]
    fn an_empty_tally_normalises_to_nothing_instead_of_dividing_by_zero() {
        // The bucket alone is a real state: every name a short session used can have been
        // merged away only after 64 of them, but a snapshot arriving mid-fold can carry
        // the scalar with an empty map.
        let t = TelemetrySnapshot {
            tool_calls_other: 12,
            total_tool_calls: 12,
            ..TelemetrySnapshot::default()
        };
        let s = tool_usage(&t);
        assert_eq!(s.total, 0);
        assert_eq!(
            s.body,
            SectionBody::Empty("(no tool calls yet)".to_string())
        );
    }

    /// The rows of a `Rows` section, or a failure naming what it actually held.
    fn rows_of(s: Section) -> Vec<Row> {
        match s.body {
            SectionBody::Rows(v) => v,
            body => panic!("{} is not a row section: {body:?}", s.label),
        }
    }

    /// The cards of a `Cards` section, or a failure naming what it actually held.
    fn cards_of(s: Section) -> Vec<Card> {
        match s.body {
            SectionBody::Cards(v) => v,
            body => panic!("{} is not a card section: {body:?}", s.label),
        }
    }

    /// The bars of a `Bars` section, or a failure naming what it actually held.
    fn bars_of(s: Section) -> Vec<Bar> {
        match s.body {
            SectionBody::Bars(v) => v,
            body => panic!("{} is not a bar section: {body:?}", s.label),
        }
    }

    fn tool(name: &str, ok: bool, duration_ms: Option<u64>) -> CompletedTool {
        CompletedTool {
            name: name.to_string(),
            ok,
            duration_ms,
            start_ms: None,
        }
    }

    /// The first row whose verb reads `verb` — by label, never by index, so adding a
    /// fixture row cannot silently re-point an assertion at a different operation.
    fn row_with_verb(rows: &[Row], verb: &str) -> Row {
        rows.iter()
            .find(|r| r.label.as_ref().is_some_and(|l| l.text == verb))
            .unwrap_or_else(|| panic!("no {verb} row"))
            .clone()
    }

    #[test]
    fn r5_6_the_file_verb_carries_the_designs_role() {
        // WRITE magenta, READ teal; the operations a reader scans for are the ones
        // coloured.
        let (t, _) = full();
        let rows = rows_of(file_activity(&t));
        assert_eq!(
            row_with_verb(&rows, "READ").label.expect("verb").color,
            ColorRole::Teal
        );
        assert_eq!(
            row_with_verb(&rows, "WRITE").label.expect("verb").color,
            ColorRole::Magenta
        );
        assert_eq!(
            row_with_verb(&rows, "GREP").label.expect("verb").color,
            ColorRole::TextMuted,
            "the design colours two operations, not five"
        );
        assert!(
            rows.iter().all(|r| r.label.is_some()),
            "every file row has a verb"
        );
    }

    #[test]
    fn a_failed_call_colours_its_mark_and_its_duration_together() {
        // `Row::value_color` drives both, so a `✗` in the duration's colour would
        // announce a failure in the palette of a fast call.
        let (t, _) = full();
        let rows = rows_of(file_activity(&t));
        let failed = row_with_verb(&rows, "WRITE");
        assert_eq!(failed.glyph, Some(FAIL));
        assert_eq!(failed.value_color, ColorRole::Rose);
        let ok = row_with_verb(&rows, "READ");
        assert_eq!(ok.glyph, Some(OK));
        assert_ne!(ok.value_color, ColorRole::Rose);
    }

    #[test]
    fn a_missing_duration_renders_the_placeholder() {
        let (t, _) = full();
        for body in [file_activity(&t).body, recent_calls(&t).body] {
            let SectionBody::Rows(rows) = body else {
                panic!("rows expected");
            };
            assert!(
                rows.iter().any(|r| r.value == DASH),
                "a call with no derivable duration must not read 0ms"
            );
        }
    }

    #[test]
    fn paths_keep_the_filename_and_one_parent() {
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
    fn an_over_long_basename_truncates_from_the_left() {
        // Left, so the extension survives; right-truncation would clip the one part of a
        // path worth reading.
        let long = format!("/a/b/{}.rs", "x".repeat(120));
        let out = short_path(&long);
        assert_eq!(out.chars().count(), TEXT_CAP);
        assert!(out.starts_with('…') && out.ends_with(".rs"), "{out}");
    }

    #[test]
    fn grep_and_glob_render_their_pattern_not_a_path() {
        // Path-shortening a regex mangles one containing `/`.
        assert_eq!(
            file_target(FileOp::Grep, r"src/.*/mod\.rs"),
            r"src/.*/mod\.rs"
        );
        assert_eq!(file_target(FileOp::List, "**/*.toml"), "**/*.toml");
        // ... and the head is what survives truncation, since that is a pattern's
        // discriminating part.
        let long = "a/".repeat(80);
        let out = file_target(FileOp::Grep, &long);
        assert_eq!(out.chars().count(), TEXT_CAP);
        assert!(out.starts_with("a/a/") && out.ends_with('…'), "{out}");
    }

    #[test]
    fn no_model_string_is_unbounded_on_hostile_telemetry() {
        // The passthrough fields — a task's subject and status, an agent's type,
        // description, model and agent id, a tool's name, a file's path — are raw strings
        // the crate never bounds. The renderer truncates for the panel's width, but an
        // unbounded one would still be cloned into the model and compared field by field
        // on every frame by the R7.3 shaping cache.
        //
        // `agent_id` reaches no string in the model today — it is a join key, never
        // rendered — so it is loaded here purely as a tripwire: the day someone surfaces
        // it on a card, this assertion fails instead of the panel.
        let now = 1_785_226_000_000;
        let t = TelemetrySnapshot {
            model: Some("m".repeat(400)),
            tasks: vec![task("t", &"s".repeat(300), &"x".repeat(300), 0, 0)],
            subagents: vec![Subagent {
                id: "a".into(),
                subagent_type: Some("y".repeat(300)),
                description: Some("d".repeat(300)),
                model: Some("m".repeat(300)),
                start_ms: Some(0),
                end_ms: None,
                completed: false,
                agent_id: Some("g".repeat(300)),
                ended_ok: true,
            }],
            files: vec![file(FileOp::Read, &"p".repeat(400), true, Some(u64::MAX))],
            tools: vec![CompletedTool {
                name: "n".repeat(300),
                ok: true,
                duration_ms: Some(u64::MAX),
                start_ms: None,
            }],
            skills: vec![SkillEntry {
                name: "k".repeat(300),
                start_ms: Some(0),
            }],
            ..TelemetrySnapshot::default()
        };
        let m = model(Some(&t), &env(now));
        // A generous bound on a *composed* string: the longest is a card detail, which is
        // a clipped passthrough plus a short suffix.
        let limit = TEXT_CAP * 2 + 32;
        for (what, s) in strings(&m) {
            assert!(
                s.chars().count() <= limit,
                "{what} carries {} chars: {s}",
                s.chars().count()
            );
        }
    }

    /// Every variable-length string the model carries, with a label naming where it came
    /// from — so a failure says which field grew, not merely that one did.
    fn strings(m: &MonitorModel) -> Vec<(String, String)> {
        let mut out = vec![
            ("title".into(), m.title.clone()),
            ("session.model".into(), m.session.model.clone()),
            ("footer".into(), m.footer.clone()),
        ];
        out.extend(m.session.note.iter().map(|n| ("note".into(), n.clone())));
        out.extend(
            m.tiles
                .iter()
                .map(|t| (format!("tile {}", t.label), t.value.clone())),
        );
        for s in &m.sections {
            let at = |what: &str| format!("{} {what}", s.label);
            match &s.body {
                SectionBody::Empty(state) => out.push((at("empty"), state.clone())),
                SectionBody::Cards(v) => {
                    for c in v {
                        out.push((at("card.title"), c.title.clone()));
                        out.push((at("card.value"), c.value.clone()));
                        out.extend(c.detail.iter().map(|d| (at("card.detail"), d.clone())));
                    }
                }
                SectionBody::Rows(v) => {
                    for r in v {
                        out.push((at("row.text"), r.text.clone()));
                        out.push((at("row.value"), r.value.clone()));
                        out.extend(r.label.iter().map(|l| (at("row.label"), l.text.clone())));
                    }
                }
                SectionBody::Bars(v) => {
                    for b in v {
                        out.push((at("bar.name"), b.name.clone()));
                    }
                }
            }
        }
        out
    }

    // ---- formatting ----------------------------------------------------------

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

    // ---- attachment rendering ------------------------------------------------

    #[test]
    fn an_unattached_snapshot_shows_none_of_its_data() {
        // Before attaching, whatever the resolver happened to find must not be presented
        // as this tab's session.
        let (t, now) = full();
        let out = project(&model(Some(&t), &unattached(now)));
        assert!(out.contains("(no active session"), "{out}");
        assert!(!out.contains("claude-opus-5"), "no model leak:\n{out}");
        assert!(!out.contains("19.3K"), "no spend leak:\n{out}");
        assert!(!out.contains("45m12s"), "no uptime leak:\n{out}");
        assert!(out.contains("(no tasks)") && out.contains("(no agents yet)"));
    }

    #[test]
    fn an_attached_session_is_never_blanked_however_stale_it_looks() {
        // THE requirement: data is preserved from session start to session end. Once
        // attached, no property of the snapshot may empty the panel — this is why the
        // freshness question is answered once, in `overlay.rs`, and not re-asked here.
        let (mut t, now) = full();
        // As stale as it gets: the last event at the epoch, and no event times at all.
        for last in [Some(0), None] {
            t.last_event_ms = last;
            t.first_event_ms = last;
            let out = project(&model(Some(&t), &env(now)));
            assert!(!out.contains("no active session"), "{out}");
            assert!(out.contains("claude-opus-5"), "data intact:\n{out}");
            assert!(out.contains("Fold usage per message id"), "tasks intact");
            assert!(out.contains("harness-ops:loop"), "skills intact");
            assert!(out.contains("1.9M"), "spend intact");
        }
    }

    #[test]
    fn the_unattached_row_reports_which_case_it_is() {
        let (mut t, now) = populated();
        let note = |t: &TelemetrySnapshot| {
            model(Some(t), &unattached(now))
                .session
                .note
                .expect("a detached session row states which nothing this is")
        };
        // A log that went quiet: say how long ago.
        t.last_event_ms = Some(now - 110 * 60 * 1000);
        assert_eq!(note(&t), "(no active session — newest log idle 1h50m)");
        // A log with content but no clock: say that instead of inventing an age.
        t.last_event_ms = None;
        assert_eq!(note(&t), "(no active session — newest log is undated)");
        // No log at all: claim nothing about a log.
        assert_eq!(
            note(&TelemetrySnapshot::default()),
            "(no Claude session in this tab)"
        );
    }

    #[test]
    fn a_tab_with_no_agent_process_is_told_exactly_that() {
        // The everyday case: a tab that simply is not running an AI agent at all.
        let out = project(&model(None, &no_session(1_785_226_000_000, None)));
        assert!(out.contains("(no Claude session in this tab)"), "{out}");
        assert!(!out.contains("has not written a log"), "{out}");
    }

    #[test]
    fn a_tab_running_claude_with_nothing_confirmed_is_told_the_other_thing() {
        // The reported case: `claude` is running in this tab and the user can watch it,
        // but no log in the project directory can be confirmed as its own. "(no Claude
        // session in this tab)" here would read as a bug, which is why the wording is
        // keyed on process presence rather than on the hint's usability.
        let out = project(&model(
            None,
            &no_session(1_785_226_000_000, Some(crate::tabmeta::AgentKind::Claude)),
        ));
        assert!(
            out.contains("(this tab's session has not written a log yet)"),
            "{out}"
        );
        assert!(!out.contains("(no Claude session in this tab)"), "{out}");
    }

    #[test]
    fn nothing_resolved_leaks_nothing_from_the_session_that_was_declined() {
        // The goal-level assertion, and the whole reason this feature exists. The foreign
        // values are taken from a REAL snapshot rather than written out as literals, so
        // the test cannot pass by asserting the absence of strings this module never
        // emits: every one of these appears when that snapshot is composed attached
        // (asserted below), and none may appear when strict resolution answered `None`.
        let (foreign, now) = populated();
        let leaked = project(&model(Some(&foreign), &env(now)));
        let markers = [
            "5e59b629",      // session id, as the row truncates it
            "claude-opus-5", // model
            "19.3K",         // spend (output)
            "1.9M",          // spend (cache)
            "59%",           // occupancy
            "45m12s",        // uptime
        ];
        for m in markers {
            assert!(leaked.contains(m), "fixture no longer emits {m}:\n{leaked}");
        }

        use crate::tabmeta::AgentKind::*;
        for (tab_agent, wording) in [
            (None, "(no Claude session in this tab)"),
            (
                Some(Claude),
                "(this tab's session has not written a log yet)",
            ),
            // AG10/R5.1: an `agy` tab gets the waiting wording, not a claim about Claude.
            (
                Some(Antigravity),
                "(this tab's session has not written a log yet)",
            ),
            // An agent the monitor does not read must not be described as pending.
            (Some(Codex), "(no Claude session in this tab)"),
        ] {
            let out = project(&model(None, &no_session(now, tab_agent)));
            for m in markers {
                assert!(
                    !out.contains(m),
                    "{m} leaked from a foreign session:\n{out}"
                );
            }
            // ... and it is not merely blank: the row says which nothing this is.
            assert!(out.contains(wording), "{tab_agent:?} -> {out}");
        }
    }

    // ---- ai-monitor-antigravity T6: the Antigravity panel's own pinned form ----

    /// A whole Antigravity panel, pinned (AG4/AG10).
    ///
    /// A NEW snapshot rather than a change to an existing one, which is the shape AG10's
    /// split was aiming for: the three Claude panels stay byte-identical (R5.6 proved that in
    /// T5 and still holds), and the presentation this feature adds gets its own baseline
    /// instead of being reviewed as a diff against Claude's.
    ///
    /// What to look for when reviewing the accepted output:
    /// * `provider="[Antigravity]"` — the agent name, in the tag that already existed.
    /// * `tile TURNS 3·4` — real data, in the slot IN·OUT would have held.
    /// * `tile CACHE —` and `tile CONTEXT —` with **`gauge=none`** — the point of R5.5. Not
    ///   `0` and not `gauge=0`, because this agent reports no tokens at all and a zero would
    ///   assert a measurement that was never taken.
    /// * `short_id="4de5fc70"` — an id the log never states. It is the `brain/<id>/`
    ///   directory's name, recovered from the path, and it renders through the same
    ///   eight-character clip as Claude's.
    #[test]
    fn the_antigravity_panel_matches_its_pinned_form() {
        let now = 1_785_226_000_000u64;
        let t = TelemetrySnapshot {
            // AG5: the conversation id is the DIRECTORY's name, not the log's — so it is
            // seeded from the path at construction rather than folded out of a line.
            session_id: Some("4de5fc70-2d8a-44c2-8e25-a6901c93fb86".into()),
            model: Some("Gemini 3.6 Flash (High)".into()),
            turns_user: 3,
            turns_assistant: 4,
            first_event_ms: Some(now - (12 * 60 + 30) * 1000),
            last_event_ms: Some(now - 8_000),
            total_tool_calls: 27,
            tool_calls: vec![
                ("run_command".into(), 14),
                ("view_file".into(), 9),
                ("grep_search".into(), 4),
            ],
            subagents: vec![ai_monitor::telemetry::Subagent {
                subagent_type: Some("skill-portfolio-analyzer".into()),
                description: Some("Analyzes installed skills and MCP servers".into()),
                id: "ed14342c-c779-48bd-af30-fbc139e151f2".into(),
                model: None,
                start_ms: Some(now - 45_000),
                end_ms: None,
                completed: false,
                // No Antigravity invocation is ever notified out of band, so it has no
                // agent id to join on and nothing can mark its end as a failure.
                agent_id: None,
                ended_ok: true,
            }],
            ..TelemetrySnapshot::default()
        };
        let env = Env {
            tab_agent: Some(crate::tabmeta::AgentKind::Antigravity),
            ..env(now)
        };
        insta::assert_snapshot!(project(&model(Some(&t), &env)));
    }

    // ---- ai-monitor-antigravity T5: the Antigravity session in the existing cards ----

    /// R4.1: an Antigravity subagent populates ACTIVE AGENTS through the SAME card renderer
    /// Claude's subagents use.
    ///
    /// The point of the test is that no Antigravity-specific rendering exists or is needed.
    /// AG5 maps the format's two-record subagent (`define_subagent` names it,
    /// `INVOKE_SUBAGENT` carries the child conversation id) onto the existing `Subagent`
    /// fields, so if that mapping is ever "simplified" into, say, stuffing the conversation
    /// id into `subagent_type`, this card starts rendering a UUID at the user and this test
    /// is what says so.
    #[test]
    fn an_antigravity_subagent_renders_in_the_existing_agents_card() {
        let now = 1_785_226_000_000u64;
        let t = TelemetrySnapshot {
            subagents: vec![ai_monitor::telemetry::Subagent {
                // `define_subagent`'s `name`, which is what a human reads.
                subagent_type: Some("skill-portfolio-analyzer".into()),
                description: Some("Analyzes installed skills and MCP servers".into()),
                // The CHILD conversation id (AG5's join key) — deliberately not the title.
                id: "ed14342c-c779-48bd-af30-fbc139e151f2".into(),
                // Antigravity records no per-subagent model, so the detail line must not
                // invent a separator for one.
                model: None,
                start_ms: Some(now - 30_000),
                end_ms: None,
                completed: false,
                // See the sibling test: the task-notification join has no counterpart
                // in this format, so both fields stay at their un-notified values.
                agent_id: None,
                ended_ok: true,
            }],
            ..TelemetrySnapshot::default()
        };

        let out = project(&model(
            Some(&t),
            &Env {
                tab_agent: Some(crate::tabmeta::AgentKind::Antigravity),
                ..env(now)
            },
        ));

        assert!(
            out.contains("skill-portfolio-analyzer"),
            "the card must title itself with the subagent's NAME:\n{out}"
        );
        assert!(
            out.contains("Analyzes installed skills"),
            "the detail line carries the definition's description:\n{out}"
        );
        assert!(
            !out.contains("ed14342c"),
            "the child conversation id is a join key, not something to show a user:\n{out}"
        );
        assert!(
            !out.contains(" · "),
            "no model was recorded, so no model separator may appear:\n{out}"
        );
    }

    // ---- the docked panel's chrome (R5.1–R5.5, R5.10) ------------------------

    #[test]
    fn r5_1_every_pinned_region_is_present_whatever_the_input() {
        // The chrome is the panel's identity, so it is built for a populated session, a
        // detached tab and an empty snapshot alike — a layout that appeared and
        // disappeared with the data would look broken rather than empty.
        let (t, now) = populated();
        for m in [
            model(Some(&t), &env(now)),
            model(None, &no_session(now, None)),
            model(Some(&TelemetrySnapshot::default()), &env(now)),
        ] {
            assert_eq!(m.title, "ai-monitor");
            assert!(
                !m.session.name.is_empty(),
                "the session row is always drawn"
            );
            let labels: Vec<&str> = m.tiles.iter().map(|t| t.label.as_str()).collect();
            assert_eq!(
                labels,
                vec!["UPTIME", "IDLE", "ERRORS", "IN·OUT", "CACHE", "CONTEXT"],
                "the design's 3x2 grid, in its order"
            );
            assert!(m.footer.starts_with("Last update"), "{}", m.footer);
            // Visibility is `overlay.rs`'s fact, stamped after this returns.
            assert!(!m.visible);
        }
    }

    #[test]
    fn r5_2_the_status_pill_has_exactly_three_states() {
        let (mut t, now) = populated();
        // Attached and writing.
        t.last_event_ms = Some(now - 2_000);
        assert_eq!(model(Some(&t), &env(now)).status, PanelStatus::Connected);
        // Attached but quiet past the threshold: the same session, a different reading.
        t.last_event_ms = Some(now - IDLE_AFTER_MS - 1);
        assert_eq!(model(Some(&t), &env(now)).status, PanelStatus::Idle);
        // An attached log with no parseable time is IDLE, never CONNECTED: absence of a
        // clock is not evidence of liveness.
        t.last_event_ms = None;
        assert_eq!(model(Some(&t), &env(now)).status, PanelStatus::Idle);
        // A tab with no Claude session at all.
        assert_eq!(
            model(None, &no_session(now, None)).status,
            PanelStatus::NoSession
        );
        // All four detached cases compress to the one badge — the wordings live on the
        // row below (R5.3), which is the point of D5.
        assert_eq!(
            model(Some(&t), &unattached(now)).status,
            PanelStatus::NoSession
        );
    }

    #[test]
    fn r5_3_the_session_row_keeps_all_four_detached_wordings_verbatim() {
        let (mut t, now) = populated();
        let note = |snap: Option<&TelemetrySnapshot>, e: &Env| {
            model(snap, e)
                .session
                .note
                .expect("a detached session row states which nothing this is")
        };
        t.last_event_ms = Some(now - 110 * 60 * 1000);
        assert_eq!(
            note(Some(&t), &unattached(now)),
            "(no active session — newest log idle 1h50m)"
        );
        t.last_event_ms = None;
        assert_eq!(
            note(Some(&t), &unattached(now)),
            "(no active session — newest log is undated)"
        );
        assert_eq!(
            note(
                None,
                &no_session(now, Some(crate::tabmeta::AgentKind::Claude))
            ),
            "(this tab's session has not written a log yet)"
        );
        assert_eq!(
            note(None, &no_session(now, None)),
            "(no Claude session in this tab)"
        );
        // An attached row carries no note...
        assert!(model(Some(&t), &env(now)).session.note.is_none());
        // ... and a detached one carries nothing BUT the note: the identity fields would
        // otherwise describe the log this tab declined to claim.
        let detached = model(Some(&t), &unattached(now)).session;
        assert!(detached.short_id.is_empty() && detached.model.is_empty());
        assert!(detached.provider.is_empty());
    }

    #[test]
    fn an_attached_session_row_names_the_session_and_the_model() {
        let (t, now) = populated();
        let row = model(Some(&t), &env(now)).session;
        assert_eq!(row.short_id, "5e59b629", "8 chars of the session id");
        assert_eq!(row.model, "claude-opus-5");
        assert_eq!(row.provider, "[Claude]");
    }

    #[test]
    fn r5_4_the_context_tile_carries_a_proportional_gauge() {
        let (mut t, now) = populated();
        t.context_used = 90_000; // 45% of the 200k window claude-opus-* maps to
        let m = model(Some(&t), &env(now));
        let tile = tile_of(&m, "CONTEXT");
        assert_eq!(tile.value, "45%");
        assert_eq!(tile.gauge, Some(0.45));
        // Over the assumed window the *number* is refused, not the gauge: the reading is
        // real, the denominator is what is wrong.
        //
        // The INPUT moved (256,551 -> 1,256,551) when window promotion landed; the two
        // assertions below are untouched. 256,551 is no longer a `>100%` case at all —
        // it is evidence of a 1M window and now reads 26% — so leaving it here would
        // have pinned the promotion OFF and silently re-frozen the bug. Past every tier
        // `WINDOW_TIERS` knows, nothing fits, so the clamp is still what fires.
        t.context_used = 1_256_551;
        let m = model(Some(&t), &env(now));
        assert_eq!(tile_of(&m, "CONTEXT").value, ">100%");
        assert_eq!(tile_of(&m, "CONTEXT").gauge, Some(1.0));
    }

    #[test]
    fn r5_5_an_unknown_context_window_emits_no_percentage_and_no_gauge() {
        let (mut t, now) = populated();
        t.model = Some("<synthetic>".into()); // matches no prefix in MODEL_WINDOWS
        let m = model(Some(&t), &env(now));
        let tile = tile_of(&m, "CONTEXT");
        assert_eq!(
            tile.gauge, None,
            "None, never Some(0.0): an empty track claims 0% used, which is not known"
        );
        assert_eq!(tile.value, DASH, "no percentage at all — and not `0%`");
        assert!(!tile.value.contains('%'));
        assert_eq!(m.session.model, "<synthetic>", "the model is still named");
        // A fresh session's occupancy is unknown too, not zero.
        t.model = Some("claude-opus-5".into());
        t.context_used = 0;
        assert_eq!(tile_of(&model(Some(&t), &env(now)), "CONTEXT").gauge, None);
    }

    #[test]
    fn the_configured_window_overrides_the_model_table() {
        // The fix for a session whose real window the log does not record: the same
        // 256,551 tokens against the 1M window the session actually had.
        let (mut t, now) = populated();
        t.context_used = 256_551;
        let e = Env {
            context_window: Some(1_000_000),
            ..env(now)
        };
        assert_eq!(tile_of(&model(Some(&t), &e), "CONTEXT").value, "26%");
        // ... and without it, the honest refusal rather than an impossible number.
        //
        // This half needed a bigger reading once promotion landed: at 256,551 the
        // deduction now reaches the same 1M window the config states, so the contrast
        // the assertion below is drawing no longer exists at that size. Past every known
        // tier it does. What the config uniquely buys is pinned separately, by
        // `a_configured_window_outranks_the_deduced_one`.
        t.context_used = 1_256_551;
        assert_eq!(
            tile_of(&model(Some(&t), &env(now)), "CONTEXT").value,
            ">100%"
        );
    }

    #[test]
    fn the_1m_repro_a_session_bigger_than_200k_is_measured_against_the_tier_that_fits() {
        // Measured on a real 1M-context session: one turn reached 514,930 tokens of
        // context. Against the assumed 200k that is 257% — clamped to a useless
        // `>100%` on a session that is barely half full. The number is not a guess: the
        // API accepted those tokens, so the window was never 200k.
        let (mut t, now) = populated();
        t.context_used = 514_930;
        let m = model(Some(&t), &env(now));
        let tile = tile_of(&m, "CONTEXT");
        assert_eq!(tile.value, "51%", "514,930 of the 1M window that fits it");
        assert_eq!(
            tile.gauge,
            Some(0.51),
            "and a gauge that is half full, not full"
        );
    }

    #[test]
    fn the_deduced_window_survives_a_compact_that_drops_occupancy() {
        // The amnesia case, from real logs: a `/compact` took one session from 517,389
        // tokens of context to 53,268 between two adjacent turns. Read against the
        // CURRENT occupancy the deduction forgets — 53,268 fits 200k, so the window
        // silently reverts and the panel reports 27% of a window the session has already
        // proved it is not on. The high-water is what makes the deduction a property of
        // the session rather than of the newest line.
        let (mut t, now) = populated();
        t.context_used = 517_389;
        t.context_high_water = 517_389;
        assert_eq!(tile_of(&model(Some(&t), &env(now)), "CONTEXT").value, "52%");

        // ... then the compact lands. Occupancy really did fall — the gauge must follow
        // it down — but the DENOMINATOR must not.
        t.context_used = 53_268;
        assert_eq!(
            tile_of(&model(Some(&t), &env(now)), "CONTEXT").value,
            "5%",
            "5% of the proven 1M window, not 27% of a window it outgrew"
        );

        // And the evidence is the high-water alone: a session that never exceeded 200k
        // is untouched by any of this.
        let (mut u, _) = populated();
        u.context_used = 53_268;
        u.context_high_water = 53_268;
        assert_eq!(
            tile_of(&model(Some(&u), &env(now)), "CONTEXT").value,
            "27%",
            "no evidence of a bigger window, so the table's 200k stands"
        );
    }

    #[test]
    fn promotion_fires_on_evidence_only_never_on_the_model_name() {
        // The whole safety property: `claude-opus-5` CAN run a 1M window, and that fact
        // is never consulted. Only the session's own usage moves the denominator, so a
        // session inside its assumed window is bit-for-bit what it was before.
        let (mut t, now) = populated();
        for (used, pct) in [(90_000u64, "45%"), (150_000, "75%"), (200_000, "100%")] {
            t.context_used = used;
            assert_eq!(
                tile_of(&model(Some(&t), &env(now)), "CONTEXT").value,
                pct,
                "{used} is within the assumed 200k — measured against 200k"
            );
        }
        // One token past it is evidence, and the smallest tier that fits takes over.
        t.context_used = 200_001;
        assert_eq!(tile_of(&model(Some(&t), &env(now)), "CONTEXT").value, "20%");
        // The unit rule behind both, including the exact boundaries.
        assert_eq!(promoted_window(200_000, 199_999), 200_000);
        assert_eq!(
            promoted_window(200_000, 200_000),
            200_000,
            "fits: no promotion"
        );
        assert_eq!(promoted_window(200_000, 200_001), 1_000_000);
        assert_eq!(promoted_window(200_000, 1_000_000), 1_000_000, "exact fit");
        // `promoted_window` takes the FIRST tier that fits, so "first" has to mean
        // "smallest". Only one tier sits above 200k today, so a mis-sorted table would
        // change no output and no behavioural test could catch it — pinned here before a
        // third tier makes the ordering silently load-bearing.
        assert!(
            WINDOW_TIERS.windows(2).all(|w| w[0] < w[1]),
            "WINDOW_TIERS must be strictly ascending"
        );
    }

    #[test]
    fn usage_past_every_known_tier_still_refuses_the_number() {
        // The `>100%` clamp is not dead code after promotion — it is what remains when
        // the deduction has nothing to deduce. A session past 1M fits no tier this build
        // knows, so the panel says so instead of inventing a denominator.
        let (mut t, now) = populated();
        t.context_used = 1_000_001;
        let m = model(Some(&t), &env(now));
        let tile = tile_of(&m, "CONTEXT");
        assert_eq!(tile.value, ">100%");
        assert_eq!(tile.gauge, Some(1.0));
        assert_eq!(
            promoted_window(200_000, 1_000_001),
            200_000,
            "no tier fits, so the assumption stands and the caller clamps"
        );
    }

    #[test]
    fn a_configured_window_outranks_the_deduced_one() {
        // `[ai_monitor] context_window` is knowledge; promotion is inference. The one
        // case that separates them is a config NARROWER than what the usage would
        // deduce — if the deduction could overrule it, this reads 51%.
        let (mut t, now) = populated();
        t.context_used = 514_930;
        let e = Env {
            context_window: Some(200_000),
            ..env(now)
        };
        assert_eq!(
            tile_of(&model(Some(&t), &e), "CONTEXT").value,
            ">100%",
            "the user said 200k; the panel does not argue"
        );
        assert_eq!(
            tile_of(&model(Some(&t), &env(now)), "CONTEXT").value,
            "51%",
            "and without the config, the deduction does its job"
        );
    }

    #[test]
    fn the_configured_window_applies_to_an_unmatched_model_too() {
        // `<synthetic>` matches no prefix, so the table yields nothing — but an explicit
        // window is knowledge, not a guess, and does apply.
        let (mut t, now) = populated();
        t.model = Some("<synthetic>".into());
        let e = Env {
            context_window: Some(200_000),
            ..env(now)
        };
        assert_eq!(context_pct(&t, &e), Some(59));
        assert_eq!(
            context_pct(&t, &env(now)),
            None,
            "without it, still omitted"
        );
    }

    // ---- the read window as a denominator (`ai-monitor-context-window`) ------

    /// A snapshot carrying a `/context` reading taken on `model_at_read`.
    fn with_reading(tokens: u64, model_at_read: Option<&str>) -> (TelemetrySnapshot, u64) {
        let (mut t, now) = populated();
        t.context_window_reading = Some(ContextWindowReading {
            tokens,
            model_at_read: model_at_read.map(str::to_string),
        });
        (t, now)
    }

    #[test]
    fn the_configured_window_still_beats_a_reading() {
        // R4.1 — a reading is knowledge the panel gained, but the user's own statement
        // is knowledge the panel could not have. The existing doctrine is untouched.
        let (t, now) = with_reading(700_000, Some("claude-opus-5"));
        let e = Env {
            context_window: Some(500_000),
            ..env(now)
        };
        assert_eq!(context_pct(&t, &e), Some(23), "117066/500000 = 23.4%");
        assert_eq!(context_pct(&t, &env(now)), Some(17), "117066/700000");
    }

    #[test]
    fn a_reading_beats_the_model_table() {
        // The headline defect: `claude-opus-5` guesses 200k, `/context` printed 700k.
        // Same numerator, and it already agreed to the token.
        let (t, now) = with_reading(700_000, Some("claude-opus-5"));
        assert_eq!(context_pct(&t, &env(now)), Some(17));
        let (plain, _) = populated();
        assert_eq!(
            context_pct(&plain, &env(now)),
            Some(59),
            "without a reading, the table's guess stands"
        );
    }

    #[test]
    fn a_reading_is_terminal_and_is_never_promoted() {
        // R4.2 — the reversal that matters. `promoted_window(700_000, 750_000)` returns
        // 1,000,000, a denominator `/context` never prints; feeding the reading in as an
        // assumption would swap the one measured truth for the guess this path retires.
        let (mut t, now) = with_reading(700_000, Some("claude-opus-5"));
        t.context_used = 350_000;
        t.context_high_water = 750_000;
        assert_eq!(context_pct(&t, &env(now)), Some(50), "350000/700000");
        assert_eq!(
            promoted_window(700_000, 750_000),
            1_000_000,
            "which is what the reading is being kept away from"
        );
    }

    #[test]
    fn a_reading_short_circuits_the_model_requirement() {
        // R4.3 — `context_window(t.model?)?` gives up the moment the model is unknown,
        // so the percentage disappears. A reading does not depend on the model id at
        // all, which is why its branch has to come FIRST; behind the `?` it would be
        // unreachable in exactly the case it was added for.
        let (mut t, now) = with_reading(700_000, None);
        t.model = None;
        t.context_used = 70_000;
        assert_eq!(context_pct(&t, &env(now)), Some(10));

        let (mut without, _) = populated();
        without.model = None;
        without.context_used = 70_000;
        assert_eq!(
            context_pct(&without, &env(now)),
            None,
            "and with no reading, the model requirement still bites"
        );
    }

    #[test]
    fn usage_past_a_read_window_reads_over_a_hundred_percent() {
        // R4.4 — and here it is a TRUE statement (past the auto-compact threshold),
        // not the table's "this build knows no window that big".
        let (mut t, now) = with_reading(700_000, Some("claude-opus-5"));
        t.context_used = 750_000;
        t.context_high_water = 750_000;
        assert_eq!(
            context_pct(&t, &env(now)),
            Some(107),
            "750000/700000 = 107.1%"
        );
        assert_eq!(
            tile_of(&model(Some(&t), &env(now)), "CONTEXT").value,
            ">100%"
        );
    }

    #[test]
    fn a_reading_survives_while_the_model_is_unchanged() {
        // R5.1
        let (t, now) = with_reading(700_000, Some("claude-opus-5"));
        assert_eq!(usable_window_reading(&t), Some(700_000));
        assert_eq!(
            context_pct(&t, &env(now)),
            Some(17),
            "117066/700000 = 16.7%"
        );
    }

    #[test]
    fn a_reading_taken_before_any_model_line_still_applies() {
        // R5.2 — `None` at read time means "no evidence yet", not "a different model".
        // Reading it as a mismatch would silently discard a perfectly good reading from
        // the ordinary case of typing `/context` before the first assistant turn.
        let (t, _) = with_reading(700_000, None);
        assert_eq!(t.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(usable_window_reading(&t), Some(700_000));
    }

    #[test]
    fn a_reading_is_refused_after_the_model_changes() {
        // R5.3 — the under-reporting hole: read 1M, switch down to a 200k model, and
        // last-wins has nothing newer. The panel must fall back rather than divide by a
        // window the session is no longer on.
        let (mut t, now) = with_reading(1_000_000, Some("claude-opus-5"));
        t.model = Some("claude-haiku-4-5".into());
        assert_eq!(usable_window_reading(&t), None);
        assert_eq!(
            context_pct(&t, &env(now)),
            Some(59),
            "falls through to table then promotion: 117066/200000"
        );
    }

    #[test]
    fn a_reading_is_refused_when_the_model_becomes_unknown() {
        // The fourth arm. Unreachable today — `is_real_model` never writes `None` back
        // and `restarted()` discards the whole accumulator — but written out because it
        // is NOT free: refusing here means the fallback's `?` fires on a missing model
        // and the percentage vanishes altogether.
        let (mut t, now) = with_reading(700_000, Some("claude-opus-5"));
        t.model = None;
        assert_eq!(usable_window_reading(&t), None);
        assert_eq!(context_pct(&t, &env(now)), None);
    }

    #[test]
    fn a_reading_returns_when_the_model_returns() {
        // R5.4 — the observable difference between a predicate and a mutation. The
        // reading is a fact measured on opus; opus is running again, so it is a fact
        // again. A discard-on-switch would have lost it for good.
        let (mut t, now) = with_reading(700_000, Some("claude-opus-5"));
        t.model = Some("claude-haiku-4-5".into());
        assert_eq!(usable_window_reading(&t), None);
        t.model = Some("claude-opus-5".into());
        assert_eq!(usable_window_reading(&t), Some(700_000));
        assert_eq!(context_pct(&t, &env(now)), Some(17));
    }

    #[test]
    fn the_model_pairing_is_compared_whole_never_by_tier() {
        // A deliberate cost, stated where someone will be tempted to "fix" it: the same
        // tier under a dated id loses its reading. Tier comparison would need a table
        // mapping ids to tiers — which is precisely the knowledge this spec proved a log
        // does not carry, and the guess it exists to remove.
        let (mut t, _) = with_reading(700_000, Some("claude-opus-5"));
        t.model = Some("claude-opus-5-20260301".into());
        assert_eq!(
            usable_window_reading(&t),
            None,
            "lost, and losing it over-reports — the safe direction"
        );
    }

    #[test]
    fn a_session_with_no_reading_behaves_exactly_as_before() {
        // R0.2 — the preservation property. 37 of the corpus's 42 logs take this path.
        let (mut t, now) = populated();
        assert_eq!(t.context_window_reading, None);
        assert_eq!(context_pct(&t, &env(now)), Some(59));
        t.context_high_water = 250_000;
        assert_eq!(
            context_pct(&t, &env(now)),
            Some(12),
            "promotion to 1M still happens when nothing was read"
        );
    }

    #[test]
    fn an_antigravity_style_session_never_reaches_the_reading() {
        // R0.3 — no token usage is ever recorded in that dialect, so `context_used`
        // stays 0 and the function returns before any of this. A reading cannot
        // resurrect a percentage there, and must not appear to.
        let (mut t, now) = with_reading(700_000, None);
        t.context_used = 0;
        t.context_high_water = 0;
        assert_eq!(context_pct(&t, &env(now)), None);
        assert_eq!(tile_of(&model(Some(&t), &env(now)), "CONTEXT").value, DASH);
    }

    #[test]
    fn a_zero_reading_cannot_divide_by_zero() {
        // The crate floors readings at 1,000, so this is unconstructible from a log —
        // but the contract is a plain `u64` and the guard below it is what makes that
        // safe. Assert the guard rather than trusting the producer.
        let (t, now) = with_reading(0, Some("claude-opus-5"));
        assert_eq!(context_pct(&t, &env(now)), None);
    }

    /// The `ai-monitor` fixture, folded here rather than restated — so this asserts the
    /// whole path (raw JSONL bytes → strip → anchors → snapshot → denominator → tile)
    /// and not a hand-built snapshot that happens to agree with one.
    fn folded_context_fixture() -> (TelemetrySnapshot, u64) {
        let mut t = SessionTelemetry::new();
        for l in include_str!("../../ai-monitor/tests/fixtures/context_command.jsonl").lines() {
            t.apply(l);
        }
        let s = t.snapshot();
        let now = s.last_event_ms.expect("the fixture is timestamped");
        (s, now)
    }

    #[test]
    fn a_real_context_log_makes_the_tile_agree_with_the_command() {
        // R0.1 — the defect, end to end, at the measured numbers. `/context` printed
        // 31% (217.7k/700k); the panel printed 22%, because it guessed 200k from
        // `claude-opus-5` and then promoted that to 1M off the high-water. The
        // numerator was never wrong.
        let (t, now) = folded_context_fixture();
        assert_eq!(t.context_used, 217_737);
        assert_eq!(context_pct(&t, &env(now)), Some(31));
        assert_eq!(tile_of(&model(Some(&t), &env(now)), "CONTEXT").value, "31%");

        // The same snapshot with the reading removed is today's behaviour, so the two
        // numbers in this spec's opening paragraph sit side by side in one assertion.
        let mut guessed = t.clone();
        guessed.context_window_reading = None;
        assert_eq!(context_pct(&guessed, &env(now)), Some(22));
    }

    #[test]
    fn a_log_that_never_ran_the_command_is_byte_identical_to_before() {
        // R0.2 — 37 of the corpus's 42 logs. The fold must leave the field empty and
        // the cascade must then take exactly the path it took before this feature.
        let mut t = SessionTelemetry::new();
        for l in include_str!("../../ai-monitor/tests/fixtures/session.jsonl").lines() {
            t.apply(l);
        }
        let s = t.snapshot();
        assert_eq!(s.context_window_reading, None);
        assert_eq!(usable_window_reading(&s), None);

        let now = s.last_event_ms.unwrap_or_default();
        let expected = promoted_window(
            context_window(s.model.as_deref().expect("fixture names a model"))
                .expect("a known model"),
            s.context_high_water.max(s.context_used),
        );
        assert_eq!(
            context_pct(&s, &env(now)),
            Some(
                (s.context_used.saturating_mul(200) / expected)
                    .div_ceil(2)
                    .max(1)
            ),
            "the table-then-promotion path, unchanged"
        );
    }
    #[test]
    fn the_errors_tile_counts_failed_calls_not_parser_faults() {
        // D3/R2.3 — `parse_errors` counts PARSER faults. Showing it under an ERRORS
        // label would be a false statement about what failed.
        let (mut t, now) = populated();
        t.failed_tool_calls = 3;
        t.parse_errors = 99;
        let m = model(Some(&t), &env(now));
        assert_eq!(tile_of(&m, "ERRORS").value, "3");
        assert_eq!(tile_of(&m, "ERRORS").color, ColorRole::Rose);
        t.failed_tool_calls = 0;
        let m = model(Some(&t), &env(now));
        assert_eq!(
            tile_of(&m, "ERRORS").value,
            "0",
            "zero failures is a fact worth stating, not an unknown"
        );
        assert_ne!(
            tile_of(&m, "ERRORS").color,
            ColorRole::Rose,
            "a rose zero would read as an alarm about nothing"
        );
    }

    #[test]
    fn the_spend_tiles_sum_cache_and_only_context_is_a_percentage() {
        // The cumulative COST counters, never presented as occupancy: cache-read is
        // re-billed every turn, so these run ~16x the live context on a real session.
        let (t, now) = populated();
        let m = model(Some(&t), &env(now));
        assert_eq!(tile_of(&m, "IN·OUT").value, "45·19.3K");
        assert_eq!(
            tile_of(&m, "CACHE").value,
            "1.9M",
            "read + creation, summed because both are billed"
        );
        assert_eq!(
            project(&m).matches('%').count(),
            1,
            "the context gauge is the only percentage on the panel"
        );
    }

    #[test]
    fn the_activity_tiles_are_readings_taken_at_the_footer_s_timestamp() {
        // D20 — UPTIME and IDLE are `now - first/last`, frozen between snapshots, which
        // the footer's absolute time is what qualifies.
        let (mut t, now) = populated();
        let m = model(Some(&t), &env(now));
        assert_eq!(tile_of(&m, "UPTIME").value, "45m12s");
        assert_eq!(tile_of(&m, "IDLE").value, "2m03s");
        // Missing event times render the placeholder, never an epoch-derived duration.
        t.first_event_ms = None;
        t.last_event_ms = None;
        let m = model(Some(&t), &env(now));
        assert_eq!(tile_of(&m, "UPTIME").value, DASH);
        assert_eq!(tile_of(&m, "IDLE").value, DASH);
    }

    #[test]
    fn r5_10_the_footer_is_an_absolute_local_time_with_no_relative_age() {
        // D9 — a relative age freezes and starts lying the moment nothing repaints, which
        // under D2 is most of the time. The epoch below is 07:21:18 UTC.
        let (mut t, now) = populated();
        t.last_event_ms = Some(1_785_223_278_000);
        let kst = Env {
            utc_offset_secs: 32_400,
            ..env(now)
        };
        assert_eq!(model(Some(&t), &kst).footer, "Last update 16:21:18");
        // The offset is injected, so the same snapshot reads differently by zone — and
        // identically on every machine in one zone, which is what keeps the snapshots
        // byte-stable (D19).
        assert_eq!(
            model(Some(&t), &env(now)).footer,
            "Last update 07:21:18",
            "utc_offset_secs = 0"
        );
        for footer in [
            model(Some(&t), &kst).footer,
            model(None, &no_session(now, None)).footer,
        ] {
            assert!(!footer.contains("ago"), "{footer}");
            assert!(!footer.contains("idle"), "{footer}");
        }
        // Nothing folded yet: the placeholder, not an epoch rendered as midnight UTC.
        t.last_event_ms = None;
        assert_eq!(model(Some(&t), &kst).footer, format!("Last update {DASH}"));
    }

    #[test]
    fn r1_4_the_footer_draws_no_q_keycap() {
        // D1 dropped the design's `q` keycap rather than draw a promise the panel does not
        // keep. The design's footer reads "Streaming live · updated 6s ago" with a boxed
        // `q` beside it, and that keycap is a *claim*: it says pressing `q` closes the
        // panel. It does not — `q` goes to the shell, where it quits `less` (R1.3). A
        // keycap for a key nothing handles is worse than no keycap, because the user
        // presses it, `less` exits, and the panel stays open.
        //
        // Asserted on the model rather than on pixels because the model is the only thing
        // the app composes: `monitor_layout::footer` draws `model.footer` and one hairline
        // and nothing else, so a keycap could only enter here (the layout half is pinned
        // in `monitor_layout.rs`).
        let (t, now) = full();
        let kst = Env {
            utc_offset_secs: 32_400,
            ..env(now)
        };
        // Every state the footer has: attached, detached, and no session at all. A close
        // affordance drawn in only one of them is the easiest version of this to ship.
        //
        // NOTE (ai-monitor-panel-scroll): the *layout* half of R1.4 was deliberately
        // reversed — the footer band now shows a chord hint when the badge is up, because
        // the panel finally has a binding to promise and D30 makes it the only way back.
        // See `monitor_layout::tests::r1_4_the_footer_band_carries_one_line_and_no_control_that_lies`.
        // This half stands unchanged, and gained a second job below.
        for footer in [
            model(Some(&t), &kst).footer,
            model(Some(&t), &unattached(now)).footer,
            model(
                None,
                &no_session(now, Some(crate::tabmeta::AgentKind::Claude)),
            )
            .footer,
            model(None, &no_session(now, None)).footer,
        ] {
            assert!(
                footer.starts_with("Last update "),
                "the footer is the last-update time and nothing else: {footer:?}"
            );
            // ai-monitor-panel-scroll R7.7 leans on this half of R1.4 rather than retiring
            // it. The badge replaces this line ON SCREEN (D34), but it must never be written
            // INTO the model: the model is stage one's cache key, so a count folded in here
            // would re-lay-out the whole document every time it changed (C4) — and it would
            // break this assertion for a reason that has nothing to do with a close
            // affordance. The composer stays ignorant of the badge; stage two draws it.
            for digit in ["new above", "⌘⌥1"] {
                assert!(
                    !footer.contains(digit),
                    "the badge leaked into the model footer: {footer:?}"
                );
            }
            // The design's keycap and the two other close affordances it draws (the header
            // `✕`, the `Esc` hint). None of them is a key the panel answers to.
            for affordance in ["q", "Q", "esc", "Esc", "ESC", "✕", "×", "close", "quit"] {
                assert!(
                    !footer.contains(affordance),
                    "{affordance:?} in the footer {footer:?} — the panel closes on \
                     Cmd+Shift+M only (D1), so any other close affordance is a lie"
                );
            }
        }
    }

    #[test]
    fn the_clock_wraps_west_of_greenwich_instead_of_going_negative() {
        // A naive `%` on a pre-dawn UTC time with a negative offset yields a negative
        // hour, which formats as `-1:59:59`.
        assert_eq!(clock(1_785_223_278_000, 32_400), "16:21:18");
        assert_eq!(clock(1_785_223_278_000, 0), "07:21:18");
        assert_eq!(clock(1_785_223_278_000, -8 * 3_600), "23:21:18");
        assert_eq!(clock(0, -3_600), "23:00:00");
        assert_eq!(clock(0, 0), "00:00:00");
    }
}
