#!/bin/sh
# R3.3 / D11: the fixed-width string-assembly mechanism must be gone from dashboard.rs.
#
# Scans the PRODUCTION half only. A naive whole-file grep matches the very test that
# asserts these names are absent (dashboard.rs's `r3_3` lists them as string literals),
# so it reports a failure whenever the requirement is *proved* — the check must cut at
# the test module, and fail loudly if that boundary is missing rather than scanning nothing.
set -e
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
F="$ROOT/crates/app/src/dashboard.rs"
test -f "$F" || { echo "FAIL R3.3: $F not found"; exit 1; }
grep -q '^#\[cfg(test)\]' "$F" || { echo "FAIL R3.3: no test-module boundary in $F"; exit 1; }
PROD=$(sed '/^#\[cfg(test)\]/,$d' "$F" | sed 's://.*::')
HITS=$(printf '%s\n' "$PROD" | grep -nE '\b(RULE_COLS|fn fit_row|fn rule|fn rule_with|fn rule_empty|fn compose)\b' || true)
if [ -n "$HITS" ]; then
  echo "FAIL R3.3: the width-blind mechanism is still live in dashboard.rs:"
  printf '%s\n' "$HITS"
  exit 1
fi
echo "OK R3.3: RULE_COLS / fit_row / rule* / compose are absent from production code."
