#!/usr/bin/env bash
# PreToolUse hook: block direct edits to Cargo.lock.
# Exit 2 denies the tool call; the message on stderr tells the agent why.
set -euo pipefail

input="$(cat)"
file="$(printf '%s' "$input" | jq -r '.tool_input.file_path // empty')"

if [ "$(basename "$file")" = "Cargo.lock" ]; then
  echo "Refusing to edit Cargo.lock directly. Use 'cargo add', 'cargo update', or 'cargo remove' so the lockfile stays consistent." >&2
  exit 2
fi
exit 0
