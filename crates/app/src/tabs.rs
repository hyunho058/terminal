//! The tab collection: `Vec<Session>` + an active index, with create/close/
//! switch/jump and resize-all (D1, R0/R1/R5).
//!
//! The index arithmetic (which tab is active after a wrap/close) is factored into
//! pure free functions so it is unit-tested without spawning a shell.

use std::path::PathBuf;

use renderer::sidebar_row::SidebarRow;
use term_core::Color;
use winit::event_loop::EventLoopProxy;

use crate::session::{Session, TabId};
use crate::terminal_app::UserEvent;

/// All open tabs plus the active index and the next id to hand out.
pub struct Tabs {
    sessions: Vec<Session>,
    active: usize,
    next_id: u64,
    fg: Color,
    bg: Color,
    proxy: EventLoopProxy<UserEvent>,
}

impl Tabs {
    /// Create the tab set with its first session (spawned by the caller).
    pub fn new(first: Session, fg: Color, bg: Color, proxy: EventLoopProxy<UserEvent>) -> Self {
        let next_id = first.id.0 + 1;
        Tabs {
            sessions: vec![first],
            active: 0,
            next_id,
            fg,
            bg,
            proxy,
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
        let session = Session::spawn(id, cols, rows, self.fg, self.bg, self.proxy.clone())?;
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
    pub fn set_default_colors_all(&mut self, fg: Color, bg: Color) {
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
