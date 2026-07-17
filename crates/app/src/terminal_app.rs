//! The winit `ApplicationHandler` — THE vertical-slice exemplar (T3).
//!
//! This is the one flow an agent copies to build every future feature. It shows,
//! end to end:
//!   1. window + GPU surface creation (`resumed`)
//!   2. spawning zsh in a PTY and reading it on a background thread (D7)
//!   3. waking the UI thread via `EventLoopProxy` on new output (C6)
//!   4. feeding bytes through `term-core` and drawing the snapshot (`renderer`)
//!   5. keyboard input → PTY, and window resize → grid/PTY/renderer (T8)
//!   6. graceful teardown: dropping `Running` closes the PTY and the reader ends.

use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::JoinHandle;

use common::config::Config;
use renderer::Renderer;
use term_core::{Color, Pty, Terminal};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// Custom event used only to wake the event loop when PTY output is available.
#[derive(Debug, Clone, Copy)]
pub enum UserEvent {
    PtyOutput,
}

/// Top-level app. `inner` is populated on `resumed` (the winit lifecycle point
/// where a window/surface may be created).
pub struct App {
    cfg: Config,
    proxy: EventLoopProxy<UserEvent>,
    inner: Option<Running>,
}

impl App {
    pub fn new(cfg: Config, proxy: EventLoopProxy<UserEvent>) -> Self {
        App {
            cfg,
            proxy,
            inner: None,
        }
    }
}

/// Everything that only exists once we have a window + GPU + shell.
struct Running {
    window: Arc<Window>,
    renderer: Renderer,
    terminal: Terminal,
    pty: Pty,
    writer: Box<dyn Write + Send>,
    rx: Receiver<Vec<u8>>,
    _reader: JoinHandle<()>,
}

impl App {
    fn start(&mut self, event_loop: &ActiveEventLoop) -> anyhow_lite::Result<Running> {
        let attrs = Window::default_attributes()
            .with_title("mac-terminal")
            .with_inner_size(winit::dpi::LogicalSize::new(900.0, 540.0));
        let window = Arc::new(event_loop.create_window(attrs)?);

        let size = window.inner_size();
        let scale = window.scale_factor();
        let renderer = Renderer::new(window.clone(), size.width, size.height, scale, &self.cfg)
            .map_err(err)?;

        let (cols, rows) = renderer.grid_size();
        let terminal = Terminal::new(
            cols,
            rows,
            Color(self.cfg.theme.foreground),
            Color(self.cfg.theme.background),
        );

        let pty = Pty::spawn(cols, rows).map_err(err)?;
        let writer = pty.writer().map_err(err)?;
        let mut reader = pty.reader().map_err(err)?;

        // PTY reader thread: read → channel → wake the UI thread (D7, C6).
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let proxy = self.proxy.clone();
        let reader_thread = std::thread::Builder::new()
            .name("pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break, // EOF or shell exited
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                            let _ = proxy.send_event(UserEvent::PtyOutput);
                        }
                    }
                }
            })
            .map_err(err)?;

        tracing::info!(cols, rows, "terminal started: zsh spawned, grid ready");
        window.request_redraw();
        Ok(Running {
            window,
            renderer,
            terminal,
            pty,
            writer,
            rx,
            _reader: reader_thread,
        })
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.inner.is_some() {
            return;
        }
        match self.start(event_loop) {
            Ok(running) => self.inner = Some(running),
            Err(e) => {
                tracing::error!("failed to start terminal: {e}");
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: UserEvent) {
        let Some(inner) = self.inner.as_mut() else {
            return;
        };
        let mut got = false;
        while let Ok(bytes) = inner.rx.try_recv() {
            inner.terminal.advance(&bytes);
            got = true;
        }
        if got {
            inner.window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(inner) = self.inner.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                inner.renderer.resize(size.width, size.height);
                let (cols, rows) = inner.renderer.grid_size();
                inner.terminal.resize(cols, rows);
                if let Err(e) = inner.pty.resize(cols, rows) {
                    tracing::warn!("pty resize failed: {e}");
                }
                inner.window.request_redraw();
            }

            WindowEvent::KeyboardInput {
                event: key_event, ..
            } if key_event.state == ElementState::Pressed => {
                if let Some(bytes) = encode_key(&key_event.logical_key, key_event.text.as_deref()) {
                    if let Err(e) = inner
                        .writer
                        .write_all(&bytes)
                        .and_then(|_| inner.writer.flush())
                    {
                        tracing::warn!("pty write failed: {e}");
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                let snapshot = inner.terminal.snapshot();
                if let Err(e) = inner.renderer.render(&snapshot) {
                    tracing::warn!("render failed: {e}");
                }
            }

            _ => {}
        }
    }
}

/// Translate a key press into the bytes a shell expects. Named keys first, then
/// fall back to the event's text (covers letters, symbols, and IME output).
fn encode_key(logical: &Key, text: Option<&str>) -> Option<Vec<u8>> {
    match logical {
        Key::Named(NamedKey::Enter) => Some(vec![b'\r']),
        Key::Named(NamedKey::Backspace) => Some(vec![0x7f]),
        Key::Named(NamedKey::Tab) => Some(vec![b'\t']),
        Key::Named(NamedKey::Escape) => Some(vec![0x1b]),
        Key::Named(NamedKey::Space) => Some(vec![b' ']),
        _ => text.map(|t| t.as_bytes().to_vec()),
    }
}

/// Minimal error plumbing so `start` can use `?` across heterogeneous errors
/// without pulling in the full `anyhow` crate.
mod anyhow_lite {
    pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
}

fn err<E: Into<Box<dyn std::error::Error + Send + Sync>>>(
    e: E,
) -> Box<dyn std::error::Error + Send + Sync> {
    e.into()
}
