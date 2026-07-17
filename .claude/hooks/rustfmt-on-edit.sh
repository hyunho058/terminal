#!/usr/bin/env bash
# PostToolUse hook: format an edited Rust file with rustfmt.
# Receives the tool-call JSON on stdin; formats only *.rs paths. Never fails the
# tool call (best-effort formatting).
set -euo pipefail

input="$(cat)"
file="$(printf '%s' "$input" | jq -r '.tool_input.file_path // empty')"

case "$file" in
  *.rs)
    if command -v rustfmt >/dev/null 2>&1 && [ -f "$file" ]; then
      rustfmt "$file" 2>/dev/null || true
    fi
    ;;
esac
exit 0
