# loop.md — Loop Contract: overlay-tab-binding

## Goal
Bind the ai-monitor panel to **the Claude session running in this tab**, identified from the tab's own process tree — so a session another application is running in the same project directory can never be shown as this tab's.

## The defect (measured, 2026-07-28)
A newly opened tab showed `session 54e576c2 · claude-opus-4-8 · context >100% · up 116h40m · idle 0s`. The `idle 0s` is the tell: that log was being written *right then*, so it passed every freshness rule legitimately.

It belongs to **Warp**:

```
claude --resume 54e576c2-bd76-4acd-8eed-bda54f60a155   (pid 10502, started 07-26)
  └─ zsh                                                (53653)
       └─ Warp.app/Contents/MacOS/stable terminal-server
```

That session resumed a 07-23 conversation (hence `up 116h40m`) in `~/IdeaProjects/terminal`, so it writes into the same `~/.claude/projects/-Users-hyunhokim-IdeaProjects-terminal/` directory this tab resolves. The tab's own session was `492a2fc3` (first event 13:47), whose `claude` processes (35303, 35404) are descendants of the mac-terminal process.

**`~/.claude/projects/<cwd>/` has no notion of which application, window or tab wrote a log.** Warp, iTerm, VS Code and every other mac-terminal window share it. Three previous rounds refined *which* log in that directory to believe; none of them could have fixed this, because the discriminating information is not in the directory at all. It is in the process tree.

## Approach (human-approved 2026-07-28)
1. **Identify the tab's own `claude` process.** The app already holds each tab's shell pid (`Pty::child_pid`, `crates/term-core/src/pty.rs:148`, whose doc names ai-monitor log resolution as its purpose and which nothing has used yet). Walk its descendants with `libc::proc_listchildpids` + `PROC_PIDTBSDINFO`, mirroring the existing `cwd_of_pid` precedent (`crates/app/src/session.rs:185`).
2. **Turn that process into a session hint.** `--resume <id>` in its argv names the session **exactly**. Otherwise its start time dates the session, and the tab's log is the one whose *first* event sits just after it.
3. **Only a hint-confirmed log is ever shown.** Resolution returns a session confirmed as this tab's, or nothing: a session belonging to no tab-process is never displayed, and the panel renders an explicit empty state instead. *(**Reversed 2026-08-01** by `specs/ai-monitor-session-isolation/spec.md`. This line previously read: "Resolution prefers an owned candidate; a session belonging to no tab-process can still be shown when nothing better exists, but it is never allowed to hold the panel against an owned one — the approved re-attach rule." Why it was reversed: a tab not running `claude` at all, or whose own session had not yet written its first event, was being shown a stranger's dashboard — the `owned` flag never reached the screen, so a guess rendered pixel-identical to a confirmed session with nothing on screen to indicate it was a guess. The re-attach rule survives as the contract for the retained ranked fallback; see the Gate 2 rows below.)*
4. **Data preservation is unchanged.** Once attached to an *owned* session, the panel is never blanked and never replaced (`specs/overlay-session-attach/loop.md`, rounds 1-2).

## Scope & non-goals
- In: `crates/app/src/tabsession.rs` (new), `crates/app/src/session.rs`, `crates/app/src/tabs.rs`, `crates/app/src/seam.rs`, `crates/app/src/terminal_app.rs`, `crates/app/src/overlay.rs`, `crates/app/src/main.rs`, `crates/ai-monitor/src/lib.rs`
- The fence around `seam.rs` / `terminal_app.rs` is **lifted for this contract only**, by explicit approval, because the pid cannot reach the overlay without it.
- Out: `crates/renderer/**`, `crates/common/**`, `crates/term-core/**` (`child_pid` already exists — no change needed).
- Not a goal: identifying a session with no live process (a finished session cannot be bound to a tab, and does not need to be).

---

## Gate 1 — Pass/Fail

| Check | Command |
|-------|---------|
| build | `cargo build --workspace` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |

## Gate 2 — Quantitative

| Metric | Threshold | How measured |
|--------|-----------|--------------|
| test failures | `== 0` | `cargo test --workspace` |
| finds its own process | the walk locates a `claude` descendant of a shell pid, at any depth | named test against a real spawned process tree |
| exact on resume | `--resume <uuid>` in argv yields that session id | named test |
| dates a fresh session | no `--resume` yields the process start time, within a second of truth | named test against a real process |
| owned beats unowned *(retained fallback path)* | still holds where it is still reachable — in `resolve_session`, the ranked fallback kept under test with no production caller since `specs/ai-monitor-session-isolation/spec.md`: given a foreign log with a NEWER last event and an owned log with an older one, it picks the **owned** one. The production path is now the strict resolver `resolve_owned_session`, which does not rank the foreign log at all — it answers `None` | fixture test |
| re-attach *(retained fallback path)* | still holds for a caller using `resolve_session`: an owned candidate replaces an unowned attachment, and an owned attachment is never replaced. Production can no longer reach the displacement branch — under strict resolution every attachment is confirmed, so there is no unowned one to displace — so this row is certified by test, not exercised in production | named test |
| preservation intact | rounds 1-2 guarantees still hold: empty never replaces, different session never replaces an owned attachment | the existing named tests still pass |
| snapshots | the 9 `.snap` files unchanged | `git status --porcelain crates/app/src/snapshots/` |

## Gate 3 — Qualitative
- **The discriminator is identity, not timing** — where a process names its session, that name is used; timing is the fallback, and it is labelled as one.
- **Degrades to the current behaviour** — no claude process, no permission, a dead pid, a sandbox: resolution falls back to the round-2 ranking rather than showing nothing.
- **Dependency direction** — process inspection lives in `app` (which already has `libc`); `ai-monitor` takes a hint and gains no new dependency.
- **No panic path** — every FFI call is length-checked, every buffer bounded; a hostile or dead pid cannot abort the terminal.
- **Cost is bounded** — the walk is depth-limited and breadth-limited, and runs at open / re-resolve, never per frame.

---

## Autonomy Boundary
**✅ Auto-fix:** anything within the declared files, including the seam and event-loop edits needed to carry the pid.
**🛑 STOP — escalate:**
- Any change to `crates/renderer/**` or `crates/common/**`.
- Adding a crate dependency to any crate (`libc` is already in `app`; `ai-monitor` stays `serde_json` + `notify`).
- Any write to a session log, under `~/.claude`, or to any process (no signals, no ptrace — read-only observation).
- Weakening the rounds 1-2 preservation guarantees to make binding easier.

## Exit criteria
Every gate passes and the Evidence Report is emitted.
