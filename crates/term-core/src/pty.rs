//! Local PTY: spawns zsh and exposes read/write/resize handles.
//!
//! Constraint C5 — zsh + local PTY only. No SSH, no remote, no shell selection.
//! The reader/writer are handed to the app's I/O thread (D7: threads + channels);
//! `term-core` itself stays synchronous and runtime-free.

use std::io::{Read, Write};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use crate::error::{Error, Result};

/// The fixed shell for this terminal (C5).
pub const SHELL: &str = "/bin/zsh";

/// A spawned zsh session behind a pseudo-terminal.
///
/// Keep this alive for the lifetime of the session: dropping it closes the
/// master and signals the child.
pub struct Pty {
    master: Box<dyn portable_pty::MasterPty + Send>,
    #[allow(dead_code)]
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl Pty {
    /// Spawn zsh in a PTY sized to `cols * rows`.
    pub fn spawn(cols: u16, rows: u16) -> Result<Pty> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(Error::pty)?;

        let mut cmd = CommandBuilder::new(SHELL);
        cmd.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(cmd).map_err(Error::pty)?;
        // Slave handle is not needed once the child holds it.
        drop(pair.slave);

        Ok(Pty {
            master: pair.master,
            child,
        })
    }

    /// Clone a reader for the PTY output stream (move to the reader thread).
    pub fn reader(&self) -> Result<Box<dyn Read + Send>> {
        self.master.try_clone_reader().map_err(Error::pty)
    }

    /// Take the single writer for sending input to the shell.
    pub fn writer(&self) -> Result<Box<dyn Write + Send>> {
        self.master.take_writer().map_err(Error::pty)
    }

    /// Inform the kernel of a new window size so the shell reflows.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(Error::pty)
    }
}
