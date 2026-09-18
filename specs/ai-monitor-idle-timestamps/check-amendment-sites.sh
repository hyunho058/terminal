#!/bin/sh
# R7.1 / R7.2 / R7.3 / R8.2 — the reversal is written down at its sites, and the cost comment
# states the weaker, true claim.
#
# These four requirements have no runtime behaviour to assert, so without a check they would be
# "verified by having been typed". Each grep below targets a phrase that carries the REQUIRED
# MEANING, not a heading, so a site gutted back to its old wording fails here.
set -eu
fail=0
check() { # <label> <file> <pattern>
  if grep -q "$3" "$2"; then
    printf '  PASS  %s\n' "$1"
  else
    printf '  FAIL  %s (%s)\n' "$1" "$2"
    fail=1
  fi
}

echo "amendment sites:"
check "R7.1 RP-D2 draws cadence vs bounded edge" \
  specs/ai-monitor-right-panel/spec.md "AMENDED by .specs/ai-monitor-idle-timestamps"
check "R7.2 publish_if_changed: a wake no longer implies a change" \
  crates/app/src/tabmeta/worker.rs "no longer the only .wake(). site in this file"
check "R7.3 R7.4's part-2 comment no longer claims two triggers" \
  crates/app/src/terminal_app.rs "CORRECTED (ai-monitor-idle-timestamps D2/D16)"
check "R8.2 the cost comment says 'no new wakes', not 'free'" \
  crates/app/src/terminal_app.rs "no new WAKES, which is not the same as"

# NO negative word-absence check here, deliberately.
#
# The obvious companion — "fail if the file still says 'free refresh'" — was written, and it
# failed on the comment that DISAVOWS the phrase: "An earlier framing called this a free
# refresh; it is not." That is the third time in this one verification run that an absence grep
# matched the prose documenting the absence rather than a real violation (the others were the
# `cfg(target_os)` scan and RP-D2's citation check). The pattern is the point: over prose, a
# word-presence check cannot distinguish a claim from its retraction, so it is the wrong
# instrument and tuning the regex until it agrees is how a gate ends up protecting nothing.
# The positive check above carries the requirement's actual meaning; see lesson L4.

exit $fail
