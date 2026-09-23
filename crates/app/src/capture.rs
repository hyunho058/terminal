//! Interactive region screen capture, to the clipboard (region-capture).
//!
//! `Cmd+Shift+G` runs `screencapture -i -s -c`: the user drags a rectangle and the
//! image lands on the system pasteboard. Nothing is written to disk and nothing is
//! typed into the shell — `Cmd+V` afterwards is what moves it onward, and inside this
//! terminal that hits the existing image-paste path (`clipboard.rs`), which writes a
//! PNG and types its path for `claude`/`gemini`.
//!
//! **This reverses the original spec's delivery model, deliberately and on the user's
//! instruction (2026-09-23).** `specs/region-capture/spec.md` was written so the agent
//! got a path "with no clipboard involvement", skipping a copy/paste round trip; the
//! revision puts that round trip back in exchange for an image that can go anywhere,
//! not just to the tab's agent. The spec carries a Revision section recording the swap,
//! and the machinery the old model needed — a tab-tagged delivery seam, a cache
//! directory, a file-based verdict — was **deleted** rather than left orphaned.
//!
//! What survives unchanged is the part that was never about delivery: the chord, the
//! watcher thread that keeps `screencapture` off the UI thread (D5/C4), the
//! single-flight budget (D7), and `Consumed` on every failure path (D8).

use std::ffi::OsString;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use objc2_app_kit::NSPasteboard;
use winit::keyboard::Key;

use crate::seam::{AppCtx, Handled, KeyInput, Overlay};

/// The capture binary — `/usr/sbin`, **not** `/usr/bin` (D2).
///
/// Verified 2026-09-23 on macOS 26.6.2 (25G83): present at `/usr/sbin/screencapture`,
/// absent from `/usr/bin/`. Absolute, like the crate's production subprocess precedent
/// (`tabsession.rs:523`), so no `PATH` lookup decides which program runs.
pub(crate) const SCREENCAPTURE: &str = "/usr/sbin/screencapture";

/// How long a capture may stay in flight before the child is killed (D7).
///
/// **Five minutes, load-bearing in the opposite direction from the crate's existing
/// budget.** `tabsession::RESOLVE_BUDGET` is ~250 ms for a `ps` walk; copying that here
/// would abort every real capture mid-drag. This is a wedge-recovery device, not a UX
/// deadline — far beyond any human selection, still re-arming within one work session.
const BUDGET: Duration = Duration::from_secs(300);

/// How often the watcher asks whether the child has exited.
///
/// `std::process::Child` has no `wait_timeout` on stable, so D7's deadline is enforced
/// by polling. 50 ms is imperceptible against a human dragging a rectangle and costs
/// the watcher thread — never the UI thread — twenty wakeups a second.
const TICK: Duration = Duration::from_millis(50);

/// The fixed, region-only, clipboard-bound invocation.
///
/// `-i` interactive selection (Escape cancels); `-s` "only allow mouse selection mode",
/// which makes the region-only non-goal a property of the invocation rather than a
/// convention the code must police; `-c` "force screen capture to go to the clipboard".
///
/// **`-d` is gone with the file.** It asked `screencapture` to report errors
/// graphically, and D2 chose it so a denied capture would explain itself. Measured
/// 2026-09-23 on macOS 26.6.2: it does put up a dialog, but the dialog reads
/// "Screenshots are not allowed." and names no permission, no application and no
/// remedy — so it never delivered what D2 wanted from it. It is dropped here because
/// this form has no destination path to fail at writing; the log line in
/// [`Capture::poll`] is what reports a capture that produced nothing.
///
/// Takes no path: with `-c` the destination is the pasteboard.
pub(crate) fn capture_args() -> Vec<OsString> {
    vec![
        OsString::from("-i"),
        OsString::from("-s"),
        OsString::from("-c"),
    ]
}

/// A spawned `screencapture`, as the watcher sees it.
///
/// A trait rather than `std::process::Child` because `screencapture -i` waits for a
/// human and so can never run under `cargo test`. The in-crate precedent is
/// `resolved_root_with`, "generic over the resolver so the budget is testable without
/// actually hanging the suite" (`tabsession.rs:427`).
pub(crate) trait CaptureChild: Send {
    /// Has it exited? `Ok(true)` = yes. Never blocks.
    fn try_wait(&mut self) -> std::io::Result<bool>;
    /// End it, and reap it. Called only on budget expiry.
    fn kill(&mut self);
}

/// Start a capture. `Arc` + `Send + Sync`: cloned onto the watcher thread each time.
pub(crate) type Spawn = Arc<dyn Fn() -> std::io::Result<Box<dyn CaptureChild>> + Send + Sync>;

/// Ask the UI thread to come and look.
///
/// A callback rather than the `EventLoopProxy` it wraps, for the reason
/// [`crate::session::Wake`] is one: macOS builds an `EventLoop` only on the main thread
/// and the harness runs every test on a spawned one, so a watcher that demanded a proxy
/// could never have its wake observed by any test — and deleting the `send_event` would
/// leave the suite green while every capture waited for some unrelated background tick.
pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;

/// Reads the pasteboard's change counter.
///
/// Injected for the same reason as [`Spawn`]: it is the only way to tell a completed
/// capture from a cancelled one, and a test must be able to drive both without a real
/// pasteboard.
pub(crate) type ChangeCount = Arc<dyn Fn() -> isize + Send + Sync>;

/// A real OS process, adapted to [`CaptureChild`].
///
/// At module scope rather than nested inside [`real_spawn`] so it is reachable from a
/// test: nested, the only way to construct one was to launch `screencapture`, which
/// waits for a human — so `try_wait` could be replaced with a constant and `kill` with
/// a no-op, and the whole suite stayed green.
struct ProcChild(std::process::Child);

impl CaptureChild for ProcChild {
    fn try_wait(&mut self) -> std::io::Result<bool> {
        self.0.try_wait().map(|status| status.is_some())
    }
    fn kill(&mut self) {
        let _ = self.0.kill();
        // Reap it, so a killed capture leaves no zombie for the life of the process.
        let _ = self.0.wait();
    }
}

/// `screencapture` itself.
fn real_spawn() -> std::io::Result<Box<dyn CaptureChild>> {
    let child = std::process::Command::new(SCREENCAPTURE)
        .args(capture_args())
        .spawn()?;
    Ok(Box::new(ProcChild(child)))
}

/// The general pasteboard's change counter.
///
/// Runs on the **UI thread only**, mirroring `clipboard.rs`'s discipline for every
/// other pasteboard read (`read_clipboard_image_png`, `read_clipboard_string`).
/// `NSPasteboard` is not documented thread-safe, and keeping this off the watcher is
/// what lets the whole feature touch AppKit from one thread.
fn pasteboard_change_count() -> isize {
    // No `unsafe` block: this binding's `changeCount` is a safe method, unlike the
    // `dataForType`/`stringForType` reads in `clipboard.rs` that do need one.
    NSPasteboard::generalPasteboard().changeCount()
}

/// What a completed capture was judged to be.
///
/// **The read side of a decision that would otherwise be write-only.** The verdict
/// drives only a log line — the image is already on the clipboard either way — and a
/// decision whose sole effect is a log is a decision no test can see: measured, both
/// comparisons in [`Capture::poll`] could be inverted with the entire suite green, so
/// a capture that produced nothing would cheerfully report success. `wiring.rs` makes
/// the same move for registration ("the registries are otherwise write-only … this is
/// the read side of that").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// The pasteboard counter moved: something was copied.
    Copied,
    /// The counter did not move — cancelled, or refused.
    NothingCopied,
    /// The budget expired and the child was killed (D7).
    Killed,
    /// The spawn itself failed.
    NotSpawned,
}

/// How one capture ended, as the **watcher** can see it.
///
/// Deliberately says nothing about success. With `-c` there is no file to inspect and
/// the only evidence is the pasteboard, which only the UI thread may read — so the
/// watcher reports that the child is done and [`Capture::poll`] decides what that
/// meant. Every variant is published, because D7 requires the in-flight flag to clear
/// on cancel, timeout and spawn failure alike; a success-only value would leave the
/// chord disarmed for the rest of the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The child exited on its own. Whether anything reached the clipboard is still
    /// unknown here.
    Exited,
    /// The 5-minute budget expired and the child was killed (D7).
    TimedOut,
    /// The spawn itself failed — a missing or unexecutable binary.
    SpawnFailed,
}

/// The watcher thread's whole job: spawn, poll to the deadline, report.
///
/// **The deadline lives here, on this thread, and that is the half of the
/// `resolved_root_with` precedent that must NOT transfer.** That function spawns a
/// thread and then calls `rx.recv_timeout(RESOLVE_BUDGET)` *on the calling thread*
/// (`tabsession.rs:436`). Copied literally with [`BUDGET`] it would freeze the UI for
/// five minutes — and C6's continuous redraw exists because the compositor drops the
/// presented frame once the run loop sleeps (winit#2640), so that stall does not merely
/// lag the window, it blanks it.
fn run_capture(spawn: &Spawn, budget: Duration, tick: Duration) -> Outcome {
    let mut child = match spawn() {
        Ok(child) => child,
        Err(e) => {
            tracing::warn!("capture: could not spawn {SCREENCAPTURE}: {e}");
            return Outcome::SpawnFailed;
        }
    };
    let deadline = Instant::now() + budget;
    loop {
        match child.try_wait() {
            Ok(true) => return Outcome::Exited,
            // Supervision failed, which for a child we own means essentially never.
            // Kill it rather than poll a child we cannot read — an unkilled crosshair
            // with the flag cleared would let a second capture start under the first.
            Err(_) => {
                child.kill();
                return Outcome::Exited;
            }
            Ok(false) => {}
        }
        if Instant::now() >= deadline {
            child.kill();
            tracing::warn!(
                "capture: still running after {}s — child killed, chord re-armed (D7). \
                 Cause unknown: a selection left open, or a dialog behind another window.",
                budget.as_secs()
            );
            return Outcome::TimedOut;
        }
        std::thread::sleep(tick);
    }
}

/// One capture at a time, and the chord that re-arms after every outcome.
///
/// Owns the in-flight flag (D7), the channel the watcher publishes over, and the
/// pasteboard counter read at press time. The `Overlay` impl that puts it on the seam
/// is separate, so this half is drivable with no window, no proxy and no screen.
pub(crate) struct Capture {
    spawn: Spawn,
    wake: Wake,
    change_count: ChangeCount,
    budget: Duration,
    tick: Duration,
    /// **One at a time** (D7): `screencapture` owns the whole screen and a second
    /// crosshair is incoherent. Cleared on *every* terminal outcome in [`Self::poll`] —
    /// success, cancel, timeout, spawn failure — because a flag that leaks on one path
    /// disables the chord for the rest of the session.
    in_flight: bool,
    /// The pasteboard's counter when this capture started, and the entire basis for
    /// deciding whether it produced anything. See [`Self::poll`].
    count_at_start: isize,
    /// The most recent completed capture's [`Verdict`]. See its doc for why a value
    /// exists at all when only a log consumes it.
    last: Option<Verdict>,
    tx: Sender<Outcome>,
    rx: Receiver<Outcome>,
}

impl Capture {
    /// The production capture: the real binary, the real pasteboard.
    pub(crate) fn new(wake: Wake) -> Self {
        Self::with(
            Arc::new(real_spawn),
            wake,
            Arc::new(pasteboard_change_count),
            BUDGET,
            TICK,
        )
    }

    /// Every collaborator injected — what the tests drive.
    fn with(
        spawn: Spawn,
        wake: Wake,
        change_count: ChangeCount,
        budget: Duration,
        tick: Duration,
    ) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Capture {
            spawn,
            wake,
            change_count,
            budget,
            tick,
            in_flight: false,
            count_at_start: 0,
            last: None,
            tx,
            rx,
        }
    }

    /// Begin a capture; `true` if one was actually started.
    ///
    /// **Returns immediately** — the watcher thread owns the wait (D5/C4). The caller
    /// consumes the chord either way (D8): `false` here means "nothing started", never
    /// "let the key through", because falling through would type a literal `g`.
    ///
    /// Reads the pasteboard counter *before* spawning, on the UI thread, which is the
    /// only place it may be read.
    pub(crate) fn start(&mut self) -> bool {
        if self.in_flight {
            return false;
        }
        self.count_at_start = (self.change_count)();
        self.in_flight = true;
        let spawn = self.spawn.clone();
        let wake = self.wake.clone();
        let tx = self.tx.clone();
        let (budget, tick) = (self.budget, self.tick);
        std::thread::spawn(move || {
            let outcome = run_capture(&spawn, budget, tick);
            // The receiver is the overlay, which lives as long as the app; a send that
            // fails means the app is going away, which is not an error here.
            let _ = tx.send(outcome);
            // **Every** outcome wakes the loop, not just the successful one. Without
            // the wake on a cancel, the in-flight flag would clear only when some
            // unrelated watcher happened to tick — and on a quiet tab there may never
            // be one, leaving the chord dead.
            wake();
        });
        true
    }

    /// Fold a finished capture in (the [`crate::seam::Overlay::poll`] half).
    ///
    /// **The pasteboard counter is the verdict, and it is the counterpart of the old
    /// file-existence rule.** With `-c` nothing is written to disk, so there is no file
    /// to stat; what `screencapture` does on success is put an image on the pasteboard,
    /// and every pasteboard write bumps `changeCount`. A capture cancelled with Escape
    /// writes nothing and leaves the counter where it was.
    ///
    /// It is a *proxy*, and the ways it can lie are worth naming rather than
    /// discovering: anything else that copies during the drag — another app, a clipboard
    /// manager — also bumps the counter, so a cancelled capture can read as a successful
    /// one. The consequence is bounded: this verdict drives **nothing but a log line**,
    /// because the image is already wherever it is going. Nothing is typed, nothing is
    /// written, and a wrong answer misreports rather than misbehaves.
    ///
    /// Clearing the flag here rather than on the watcher thread is what makes the four
    /// outcomes one path instead of four: whatever ended the capture, it arrives as a
    /// value on this channel, and the flag clears once, where the value is read.
    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(outcome) = self.rx.try_recv() {
            self.in_flight = false;
            changed = true;
            let verdict = match outcome {
                Outcome::TimedOut => Verdict::Killed,
                Outcome::SpawnFailed => Verdict::NotSpawned,
                Outcome::Exited if (self.change_count)() == self.count_at_start => {
                    Verdict::NothingCopied
                }
                Outcome::Exited => Verdict::Copied,
            };
            match verdict {
                Verdict::Copied => {
                    tracing::info!("capture: image on the clipboard — press Cmd+V to use it")
                }
                Verdict::NothingCopied => tracing::warn!(
                    "capture: nothing reached the clipboard — cancelled (Esc), or refused \
                     by macOS. If a dialog appeared, this app needs Screen Recording \
                     (System Settings > Privacy & Security)."
                ),
                // Both already logged where they were decided, with detail this point
                // no longer has (the io error, the elapsed budget).
                Verdict::Killed | Verdict::NotSpawned => {}
            }
            self.last = Some(verdict);
        }
        changed
    }

    /// How the most recent completed capture was judged, or `None` before the first.
    ///
    /// `#[cfg(test)]`, like [`Self::in_flight`], and for the same honest reason: in
    /// production the verdict's only consumer is the log line above. This is the read
    /// side that makes the decision assertable — without it, both comparisons that
    /// produce it could be inverted with the suite green. Delete the attribute when a
    /// real caller appears; an `#[allow(dead_code)]` promising one indefinitely is what
    /// `lib.rs` removed twenty of.
    #[cfg(test)]
    pub(crate) fn last_verdict(&self) -> Option<Verdict> {
        self.last
    }

    /// Whether a crosshair is currently up.
    #[cfg(test)]
    fn in_flight(&self) -> bool {
        self.in_flight
    }
}

/// Is this `Cmd+Shift+G`? (D1)
///
/// **"G" for Grab**, the name of Apple's original screenshot utility — and free: the
/// case-folded letter chords taken across `crates/app/src/` are exactly `a c e m r s v`.
///
/// **Rejected — `Cmd+Shift+4`**: macOS takes `Cmd+Shift+3/4/5` before any application
/// sees the event. This repo already paid for that lesson — the ai-monitor section jumps
/// were `Cmd+Shift+<digit>` and 3-5 were unreachable, which is why they moved to
/// `Cmd+Opt+<digit>` (`input.rs:623-627`).
///
/// **Rejected — `Cmd+Shift+A`**: `rename.rs:220-222` matches `Cmd+A` case-folded
/// **without excluding shift**, and overlays dispatch before the default path, so with a
/// rename field open it would re-select the tab name instead of capturing.
///
/// Case-folded, like every other letter chord in the crate: with Shift held, the key
/// arrives as `G`.
fn is_capture_chord(input: &KeyInput) -> bool {
    if !(input.mods.super_key() && input.mods.shift_key()) {
        return false;
    }
    matches!(&input.key, Key::Character(s) if s.eq_ignore_ascii_case("g"))
}

/// The capture's seat on the SD4 overlay seam.
///
/// An **overlay**, not an `InputHandler`, and it stays one even though the clipboard
/// revision removed the delivery that originally forced it: `feature_handlers()` takes
/// no arguments (`lib.rs`) while `feature_overlays(proxy, arm)` is handed an
/// `EventLoopProxy`, and the watcher still needs to wake the loop so `poll` runs and the
/// in-flight flag clears. A handler could not do that.
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
    /// `0`, below the rename field's 200 — and **D1 silently depends on it**. D1's known
    /// modal limit (an open rename field swallows every `Cmd`/`Ctrl` chord,
    /// `rename.rs:276-278`) is only true while the capture sits below it.
    fn priority(&self) -> i32 {
        0
    }

    /// Never. Not an input-capturing surface: it consumes its own chord and nothing else.
    fn visible(&self) -> bool {
        false
    }

    fn on_key(&mut self, input: &KeyInput, _ctx: &mut AppCtx) -> Handled {
        if !is_capture_chord(input) {
            return Handled::Passthrough;
        }
        self.capture.start();
        // **`Consumed` on every path, including every failure path** (D8).
        // `Passthrough` does not mean "do nothing" for this chord: winit reports `text`
        // for Cmd chords, and `encode_key`'s terminal arm writes it to the shell — so
        // falling through would type a literal `g` at the prompt on every failed
        // capture, breaking D1 and C6.
        Handled::Consumed
    }

    /// The watcher's finished captures, folded in.
    fn poll(&mut self) -> bool {
        self.capture.poll()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    /// How long a helper waits for the watcher thread before giving up.
    ///
    /// Bounds a *failure*, never a success: the stub child exits within a poll or two of
    /// a 1 ms tick, so a passing test spends single-digit milliseconds here. Kept short
    /// deliberately — a mutant that stops `start` from ever spawning makes every helper
    /// below run to this limit at once, and a suite that HANGS reports worse than one
    /// that fails.
    const POLL_LIMIT: Duration = Duration::from_secs(3);

    #[test]
    fn the_binary_is_in_usr_sbin_not_usr_bin() {
        // `/usr/bin/screencapture` does not exist; naming it would make the feature
        // silently unspawnable.
        assert_eq!(SCREENCAPTURE, "/usr/sbin/screencapture");
    }

    #[test]
    fn the_argv_is_the_region_only_clipboard_form() {
        // Region-only (`-s`) and clipboard-bound (`-c`), in that order, and carrying no
        // path — with `-c` the destination is the pasteboard.
        assert_eq!(
            capture_args(),
            vec![
                OsString::from("-i"),
                OsString::from("-s"),
                OsString::from("-c"),
            ]
        );
    }

    #[test]
    fn the_argv_writes_no_file_and_asks_for_no_dialog() {
        // The two things the clipboard revision removed. `-d` went with the file (it
        // reported failures writing to a destination that no longer exists); any path
        // argument would reintroduce the disk write this revision deleted.
        let args = capture_args();
        assert!(!args.iter().any(|a| a == "-d"), "{args:?}");
        assert!(
            !args.iter().any(|a| a.to_string_lossy().contains('/')),
            "a path argument survived the clipboard revision: {args:?}"
        );
    }

    /// A stub `screencapture`: exits after `exits_after` polls, and records its kill.
    struct FakeChild {
        polls: u32,
        exits_after: Option<u32>,
        killed: Arc<AtomicBool>,
    }

    impl CaptureChild for FakeChild {
        fn try_wait(&mut self) -> std::io::Result<bool> {
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
        killed: Arc<AtomicBool>,
        wakes: Arc<AtomicU64>,
        /// The fake pasteboard counter. A capture "succeeds" by bumping it.
        count: Arc<AtomicU64>,
        /// Whether the stub bumps the counter when it runs.
        copies: Arc<AtomicBool>,
    }

    /// A [`Capture`] whose child exits promptly and optionally "copies" something.
    ///
    /// The whole point of the injected seams: `screencapture -i` waits for a human, so
    /// every outcome path below is reached with no human, no screen and no pasteboard.
    fn rig(copies: bool, exits_after: Option<u32>) -> (Capture, Rig) {
        let spawns = Arc::new(AtomicU64::new(0));
        let killed = Arc::new(AtomicBool::new(false));
        let wakes = Arc::new(AtomicU64::new(0));
        let count = Arc::new(AtomicU64::new(7));
        let copies = Arc::new(AtomicBool::new(copies));

        let (s, k, c, cp) = (
            spawns.clone(),
            killed.clone(),
            count.clone(),
            copies.clone(),
        );
        let spawn: Spawn = Arc::new(move || {
            s.fetch_add(1, Ordering::SeqCst);
            if cp.load(Ordering::SeqCst) {
                // What a real capture does on success: put something on the pasteboard.
                c.fetch_add(1, Ordering::SeqCst);
            }
            Ok(Box::new(FakeChild {
                polls: 0,
                exits_after,
                killed: k.clone(),
            }) as Box<dyn CaptureChild>)
        });

        let w = wakes.clone();
        let wake: Wake = Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        });

        let cc = count.clone();
        let change_count: ChangeCount = Arc::new(move || cc.load(Ordering::SeqCst) as isize);

        let capture = Capture::with(
            spawn,
            wake,
            change_count,
            Duration::from_secs(5),
            Duration::from_millis(1),
        );
        (
            capture,
            Rig {
                spawns,
                killed,
                wakes,
                count,
                copies,
            },
        )
    }

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

    // -----------------------------------------------------------------------
    // The verdict: the pasteboard counter replaces the file-existence rule.
    // -----------------------------------------------------------------------

    #[test]
    fn a_capture_that_copies_is_a_completed_capture() {
        let (mut capture, rig) = rig(true, Some(0));
        let before = rig.count.load(Ordering::SeqCst);
        assert!(capture.start());
        assert!(settle(&mut capture), "the watcher never published");
        assert!(
            rig.count.load(Ordering::SeqCst) > before,
            "the stub did not simulate a pasteboard write"
        );
        assert_eq!(
            capture.last_verdict(),
            Some(Verdict::Copied),
            "a capture that moved the counter must be judged Copied"
        );
        assert!(!capture.in_flight());
    }

    #[test]
    fn a_capture_that_copies_nothing_is_a_cancel() {
        // Escape writes nothing and leaves the counter where it was.
        let (mut capture, rig) = rig(false, Some(0));
        let before = rig.count.load(Ordering::SeqCst);
        assert!(capture.start());
        assert!(settle(&mut capture));
        assert_eq!(
            rig.count.load(Ordering::SeqCst),
            before,
            "a cancelled capture must not touch the pasteboard"
        );
        assert_eq!(
            capture.last_verdict(),
            Some(Verdict::NothingCopied),
            "an unmoved counter must be judged NothingCopied — inverting this \
             comparison would report every cancel as a success"
        );
        assert!(!capture.in_flight());
    }

    #[test]
    fn the_budget_and_spawn_failure_get_their_own_verdicts() {
        // The other two arms of the same match. Without these, `Outcome::TimedOut` and
        // `Outcome::SpawnFailed` could both be folded into the pasteboard comparison —
        // and a timed-out capture would be reported as "image on the clipboard"
        // whenever anything else had copied during the five minutes it was up.
        let (mut capture, _rig) = rig(true, None);
        capture.budget = Duration::from_millis(20);
        assert!(capture.start());
        assert!(settle(&mut capture));
        assert_eq!(
            capture.last_verdict(),
            Some(Verdict::Killed),
            "a killed capture must not be judged by the pasteboard"
        );

        let spawn: Spawn = Arc::new(|| Err(std::io::Error::from(std::io::ErrorKind::NotFound)));
        let mut capture = Capture::with(
            spawn,
            Arc::new(|| {}),
            // A counter that always moves: if the spawn-failure arm read it, this would
            // report Copied for a capture that never ran.
            Arc::new(|| 99),
            Duration::from_secs(5),
            Duration::from_millis(1),
        );
        assert!(capture.start());
        assert!(settle(&mut capture));
        assert_eq!(capture.last_verdict(), Some(Verdict::NotSpawned));
    }

    #[test]
    fn there_is_no_verdict_before_the_first_capture_completes() {
        // `None` is meaningfully different from a default: it is what distinguishes
        // "nothing has finished yet" from "the last one produced nothing".
        let (mut capture, _rig) = rig(true, None);
        assert_eq!(capture.last_verdict(), None);
        assert!(capture.start());
        assert_eq!(
            capture.last_verdict(),
            None,
            "a verdict appeared before the capture finished"
        );
    }

    #[test]
    fn the_production_pasteboard_read_reports_the_real_counter() {
        // The production read, which nothing else reaches: it can only be exercised
        // against the live pasteboard, and a constant would make every capture read as
        // successful (or every one as a cancel) with the suite green.
        //
        // Compared against an independent call to the same API rather than a literal,
        // because the real value is whatever this machine's session has reached — and
        // deliberately NOT by writing to the pasteboard, which would clobber whatever
        // the person running `cargo test` had copied.
        let direct = NSPasteboard::generalPasteboard().changeCount();
        assert_eq!(
            pasteboard_change_count(),
            direct,
            "the pasteboard read does not report the real counter"
        );
        assert_eq!(
            pasteboard_change_count(),
            pasteboard_change_count(),
            "two reads with no intervening copy must agree"
        );
    }

    #[test]
    fn the_overlay_poll_reports_what_the_capture_reports() {
        // `CaptureOverlay::poll` is a one-line delegation, and a one-line delegation is
        // exactly what gets replaced by a constant unnoticed: `true` would make the app
        // redraw on every unrelated tabmeta and ai-monitor wake, `false` would mean the
        // in-flight flag never visibly clears.
        let (capture, _rig) = rig(true, Some(0));
        let mut o = CaptureOverlay { capture };
        assert!(!o.poll(), "nothing has finished yet");

        assert_eq!(press(&mut o, &chord("g", CMD_SHIFT)).0, Handled::Consumed);
        let deadline = Instant::now() + POLL_LIMIT;
        let mut saw_true = false;
        while Instant::now() < deadline {
            if o.poll() {
                saw_true = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(saw_true, "poll never reported the finished capture");
        assert!(!o.poll(), "poll kept reporting a change after draining");
    }

    #[test]
    fn the_counter_is_read_before_the_capture_starts() {
        // The comparison is only meaningful against a value taken at press time. Read it
        // afterwards and every capture looks unchanged; never read it and a stale 0 makes
        // every capture look successful.
        let (mut capture, rig) = rig(true, Some(0));
        rig.count.store(41, Ordering::SeqCst);
        assert!(capture.start());
        assert_eq!(
            capture.count_at_start, 41,
            "start did not snapshot the pasteboard counter"
        );
    }

    #[test]
    fn a_second_capture_compares_against_its_own_start_value() {
        // The counter is monotonic across the process, so a stale `count_at_start` would
        // make every capture after the first read as successful.
        let (mut capture, rig) = rig(true, Some(0));
        assert!(capture.start());
        assert!(settle(&mut capture));
        let after_first = rig.count.load(Ordering::SeqCst);

        rig.copies.store(false, Ordering::SeqCst);
        assert!(capture.start());
        assert_eq!(
            capture.count_at_start, after_first as isize,
            "the second capture compared against the first one's snapshot"
        );
        assert!(settle(&mut capture));
        assert_eq!(rig.count.load(Ordering::SeqCst), after_first);
    }

    // -----------------------------------------------------------------------
    // The watcher, the budget and single-flight (D5 / D7 / C4).
    // -----------------------------------------------------------------------

    #[test]
    fn start_returns_while_the_capture_is_still_in_flight() {
        // C4 — `screencapture -i` blocks for as long as the human takes to drag a
        // rectangle, and blocking the UI thread does not merely lag the window, it BLANKS
        // it (winit#2640). The child here never exits.
        let (mut capture, rig) = rig(false, None);
        let t0 = Instant::now();
        assert!(capture.start());
        let elapsed = t0.elapsed();
        assert!(
            elapsed < Duration::from_millis(500),
            "start blocked for {elapsed:?} — the UI thread waited on the capture"
        );
        assert!(capture.in_flight());
        assert_eq!(wait_spawns(&rig.spawns, 1), 1);
    }

    #[test]
    fn a_second_chord_while_in_flight_starts_nothing() {
        let (mut capture, rig) = rig(false, None);
        assert!(capture.start());
        assert!(!capture.start(), "a second crosshair is incoherent");
        assert_eq!(wait_spawns(&rig.spawns, 1), 1);
    }

    #[test]
    fn the_chord_re_arms_after_a_completed_capture() {
        let (mut capture, rig) = rig(true, Some(0));
        assert!(capture.start());
        assert!(settle(&mut capture));
        assert!(!capture.in_flight(), "the flag leaked on the success path");
        assert!(capture.start());
        assert_eq!(wait_spawns(&rig.spawns, 2), 2);
    }

    #[test]
    fn the_chord_re_arms_after_a_cancelled_capture() {
        let (mut capture, rig) = rig(false, Some(0));
        assert!(capture.start());
        assert!(settle(&mut capture));
        assert!(!capture.in_flight(), "the flag leaked on the cancel path");
        assert!(capture.start());
        assert_eq!(wait_spawns(&rig.spawns, 2), 2);
    }

    #[test]
    fn the_budget_expires_kills_the_child_and_re_arms() {
        // D7's wedge recovery, at a budget a test can wait out. A child that never exits
        // — a permission dialog left open behind another window — must not disable the
        // chord until the terminal restarts.
        let (mut capture, rig) = rig(false, None);
        capture.budget = Duration::from_millis(20);
        assert!(capture.start());
        assert!(settle(&mut capture), "the deadline never fired");
        assert!(
            rig.killed.load(Ordering::SeqCst),
            "the budget expired without killing the child"
        );
        assert!(!capture.in_flight(), "the flag leaked on the timeout path");
        assert!(capture.start());
        assert_eq!(wait_spawns(&rig.spawns, 2), 2);
    }

    #[test]
    fn a_child_that_exits_inside_the_budget_is_never_killed() {
        // The kill is the timeout's, not every capture's. Killing a crosshair the user is
        // actively dragging would be exactly the wrong thing.
        let (mut capture, rig) = rig(true, Some(2));
        assert!(capture.start());
        assert!(settle(&mut capture));
        assert!(
            !rig.killed.load(Ordering::SeqCst),
            "a capture that finished on its own was killed anyway"
        );
    }

    #[test]
    fn the_chord_re_arms_after_a_spawn_failure() {
        // A missing or unexecutable `/usr/sbin/screencapture`.
        let spawns = Arc::new(AtomicU64::new(0));
        let s = spawns.clone();
        let spawn: Spawn = Arc::new(move || {
            s.fetch_add(1, Ordering::SeqCst);
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        let mut capture = Capture::with(
            spawn,
            Arc::new(|| {}),
            Arc::new(|| 0),
            Duration::from_secs(5),
            Duration::from_millis(1),
        );
        assert!(capture.start());
        assert!(settle(&mut capture));
        assert!(
            !capture.in_flight(),
            "the flag leaked on the spawn-failure path"
        );
        assert!(capture.start());
        assert_eq!(wait_spawns(&spawns, 2), 2);
    }

    #[test]
    fn every_outcome_wakes_the_loop() {
        // Not just the successful one. Without the wake on a cancel, the flag would clear
        // only when some unrelated watcher happened to tick — and on a quiet tab there
        // may never be one, leaving the chord dead with nothing to diagnose.
        for copies in [true, false] {
            let (mut capture, rig) = rig(copies, Some(0));
            assert!(capture.start());
            assert!(settle(&mut capture));
            assert_eq!(
                rig.wakes.load(Ordering::SeqCst),
                1,
                "copies={copies}: the watcher published without asking for a look"
            );
        }
    }

    #[test]
    fn poll_reports_nothing_when_no_capture_has_finished() {
        // This arm runs on every OverlayPending wake, and tabmeta and the ai-monitor
        // produce most of them. A `poll` that always claimed a change would add a redraw
        // to every one of those.
        let (mut capture, _rig) = rig(false, None);
        assert!(!capture.poll());
        assert!(capture.start());
        assert!(!capture.poll(), "the capture has not finished yet");
    }

    #[test]
    fn the_production_budget_and_tick_are_the_decided_values() {
        // D7's five minutes, pinned. **`cargo mutants` cannot see this**: it generates no
        // mutant for a `const`, so `BUDGET` could be edited to 3 seconds and both Gate 1
        // and the mutation gate would stay fully green while every real capture died
        // mid-drag. Measured: `cargo mutants --list -f capture.rs` names no mutant for
        // `BUDGET`, `TICK` or `Capture::new`. (No absolute total is written here — a
        // file's mutant count moves with every edit to it.)
        assert_eq!(BUDGET, Duration::from_secs(300), "D7's budget changed");
        assert_eq!(TICK, Duration::from_millis(50));
        assert!(
            TICK < BUDGET,
            "a tick at or beyond the budget makes the deadline unreachable"
        );
    }

    #[test]
    fn the_production_capture_is_built_idle_with_the_decided_budget() {
        // The other half of the same blind spot: `Capture::new` is the only place the two
        // constants are wired in, and passing them in the wrong order type-checks — both
        // are `Duration`. That would give a 50 ms budget and a 300 s tick: every capture
        // killed before the crosshair was drawn.
        let capture = Capture::new(Arc::new(|| {}));
        assert!(!capture.in_flight(), "a fresh capture starts disarmed");
        assert_eq!(capture.budget, BUDGET, "budget and tick are swapped");
        assert_eq!(capture.tick, TICK, "budget and tick are swapped");
    }

    /// A real child process that is not `screencapture`: it exits on its own, and can be
    /// killed before it does.
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
        // while it was nested inside `real_spawn`. Both answers matter — a constant `true`
        // judges every capture the instant it starts, and a constant `false` never
        // finishes one, so every capture runs to the budget and is killed.
        let mut child = sleeper("0.3");
        assert!(!child.try_wait().expect("try_wait"));
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
        // `screencapture` owning the screen with its crosshair up while the flag clears
        // and the chord re-arms — two crosshairs, which is what D7 exists to prevent.
        let mut child = sleeper("30");
        assert!(!child.try_wait().expect("try_wait"));
        child.kill();
        assert!(
            child.try_wait().expect("try_wait"),
            "the child outlived its kill"
        );
    }

    // -----------------------------------------------------------------------
    // The chord and the overlay (D1 / D8).
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

    fn overlay_rig() -> (CaptureOverlay, Rig) {
        let (capture, rig) = rig(false, None);
        (CaptureOverlay { capture }, rig)
    }

    /// Dispatch one key at the overlay, recording what reached the PTY.
    fn press(overlay: &mut CaptureOverlay, input: &KeyInput) -> (Handled, Vec<u8>) {
        let mut buf: Vec<u8> = Vec::new();
        let handled = {
            let mut ctx = AppCtx::new(&mut buf, 1, 0, None);
            overlay.on_key(input, &mut ctx)
        };
        (handled, buf)
    }

    #[test]
    fn cmd_shift_g_starts_a_capture_and_is_consumed() {
        let (mut o, rig) = overlay_rig();
        let (handled, wrote) = press(&mut o, &chord("g", CMD_SHIFT));
        assert_eq!(handled, Handled::Consumed);
        assert_eq!(wait_spawns(&rig.spawns, 1), 1);
        assert!(wrote.is_empty(), "the chord itself writes nothing");
    }

    #[test]
    fn the_chord_is_matched_case_folded() {
        // With Shift held the key arrives as `G`, which is the ONLY way it can arrive. A
        // match on lowercase `g` alone would make the chord unreachable.
        let (mut o, rig) = overlay_rig();
        assert_eq!(press(&mut o, &chord("G", CMD_SHIFT)).0, Handled::Consumed);
        assert_eq!(wait_spawns(&rig.spawns, 1), 1);
    }

    #[test]
    fn a_bare_g_is_not_the_chord() {
        // Typing `g` at the prompt must reach the shell.
        let (mut o, rig) = overlay_rig();
        let (handled, _) = press(
            &mut o,
            &chord("g", winit::keyboard::ModifiersState::empty()),
        );
        assert_eq!(handled, Handled::Passthrough);
        assert_eq!(rig.spawns.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn cmd_g_without_shift_is_not_the_chord() {
        let (mut o, rig) = overlay_rig();
        assert_eq!(press(&mut o, &chord("g", CMD)).0, Handled::Passthrough);
        assert_eq!(rig.spawns.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_chord_collides_with_no_existing_chord() {
        // `Cmd+Shift+M` (ai-monitor), `Cmd+Shift+R` (rename), `Cmd+Shift+S` (sidebar),
        // `Cmd+C`/`Cmd+V`, `Cmd+T`/`Cmd+W`, `Cmd+<digit>`, and rename's modal `Cmd+A`.
        let (mut o, rig) = overlay_rig();
        for (ch, mods) in [
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
        ] {
            let (handled, wrote) = press(&mut o, &chord(ch, mods));
            assert_eq!(
                handled,
                Handled::Passthrough,
                "the capture overlay swallowed {ch:?} — another feature's chord"
            );
            assert!(wrote.is_empty());
        }
        assert_eq!(
            rig.spawns.load(Ordering::SeqCst),
            0,
            "another feature's chord started a screen capture"
        );
    }

    #[test]
    fn a_second_chord_while_in_flight_is_still_consumed() {
        // D8's other half. If the in-flight case fell through, a user pressing the chord
        // twice would type a literal `g` at their agent's prompt.
        let (mut o, rig) = overlay_rig();
        assert_eq!(press(&mut o, &chord("g", CMD_SHIFT)).0, Handled::Consumed);
        assert_eq!(wait_spawns(&rig.spawns, 1), 1);

        let (handled, wrote) = press(&mut o, &chord("g", CMD_SHIFT));
        assert_eq!(handled, Handled::Consumed, "the second chord fell through");
        assert!(wrote.is_empty(), "the second chord typed a literal g");
        assert_eq!(rig.spawns.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_overlay_starts_invisible_at_priority_zero() {
        // It draws nothing and owns no band, so it must never be "visible"; and priority
        // 0 keeps it BELOW the rename field, which is what makes D1's known modal limit
        // true.
        let (o, _) = overlay_rig();
        assert!(!o.visible());
        assert_eq!(o.priority(), 0);
    }

    #[test]
    fn the_registration_list_names_the_capture_overlay() {
        // **Identity, which every shape assertion misses.** `wiring.rs` pins the set's
        // length, priorities, layer count, order and initial invisibility — and all of
        // those survive replacing this overlay with a second `ImagePresenter`, because
        // that substitution changes no count and no priority. Measured: with the swap in
        // place the whole wiring suite reported 7 passed / 0 failed while `Cmd+Shift+G`
        // reached nothing.
        //
        // Structural rather than behavioural because the registered overlay carries the
        // **production** spawn: dispatching the real chord at the real set would put a
        // `screencapture` crosshair on the screen of whoever ran `cargo test`.
        //
        // Comments are stripped before the match — an earlier version of this guard
        // searched raw source, and a `// TODO: restore CaptureOverlay::new(..)` comment
        // satisfied it while the overlay was gone.
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
    fn priority_zero_still_receives_the_chord() {
        // Registered is not the same as reachable: an overlay behind one that eats its
        // keys is as dead as one never registered. `rename.rs:276-278` consumes every
        // `Cmd`/`Ctrl` chord while its field is open, and D1's "known modal limit" is only
        // acceptable because that field is closed by default.
        //
        // The real four, in the real order — `lib.rs` pushes capture LAST among the three
        // priority-0 overlays, and `OverlayHost::register` sorts with `sort_by_key`, which
        // is stable, so ties keep push order.
        let (capture, rig) = rig(false, None);
        let mut host = crate::seam::OverlayHost::default();
        host.register(Box::new(crate::rename::RenameOverlay::new()));
        host.register(Box::new(crate::overlay::AiMonitorOverlay::with_waker(
            None,
            crate::overlay::no_deadline(),
        )));
        host.register(Box::new(crate::clipboard::ImagePresenter));
        host.register(Box::new(crate::copy::SelectionPresenter));
        host.register(Box::new(CaptureOverlay { capture }));
        assert_eq!(
            host.priorities(),
            vec![200, 100, 0, 0, 0],
            "the rehearsed set no longer matches the registered one"
        );

        let mut buf: Vec<u8> = Vec::new();
        let handled = {
            let mut ctx = AppCtx::new(&mut buf, 1, 0, None);
            host.dispatch(&chord("g", CMD_SHIFT), &mut ctx)
        };
        assert_eq!(
            handled,
            Handled::Consumed,
            "the chord crossed the whole registered set and fell through to the shell"
        );
        assert!(buf.is_empty(), "the chord itself put bytes on the PTY");
        assert_eq!(
            wait_spawns(&rig.spawns, 1),
            1,
            "the chord was consumed by something ABOVE capture — registered, but \
             unreachable, which is as dead as never registered at all"
        );
    }
}
