//! Interactive region screen capture (region-capture).
//!
//! `Cmd+Shift+G` runs `screencapture -i -s -d <path>`; on a completed selection the
//! PNG's absolute path plus one space is typed into the **originating** tab's PTY, so
//! `claude`/`gemini` receive a real file path with no clipboard round trip. That back
//! half is deliberately the same shape image-paste already uses (`clipboard.rs:74-76`),
//! and it reuses that feature's cache directory outright (D4).
//!
//! This module holds the parts that can be tested without a human, a screen or a GPU:
//! target-path computation, the fixed argv, the outcome model, and the bytes that
//! cross the delivery seam. The parts that cannot — `screencapture -i` blocks until
//! someone drags a rectangle — reach the suite through an injected spawn (D5).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use winit::keyboard::Key;

use crate::seam::{AppCtx, Handled, KeyInput, Overlay};
use crate::session::TabId;

/// The capture binary — `/usr/sbin`, **not** `/usr/bin` (D2).
///
/// Verified present at `/usr/sbin/screencapture` and verified absent from
/// `/usr/bin/`. Absolute, like the crate's production subprocess precedent
/// (`tabsession.rs:523`), so no `PATH` lookup decides which program runs.
pub(crate) const SCREENCAPTURE: &str = "/usr/sbin/screencapture";

/// Per-process sequence so two captures within one second never collide (C5, R4.3).
///
/// Its own counter rather than a share of `clipboard.rs`'s: the two name spaces are
/// already disjoint by prefix, so coupling them would buy nothing and make a paste
/// advance a capture's sequence.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// The fixed, region-only, self-reporting invocation (D2, R2.1).
///
/// `-i` interactive selection (Escape cancels); `-s` "only allow mouse selection
/// mode", which makes the region-only non-goal a property of the invocation rather
/// than a convention the code must police — window selection cannot be reached even
/// by pressing Space; `-d` "display errors to the user graphically", so a missing
/// Screen Recording grant is explained by Apple's own dialog, naming the right
/// permission, with no UI code here and no bytes on the PTY.
///
/// That last sentence states D2's *intent*, and the spec marks its premise unmeasured:
/// **A2** — whether `screencapture -i` needs the Screen Recording grant at all is
/// unknown, because the capture is performed by Apple's own signed binary and TCC
/// attribution to a spawned child's responsible process is undocumented. **A1** — TCC
/// grants bind to code identity and `scripts/bundle-app.sh` builds an unsigned,
/// un-notarized bundle by explicit non-goal, so a grant may not survive a rebuild.
/// Neither is asserted anywhere in this module, and neither may be.
///
/// `OsString`, not `String`: the target path comes from `$HOME` and need not be UTF-8.
pub(crate) fn capture_args(path: &Path) -> Vec<OsString> {
    vec![
        OsString::from("-i"),
        OsString::from("-s"),
        OsString::from("-d"),
        path.as_os_str().to_os_string(),
    ]
}

/// The filename for a capture (R4.2).
///
/// `capture-` rather than image-paste's `paste-` prefix: both features write into one
/// directory, and the prefix is what keeps provenance readable there, at zero cost.
pub(crate) fn capture_name(secs: u64, seq: u64) -> String {
    format!("capture-{secs}-{seq}.png")
}

/// Why no capture could be started.
///
/// Both variants abort **before** spawning, and both are consumed rather than passed
/// through (D8): falling through would type a literal `g` at the prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TargetError {
    /// `$HOME` is unset, or the cache directory could not be created (R4.5).
    NoCacheDir,
    /// A file already sits at the computed path (R4.4).
    ///
    /// Best-effort, and C5 says why it cannot be more: image-paste's `path.exists()`
    /// guard is a real one because the *app* performs that write, whereas here
    /// `screencapture` writes the file and would clobber, so this check has a TOCTOU
    /// window. The guarantee rests on the timestamp plus [`SEQ`]; this is the cheap
    /// second line, not the guarantee.
    Occupied(PathBuf),
}

/// Compose a target path from an already-resolved directory (R4.1-R4.4).
///
/// Split from [`next_target`] so every one of R4's sub-requirements is a pure function
/// test: `dir` arrives as an `Option` rather than being read from `$HOME` here, which
/// is what lets R4.5's "unusable cache directory" be exercised without mutating
/// process-global environment state under a parallel test harness.
fn target_in(dir: Option<PathBuf>, secs: u64, seq: u64) -> Result<PathBuf, TargetError> {
    let dir = dir.ok_or(TargetError::NoCacheDir)?;
    let path = dir.join(capture_name(secs, seq));
    if path.exists() {
        return Err(TargetError::Occupied(path));
    }
    Ok(path)
}

/// Epoch seconds, or 0 if the clock is before the epoch — the `clipboard.rs:63-66`
/// shape, so the two features agree on what a timestamp is.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The next capture's target under the image-paste cache directory (D4, D11, R4.1).
///
/// Advances [`SEQ`] on **every** call, including the ones that go on to fail: a
/// sequence that only advanced on success would hand the next capture a number an
/// aborted one had already composed a name from.
pub(crate) fn next_target() -> Result<PathBuf, TargetError> {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    target_in(crate::clipboard::pasted_dir(), now_secs(), seq)
}

/// How one capture ended.
///
/// **Every** terminal outcome is published, not just the successful one, and D10 says
/// why: D7 requires the in-flight flag to clear on cancel, timeout, spawn failure and
/// dead tab alike. Drained as a success-only value, those paths would never tell the
/// UI thread the capture ended and the chord would stay disarmed for the rest of the
/// session. Only [`Outcome::Captured`] carries bytes.
///
/// The dead-tab case is deliberately **not** a variant: the watcher cannot know the
/// tab is gone, and the drain discovers it by `Tabs::by_id_mut` answering `None`
/// (R7.7). It is a `Captured` whose delivery finds nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// A non-empty file exists at the target: deliver its path to tab `TabId` (R7.1).
    Captured(TabId, PathBuf),
    /// The capture ended without writing a file (R7.2).
    ///
    /// One variant for two causes, because D3 makes them one rule: Escape cancels,
    /// and under `-i` "the control key will cause the screenshot to go to the
    /// clipboard" — a *completed* capture that writes no file. Indistinguishable at
    /// the filesystem, identical in what should happen, so they are not separated.
    NoFile,
    /// The 5-minute budget expired and the child was killed (D7, R6.4).
    TimedOut,
    /// The spawn itself failed — a missing or unexecutable binary (R6.5, R8.1).
    ///
    /// `-d` cannot report this one, because nothing spawned.
    SpawnFailed,
}

impl Outcome {
    /// The bytes to type, or `None` for every non-success outcome (R7.2).
    pub(crate) fn bytes(&self) -> Option<(TabId, Vec<u8>)> {
        match self {
            Outcome::Captured(id, path) => Some((*id, path_bytes(path))),
            Outcome::NoFile | Outcome::TimedOut | Outcome::SpawnFailed => None,
        }
    }
}

/// The absolute path followed by exactly one space (D9, R7.5, R7.6).
///
/// Not bracketed-paste wrapped, and that matches `clipboard.rs:74-76`'s image branch:
/// `wrap_paste` exists to stop multi-line pasted *text* executing, and a single path
/// token with no newline needs none of it. Wrapping would make the two image paths
/// differ for no reason.
fn path_bytes(path: &Path) -> Vec<u8> {
    let mut bytes = path.to_string_lossy().into_owned().into_bytes();
    bytes.push(b' ');
    bytes
}

/// How long a capture may stay in flight before the child is killed (D7, R6.4).
///
/// **Five minutes, and the number is load-bearing in the opposite direction from the
/// crate's existing budget.** `tabsession::RESOLVE_BUDGET` is ~250 ms for a `ps` walk;
/// copying that constant here would abort every real capture mid-drag. This is a
/// wedge-recovery device, not a UX deadline — far beyond any human selection, still
/// re-arming within one work session.
///
/// Rejected — 30 s: an interrupted user returns to a silently dead crosshair.
/// Rejected — no timeout: a child that never exits (a permission dialog left open
/// behind another window) disables the chord until the terminal restarts.
const BUDGET: Duration = Duration::from_secs(300);

/// How often the watcher asks whether the child has exited.
///
/// `std::process::Child` has no `wait_timeout` on stable, so D7's deadline is enforced
/// by polling. 50 ms is imperceptible against a human dragging a rectangle and costs
/// the watcher thread — never the UI thread — twenty wakeups a second.
const TICK: Duration = Duration::from_millis(50);

/// A spawned `screencapture`, as the watcher sees it.
///
/// A trait rather than `std::process::Child` because `screencapture -i` waits for a
/// human and so can never run under `cargo test` (R5.2). The in-crate precedent is
/// `resolved_root_with`, which is "generic over the resolver so the budget is testable
/// without actually hanging the suite" (`tabsession.rs:427`) — without the same move
/// here, D5, D7 and D12 would have no testable requirement at all.
pub(crate) trait CaptureChild: Send {
    /// Has it exited? `Ok(true)` = yes. Never blocks.
    fn try_wait(&mut self) -> std::io::Result<bool>;
    /// End it, and reap it. Called only on budget expiry.
    fn kill(&mut self);
}

/// Start a capture writing to `path`.
///
/// `Arc`, and `Send + Sync`: it is cloned onto the watcher thread on every capture.
pub(crate) type Spawn = Arc<dyn Fn(&Path) -> std::io::Result<Box<dyn CaptureChild>> + Send + Sync>;

/// Where the next capture should write.
///
/// Injected for the same reason as [`Spawn`]: it makes R4.4's occupied path and R4.5's
/// unusable cache directory reachable from a test without mutating `$HOME`, which is
/// process-global and would race a parallel harness.
pub(crate) type Target = Arc<dyn Fn() -> Result<PathBuf, TargetError> + Send + Sync>;

/// Ask the UI thread to come and look (R7.3).
///
/// A callback rather than the `EventLoopProxy` it wraps, for exactly the reason
/// [`crate::session::Wake`] is one: on macOS an `EventLoop` may be built only on the
/// process's main thread and the harness runs every test on a spawned one, so a
/// watcher that demanded a proxy could never have its wake observed by any test — and
/// deleting the `send_event` would leave the suite green while every capture waited
/// for some unrelated background tick to deliver it.
pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;

/// A real OS process, adapted to [`CaptureChild`].
///
/// At module scope rather than nested inside [`real_spawn`] so it is reachable from a
/// test. That is not cosmetic: nested, the only way to construct one was to launch
/// `screencapture`, which waits for a human — so `try_wait` could be replaced with a
/// constant and `kill` with a no-op, and the whole suite stayed green. It is wrapped
/// around whatever `Child` it is handed, so a test uses `/bin/sleep` instead.
struct ProcChild(std::process::Child);

impl CaptureChild for ProcChild {
    fn try_wait(&mut self) -> std::io::Result<bool> {
        self.0.try_wait().map(|status| status.is_some())
    }
    fn kill(&mut self) {
        let _ = self.0.kill();
        // Reap it, so a killed capture leaves no zombie behind for the life of the
        // process. `kill` alone only sends the signal.
        let _ = self.0.wait();
    }
}

/// `screencapture` itself (D2, R2.1).
fn real_spawn(path: &Path) -> std::io::Result<Box<dyn CaptureChild>> {
    let child = std::process::Command::new(SCREENCAPTURE)
        .args(capture_args(path))
        .spawn()?;
    Ok(Box::new(ProcChild(child)))
}

/// Did a capture happen? **The file decides, never the exit status** (D3, R3).
///
/// The man page documents no exit code for cancellation and opens by admitting the
/// tool "is not very well documented to date". Worse, it documents a case where the
/// user *completes* a capture and no file appears: under `-i`, "the control key will
/// cause the screenshot to go to the clipboard". Cancel and Ctrl-divert are
/// indistinguishable at the process level and identical at the filesystem — in both,
/// nothing was written and nothing should be typed. One rule collapses all three
/// outcomes correctly and depends on nothing undocumented.
///
/// A zero-length file is not a capture (R3.3): `screencapture` creates the file before
/// it has the pixels, so an empty one is a capture that did not finish.
fn verdict(id: TabId, path: &Path) -> Outcome {
    match std::fs::metadata(path) {
        Ok(m) if m.len() > 0 => Outcome::Captured(id, path.to_path_buf()),
        _ => Outcome::NoFile,
    }
}

/// The watcher thread's whole job: spawn, poll to the deadline, judge by the file.
///
/// **The deadline lives here, on this thread, and that is the half of the
/// `resolved_root_with` precedent that must NOT transfer.** That function spawns a
/// thread and then calls `rx.recv_timeout(RESOLVE_BUDGET)` *on the calling thread*
/// (`tabsession.rs:436`). Copied literally with [`BUDGET`] it would freeze the UI for
/// five minutes — and C6's continuous redraw exists because the compositor drops the
/// presented frame once the run loop sleeps (winit#2640), so that stall does not merely
/// lag the window, it blanks it.
fn run_capture(spawn: &Spawn, id: TabId, path: &Path, budget: Duration, tick: Duration) -> Outcome {
    let mut child = match spawn(path) {
        Ok(child) => child,
        // Nothing spawned, so `-d` cannot report this one (R6.5, R8.1).
        Err(_) => return Outcome::SpawnFailed,
    };
    let deadline = Instant::now() + budget;
    loop {
        match child.try_wait() {
            Ok(true) => return verdict(id, path),
            // Supervision failed, which for a child we own means essentially never.
            // Kill it rather than poll a child we cannot read — an unkilled crosshair
            // with the flag cleared would let a second capture start under the first.
            // Then judge by the file anyway, because D3's rule never looked at the
            // process to begin with.
            Err(_) => {
                child.kill();
                return verdict(id, path);
            }
            Ok(false) => {}
        }
        if Instant::now() >= deadline {
            child.kill();
            return Outcome::TimedOut;
        }
        std::thread::sleep(tick);
    }
}

/// One tab-tagged capture at a time, and the chord that re-arms after every outcome.
///
/// Owns the in-flight flag (D7), the channel the watcher publishes over, and the bytes
/// waiting for the drain. The `Overlay` impl that puts it on the seam is separate, so
/// this half is drivable with no window, no proxy and no screen.
pub(crate) struct Capture {
    spawn: Spawn,
    target: Target,
    wake: Wake,
    budget: Duration,
    tick: Duration,
    /// **One at a time** (D7): `screencapture` owns the whole screen and a second
    /// crosshair is incoherent. Cleared on *every* terminal outcome in [`Self::poll`]
    /// — success, cancel, timeout, spawn failure, dead tab — because a flag that leaks
    /// on one path disables the chord for the rest of the session.
    in_flight: bool,
    tx: Sender<Outcome>,
    rx: Receiver<Outcome>,
    /// Bytes the drain has not collected yet (R7.4).
    pending: Option<(TabId, Vec<u8>)>,
}

impl Capture {
    /// The production capture: the real binary, the real cache directory, and a wake
    /// that goes through `waker`.
    pub(crate) fn new(wake: Wake) -> Self {
        Self::with(
            Arc::new(real_spawn),
            Arc::new(next_target),
            wake,
            BUDGET,
            TICK,
        )
    }

    /// Every collaborator injected — what the tests drive (R5.2).
    fn with(spawn: Spawn, target: Target, wake: Wake, budget: Duration, tick: Duration) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Capture {
            spawn,
            target,
            wake,
            budget,
            tick,
            in_flight: false,
            tx,
            rx,
            pending: None,
        }
    }

    /// Begin a capture for tab `id`; `true` if one was actually started.
    ///
    /// **Returns immediately** — the watcher thread owns the wait (D5/C4, R5.1). The
    /// caller consumes the chord either way (D8): `false` here means "nothing started",
    /// never "let the key through", because falling through would type a literal `g`.
    pub(crate) fn start(&mut self, id: TabId) -> bool {
        // R6.1 — a second chord while the crosshair is up does nothing.
        if self.in_flight {
            return false;
        }
        // R4.4 / R4.5 — an occupied target or an unusable cache directory aborts
        // BEFORE spawning, so a failed capture never leaves a crosshair on screen.
        let Ok(path) = (self.target)() else {
            return false;
        };
        self.in_flight = true;
        let spawn = self.spawn.clone();
        let wake = self.wake.clone();
        let tx = self.tx.clone();
        let (budget, tick) = (self.budget, self.tick);
        std::thread::spawn(move || {
            let outcome = run_capture(&spawn, id, &path, budget, tick);
            // The receiver is the overlay, which lives as long as the app; a send that
            // fails means the app is going away, which is not an error here.
            let _ = tx.send(outcome);
            // **Every** outcome wakes the loop, not just the successful one (R7.3).
            // Without the wake on a cancel, the in-flight flag would clear only when
            // some unrelated watcher happened to tick — and on a quiet tab there may
            // never be one, leaving the chord dead.
            wake();
        });
        true
    }

    /// Fold finished captures in (the [`crate::seam::Overlay::poll`] half).
    ///
    /// Clearing the flag here rather than on the watcher thread is what makes the six
    /// outcomes one path instead of six: whatever ended the capture, it arrives as a
    /// value on this channel, and the flag clears once, where the value is read.
    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(outcome) = self.rx.try_recv() {
            self.in_flight = false;
            if let Some(bytes) = outcome.bytes() {
                self.pending = Some(bytes);
            }
            changed = true;
        }
        changed
    }

    /// Hand the drain whatever a completed capture produced (R7.4).
    pub(crate) fn take_pending_write(&mut self) -> Option<(TabId, Vec<u8>)> {
        self.pending.take()
    }

    /// Whether a crosshair is currently up.
    #[cfg(test)]
    fn in_flight(&self) -> bool {
        self.in_flight
    }
}

/// Is this `Cmd+Shift+G`? (D1, R1.1-R1.4)
///
/// **"G" for Grab**, the name of Apple's original screenshot utility — and free:
/// the case-folded letter chords taken across `crates/app/src/` are exactly
/// `a c e m r s v`.
///
/// **Rejected — `Cmd+Shift+4`**: macOS takes `Cmd+Shift+3/4/5` before any application
/// sees the event. This repo already paid for that lesson — the ai-monitor section
/// jumps were `Cmd+Shift+<digit>` and 3-5 were simply unreachable, which is why they
/// moved to `Cmd+Opt+<digit>` (`input.rs:623-627`). Reclaiming it needs the system
/// shortcut disabled globally, breaking screenshots in every other app to gain it here.
///
/// **Rejected — `Cmd+Shift+A`**: `rename.rs:220-222` matches `Cmd+A` case-folded
/// **without excluding shift**, and overlays dispatch before the default path, so with
/// a rename field open it would re-select the tab name instead of capturing.
///
/// Case-folded, like every other letter chord in the crate (`copy.rs:21`,
/// `input.rs:231`): with Shift held, the key arrives as `G`.
fn is_capture_chord(input: &KeyInput) -> bool {
    if !(input.mods.super_key() && input.mods.shift_key()) {
        return false;
    }
    matches!(&input.key, Key::Character(s) if s.eq_ignore_ascii_case("g"))
}

/// The capture's seat on the SD4 overlay seam (D10).
///
/// An **overlay**, not an `InputHandler`, and that is the decision the whole feature
/// turned on: `feature_handlers()` takes no arguments (`lib.rs:174`) while
/// `feature_overlays(proxy, arm)` is handed an `EventLoopProxy` — so only an overlay
/// can wake the loop from a background thread. A capture finishes seconds after the
/// chord, on a thread, and without that wake it would be delivered whenever some
/// unrelated tabmeta or ai-monitor tick happened next.
///
/// Invisible, like `ImagePresenter`: it draws nothing and owns no band. Dispatch offers
/// keys to hidden overlays too, which is what lets it consume its own chord and pass
/// everything else through.
pub(crate) struct CaptureOverlay {
    capture: Capture,
}

impl CaptureOverlay {
    pub(crate) fn new(wake: Wake) -> Self {
        CaptureOverlay {
            capture: Capture::new(wake),
        }
    }
}

impl Overlay for CaptureOverlay {
    /// `0`, below the rename field's 200 — and **D1 silently depends on it**. D1's
    /// known modal limit (an open rename field swallows every `Cmd`/`Ctrl` chord,
    /// `rename.rs:276-278`) is only true while the capture sits below it.
    fn priority(&self) -> i32 {
        0
    }

    /// Never (R10.2). Not an input-capturing surface: it consumes its own chord and
    /// nothing else.
    fn visible(&self) -> bool {
        false
    }

    fn on_key(&mut self, input: &KeyInput, ctx: &mut AppCtx) -> Handled {
        if !is_capture_chord(input) {
            return Handled::Passthrough;
        }
        // D6 — the capture belongs to the tab that was active when the chord was
        // pressed, named by id and not by index, because the roster can change before
        // it completes. `None` only in a test that built a bare context.
        if let Some(id) = ctx.active_tab {
            self.capture.start(id);
        }
        // **`Consumed` on every path, including every failure path** (D8, R8).
        // `Passthrough` does not mean "do nothing" for this chord: winit reports `text`
        // for Cmd chords, and `encode_key`'s terminal arm writes it to the shell — so
        // falling through would type a literal `g` at the prompt on every failed
        // capture, breaking D1 and C6.
        Handled::Consumed
    }

    /// The watcher's finished captures, folded in (D10).
    fn poll(&mut self) -> bool {
        self.capture.poll()
    }

    fn take_pending_write(&mut self) -> Option<(TabId, Vec<u8>)> {
        self.capture.take_pending_write()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long a helper waits for the watcher thread before giving up.
    ///
    /// Bounds a *failure*, never a success: the stub child exits within a poll or two
    /// of a 1 ms tick, so a passing test spends single-digit milliseconds here. Kept
    /// short deliberately — a mutant that stops `start` from ever spawning makes every
    /// helper below run to this limit at once, and a suite that HANGS reports worse
    /// than one that fails.
    const POLL_LIMIT: Duration = Duration::from_secs(3);

    /// A temp directory of this test's own, so nothing here touches `~/.cache`.
    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("capture-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_binary_is_in_usr_sbin_not_usr_bin() {
        // D2 — `/usr/bin/screencapture` does not exist; naming it would make the
        // feature silently unspawnable.
        assert_eq!(SCREENCAPTURE, "/usr/sbin/screencapture");
    }

    #[test]
    fn the_argv_is_exactly_the_region_only_self_reporting_form() {
        // R2.1 — program and arguments, in that order.
        let p = Path::new("/tmp/x.png");
        assert_eq!(
            capture_args(p),
            vec![
                OsString::from("-i"),
                OsString::from("-s"),
                OsString::from("-d"),
                OsString::from("/tmp/x.png"),
            ]
        );
    }

    #[test]
    fn the_argv_carries_the_target_path_last() {
        // R2.1 — the path is the final argument, not a flag value spliced elsewhere.
        let args = capture_args(Path::new("/a/b/c.png"));
        assert_eq!(args.len(), 4);
        assert_eq!(args[3], OsString::from("/a/b/c.png"));
    }

    #[test]
    fn the_name_carries_the_capture_prefix_and_timestamp() {
        // R4.2.
        assert_eq!(capture_name(1700000000, 7), "capture-1700000000-7.png");
    }

    #[test]
    fn the_name_is_distinguishable_from_a_paste() {
        // R4.2 — one directory holds both; the prefix is what keeps provenance readable.
        let n = capture_name(42, 0);
        assert!(n.starts_with("capture-"), "{n}");
        assert!(!n.starts_with("paste-"), "{n}");
        assert!(n.ends_with(".png"), "{n}");
    }

    #[test]
    fn two_captures_in_one_second_get_distinct_names() {
        // R4.3 — same epoch second, advanced sequence.
        assert_ne!(capture_name(1000, 0), capture_name(1000, 1));
    }

    #[test]
    fn the_target_sits_in_the_given_directory() {
        // R4.1 — parent is the cache dir, filename is the capture name.
        let dir = tmp("target");
        let p = target_in(Some(dir.clone()), 500, 3).unwrap();
        assert_eq!(p.parent(), Some(dir.as_path()));
        assert_eq!(p.file_name().unwrap(), "capture-500-3.png");
    }

    #[test]
    fn an_absent_cache_directory_aborts() {
        // R4.5 — `$HOME` unset, or the directory could not be created.
        assert_eq!(target_in(None, 1, 1), Err(TargetError::NoCacheDir));
    }

    #[test]
    fn an_occupied_target_aborts_and_names_the_path() {
        // R4.4 — never overwrite, the `clipboard.rs:70-71` guard (C5's cheap second line).
        let dir = tmp("occupied");
        let taken = dir.join(capture_name(9, 0));
        std::fs::write(&taken, b"already here").unwrap();
        assert_eq!(
            target_in(Some(dir), 9, 0),
            Err(TargetError::Occupied(taken.clone()))
        );
        // The existing file is untouched by the abort.
        assert_eq!(std::fs::read(&taken).unwrap(), b"already here");
    }

    #[test]
    fn an_unoccupied_target_is_accepted() {
        // R4.4's negative — the guard must reject only what exists, not everything.
        let dir = tmp("free");
        assert!(target_in(Some(dir), 9, 0).is_ok());
    }

    #[test]
    fn next_target_lands_under_the_image_paste_cache_directory() {
        // R4.1 through the production path: same directory as Cmd+V (D4/D11).
        let Some(expected) = crate::clipboard::pasted_dir() else {
            // No `$HOME` on this machine — R4.5's case, asserted by its own test.
            return;
        };
        let p = next_target().expect("a fresh capture-<secs>-<seq> name cannot be occupied");
        assert_eq!(p.parent(), Some(expected.as_path()));
        assert!(
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("capture-"),
            "{p:?}"
        );
    }

    #[test]
    fn next_target_advances_the_sequence_every_call() {
        // R4.3 through the production path — two calls in the same second differ.
        if crate::clipboard::pasted_dir().is_none() {
            return;
        }
        let a = next_target().unwrap();
        let b = next_target().unwrap();
        assert_ne!(a, b, "the per-process sequence did not advance");
    }

    #[test]
    fn a_capture_carries_its_tab_and_its_path() {
        // R7.1 — the published value names the ORIGINATING tab (D6).
        let out = Outcome::Captured(TabId(7), PathBuf::from("/tmp/c.png"));
        let (id, bytes) = out.bytes().expect("a capture carries bytes");
        assert_eq!(id, TabId(7));
        assert_eq!(bytes, b"/tmp/c.png ");
    }

    #[test]
    fn no_non_success_outcome_carries_bytes() {
        // R7.2 — cancel, timeout and spawn failure all report, none of them types.
        for out in [Outcome::NoFile, Outcome::TimedOut, Outcome::SpawnFailed] {
            assert_eq!(out.bytes(), None, "{out:?} must carry no bytes");
        }
    }

    #[test]
    fn the_bytes_are_the_path_plus_exactly_one_space() {
        // R7.5.
        let bytes = path_bytes(Path::new("/a/b/capture-1-0.png"));
        assert_eq!(bytes, b"/a/b/capture-1-0.png ");
        assert_eq!(bytes.last(), Some(&b' '));
        assert_eq!(
            bytes.iter().filter(|b| **b == b' ').count(),
            1,
            "exactly one space, and it is the terminator"
        );
    }

    #[test]
    fn the_bytes_are_not_bracketed_paste_wrapped() {
        // R7.6 — matching image-paste's image branch, which also does not wrap.
        let bytes = path_bytes(Path::new("/a/b.png"));
        assert!(!bytes.starts_with(b"\x1b[200~"));
        assert!(!bytes.ends_with(b"\x1b[201~"));
        assert!(!bytes.windows(2).any(|w| w == b"\x1b["));
    }

    // -----------------------------------------------------------------------
    // The verdict (D3 / R3) — the file decides, never the exit status.
    // -----------------------------------------------------------------------

    #[test]
    fn r3_1_a_non_empty_file_is_a_completed_capture() {
        let dir = tmp("verdict-ok");
        let p = dir.join("c.png");
        std::fs::write(&p, b"\x89PNG....").unwrap();
        assert_eq!(verdict(TabId(2), &p), Outcome::Captured(TabId(2), p));
    }

    #[test]
    fn r3_2_a_missing_file_is_not_a_capture() {
        // Esc mid-capture writes nothing.
        let dir = tmp("verdict-missing");
        assert_eq!(verdict(TabId(2), &dir.join("absent.png")), Outcome::NoFile);
    }

    #[test]
    fn r3_3_a_zero_length_file_is_not_a_capture() {
        // `screencapture` creates the file before it has the pixels.
        let dir = tmp("verdict-empty");
        let p = dir.join("c.png");
        std::fs::write(&p, b"").unwrap();
        assert_eq!(verdict(TabId(2), &p), Outcome::NoFile);
    }

    // -----------------------------------------------------------------------
    // The watcher (D5 / D7 / R5 / R6 / R7) — driven through an injected spawn.
    // -----------------------------------------------------------------------

    /// A stub `screencapture`: exits after `exits_after` polls, and records its kill.
    struct FakeChild {
        polls: u32,
        exits_after: Option<u32>,
        killed: Arc<std::sync::atomic::AtomicBool>,
        /// Make `try_wait` itself fail — the "cannot supervise" branch.
        broken: bool,
    }

    impl CaptureChild for FakeChild {
        fn try_wait(&mut self) -> std::io::Result<bool> {
            if self.broken {
                return Err(std::io::Error::other("no such process"));
            }
            self.polls += 1;
            Ok(self.exits_after.is_some_and(|n| self.polls > n))
        }
        fn kill(&mut self) {
            self.killed.store(true, Ordering::SeqCst);
        }
    }

    /// Everything a test needs to see about one capture run.
    struct Rig {
        spawns: Arc<AtomicU64>,
        killed: Arc<std::sync::atomic::AtomicBool>,
        wakes: Arc<AtomicU64>,
        dir: PathBuf,
    }

    /// A [`Capture`] whose child exits promptly, writing `contents` (if any) first.
    ///
    /// The whole point of R5.2: `screencapture -i` waits for a human, so every outcome
    /// path below is reached with no human, no screen and no GPU.
    fn rig(tag: &str, contents: Option<&'static [u8]>, exits_after: Option<u32>) -> (Capture, Rig) {
        let dir = tmp(tag);
        let target_dir = dir.clone();
        let spawns = Arc::new(AtomicU64::new(0));
        let killed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let wakes = Arc::new(AtomicU64::new(0));

        let s = spawns.clone();
        let k = killed.clone();
        let spawn: Spawn = Arc::new(move |path: &Path| {
            s.fetch_add(1, Ordering::SeqCst);
            if let Some(bytes) = contents {
                std::fs::write(path, bytes).expect("stub write");
            }
            Ok(Box::new(FakeChild {
                polls: 0,
                exits_after,
                killed: k.clone(),
                broken: false,
            }) as Box<dyn CaptureChild>)
        });

        let seq = AtomicU64::new(0);
        let target: Target = Arc::new(move || {
            let n = seq.fetch_add(1, Ordering::SeqCst);
            target_in(Some(target_dir.clone()), 7, n)
        });

        let w = wakes.clone();
        let wake: Wake = Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        });

        let capture = Capture::with(
            spawn,
            target,
            wake,
            Duration::from_secs(5),
            Duration::from_millis(1),
        );
        (
            capture,
            Rig {
                spawns,
                killed,
                wakes,
                dir,
            },
        )
    }

    /// Wait for the spawn count to reach `n`.
    ///
    /// The count cannot be read straight after `start`, and that is the feature working:
    /// `start` returns before the watcher thread has even called the spawn (R5.1/C4), so
    /// an immediate assertion races the thread rather than observing it.
    fn wait_spawns(counter: &AtomicU64, n: u64) -> u64 {
        let deadline = Instant::now() + POLL_LIMIT;
        while Instant::now() < deadline {
            let seen = counter.load(Ordering::SeqCst);
            if seen >= n {
                return seen;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        counter.load(Ordering::SeqCst)
    }

    /// Poll until the watcher has published, or give up. Bounds a *failure*.
    fn settle(capture: &mut Capture) -> bool {
        let deadline = Instant::now() + POLL_LIMIT;
        while Instant::now() < deadline {
            if capture.poll() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    #[test]
    fn r5_2_a_successful_capture_runs_end_to_end_with_no_screen() {
        // R0.1 / R7.1 — the published value names the ORIGINATING tab and carries the
        // path of an existing, non-empty PNG under the target directory.
        let (mut capture, rig) = rig("run-ok", Some(b"\x89PNG-data"), Some(0));
        assert!(capture.start(TabId(3)));
        assert!(settle(&mut capture), "the watcher never published");

        let (id, bytes) = capture.take_pending_write().expect("a capture delivers");
        assert_eq!(id, TabId(3));
        let path = PathBuf::from(String::from_utf8(bytes[..bytes.len() - 1].to_vec()).unwrap());
        assert_eq!(path.parent(), Some(rig.dir.as_path()));
        assert!(path.exists() && path.metadata().unwrap().len() > 0);
        assert_eq!(bytes.last(), Some(&b' '), "R7.5 — one trailing space");
    }

    #[test]
    fn r0_2_a_cancelled_capture_delivers_nothing() {
        // No file written — Esc, or the Ctrl-divert to the clipboard.
        let (mut capture, _rig) = rig("run-cancel", None, Some(0));
        assert!(capture.start(TabId(3)));
        assert!(settle(&mut capture));
        assert_eq!(
            capture.take_pending_write(),
            None,
            "a cancel must put zero bytes on any PTY"
        );
    }

    #[test]
    fn r3_4_there_is_no_exit_status_for_the_verdict_to_read() {
        // R3.4's deliberate negative — and the name says what it can actually observe.
        // An earlier name claimed "a non-zero exit with a valid file", which `FakeChild`
        // cannot construct: `CaptureChild::try_wait` returns `io::Result<bool>` and so
        // carries no status at all. That is the *point* rather than a gap — giving the
        // stub a status would first require adding to the trait the very thing D3
        // forbids reading. What is asserted here is the consequence: the verdict is
        // identical to R3.1's because nothing downstream of `try_wait` has a status to
        // branch on. If this trait ever grows one, A3 is why it must not be read.
        let (mut capture, _rig) = rig("run-nonzero", Some(b"\x89PNG"), Some(0));
        assert!(capture.start(TabId(1)));
        assert!(settle(&mut capture));
        assert!(
            capture.take_pending_write().is_some(),
            "the verdict changed with the exit status"
        );
    }

    #[test]
    fn r5_1_start_returns_while_the_capture_is_still_in_flight() {
        // C4 — `screencapture -i` blocks for as long as the human takes to drag a
        // rectangle, and blocking the UI thread does not merely lag the window, it
        // BLANKS it (winit#2640). The child here never exits.
        let (mut capture, rig) = rig("run-blocking", None, None);
        let t0 = Instant::now();
        assert!(capture.start(TabId(0)));
        let elapsed = t0.elapsed();
        assert!(
            elapsed < Duration::from_millis(500),
            "start blocked for {elapsed:?} — the UI thread waited on the capture"
        );
        assert!(capture.in_flight(), "the capture is still running");
        assert_eq!(wait_spawns(&rig.spawns, 1), 1);
    }

    #[test]
    fn r6_1_a_second_chord_while_in_flight_is_still_consumed() {
        // R6.1 has two halves and the other test covers only one. "No second spawn
        // occurs" is about `Capture::start`; "**and the chord is still consumed**" is
        // about `on_key`, and going through `start` alone can never see it. If the
        // in-flight case fell through, a user pressing the chord twice would type a
        // literal `g` at their agent's prompt.
        let (mut o, spawns) = overlay_rig("chord-inflight-consumed");
        assert_eq!(press(&mut o, &chord("g", CMD_SHIFT)).0, Handled::Consumed);
        assert_eq!(wait_spawns(&spawns, 1), 1);

        let (handled, wrote) = press(&mut o, &chord("g", CMD_SHIFT));
        assert_eq!(handled, Handled::Consumed, "the second chord fell through");
        assert!(wrote.is_empty(), "the second chord typed a literal g");
        assert_eq!(
            spawns.load(Ordering::SeqCst),
            1,
            "a second crosshair appeared"
        );
    }

    #[test]
    fn a_chord_with_no_active_tab_is_consumed_and_starts_nothing() {
        // The fourth path out of `on_key`, and the only one no other test reaches:
        // `ctx.active_tab` is `None`. There is no tab to deliver to, so no capture
        // begins — but D8 still forbids `Passthrough`, because the fall-through types a
        // literal `g` regardless of why the capture did not start.
        let (mut o, spawns) = overlay_rig("chord-no-tab");
        let mut buf: Vec<u8> = Vec::new();
        let handled = {
            // No `.with_active_tab(..)` — a bare context, as the seam's own docs say
            // only a test builds.
            let mut ctx = AppCtx::new(&mut buf, 1, 0, None);
            o.on_key(&chord("g", CMD_SHIFT), &mut ctx)
        };
        assert_eq!(handled, Handled::Consumed);
        assert!(buf.is_empty());
        assert_eq!(spawns.load(Ordering::SeqCst), 0);
        assert!(
            !o.capture.in_flight(),
            "a capture began with no tab to deliver to"
        );
    }

    #[test]
    fn r6_1_a_second_chord_while_in_flight_starts_nothing() {
        let (mut capture, rig) = rig("single-flight", None, None);
        assert!(capture.start(TabId(0)));
        assert!(!capture.start(TabId(0)), "a second crosshair is incoherent");
        assert!(!capture.start(TabId(1)), "not even from another tab");
        assert_eq!(
            wait_spawns(&rig.spawns, 1),
            1,
            "a second crosshair reached the screen"
        );
    }

    #[test]
    fn r6_2_the_chord_re_arms_after_a_successful_capture() {
        let (mut capture, rig) = rig("rearm-ok", Some(b"\x89PNG"), Some(0));
        assert!(capture.start(TabId(0)));
        assert!(settle(&mut capture));
        assert!(!capture.in_flight(), "the flag leaked on the success path");
        assert!(capture.start(TabId(0)));
        assert_eq!(wait_spawns(&rig.spawns, 2), 2);
    }

    #[test]
    fn r6_3_the_chord_re_arms_after_a_cancelled_capture() {
        let (mut capture, rig) = rig("rearm-cancel", None, Some(0));
        assert!(capture.start(TabId(0)));
        assert!(settle(&mut capture));
        assert!(!capture.in_flight(), "the flag leaked on the cancel path");
        assert!(capture.start(TabId(0)));
        assert_eq!(wait_spawns(&rig.spawns, 2), 2);
    }

    #[test]
    fn r6_4_the_budget_expires_kills_the_child_and_re_arms() {
        // D7's wedge recovery, at a budget a test can wait out. A child that never
        // exits — a permission dialog left open behind another window — must not
        // disable the chord until the terminal restarts.
        let (mut capture, rig) = rig("budget", None, None);
        capture.budget = Duration::from_millis(20);
        assert!(capture.start(TabId(0)));
        assert!(settle(&mut capture), "the deadline never fired");

        assert!(
            rig.killed.load(Ordering::SeqCst),
            "the budget expired without killing the child"
        );
        assert_eq!(
            capture.take_pending_write(),
            None,
            "a timed-out capture must deliver nothing"
        );
        assert!(!capture.in_flight(), "the flag leaked on the timeout path");
        assert!(capture.start(TabId(0)));
        assert_eq!(wait_spawns(&rig.spawns, 2), 2);
    }

    #[test]
    fn r6_6_the_chord_re_arms_when_the_originating_tab_is_gone() {
        // The sixth outcome, and the one with no variant of its own: the capture
        // SUCCEEDED, so the watcher published `Captured` and the flag cleared here —
        // the tab's absence is discovered later, at the drain (R7.7). Which is exactly
        // why the flag must clear on the value arriving, not on the delivery landing:
        // gated on delivery, a closed tab would disable the chord for good.
        let (mut capture, rig) = rig("rearm-dead-tab", Some(b"\x89PNG"), Some(0));
        assert!(capture.start(TabId(9)));
        assert!(settle(&mut capture));
        assert!(!capture.in_flight());

        // The drain takes the bytes and finds no tab 9; nothing here knows or cares.
        let (id, _) = capture.take_pending_write().expect("it did capture");
        assert_eq!(id, TabId(9));

        assert!(capture.start(TabId(0)), "a new capture, from another tab");
        assert_eq!(wait_spawns(&rig.spawns, 2), 2);
    }

    #[test]
    fn a_child_that_exits_inside_the_budget_is_never_killed() {
        // R6.4's negative — the kill is the timeout's, not every capture's. Killing a
        // crosshair the user is actively dragging would be exactly the wrong thing.
        let (mut capture, rig) = rig("no-kill", Some(b"\x89PNG"), Some(2));
        assert!(capture.start(TabId(0)));
        assert!(settle(&mut capture));
        assert!(
            !rig.killed.load(Ordering::SeqCst),
            "a capture that finished on its own was killed anyway"
        );
    }

    #[test]
    fn r6_5_the_chord_re_arms_after_a_spawn_failure() {
        // A missing or unexecutable `/usr/sbin/screencapture`. `-d` cannot report this
        // one, because nothing spawned.
        let dir = tmp("spawn-fail");
        let spawns = Arc::new(AtomicU64::new(0));
        let s = spawns.clone();
        let spawn: Spawn = Arc::new(move |_: &Path| {
            s.fetch_add(1, Ordering::SeqCst);
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        let seq = AtomicU64::new(0);
        let target: Target =
            Arc::new(move || target_in(Some(dir.clone()), 7, seq.fetch_add(1, Ordering::SeqCst)));
        let mut capture = Capture::with(
            spawn,
            target,
            Arc::new(|| {}),
            Duration::from_secs(5),
            Duration::from_millis(1),
        );

        assert!(capture.start(TabId(0)));
        assert!(settle(&mut capture));
        assert_eq!(capture.take_pending_write(), None);
        assert!(
            !capture.in_flight(),
            "the flag leaked on the spawn-failure path"
        );
        assert!(capture.start(TabId(0)));
        assert_eq!(wait_spawns(&spawns, 2), 2);
    }

    #[test]
    fn an_unsupervisable_child_is_killed_rather_than_polled_forever() {
        // `try_wait` failing means the watcher cannot read the child. Leaving it alive
        // while clearing the flag would let a second capture start under the first
        // one's crosshair.
        let dir = tmp("broken-child");
        let killed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let k = killed.clone();
        let spawn: Spawn = Arc::new(move |_: &Path| {
            Ok(Box::new(FakeChild {
                polls: 0,
                exits_after: None,
                killed: k.clone(),
                broken: true,
            }) as Box<dyn CaptureChild>)
        });
        let seq = AtomicU64::new(0);
        let target: Target =
            Arc::new(move || target_in(Some(dir.clone()), 7, seq.fetch_add(1, Ordering::SeqCst)));
        let mut capture = Capture::with(
            spawn,
            target,
            Arc::new(|| {}),
            Duration::from_secs(5),
            Duration::from_millis(1),
        );
        assert!(capture.start(TabId(0)));
        assert!(settle(&mut capture));
        assert!(killed.load(Ordering::SeqCst));
        assert!(!capture.in_flight());
    }

    #[test]
    fn r4_4_an_occupied_target_starts_nothing() {
        // The abort happens BEFORE the spawn, so no crosshair appears at all.
        let dir = tmp("start-occupied");
        std::fs::write(dir.join(capture_name(7, 0)), b"taken").unwrap();
        let spawns = Arc::new(AtomicU64::new(0));
        let s = spawns.clone();
        let spawn: Spawn = Arc::new(move |_: &Path| {
            s.fetch_add(1, Ordering::SeqCst);
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        let target: Target = Arc::new(move || target_in(Some(dir.clone()), 7, 0));
        let mut capture = Capture::with(
            spawn,
            target,
            Arc::new(|| {}),
            Duration::from_secs(5),
            Duration::from_millis(1),
        );
        assert!(!capture.start(TabId(0)));
        assert_eq!(spawns.load(Ordering::SeqCst), 0, "it spawned anyway");
        assert!(!capture.in_flight(), "an aborted start armed the flag");
    }

    #[test]
    fn r4_5_an_unusable_cache_directory_starts_nothing() {
        let spawns = Arc::new(AtomicU64::new(0));
        let s = spawns.clone();
        let spawn: Spawn = Arc::new(move |_: &Path| {
            s.fetch_add(1, Ordering::SeqCst);
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        let mut capture = Capture::with(
            spawn,
            Arc::new(|| Err(TargetError::NoCacheDir)),
            Arc::new(|| {}),
            Duration::from_secs(5),
            Duration::from_millis(1),
        );
        assert!(!capture.start(TabId(0)));
        assert_eq!(spawns.load(Ordering::SeqCst), 0);
        assert!(!capture.in_flight());
    }

    #[test]
    fn r7_3_every_outcome_wakes_the_loop() {
        // Not just the successful one. Without the wake on a cancel, the flag would
        // clear only when some unrelated watcher happened to tick — and on a quiet tab
        // there may never be one, leaving the chord dead with nothing to diagnose.
        for (tag, contents) in [("wake-ok", Some(&b"\x89PNG"[..])), ("wake-cancel", None)] {
            let (mut capture, rig) = rig(tag, contents, Some(0));
            assert!(capture.start(TabId(0)));
            assert!(settle(&mut capture));
            assert_eq!(
                rig.wakes.load(Ordering::SeqCst),
                1,
                "{tag}: the watcher published without asking for a look"
            );
        }
    }

    #[test]
    fn r7_2_a_non_success_outcome_still_reports_so_the_flag_can_clear() {
        // The reason the value crossing the channel is an OUTCOME, not a success:
        // drained as success-only, cancel/timeout/spawn-failure would never tell the
        // UI thread the capture ended, and the chord would stay disarmed.
        let (mut capture, _rig) = rig("report-cancel", None, Some(0));
        assert!(capture.start(TabId(0)));
        assert!(
            settle(&mut capture),
            "poll reported no change, so nothing crossed the channel"
        );
        assert!(!capture.in_flight());
    }

    #[test]
    fn a_pending_capture_is_handed_over_once() {
        // `take`, not `peek` — a second drain would type the path twice.
        let (mut capture, _rig) = rig("take-once", Some(b"\x89PNG"), Some(0));
        assert!(capture.start(TabId(0)));
        assert!(settle(&mut capture));
        assert!(capture.take_pending_write().is_some());
        assert_eq!(capture.take_pending_write(), None);
    }

    // -----------------------------------------------------------------------
    // The chord and the overlay (D1 / D8 / R0 / R1 / R8 / R10 / R11).
    // -----------------------------------------------------------------------

    fn chord(ch: &str, mods: winit::keyboard::ModifiersState) -> KeyInput {
        KeyInput {
            key: Key::Character(ch.into()),
            // Present for a Cmd chord — which is exactly why D8 forbids `Passthrough`:
            // `encode_key`'s terminal arm would write this to the shell.
            text: Some(ch.to_string()),
            mods,
            physical: crate::seam::NO_PHYSICAL,
        }
    }

    const CMD: winit::keyboard::ModifiersState = winit::keyboard::ModifiersState::SUPER;
    const CMD_SHIFT: winit::keyboard::ModifiersState =
        winit::keyboard::ModifiersState::SUPER.union(winit::keyboard::ModifiersState::SHIFT);

    /// A capture overlay whose spawn is a counter, so "was a capture requested" is a
    /// number rather than a screen.
    fn overlay_rig(tag: &str) -> (CaptureOverlay, Arc<AtomicU64>) {
        let (capture, rig) = rig(tag, None, None);
        (CaptureOverlay { capture }, rig.spawns)
    }

    /// Dispatch one key at the overlay, recording what reached the PTY.
    fn press(overlay: &mut CaptureOverlay, input: &KeyInput) -> (Handled, Vec<u8>) {
        let mut buf: Vec<u8> = Vec::new();
        let handled = {
            let mut ctx = AppCtx::new(&mut buf, 1, 0, None).with_active_tab(TabId(5), None);
            overlay.on_key(input, &mut ctx)
        };
        (handled, buf)
    }

    #[test]
    fn r1_1_cmd_shift_g_starts_a_capture_and_is_consumed() {
        let (mut o, spawns) = overlay_rig("chord-start");
        let (handled, wrote) = press(&mut o, &chord("g", CMD_SHIFT));
        assert_eq!(handled, Handled::Consumed);
        assert_eq!(
            wait_spawns(&spawns, 1),
            1,
            "exactly one capture was requested"
        );
        assert!(wrote.is_empty(), "R1.5 — the chord itself writes nothing");
    }

    #[test]
    fn r1_2_the_chord_is_matched_case_folded() {
        // With Shift held the key arrives as `G`, which is the ONLY way it can arrive.
        // A match on lowercase `g` alone would make the chord unreachable.
        let (mut o, spawns) = overlay_rig("chord-upper");
        assert_eq!(press(&mut o, &chord("G", CMD_SHIFT)).0, Handled::Consumed);
        assert_eq!(wait_spawns(&spawns, 1), 1);
    }

    #[test]
    fn r1_3_a_bare_g_is_not_the_chord() {
        // Typing `g` at the prompt must reach the shell.
        let (mut o, spawns) = overlay_rig("chord-bare");
        let (handled, _) = press(
            &mut o,
            &chord("g", winit::keyboard::ModifiersState::empty()),
        );
        assert_eq!(handled, Handled::Passthrough);
        assert_eq!(spawns.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn r1_4_cmd_g_without_shift_is_not_the_chord() {
        let (mut o, spawns) = overlay_rig("chord-noshift");
        assert_eq!(press(&mut o, &chord("g", CMD)).0, Handled::Passthrough);
        assert_eq!(spawns.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn r1_6_the_chord_collides_with_no_existing_chord() {
        // C6's list, dispatched one at a time. `Cmd+Shift+M` (ai-monitor),
        // `Cmd+Shift+R` (rename), `Cmd+Shift+S` (sidebar), `Cmd+C`/`Cmd+V`,
        // `Cmd+T`/`Cmd+W`, `Cmd+<digit>`, and rename's modal `Cmd+A`.
        let (mut o, spawns) = overlay_rig("chord-collisions");
        let others = [
            ("m", CMD_SHIFT),
            ("r", CMD_SHIFT),
            ("s", CMD_SHIFT),
            ("a", CMD_SHIFT),
            ("a", CMD),
            ("c", CMD),
            ("v", CMD),
            ("t", CMD),
            ("w", CMD),
            ("1", CMD),
            ("9", CMD),
        ];
        for (ch, mods) in others {
            let (handled, wrote) = press(&mut o, &chord(ch, mods));
            assert_eq!(
                handled,
                Handled::Passthrough,
                "the capture overlay swallowed {ch:?} — another feature's chord"
            );
            assert!(wrote.is_empty());
        }
        assert_eq!(
            spawns.load(Ordering::SeqCst),
            0,
            "another feature's chord started a screen capture"
        );
    }

    #[test]
    fn r8_1_and_r8_2_every_failure_path_consumes_and_writes_nothing() {
        // D8 — `Passthrough` does not mean "do nothing" for this chord: winit reports
        // `text` for Cmd chords and `encode_key`'s terminal arm writes it, so falling
        // through would type a literal `g` at the prompt on every failed capture.
        //
        // All three failure paths: an unusable cache directory (R4.5), an occupied
        // target (R4.4), and a spawn failure (R6.5).
        let failing_spawn: Spawn =
            Arc::new(|_: &Path| Err(std::io::Error::from(std::io::ErrorKind::NotFound)));

        let occupied_dir = tmp("fail-occupied");
        std::fs::write(occupied_dir.join(capture_name(7, 0)), b"taken").unwrap();

        let targets: Vec<Target> = vec![
            // R4.5
            Arc::new(|| Err(TargetError::NoCacheDir)),
            // R4.4
            Arc::new(move || target_in(Some(occupied_dir.clone()), 7, 0)),
            // R6.5 — a good target, but the spawn itself fails.
            Arc::new(|| Ok(std::env::temp_dir().join("capture-never-written.png"))),
        ];

        for (i, target) in targets.into_iter().enumerate() {
            let mut o = CaptureOverlay {
                capture: Capture::with(
                    failing_spawn.clone(),
                    target,
                    Arc::new(|| {}),
                    Duration::from_secs(5),
                    Duration::from_millis(1),
                ),
            };
            let (handled, wrote) = press(&mut o, &chord("g", CMD_SHIFT));
            assert_eq!(
                handled,
                Handled::Consumed,
                "failure path #{i} returned Passthrough, which types a literal `g`"
            );
            assert!(
                wrote.is_empty(),
                "failure path #{i} put {:?} on the PTY",
                String::from_utf8_lossy(&wrote)
            );
        }
    }

    #[test]
    fn r10_2_the_overlay_starts_invisible_at_priority_zero() {
        // R10.1 / R10.2 — it draws nothing and owns no band, so it must never be
        // "visible"; and priority 0 keeps it BELOW the rename field, which is what
        // makes D1's known modal limit true.
        let (o, _) = overlay_rig("overlay-shape");
        assert!(!o.visible());
        assert_eq!(o.priority(), 0);
    }

    #[test]
    fn r10_1_the_registration_list_names_the_capture_overlay() {
        // **Identity, which every shape assertion misses.** `wiring.rs` pins the set's
        // length, its priorities, its layer count, its descending order and its initial
        // invisibility — and all five survive replacing this overlay with a second
        // `ImagePresenter`, because that substitution changes no count and no priority.
        // Measured, not feared: with `CaptureOverlay::new(..)` swapped for
        // `Box::new(ImagePresenter)` the whole wiring suite reported 7 passed / 0
        // failed, while `Cmd+Shift+G` reached nothing at all.
        //
        // Structural rather than behavioural, and the reason is the feature itself: the
        // registered overlay carries the **production** spawn, so dispatching the real
        // chord at the real set would put a `screencapture` crosshair on the screen of
        // whoever ran `cargo test`. The behavioural half — that priority 0 is early
        // enough to receive the chord — is the test below.
        //
        // **Comments are stripped before the match, and that is not cosmetic.** The
        // first version of this guard searched the raw source, so leaving behind a
        // `// TODO: restore Box::new(CaptureOverlay::new(..))` comment satisfied it
        // while the overlay was gone — measured: 2 passed, 0 failed, chord dead. A
        // guard that a comment can satisfy is not a guard. Its two siblings in
        // `seam.rs` already strip; this one now does too.
        let src = include_str!("lib.rs");
        let body = src
            .split_once("pub fn feature_overlays(")
            .expect("the registration list exists")
            .1
            .split_once("\n}")
            .expect("its body ends")
            .0
            .lines()
            .map(|l| l.split("//").next().unwrap_or(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains("CaptureOverlay::new"),
            "the capture overlay left the registration list; the chord now reaches \
             nothing and no count-based assertion can see it: {body}"
        );
    }

    #[test]
    fn r10_1_priority_zero_still_receives_the_chord() {
        // **The real overlays, in the real order** — an earlier version of this test
        // used four `Passthrough` stand-ins and so could not fail for the reason its
        // name gives. Two things were wrong with it. The stand-ins consumed nothing, so
        // "an overlay above capture swallows the chord" was untestable by construction.
        // And it registered capture *first* among the three priority-0 overlays, while
        // `lib.rs` pushes it *last* (`ImagePresenter`, `SelectionPresenter`, then
        // capture) — and `OverlayHost::register` sorts with `sort_by_key`, which is
        // **stable**, so ties keep push order. The rehearsal was testing a slot
        // production does not use.
        //
        // What this catches that the structural guard cannot: registered is not the
        // same as reachable. `rename.rs:276-278` consumes every `Cmd`/`Ctrl` chord
        // while its field is open, and D1's "known modal limit" is only acceptable
        // because that field is *closed* by default — if any of these four claimed
        // `Cmd+Shift+G` unconditionally, the chord would be dead on arrival.
        let (capture, rig) = rig("registered-order", None, None);
        let mut host = crate::seam::OverlayHost::default();
        host.register(Box::new(crate::rename::RenameOverlay::new()));
        host.register(Box::new(crate::overlay::AiMonitorOverlay::with_waker(
            None,
            crate::overlay::no_deadline(),
        )));
        host.register(Box::new(crate::clipboard::ImagePresenter));
        host.register(Box::new(crate::copy::SelectionPresenter));
        // Last, exactly as `feature_overlays` pushes it.
        host.register(Box::new(CaptureOverlay { capture }));
        assert_eq!(
            host.priorities(),
            vec![200, 100, 0, 0, 0],
            "the rehearsed set no longer matches the registered one"
        );

        let mut buf: Vec<u8> = Vec::new();
        let handled = {
            let mut ctx = AppCtx::new(&mut buf, 1, 0, None).with_active_tab(TabId(1), None);
            host.dispatch(&chord("g", CMD_SHIFT), &mut ctx)
        };
        assert_eq!(
            handled,
            Handled::Consumed,
            "the chord crossed the whole registered set and fell through to the shell"
        );
        assert!(
            buf.is_empty(),
            "R1.5 — the chord itself put bytes on the PTY"
        );
        assert_eq!(
            wait_spawns(&rig.spawns, 1),
            1,
            "the chord was consumed by something ABOVE capture — registered, but \
             unreachable, which is as dead as never registered at all"
        );
    }

    #[test]
    fn the_production_budget_and_tick_are_the_decided_values() {
        // D7's five minutes, pinned. **`cargo mutants` cannot see this**: it generates
        // no mutant for a `const`, so `BUDGET` could be edited to 3 seconds and both
        // Gate 1 and the mutation gate would stay fully green while every real capture
        // died mid-drag. Measured: `cargo mutants --list -f capture.rs` reports 53
        // mutants and none of them names `BUDGET`, `TICK` or `Capture::new`.
        assert_eq!(BUDGET, Duration::from_secs(300), "D7's budget changed");
        assert_eq!(
            TICK,
            Duration::from_millis(50),
            "the watcher's poll interval"
        );
        assert!(
            TICK < BUDGET,
            "a tick at or beyond the budget makes the deadline unreachable"
        );
    }

    #[test]
    fn the_production_capture_is_built_idle_with_the_decided_budget() {
        // The other half of the same blind spot: `Capture::new` is the only place the
        // two constants are wired in, and passing them in the wrong order type-checks —
        // both are `Duration`. That would give a 50 ms budget and a 300 s tick: every
        // capture killed before the crosshair was drawn.
        let capture = Capture::new(Arc::new(|| {}));
        assert!(!capture.in_flight(), "a fresh capture starts disarmed");
        assert_eq!(capture.budget, BUDGET, "budget and tick are swapped");
        assert_eq!(capture.tick, TICK, "budget and tick are swapped");
    }

    #[test]
    fn r11_1_closing_the_originating_tab_does_not_kill_the_capture() {
        // D12 — killing it would yank a crosshair the user is actively dragging. The
        // capture is left to finish; its bytes are dropped at the drain (R7.7), which
        // `tabs::deliver` owns. Here: nothing in the overlay's own surface can end a
        // capture early, so a tab closing cannot.
        let (mut o, spawns) = overlay_rig("tab-close");
        press(&mut o, &chord("g", CMD_SHIFT));
        assert_eq!(wait_spawns(&spawns, 1), 1);
        assert!(o.capture.in_flight(), "the capture is up");

        // Everything the app can do to an overlay while a tab goes away.
        o.on_active_changed(None, None, None, None);
        assert_eq!(
            o.on_command(&crate::seam::OverlayCommand::Cancel),
            Handled::Passthrough,
            "the capture must not claim Cancel — Esc belongs to `screencapture`"
        );
        o.poll();
        assert!(
            o.capture.in_flight(),
            "the capture ended when its tab did; a crosshair mid-drag was yanked"
        );
    }

    #[test]
    fn r0_1_a_completed_capture_reaches_the_seam_from_a_keypress() {
        // The whole feature in one test: chord → watcher → poll → the bytes the drain
        // will write. Everything except `Tabs::deliver`, which needs a real tab set
        // and is asserted in `tabs.rs`.
        let (capture, rig) = rig("end-to-end", Some(b"\x89PNG-data"), Some(0));
        let mut o = CaptureOverlay { capture };

        let (handled, wrote) = press(&mut o, &chord("g", CMD_SHIFT));
        assert_eq!(handled, Handled::Consumed);
        assert!(wrote.is_empty(), "the keypress itself types nothing");

        let deadline = Instant::now() + POLL_LIMIT;
        while Instant::now() < deadline && !o.poll() {
            std::thread::sleep(Duration::from_millis(2));
        }
        let (id, bytes) = o
            .take_pending_write()
            .expect("the capture reached the seam");
        assert_eq!(id, TabId(5), "delivered to the tab that pressed the chord");
        let text = String::from_utf8(bytes).unwrap();
        let path = PathBuf::from(text.trim_end());
        assert_eq!(path.parent(), Some(rig.dir.as_path()));
        assert!(path.exists() && path.metadata().unwrap().len() > 0);
        assert!(text.ends_with(' '));
    }

    #[test]
    fn r0_2_a_cancelled_capture_reaches_the_seam_with_nothing() {
        let (capture, _rig) = rig("end-to-end-cancel", None, Some(0));
        let mut o = CaptureOverlay { capture };
        assert_eq!(press(&mut o, &chord("g", CMD_SHIFT)).0, Handled::Consumed);

        let deadline = Instant::now() + POLL_LIMIT;
        while Instant::now() < deadline && !o.poll() {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            o.take_pending_write(),
            None,
            "a cancelled capture must leave the prompt untouched"
        );
    }

    #[test]
    fn now_secs_reads_the_real_clock() {
        // The timestamp half of the collision guard (C5). A constant here is unique
        // enough WITHIN one process — [`SEQ`] still advances — and collides across
        // restarts: the second run recomputes `capture-<constant>-0.png`, finds it
        // occupied, and R4.4 aborts. The chord would work until the first restart and
        // then silently stop.
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let n = now_secs();
        assert!(
            n > 1_700_000_000,
            "not a plausible epoch second: {n} — the clock is not being read"
        );
        assert!(
            n >= before && n <= before + 5,
            "{n} is not the current second ({before})"
        );
    }

    /// A real child process that is not `screencapture`: it exits on its own, and can
    /// be killed before it does.
    fn sleeper(secs: &str) -> ProcChild {
        ProcChild(
            std::process::Command::new("/bin/sleep")
                .arg(secs)
                .spawn()
                .expect("/bin/sleep"),
        )
    }

    #[test]
    fn a_real_child_reports_running_then_exited() {
        // The production adapter over `std::process::Child`, which nothing could reach
        // while it was nested inside `real_spawn`: the only way to build one was to
        // launch `screencapture`, which waits for a human. Both answers matter — a
        // constant `true` judges every capture the instant it starts (no file yet, so
        // every capture reads as a cancel), and a constant `false` never finishes one,
        // so every capture runs to the 5-minute budget and is killed mid-drag.
        let mut child = sleeper("0.3");
        assert!(
            !child.try_wait().expect("try_wait"),
            "a running child reported as exited"
        );
        let deadline = Instant::now() + POLL_LIMIT;
        while Instant::now() < deadline {
            if child.try_wait().expect("try_wait") {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("an exited child never reported as exited");
    }

    #[test]
    fn a_real_child_can_be_killed() {
        // D7's wedge recovery, on a real process. A `kill` that does nothing leaves
        // `screencapture` owning the screen with its crosshair up, while the flag
        // clears and the chord re-arms — two crosshairs, which is the state D7 exists
        // to prevent.
        let mut child = sleeper("30");
        assert!(!child.try_wait().expect("try_wait"));
        child.kill();
        assert!(
            child.try_wait().expect("try_wait"),
            "the child outlived its kill"
        );
    }

    #[test]
    fn poll_reports_nothing_when_no_capture_has_finished() {
        // This arm runs on every OverlayPending wake, and tabmeta and the ai-monitor
        // produce most of them. A `poll` that always claimed a change would add a
        // redraw to every one of those.
        let (mut capture, _rig) = rig("poll-idle", None, None);
        assert!(!capture.poll());
        assert!(capture.start(TabId(0)));
        assert!(!capture.poll(), "the capture has not finished yet");
    }
}
