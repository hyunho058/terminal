//! The tab collection: `Vec<Session>` + an active index, with create/close/
//! switch/jump and resize-all (D1, R0/R1/R5).
//!
//! The index arithmetic (which tab is active after a wrap/close) is factored into
//! pure free functions so it is unit-tested without spawning a shell.

use std::path::PathBuf;

use renderer::sidebar_row::SidebarRow;
use term_core::Color;

use crate::session::{Session, TabId, WakeSource};

/// All open tabs plus the active index and the next id to hand out.
pub struct Tabs {
    sessions: Vec<Session>,
    active: usize,
    next_id: u64,
    fg: Color,
    bg: Color,
    /// Mints the wake callback for each session this set spawns. Holding the
    /// source rather than an `EventLoopProxy` is what keeps `Tabs` — and so the
    /// whole tab lifecycle — constructible without an event loop.
    wake: WakeSource,
}

impl Tabs {
    /// Create the tab set with its first session (spawned by the caller).
    pub fn new(first: Session, fg: Color, bg: Color, wake: WakeSource) -> Self {
        let next_id = first.id.0 + 1;
        Tabs {
            sessions: vec![first],
            active: 0,
            next_id,
            fg,
            bg,
            wake,
        }
    }

    /// The next stable id (monotonic).
    fn mint_id(&mut self) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }
    /// Paired with `len` (clippy) — the tab set is never empty in practice
    /// (closing the last tab exits the app).
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
    pub fn active_index(&self) -> usize {
        self.active
    }
    pub fn active(&self) -> &Session {
        &self.sessions[self.active]
    }
    pub fn active_mut(&mut self) -> &mut Session {
        &mut self.sessions[self.active]
    }

    /// Look up a session by its stable id (for tagged-wake routing, D2).
    pub fn by_id_mut(&mut self, id: TabId) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    /// Read-only lookup by stable id (sticky-tab-title R3.6) — the card menu reads a
    /// background tab's user title to decide its item set and to prefill the rename field.
    pub fn by_id(&self, id: TabId) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    /// Whether `id` is the active tab (so background output skips redraw, R1.2).
    /// Bounds-safe: after the last tab closes (the app is exiting) a still-queued
    /// PTY wake must not index an empty set (C3 — clean exit).
    pub fn is_active(&self, id: TabId) -> bool {
        self.sessions.get(self.active).is_some_and(|s| s.id == id)
    }

    /// Open a new tab (Cmd+T, R1.1): spawn a session sized to the viewport, make
    /// it active.
    pub fn open(&mut self, cols: u16, rows: u16) -> crate::session::anyhow_lite::Result<TabId> {
        let id = self.mint_id();
        let session = Session::spawn(id, cols, rows, self.fg, self.bg, (self.wake)())?;
        self.sessions.push(session);
        self.active = self.sessions.len() - 1;
        Ok(id)
    }

    /// Close the active tab (Cmd+W, R1.3). Dropping the `Session` ends its PTY +
    /// reader thread. Returns `false` when the last tab was closed (the app
    /// should exit).
    pub fn close_active(&mut self) -> bool {
        if self.sessions.len() <= 1 {
            self.sessions.clear();
            return false;
        }
        self.sessions.remove(self.active);
        self.active = active_after_close(self.active, self.sessions.len());
        true
    }

    /// Close the tab with this id, whichever tab is active (sticky-tab-title R3.6/R3.7).
    /// Returns `false` when the last tab was closed (the app should exit) — the same
    /// contract as [`close_active`](Tabs::close_active), so the card menu's `Close` and
    /// Cmd+W agree on what closing the final tab means (D4).
    ///
    /// Unknown id → `true` (nothing closed, keep running): the tab may have exited
    /// between the click and the apply.
    pub fn close(&mut self, id: TabId) -> bool {
        let Some(i) = self.sessions.iter().position(|s| s.id == id) else {
            return true;
        };
        if self.sessions.len() <= 1 {
            self.sessions.clear();
            return false;
        }
        self.sessions.remove(i);
        // The ACTIVE tab keeps its identity (R3.7) — closing a background tab must not
        // switch tabs. Only its index moves, and only when the closed tab sat before it.
        // Closing the active tab itself falls back to the Cmd+W rule.
        self.active = if i == self.active {
            active_after_close(self.active, self.sessions.len())
        } else {
            active_after_close_other(i, self.active)
        };
        true
    }

    /// Switch to the next tab, wrapping (Cmd+Shift+], R3.2).
    pub fn next(&mut self) {
        self.active = wrap_next(self.active, self.sessions.len());
    }

    /// Switch to the previous tab, wrapping (Cmd+Shift+[).
    pub fn prev(&mut self) {
        self.active = wrap_prev(self.active, self.sessions.len());
    }

    /// Jump to tab `i` if it exists (Cmd+1..9).
    pub fn jump(&mut self, i: usize) {
        if i < self.sessions.len() {
            self.active = i;
        }
    }

    /// Re-fit every tab's grid + PTY to a new size — background tabs too (R5.1).
    pub fn resize_all(&mut self, cols: u16, rows: u16) {
        for s in &mut self.sessions {
            if let Err(e) = s.resize(cols, rows) {
                tracing::warn!(tab = s.id.0, "resize failed: {e}");
            }
        }
    }

    /// Re-theme every tab's default cell colors live (settings feature, R3.3).
    ///
    /// Updates the set's **own** defaults as well as the open tabs. Those are one
    /// fact held in two places — the colors each live `Terminal` paints a blank
    /// cell in, and the colors [`Tabs::open`] hands the next `Session::spawn` —
    /// and letting them drift is invisible right up until someone opens a tab:
    /// every tab on screen re-themes correctly, and then the next Cmd+T produces
    /// one wearing the *previous* theme, on a path no later re-theme revisits.
    ///
    /// Iterating `self.sessions` alone is the shape a test that also only iterates
    /// `self.sessions` cannot fail on, which is why this went unnoticed through a
    /// mutation sweep of this file.
    pub fn set_default_colors_all(&mut self, fg: Color, bg: Color) {
        self.fg = fg;
        self.bg = bg;
        for s in &mut self.sessions {
            s.terminal.set_default_colors(fg, bg);
        }
    }

    /// Rows for the sidebar model (D2) — each carries the tab's stable `TabId` (as a
    /// plain `u64`, so async metadata joins by id, D10) plus its title: the cwd basename
    /// **as of this call**, or a `Tab N` fallback.
    ///
    /// That title is the card's *pre-publish fallback*, not its live folder name
    /// (card-live-cwd D2). This runs on tab actions, so a `cd` never reaches it; the card
    /// resolves the live cwd off the metadata pull instead (`sidebar_model::fill`) and falls
    /// back to this only while a tab is still unpolled. Reading it as "the live cwd
    /// basename" — as this doc did — is what left cards naming the directory they opened in.
    ///
    /// The old SD4 justification for this cadence ("its per-tab `proc_pidinfo` is not on the
    /// render path") is **spent**: since `tmux-pane-cwd`, `Session::cwd` returns the
    /// published value through `published_or` and costs no syscall at all when one exists.
    /// The cadence is now merely where the rows happen to be assembled.
    ///
    /// The field/method keep the name `titles` although they now carry rows — a documented
    /// cost of the frozen `terminal_app` struct literal (D2).
    pub fn titles(&self, meta: &crate::tabmeta::TabMetaSnapshot) -> Vec<SidebarRow> {
        // Push the live roster + active tab to the metadata worker (the tab-metadata
        // seam's natural site — called on tab actions, not per frame), so it derives each
        // tab's git branch + agent state off the UI thread. Idempotent; a no-op when the
        // worker failed to spawn.
        if let Some(handle) = crate::tabmeta::global() {
            let roster: Vec<crate::tabmeta::TabPid> = self
                .sessions
                .iter()
                .filter_map(|s| {
                    s.pty
                        .child_pid()
                        .map(|pid| crate::tabmeta::TabPid { id: s.id, pid })
                })
                .collect();
            handle.set_roster(&roster);
            handle.set_active(self.sessions.get(self.active).map(|s| s.id));
        }
        self.sessions
            .iter()
            .enumerate()
            .map(|(i, s)| {
                // The folder name AT ASSEMBLY TIME, through the one shared derivation (D7).
                // `Tab {n}` is this caller's own fallback — the helper deliberately has none.
                let title = crate::sidebar_model::folder_name(s.cwd(meta.get(s.id)).as_deref())
                    .unwrap_or_else(|| format!("Tab {}", i + 1));
                // The two title sources travel separately (sticky-tab-title D9/R1.1): the
                // user's sticky title, which outranks everything, and the last title a
                // program set, which ranks below the AI-task label and is hidden entirely
                // while a user title exists. They are NOT collapsed here — the chain is
                // resolved in `sidebar_model::fill`, the only place the AI-task label
                // between them is fresh (R1.2).
                SidebarRow {
                    tab_id: s.id.0,
                    title,
                    label: s.user_title().map(str::to_string),
                    program: Some(s.program_title())
                        .filter(|t| !t.is_empty())
                        .map(str::to_string),
                }
            })
            .collect()
    }

    /// The active tab's live working directory (ai-monitor log resolution).
    pub fn active_cwd(&self, meta: &crate::tabmeta::TabMetaSnapshot) -> Option<PathBuf> {
        let a = self.active();
        a.cwd(meta.get(a.id))
    }

    /// The active tab's shell pid (ai-monitor tab binding).
    pub fn active_shell_pid(&self) -> Option<u32> {
        self.active().shell_pid()
    }

    /// The active tab's **Claude** process pid, as last derived by the metadata worker.
    ///
    /// Straight off the snapshot with no fallback, unlike [`active_cwd`](Self::active_cwd):
    /// there is no second source to fall back *to*, and an absent entry (a tab not yet polled,
    /// or one with no agent) must read as "no information" rather than as a value. The
    /// ai-monitor panel is the only consumer, and it treats `None` that way by construction.
    pub fn active_agent_pid(&self, meta: &crate::tabmeta::TabMetaSnapshot) -> Option<u32> {
        meta.get(self.active().id).and_then(|m| m.agent_pid)
    }
}

/// The active index after closing a **different** tab (sticky-tab-title R3.7): the active
/// session keeps its identity, so its index shifts down by one only when the closed tab
/// sat before it. Pure, so the off-by-one that would silently switch tabs is unit-tested.
fn active_after_close_other(closed: usize, active: usize) -> usize {
    if closed < active {
        active - 1
    } else {
        active
    }
}

/// Active index after wrapping forward.
fn wrap_next(active: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (active + 1) % len
    }
}

/// Active index after wrapping backward.
fn wrap_prev(active: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (active + len - 1) % len
    }
}

/// Active index after removing the (previously active) tab: stay at the same
/// slot — now the next tab — or clamp to the last tab if we closed the last one.
fn active_after_close(closed_active: usize, new_len: usize) -> usize {
    if new_len == 0 {
        0
    } else {
        closed_active.min(new_len - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------
    // `Tabs` itself.
    //
    // The tests below this block cover the pure index helpers, which is what this
    // file's header promised: "the index arithmetic is factored into pure free
    // functions so it is unit-tested without spawning a shell." True — and it left
    // `Tabs` with no test at all, because building one used to need an
    // `EventLoopProxy`. A `cargo mutants` run over this file left 41 survivors
    // (a count, not a ratio — see CLAUDE.md; a file's mutant total moves with
    // every edit to it, which is how the same run for `session.rs` came to be
    // written down three different ways): `next`,
    // `prev`, `jump` and `resize_all` could each be replaced with a **no-op** —
    // tab switching does nothing, background tabs never resize — and the suite
    // stayed green, because the helpers were tested and nothing checked that
    // anything called them.
    //
    // Now reachable: `Tabs::new` takes a `WakeSource`, so the whole set is
    // constructible headlessly. Each tab is a real zsh, which is what these are
    // buying — the lifecycle, not the arithmetic.
    // ---------------------------------------------------------------------

    /// A wake that goes nowhere. These tests drive `Tabs` directly rather than
    /// through the event loop, so nothing is listening.
    fn silent_wake() -> crate::session::WakeSource {
        std::rc::Rc::new(|| Box::new(|_| {}))
    }

    const FG: Color = Color(0xE0E0E0);
    const BG: Color = Color(0x101010);

    /// A tab set with `n` real sessions, active on the last one (as `open` leaves it).
    /// `None` when shell tests are opted out of (`SKIP_SHELL_TESTS=1`). Each tab
    /// here is a real zsh, which is what these tests are buying — the lifecycle,
    /// not the arithmetic — so there is nothing to run without one.
    fn tabs_with(n: usize) -> Option<Tabs> {
        if !crate::session::shell_tests_enabled() {
            return None;
        }
        let wake = silent_wake();
        let first = Session::spawn(TabId(0), 80, 24, FG, BG, (wake)()).expect("spawn first");
        let mut tabs = Tabs::new(first, FG, BG, wake);
        for _ in 1..n {
            tabs.open(80, 24).expect("open");
        }
        Some(tabs)
    }

    fn ids(tabs: &Tabs) -> Vec<TabId> {
        tabs.sessions.iter().map(|s| s.id).collect()
    }

    #[test]
    fn opening_a_tab_appends_it_and_makes_it_active() {
        let Some(mut tabs) = tabs_with(1) else {
            return;
        };
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active_index(), 0);

        let id = tabs.open(80, 24).expect("open");
        assert_eq!(tabs.len(), 2, "the new tab must join the set");
        assert_eq!(
            tabs.active_index(),
            1,
            "a newly opened tab is the one you are looking at (R1.1)"
        );
        assert_eq!(tabs.active().id, id, "and it is the one that was opened");
    }

    #[test]
    fn tab_ids_are_monotonic_and_never_reused() {
        // D2 — the id is what a queued PTY wake, the sidebar and the panel all key
        // on. A reused id routes one tab's output into another's grid.
        let Some(mut tabs) = tabs_with(3) else {
            return;
        };
        let before = ids(&tabs);
        assert_eq!(before.len(), 3);
        assert!(
            before.windows(2).all(|w| w[0].0 < w[1].0),
            "ids must increase: {before:?}"
        );

        // Close one and open another: the new id must clear every id ever handed
        // out, not fill the hole.
        tabs.close_active();
        let fresh = tabs.open(80, 24).expect("open");
        assert!(
            before.iter().all(|old| old.0 < fresh.0),
            "id {} reuses or precedes an earlier id in {before:?}",
            fresh.0
        );
    }

    #[test]
    fn lookup_by_id_finds_that_tab_and_only_that_tab() {
        let Some(mut tabs) = tabs_with(3) else {
            return;
        };
        let all = ids(&tabs);
        for want in &all {
            assert_eq!(tabs.by_id(*want).map(|s| s.id), Some(*want));
            assert_eq!(tabs.by_id_mut(*want).map(|s| s.id), Some(*want));
        }
        let absent = TabId(9_999);
        assert!(
            tabs.by_id(absent).is_none(),
            "an unknown id resolves to no tab"
        );
        assert!(tabs.by_id_mut(absent).is_none());
    }

    #[test]
    fn is_active_names_the_active_tab_and_rejects_the_rest() {
        // R1.2 — background output skips the redraw on the strength of this, so a
        // constant answer either repaints for every background tab or never repaints.
        let Some(tabs) = tabs_with(3) else {
            return;
        };
        let all = ids(&tabs);
        let active = tabs.active().id;
        assert!(tabs.is_active(active));
        for other in all.iter().filter(|i| **i != active) {
            assert!(!tabs.is_active(*other), "{other:?} is not the active tab");
        }
        assert!(!tabs.is_active(TabId(9_999)), "an unknown id is not active");
    }

    #[test]
    fn next_and_prev_actually_move_the_active_tab() {
        // The helpers below are already tested; this asserts `Tabs` *calls* them.
        let Some(mut tabs) = tabs_with(3) else {
            return;
        };
        assert_eq!(tabs.active_index(), 2);
        tabs.next();
        assert_eq!(tabs.active_index(), 0, "next wraps off the end (R3.2)");
        tabs.next();
        assert_eq!(tabs.active_index(), 1);
        tabs.prev();
        assert_eq!(tabs.active_index(), 0);
        tabs.prev();
        assert_eq!(tabs.active_index(), 2, "prev wraps off the front");
    }

    #[test]
    fn jump_reaches_existing_tabs_and_ignores_the_rest() {
        let Some(mut tabs) = tabs_with(3) else {
            return;
        };
        tabs.jump(0);
        assert_eq!(tabs.active_index(), 0);
        tabs.jump(2);
        assert_eq!(tabs.active_index(), 2);
        // Out of range is a no-op, not a panic and not an out-of-bounds active
        // index — `active()` indexes the vec directly, so a bad jump is a crash
        // one keystroke later.
        tabs.jump(3);
        assert_eq!(tabs.active_index(), 2, "Cmd+4 with three tabs does nothing");
        tabs.jump(usize::MAX);
        assert_eq!(tabs.active_index(), 2);
        let _ = tabs.active(); // must not panic
    }

    #[test]
    fn len_and_is_empty_agree_about_the_set() {
        // `is_empty` has no production caller yet (it exists to pair with `len` for
        // clippy), which is exactly why nothing was asserting it: a constant `true`
        // or `false` here is invisible until the first consumer trusts it.
        let Some(mut tabs) = tabs_with(2) else {
            return;
        };
        assert_eq!(tabs.len(), 2);
        assert!(!tabs.is_empty());
        assert!(tabs.close_active());
        assert!(!tabs.is_empty(), "one tab is still open");
        assert!(!tabs.close_active(), "that was the last one");
        assert_eq!(tabs.len(), 0);
        assert!(tabs.is_empty(), "the set is empty once the last tab closes");
    }

    #[test]
    fn closing_the_active_tab_reports_whether_any_remain() {
        let Some(mut tabs) = tabs_with(2) else {
            return;
        };
        assert!(tabs.close_active(), "one tab still open, so keep running");
        assert_eq!(tabs.len(), 1);
        assert!(
            !tabs.close_active(),
            "closing the last tab tells the app to exit (R1.3)"
        );
        assert_eq!(tabs.len(), 0);
    }

    #[test]
    fn closing_a_background_tab_leaves_the_active_one_where_it_was() {
        // R3.7 — the active tab keeps its *identity*, only its index shifts, and
        // only when the closed tab sat before it. Getting this wrong silently
        // switches the tab under the user.
        let Some(mut tabs) = tabs_with(3) else {
            return;
        };
        let all = ids(&tabs);
        tabs.jump(1);
        let active = tabs.active().id;

        assert!(tabs.close(all[0]), "closing a background tab keeps running");
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs.active().id, active, "the active tab must not change");
        assert_eq!(tabs.active_index(), 0, "but its index shifts down by one");

        // Closing a tab *after* the active one does not move it at all.
        let Some(mut tabs) = tabs_with(3) else {
            return;
        };
        let all = ids(&tabs);
        tabs.jump(0);
        assert!(tabs.close(all[2]));
        assert_eq!(tabs.active_index(), 0);
        assert_eq!(tabs.active().id, all[0]);
    }

    #[test]
    fn closing_an_unknown_id_changes_nothing_and_keeps_running() {
        // The tab may have exited between the click and the apply (R3.6).
        let Some(mut tabs) = tabs_with(2) else {
            return;
        };
        let before = ids(&tabs);
        let active = tabs.active().id;
        assert!(tabs.close(TabId(9_999)));
        assert_eq!(ids(&tabs), before);
        assert_eq!(tabs.active().id, active);
    }

    #[test]
    fn closing_the_last_tab_by_id_also_reports_the_exit() {
        let Some(mut tabs) = tabs_with(1) else {
            return;
        };
        let only = tabs.active().id;
        assert!(!tabs.close(only), "the same contract as close_active (D4)");
        assert_eq!(tabs.len(), 0);
    }

    #[test]
    fn resize_all_refits_background_tabs_too() {
        // R5.1 — the whole point of `resize_all` over resizing the active tab: a
        // background tab that kept the old size reflows wrongly the moment you
        // switch to it.
        let Some(mut tabs) = tabs_with(3) else {
            return;
        };
        for s in &tabs.sessions {
            assert_eq!(s.terminal.dimensions(), (80, 24));
        }
        tabs.resize_all(40, 10);
        for s in &tabs.sessions {
            assert_eq!(
                s.terminal.dimensions(),
                (40, 10),
                "tab {} kept the old size",
                s.id.0
            );
        }
    }

    #[test]
    fn set_default_colors_all_rethemes_background_tabs_too() {
        // R3.3 (settings) — a live theme change must reach every tab, not just the
        // one on screen.
        let Some(mut tabs) = tabs_with(3) else {
            return;
        };
        let (fg2, bg2) = (Color(0x00FF00), Color(0x123456));
        tabs.set_default_colors_all(fg2, bg2);
        for s in &tabs.sessions {
            // The bottom-right cell: past anything a prompt has written, so it is
            // still a blank cell wearing the defaults.
            let snap = s.terminal.snapshot();
            let cell = snap
                .cell(snap.cols - 1, snap.rows - 1)
                .expect("corner cell");
            assert_eq!(cell.bg, bg2, "tab {} kept the old background", s.id.0);
            assert_eq!(cell.fg, fg2, "tab {} kept the old foreground", s.id.0);
        }
    }

    #[test]
    fn a_tab_opened_after_a_re_theme_wears_the_new_theme() {
        // The half the test above structurally cannot see. It iterates the tabs
        // that already exist, so a setter that re-themes them and forgets the set's
        // own `fg`/`bg` passes it — and `Tabs::open` spawns from exactly those, so
        // the drift surfaces only on the *next* Cmd+T. Reported as a real defect:
        // edit `theme.background`, watch every open tab re-theme, press Cmd+T, and
        // the new tab comes up on the old background.
        let Some(mut tabs) = tabs_with(1) else {
            return;
        };
        let (fg2, bg2) = (Color(0x00FF00), Color(0x123456));
        tabs.set_default_colors_all(fg2, bg2);

        tabs.open(80, 24).expect("open a tab after the re-theme");
        let snap = tabs.active().terminal.snapshot();
        let cell = snap
            .cell(snap.cols - 1, snap.rows - 1)
            .expect("corner cell");
        assert_eq!(
            cell.bg, bg2,
            "a tab opened after the re-theme was spawned with the old background"
        );
        assert_eq!(
            cell.fg, fg2,
            "a tab opened after the re-theme was spawned with the old foreground"
        );
    }

    #[test]
    fn the_active_tabs_shell_pid_is_that_tabs_own_live_shell() {
        // The root the ai-monitor walks. A constant here binds the panel to the
        // wrong process without failing anything.
        let Some(mut tabs) = tabs_with(2) else {
            return;
        };
        let first = tabs.active().shell_pid().expect("shell");
        assert!(first > 1, "pid {first} is not a real process");
        assert_eq!(tabs.active_shell_pid(), Some(first));

        // And it follows the active tab rather than reporting one tab forever.
        tabs.jump(0);
        let second = tabs.active_shell_pid().expect("shell");
        assert!(second > 1);
        assert_ne!(second, first, "each tab runs its own shell");
    }

    #[test]
    fn the_active_tabs_cwd_and_agent_pid_come_from_that_tabs_published_metadata() {
        // Both read the snapshot keyed by the *active* tab's id. Against an empty
        // snapshot every honest implementation answers `None`, so the fixture has
        // to carry entries for the answer to mean anything.
        let Some(mut tabs) = tabs_with(2) else {
            return;
        };
        let all = ids(&tabs);
        let meta_for = |cwd: &str, agent_pid: u32| crate::tabmeta::TabMeta {
            cwd: Some(PathBuf::from(cwd)),
            agent_pid: Some(agent_pid),
            ..Default::default()
        };
        let snap = crate::tabmeta::TabMetaSnapshot::from_entries(vec![
            (all[0], meta_for("/proj/first", 111)),
            (all[1], meta_for("/proj/second", 222)),
        ]);

        assert_eq!(tabs.active_index(), 1);
        assert_eq!(tabs.active_cwd(&snap), Some(PathBuf::from("/proj/second")));
        assert_eq!(tabs.active_agent_pid(&snap), Some(222));

        tabs.jump(0);
        assert_eq!(tabs.active_cwd(&snap), Some(PathBuf::from("/proj/first")));
        assert_eq!(
            tabs.active_agent_pid(&snap),
            Some(111),
            "the pid must follow the active tab, not stay on the first one"
        );
    }

    #[test]
    fn a_sidebar_row_carries_the_program_title_only_when_there_is_one() {
        // D9/R1.1 — the program channel travels separately from the user's, and an
        // empty program title is *absent*, not an empty string the sidebar would
        // render as a blank line.
        let Some(mut tabs) = tabs_with(2) else {
            return;
        };
        let all = ids(&tabs);
        let snap = crate::tabmeta::TabMetaSnapshot::default();

        let rows = tabs.titles(&snap);
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter().all(|r| r.program.is_none()),
            "no program has set a title yet"
        );

        tabs.by_id_mut(all[0])
            .expect("tab")
            .set_user_title(Some("named".into()));
        let rows = tabs.titles(&snap);
        let row = rows.iter().find(|r| r.tab_id == all[0].0).expect("row");
        assert_eq!(row.label.as_deref(), Some("named"));
        assert!(
            row.program.is_none(),
            "a user title is not a program title (D9)"
        );
    }

    #[test]
    fn next_and_prev_wrap() {
        assert_eq!(wrap_next(0, 3), 1);
        assert_eq!(wrap_next(2, 3), 0); // wraps
        assert_eq!(wrap_prev(0, 3), 2); // wraps
        assert_eq!(wrap_prev(2, 3), 1);
        // Degenerate sizes never panic.
        assert_eq!(wrap_next(0, 1), 0);
        assert_eq!(wrap_prev(0, 1), 0);
        assert_eq!(wrap_next(0, 0), 0);
    }

    #[test]
    fn closing_another_tab_keeps_the_active_tab_pointing_at_the_same_session() {
        // sticky-tab-title R3.7 — the card menu can close a BACKGROUND tab. The active
        // session must not change underneath the user; only its index moves, and only
        // when the closed tab sat before it.
        // [0,1,2] active=2, close 0 → the active session is now at index 1.
        assert_eq!(active_after_close_other(0, 2), 1);
        // close 1 (still before) → index 1.
        assert_eq!(active_after_close_other(1, 2), 1);
        // close 3 (after the active) → the active index is untouched.
        assert_eq!(active_after_close_other(3, 2), 2);
        // Closing the tab right after the active one changes nothing.
        assert_eq!(active_after_close_other(1, 0), 0);
        // The boundary, `closed == active`. `Tabs::close` routes that case to
        // `active_after_close` instead, so no caller reaches it today — but this is
        // a pure helper with a stated contract ("only when the closed tab sat
        // *before* it"), and the strict `<` is the whole contract. Left untested,
        // relaxing it to `<=` is invisible: found by `cargo mutants`, which flagged
        // exactly this flip while the neighbouring cases above all held.
        assert_eq!(active_after_close_other(2, 2), 2);
    }

    #[test]
    fn close_keeps_a_valid_active() {
        // Close a middle/first tab (was active 1 of 3) → 2 remain, active clamps to 1.
        assert_eq!(active_after_close(1, 2), 1);
        // Close the last tab (was active 2 of 3) → 2 remain, active clamps to 1.
        assert_eq!(active_after_close(2, 2), 1);
        // Close first (was active 0 of 2) → 1 remains, active 0.
        assert_eq!(active_after_close(0, 1), 0);
    }
}
