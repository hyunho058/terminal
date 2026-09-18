#!/bin/sh
# R7.1 — RP-D2's G1 citation must point at the LIVE guarantee, not the deleted file.
#
# Scans D2's *Rationale bullet only*, not the whole spec and not the whole D2 section.
# Two earlier versions of this check failed on prose rather than substance: `overlay_layer.rs`
# legitimately appears elsewhere in that spec as history, and the dead citation is quoted
# inside the very amendment note that records it as dead. An absence check must scan only the
# region the requirement is about (this contract's lesson L3).
set -eu
SPEC=specs/ai-monitor-right-panel/spec.md
test -f "$SPEC" || { echo "FAIL: $SPEC missing"; exit 1; }

# D2's rationale: from the '- **Rationale**' line inside '### D2' up to the next top-level bullet.
RATIONALE=$(sed -n '/^### D2: No animation/,/^### D3:/p' "$SPEC" \
  | sed -n '/^- \*\*Rationale\*\*/,/^- \*\*[A-Z]/p' \
  | sed '$d')

test -n "$RATIONALE" || { echo "FAIL: could not isolate D2's rationale — the spec's shape changed"; exit 1; }

echo "$RATIONALE" | grep -q 'monitor_panel\.rs:992' \
  || { echo "FAIL: D2's rationale does not cite the live G1 guarantee (monitor_panel.rs:992)"; exit 1; }
echo "$RATIONALE" | grep -q 'overlay_layer\.rs' \
  && { echo "FAIL: D2's rationale still cites the deleted overlay_layer.rs"; exit 1; }

echo "PASS: RP-D2 cites the live G1 guarantee"
