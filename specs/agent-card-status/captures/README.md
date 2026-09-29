# Captures (agent-card-status D12)

Real screens, taken from this terminal's own `waiting-scan` debug line (T0's `scan_dump`:
the matcher's whole `TAIL_ROWS` input, Debug-escaped) and decoded back to text. They are the
source of truth for `prompt.rs`'s Antigravity vocabulary — markers come from these bytes, not
from screenshots or the `agy` binary's strings.

- `antigravity_permission_dialog.txt` — Antigravity CLI 1.2.12 (Gemini 3.8 Flash), a
  `run_command` permission for `ls -la`, captured 2026-09-28. The same tail on every scan
  while the dialog stayed up (24 when it was extracted; the dialog was left open for hours
  afterwards). The pointer is ASCII `> ` (not Claude's `❯`); the footer
  is two rows, `esc to cancel` starting the last one with the model name on the same row.
  At capture time the tab read `state=Unknown detected=false` — the bug this spec fixes.
- `antigravity_generating.txt` — the same session a moment earlier, mid-turn with no dialog
  (`⣾  Generating...`), 24 scans across its spinner frames. It carries `esc to cancel … <model>`
  too, which is why that row is not a dialog marker (D5, amended): the negative fixture.
  One edit: the login banner's account email is redacted to `user@example.com`; every other
  byte is as captured, and the banner plays no part in detection.
