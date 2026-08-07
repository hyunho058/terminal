#!/bin/sh
# R0.4 guard: the ai-monitor-right-panel feature must not touch `crates/term-core/`.
#
# Non-destructive reflow is a dependency (D22), specified separately at
# `specs/grid-reflow/spec.md`. Every term-core file — grid.rs, scrollback.rs,
# snapshot.rs, links.rs — belongs to that spec, not this one (Declared Surface).
#
# Exits 0 when clean, 1 when any staged or unstaged change lands under term-core.

set -eu

# Resolve the repo from the script's own location, so it runs from any cwd.
repo_root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)

hits=$(
  {
    git -C "$repo_root" diff --name-only
    git -C "$repo_root" diff --cached --name-only
  } | grep '^crates/term-core/' | sort -u
) || true

if [ -n "$hits" ]; then
  echo "FAIL R0.4: this feature must not change term-core." >&2
  echo "These files belong to specs/grid-reflow/spec.md (D22), not to" >&2
  echo "specs/ai-monitor-right-panel/spec.md:" >&2
  echo "$hits" | sed 's/^/  /' >&2
  exit 1
fi

echo "OK R0.4: no crates/term-core/ path in the working tree or index."
