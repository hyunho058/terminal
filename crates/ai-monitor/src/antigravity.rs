//! Antigravity CLI (`agy`) session discovery — the headless half of AG1/AG6.
//!
//! `agy` records a conversation in three places under `~/.gemini/antigravity-cli`:
//! a `presence/<id>.lock` it holds **open** for the life of the session, a
//! `brain/<id>/.system_generated/logs/transcript.jsonl` it appends steps to, and a line
//! in `history.jsonl` pairing that conversation with the workspace the user ran it in.
//!
//! This module owns the paths and the `history.jsonl` tier. It deliberately does **not**
//! own the open-FD question — "which lock does pid P hold?" needs `proc_pidinfo`, and
//! acquiring `libc` here would trade away exactly the headless purity that makes this
//! crate testable (AG11). `app` answers it and hands us the conversation id.
//!
//! Read-only (Constraints): every path below is opened for reading or not at all. Nothing
//! under `~/.gemini/antigravity-cli` is created, locked, or written — including the lock
//! files, which we never open even to test them.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{looks_like_session_id, Resolved};

/// How much of `history.jsonl` one lookup reads, from the **end**.
///
/// The file is small (43 lines when measured) but it is appended live and unbounded, and
/// resolution runs on a user-visible path. Reading the tail rather than the head is not
/// merely a cap: the tier wants the *newest* record for the workspace, so the bytes worth
/// having are the last ones. A record sliced in half by the seek simply fails to parse,
/// which is the same tolerance [`crate::backfill`] and the Claude tail reads already rely on.
const HISTORY_READ_CAP: u64 = 256 * 1024;

/// How much of a `transcript.jsonl` is scanned for its last step time.
///
/// Deliberately the same figure as [`crate::activity::TAIL_CAP`], for the same reason: this
/// is a freshness probe, not a parse, and a session that wrote nothing in its final 8 KiB
/// is one whose last step time we would rather report as unknown than pay a full read for.
const TRANSCRIPT_TAIL_CAP: u64 = crate::activity::TAIL_CAP;

/// `~/.gemini/antigravity-cli` — the root every other path here hangs off. `None` when
/// `$HOME` is unset, matching [`crate::claude_projects_dir`]'s contract.
pub fn antigravity_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".gemini").join("antigravity-cli"))
}

/// `<root>/presence` — where `agy` keeps the lock it holds open per live conversation.
///
/// The directory is what `app`'s FD probe matches vnode paths against; this crate never
/// reads it, because **its contents prove nothing**. Locks outlive their sessions (30 files
/// for a handful of live sessions, when measured), so `presence/<id>.lock` existing is not
/// evidence that `<id>` is running — only an open descriptor on it is (AG1, R1.2).
/// The presence directory for the real installation, if there is one.
///
/// Public because the app arms a [`crate::WatchScope::AwaitingLock`] watch on it: a tab whose
/// agent has not yet published a lock has nothing to tail, and this is the one directory
/// where the fact that changes that will appear.
pub fn presence_dir() -> Option<PathBuf> {
    Some(presence_dir_in(&antigravity_root()?))
}

pub fn presence_dir_in(root: &Path) -> PathBuf {
    root.join("presence")
}

/// `<root>/presence/<id>.lock`, for a conversation id that passes the path guard.
pub fn presence_lock_in(root: &Path, conversation_id: &str) -> Option<PathBuf> {
    checked_id(conversation_id).map(|id| presence_dir_in(root).join(format!("{id}.lock")))
}

/// The conversation id a presence lock path names, or `None` if it is not one.
///
/// Both halves are checked, and both are load-bearing. The `.lock` extension is what
/// distinguishes the file from anything else `agy` may put beside it; the UUID shape is the
/// **path guard** [`looks_like_session_id`] exists for — this id arrives from another
/// program's on-disk state via a kernel FD listing, and it is about to become a path
/// component under `brain/`.
pub fn conversation_id_from_lock(lock: &Path) -> Option<String> {
    if lock.extension().and_then(|e| e.to_str()) != Some("lock") {
        return None;
    }
    let stem = lock.file_stem().and_then(|s| s.to_str())?;
    checked_id(stem).map(str::to_string)
}

/// `<root>/brain/<id>` — one conversation's directory, whether or not it exists yet.
pub fn brain_dir_in(root: &Path, conversation_id: &str) -> Option<PathBuf> {
    checked_id(conversation_id).map(|id| root.join("brain").join(id))
}

/// `<root>/brain/<id>/.system_generated/logs/transcript.jsonl` — the append-only step log,
/// whether or not it exists yet (R1.1).
///
/// `transcript.jsonl`, not `transcript_full.jsonl` and not the `chunks/transcript/*.jsonl`
/// mirrors. The chunks are byte-exact slices of this file (verified: 102400 + 102400 + 14396
/// = the file's size), so tailing them as well would double-fold every step; the `_full`
/// twin holds the same lines with untruncated bodies, and telemetry needs counts and paths
/// rather than bodies.
pub fn transcript_in(root: &Path, conversation_id: &str) -> Option<PathBuf> {
    Some(
        brain_dir_in(root, conversation_id)?
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl"),
    )
}

/// `<root>/history.jsonl` — the user-interaction log the fallback tier reads.
pub fn history_path_in(root: &Path) -> PathBuf {
    root.join("history.jsonl")
}

/// The conversation that owns `log`, recovered from its PATH (AG5).
///
/// The inverse of [`transcript_in`], and the only correct source for this id. A transcript
/// does not name itself: measured across all 60 on this machine, **none** carries its own
/// conversation id anywhere in its content. Six do contain a `conversationId` — every one of
/// them a *child* subagent's, spliced into an `INVOKE_SUBAGENT` result, which is why
/// [`ag_conversation_id`](crate::telemetry) reads that field for the SUBAGENTS card and
/// nothing else. Scraping the file for a session id would therefore not merely fail; on those
/// six it would confidently display another conversation's id (`22c93e00…`'s transcript names
/// only `7070769f…`).
///
/// Every component is checked, not just the last: a path that is not laid out like a
/// transcript, or whose directory is not UUID-shaped, yields `None` rather than a fabricated
/// id built out of whatever happened to sit three levels up.
pub fn conversation_id_of(log: &Path) -> Option<String> {
    let logs = log.parent()?;
    let generated = logs.parent()?;
    let brain_child = generated.parent()?;
    if logs.file_name()? != "logs" || generated.file_name()? != ".system_generated" {
        return None;
    }
    checked_id(brain_child.file_name()?.to_str()?).map(str::to_string)
}

/// The conversation `history.jsonl` most recently recorded for `cwd` (AG6, R1.3).
///
/// The only source on disk that pairs a conversation with a working directory. It is sparse
/// — 31 of 43 records are `slash_command` entries carrying no `conversationId` at all, and
/// the 12 that do name just 9 conversations — but every one of those 9 resolved to a real
/// `brain/` directory and **none of them was a subagent**. That last property is the whole
/// reason this tier exists rather than a directory scan: it is a log of things a *human*
/// did, so a machine-spawned child conversation cannot appear in it by construction (G3).
///
/// Candidates are ranked newest-first by the record's own `timestamp` (epoch millis) and the
/// first whose `brain/<id>/` directory exists wins. Note what that recency is *over*: user
/// interactions with this workspace, ordered by a field the log itself records. It is not a
/// scan of `brain/`, and it must never become one — see [`resolve_in`].
pub fn history_conversation_in(root: &Path, cwd: &Path) -> Option<String> {
    let text = history_text(root)?;
    // (timestamp, line index, id) — the index breaks timestamp ties toward the later
    // append, which for an append-only log is the later interaction. A record with no
    // usable timestamp sorts oldest rather than being dropped: it is still evidence that
    // this workspace used that conversation, just undated.
    let mut hits: Vec<(u64, usize, String)> = text
        .lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let v: Value = serde_json::from_str(line).ok()?;
            let workspace = v.get("workspace").and_then(Value::as_str)?;
            if Path::new(workspace) != cwd {
                return None;
            }
            let id = v.get("conversationId").and_then(Value::as_str)?;
            let ts = v.get("timestamp").and_then(epoch_ms).unwrap_or(0);
            checked_id(id).map(|id| (ts, i, id.to_string()))
        })
        .collect();
    hits.sort_unstable_by_key(|(ts, i, _)| std::cmp::Reverse((*ts, *i)));
    // `brain/<id>/` must exist. `history.jsonl` is another program's undocumented on-disk
    // detail (the same standing `Registered` states for Claude's registry), so a record
    // naming a directory that was since deleted is a thing to skip past, not to trust.
    hits.into_iter()
        .find(|(_, _, id)| brain_dir_in(root, id).is_some_and(|d| d.is_dir()))
        .map(|(_, _, id)| id)
}

/// Resolve the transcript this tab should tail, under an explicit root (AG6's ladder).
///
/// `probed` is the conversation id `app`'s FD probe found the tab's `agy` holding open, or
/// `None` when it could not read one. `cwd` is the tab's working directory, which is what
/// the `history.jsonl` tier matches on.
///
/// Two tiers, in order:
///
/// 1. **The open FD** (R1.1). A held lock names the conversation *exactly* — no timing
///    heuristic, no rival contest, none of the machinery Claude needs because its logs carry
///    no owner. Resolves `owned: true`.
/// 2. **`history.jsonl` scoped to `cwd`** (R1.3). Resolves `owned: false`: the record says
///    this workspace used that conversation, not that *this process* is running it.
///
/// A probed id does **not** fall through to tier 2 when its transcript has not been written
/// yet. That is R1.6: a freshly launched `agy` is unattached, not attached to whatever it
/// ran here last. The rule is Claude's tier 1 verbatim — "if its log does not exist yet, the
/// honest answer is 'not written yet', NOT a timing guess" ([`crate::resolve_owned_session`])
/// — and it matters more here, because the previous conversation in this workspace is a real
/// transcript that would stream real, live, wrong telemetry into the panel.
pub fn resolve_in(root: &Path, probed: Probed<'_>, cwd: &Path) -> Option<Resolved> {
    match probed {
        Probed::Conversation(id) => return resolved(root, id, true),
        // ---------------------------------------------------------------------------
        // A LIVE AGENT THAT NAMED NO CONVERSATION MUST NOT FALL BACK (AG6, amended).
        //
        // `history.jsonl` records a conversation only once it has run a slash command,
        // so a session that started seconds ago is *structurally absent* from it. When a
        // live `agy` exists but its lock is not readable yet, every candidate the history
        // tier can offer is therefore an OLDER conversation — the tier cannot be right
        // here, only wrong.
        //
        // Measured, this is not hypothetical. `agy` creates its presence lock and its
        // transcript ~7s after the process starts (observed: process 00:07:22, lock and
        // transcript both 00:07:29). A panel opening inside that window fell through to
        // history and adopted the conversation whose newest record was the user's own
        // `/exit` — i.e. proof that session had ENDED — and rendered its 11-minute-old
        // telemetry as the tab's live session.
        //
        // So: no log, and the caller re-resolves once the lock appears.
        Probed::LiveButUnnamed => return None,
        Probed::NoAgent => {}
    }
    if let Some(id) = history_conversation_in(root, cwd) {
        return resolved(root, &id, false);
    }
    // ---------------------------------------------------------------------------------
    // THERE IS NO THIRD TIER, AND THE ABSENCE IS THE FEATURE (R1.4, AG6).
    //
    // The obvious next move — "take the newest transcript under `brain/`" — is rejected on
    // measurement, not taste. 18 of the 51 transcripts on the machine this was measured on
    // are *subagent children*: they sit in `brain/` beside their parents with no marker
    // distinguishing them, their step 0 is a `USER_INPUT` with the same `<USER_REQUEST>`
    // envelope a human's carries, and `<ADDITIONAL_METADATA>` appears in 18/18 children and
    // 33/33 non-children, so no content test separates them either.
    //
    // Worse, an mtime tiebreak fails *specifically* when it is consulted. A running subagent
    // writes flat out precisely while its parent sits idle waiting for it, so at the moment
    // any fallback runs, the newest-written transcript on disk is the child. The panel would
    // then show real, live, wrong telemetry for the busiest sessions — silent-wrong, which
    // is the expensive kind.
    //
    // `conversation_summaries.db` is rejected for a duller reason: its ids and `brain/`'s
    // directory names are disjoint sets (102 rows, 54 directories, intersection 0), so no
    // query against it can name a transcript path at all.
    //
    // So when both tiers miss, the answer is `None` and the panel stays unattached. That is
    // the rule this crate already applies to Claude — "a tab with no identifiable session
    // must show nothing rather than adopt whatever else happens to be writing there" — and
    // the subagent measurement is what makes it the same rule here rather than a stricter
    // one. Do not add a recency tiebreak across `brain/` at any tier.
    // ---------------------------------------------------------------------------------
    None
}

/// [`resolve_in`] against the real `~/.gemini/antigravity-cli`.
pub fn resolve(probed: Probed<'_>, cwd: &Path) -> Option<Resolved> {
    resolve_in(&antigravity_root()?, probed, cwd)
}

/// What the caller's FD probe was able to say about the tab (AG6).
///
/// Three states, not an `Option`, because "no lock to read" and "no agent to ask" are
/// different facts with opposite consequences for the fallback — and collapsing them into
/// `None` is exactly the bug this type exists to make unrepresentable. The history tier is
/// safe only in the third case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probed<'a> {
    /// The probe named the conversation this tab's agent holds open.
    Conversation(&'a str),
    /// A live agent is running under this tab, but named no conversation — its lock is
    /// not readable yet (`agy` writes it ~7s after start), or could not be read at all.
    /// The live conversation cannot be in `history.jsonl` yet, so no fallback applies.
    LiveButUnnamed,
    /// No agent process under this tab at all. Only here can the history tier help.
    NoAgent,
}

/// A [`Resolved`] for `id`, or `None` when its transcript is not on disk yet (R1.6).
fn resolved(root: &Path, id: &str, owned: bool) -> Option<Resolved> {
    let path = transcript_in(root, id)?;
    path.is_file().then(|| Resolved {
        last_event_ms: last_step_in_tail(&path),
        path,
        owned,
    })
}

/// The path guard, applied at every point an id becomes a path component.
///
/// The ids reaching this module come from places we do not control: a lock filename read out
/// of a kernel FD listing, and another program's `history.jsonl`. `../../etc/passwd` is a
/// perfectly good string and a very bad path component, so anything that is not UUID-shaped
/// is rejected before it is ever joined. Antigravity conversation ids are the same 36-char
/// lowercase-hex shape as Claude session ids, so [`looks_like_session_id`] is reusable as-is
/// rather than needing a second, drift-prone spelling of the same rule.
fn checked_id(id: &str) -> Option<&str> {
    looks_like_session_id(id).then_some(id)
}

/// The last [`HISTORY_READ_CAP`] bytes of `history.jsonl`, or `None` if it is unreadable.
fn history_text(root: &Path) -> Option<String> {
    let mut f = std::fs::File::open(history_path_in(root)).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(HISTORY_READ_CAP)))
        .ok()?;
    let mut buf = Vec::new();
    f.take(HISTORY_READ_CAP).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// A JSON number read as epoch millis. `history.jsonl`'s `timestamp` is an integer in every
/// record measured, but JSON has one number type and a writer is free to emit `1.7e12`, so
/// the float form is accepted rather than silently sorting such a record as undated.
/// Negative and non-finite values yield `None`.
fn epoch_ms(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| {
        let f = v.as_f64()?;
        (f.is_finite() && f >= 0.0).then_some(f as u64)
    })
}

/// The newest parseable `created_at` in the final [`TRANSCRIPT_TAIL_CAP`] bytes of a
/// transcript, in epoch millis.
///
/// `created_at`, not `timestamp`: that is the step envelope's field, present on all 1,761
/// steps measured, RFC3339 with a `Z` suffix. It is explicitly **not** the local-offset
/// timestamp that sometimes appears inside a step's `content` (`Created At:
/// 2026-07-26T09:09:03-07:00`) — those are a different clock and mixing them would produce
/// a session time hours out.
fn last_step_in_tail(transcript: &Path) -> Option<u64> {
    let mut f = std::fs::File::open(transcript).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(TRANSCRIPT_TAIL_CAP)))
        .ok()?;
    let mut buf = Vec::new();
    f.take(TRANSCRIPT_TAIL_CAP).read_to_end(&mut buf).ok()?;
    String::from_utf8_lossy(&buf)
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| {
            v.get("created_at")
                .and_then(Value::as_str)
                .and_then(crate::telemetry::parse_ts)
        })
        .max()
}
