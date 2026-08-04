#!/bin/bash
# Gate 1: the 5 protected resolve_session fallback tests must not change MEANING.
# The `&[]` -> `&[], true` arity retype is permitted (spec Done-when #4); anything else is not.
set -u
fns="a_named_session_wins_outright
a_session_started_by_this_tab_beats_a_more_active_foreign_one
two_tabs_on_one_project_get_their_own_sessions
an_empty_hint_degrades_to_ranking_and_claims_nothing
nothing_confirmable_resolves_to_none_rather_than_a_foreign_log"
rc=0
while read -r f; do
  [ -z "$f" ] && continue
  git show HEAD:crates/ai-monitor/src/lib.rs | awk "/fn $f\(\)/,/^    }\$/" \
    | sed -E 's/&\[\]\)/\&[], true)/g; s/&\[\], true, true\)/\&[], true)/g' > /tmp/pb_a.txt
  awk "/fn $f\(\)/,/^    }\$/" crates/ai-monitor/src/lib.rs > /tmp/pb_b.txt
  if ! diff -q /tmp/pb_a.txt /tmp/pb_b.txt >/dev/null; then
    echo "CHANGED BEYOND RETYPE: $f"; diff /tmp/pb_a.txt /tmp/pb_b.txt | head -8; rc=1
  fi
done <<< "$fns"
[ $rc -eq 0 ] && echo "all 5 protected bodies: unchanged modulo the permitted arity retype"
exit $rc
