//! Which pid a tab's metadata derivation starts from.
//!
//! A tmux pane's shell is **not** a descendant of the tab's shell — it is a child of the tmux
//! *server* — so no depth increase in [`super::agent::walk`] could ever reach the agent running
//! inside a pane. The fix is to **re-root**: when a tab's tree contains an attached tmux client,
//! derive from that client's `pane_pid` instead of the tab's own shell. Everything downstream
//! then works unchanged, because `pane_pid` is the pane's `-zsh` and the agent is its *direct
//! child* — exactly the depth-1 shape the existing walk already handles.
//!
//! The join is on the **controlling tty device**: a tmux client shares its tty with the tab's
//! shell (`#{client_tty}` → `/dev/ttys002`), while the pane it is looking at runs on a different
//! one. So the tab's `e_tdev` ([`super::proc::tty_dev_of_pid`]) is matched against the `st_rdev`
//! of each client tty path ([`super::proc::rdev_of_path`]).
//!
//! Cost is gated twice (the active tab refreshes every 500 ms): a **presence gate** means a
//! machine without tmux — or a tab not running it — spawns nothing at all, and a **per-tick
//! memo** means one `list-clients` per tick regardless of how many tmux tabs exist. The memo
//! caches the client→pane *map*, never a resolved root: `pane_pid` changes when the user
//! switches tmux windows, and following the active pane is the point.
//!
//! Every impure edge is a thin wrapper around a pure core ([`parse_clients`], [`select_pane`])
//! or takes its syscall as an injected closure ([`resolve_rdevs`]), so every branch is tested
//! with no tmux installed and no live tty.
//!
//! **Pane enumeration** (session-flow-viewer D9) is the second, opt-in half: while the session
//! graph is open the worker also needs the *other* panes of the window a tab's client shows.
//! `list-clients` gains a `#{session_name}:#{window_index}` column naming that window, and one
//! memoised `list-panes -a` per tick ([`PaneMemo`], the same one-entry shape as [`Memo`]) lists
//! every pane on the server; [`panes_of`] joins the two. The active pane is the one whose pid
//! is the client's `pane_pid` — the pid [`select_pane`] re-rooted the card at — never
//! `#{pane_active}`, because two process calls can disagree within a tick and the card and the
//! graph must not describe different panes as "active".

// This module was the mechanism half of the `tmux-pane-resolution` set, and carried a
// blanket `#![allow(dead_code)]` while its consumers (`worker.rs`, `tabsession.rs`) were
// still to land. They have; the chain is reachable from a live root, and the one item that
// is still unread carries its own narrow allow instead. A module-wide allow would go on
// hiding the next one.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use super::worker::TICK;

/// A tmux client found in a tab's process tree, plus the binary it is running.
///
/// The exe path is carried (not just the pid) so the spawn can use *that* binary rather than
/// resolving `tmux` through `PATH`: a `.app` bundle launched from Finder inherits a minimal
/// `PATH` that contains neither `/opt/homebrew/bin` nor `/usr/local/bin`, so a `PATH`-resolved
/// spawn would work under `cargo run` and then silently never resolve in the shipped app.
/// It costs no extra syscall — the gate already read this path to classify the process.
pub(crate) struct TmuxClient {
    /// Which client this is. Nothing reads it yet — the join runs on `exe` and the
    /// tty device — but it is what identifies the record when one has to be
    /// reported or matched, so dropping it would be losing the row's identity.
    #[allow(dead_code)]
    pub(crate) pid: u32,
    pub(crate) exe: PathBuf,
}

/// Exact, case-insensitive, basename-only — `tmux` and `TMUX` match, `tmux-next` and `mytmux`
/// do not. Mirrors [`super::agent::classify_exe`]; unlike it, no dot-component or
/// `<name>/versions` handling is wanted, since tmux ships no in-place auto-updater.
fn is_tmux(path: &Path) -> bool {
    path.file_name()
        .map(|s| s.to_string_lossy())
        .is_some_and(|b| b.eq_ignore_ascii_case("tmux"))
}

/// The presence gate, generic over its two syscalls so every case is testable against a fake
/// tree (D13(2)'s idiom, as [`super::agent::walk`] uses).
///
/// Checks the **root itself first**, then its direct children. Depth 0 is not an edge case: the
/// common `[[ -z $TMUX ]] && exec tmux new -A -s main` idiom replaces the shell's image at the
/// *same pid*, so such a tab has no tmux child at all and a children-only gate — which is what
/// `agent::walk` does, since it never classifies its root — would make this feature a silent
/// no-op for those users. It costs one `exe_path`, cheaper than the child scan it precedes.
///
/// Depth stops at 1: the measured interactive case has the client as a direct child, and a full
/// bounded walk would cost up to `MAX_NODES` classifications per tab per tick to catch a tmux
/// launched behind a wrapper — the rarer case, which degrades to "no re-root" rather than to a
/// wrong answer.
pub(crate) fn tmux_client_of_with(
    root: u32,
    exe_of: impl Fn(u32) -> Option<PathBuf>,
    children_of: impl Fn(u32) -> Vec<u32>,
) -> Option<TmuxClient> {
    if let Some(exe) = exe_of(root) {
        if is_tmux(&exe) {
            return Some(TmuxClient { pid: root, exe });
        }
    }
    for child in children_of(root) {
        if let Some(exe) = exe_of(child) {
            if is_tmux(&exe) {
                return Some(TmuxClient { pid: child, exe });
            }
        }
    }
    None
}

/// Production entry point for the gate, bound to the real syscalls.
fn tmux_client_of(root: u32) -> Option<TmuxClient> {
    tmux_client_of_with(root, super::proc::exe_path, super::proc::children_of)
}

/// The `list-clients` format. The third column (session-flow-viewer D9) names the window the
/// client is showing, in the same `session:index` spelling [`PANES_FORMAT`] prints, so the two
/// outputs join on plain string equality.
const CLIENTS_FORMAT: &str = "#{client_tty} #{pane_pid} #{session_name}:#{window_index}";

/// The `list-panes -a` format (session-flow-viewer D9): window first, so a session name
/// containing spaces is recovered by parsing the two numeric columns off the **right**.
const PANES_FORMAT: &str = "#{session_name}:#{window_index} #{pane_active} #{pane_pid}";

/// At most this many panes of one window are published per tab (session-flow-viewer D9). The
/// active pane is always among them — it is the one the card describes — so a cap that could
/// drop it would make the split tree disagree with the card.
pub const MAX_PANES: usize = 8;

/// One attached tmux client: the tty it runs on, the pane it is showing, and that pane's window
/// (`session_name:window_index`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientRow {
    pub tty: PathBuf,
    pub pane_pid: u32,
    pub window: String,
}

/// One pane on the tmux server, as `list-panes -a` reports it.
///
/// `active` is `#{pane_active}` in a row from [`parse_panes`]. In a row returned by
/// [`panes_of`] it means "this is the client's pane" instead — the only meaning anything
/// downstream reads (session-flow-viewer D9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneRow {
    pub window: String,
    pub active: bool,
    pub pid: u32,
}

/// Pure: parse `tmux list-clients -F` [`CLIENTS_FORMAT`] output into its rows.
///
/// Tolerant **per row** — a short, malformed or non-numeric line is dropped individually rather
/// than failing the whole map, because one unparseable client must not blind the tab to a
/// second, valid one. A row with no window column is dropped too: tmux always prints one for
/// this format, so its absence means the line is not what was asked for. The window is the
/// rest of the line, so a session name containing spaces survives.
pub fn parse_clients(out: &str) -> Vec<ClientRow> {
    out.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, ' ');
            let tty = fields.next()?;
            let pane = fields.next()?.parse::<u32>().ok()?;
            let window = fields.next()?.trim_end();
            (pane > 0 && !tty.is_empty() && !window.is_empty()).then(|| ClientRow {
                tty: PathBuf::from(tty),
                pane_pid: pane,
                window: window.to_string(),
            })
        })
        .collect()
}

/// Pure: parse `tmux list-panes -a -F` [`PANES_FORMAT`] output into its rows.
///
/// Tolerant per row, on [`parse_clients`]' terms: a line whose last two columns are not a
/// `0`/`1` flag and a positive pid, or whose window is empty, is skipped on its own.
pub fn parse_panes(out: &str) -> Vec<PaneRow> {
    out.lines()
        .filter_map(|line| {
            let mut fields = line.trim_end().rsplitn(3, ' ');
            let pid = fields.next()?.parse::<u32>().ok()?;
            let active = match fields.next()? {
                "1" => true,
                "0" => false,
                _ => return None,
            };
            let window = fields.next()?;
            (pid > 0 && !window.is_empty()).then(|| PaneRow {
                window: window.to_string(),
                active,
                pid,
            })
        })
        .collect()
}

/// Pure: the panes of the window `client` shows, in tmux's order, with `active` true for
/// exactly the client's own pane (session-flow-viewer D9; R6.3, R6.4, R6.7).
///
/// Empty — the tab renders unsplit — when there is no client, when the window has fewer than
/// two panes, or when no listed pane is the client's. The last is the within-tick disagreement
/// between `list-clients` and `list-panes`, and publishing nothing beats guessing which pane the
/// card is describing. At most [`MAX_PANES`] rows: the client's pane is always kept, and the
/// others are taken in order until the cap.
pub fn panes_of(client: Option<&ClientRow>, panes: &[PaneRow]) -> Vec<PaneRow> {
    let Some(client) = client else {
        return Vec::new();
    };
    let in_window = || panes.iter().filter(|p| p.window == client.window);
    if in_window().count() < 2 || !in_window().any(|p| p.pid == client.pane_pid) {
        return Vec::new();
    }
    let mut others = 0usize;
    in_window()
        .filter(|p| {
            if p.pid == client.pane_pid {
                return true;
            }
            others += 1;
            others < MAX_PANES
        })
        .map(|p| PaneRow {
            window: p.window.clone(),
            active: p.pid == client.pane_pid,
            pid: p.pid,
        })
        .collect()
}

/// Resolve each client tty path to its device number, dropping rows whose tty cannot be
/// `stat`ed. The one injected seam: production passes [`super::proc::rdev_of_path`], tests pass
/// a fake device table so the join is exercised with no live tty.
///
/// Resolving here — once, at fetch — rather than at selection is what keeps the cost at N stats
/// per tick instead of N per tab per tick.
pub(crate) fn resolve_rdevs(
    rows: &[ClientRow],
    rdev_of: impl Fn(&Path) -> Option<u32>,
) -> Vec<(u32, u32)> {
    rows.iter()
        .filter_map(|row| rdev_of(&row.tty).map(|dev| (dev, row.pane_pid)))
        .collect()
}

/// Pure: the pane whose client holds `tty_dev`. First match wins, which makes the degenerate
/// case of two clients on one tty deterministic (by tmux's own output order) rather than
/// arbitrary — mirroring the walk's first-match-short-circuits rule.
pub fn select_pane(clients: &[(u32, u32)], tty_dev: u32) -> Option<u32> {
    clients
        .iter()
        .find(|(dev, _)| *dev == tty_dev)
        .map(|(_, pane)| *pane)
}

/// One `list-clients` invocation against the **default socket**. A non-zero exit (no server, no
/// sessions) yields no rows rather than an error.
///
/// A tab using a non-default socket (`tmux -L work`) has a server this query cannot see, so it
/// simply never matches and the tab keeps today's behaviour; the cost of that permanent miss is
/// bounded to the one memoised spawn per tick, not one per tab.
fn fetch_rows(exe: &Path) -> Vec<ClientRow> {
    // Bumped *before* the spawn, so a failed spawn still counts — which is what makes the
    // "at most one per tick" assertion meaningful rather than vacuous on the failure path.
    #[cfg(test)]
    super::probe::TMUX_LIST.bump(1);

    let out = match Command::new(exe)
        .args(["list-clients", "-F", CLIENTS_FORMAT])
        .output()
    {
        Ok(out) => out,
        Err(_) => return Vec::new(),
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_clients(&String::from_utf8_lossy(&out.stdout))
}

/// The per-tick client→pane map (D9's shape, one entry, replaced wholesale).
///
/// Unlike [`super::git::Memo`], which is keyed by cwd and therefore needs TTL expiry, eviction
/// and a size cap, this holds exactly one entry and **cannot grow** — so it needs no
/// reclamation logic at all.
pub(crate) struct Memo {
    entry: Option<Entry>,
}

struct Entry {
    tick: u64,
    fetched_at: Instant,
    /// Already-resolved `(rdev, pane_pid)` — not `(PathBuf, pane_pid)`, so selection is pure
    /// integer comparison and the `stat`s happen once per tick rather than once per tab.
    clients: Vec<(u32, u32)>,
    /// The parsed rows the map was resolved from, kept for one lookup: the window of the pane
    /// [`select_pane`] chose (session-flow-viewer D9). Never consulted for the selection itself,
    /// so the card's root is decided exactly as before.
    rows: Vec<ClientRow>,
}

impl Memo {
    pub(crate) fn new() -> Memo {
        Memo { entry: None }
    }

    /// A hit needs the tick to match **and** the entry to be younger than one tick. The tick
    /// alone is not a sound freshness key: the worker advances `tick_index` only on its timeout
    /// branch, while `SetActive` and `SetEnabled` derive at the current, unchanged index and the
    /// loop blocks indefinitely on an empty roster — so one tick value can span unbounded
    /// wall-clock time, and `SetActive` (fired the moment a user switches tabs) is exactly where
    /// a stale map would be most visible.
    fn get(&self, tick: u64) -> Option<&[(u32, u32)]> {
        self.fresh(tick).map(|e| e.clients.as_slice())
    }

    fn fresh(&self, tick: u64) -> Option<&Entry> {
        self.entry
            .as_ref()
            .filter(|e| e.tick == tick && e.fetched_at.elapsed() < TICK)
    }

    /// The client row showing `pane`, from the same fresh entry [`Memo::get`] served.
    ///
    /// Looked up by pane pid rather than by device because the rows are kept unresolved. Two
    /// clients showing one pane are showing one window (a grouped session aside, whose other
    /// name lists the same panes), so the first such row names the right pane set.
    fn client_showing(&self, tick: u64, pane: u32) -> Option<&ClientRow> {
        self.fresh(tick)?.rows.iter().find(|r| r.pane_pid == pane)
    }

    /// Every fetch attempt writes an entry — **including a failed one**, which stores an empty
    /// row set. Caching only successes would mean a machine with N tmux tabs and no reachable
    /// server re-spawns once per *tab* per tick.
    fn put_rows(&mut self, tick: u64, clients: Vec<(u32, u32)>, rows: Vec<ClientRow>) {
        self.entry = Some(Entry {
            tick,
            fetched_at: Instant::now(),
            clients,
            rows,
        });
    }

    /// [`Memo::put_rows`] with no rows, for the memo tests that predate the window column.
    #[cfg(test)]
    fn put(&mut self, tick: u64, clients: Vec<(u32, u32)>) {
        self.put_rows(tick, clients, Vec::new());
    }
}

/// The client a re-rooted tab is attached to, and the tmux binary that found it — the second is
/// what [`panes_for`] spawns `list-panes` with, for [`fetch_rows`]' `PATH` reason.
pub(crate) struct Attached {
    pub(crate) client: ClientRow,
    pub(crate) exe: PathBuf,
}

/// [`effective_root`]'s answer plus, when it re-rooted, the client it re-rooted through
/// (session-flow-viewer D9). `attached` is `Some` only when `root` is a tmux pane.
pub(crate) struct Rooted {
    pub(crate) root: u32,
    pub(crate) attached: Option<Attached>,
}

/// The derivation root for `pid`: the tmux client's active `pane_pid` when this tab is running
/// one, else `pid` unchanged.
///
/// Returns `u32`, never `Option<u32>`, so no caller has to branch on absence — every failure
/// (no tmux, no tty, a dead server, unparseable output, an unstat-able tty, no matching client,
/// or a pane that has since exited) resolves to the pid that was passed in, which is precisely
/// today's behaviour.
pub(crate) fn effective_root(memo: &mut Memo, pid: u32, tick: u64) -> u32 {
    rooted(memo, pid, tick).root
}

/// [`effective_root`] keeping the client it re-rooted through, for the worker's pane pass
/// (session-flow-viewer D9). Both run the one body, so the card's root cannot drift from the
/// `root` this returns.
pub(crate) fn rooted(memo: &mut Memo, pid: u32, tick: u64) -> Rooted {
    rooted_with(
        memo,
        pid,
        tick,
        tmux_client_of,
        super::proc::tty_dev_of_pid,
        fetch_rows,
        super::proc::rdev_of_path,
        |pane| super::proc::cwd_of_pid(pane).is_some(),
    )
}

/// [`effective_root`]'s body, generic over all five of its impure operations — the same
/// injected-seam idiom [`super::agent::walk`] and [`super::agent::classify_with`] use.
///
/// Without this seam the orchestration is untestable: on any machine running the suite the
/// presence gate closes immediately, so the memo re-use, the negative-cache and the liveness
/// branches are all unreachable and their "measurements" would be tautologies rather than
/// counts. With it, every branch is exercised against fakes — no tmux, no live tty, no live pid.
#[allow(clippy::too_many_arguments)]
// five seams + the three real inputs; the alternative is a
// params struct that no production caller would ever build
#[cfg(test)]
fn effective_root_with(
    memo: &mut Memo,
    pid: u32,
    tick: u64,
    gate: impl Fn(u32) -> Option<TmuxClient>,
    tty_of: impl Fn(u32) -> Option<u32>,
    fetch: impl Fn(&Path) -> Vec<ClientRow>,
    rdev_of: impl Fn(&Path) -> Option<u32>,
    alive: impl Fn(u32) -> bool,
) -> u32 {
    rooted_with(memo, pid, tick, gate, tty_of, fetch, rdev_of, alive).root
}

/// [`rooted`]'s body: [`effective_root_with`]'s seams, returning the client as well.
#[allow(clippy::too_many_arguments)] // as `effective_root_with`
fn rooted_with(
    memo: &mut Memo,
    pid: u32,
    tick: u64,
    gate: impl Fn(u32) -> Option<TmuxClient>,
    tty_of: impl Fn(u32) -> Option<u32>,
    fetch: impl Fn(&Path) -> Vec<ClientRow>,
    rdev_of: impl Fn(&Path) -> Option<u32>,
    alive: impl Fn(u32) -> bool,
) -> Rooted {
    let unrooted = Rooted {
        root: pid,
        attached: None,
    };
    // Presence gate first: a tab with no tmux pays nothing beyond this — no spawn, no tty read.
    let Some(client) = gate(pid) else {
        return unrooted;
    };
    let Some(tty_dev) = tty_of(pid) else {
        return unrooted;
    };

    if memo.get(tick).is_none() {
        let rows = fetch(&client.exe);
        let resolved = resolve_rdevs(&rows, &rdev_of);
        memo.put_rows(tick, resolved, rows);
    }
    let Some(clients) = memo.get(tick) else {
        return unrooted;
    };

    match select_pane(clients, tty_dev) {
        // A resolved-but-dead pane is worse than no re-root: every downstream read would come
        // back absent and the tab card would go *blank*, where today it still shows the tab
        // shell's own branch and agent. One syscall buys that back.
        Some(pane) if alive(pane) => Rooted {
            root: pane,
            attached: memo.client_showing(tick, pane).map(|row| Attached {
                client: row.clone(),
                exe: client.exe,
            }),
        },
        _ => unrooted,
    }
}

/// One `list-panes -a` against the default socket, on [`fetch_rows`]' terms: a non-zero exit
/// (no server) or a failed spawn is no rows, which [`panes_of`] turns into an unsplit tab
/// (session-flow-viewer D14, R6.7).
fn fetch_panes(exe: &Path) -> Vec<PaneRow> {
    // Bumped before the spawn, for `fetch_rows`' reason: a failed spawn must still count.
    #[cfg(test)]
    super::probe::TMUX_PANES.bump(1);

    let out = match Command::new(exe)
        .args(["list-panes", "-a", "-F", PANES_FORMAT])
        .output()
    {
        Ok(out) => out,
        Err(_) => return Vec::new(),
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_panes(&String::from_utf8_lossy(&out.stdout))
}

/// The per-tick pane list — [`Memo`]'s shape and validity rule (tick **and** wall clock), one
/// entry replaced wholesale, a failed fetch cached as empty (session-flow-viewer D9).
pub(crate) struct PaneMemo {
    entry: Option<PaneEntry>,
}

struct PaneEntry {
    tick: u64,
    fetched_at: Instant,
    panes: Vec<PaneRow>,
}

impl PaneMemo {
    pub(crate) fn new() -> PaneMemo {
        PaneMemo { entry: None }
    }

    /// A memo already holding `panes` for `tick`, so a sibling module's test can drive the
    /// pane pass without a tmux server (the fresh-memo path spawns nothing).
    #[cfg(test)]
    pub(crate) fn seeded(tick: u64, panes: Vec<PaneRow>) -> PaneMemo {
        let mut memo = PaneMemo::new();
        memo.put(tick, panes);
        memo
    }

    fn get(&self, tick: u64) -> Option<&[PaneRow]> {
        self.entry
            .as_ref()
            .filter(|e| e.tick == tick && e.fetched_at.elapsed() < TICK)
            .map(|e| e.panes.as_slice())
    }

    fn put(&mut self, tick: u64, panes: Vec<PaneRow>) {
        self.entry = Some(PaneEntry {
            tick,
            fetched_at: Instant::now(),
            panes,
        });
    }
}

/// The panes of the window a tab's client shows ([`panes_of`]), listing the server at most once
/// per tick. A tab with no client spawns nothing: the list is fetched only for a tab that
/// [`rooted`] actually re-rooted.
pub(crate) fn panes_for(
    memo: &mut PaneMemo,
    attached: Option<&Attached>,
    tick: u64,
) -> Vec<PaneRow> {
    panes_for_with(memo, attached, tick, fetch_panes)
}

/// [`panes_for`] with its one spawn injected.
fn panes_for_with(
    memo: &mut PaneMemo,
    attached: Option<&Attached>,
    tick: u64,
    fetch: impl Fn(&Path) -> Vec<PaneRow>,
) -> Vec<PaneRow> {
    let Some(attached) = attached else {
        return Vec::new();
    };
    if memo.get(tick).is_none() {
        memo.put(tick, fetch(&attached.exe));
    }
    panes_of(Some(&attached.client), memo.get(tick).unwrap_or_default())
}

/// The unmemoised form, for the event-driven consumer (the ai-monitor overlay's session hint),
/// which has no tick to key a memo on. One spawn per call — a user-initiated cost, not a
/// cadence cost, so it needs no budget.
pub(crate) fn effective_root_uncached(pid: u32) -> u32 {
    effective_root(&mut Memo::new(), pid, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_client_scan_inherits_the_deterministic_child_order() {
        // R1.6 — `tmux_client_of_with` returns its FIRST matching child, so its answer is only
        // as deterministic as the child list it is handed. In production that list comes from
        // `proc::children_of` → `parse_childpids`, which sorts ascending
        // (tabmeta-session-keying D11).
        //
        // The provider is fed the **raw, unsorted** order a kernel might return and the list
        // is put through the production path (`parse_childpids`) rather than sorted inline by
        // the test. An earlier version sorted its own input before feeding it in, which made
        // the assertion a tautology about `sort_unstable`: deleting the sort from
        // `parse_childpids` left this test green.
        let mut raw = [0i32; 128];
        raw[0] = 7;
        raw[1] = 3;
        raw[2] = 9;
        let children = super::super::proc::parse_childpids(&raw, 3);

        let exe_of = |pid: u32| -> Option<PathBuf> {
            match pid {
                1 => Some(PathBuf::from("/bin/zsh")), // the root is not a client
                _ => Some(PathBuf::from("/opt/homebrew/bin/tmux")),
            }
        };
        let got = tmux_client_of_with(1, exe_of, move |_| children.clone());
        assert_eq!(
            got.map(|c| c.pid),
            Some(3),
            "the lowest pid wins — if `parse_childpids` stops sorting, this returns 7"
        );
    }
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// A fake exe-path provider. Split from [`kids_of`] rather than returning both from one
    /// helper so each has a single reference parameter and lifetime elision applies.
    fn exe_of<'a>(exes: &'a HashMap<u32, &'static str>) -> impl Fn(u32) -> Option<PathBuf> + 'a {
        move |pid| exes.get(&pid).map(PathBuf::from)
    }

    /// A fake children provider.
    fn kids_of(kids: &HashMap<u32, Vec<u32>>) -> impl Fn(u32) -> Vec<u32> + '_ {
        move |pid| kids.get(&pid).cloned().unwrap_or_default()
    }

    // ---- the presence gate (R2.1-R2.6) ----

    #[test]
    fn gate_finds_tmux_at_depth_zero() {
        // R2.1 — `exec tmux` replaces the shell's image at the same pid, so the tab has no
        // tmux *child* at all. A children-only gate would never fire for these users.
        let exes = HashMap::from([(1u32, "/opt/homebrew/bin/tmux")]);
        let kids = HashMap::new();
        let got = tmux_client_of_with(1, exe_of(&exes), kids_of(&kids)).expect("root is tmux");
        assert_eq!(got.pid, 1);
        assert_eq!(got.exe, PathBuf::from("/opt/homebrew/bin/tmux"));
    }

    #[test]
    fn gate_finds_tmux_at_depth_one() {
        // R2.2 — the measured interactive case: zsh -> tmux.
        let exes = HashMap::from([(1u32, "/bin/zsh"), (2u32, "/opt/homebrew/bin/tmux")]);
        let kids = HashMap::from([(1u32, vec![2u32])]);
        let got = tmux_client_of_with(1, exe_of(&exes), kids_of(&kids)).expect("child is tmux");
        assert_eq!(got.pid, 2);
    }

    #[test]
    fn gate_prefers_the_root_over_a_child() {
        // R2.3 — both are tmux; depth 0 wins, so the result is deterministic.
        let exes = HashMap::from([(1u32, "/usr/bin/tmux"), (2u32, "/usr/bin/tmux")]);
        let kids = HashMap::from([(1u32, vec![2u32])]);
        assert_eq!(
            tmux_client_of_with(1, exe_of(&exes), kids_of(&kids))
                .unwrap()
                .pid,
            1
        );
    }

    #[test]
    fn gate_does_not_reach_depth_two() {
        // R2.4 — the accepted Known Gap: a tmux behind a wrapper is not found, and the tab
        // simply keeps today's behaviour.
        let exes = HashMap::from([
            (1u32, "/bin/zsh"),
            (2u32, "/bin/sh"),
            (3u32, "/usr/bin/tmux"),
        ]);
        let kids = HashMap::from([(1u32, vec![2u32]), (2u32, vec![3u32])]);
        assert!(tmux_client_of_with(1, exe_of(&exes), kids_of(&kids)).is_none());
    }

    #[test]
    fn gate_basename_match_is_exact_and_case_insensitive() {
        // R2.5 — near-misses must not be treated as the tab's tmux.
        for (path, want) in [
            ("/opt/homebrew/bin/tmux", true),
            ("TMUX", true),
            ("/usr/local/bin/TmUx", true),
            ("tmux-next", false),
            ("mytmux", false),
            ("tm", false),
            ("/usr/bin/tmuxinator", false),
        ] {
            assert_eq!(is_tmux(Path::new(path)), want, "{path}");
        }
    }

    #[test]
    fn gate_on_a_plain_shell_finds_nothing() {
        let exes = HashMap::from([(1u32, "/bin/zsh"), (2u32, "/usr/bin/vim")]);
        let kids = HashMap::from([(1u32, vec![2u32])]);
        assert!(tmux_client_of_with(1, exe_of(&exes), kids_of(&kids)).is_none());
    }

    // ---- parsing (R3.2, R3.3) ----

    #[test]
    fn parse_clients_reads_one_row_per_client_in_order() {
        // R3.2 — the measured output shape, order preserved (D11 depends on it).
        // (session-flow-viewer D9 added the window column; the rows carry it.)
        let got = parse_clients("/dev/ttys002 74877 main:0\n/dev/ttys004 38200 work:1\n");
        assert_eq!(
            got,
            vec![
                row("/dev/ttys002", 74877, "main:0"),
                row("/dev/ttys004", 38200, "work:1"),
            ]
        );
    }

    #[test]
    fn parse_clients_drops_bad_rows_individually() {
        // R3.3 — one unparseable client must not blind the tab to a valid one.
        let got = parse_clients(
            "/dev/ttys002 74877 main:0\n\
             /dev/ttys003\n\
             /dev/ttys004 notanumber main:0\n\
             \n\
             /dev/ttys005 0 main:0\n\
             /dev/ttys006 4100\n",
        );
        assert_eq!(got, vec![row("/dev/ttys002", 74877, "main:0")]);
        assert!(parse_clients("").is_empty());
    }

    /// A parsed client row, for the fixtures below.
    fn row(tty: &str, pane_pid: u32, window: &str) -> ClientRow {
        ClientRow {
            tty: PathBuf::from(tty),
            pane_pid,
            window: window.to_string(),
        }
    }

    // ---- pane enumeration (session-flow-viewer R6.1-R6.4, R6.7) ----

    fn pane(window: &str, active: bool, pid: u32) -> PaneRow {
        PaneRow {
            window: window.to_string(),
            active,
            pid,
        }
    }

    #[test]
    fn parse_clients_keeps_a_session_name_with_spaces() {
        let got = parse_clients("/dev/ttys004 4100 my work:1\n");
        assert_eq!(got, vec![row("/dev/ttys004", 4100, "my work:1")]);
    }

    #[test]
    fn parse_panes_reads_rows_and_skips_malformed_lines() {
        let got = parse_panes(
            "work:1 1 4100\nwork:1 0 4200\nmisc:0 1 5000\ngarbage\nwork:1 2 4300\nwork:1 0 0\n \
             0 4400\nmy work:2 0 4500\n",
        );
        assert_eq!(
            got,
            vec![
                pane("work:1", true, 4100),
                pane("work:1", false, 4200),
                pane("misc:0", true, 5000),
                pane("my work:2", false, 4500),
            ]
        );
        assert!(parse_panes("").is_empty());
    }

    #[test]
    fn panes_of_marks_the_clients_pane_active_whatever_tmux_said() {
        let client = row("/dev/ttys004", 4100, "work:1");
        // tmux claims 4200 is active; the client's pid wins.
        let panes = [
            pane("work:1", false, 4100),
            pane("work:1", true, 4200),
            pane("misc:0", true, 5000),
        ];
        let got = panes_of(Some(&client), &panes);
        assert_eq!(
            got,
            vec![pane("work:1", true, 4100), pane("work:1", false, 4200)]
        );
        // No pane is the client's → empty, not a guess.
        let stale = row("/dev/ttys004", 9999, "work:1");
        assert!(panes_of(Some(&stale), &panes).is_empty());
    }

    #[test]
    fn panes_of_is_empty_for_one_pane_no_client_or_no_panes() {
        let client = row("/dev/ttys004", 4100, "work:1");
        assert!(panes_of(Some(&client), &[pane("work:1", true, 4100)]).is_empty());
        assert!(panes_of(
            None,
            &[pane("work:1", true, 4100), pane("work:1", false, 1)]
        )
        .is_empty());
        assert!(panes_of(Some(&client), &[]).is_empty());
    }

    #[test]
    fn panes_of_caps_at_max_panes_keeping_the_active_one() {
        // Ten panes with the client's last: the first seven others plus it.
        let panes: Vec<PaneRow> = (1..=10).map(|i| pane("w:0", false, i)).collect();
        let client = row("/dev/ttys004", 10, "w:0");
        let got = panes_of(Some(&client), &panes);
        assert_eq!(got.len(), MAX_PANES);
        assert_eq!(
            got.iter().map(|p| p.pid).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6, 7, 10]
        );
        assert_eq!(got.iter().filter(|p| p.active).count(), 1);
        // The active one first: it and the next seven.
        let first = row("/dev/ttys004", 1, "w:0");
        let got = panes_of(Some(&first), &panes);
        assert_eq!(
            got.iter().map(|p| p.pid).collect::<Vec<_>>(),
            (1..=8).collect::<Vec<_>>()
        );
        // Exactly MAX_PANES panes are all kept.
        let eight: Vec<PaneRow> = (1..=8).map(|i| pane("w:0", false, i)).collect();
        assert_eq!(panes_of(Some(&first), &eight).len(), MAX_PANES);
    }

    #[test]
    fn panes_for_lists_once_per_tick_and_never_without_a_client() {
        let fetches = RefCell::new(0usize);
        let fetch = |_: &Path| {
            *fetches.borrow_mut() += 1;
            parse_panes("work:1 1 4100\nwork:1 0 4200\n")
        };
        let attached = Attached {
            client: row("/dev/ttys004", 4100, "work:1"),
            exe: PathBuf::from("/opt/homebrew/bin/tmux"),
        };
        let mut memo = PaneMemo::new();
        assert!(panes_for_with(&mut memo, None, 3, fetch).is_empty());
        assert_eq!(*fetches.borrow(), 0, "no client -> no spawn");
        for _ in 0..5 {
            assert_eq!(
                panes_for_with(&mut memo, Some(&attached), 3, fetch).len(),
                2
            );
        }
        assert_eq!(*fetches.borrow(), 1, "one list-panes per tick");
        panes_for_with(&mut memo, Some(&attached), 4, fetch);
        assert_eq!(*fetches.borrow(), 2, "a new tick refetches");
        // A failed fetch is cached as empty and degrades to unsplit.
        let mut failed = PaneMemo::new();
        let empty = |_: &Path| {
            *fetches.borrow_mut() += 1;
            Vec::new()
        };
        assert!(panes_for_with(&mut failed, Some(&attached), 9, empty).is_empty());
        assert!(panes_for_with(&mut failed, Some(&attached), 9, empty).is_empty());
        assert_eq!(*fetches.borrow(), 3, "the failure was cached");
    }

    /// A shell script standing in for `tmux`: prints `stdout` and exits with `code`, so the
    /// exit-status check in the fetchers is exercised with real process output.
    fn fake_tmux(tag: &str, stdout: &str, code: i32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = std::env::temp_dir().join(format!("fake-tmux-{}-{tag}", std::process::id()));
        std::fs::write(&p, format!("#!/bin/sh\nprintf '{stdout}'\nexit {code}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn a_non_zero_exit_discards_whatever_tmux_printed() {
        // The exit status, not the output, decides (D9): a server that prints rows and then
        // fails must not be trusted, and a clean exit with the same rows must be.
        let _g = super::super::probe::serial();
        let panes = "work:1 1 4100\\n";
        let ok = fake_tmux("panes-ok", panes, 0);
        let bad = fake_tmux("panes-bad", panes, 1);
        assert_eq!(fetch_panes(&ok).len(), 1);
        assert!(fetch_panes(&bad).is_empty());
        let clients = "/dev/ttys004 4100 work:1\\n";
        let ok = fake_tmux("clients-ok", clients, 0);
        let bad = fake_tmux("clients-bad", clients, 1);
        assert_eq!(fetch_rows(&ok).len(), 1);
        assert!(fetch_rows(&bad).is_empty());
        for tag in ["panes-ok", "panes-bad", "clients-ok", "clients-bad"] {
            let _ = std::fs::remove_file(
                std::env::temp_dir().join(format!("fake-tmux-{}-{tag}", std::process::id())),
            );
        }
    }

    #[test]
    fn panes_for_serves_a_fresh_memo_without_spawning() {
        // The production entry point, not the injected one: a list already fetched this tick
        // is the answer, so even a bogus binary path spawns nothing and the window's panes
        // come back (D9's one `list-panes` per tick).
        let attached = Attached {
            client: row("/dev/ttys004", 4100, "work:1"),
            exe: PathBuf::from("/definitely/not/a/tmux/binary"),
        };
        let mut memo = PaneMemo::new();
        memo.put(7, parse_panes("work:1 1 4100\nwork:1 0 4200\n"));
        let got = panes_for(&mut memo, Some(&attached), 7);
        assert_eq!(got.iter().map(|p| p.pid).collect::<Vec<_>>(), [4100, 4200]);
    }

    #[test]
    fn a_failed_list_panes_spawn_is_no_rows_and_still_counts() {
        let _g = super::super::probe::serial();
        super::super::probe::reset_all();
        assert!(fetch_panes(Path::new("/definitely/not/a/tmux/binary")).is_empty());
        assert_eq!(super::super::probe::TMUX_PANES.get(), 1);
    }

    #[test]
    fn rooted_carries_the_client_it_re_rooted_through() {
        let mut memo = Memo::new();
        let got = rooted_with(
            &mut memo,
            1,
            0,
            tmux_gate(),
            |_| Some(TAB_DEV),
            two_clients,
            fake_rdev,
            |_| true,
        );
        assert_eq!(got.root, 74877);
        let attached = got.attached.expect("re-rooted -> attached");
        assert_eq!(attached.client, row("/dev/ttys002", 74877, "main:0"));
        assert_eq!(attached.exe, PathBuf::from("/opt/homebrew/bin/tmux"));
        // Not re-rooted (dead pane) → no client.
        let mut memo = Memo::new();
        let got = rooted_with(
            &mut memo,
            1,
            0,
            tmux_gate(),
            |_| Some(TAB_DEV),
            two_clients,
            fake_rdev,
            |_| false,
        );
        assert_eq!(got.root, 1);
        assert!(got.attached.is_none());
    }

    // ---- device resolution + selection (R4.1-R4.6) ----

    fn devs<'a>(table: &'a HashMap<&'static str, u32>) -> impl Fn(&Path) -> Option<u32> + 'a {
        move |p| table.get(p.to_string_lossy().as_ref()).copied()
    }

    #[test]
    fn resolve_rdevs_maps_each_tty_once() {
        // R4.1
        let table = HashMap::from([("/dev/ttys002", 0x1000_0002u32)]);
        let rows = vec![row("/dev/ttys002", 74877, "main:0")];
        assert_eq!(
            resolve_rdevs(&rows, devs(&table)),
            vec![(0x1000_0002, 74877)]
        );
    }

    #[test]
    fn resolve_rdevs_drops_unstatable_rows() {
        // R4.2 — an absent tty is dropped, not fatal.
        let table = HashMap::from([("/dev/ttys004", 0x1000_0004u32)]);
        let rows = vec![
            row("/dev/ttys002", 74877, "main:0"),
            row("/dev/ttys004", 38200, "main:0"),
        ];
        assert_eq!(
            resolve_rdevs(&rows, devs(&table)),
            vec![(0x1000_0004, 38200)]
        );
    }

    #[test]
    fn select_pane_matches_on_device_number() {
        // R4.3, R4.4, R4.6
        let clients = [(0x1000_0002u32, 74877u32), (0x1000_0004, 38200)];
        assert_eq!(select_pane(&clients, 0x1000_0004), Some(38200));
        assert_eq!(select_pane(&clients, 0x1000_0009), None);
        assert_eq!(select_pane(&[], 0x1000_0002), None);
    }

    #[test]
    fn select_pane_takes_the_first_of_duplicate_devices() {
        // R4.5 — deterministic by tmux's output order, never arbitrary.
        let clients = [(0x1000_0002u32, 111u32), (0x1000_0002, 222)];
        assert_eq!(select_pane(&clients, 0x1000_0002), Some(111));
    }

    // ---- the memo (R5.1-R5.6) ----

    #[test]
    fn memo_hits_within_the_same_tick_and_misses_on_a_new_one() {
        // R5.1, R5.2
        let mut m = Memo::new();
        assert!(m.get(5).is_none(), "cold memo is a miss");
        m.put(5, vec![(0x1000_0002, 74877)]);
        assert_eq!(m.get(5), Some([(0x1000_0002u32, 74877u32)].as_slice()));
        assert!(m.get(6).is_none(), "a new tick must refetch");
    }

    #[test]
    fn memo_caches_a_failed_fetch_too() {
        // R5.4 — negative caching is what makes "<= 1 spawn per tick" hold on the failure
        // path; without it, N tmux tabs with no reachable server would spawn N times.
        let mut m = Memo::new();
        m.put(5, Vec::new());
        assert_eq!(
            m.get(5),
            Some([].as_slice()),
            "an empty result is still a hit"
        );
    }

    #[test]
    fn memo_holds_at_most_one_entry() {
        // R5.5 — it is replaced wholesale, so it cannot grow and needs no reclamation.
        let mut m = Memo::new();
        for tick in 0..100 {
            m.put(tick, vec![(tick as u32, tick as u32)]);
            assert!(m.entry.is_some());
        }
        assert_eq!(m.get(99).map(|c| c.len()), Some(1));
        assert!(m.get(0).is_none(), "only the newest entry survives");
    }

    #[test]
    fn memo_entry_older_than_a_tick_is_stale_even_at_the_same_index() {
        // R5.3 — the wall-clock half of the validity rule. `SetActive` derives at an
        // *unadvanced* tick index, so a tick-only key could serve an arbitrarily old map.
        let mut m = Memo::new();
        m.put(5, vec![(0x1000_0002, 74877)]);
        // Backdate the entry past one tick without sleeping.
        if let Some(e) = m.entry.as_mut() {
            e.fetched_at = Instant::now() - (TICK + std::time::Duration::from_millis(1));
        }
        assert!(
            m.get(5).is_none(),
            "stale by wall clock despite a tick match"
        );
    }

    // ---- the public entry points (R0.2, R6.1-R6.4) ----

    #[test]
    fn a_tab_with_no_tmux_is_returned_unchanged_and_spawns_nothing() {
        // R0.2 / R6.4 — the test binary has no tmux at depth 0 or 1, so the gate closes and
        // TMUX_LIST stays 0. (Note the gate looks *down*; a developer running `cargo test`
        // inside a tmux pane has tmux as an ancestor, so this stays stable either way.)
        let _g = super::super::probe::serial();
        super::super::probe::reset_all();
        let me = std::process::id();
        let mut memo = Memo::new();
        assert_eq!(effective_root(&mut memo, me, 0), me);
        assert_eq!(
            super::super::probe::TMUX_LIST.get(),
            0,
            "no tmux in the tree -> no spawn"
        );
        assert_eq!(
            super::super::probe::PROC_TTY.get(),
            0,
            "the gate closes before the tty read"
        );
    }

    #[test]
    fn a_dead_pid_is_returned_unchanged_without_panicking() {
        // R6.4 — pid 0 cannot exist; the gate finds nothing and the input pid comes back.
        let _g = super::super::probe::serial();
        let mut memo = Memo::new();
        assert_eq!(effective_root(&mut memo, 0, 0), 0);
        assert_eq!(effective_root_uncached(0), 0);
        assert_eq!(effective_root_uncached(u32::MAX), u32::MAX);
    }

    #[test]
    fn the_uncached_entry_point_agrees_with_a_cold_memo() {
        // R6.2 — the event-driven consumer gets the same answer, with no tick to key on.
        let _g = super::super::probe::serial();
        let me = std::process::id();
        let mut memo = Memo::new();
        assert_eq!(
            effective_root_uncached(me),
            effective_root(&mut memo, me, 0)
        );
    }

    // ---- the cost budget, counted rather than argued (R7.1-R7.3) ----

    #[test]
    fn the_three_new_counters_reset_with_the_rest() {
        // R7.1 — a counter missing from reset_all() would silently carry state between cost
        // tests and make every later assertion meaningless.
        let _g = super::super::probe::serial();
        super::super::probe::PROC_TTY.bump(3);
        super::super::probe::TTY_STAT.bump(5);
        super::super::probe::TMUX_LIST.bump(7);
        super::super::probe::reset_all();
        assert_eq!(super::super::probe::PROC_TTY.get(), 0);
        assert_eq!(super::super::probe::TTY_STAT.get(), 0);
        assert_eq!(super::super::probe::TMUX_LIST.get(), 0);
    }

    #[test]
    fn the_gate_reads_the_root_then_each_child() {
        // R2.6 / D9 — the gate's exact syscall shape, proven against a fake tree so it is
        // deterministic. (Against the real process table this would be racy: `k` is a live
        // child count that a concurrently-running test spawning a process can change.)
        // `exe_path(root)` is the genuinely NEW read — `agent::walk` never classifies its
        // root, so no fusion with the agent walk could ever recover it.
        let exes = HashMap::from([
            (1u32, "/bin/zsh"),
            (2u32, "/usr/bin/vim"),
            (3u32, "/usr/bin/less"),
        ]);
        let kids = HashMap::from([(1u32, vec![2u32, 3u32])]);
        let exe_calls = RefCell::new(0usize);
        let child_calls = RefCell::new(0usize);
        let got = tmux_client_of_with(
            1,
            |pid| {
                *exe_calls.borrow_mut() += 1;
                exe_of(&exes)(pid)
            },
            |pid| {
                *child_calls.borrow_mut() += 1;
                kids_of(&kids)(pid)
            },
        );
        assert!(got.is_none(), "no tmux in this tree");
        assert_eq!(*child_calls.borrow(), 1, "exactly one child listing");
        assert_eq!(
            *exe_calls.borrow(),
            3,
            "1 for the root (depth 0) + k for its 2 direct children"
        );
    }

    #[test]
    fn tmux_list_counts_attempts_not_successes() {
        // R7.2 — the counter is bumped before the spawn, so a spawn that cannot even execute
        // still counts. Without this, "at most one spawn per tick" would be vacuously true on
        // exactly the path where over-spawning would hurt (a machine with no reachable server).
        let _g = super::super::probe::serial();
        super::super::probe::reset_all();
        let rows = fetch_rows(Path::new("/definitely/not/a/tmux/binary"));
        assert!(
            rows.is_empty(),
            "a failed spawn yields no rows, not a panic"
        );
        assert_eq!(
            super::super::probe::TMUX_LIST.get(),
            1,
            "a failed spawn must still count"
        );
    }

    // (R7.3's "N stats, not M x N" is measured end-to-end by
    // `m_tabs_in_one_tick_share_one_fetch_and_n_stats` below. An earlier test asserted
    // `TTY_STAT == 0` while calling only `select_pane` — a function that contains no `stat`,
    // so the assertion could not fail. It was deleted rather than kept as apparent coverage.)

    // ---- effective_root end-to-end, against fakes (R0.1, R5.1/R5.2/R5.4/R5.6, R6.3, R7.3) ----

    /// A tab whose shell (pid 1) has a tmux client child (pid 2) on tty device `0x1000_0002`.
    fn tmux_gate() -> impl Fn(u32) -> Option<TmuxClient> {
        |pid| {
            (pid == 1).then(|| TmuxClient {
                pid: 2,
                exe: PathBuf::from("/opt/homebrew/bin/tmux"),
            })
        }
    }
    const TAB_DEV: u32 = 0x1000_0002;
    fn two_clients(_: &Path) -> Vec<ClientRow> {
        parse_clients("/dev/ttys002 74877 main:0\n/dev/ttys004 38200 work:1\n")
    }
    fn fake_rdev(p: &Path) -> Option<u32> {
        match p.to_string_lossy().as_ref() {
            "/dev/ttys002" => Some(TAB_DEV),
            "/dev/ttys004" => Some(0x1000_0004),
            _ => None,
        }
    }

    #[test]
    fn a_tmux_tab_resolves_to_its_own_clients_pane() {
        // R0.1 — the whole point: two clients are attached, and the tty join picks THIS tab's
        // pane (74877), not the other terminal application's (38200).
        let mut memo = Memo::new();
        let got = effective_root_with(
            &mut memo,
            1,
            0,
            tmux_gate(),
            |_| Some(TAB_DEV),
            two_clients,
            fake_rdev,
            |_| true,
        );
        assert_eq!(got, 74877);
    }

    #[test]
    fn a_dead_pane_falls_back_to_the_tab_instead_of_blanking_the_card() {
        // R6.3 / D13 — the liveness gate. A resolved-but-dead pane would make every downstream
        // read absent and blank the card, which is strictly worse than not re-rooting at all.
        let mut memo = Memo::new();
        let got = effective_root_with(
            &mut memo,
            1,
            0,
            tmux_gate(),
            |_| Some(TAB_DEV),
            two_clients,
            fake_rdev,
            |_| false, // the pane exited between resolution and use
        );
        assert_eq!(got, 1, "falls back to the tab's own pid");
    }

    #[test]
    fn no_matching_client_row_falls_back_to_the_tab() {
        // R6.4 — a tab whose tty matches no attached client (e.g. D14's non-default socket).
        let mut memo = Memo::new();
        let got = effective_root_with(
            &mut memo,
            1,
            0,
            tmux_gate(),
            |_| Some(0x1000_00ff), // a device no client holds
            two_clients,
            fake_rdev,
            |_| true,
        );
        assert_eq!(got, 1);
    }

    #[test]
    fn an_absent_tty_device_returns_the_input_pid_without_fetching() {
        // R6.4 — the tty read is the second gate; failing it must not spawn.
        let _g = super::super::probe::serial();
        super::super::probe::reset_all();
        let mut memo = Memo::new();
        let fetches = RefCell::new(0usize);
        let got = effective_root_with(
            &mut memo,
            1,
            0,
            tmux_gate(),
            |_| None, // no controlling tty
            |p| {
                *fetches.borrow_mut() += 1;
                two_clients(p)
            },
            fake_rdev,
            |_| true,
        );
        assert_eq!(got, 1);
        assert_eq!(*fetches.borrow(), 0, "no tty -> no spawn");
    }

    #[test]
    fn m_tabs_in_one_tick_share_one_fetch_and_n_stats() {
        // R5.1 + R7.3 — the memo's whole purpose, measured rather than asserted: TEN tabs
        // deriving within one tick cost ONE list-clients and N stats (N = attached clients),
        // not M x N. This is the assertion the previous select_pane-only test could not make.
        let fetches = RefCell::new(0usize);
        let stats = RefCell::new(0usize);
        let mut memo = Memo::new();
        for _ in 0..10 {
            effective_root_with(
                &mut memo,
                1,
                7, // same tick
                tmux_gate(),
                |_| Some(TAB_DEV),
                |p| {
                    *fetches.borrow_mut() += 1;
                    two_clients(p)
                },
                |p| {
                    *stats.borrow_mut() += 1;
                    fake_rdev(p)
                },
                |_| true,
            );
        }
        assert_eq!(*fetches.borrow(), 1, "one spawn per tick, not per tab");
        assert_eq!(*stats.borrow(), 2, "N stats (2 clients), not M x N = 20");
    }

    #[test]
    fn a_failed_fetch_is_not_retried_within_the_same_tick() {
        // R5.4 — negative caching, measured. Without it, N tmux tabs on a machine with no
        // reachable server would spawn N times per tick forever.
        let fetches = RefCell::new(0usize);
        let mut memo = Memo::new();
        for _ in 0..5 {
            let got = effective_root_with(
                &mut memo,
                1,
                3,
                tmux_gate(),
                |_| Some(TAB_DEV),
                |_| {
                    *fetches.borrow_mut() += 1;
                    Vec::new() // non-zero exit / no server
                },
                fake_rdev,
                |_| true,
            );
            assert_eq!(got, 1, "no rows -> no re-root");
        }
        assert_eq!(*fetches.borrow(), 1, "the failure was cached, not retried");
    }

    #[test]
    fn a_stale_entry_refetches_even_at_the_same_tick_index() {
        // R5.3 / D7(d) — the wall-clock half of the validity rule, end-to-end rather than by
        // poking `Memo` directly. `SetActive` derives at an *unadvanced* tick index, so a
        // tick-only key would serve an arbitrarily old client map on exactly the path a user
        // notices: switching to a tab.
        let fetches = RefCell::new(0usize);
        let mut memo = Memo::new();
        {
            let call = |memo: &mut Memo| {
                effective_root_with(
                    memo,
                    1,
                    4, // the SAME tick index both times
                    tmux_gate(),
                    |_| Some(TAB_DEV),
                    |p| {
                        *fetches.borrow_mut() += 1;
                        two_clients(p)
                    },
                    fake_rdev,
                    |_| true,
                )
            };
            call(&mut memo);
            assert_eq!(*fetches.borrow(), 1);
            // Age the entry past one tick without sleeping.
            if let Some(e) = memo.entry.as_mut() {
                e.fetched_at = Instant::now() - (TICK + std::time::Duration::from_millis(1));
            }
            call(&mut memo);
        }
        assert_eq!(
            *fetches.borrow(),
            2,
            "a stale map must be refetched even though the tick index did not advance"
        );
    }

    #[test]
    fn a_new_tick_refetches_so_a_window_switch_is_followed() {
        // R5.2 / R5.6 — the map is cached, never the resolved root, so switching tmux windows
        // moves the card on the very next tick.
        let mut memo = Memo::new();
        let pane_a = |_: &Path| parse_clients("/dev/ttys002 74877 main:0\n");
        let pane_b = |_: &Path| parse_clients("/dev/ttys002 99999 main:1\n");
        let first = effective_root_with(
            &mut memo,
            1,
            0,
            tmux_gate(),
            |_| Some(TAB_DEV),
            pane_a,
            fake_rdev,
            |_| true,
        );
        let second = effective_root_with(
            &mut memo,
            1,
            1,
            tmux_gate(),
            |_| Some(TAB_DEV),
            pane_b,
            fake_rdev,
            |_| true,
        );
        assert_eq!(first, 74877);
        assert_eq!(second, 99999, "the new tick saw the new active pane");
    }

    #[test]
    fn injected_seams_cover_every_branch_without_tmux_or_a_live_tty() {
        // R8.1 — the whole join, exercised with fake syscalls: gate -> parse -> resolve ->
        // select, including the multi-client discrimination that the tty join exists for.
        let exes = HashMap::from([(1u32, "/bin/zsh"), (2u32, "/opt/homebrew/bin/tmux")]);
        let kids = HashMap::from([(1u32, vec![2u32])]);
        let client = tmux_client_of_with(1, exe_of(&exes), kids_of(&kids)).expect("gate fires");
        assert!(is_tmux(&client.exe));

        // Two attached clients, as measured on a real machine — one belongs to this tab.
        let rows = parse_clients("/dev/ttys002 74877 main:0\n/dev/ttys004 38200 work:1\n");
        let table = HashMap::from([
            ("/dev/ttys002", 0x1000_0002u32),
            ("/dev/ttys004", 0x1000_0004u32),
        ]);
        let stat_calls = RefCell::new(0usize);
        let resolved = resolve_rdevs(&rows, |p| {
            *stat_calls.borrow_mut() += 1;
            devs(&table)(p)
        });
        assert_eq!(
            *stat_calls.borrow(),
            2,
            "one stat per client row, resolved once"
        );
        // The tab on ttys002 gets its own pane, not the other client's.
        assert_eq!(select_pane(&resolved, 0x1000_0002), Some(74877));
        assert_eq!(select_pane(&resolved, 0x1000_0004), Some(38200));
    }
}
