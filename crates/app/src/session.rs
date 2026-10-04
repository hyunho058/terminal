//! One tab's session: a `Pty` + `Terminal` + a reader thread (D1).
//!
//! Each session is independent — its own zsh, its own grid. The reader thread
//! tags every wake with the session's stable [`TabId`] (D2) so the app advances
//! the right grid and only redraws when the *active* tab produced output.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::Instant;

use term_core::{Color, CommandEnd, Pty, Terminal};

use crate::tabmeta::TabMeta;

/// A stable, monotonic tab identifier (never reused; survives index shifts on
/// close, unlike a `Vec` position).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct TabId(pub u64);

/// How a session's reader thread wakes the UI when its PTY produces output.
///
/// A plain callback rather than the `EventLoopProxy` it wraps, for exactly the
/// reason the renderer takes a raw window handle instead of a `winit::Window`
/// (C4): it keeps the whole input→shell→grid path constructible without an event
/// loop. That is not a stylistic preference here — on macOS a `winit::EventLoop`
/// can only be built on the main thread, and the test harness runs every test on
/// a spawned one, so a `Session` that demanded a proxy could never be exercised
/// end to end by any test at all. It was not: swallowing every byte in
/// [`Session::write`] (a terminal that types nothing) left the workspace suite at
/// 1460 passed / 0 failed.
///
/// `App::start` passes a closure over the real `EventLoopProxy` — it is the one
/// place in the app that makes one. A test passes a recorder.
pub type Wake = Box<dyn Fn(TabId) + Send + 'static>;

/// Mints a fresh [`Wake`] per session.
///
/// [`Tabs`](crate::tabs::Tabs) spawns sessions on demand and so needs to produce
/// wakers rather than hold one. `Rc`, not `Arc`: the tab set lives on the UI
/// thread and never crosses one — only the `Wake` it hands out does.
pub type WakeSource = std::rc::Rc<dyn Fn() -> Wake>;

/// One tab's terminal session.
pub struct Session {
    pub id: TabId,
    pub terminal: Terminal,
    pub pty: Pty,
    pub writer: Box<dyn Write + Send>,
    pub rx: Receiver<Vec<u8>>,
    /// The title the **user** set on this tab (sticky-tab-title D9), or `None` when the
    /// tab has never been named / was reset to auto. Nothing but an explicit user action
    /// writes this — never the PTY — which is the whole point: OSC 0/2 is a channel any
    /// program can write, so it cannot be the user's channel (D1/D3).
    ///
    /// Highest precedence on the card: **user > AI-task > program-OSC > cwd** (D5/D12).
    user_title: Option<String>,
    /// The most recent OSC 0/2 title any program on this PTY set — zsh's precmd, vim,
    /// ssh, tmux (sticky-tab-title D3). Recorded even while [`user_title`] hides it, so
    /// the channel keeps working for a tab that has neither a user title nor an AI task
    /// (D3); `""` = none seen.
    ///
    /// [`user_title`]: Session::user_title
    program_title: String,
    /// Set when a change could alter what the card *shows*, cleared by
    /// [`take_title_dirty`](Session::take_title_dirty). Lets the event loop resync the
    /// tab model so a new label shows immediately, not on the next tab switch — even for
    /// a background tab. A program title that lands under a user title sets nothing: it
    /// is recorded but invisible, so redrawing for it would be pure waste (R1.3).
    title_dirty: bool,
    /// Most-recent OSC 133 command-end event observed on this session's PTY,
    /// keyed implicitly to `id`. Pulled from the terminal in [`drain`] and
    /// consumed by the waiting model (T3) via [`take_command_end`].
    ///
    /// [`drain`]: Session::drain
    /// [`take_command_end`]: Session::take_command_end
    command_end: Option<CommandEnd>,
    /// The last command end seen on this PTY, **not** consumed by anyone — what the
    /// sidebar card reads for `Exited N` (sidebar-weather-cards D8). Separate from
    /// [`command_end`](Session::command_end) on purpose: that one is the waiting band's
    /// one-shot, and a take that emptied the card's copy would blank its status.
    last_end: Option<CommandEnd>,
    /// Whether the shell is running a foreground command — mirrored from the terminal's
    /// OSC 133 state on every [`drain`](Session::drain) (D8).
    command_running: bool,
    /// Set when [`command_running`](Session::command_running) or the last exit **code**
    /// changed — the two inputs of the card's status — and cleared by
    /// [`take_command_state_dirty`](Session::take_command_state_dirty), so the event loop
    /// resyncs the row exactly as it does for a visible title change (D8). A repeat of the
    /// same failing code changes nothing on the card and sets nothing.
    command_dirty: bool,
    /// Whether this tab's live screen shows a dialog that blocks on the user — a **level**,
    /// set only by the event loop's scan (agent-card-status D7) and read by both the
    /// waiting band and the card, so the two cannot disagree (D4). Held while the view is
    /// scrolled back (the scrolled view is history, not the live screen) and false while
    /// the tab has no agent.
    blocking_dialog: bool,
    _reader: JoinHandle<()>,
}

impl Session {
    /// Spawn a zsh session sized to `cols`×`rows`, wiring its reader thread to
    /// call `wake` with this session's id on every chunk (D2).
    pub fn spawn(
        id: TabId,
        cols: u16,
        rows: u16,
        fg: Color,
        bg: Color,
        wake: Wake,
    ) -> anyhow_lite::Result<Session> {
        let terminal = Terminal::new(cols, rows, fg, bg);
        let pty = Pty::spawn(cols, rows).map_err(err)?;
        let writer = pty.writer().map_err(err)?;
        let mut reader = pty.reader().map_err(err)?;

        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let reader_thread = std::thread::Builder::new()
            .name(format!("pty-reader-{}", id.0))
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break, // EOF / shell exited
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                            // Tagged wake: this session's id (D2).
                            wake(id);
                        }
                    }
                }
            })
            .map_err(err)?;

        Ok(Session {
            id,
            terminal,
            pty,
            writer,
            rx,
            user_title: None,
            program_title: String::new(),
            title_dirty: false,
            command_end: None,
            last_end: None,
            command_running: false,
            command_dirty: false,
            blocking_dialog: false,
            _reader: reader_thread,
        })
    }

    /// Drain any pending PTY output into the grid. Returns true if anything was
    /// advanced (so the caller can decide whether to redraw).
    pub fn drain(&mut self) -> bool {
        let mut got = false;
        while let Ok(bytes) = self.rx.try_recv() {
            self.terminal.advance(&bytes);
            got = true;
        }
        // Once per drain, never per chunk (selection-scroll-anchor D5): a frame larger than
        // one 1024-byte PTY read arrives in pieces, and a check between them would clear a
        // selection over text the frame was only half-way through repainting. The terminal
        // owns the rule; the clock is read here because `term-core` keeps none.
        self.terminal.settle_selection(Instant::now());
        // After advancing, pull any OSC 133 command-end the shell emitted on this
        // session's PTY. Keeps a still-unconsumed event if none is newer, so a
        // slow waiting-model poll (T3) can't miss one (spec R4.6).
        if let Some(ev) = self.terminal.take_command_end() {
            self.command_end = Some(ev);
            // The card's copy (sidebar-weather-cards D8): kept, never taken. Dirty only
            // when the code moves — that, not the event, is what the card shows.
            if self.last_end.map(|e| e.exit_code) != Some(ev.exit_code) {
                self.command_dirty = true;
            }
            self.last_end = Some(ev);
        }
        // Mirror the terminal's running state as the stream left it (D8): a `C…D` in one
        // chunk reads as finished, a `D…C` as running, because only the final value is read.
        let running = self.terminal.command_running();
        if running != self.command_running {
            self.command_running = running;
            self.command_dirty = true;
        }
        // Pull any OSC 0/2 title a *program* set on this PTY — zsh's precmd, vim, ssh
        // (sticky-tab-title D3). This is never the user's channel: it lands in
        // `program_title` only, and can neither overwrite a user title nor masquerade as
        // one. The latest wins; `""` means the program cleared its title.
        //
        // The dirty flag is gated on visibility (R1.3): while a user title is set the
        // program title is recorded but not shown, so a change to it alters nothing on
        // screen and must not cost a resync + redraw.
        if let Some(title) = self.terminal.take_osc_title() {
            if title != self.program_title {
                self.program_title = title;
                self.title_dirty = program_title_is_visible(self.user_title.as_deref());
            }
        }
        got
    }

    /// Whether an OSC 0/2 title changed the manual label since the last check, clearing the
    /// flag (tab-identity). The event loop resyncs the tab model + redraws when this is true.
    pub fn take_title_dirty(&mut self) -> bool {
        std::mem::take(&mut self.title_dirty)
    }

    /// Take the most-recent OSC 133 command-end event seen on this session's PTY
    /// since the last poll, clearing it. The app keys it to this session's
    /// [`TabId`] and feeds the waiting model (T3); `None` if none pending.
    pub fn take_command_end(&mut self) -> Option<CommandEnd> {
        self.command_end.take()
    }

    /// The last command end seen on this PTY, if any — **not** consumed by reading, unlike
    /// [`take_command_end`](Session::take_command_end) (sidebar-weather-cards D8).
    pub fn last_end(&self) -> Option<CommandEnd> {
        self.last_end
    }

    /// Whether the shell is running a foreground command (OSC 133 `C` with no end since).
    pub fn command_running(&self) -> bool {
        self.command_running
    }

    /// Whether the card's status inputs changed since the last check, clearing the flag.
    /// The event loop resyncs the tab model when this is true, beside
    /// [`take_title_dirty`](Session::take_title_dirty).
    pub fn take_command_state_dirty(&mut self) -> bool {
        std::mem::take(&mut self.command_dirty)
    }

    /// Whether this tab's live screen last showed a blocking dialog (agent-card-status D7).
    pub fn blocking_dialog(&self) -> bool {
        self.blocking_dialog
    }

    /// Store the dialog level a scan produced, answering whether it **changed** — the one
    /// thing that obliges the event loop to resync the card's row (D7). A repeat of the
    /// same level, which every PTY chunk of a busy agent produces, costs nothing.
    pub fn set_blocking_dialog(&mut self, level: bool) -> bool {
        let changed = self.blocking_dialog != level;
        self.blocking_dialog = level;
        changed
    }

    /// Re-read this tab's dialog level on a scan event (agent-card-status D7) and store it:
    /// `(level, changed)`. The rule is [`next_dialog_level`]; this feeds it the tab's own
    /// facts and, only when the rule asks, its live screen.
    ///
    /// Deliberately a **level**, not an event: it is re-evaluated on every trigger, so
    /// answering the prompt makes the dialog disappear and the tab leaves `Waiting` through
    /// the existing falling edge. Nothing to latch, expire, or replay.
    ///
    /// Uses `viewport_tail_text` rather than a full snapshot: this runs per PTY chunk, and
    /// cloning the grid at that rate would be the G1 idle-CPU debt all over again.
    pub fn rescan_dialog(&mut self, has_agent: bool) -> (bool, bool) {
        let terminal = &self.terminal;
        let level = next_dialog_level(
            has_agent,
            terminal.view_offset() == 0,
            self.blocking_dialog,
            || {
                crate::prompt::shows_blocking_prompt(
                    &terminal.viewport_tail_text(crate::prompt::TAIL_ROWS),
                )
            },
        );
        (level, self.set_blocking_dialog(level))
    }

    /// Write bytes to this session's shell.
    pub fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    /// Re-fit the grid and PTY to a new size (R5).
    pub fn resize(&mut self, cols: u16, rows: u16) -> anyhow_lite::Result<()> {
        self.terminal.resize(cols, rows);
        self.pty.resize(cols, rows).map_err(err)?;
        Ok(())
    }

    /// This tab's working directory — the **active tmux pane's** when the tab runs a tmux
    /// client, else the shell's own. Drives cwd-basename tab titles (R6.1) and the rename
    /// prefill; the ai-monitor resolves its own on open (tmux-pane-cwd D12).
    ///
    /// Reads `meta.cwd`, which the tabmeta worker publishes (tmux-pane-cwd D1/D2). That is
    /// what makes this **free on the hot paths**: it is called twice per keystroke
    /// (`terminal_app.rs`) and once per tab per sidebar resync, and a published read costs no
    /// syscall where the old direct `proc_pidinfo` cost one every time.
    ///
    /// Falls back to that direct lookup when nothing is published — a tab the worker has not
    /// polled yet, or one where no cwd resolved — so this is **never emptier than the old
    /// implementation** (R2.3), and still returns `None` for a dead shell once the worker has
    /// observed it (R8.2). The fallback returns the *tab's own* cwd, which under tmux is the
    /// client's rather than the pane's; that window is why the overlay does not rely on this
    /// (D12) and why a briefly-wrong title is an accepted residue.
    pub fn cwd(&self, meta: Option<&TabMeta>) -> Option<PathBuf> {
        published_or(meta, || cwd_of_pid(self.pty.child_pid()?))
    }

    /// The pid of this tab's shell. The root of the process tree the ai-monitor
    /// overlay searches to find *this tab's* Claude session — the only thing that
    /// distinguishes it from a session another application is running in the same
    /// project directory.
    pub fn shell_pid(&self) -> Option<u32> {
        self.pty.child_pid()
    }

    /// The title the user set on this tab, or `None` (sticky-tab-title R1.1). Highest
    /// precedence on the card — it outranks the live AI-task label, which is what makes
    /// a named tab *stay* named (D3/D5).
    pub fn user_title(&self) -> Option<&str> {
        self.user_title.as_deref()
    }

    /// Set or clear the user title (R2.2/R2.3). `Some(name)` names the tab; `None` resets
    /// it to the live auto label (AI-task, else program title, else cwd). Whitespace-only
    /// input is normalized to `None` here so every caller — chord, menu, future ones —
    /// gets the same "empty means reset" rule without repeating it (R2.3).
    ///
    /// Flags the tab dirty on a real change so the event loop resyncs + redraws at once,
    /// including for a background tab. Returns whether anything changed.
    pub fn set_user_title(&mut self, title: Option<String>) -> bool {
        let next = normalize_user_title(title);
        if next == self.user_title {
            return false;
        }
        self.user_title = next;
        self.title_dirty = true;
        true
    }

    /// The most recent OSC 0/2 title a program set on this PTY, or `""` (R1.1). Ranks
    /// *below* the AI-task label (D12) and is hidden entirely while a user title is set
    /// (D3) — it is the label only for a tab that is neither named nor running an agent.
    pub fn program_title(&self) -> &str {
        &self.program_title
    }
}

/// Minimal error plumbing (mirrors `terminal_app`), so `?` works across the
/// heterogeneous PTY/thread errors without pulling in full `anyhow`.
pub mod anyhow_lite {
    pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
}

fn err<E: Into<Box<dyn std::error::Error + Send + Sync>>>(
    e: E,
) -> Box<dyn std::error::Error + Send + Sync> {
    e.into()
}

#[cfg(test)]
thread_local! {
    /// Test-only count of real `proc_pidinfo` calls made through [`cwd_of_pid`]. Exists so
    /// R6.1's "the hot path costs zero syscalls" is a *measured delta* rather than an argument
    /// about whether a closure ran. Not compiled into the shipping binary.
    ///
    /// **Thread-local, not a global.** libtest runs tests concurrently and
    /// `cwd_of_pid_resolves_current_process` calls `cwd_of_pid` directly in this same binary; a
    /// process-wide counter would let it land inside another test's measurement window and fail
    /// it spuriously. Both the calls and the reads happen on the measuring test's own thread,
    /// so a thread-local is exactly the scope wanted — and needs no serialization.
    pub(crate) static CWD_SYSCALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The current working directory of process `pid` via macOS `proc_pidinfo`
/// (`PROC_PIDVNODEPATHINFO`). Read-only. `None` on any failure (dead pid,
/// permission, empty path) — callers degrade gracefully.
///
/// `pub(crate)` since tmux-pane-cwd: `overlay.rs` calls it to turn the resolved tmux pane pid
/// into a path (D12). In-surface, so it needs no deviation record — but it is a real new
/// cross-module coupling and is named here so it is not a silent one.
pub(crate) fn cwd_of_pid(pid: u32) -> Option<PathBuf> {
    #[cfg(test)]
    CWD_SYSCALLS.with(|c| c.set(c.get() + 1));

    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    let n = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    // `!=`, not `<`: `proc_pidinfo` answers exactly `size` on success and 0 or -1
    // on failure, so the two forms are equivalent — but `<` also admits a `>`
    // mutant that no test can kill, and an equivalent mutant in the report costs
    // more than the character it saves. Same shape as `tabmeta::proc`'s other
    // three `proc_pidinfo` guards.
    if n != size {
        return None; // error or short read
    }
    // pvi_cdir.vip_path is a fixed 1024-byte, NUL-terminated C path.
    let raw = &info.pvi_cdir.vip_path;
    let bytes = unsafe {
        std::slice::from_raw_parts(raw.as_ptr() as *const u8, std::mem::size_of_val(raw))
    };
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(0);
    if end == 0 {
        return None;
    }
    std::str::from_utf8(&bytes[..end]).ok().map(PathBuf::from)
}

/// The cwd-resolution rule, split from [`Session::cwd`] so it is testable without a PTY
/// (tmux-pane-cwd R2.1-R2.3) — the same pure-logic/thin-wrapper split `proc.rs`'s parsers and
/// `tabsession::resolved_root_with` already use.
///
/// `fallback` is a closure, not a value, so it is **not evaluated** when a cwd is published.
/// That laziness is the whole performance claim: on the hot paths (twice per keystroke, once
/// per tab per sidebar resync) a published read must cost zero syscalls, and eagerly computing
/// a fallback that is then discarded would reinstate exactly the `proc_pidinfo` this removes.
fn published_or(
    meta: Option<&TabMeta>,
    fallback: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    match meta.and_then(|m| m.cwd.clone()) {
        Some(cwd) => Some(cwd),
        None => fallback(),
    }
}

/// A tab's dialog level after a scan event (agent-card-status D7(a)/(c)). Pure, so each
/// case is testable without a window:
///
/// - **no agent** → `false`: a shell printing dialog-like text is never waiting, and an
///   agent that left takes its level with it (D7c);
/// - **an agent at the live bottom** → the `scan`;
/// - **an agent scrolled back** → the level it `held`, and the screen is **not** scanned.
///   The scrolled view is history, which may hold a dialog answered long ago, so it cannot
///   raise the level; and it cannot clear it either, because an agent blocked on a dialog
///   draws nothing more — clearing here would lose it until something else woke the tab.
///   Returning to the bottom re-reads it (D7b, the event loop's `rescan_at_live_bottom`).
fn next_dialog_level(
    has_agent: bool,
    at_live_bottom: bool,
    held: bool,
    scan: impl FnOnce() -> bool,
) -> bool {
    match (has_agent, at_live_bottom) {
        (false, _) => false,
        (true, true) => scan(),
        (true, false) => held,
    }
}

/// Normalize a user title on its way in (sticky-tab-title R2.3): a blank or whitespace-only
/// name is not a name — it is the user clearing the tab back to its auto label. Applied at
/// the single setter so every entry point (the rename chord, the card menu, anything later)
/// shares the rule instead of restating it.
fn normalize_user_title(title: Option<String>) -> Option<String> {
    title.filter(|t| !t.trim().is_empty())
}

/// Whether a *program*-set title can currently be seen on the card (sticky-tab-title R1.3).
/// It cannot while a user title is set — the user title hides it (D3) — so recording one
/// must not trigger a resync + redraw for a label that did not visibly change.
fn program_title_is_visible(user_title: Option<&str>) -> bool {
    user_title.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `TabMeta` carrying just a cwd — the only field these tests care about.
    fn meta_with(cwd: Option<&str>) -> TabMeta {
        TabMeta {
            cwd: cwd.map(PathBuf::from),
            ..TabMeta::default()
        }
    }

    /// A live session **plus** the sender its reader thread would own, so a test can
    /// push PTY bytes and call [`Session::drain`] over them.
    ///
    /// [`live_session`] drops its sender, which leaves `rx` disconnected and `drain`
    /// a no-op — fine for the cwd tests it was written for, useless for anything that
    /// arrives *from* the shell. Everything the OSC channels do (titles, command-end)
    /// is only reachable through here.
    /// `None` when shell tests are opted out of — see
    /// [`term_core::testing::shell_tests_enabled`]. Returning an `Option` rather than
    /// gating inside each test is the point: a new test that needs a shell cannot
    /// compile without handling the skip.
    fn session_with_feed() -> Option<(Session, mpsc::Sender<Vec<u8>>)> {
        if !term_core::testing::shell_tests_enabled() {
            return None;
        }
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let pty = Pty::spawn(80, 24).expect("pty");
        let writer = pty.writer().expect("writer");
        let session = Session {
            id: TabId(1),
            terminal: Terminal::new(80, 24, Color::rgb(200, 200, 200), Color::rgb(0, 0, 0)),
            pty,
            writer,
            rx,
            user_title: None,
            program_title: String::new(),
            title_dirty: false,
            command_end: None,
            last_end: None,
            command_running: false,
            command_dirty: false,
            blocking_dialog: false,
            _reader: std::thread::spawn(|| {}),
        };
        Some((session, tx))
    }

    /// Push bytes the way the reader thread would, then advance the grid over them.
    fn feed(session: &mut Session, tx: &mpsc::Sender<Vec<u8>>, bytes: &[u8]) {
        tx.send(bytes.to_vec()).expect("send");
        session.drain();
    }

    // ---------------------------------------------------------------------
    // The title channels (sticky-tab-title D3/D5, R1.1/R1.3/R2.2/R2.3).
    //
    // Added after a `cargo mutants` run over this file: 16 mutants survived, and
    // all but two of those were in this precedence machinery. (Survivors, not a
    // ratio — the total moves with every edit to the file, and three different
    // ones for this same run were recorded in three places. See CLAUDE.md.)
    // `set_user_title` could be made to return a constant `true` or `false`, its
    // `==` could be flipped to `!=`, `user_title` could answer `Some("xyzzy")`,
    // `program_title` could answer `""` — and the whole workspace suite stayed
    // green. The feature's own tests live at the layers above (`rename.rs`,
    // `card_menu.rs`, the tab model) and drive their own state, so nothing was
    // asserting the state machine they all read.
    // ---------------------------------------------------------------------

    #[test]
    fn a_fresh_session_carries_neither_title_and_is_not_dirty() {
        let Some((mut s, _tx)) = session_with_feed() else {
            return;
        };
        assert_eq!(s.user_title(), None, "a new tab is not named");
        assert_eq!(s.program_title(), "", "no program has set a title yet");
        assert!(
            !s.take_title_dirty(),
            "nothing has changed, so nothing to redraw"
        );
    }

    #[test]
    fn naming_a_tab_reports_the_change_and_asks_for_one_redraw() {
        let Some((mut s, _tx)) = session_with_feed() else {
            return;
        };
        assert!(
            s.set_user_title(Some("build".into())),
            "a new name is a change"
        );
        assert_eq!(s.user_title(), Some("build"));
        // Dirty is a *take*: the loop must be asked to resync exactly once, or a
        // background tab's new label either never shows or repaints every tick.
        assert!(s.take_title_dirty(), "a rename must ask for a resync");
        assert!(!s.take_title_dirty(), "and must not keep asking");
    }

    #[test]
    fn renaming_a_tab_to_the_name_it_already_has_changes_nothing() {
        // The `==` guard (R2.2). Flipped to `!=` this still *looks* right — the tab
        // keeps its name — but every no-op rename reports a change and drags a
        // resync + redraw behind it.
        let Some((mut s, _tx)) = session_with_feed() else {
            return;
        };
        assert!(s.set_user_title(Some("build".into())));
        assert!(s.take_title_dirty());
        assert!(
            !s.set_user_title(Some("build".into())),
            "setting the same name twice is not a change"
        );
        assert!(
            !s.take_title_dirty(),
            "a no-op rename must not cost a resync"
        );
        assert_eq!(s.user_title(), Some("build"));
    }

    #[test]
    fn a_blank_name_resets_the_tab_to_its_auto_label() {
        // R2.3 — whitespace-only is the user clearing the name, not naming the tab
        // "   ". Normalized at the setter so no caller has to know the rule.
        let Some((mut s, _tx)) = session_with_feed() else {
            return;
        };
        assert!(s.set_user_title(Some("build".into())));
        assert!(
            s.set_user_title(Some("   ".into())),
            "blank clears the name"
        );
        assert_eq!(s.user_title(), None);
        assert!(
            !s.set_user_title(None),
            "clearing an already-clear name is not a change"
        );
    }

    #[test]
    fn a_program_title_lands_in_the_program_channel_and_shows() {
        // R1.1/D3 — OSC 2 is any program's channel, kept separate from the user's.
        let Some((mut s, tx)) = session_with_feed() else {
            return;
        };
        feed(&mut s, &tx, b"\x1b]2;vim README.md\x07");
        assert_eq!(s.program_title(), "vim README.md");
        assert_eq!(s.user_title(), None, "a program cannot name the tab");
        assert!(
            s.take_title_dirty(),
            "with no user title the program's title is what shows, so it must redraw"
        );
    }

    #[test]
    fn a_program_title_hidden_under_a_user_title_is_recorded_but_costs_no_redraw() {
        // R1.3 — recorded so the channel keeps working if the user clears their
        // name, but invisible now, and redrawing for an invisible change is waste.
        let Some((mut s, tx)) = session_with_feed() else {
            return;
        };
        assert!(s.set_user_title(Some("mine".into())));
        assert!(s.take_title_dirty());

        feed(&mut s, &tx, b"\x1b]2;vim README.md\x07");
        assert_eq!(
            s.program_title(),
            "vim README.md",
            "the program channel keeps recording underneath"
        );
        assert_eq!(s.user_title(), Some("mine"), "and never outranks the user");
        assert!(
            !s.take_title_dirty(),
            "an invisible change must not trigger a resync (R1.3)"
        );
    }

    #[test]
    fn a_command_end_is_delivered_once_and_then_is_gone() {
        // T3 — the waiting model polls this. Delivered twice it raises a phantom
        // second completion; never cleared it raises one every tick.
        let Some((mut s, tx)) = session_with_feed() else {
            return;
        };
        assert_eq!(s.take_command_end(), None, "nothing has run yet");

        feed(&mut s, &tx, b"\x1b]133;D;7;1234\x07");
        assert_eq!(
            s.take_command_end(),
            Some(CommandEnd {
                exit_code: 7,
                elapsed_ms: 1234
            })
        );
        assert_eq!(s.take_command_end(), None, "and it is consumed");
    }

    #[test]
    fn the_cards_last_end_survives_the_waiting_bands_take() {
        // sidebar-weather-cards R8.4 — the band consumes its one-shot; the card's copy
        // stays, and the running state mirrors the terminal's.
        let Some((mut s, tx)) = session_with_feed() else {
            return;
        };
        assert_eq!((s.last_end(), s.command_running()), (None, false));
        feed(&mut s, &tx, b"\x1b]133;C\x07");
        assert!(s.command_running(), "C → running");
        assert!(s.take_command_state_dirty(), "a change the card shows");
        feed(&mut s, &tx, b"\x1b]133;D;101;40\x07");
        assert!(!s.command_running(), "D → finished");
        let _ = s.take_command_end(); // the waiting path takes its one-shot …
        assert_eq!(
            s.last_end(),
            Some(CommandEnd {
                exit_code: 101,
                elapsed_ms: 40
            }),
            "… and the card's copy is still there"
        );
        assert!(s.take_command_state_dirty());
        assert!(!s.take_command_state_dirty(), "the flag clears on take");
    }

    #[test]
    fn a_repeat_of_the_same_failure_does_not_resync() {
        // D8 — the dirty flag tracks what the card shows: a fast rerun that fails the
        // same way (C…D in one chunk) changes neither input.
        let Some((mut s, tx)) = session_with_feed() else {
            return;
        };
        feed(&mut s, &tx, b"\x1b]133;C\x07out\x1b]133;D;2;5\x07");
        assert!(s.take_command_state_dirty(), "the first failure shows");
        feed(&mut s, &tx, b"\x1b]133;C\x07out\x1b]133;D;2;7\x07");
        assert!(
            !s.take_command_state_dirty(),
            "the same code again changes nothing"
        );
        assert_eq!(
            s.last_end().map(|e| e.elapsed_ms),
            Some(7),
            "but the copy is fresh"
        );
        feed(&mut s, &tx, b"\x1b]133;C\x07\x1b]133;D;0;1\x07");
        assert!(s.take_command_state_dirty(), "a different code does");
    }

    #[test]
    fn resize_refits_the_grid_and_not_only_the_pty() {
        // R5 — both halves. A resize that moved the PTY but left the grid would
        // reflow the shell's output into a grid that is still the old size.
        let Some((mut s, _tx)) = session_with_feed() else {
            return;
        };
        assert_eq!(s.terminal.dimensions(), (80, 24));
        s.resize(40, 10).expect("resize");
        assert_eq!(
            s.terminal.dimensions(),
            (40, 10),
            "the grid must follow the resize, not just the PTY"
        );
    }

    #[test]
    fn shell_pid_names_this_sessions_own_live_child() {
        // The root the ai-monitor walks to find *this tab's* agent. A wrong pid does
        // not fail loudly — it silently binds the panel to another tab's session.
        let Some((s, _tx)) = session_with_feed() else {
            return;
        };
        let pid = s.shell_pid().expect("a spawned session has a shell");
        assert!(pid > 1, "pid {pid} is not a real child process");
        // A live process has a cwd; this is the session's own child, so the check is
        // about *this* process rather than about what the machine happens to run.
        assert!(
            cwd_of_pid(pid).is_some(),
            "pid {pid} does not resolve to a live process"
        );
    }

    #[test]
    fn a_published_cwd_wins_and_costs_no_fallback() {
        // R2.1 — the published value is returned.
        let got = published_or(Some(&meta_with(Some("/a/b"))), || {
            Some(PathBuf::from("/fallback"))
        });
        assert_eq!(got, Some(PathBuf::from("/a/b")));
    }

    /// A real `Session` with no winit event loop, and no sender for its reader
    /// channel.
    ///
    /// No `EventLoopProxy` anywhere: [`Session::spawn`] takes a [`Wake`] callback and
    /// `Session` holds no proxy, so the struct is constructible directly here and
    /// `Session::cwd` (the actual subject of R6.1) is measurable rather than merely
    /// arguable. Spawns a real `/bin/zsh` via `Pty::spawn`, which is consistent with
    /// existing practice in this crate (`tabsession` shells out to `/bin/ps`) and with
    /// the macOS-only rule.
    ///
    /// [`crate::tabs::Tabs`] is reachable the same way and for the same reason — it
    /// holds a [`WakeSource`], not a proxy — and `tabs.rs` constructs one directly in
    /// its own tests. An earlier version of this comment said the opposite ("`Tabs`
    /// genuinely is out of reach — it holds an `EventLoopProxy` as a field"), which
    /// would send a reader looking for a workaround that is no longer needed.
    ///
    /// Dropping `session_with_feed`'s sender is the entire difference between the two
    /// fixtures: it leaves `rx` disconnected, so `drain` is a no-op. Building the
    /// `Session` twice instead meant two copies of a twelve-field literal that had to
    /// be edited in lockstep, 220 lines apart.
    fn live_session() -> Option<Session> {
        session_with_feed().map(|(s, _tx)| s)
    }

    #[test]
    fn session_cwd_performs_zero_syscalls_when_published_and_one_when_not() {
        // R6.1 — the cost claim as a MEASURED delta on the real `Session::cwd`, not an argument
        // about whether a closure ran. This is the path evaluated twice per key press
        // (`terminal_app.rs`) and once per tab per sidebar resync (`tabs.rs`), where it used to
        // cost a `proc_pidinfo` every single time.
        let Some(s) = live_session() else {
            return;
        };
        let meta = meta_with(Some("/published"));

        // Published: zero syscalls, however many times it is called.
        let before = CWD_SYSCALLS.with(|c| c.get());
        for _ in 0..100 {
            assert_eq!(s.cwd(Some(&meta)), Some(PathBuf::from("/published")));
        }
        assert_eq!(
            CWD_SYSCALLS.with(|c| c.get()) - before,
            0,
            "100 reads on the keystroke path must cost 0 syscalls (R6.1)"
        );

        // Unpublished: the fallback fires — proving the counter is live, so the zero above is
        // a real measurement and not a counter that never moves.
        let before = CWD_SYSCALLS.with(|c| c.get());
        let _ = s.cwd(None);
        assert!(
            CWD_SYSCALLS.with(|c| c.get()) - before >= 1,
            "the fallback must actually reach the syscall — otherwise the zero proves nothing"
        );
    }

    #[test]
    fn published_or_short_circuits_before_the_fallback() {
        // The pure half of R6.1: `published_or` never evaluates its fallback when a cwd is
        // published. `session_cwd_performs_zero_syscalls_when_published_and_one_when_not` is
        // the real measurement on `Session::cwd`; this one pins the rule itself, with a
        // panicking fallback so a regression is loud rather than a silent extra syscall.
        let got = published_or(Some(&meta_with(Some("/proj"))), || {
            panic!("R6.1 violated: the keystroke path performed a cwd syscall")
        });
        assert_eq!(got, Some(PathBuf::from("/proj")));
    }

    #[test]
    fn absence_falls_back_to_the_direct_lookup() {
        // R2.2 — both flavours of absence fall through: no entry for this tab at all (the
        // not-yet-polled window), and an entry that resolved no cwd.
        let fb = || Some(PathBuf::from("/fallback"));
        assert_eq!(published_or(None, fb), Some(PathBuf::from("/fallback")));
        assert_eq!(
            published_or(Some(&meta_with(None)), fb),
            Some(PathBuf::from("/fallback"))
        );
    }

    #[test]
    fn cwd_is_never_emptier_than_the_direct_lookup() {
        // R2.3 / C7 — whenever the old implementation would have produced a path, so does
        // this; and when even the fallback has nothing (a dead shell, once the worker has
        // observed it) the answer is still `None`, not a stale directory.
        assert_eq!(
            published_or(None, || Some(PathBuf::from("/live"))),
            Some(PathBuf::from("/live")),
            "no publication yet must not be emptier than today"
        );
        assert_eq!(published_or(None, || None), None, "dead shell stays None");
        assert_eq!(
            published_or(Some(&meta_with(None)), || None),
            None,
            "polled-with-no-cwd plus dead shell stays None"
        );
    }

    #[test]
    fn blank_user_title_normalizes_to_a_clear() {
        // R2.3 — an empty or whitespace-only rename is a reset-to-auto, not a blank name.
        assert_eq!(
            normalize_user_title(Some("Frontend".into())),
            Some("Frontend".into())
        );
        assert_eq!(normalize_user_title(Some(String::new())), None);
        assert_eq!(normalize_user_title(Some("   ".into())), None);
        assert_eq!(normalize_user_title(Some("\t \n".into())), None);
        assert_eq!(normalize_user_title(None), None);
        // Interior/edge whitespace inside a real name is preserved — only all-blank clears.
        assert_eq!(
            normalize_user_title(Some(" my tab ".into())),
            Some(" my tab ".into())
        );
    }

    #[test]
    fn program_title_is_invisible_under_a_user_title() {
        // R1.3 — a named tab records a program's OSC 2 but shows nothing new, so the
        // drain must not flag the tab dirty and cost a pointless redraw.
        assert!(program_title_is_visible(None));
        assert!(!program_title_is_visible(Some("Frontend")));
    }

    #[test]
    fn a_live_agent_tab_takes_the_scan() {
        // R7.1 — at the live bottom the level is whatever the screen says, both ways.
        assert!(next_dialog_level(true, true, false, || true), "rising");
        assert!(!next_dialog_level(true, true, true, || false), "falling");
    }

    #[test]
    fn a_scrolled_back_agent_tab_holds_its_level_unscanned() {
        // R7.2 — the scrolled view is history: it neither raises nor clears the level, and
        // the screen is not read at all.
        for held in [false, true] {
            let level = next_dialog_level(true, false, held, || {
                panic!("a scrolled-back tab must not be scanned")
            });
            assert_eq!(level, held);
        }
    }

    #[test]
    fn a_tab_without_an_agent_has_no_level() {
        // R7.3 / R7.5 — dialog-like text in a shell is never waiting, and an agent that left
        // takes a held `true` with it; the screen is not read.
        for (at_bottom, held) in [(true, false), (true, true), (false, true)] {
            assert!(!next_dialog_level(false, at_bottom, held, || {
                panic!("a tab without an agent must not be scanned")
            }));
        }
    }

    #[test]
    fn a_real_dialog_sets_the_level_and_scrolling_back_holds_it() {
        // agent-card-status R7.1 / R7.2 / R7.4 / R7.5 through a real grid: the captured
        // Antigravity dialog raises the level at the live bottom; scrolled back, the level
        // holds even after the dialog is answered off-screen; back at the bottom it clears;
        // and a tab without an agent never holds one.
        let Some((mut s, tx)) = session_with_feed() else {
            return;
        };
        for i in 0..60 {
            feed(&mut s, &tx, format!("earlier output {i}\r\n").as_bytes());
        }
        let dialog = crate::prompt::REAL_ANTIGRAVITY_DIALOG.replace('\n', "\r\n");
        feed(&mut s, &tx, dialog.as_bytes());
        assert_eq!(s.rescan_dialog(true), (true, true), "live dialog");

        s.terminal.scroll_up(5);
        assert_eq!(s.rescan_dialog(true), (true, false), "scrolled back: held");
        // Answered while the user reads history: the live screen moves on.
        feed(&mut s, &tx, b"\x1b[2J\x1b[H");
        feed(&mut s, &tx, b"\xe2\x97\x8f Bash(ls -la)\r\n");
        s.terminal.scroll_up(5);
        assert_ne!(s.terminal.view_offset(), 0, "still reading history");
        assert_eq!(s.rescan_dialog(true), (true, false), "still held");

        s.terminal.scroll_to_bottom();
        assert_eq!(
            s.rescan_dialog(true),
            (false, true),
            "back at the bottom: cleared"
        );

        // The dialog again, then the agent leaves: the level goes with it.
        feed(&mut s, &tx, dialog.as_bytes());
        assert_eq!(s.rescan_dialog(true), (true, true));
        assert_eq!(s.rescan_dialog(false), (false, true), "no agent, no level");
    }

    #[test]
    fn the_dialog_level_reports_only_a_change() {
        // agent-card-status R7.6 — a flip in either direction is what resyncs the card's
        // row; the same level again, which every chunk of a busy agent produces, is not.
        let Some((mut s, _tx)) = session_with_feed() else {
            return;
        };
        assert!(!s.blocking_dialog(), "a new tab shows no dialog");
        assert!(s.set_blocking_dialog(true), "rising");
        assert!(s.blocking_dialog());
        assert!(!s.set_blocking_dialog(true), "a repeat is not a change");
        assert!(s.set_blocking_dialog(false), "falling");
        assert!(!s.blocking_dialog());
        assert!(!s.set_blocking_dialog(false), "a repeat is not a change");
    }

    /// Validate the `proc_pidinfo` struct layout: our own pid must resolve to our
    /// own cwd. A wrong offset/size would yield garbage or `None`.
    #[test]
    fn cwd_of_pid_resolves_current_process() {
        let got = cwd_of_pid(std::process::id()).expect("own cwd is resolvable");
        let expected = std::env::current_dir().unwrap();
        assert_eq!(
            got.canonicalize().unwrap(),
            expected.canonicalize().unwrap(),
            "proc_pidinfo cwd matches std::env::current_dir"
        );
    }
}
