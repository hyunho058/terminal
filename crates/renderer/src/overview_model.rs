//! The Agent Monitor overview's semantic model — plain data the app hands the renderer
//! (`session-overview-ui` D20).
//!
//! Every string here is already formatted by the app's pure policy functions
//! (`app::overview`, `app::overview_detail`); every timeline position is already a window
//! fraction. The renderer only lays out and draws. So nothing here names an app type: tabs
//! are `u64` (the app's `TabId.0`), panes `u32` (the tmux pane pid, `0` for an unsplit tab —
//! the same key the old graph's fold set used).
//!
//! Colours are **roles**, never raw values: a row says which agent it is ([`AgentTag`]) and
//! which bucket it is in ([`Bucket`]), and the renderer's palette (D21) maps those to the
//! design's colours, so the palette keeps exactly one home.

/// A session's status bucket (D2). Declaration order **is** the sort order (Needs you 0,
/// Working 1, Idle 2, Agent 3), so `Ord` sorts rows the way the table shows them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Bucket {
    /// `WaitingForInput` with a blocking dialog on screen, or the log's last message asks
    /// the user (Antigravity's `ask_question`).
    NeedsYou,
    /// `Working`.
    Working,
    /// `Ready`, or `WaitingForInput` with neither signal — a finished turn.
    Idle,
    /// An agent is present but its state is unknown (`Hidden`). The default, because an
    /// unknown agent must never read as ready (D2). Counted in `All` only.
    #[default]
    Agent,
}

/// The header's filter tabs (D6). Restricts table rows only — never tiles or banner.
/// `Agent` rows have no tab of their own and appear only under `All`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Filter {
    /// Every row. The startup filter (D6: kept across reopen, reset at startup).
    #[default]
    All,
    NeedsYou,
    Working,
    Idle,
}

/// One subagent's state as the squares, tree and lanes colour it (D8, D12).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SubagentStatus {
    /// `!completed`.
    #[default]
    Running,
    /// `completed && ended_ok`.
    Done,
    /// `completed && !ended_ok`.
    Failed,
}

/// Which agent a session runs — the agent tag's label colour role (D3, D21).
///
/// This is the "agent colour index" D20 names, typed: the renderer's palette maps it to the
/// design's tag colour (Claude `#E9967A`, Codex `#A9B8FF`, Antigravity `#86D1C4`, Gemini and
/// Aider `#B8B1A7`). The label text itself travels separately as
/// [`SessionRowData::agent_label`], so the app owns the wording.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AgentTag {
    /// The default only so row data can be built with `..Default::default()` in tests.
    #[default]
    Claude,
    Codex,
    Antigravity,
    Gemini,
    Aider,
}

/// One cell of a row's subagent squares (D8): a 12 × 12 square in log order, or — when two
/// rows of squares cannot hold them all — a final `+{k}` in place of the last square.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SubagentCell {
    Square(SubagentStatus),
    /// `+{k}`: `k` subagents not drawn as squares.
    More(usize),
}

/// One line of an expanded row's subagent tree (D8).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct TreeLine {
    /// `subagent_type`, else `agent`.
    pub subagent_type: String,
    /// The description, else `—`.
    pub description: String,
    /// `running` / `done` / `failed`.
    pub status_word: String,
    /// Finished: `{m}m {ss}s`; running: D7's minute format (`<1m`, `{m}m`); `—` unknown.
    pub duration: String,
    /// The dot's colour role.
    pub status: SubagentStatus,
}

/// One table row: one agent session — a tab, or one tmux pane of a split tab (D2, D3, D14).
/// Every string is final; the renderer only truncates to its column.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SessionRowData {
    /// The app's `TabId.0`.
    pub tab: u64,
    /// The tmux pane pid, `0` for an unsplit tab.
    pub pane: u32,
    pub bucket: Bucket,
    /// Line 1, title face: the repository's main worktree basename, else the agent cwd's
    /// folder name (D3).
    pub project: String,
    /// `Claude Code`, `Codex`, `Antigravity`, `Gemini`, `Aider` (D3).
    pub agent_label: String,
    /// The agent tag's colour role (see [`AgentTag`]).
    pub agent: AgentTag,
    /// Line 2: the branch, then ` · {worktree dir}` in a linked worktree; with no branch, the
    /// `~`-abbreviated cwd (D3).
    pub branch_line: String,
    /// Whether [`Self::branch_line`] is a branch (drawn after the git-branch icon) rather
    /// than the cwd fallback (drawn without it).
    pub has_branch: bool,
    /// The Now doing column's first line (D4).
    pub now_doing: String,
    /// The user prompt, shown quoted and muted under Now doing; `None` omits the line (D3).
    pub prompt: Option<String>,
    /// Time since the latest human prompt: `<1m`, `{m}m`, `{h}h {m}m`, or `—` (D7).
    pub elapsed: String,
    /// One square per subagent, in log order — **not** capped: how many fit on two rows
    /// depends on the column width (D15), so the layout caps them and draws the trailing
    /// `+{k}` (D8, R5.2). Empty → `—`.
    pub subagents: Vec<SubagentCell>,
    /// `{r} running · {d} done` (the running part only when r > 0), `—` with none (D8).
    pub subagent_summary: String,
    /// One line per subagent, in log order — drawn only while [`Self::expanded`]. Non-empty
    /// exactly when the row has subagents, which is when it has an expand control (D8).
    pub tree: Vec<TreeLine>,
    /// The row is in the expanded set (D8, D10).
    pub expanded: bool,
    /// The row is the highlighted one.
    pub selected: bool,
}

/// The tile and filter-tab counts (D2, D6). Computed over **all** rows, never the filtered
/// ones: the filter restricts the table only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Counts {
    /// Every row, `Agent` rows included (`All {n}`, `{n} sessions`).
    pub all: usize,
    pub needs_you: usize,
    pub working: usize,
    pub idle: usize,
}

/// The subagents tile's totals over all rows (R4.6). Kept as three disjoint counts: the tile
/// reads `running` and `done + failed` (failed count into done, D8), while its bar draws
/// each part's own fraction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SubagentTotals {
    pub running: usize,
    /// Finished and ended ok — failed ones excluded.
    pub done: usize,
    pub failed: usize,
}

/// The `Waiting on you` banner (D5): the first Needs-you row in sort order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Banner {
    /// The session `Open session →` jumps to (tab only — panes are not focusable).
    pub tab: u64,
    pub pane: u32,
    /// `{Agent} · {project} — {now-doing}`, without the `(+k more)` suffix.
    pub text: String,
    /// How many other rows are Needs you; the renderer draws ` (+{more} more)` when > 0.
    pub more: usize,
}

/// The whole overlay's model (D20).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverviewModel {
    /// Before the worker's first full pass for this open: `resolving…`, no rows (D3, D17).
    pub resolving: bool,
    /// The table rows: filtered by [`Self::filter`], sorted by bucket then session order
    /// (D2, D6).
    pub rows: Vec<SessionRowData>,
    /// Over all rows, unfiltered.
    pub counts: Counts,
    /// Ignores the filter (D5).
    pub banner: Option<Banner>,
    pub filter: Filter,
    /// Over all rows, unfiltered (R4.6).
    pub total_subagents: SubagentTotals,
    pub view: View,
}

/// Which screen the overlay shows (D10). Opening always shows the overview.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum View {
    #[default]
    Overview,
    /// Boxed: a detail is several hundred bytes and the overview is the common view.
    Detail(Box<DetailData>),
}

/// A main-lane segment's kind (D12).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SegmentKind {
    /// Inside a turn, no subagent running (`#E8744F`).
    Working,
    /// Inside a turn with at least one subagent running (`#6B3A2A`).
    WaitingOnSubagent,
    /// From a turn's end to the next human prompt (`#3A3632`).
    Idle,
    /// Older than the oldest kept turn: drawn in the track colour, not idle.
    #[default]
    Blank,
}

/// One main-lane segment, as fractions `0.0..=1.0` of the timeline window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Segment {
    pub kind: SegmentKind,
    pub start: f32,
    pub end: f32,
}

/// One subagent lane (D12): its bar clipped to the window, as window fractions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lane {
    /// `└ {description}`.
    pub label: String,
    pub status: SubagentStatus,
    pub start: f32,
    pub end: f32,
    /// Drawn right of the bar.
    pub duration: String,
    /// The selected lane draws `#8FC29E` (D12).
    pub selected: bool,
}

/// One timeline tick at a whole 10-minute mark of the wall clock (D12).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tick {
    /// Window fraction.
    pub at: f32,
    /// Local `HH:MM`.
    pub label: String,
}

/// A recent event's colour role (D13).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EventTone {
    #[default]
    Plain,
    /// `Task done · …` (`#8FC29E`).
    Done,
    /// `Task failed · …` (`#E5534B`).
    Failed,
}

/// One recent-events line (D13).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct EventLine {
    /// Local `HH:MM`.
    pub time: String,
    /// `Turn finished`, `Task start · {d}`, `Task done · {d}`, `Task failed · {d}`,
    /// `{Tool} · {summary}`.
    pub text: String,
    pub tone: EventTone,
}

/// The selected subagent's panel (D13).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SubagentPanel {
    /// Title face.
    pub title: String,
    pub subagent_type: String,
    pub status: SubagentStatus,
    /// `running` / `done` / `failed`.
    pub status_word: String,
    /// Local `HH:MM:SS`.
    pub started: String,
    pub duration: String,
    /// The report's first four lines; `["—"]` when none was recorded.
    pub report: Vec<String>,
}

/// One session's detail screen (D10, D12, D13, D22).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DetailData {
    /// The session this detail is for; `Focus terminal` jumps to `tab`.
    pub tab: u64,
    pub pane: u32,
    // header — the row's own strings (D3, D4, D7)
    pub project: String,
    pub agent_label: String,
    pub agent: AgentTag,
    pub bucket: Bucket,
    pub branch_line: String,
    pub now_doing: String,
    pub prompt: Option<String>,
    pub elapsed: String,
    /// `false` for a session with no digest (non-Claude): the layout draws
    /// `No timeline — only Claude Code sessions record subagents.` instead (D12).
    pub has_timeline: bool,
    /// Oldest first, only subagents overlapping the window.
    pub lanes: Vec<Lane>,
    /// The main session lane.
    pub main_lane: Vec<Segment>,
    pub ticks: Vec<Tick>,
    /// The newest 8 inside the window, newest first (D13).
    pub events: Vec<EventLine>,
    /// Index into [`Self::lanes`]; defaults to the newest lane (D13).
    pub selected: Option<usize>,
    /// The selected lane's subagent, `None` with no lane.
    pub panel: Option<SubagentPanel>,
    /// `claude --resume {session_id}` for a Claude session with a known id; `None` hides the
    /// `Copy resume cmd` button (D22).
    pub resume_cmd: Option<String>,
}

/// What a press landed on (D20). A closed set; every session-addressed hit carries the same
/// `(tab, pane)` identity, and the three jump hits (`Open`, `Banner`, `Focus`) switch to
/// `tab` only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OverviewHit {
    /// A filter tab (D6).
    Filter(Filter),
    /// A row outside its two buttons: open its detail (D10).
    Row(u64, u32),
    /// The chevron: toggle the tree (D8).
    Expand(u64, u32),
    /// The `→` button: switch to the tab and close (D10).
    Open(u64, u32),
    /// `Open session →` (D5).
    Banner(u64, u32),
    /// `‹ All sessions` (D10).
    Back,
    /// `Focus terminal` (D10, D22).
    Focus(u64, u32),
    /// A subagent lane, by index into [`DetailData::lanes`] (D13).
    Lane(usize),
    /// `Copy resume cmd` (D22).
    CopyResume,
}

/// What a blocking dialog on screen asks about (D9). Lives renderer-side because
/// `SidebarRow` carries it and the renderer may not import app types.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DialogSubject {
    /// A permission dialog's command or target, trimmed, at most 80 chars.
    Command(String),
    /// An `AskUserQuestion` dialog or its review screen.
    Question,
    /// `ExitPlanMode`'s approval.
    Plan,
}
