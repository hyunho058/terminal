//! One tab's session: a `Pty` + `Terminal` + a reader thread (D1).
//!
//! Each session is independent — its own zsh, its own grid. The reader thread
//! tags every wake with the session's stable [`TabId`] (D2) so the app advances
//! the right grid and only redraws when the *active* tab produced output.

use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;

use term_core::{Color, Pty, Terminal};
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
        got
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

    /// The tab title (D7 fallback `Tab N` until cwd titles land — see note in
    /// `tabs.rs`). Empty string means "use the fallback".
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Set the tab title (the cwd-title path will call this once a
    /// `term_core::Pty::child_pid()` accessor exists — see `tabs::titles`).
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
