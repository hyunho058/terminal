#!/bin/bash
# L1: the rival handle the watcher reads must be the one the overlay stores.
# The orphaned-handle defect passed 1025 tests, clippy and fmt; only a checker on the
# diff found it. This is the deterministic form of that check.
set -u
f=crates/app/src/overlay.rs
rc=0
grep -q 'publish_rivals_into(&mut self\.rivals, &hint)' "$f" \
  || { echo "FAIL: open() does not publish into self.rivals"; rc=1; }
grep -q 'watch_with_rivals(rivals,' "$f" \
  || { echo "FAIL: the Watch is not built from the published handle"; rc=1; }
grep -q 'let rivals = publish_rivals_into' "$f" \
  || { echo "FAIL: publish_rivals_into's return value is discarded (M8)"; rc=1; }
[ $rc -eq 0 ] && echo "rivals wiring: open() -> self.rivals -> Watch is intact"
exit $rc
