#!/usr/bin/env python3
"""Autopilot enter-check: re-derive the six limits from the DURABLE counters.

The governor's safety mechanism. Hand-rolled shell greps got this wrong three times
(an uncompared wall-clock, a comment-number captured as max_ticks, and a timestamp
truncated at its first colon -> false HALT), so the logic lives here, once, tested.

Usage:  autopilot-check.py <runDir>
Exit:   0 = proceed with the tick   1 = HALT (reason on stdout)   2 = ledger unreadable
Prints a line per limit so a tick's transcript shows what was actually compared.
"""
import os
import re
import sys
from datetime import datetime, timezone

TS = "%Y-%m-%dT%H:%M:%SZ"


def parse(path):
    """Parse `- key: value  # comment` lines. Splits on the FIRST ': ' only, so an
    ISO timestamp's inner colons survive; strips a trailing comment."""
    out = {}
    for raw in open(path):
        line = raw.strip()
        if not line.startswith("- "):
            continue
        body = line[2:]
        if ": " not in body and not body.endswith(":"):
            continue
        key, _, val = body.partition(": ")
        val = val.split("#", 1)[0].strip()
        out[key.strip()] = val
    return out


def num(d, key, default=None):
    v = d.get(key, "")
    m = re.match(r"-?\d+", v)
    if m:
        return int(m.group())
    if default is not None:
        return default
    raise KeyError(f"{key} missing or non-numeric: {v!r}")


def main():
    run_dir = sys.argv[1] if len(sys.argv) > 1 else "."
    ledger = os.path.join(run_dir, "autopilot.md")
    try:
        d = parse(ledger)
    except OSError as e:
        print(f"LEDGER UNREADABLE: {e}")
        return 2

    now = datetime.now(timezone.utc)
    halt = []

    # kill-switch
    ks = d.get("killswitch_path", os.path.join(run_dir, "STOP"))
    if os.path.exists(ks):
        print(f"  kill-switch      PRESENT ({ks})            -> HALT")
        halt.append("kill-switch file present")
    else:
        print("  kill-switch      absent                          OK")

    # wall-clock
    cap_h = num(d, "wall_clock_cap_hours", 12)
    started = d.get("run_started_at", "")
    try:
        start = datetime.strptime(started, TS).replace(tzinfo=timezone.utc)
        elapsed_h = (now - start).total_seconds() / 3600.0
        if elapsed_h > cap_h:
            print(f"  wall-clock       {elapsed_h:.2f}h > {cap_h}h                   -> HALT")
            halt.append(f"wall-clock {elapsed_h:.2f}h > {cap_h}h")
        else:
            print(f"  wall-clock       {elapsed_h:.2f}h <= {cap_h}h                  OK")
    except ValueError:
        print(f"  wall-clock       UNPARSEABLE run_started_at={started!r} -> HALT")
        halt.append("run_started_at unparseable (fail safe)")

    # max-tick (cost cap)
    ticks, max_t = num(d, "tick_count", 0), num(d, "max_ticks", 200)
    if ticks > max_t:
        print(f"  max-tick         {ticks} > {max_t}                          -> HALT")
        halt.append(f"tick_count {ticks} > {max_t}")
    else:
        print(f"  max-tick         {ticks} <= {max_t}                         OK")

    # consecutive-park
    parks, pcap = num(d, "consecutive_parks", 0), num(d, "consecutive_park_cap", 3)
    if parks >= pcap:
        print(f"  consecutive-park {parks} >= {pcap}                          -> HALT")
        halt.append(f"consecutive_parks {parks} >= {pcap}")
    else:
        print(f"  consecutive-park {parks} < {pcap}                           OK")

    # crash-loop (M in N minutes)
    cl = d.get("crash_loop", "")
    cmax = int(re.search(r"max:\s*(\d+)", cl).group(1)) if re.search(r"max:\s*(\d+)", cl) else 3
    win = int(re.search(r"window_minutes:\s*(\d+)", cl).group(1)) if re.search(r"window_minutes:\s*(\d+)", cl) else 10
    raw = d.get("recent_crashes", "[]")
    stamps = re.findall(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", raw)
    recent = 0
    for s in stamps:
        try:
            t = datetime.strptime(s, TS).replace(tzinfo=timezone.utc)
            if (now - t).total_seconds() / 60.0 <= win:
                recent += 1
        except ValueError:
            pass
    if recent >= cmax:
        print(f"  crash-loop       {recent} >= {cmax} in {win}min                 -> HALT")
        halt.append(f"crash-loop {recent} in {win}min")
    else:
        print(f"  crash-loop       {recent} < {cmax} in {win}min                  OK")

    # per-tick-timeout: a prior tick's lease stale past the timeout => that tick crashed.
    # Reported, not a direct halt (it is counted as a crash by the reclaiming tick).
    tmo = num(d, "per_tick_timeout_seconds", 900)
    lease, done = d.get("lease_renewed_at", "—"), d.get("last_tick_completed_at", "—")
    started_at = d.get("last_tick_started_at", "—")
    if lease not in ("—", "") and started_at != "—" and done in ("—", "") :
        try:
            age = (now - datetime.strptime(lease, TS).replace(tzinfo=timezone.utc)).total_seconds()
            state = "STALE -> prior tick crashed" if age > tmo else "fresh -> tick in flight"
            print(f"  per-tick-timeout lease {age:.0f}s vs {tmo}s: {state}")
        except ValueError:
            print("  per-tick-timeout lease unparseable")
    else:
        print(f"  per-tick-timeout no in-flight lease                OK")

    if halt:
        print("HALT: " + "; ".join(halt))
        return 1
    print("PROCEED")
    return 0


if __name__ == "__main__":
    sys.exit(main())
