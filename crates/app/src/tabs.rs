//! The tab collection: `Vec<Session>` + an active index, with create/close/
//! switch/jump and resize-all (D1, R0/R1/R5).
//!
//! The index arithmetic (which tab is active after a wrap/close) is factored into
//! pure free functions so it is unit-tested without spawning a shell.

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

    /// Titles for the tab bar model (D6/D7) — the session title, or `Tab N`
    /// fallback. (cwd-basename titles need a `term_core::Pty::child_pid()`
    /// accessor, which is outside `tabs`' declared surface — a documented
    /// follow-up; the model already carries owned strings so the source swaps in
    /// without touching the renderer.)
    pub fn titles(&self) -> Vec<String> {
        self.sessions
            .iter()
            .enumerate()
            .map(|(i, s)| {
                if s.title().is_empty() {
                    format!("Tab {}", i + 1)
                } else {
                    s.title().to_string()
                }
            })
            .collect()
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
    fn close_keeps_a_valid_active() {
        // Close a middle/first tab (was active 1 of 3) → 2 remain, active clamps to 1.
        assert_eq!(active_after_close(1, 2), 1);
        // Close the last tab (was active 2 of 3) → 2 remain, active clamps to 1.
        assert_eq!(active_after_close(2, 2), 1);
        // Close first (was active 0 of 2) → 1 remains, active 0.
        assert_eq!(active_after_close(0, 1), 0);
    }
}
