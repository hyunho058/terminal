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
    title: String,
    /// Set in [`drain`](Session::drain) when an OSC 0/2 title actually changed the manual
    /// label, cleared by [`take_title_dirty`](Session::take_title_dirty). Lets the event
    /// loop resync the tab model so a new label shows immediately, not on the next tab
    /// switch (tab-identity) — even for a background tab.
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
            title: String::new(),
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
        // Pull any OSC 0/2 title the shell or user set (tab-identity manual label). The
        // latest wins; an empty title clears the label back to the auto/cwd label. Flag a
        // real change so the event loop resyncs the tab model (the label shows at once).
        if let Some(title) = self.terminal.take_osc_title() {
            if title != self.title {
                self.title = title;
                self.title_dirty = true;
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

    /// The shell's live working directory, via the child pid + `proc_pidinfo`
    /// (macOS). Follows `cd`, so it drives per-tab log resolution (ai-monitor R1.1/
    /// R3.1) and cwd-basename tab titles (R6.1). `None` if the pid/cwd is
    /// unavailable.
    pub fn cwd(&self) -> Option<PathBuf> {
        cwd_of_pid(self.pty.child_pid()?)
    }

    /// The tab's explicit manual label from an OSC 0/2 title (tab-identity), or `""`
    /// when none/cleared. `tabs::titles` uses it as the highest-precedence card label
    /// (manual > auto AI-task > cwd), which is how two sessions in the same folder get
    /// distinct labels (the D4/R5 same-cwd limitation's user-driven escape hatch).
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Set the tab title explicitly. The OSC-2 drain in [`drain`](Session::drain) is the
    /// live source; this stays for programmatic overrides/tests.
    #[allow(dead_code)]
    pub fn set_title(&mut self, title: String) {
        self.title = title;
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

/// The current working directory of process `pid` via macOS `proc_pidinfo`
/// (`PROC_PIDVNODEPATHINFO`). Read-only. `None` on any failure (dead pid,
/// permission, empty path) — callers degrade gracefully.
fn cwd_of_pid(pid: u32) -> Option<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
