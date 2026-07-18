#!/usr/bin/env python3
"""Perf gate comparison step (D5/C4/R3.4).

Reads the committed thresholds from docs/performance.md's machine-readable JSON
block (the single source of truth), reads each Criterion group's measured median
from target/criterion/<group>/new/estimates.json, and fails if any *gated* group
exceeds `threshold_ns * (1 + tolerance)`.

While docs/performance.md has `"confirmed": false`, the script reports but never
fails the job (advisory) — the numbers are not human-confirmed yet (gap G1).
"""

import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
DOCS = REPO / "docs" / "performance.md"
CRITERION = REPO / "target" / "criterion"


def load_thresholds() -> dict:
    text = DOCS.read_text(encoding="utf-8")
    m = re.search(r"```json\s*(\{.*?\})\s*```", text, re.DOTALL)
    if not m:
        sys.exit("perf_compare: no ```json thresholds block in docs/performance.md")
    return json.loads(m.group(1))


def measured_median_ns(group: str) -> float | None:
    est = CRITERION / group / "new" / "estimates.json"
    if not est.exists():
        est = CRITERION / group / "base" / "estimates.json"
    if not est.exists():
        return None
    data = json.loads(est.read_text(encoding="utf-8"))
    # Criterion: estimates.json -> {"median": {"point_estimate": <ns>}, ...}
    return float(data["median"]["point_estimate"])


def main() -> int:
    cfg = load_thresholds()
    confirmed = bool(cfg.get("confirmed", False))
    tol = float(cfg.get("tolerance", 0.0))
    failures = []

    for group, spec in cfg["groups"].items():
        measured = measured_median_ns(group)
        threshold = spec.get("threshold_ns")
        gated = bool(spec.get("gated", False))
        ceiling = None if threshold is None else threshold * (1.0 + tol)

        if measured is None:
            print(f"• {group:14s} measured=MISSING  gated={gated}  (bench did not run)")
            if gated:
                failures.append(f"{group}: no measurement produced")
            continue

        verdict = "OK"
        if ceiling is not None and measured > ceiling:
            verdict = "OVER"
            if gated:
                failures.append(
                    f"{group}: {measured:.0f}ns > {ceiling:.0f}ns "
                    f"(threshold {threshold} + {tol:.0%})"
                )
        print(
            f"• {group:14s} measured={measured:9.0f}ns  "
            f"ceiling={('n/a' if ceiling is None else f'{ceiling:.0f}ns')}  "
            f"gated={gated}  {verdict}"
        )

    if not confirmed:
        print("\nperf_compare: docs/performance.md is PROVISIONAL "
              "(confirmed=false) — advisory only, not failing the job.")
        return 0

    if failures:
        print("\nperf_compare: FAIL")
        for f in failures:
            print(f"  - {f}")
        return 1

    print("\nperf_compare: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
