#!/usr/bin/env bash
# Measure what a VISIBLE IDLE window costs, in CPU.
#
# Why this is a script and not a test. `cargo test` cannot host a winit event loop on
# macOS (the EventLoop is main-thread only), so there is no way to assert this number
# from the suite — the same constraint `r7_4_the_loop_still_waits_and_the_panel_schedules_no_wake`
# reasons about. The number is also machine-dependent: the frame rate tracks the display,
# so a 120 Hz panel roughly doubles it, and a loaded machine moves it around. A threshold
# in CI would either flake or be too loose to catch anything.
#
# What IS guarded automatically is the *shape* that sets the rate:
# `the_idle_frame_rate_stays_one_redraw_per_frame_and_stops_when_occluded` in
# `crates/app/src/terminal_app.rs` fails if a second redraw request appears in the arm
# (which doubles the rate) or if the `!occluded` gate is lost (which makes hidden windows
# cost what visible ones do). This script is how you check the number those guard.
#
# Baseline measured 2026-09-17, Apple M1, 60 Hz, release build, empty window, no panel:
#
#     release  ~7% of one core
#     debug   ~19% of one core
#
# The debug premium is `objc2` verifying Objective-C method signatures on every message
# send, which at 60fps is a great many sends. Compare like with like.
#
# The cost is NOT a defect: the continuous redraw exists because winit#2640 means the
# compositor drops the presented frame as soon as the run loop sleeps, so an idle window
# would otherwise go blank, and because the renderer's atlas-eviction invalidation relies
# on it to repaint. See `docs/architecture.md` C6. Occluded windows are free.
#
# Usage:
#   scripts/measure-idle-cpu.sh [debug|release]   launch a window and measure it
#   scripts/measure-idle-cpu.sh --pid N [debug|release]
#                                                measure an ALREADY-RUNNING window
#
# The `--pid` form exists because of a constraint worth stating: nothing here can bring a
# window to the front. A freshly launched window lands behind whatever had focus, macOS
# occludes it, the redraw loop pauses by design, and the measurement is of the wrong path.
# There is no scriptable fix without Accessibility permission, so the honest division of
# labour is that a human puts the window in front and points this at its pid.
set -euo pipefail

EXISTING_PID=""
if [ "${1:-}" = "--pid" ]; then
  EXISTING_PID="${2:-}"
  [ -n "$EXISTING_PID" ] || { echo "usage: $0 --pid N [debug|release]" >&2; exit 2; }
  kill -0 "$EXISTING_PID" 2>/dev/null || { echo "no process $EXISTING_PID" >&2; exit 1; }
  shift 2
fi

PROFILE="${1:-release}"
case "$PROFILE" in
  release) BIN=target/release/app ;;
  debug)   BIN=target/debug/app ;;
  *) echo "usage: $0 [--pid N] [debug|release]" >&2; exit 2 ;;
esac
if [ -z "$EXISTING_PID" ]; then
  case "$PROFILE" in
    release) cargo build -p app --release >/dev/null ;;
    debug)   cargo build -p app >/dev/null ;;
  esac
fi

[ -n "$EXISTING_PID" ] || [ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }

# A GUI session is required: the window must be REAL and VISIBLE, because an occluded one
# is gated off by design and would measure ~0 and look like a pass.
if ! /usr/bin/pgrep -q WindowServer; then
  echo "no WindowServer — this needs a logged-in GUI session" >&2
  exit 1
fi

if [ -n "$EXISTING_PID" ]; then
  PID="$EXISTING_PID"
  echo "measuring the already-running window, pid $PID ($PROFILE baselines)"
else
  echo "launching $BIN ..."
  RUST_LOG=warn "./$BIN" >/tmp/idle-cpu-probe.log 2>&1 &
  PID=$!
  trap 'kill "$PID" 2>/dev/null || true' EXIT
  sleep 5   # settle: first frames, GPU init, font atlas warm-up
fi

cpu_seconds() { ps -o time= -p "$1" | tr -d ' '; }
to_s() { awk -F: '{n=NF; s=$n; if(n>1) s+=60*$(n-1); if(n>2) s+=3600*$(n-2); print s}' <<<"$1"; }

# The window must be VISIBLE and UNCOVERED. This matters because the first version of
# this script measured a window that had opened behind the terminal running it, read
# 0.1%, and declared it "in line with the recorded baseline" — a measurement of the
# occluded path looks exactly like a pass, which is the one way this script can lie.
#
# `lsappinfo` is a HINT and nothing more, because frontmost is not the same as visible:
# a window can be fully unoccluded while another app holds focus, and that case measures
# correctly. Treat this as "check the window with your eyes", not as a verdict. The
# authoritative detector is the implausibly-low guard at the end, which keys on the
# thing that actually distinguishes the two paths — the cost itself.
if ! lsappinfo info "$PID" 2>/dev/null | grep -q "in front"; then
  echo
  echo "note: this window is not frontmost. That is fine IF it is still fully visible;" >&2
  echo "      only a genuinely covered window pauses the redraw loop. If the number" >&2
  echo "      below comes out near zero, that is what happened — uncover it and re-run." >&2
  echo
fi

echo "measuring 3 x 20s with the window idle — do not type into it, do not cover it"
TOTAL=0
for i in 1 2 3; do
  A=$(to_s "$(cpu_seconds "$PID")")
  sleep 20
  B=$(to_s "$(cpu_seconds "$PID")")
  PCT=$(awk -v a="$A" -v b="$B" 'BEGIN{printf "%.1f", (b-a)/20*100}')
  TOTAL=$(awk -v t="$TOTAL" -v p="$PCT" 'BEGIN{print t+p}')
  printf "  run %d: %5s%% of one core\n" "$i" "$PCT"
done

awk -v t="$TOTAL" -v p="$PROFILE" 'BEGIN{
  m=t/3
  printf "\nmean: %.1f%% of one core (%s)\n", m, p
  base = (p=="release") ? 7 : 19
  printf "baseline for %s: ~%d%%\n", p, base
  # A LOW reading is not a pass. The redraw loop is gated on `!occluded`, so a covered
  # window costs ~0 by design — which is indistinguishable from "the cost went away"
  # unless we say so. Treat anything near zero as an invalid measurement, not a win.
  if (m < 1) {
    printf "\nIMPLAUSIBLY LOW — this almost certainly measured an OCCLUDED window,\n"
    printf "which pauses the redraw loop by design. This is not a pass. Bring the\n"
    printf "window to the front, make sure nothing covers it, and re-run.\n"
    exit 1
  }
  if (m > base*1.5) {
    printf "\nWELL ABOVE BASELINE. Check the frame rate first: instrument\n"
    printf "`new_events`/`window_event` and look for more than ~60 RedrawRequested per second.\n"
    exit 1
  }
  print "\nin line with the recorded baseline."
}'
