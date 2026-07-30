//! One tab's session: a `Pty` + `Terminal` + a reader thread (D1).
//!
//! Each session is independent — its own zsh, its own grid. The reader thread
//! tags every wake with the session's stable [`TabId`] (D2) so the app advances
//! the right grid and only redraws when the *active* tab produced output.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;

use term_core::{Color, CommandEnd, Pty, Terminal};

use crate::tabmeta::TabMeta;
use winit::event_loop::EventLoopProxy;

use crate::terminal_app::UserEvent;

/// A stable, monotonic tab identifier (never reused; survives index shifts on
/// close, unlike a `Vec` position).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct TabId(pub u64);

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
    _reader: JoinHandle<()>,
}

impl Session {
    /// Spawn a zsh session sized to `cols`×`rows`, wiring its reader thread to
    /// wake the loop with `UserEvent::PtyOutput(id)` (D2).
    pub fn spawn(
        id: TabId,
        cols: u16,
        rows: u16,
        fg: Color,
        bg: Color,
        proxy: EventLoopProxy<UserEvent>,
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
                            let _ = proxy.send_event(UserEvent::PtyOutput(id));
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
        // After advancing, pull any OSC 133 command-end the shell emitted on this
        // session's PTY. Keeps a still-unconsumed event if none is newer, so a
        // slow waiting-model poll (T3) can't miss one (spec R4.6).
        if let Some(ev) = self.terminal.take_command_end() {
            self.command_end = Some(ev);
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
    // Allowed dead until the rename field (T4) and the card menu (T6) call it — the same
    // convention `seam.rs` uses for a seam landed one task ahead of its first consumer.
    #[allow(dead_code)]
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
    if n < size {
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

    #[test]
    fn a_published_cwd_wins_and_costs_no_fallback() {
        // R2.1 — the published value is returned.
        let got = published_or(Some(&meta_with(Some("/a/b"))), || {
            Some(PathBuf::from("/fallback"))
        });
        assert_eq!(got, Some(PathBuf::from("/a/b")));
    }

    /// A real `Session` with no winit event loop.
    ///
    /// `EventLoopProxy` is a parameter of [`Session::spawn`], **not a field of `Session`** — so
    /// the struct is constructible directly here, and `Session::cwd` (the actual subject of
    /// R6.1) is measurable rather than merely arguable. Spawns a real `/bin/zsh` via
    /// `Pty::spawn`, which is consistent with existing practice in this crate (`tabsession`
    /// shells out to `/bin/ps`) and with the macOS-only rule.
    ///
    /// `Tabs` genuinely is out of reach — it holds an `EventLoopProxy` as a field — but its
    /// `active_cwd`/`titles` are thin wrappers whose only cwd cost is the `Session::cwd` call
    /// measured here.
    fn live_session() -> Session {
        let (_tx, rx) = mpsc::channel::<Vec<u8>>();
        let pty = Pty::spawn(80, 24).expect("pty");
        let writer = pty.writer().expect("writer");
        Session {
            id: TabId(1),
            terminal: Terminal::new(80, 24, Color::rgb(200, 200, 200), Color::rgb(0, 0, 0)),
            pty,
            writer,
            rx,
            user_title: None,
            program_title: String::new(),
            title_dirty: false,
            command_end: None,
            _reader: std::thread::spawn(|| {}),
        }
    }

    #[test]
    fn session_cwd_performs_zero_syscalls_when_published_and_one_when_not() {
        // R6.1 — the cost claim as a MEASURED delta on the real `Session::cwd`, not an argument
        // about whether a closure ran. This is the path evaluated twice per key press
        // (`terminal_app.rs`) and once per tab per sidebar resync (`tabs.rs`), where it used to
        // cost a `proc_pidinfo` every single time.
        let s = live_session();
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
