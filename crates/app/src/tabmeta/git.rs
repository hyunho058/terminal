//! Git branch from a bounded `.git` ancestor walk, behind a cwd-keyed memo (D9).
//!
//! `cd` needs no invalidation — the worker re-resolves cwd from the live pid every tick
//! (D10), so a new cwd is simply a new key. `git checkout` changes neither process nor
//! cwd, so a warm `Repo` entry revalidates by `stat`-ing HEAD's `(mtime, len)` — one
//! `stat`, no read, in the steady state (R3.6). A non-repo cwd is memoised too, but its
//! `NotARepo` entry expires so a later `git init` is picked up (R7.9). Reclamation is by
//! disuse, never by tab removal, because two tabs in one repo **share** an entry (R1.5/R1.6).
//!
//! Reads are hard-capped: [`HEAD_READ_CAP`] bytes, so a pathological HEAD cannot push an
//! unbounded string into a struct the UI clones (R7.10); the parsed branch is capped at
//! [`BRANCH_MAX`] chars.
//!
//! Beside the branch memo sits the **repository identity** the session-flow graph groups by
//! (session-flow-graph D10): [`repo_of`] resolves a cwd to the repository's common git dir and
//! main worktree, and [`RepoMemo`] memoises it by cwd on the same terms — a warm entry
//! revalidates by `stat` alone (the found `.git`, plus `commondir` for a linked worktree), a
//! miss expires after [`NEG_MEMO_TTL_TICKS`].

use std::collections::HashMap;
use std::fs::Metadata;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Ancestors examined before giving up (R7.6).
const MAX_GIT_ANCESTORS: usize = 40;
/// Cap on bytes read from `.git`/HEAD files (R7.10).
const HEAD_READ_CAP: u64 = 4096;
/// Cap on the parsed branch string (R7.10).
const BRANCH_MAX: usize = 64;
/// A `NotARepo` entry is re-checked after this many ticks (≈10 s at the 500 ms base
/// tick), so `git init` is noticed (R7.9).
const NEG_MEMO_TTL_TICKS: u64 = 20;
/// An entry untouched for this many ticks (≈60 s) is reclaimed (R1.6).
const MEMO_TTL_TICKS: u64 = 120;
/// Hard cap on memo entries; oldest-touched evicted first (R1.6).
const MEMO_MAX: usize = 64;

/// The cwd-keyed branch memo (D9). Shared by tabs in the same repo, so eviction is by
/// disuse — never by `TabId` (R1.5).
pub(crate) struct Memo {
    entries: HashMap<PathBuf, Entry>,
}

struct Entry {
    kind: EntryKind,
    last_read_tick: u64,
}

enum EntryKind {
    /// A resolved repo: revalidated by `stat`-ing `head_path`.
    Repo {
        head_path: PathBuf,
        head_mtime: SystemTime,
        head_len: u64,
        branch: Option<String>,
    },
    /// A cwd with no `.git` ancestor; expires so `git init` is picked up.
    NotARepo { checked_at: u64 },
}

impl Memo {
    pub(crate) fn new() -> Memo {
        Memo {
            entries: HashMap::new(),
        }
    }

    /// Test-only view of the entry count (for the reclamation tests).
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Drop entries untouched for `MEMO_TTL_TICKS`, then evict oldest-touched down to
    /// `MEMO_MAX` (R1.6). Called at the end of every derivation, so a shell that `cd`s
    /// through hundreds of directories cannot grow the map without bound.
    fn reclaim(&mut self, tick: u64) {
        reclaim_map(&mut self.entries, tick, |e| e.last_read_tick);
    }
}

/// The reclamation both memos share: drop entries untouched for `MEMO_TTL_TICKS`, then evict
/// oldest-touched down to `MEMO_MAX` (R1.6).
fn reclaim_map<E>(entries: &mut HashMap<PathBuf, E>, tick: u64, last: fn(&E) -> u64) {
    entries.retain(|_, e| tick.saturating_sub(last(e)) < MEMO_TTL_TICKS);
    while entries.len() > MEMO_MAX {
        let oldest = entries
            .iter()
            .min_by_key(|(_, e)| last(e))
            .map(|(k, _)| k.clone());
        match oldest {
            Some(k) => {
                entries.remove(&k);
            }
            None => break,
        }
    }
}

/// The branch for `cwd` at `tick`, via the memo (D9). `None` for a non-repo, an
/// unresolvable cwd, or a garbage HEAD — every failure is absent, never a panic.
pub(crate) fn branch_of_cwd(memo: &mut Memo, cwd: &Path, tick: u64) -> Option<String> {
    let result = match lookup(memo, cwd, tick) {
        Some(answer) => answer,
        None => resolve(memo, cwd, tick),
    };
    memo.reclaim(tick);
    result
}

/// Fast path over an existing entry. `Some(branch)` means the memo answered (a
/// revalidated repo, or a still-fresh `NotARepo` → `Some(None)`); `None` means a full
/// [`resolve`] is needed (absent, expired `NotARepo`, or a vanished HEAD).
fn lookup(memo: &mut Memo, cwd: &Path, tick: u64) -> Option<Option<String>> {
    let entry = memo.entries.get_mut(cwd)?;
    entry.last_read_tick = tick;
    match &mut entry.kind {
        EntryKind::Repo {
            head_path,
            head_mtime,
            head_len,
            branch,
        } => match stat_meta(head_path) {
            Some((m, l)) if m == *head_mtime && l == *head_len => Some(branch.clone()),
            Some((m, l)) => {
                let b = read_and_parse_head(head_path);
                *head_mtime = m;
                *head_len = l;
                *branch = b.clone();
                Some(b)
            }
            None => None, // HEAD gone → re-resolve from scratch
        },
        EntryKind::NotARepo { checked_at } => {
            if tick.saturating_sub(*checked_at) < NEG_MEMO_TTL_TICKS {
                Some(None)
            } else {
                None
            }
        }
    }
}

/// Cold path: walk ancestors for `.git`, read HEAD, and record the outcome.
fn resolve(memo: &mut Memo, cwd: &Path, tick: u64) -> Option<String> {
    match resolve_head_path(cwd) {
        Some(head_path) => {
            let (head_mtime, head_len) = stat_meta(&head_path).unwrap_or((UNIX_EPOCH, 0));
            let branch = read_and_parse_head(&head_path);
            memo.entries.insert(
                cwd.to_path_buf(),
                Entry {
                    kind: EntryKind::Repo {
                        head_path,
                        head_mtime,
                        head_len,
                        branch: branch.clone(),
                    },
                    last_read_tick: tick,
                },
            );
            branch
        }
        None => {
            memo.entries.insert(
                cwd.to_path_buf(),
                Entry {
                    kind: EntryKind::NotARepo { checked_at: tick },
                    last_read_tick: tick,
                },
            );
            None
        }
    }
}

/// Walk up from `cwd` (≤ `MAX_GIT_ANCESTORS`) to the first `.git`, returning the path to
/// its HEAD. `.git` as a directory → `.git/HEAD`; `.git` as a `gitdir:` file (worktree /
/// submodule) → the referenced dir's HEAD (R7.4).
fn resolve_head_path(cwd: &Path) -> Option<PathBuf> {
    let mut dir: &Path = cwd;
    for _ in 0..MAX_GIT_ANCESTORS {
        let dot_git = dir.join(".git");
        if let Some(meta) = meta_of(&dot_git) {
            if meta.is_dir() {
                return Some(dot_git.join("HEAD"));
            }
            // `.git` is a file → `gitdir: <path>`, resolved relative to `dir`.
            let data = read_capped(&dot_git)?;
            let text = String::from_utf8_lossy(&data);
            let gitdir = parse_gitdir_file(&text)?;
            let base = if Path::new(&gitdir).is_absolute() {
                PathBuf::from(gitdir)
            } else {
                dir.join(gitdir)
            };
            return Some(base.join("HEAD"));
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => break,
        }
    }
    None
}

/// `symlink_metadata` (so a `.git` file is not chased), counting one stat probe. The
/// single filesystem `stat` used for both ancestor probing and HEAD revalidation.
fn meta_of(path: &Path) -> Option<std::fs::Metadata> {
    #[cfg(test)]
    super::probe::GIT_STAT.bump(1);
    std::fs::symlink_metadata(path).ok()
}

/// A path's `(mtime, len)` for memo revalidation.
fn stat_meta(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = meta_of(path)?;
    let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
    Some((mtime, meta.len()))
}

/// Read at most `HEAD_READ_CAP` bytes — a genuine cap, not a post-hoc truncation, so a
/// 10 MB HEAD costs 4 KiB of I/O (R7.10). Counts the read and the bytes read.
fn read_capped(path: &Path) -> Option<Vec<u8>> {
    #[cfg(test)]
    super::probe::GIT_READ.bump(1);
    let file = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    file.take(HEAD_READ_CAP).read_to_end(&mut buf).ok()?;
    #[cfg(test)]
    super::probe::GIT_READ_BYTES.bump(buf.len() as u64);
    Some(buf)
}

fn read_and_parse_head(head_path: &Path) -> Option<String> {
    let data = read_capped(head_path)?;
    let text = String::from_utf8_lossy(&data);
    parse_head(&text)
}

/// Extract `gitdir: <path>` from a `.git` file's contents.
fn parse_gitdir_file(text: &str) -> Option<String> {
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("gitdir:") {
            let p = rest.trim();
            if !p.is_empty() {
                return Some(p.to_string());
            }
        }
    }
    None
}

/// Pure: parse HEAD contents (R11.4). `ref: refs/heads/<name>` → `<name>` verbatim
/// (slashes kept); a bare 40-hex SHA → `@` + first 7; anything else → `None`. The result
/// is capped at `BRANCH_MAX` chars.
pub(crate) fn parse_head(text: &str) -> Option<String> {
    let line = text.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return None;
    }
    if let Some(rest) = line.strip_prefix("ref:") {
        let name = rest.trim().strip_prefix("refs/heads/")?.trim();
        if name.is_empty() {
            return None;
        }
        return Some(cap_branch(name));
    }
    if line.len() == 40 && line.bytes().all(|b| b.is_ascii_hexdigit()) {
        let short: String = line.chars().take(7).collect();
        return Some(format!("@{short}"));
    }
    None
}

fn cap_branch(name: &str) -> String {
    name.chars().take(BRANCH_MAX).collect()
}

// ---------------------------------------------------------------------------
// Repository identity (session-flow-graph D10).
// ---------------------------------------------------------------------------

/// Which repository a session runs in, and which worktree of it (session-flow-graph D10).
///
/// Sessions group by `common`; the project card shows `main_root`. Both paths are lexically
/// normalised (no `.` or `..` left) but never canonicalised, so two sessions reaching the same
/// repository by the same spelling compare equal without a `realpath` per tick.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RepoId {
    /// The repository's common git directory — `<main>/.git`, or a submodule's own git dir.
    pub common: PathBuf,
    /// The main worktree: the directory `common` belongs to (a submodule's checkout for one).
    pub main_root: PathBuf,
    /// `true` for a linked worktree (`git worktree add`), `false` for the main one.
    pub linked: bool,
}

/// What a warm [`RepoMemo`] entry revalidates against: whether the path is a directory and,
/// for a file, its `(mtime, len)`. A `.git` **directory**'s mtime moves on every git write
/// inside it, so for a directory only its kind is compared — the property that matters is that
/// it still exists.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Stamp {
    dir: bool,
    mtime: SystemTime,
    len: u64,
}

fn stamp(meta: &Metadata) -> Stamp {
    if meta.is_dir() {
        Stamp {
            dir: true,
            mtime: UNIX_EPOCH,
            len: 0,
        }
    } else {
        Stamp {
            dir: false,
            mtime: meta.modified().unwrap_or(UNIX_EPOCH),
            len: meta.len(),
        }
    }
}

/// A cold resolution: the identity plus the paths a warm lookup re-`stat`s.
#[derive(Debug)]
struct Resolved {
    id: RepoId,
    /// The `.git` the walk found, and its stamp.
    dot_git: (PathBuf, Stamp),
    /// `<g>/commondir` for a linked worktree, and its stamp.
    commondir: Option<(PathBuf, Stamp)>,
}

/// The repository `cwd` runs in, per session-flow-graph D10. Uncached — the worker goes
/// through [`repo_of_cwd`]; this is the pure rule the acceptance tests pin.
///
/// - the first `.git` within `MAX_GIT_ANCESTORS` is a **directory** → the main worktree;
/// - a `gitdir: <g>` **file** with a `<g>/commondir` → a linked worktree, `common` =
///   `<g>/<commondir>`, `main_root` = `common`'s parent;
/// - a `gitdir:` file with no `commondir` (a submodule) → its own main worktree,
///   `common` = `<g>`;
/// - no `.git`, `cwd` itself missing, a `.git` file with no `gitdir:` line, or an unreadable
///   `commondir` → `None`.
///
/// The existence check on `cwd` is load-bearing: linked worktrees also live **inside** the
/// main one (`<repo>/.claude/worktrees/<name>`), and once such a directory is removed its
/// ancestor walk would otherwise land on the main `.git` and regroup the session as a main one.
pub fn repo_of(cwd: &Path) -> Option<RepoId> {
    resolve_repo(cwd).map(|r| r.id)
}

fn resolve_repo(cwd: &Path) -> Option<Resolved> {
    meta_of(cwd)?;
    let mut dir: &Path = cwd;
    for _ in 0..MAX_GIT_ANCESTORS {
        let dot_git = dir.join(".git");
        if let Some(meta) = meta_of(&dot_git) {
            return resolve_dot_git(dir, dot_git, &meta);
        }
        dir = dir.parent()?;
    }
    None
}

/// The identity a found `.git` (at `dir/.git`, with `meta`) names.
fn resolve_dot_git(dir: &Path, dot_git: PathBuf, meta: &Metadata) -> Option<Resolved> {
    let found = (dot_git.clone(), stamp(meta));
    if meta.is_dir() {
        return Some(Resolved {
            id: RepoId {
                common: dot_git,
                main_root: dir.to_path_buf(),
                linked: false,
            },
            dot_git: found,
            commondir: None,
        });
    }
    let data = read_capped(&dot_git)?;
    let gitdir = parse_gitdir_file(&String::from_utf8_lossy(&data))?;
    let g = normalise(&dir.join(gitdir));
    let commondir = g.join("commondir");
    let Some(cmeta) = meta_of(&commondir) else {
        // A submodule: its git dir is its own common dir.
        return Some(Resolved {
            id: RepoId {
                common: g,
                main_root: dir.to_path_buf(),
                linked: false,
            },
            dot_git: found,
            commondir: None,
        });
    };
    let text = read_capped(&commondir)?;
    let rel = String::from_utf8_lossy(&text).trim().to_string();
    if rel.is_empty() {
        return None;
    }
    let common = normalise(&g.join(rel));
    let main_root = common.parent()?.to_path_buf();
    Some(Resolved {
        id: RepoId {
            common,
            main_root,
            linked: true,
        },
        dot_git: found,
        commondir: Some((commondir, stamp(&cmeta))),
    })
}

/// Lexical normalisation: drop `.` and resolve `..` against the preceding component. No
/// filesystem access, so a symlinked component is kept as spelled. (`Path::join` with an
/// absolute right-hand side replaces the left, so an absolute `gitdir:` or `commondir` comes
/// through unchanged.)
fn normalise(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The cwd-keyed repository memo (session-flow-graph D10), reclaimed exactly as [`Memo`].
pub(crate) struct RepoMemo {
    entries: HashMap<PathBuf, RepoEntry>,
}

struct RepoEntry {
    kind: RepoKind,
    last_read_tick: u64,
}

enum RepoKind {
    /// A resolved repository: revalidated by `stat`-ing what it was found from.
    Repo(Resolved),
    /// No repository; expires so a later `git init` (or a re-created worktree) is noticed.
    None { checked_at: u64 },
}

impl RepoMemo {
    pub(crate) fn new() -> RepoMemo {
        RepoMemo {
            entries: HashMap::new(),
        }
    }

    /// Test-only view of the entry count (for the reclamation tests).
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// `cwd`'s repository at `tick`, via the memo. A warm hit costs one `stat` (two for a linked
/// worktree) and no read; anything that no longer matches re-resolves cold, which is where a
/// removed worktree's missing cwd turns into `None` (D16).
pub(crate) fn repo_of_cwd(memo: &mut RepoMemo, cwd: &Path, tick: u64) -> Option<RepoId> {
    let result = match repo_lookup(memo, cwd, tick) {
        Some(answer) => answer,
        None => repo_resolve(memo, cwd, tick),
    };
    reclaim_map(&mut memo.entries, tick, |e| e.last_read_tick);
    result
}

/// `Some(answer)` when the memo answered; `None` when a cold resolve is needed.
fn repo_lookup(memo: &mut RepoMemo, cwd: &Path, tick: u64) -> Option<Option<RepoId>> {
    let entry = memo.entries.get_mut(cwd)?;
    entry.last_read_tick = tick;
    match &entry.kind {
        RepoKind::Repo(r) => (still(&r.dot_git) && r.commondir.as_ref().is_none_or(still))
            .then(|| Some(r.id.clone())),
        RepoKind::None { checked_at } => {
            (tick.saturating_sub(*checked_at) < NEG_MEMO_TTL_TICKS).then_some(None)
        }
    }
}

/// Does `path` still `stat` to the stamp it was found with? One `stat`.
fn still((path, was): &(PathBuf, Stamp)) -> bool {
    meta_of(path).map(|m| stamp(&m)) == Some(*was)
}

fn repo_resolve(memo: &mut RepoMemo, cwd: &Path, tick: u64) -> Option<RepoId> {
    let (kind, id) = match resolve_repo(cwd) {
        Some(r) => {
            let id = r.id.clone();
            (RepoKind::Repo(r), Some(id))
        }
        None => (RepoKind::None { checked_at: tick }, None),
    };
    memo.entries.insert(
        cwd.to_path_buf(),
        RepoEntry {
            kind,
            last_read_tick: tick,
        },
    );
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- pure parser (R7.2, R7.3, R7.10, R11.4) ----

    #[test]
    fn parse_head_cases() {
        assert_eq!(
            parse_head("ref: refs/heads/main\n").as_deref(),
            Some("main")
        );
        // Slashes survive verbatim (R7.2).
        assert_eq!(
            parse_head("ref: refs/heads/feat/improving-tab-design\r\n").as_deref(),
            Some("feat/improving-tab-design")
        );
        // Detached HEAD → @ + first 7 (R7.3).
        assert_eq!(
            parse_head("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0\n").as_deref(),
            Some("@a1b2c3d")
        );
        // Empty / garbage / non-heads ref → None.
        assert_eq!(parse_head(""), None);
        assert_eq!(parse_head("not a head file"), None);
        assert_eq!(parse_head("ref: refs/remotes/origin/main"), None);
        // A detached SHA needs BOTH halves: forty characters AND every one hex. Hex alone
        // (an abbreviated or truncated SHA) and length alone (forty non-hex bytes) are not
        // a HEAD and must not render as `@…` (cargo mutants: `&&` → `||` survived).
        assert_eq!(parse_head("deadbeef\n"), None, "short hex is not a SHA");
        assert_eq!(
            parse_head(&format!("{}\n", "z".repeat(40))),
            None,
            "forty non-hex bytes are not a SHA"
        );
    }

    #[test]
    fn parse_head_caps_pathological_name() {
        // R7.10 — an unbounded branch name is capped at 64 chars.
        let huge = format!("ref: refs/heads/{}\n", "a".repeat(10_000));
        let got = parse_head(&huge).expect("still parses");
        assert_eq!(got.chars().count(), BRANCH_MAX);
    }

    // ---- filesystem behaviour via hand-rolled temp dirs (R11.2) ----

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("tabmeta-git-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("create tmpdir");
        p
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir -p");
        }
        std::fs::write(path, bytes).expect("write");
    }

    #[test]
    fn branch_from_repo_three_levels_up() {
        let _g = super::super::probe::serial();
        // R7.1 — cwd three dirs below the repo root reports the branch.
        let root = tmpdir("repo1");
        write(&root.join(".git/HEAD"), b"ref: refs/heads/main\n");
        let cwd = root.join("a/b/c");
        std::fs::create_dir_all(&cwd).expect("mkdir cwd");
        let mut memo = Memo::new();
        assert_eq!(branch_of_cwd(&mut memo, &cwd, 0).as_deref(), Some("main"));
    }

    #[test]
    fn gitdir_file_is_followed() {
        let _g = super::super::probe::serial();
        // R7.4 — a worktree whose `.git` is a `gitdir:` file.
        let root = tmpdir("worktree");
        // The real git dir, holding HEAD.
        write(
            &root.join("realgit/worktrees/wt1/HEAD"),
            b"ref: refs/heads/wt-branch\n",
        );
        // The worktree dir whose `.git` is a file pointing at it (relative).
        let wt = root.join("wt");
        std::fs::create_dir_all(&wt).expect("mkdir wt");
        write(&wt.join(".git"), b"gitdir: ../realgit/worktrees/wt1\n");
        let mut memo = Memo::new();
        assert_eq!(
            branch_of_cwd(&mut memo, &wt, 0).as_deref(),
            Some("wt-branch")
        );
    }

    #[test]
    fn non_repo_reports_none_and_short_circuits() {
        let _g = super::super::probe::serial();
        // R7.5 — no `.git`; the second tick reuses the NotARepo memo with no stat.
        let dir = tmpdir("norepo");
        let mut memo = Memo::new();
        assert_eq!(branch_of_cwd(&mut memo, &dir, 0), None);
        super::super::probe::reset_all();
        assert_eq!(branch_of_cwd(&mut memo, &dir, 1), None);
        assert_eq!(
            super::super::probe::GIT_STAT.get(),
            0,
            "fresh NotARepo must not re-walk"
        );
    }

    #[test]
    fn ancestor_walk_is_capped() {
        let _g = super::super::probe::serial();
        // R7.6 — a deep non-existent path examines at most MAX_GIT_ANCESTORS dirs.
        let mut deep = PathBuf::from("/tmp");
        for i in 0..60 {
            deep.push(format!("nope{i}"));
        }
        super::super::probe::reset_all();
        assert_eq!(resolve_head_path(&deep), None);
        assert!(
            super::super::probe::GIT_STAT.get() <= MAX_GIT_ANCESTORS as u64,
            "walk examined more than {MAX_GIT_ANCESTORS} ancestors"
        );
    }

    #[test]
    fn steady_state_repo_is_one_stat_no_read() {
        let _g = super::super::probe::serial();
        // R3.6 (git half) — a warm, unchanged repo costs one stat and zero reads.
        let root = tmpdir("steady");
        write(&root.join(".git/HEAD"), b"ref: refs/heads/main\n");
        let mut memo = Memo::new();
        assert_eq!(branch_of_cwd(&mut memo, &root, 0).as_deref(), Some("main"));
        super::super::probe::reset_all();
        assert_eq!(branch_of_cwd(&mut memo, &root, 1).as_deref(), Some("main"));
        assert_eq!(super::super::probe::GIT_STAT.get(), 1);
        assert_eq!(super::super::probe::GIT_READ.get(), 0);
    }

    #[test]
    fn checkout_changes_branch_via_mtime_len() {
        let _g = super::super::probe::serial();
        // R7.8 — rewriting HEAD (changing its length) is seen on the next tick.
        let root = tmpdir("checkout");
        write(&root.join(".git/HEAD"), b"ref: refs/heads/main\n");
        let mut memo = Memo::new();
        assert_eq!(branch_of_cwd(&mut memo, &root, 0).as_deref(), Some("main"));
        write(&root.join(".git/HEAD"), b"ref: refs/heads/develop\n");
        assert_eq!(
            branch_of_cwd(&mut memo, &root, 1).as_deref(),
            Some("develop")
        );
    }

    #[test]
    fn a_same_length_checkout_is_seen_through_the_mtime() {
        let _g = super::super::probe::serial();
        // R7.8 — `main` → `mai2` keeps HEAD's length, so only its mtime can reveal the
        // change: the revalidation must need BOTH to match, not either.
        let root = tmpdir("checkout-same-len");
        let head = root.join(".git/HEAD");
        write(&head, b"ref: refs/heads/main\n");
        let mut memo = Memo::new();
        assert_eq!(branch_of_cwd(&mut memo, &root, 0).as_deref(), Some("main"));
        write(&head, b"ref: refs/heads/mai2\n");
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&head)
            .and_then(|f| f.set_modified(later))
            .expect("set mtime");
        assert_eq!(branch_of_cwd(&mut memo, &root, 1).as_deref(), Some("mai2"));
    }

    #[test]
    fn git_init_in_memoised_non_repo_expires() {
        let _g = super::super::probe::serial();
        // R7.9 — a NotARepo entry expires, and a later `.git` is then found.
        let dir = tmpdir("init");
        let mut memo = Memo::new();
        assert_eq!(branch_of_cwd(&mut memo, &dir, 0), None);
        write(&dir.join(".git/HEAD"), b"ref: refs/heads/main\n");
        // Before expiry the negative memo still short-circuits.
        assert_eq!(branch_of_cwd(&mut memo, &dir, 1), None);
        // After NEG_MEMO_TTL_TICKS the walk re-runs and finds the repo.
        assert_eq!(
            branch_of_cwd(&mut memo, &dir, NEG_MEMO_TTL_TICKS).as_deref(),
            Some("main")
        );
    }

    #[test]
    fn head_read_is_capped() {
        let _g = super::super::probe::serial();
        // R7.10 — a pathological HEAD reads at most HEAD_READ_CAP bytes.
        let root = tmpdir("bighead");
        let big = format!("ref: refs/heads/{}\n", "a".repeat(20_000));
        write(&root.join(".git/HEAD"), big.as_bytes());
        let mut memo = Memo::new();
        super::super::probe::reset_all();
        let got = branch_of_cwd(&mut memo, &root, 0);
        assert!(super::super::probe::GIT_READ_BYTES.get() <= HEAD_READ_CAP);
        assert!(got.map(|s| s.chars().count()).unwrap_or(0) <= BRANCH_MAX);

        // Binary garbage HEAD → None, still capped.
        let root2 = tmpdir("bin");
        write(&root2.join(".git/HEAD"), &vec![0xffu8; 20_000]);
        super::super::probe::reset_all();
        assert_eq!(branch_of_cwd(&mut memo, &root2, 0), None);
        assert!(super::super::probe::GIT_READ_BYTES.get() <= HEAD_READ_CAP);
    }

    // ---- memo lifetime (R1.5, R1.6) ----

    #[test]
    fn memo_reclaims_by_disuse_not_removal() {
        let _g = super::super::probe::serial();
        // R1.6 — an entry untouched for MEMO_TTL_TICKS is dropped; a still-read entry
        // in the same pass survives (R1.5's shared-entry property).
        let a = tmpdir("reclaim-a");
        let b = tmpdir("reclaim-b");
        write(&a.join(".git/HEAD"), b"ref: refs/heads/aa\n");
        write(&b.join(".git/HEAD"), b"ref: refs/heads/bb\n");
        let mut memo = Memo::new();
        branch_of_cwd(&mut memo, &a, 0);
        branch_of_cwd(&mut memo, &b, 0);
        assert_eq!(memo.len(), 2);
        // Keep reading `a` far past b's TTL; b is untouched → reclaimed, a stays.
        branch_of_cwd(&mut memo, &a, MEMO_TTL_TICKS);
        assert_eq!(memo.len(), 1, "the untouched entry was reclaimed");
        assert_eq!(
            branch_of_cwd(&mut memo, &a, MEMO_TTL_TICKS).as_deref(),
            Some("aa")
        );
    }

    #[test]
    fn memo_is_capped_at_max() {
        let _g = super::super::probe::serial();
        // R1.6 — the map never exceeds MEMO_MAX entries.
        let mut memo = Memo::new();
        for i in 0..(MEMO_MAX + 20) {
            let d = tmpdir(&format!("cap{i}"));
            // All touched at the same tick so none is reclaimed by TTL.
            branch_of_cwd(&mut memo, &d, 0);
        }
        assert!(memo.len() <= MEMO_MAX, "memo grew past the cap");
    }

    // ---- repository identity (session-flow-graph R4.1–R4.5, D10, D16) ----

    /// A main repository at `<tmp>/r` with one linked worktree `w1` registered beside it
    /// (`<tmp>/r-w1`) and the same registration checked out inside it
    /// (`<tmp>/r/.claude/worktrees/w1`) — the two placements D10 measured.
    fn repo_with_worktree(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = tmpdir(tag);
        let r = base.join("r");
        write(&r.join(".git/HEAD"), b"ref: refs/heads/main\n");
        let g = r.join(".git/worktrees/w1");
        write(&g.join("HEAD"), b"ref: refs/heads/w1\n");
        write(&g.join("commondir"), b"../..\n");
        let gitdir = format!("gitdir: {}\n", g.display());
        let beside = base.join("r-w1");
        let inside = r.join(".claude/worktrees/w1");
        write(&beside.join(".git"), gitdir.as_bytes());
        write(&inside.join(".git"), gitdir.as_bytes());
        (r, beside, inside)
    }

    fn main_of(r: &Path, linked: bool) -> Option<RepoId> {
        Some(RepoId {
            common: r.join(".git"),
            main_root: r.to_path_buf(),
            linked,
        })
    }

    #[test]
    fn a_main_worktree_resolves_from_a_subdirectory() {
        let _g = super::super::probe::serial();
        // R4.1
        let (r, _, _) = repo_with_worktree("repo-main");
        let cwd = r.join("src/x");
        std::fs::create_dir_all(&cwd).expect("mkdir cwd");
        assert_eq!(repo_of(&cwd), main_of(&r, false));
        assert_eq!(repo_of(&r), main_of(&r, false));
    }

    #[test]
    fn a_linked_worktree_beside_or_inside_resolves_to_its_main() {
        let _g = super::super::probe::serial();
        // R4.2 — both placements, and a subdirectory of one; no `..` survives.
        let (r, beside, inside) = repo_with_worktree("repo-linked");
        assert_eq!(repo_of(&beside), main_of(&r, true));
        assert_eq!(repo_of(&inside), main_of(&r, true));
        let deep = inside.join("a/b");
        std::fs::create_dir_all(&deep).expect("mkdir deep");
        assert_eq!(repo_of(&deep), main_of(&r, true));
        let got = repo_of(&beside).expect("linked");
        assert!(!got
            .common
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir)));
    }

    #[test]
    fn a_relative_gitdir_and_an_absolute_commondir_both_resolve() {
        let _g = super::super::probe::serial();
        let base = tmpdir("repo-relgitdir");
        let r = base.join("r");
        let g = r.join(".git/worktrees/w2");
        write(
            &g.join("commondir"),
            r.join(".git").display().to_string().as_bytes(),
        );
        let wt = base.join("w2");
        write(&wt.join(".git"), b"gitdir: ../r/./.git/worktrees/w2\n");
        assert_eq!(repo_of(&wt), main_of(&r, true));
    }

    #[test]
    fn a_submodule_is_its_own_repository() {
        let _g = super::super::probe::serial();
        // R4.3
        let p = tmpdir("repo-submodule");
        std::fs::create_dir_all(p.join(".git/modules/s")).expect("mkdir modules");
        let s = p.join("s");
        write(&s.join(".git"), b"gitdir: ../.git/modules/s\n");
        assert_eq!(
            repo_of(&s),
            Some(RepoId {
                common: p.join(".git/modules/s"),
                main_root: s.clone(),
                linked: false,
            })
        );
    }

    #[test]
    fn no_repository_cases_are_none() {
        let _g = super::super::probe::serial();
        // R4.4 — no ancestor, missing cwd, malformed `.git`, unreadable / empty commondir.
        let plain = tmpdir("repo-none");
        assert_eq!(repo_of(&plain), None);
        assert_eq!(repo_of(&plain.join("gone")), None);

        let bad = tmpdir("repo-badfile");
        write(&bad.join(".git"), b"not a gitdir line\n");
        assert_eq!(repo_of(&bad), None);

        let (r, beside, _) = repo_with_worktree("repo-unreadable");
        let cd = r.join(".git/worktrees/w1/commondir");
        std::fs::remove_file(&cd).expect("rm commondir");
        std::fs::create_dir_all(&cd).expect("commondir as a directory: unreadable");
        assert_eq!(repo_of(&beside), None);

        let (_, beside, _) = repo_with_worktree("repo-emptycd");
        write(&beside.join("../r/.git/worktrees/w1/commondir"), b"  \n");
        assert_eq!(repo_of(&beside), None);
    }

    #[test]
    fn a_removed_inside_worktree_does_not_regroup_as_main() {
        let _g = super::super::probe::serial();
        // D10's measured trap: the ancestor walk from a deleted inside worktree would find
        // the main `.git` two levels up. The cwd-existence check stops it.
        let (_, _, inside) = repo_with_worktree("repo-removed-inside");
        std::fs::remove_dir_all(&inside).expect("rm worktree");
        assert_eq!(repo_of(&inside), None);
    }

    #[test]
    fn the_repo_walk_is_capped() {
        let _g = super::super::probe::serial();
        // R4.4 — a repository more than MAX_GIT_ANCESTORS levels up is not found.
        let base = tmpdir("repo-deep");
        write(&base.join(".git/HEAD"), b"ref: refs/heads/main\n");
        let mut deep = base.clone();
        for i in 0..MAX_GIT_ANCESTORS {
            deep.push(format!("d{i}"));
        }
        std::fs::create_dir_all(&deep).expect("mkdir deep");
        assert_eq!(repo_of(&deep), None, "41 levels down is out of reach");
        let one_up = deep.parent().expect("parent").to_path_buf();
        assert_eq!(
            repo_of(&one_up),
            main_of(&base, false),
            "40 levels is in reach"
        );
    }

    #[test]
    fn normalise_drops_dot_and_resolves_dotdot() {
        assert_eq!(normalise(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
        assert_eq!(normalise(Path::new("/a/b/../../..")), PathBuf::from("/"));
        assert_eq!(normalise(Path::new("/a/b")), PathBuf::from("/a/b"));
    }

    #[test]
    fn a_warm_linked_entry_costs_two_stats_and_notices_removal() {
        let _g = super::super::probe::serial();
        // R4.5
        let (r, beside, _) = repo_with_worktree("repo-memo-linked");
        let mut memo = RepoMemo::new();
        assert_eq!(repo_of_cwd(&mut memo, &beside, 0), main_of(&r, true));
        super::super::probe::reset_all();
        assert_eq!(repo_of_cwd(&mut memo, &beside, 1), main_of(&r, true));
        assert!(super::super::probe::GIT_STAT.get() <= 2);
        assert_eq!(super::super::probe::GIT_READ.get(), 0);
        std::fs::remove_dir_all(&beside).expect("rm worktree");
        assert_eq!(repo_of_cwd(&mut memo, &beside, 2), None);
    }

    #[test]
    fn a_warm_main_entry_costs_one_stat() {
        let _g = super::super::probe::serial();
        let (r, _, _) = repo_with_worktree("repo-memo-main");
        let mut memo = RepoMemo::new();
        assert_eq!(repo_of_cwd(&mut memo, &r, 0), main_of(&r, false));
        super::super::probe::reset_all();
        assert_eq!(repo_of_cwd(&mut memo, &r, 1), main_of(&r, false));
        assert_eq!(super::super::probe::GIT_STAT.get(), 1);
        assert_eq!(super::super::probe::GIT_READ.get(), 0);
        // A write inside `.git` moves the directory's mtime; that is not a change of identity.
        write(&r.join(".git/index.lock"), b"x");
        super::super::probe::reset_all();
        assert_eq!(repo_of_cwd(&mut memo, &r, 2), main_of(&r, false));
        assert_eq!(super::super::probe::GIT_READ.get(), 0, "still warm");
    }

    #[test]
    fn a_rewritten_gitdir_or_commondir_re_resolves() {
        let _g = super::super::probe::serial();
        // A warm entry compares the `.git` file's and `commondir`'s (mtime, len): a rewrite of
        // either is a cold resolve, not a stale answer.
        let (r, beside, _) = repo_with_worktree("repo-memo-rewrite");
        let mut memo = RepoMemo::new();
        assert_eq!(repo_of_cwd(&mut memo, &beside, 0), main_of(&r, true));
        // commondir now points one level higher.
        write(&r.join(".git/worktrees/w1/commondir"), b"../../..\n");
        assert_eq!(
            repo_of_cwd(&mut memo, &beside, 1).map(|id| id.common),
            Some(r.clone())
        );
        // The `.git` file now names a submodule-shaped dir (no commondir).
        let sub = r.join(".git/modules/m");
        std::fs::create_dir_all(&sub).expect("mkdir module dir");
        write(
            &beside.join(".git"),
            format!("gitdir: {}\n", sub.display()).as_bytes(),
        );
        assert_eq!(
            repo_of_cwd(&mut memo, &beside, 2),
            Some(RepoId {
                common: sub,
                main_root: beside.clone(),
                linked: false,
            })
        );
    }

    #[test]
    fn a_removed_commondir_is_a_miss() {
        let _g = super::super::probe::serial();
        // The second of the two warm stats is load-bearing on its own.
        let (r, beside, _) = repo_with_worktree("repo-memo-cdgone");
        let mut memo = RepoMemo::new();
        assert_eq!(repo_of_cwd(&mut memo, &beside, 0), main_of(&r, true));
        std::fs::remove_file(r.join(".git/worktrees/w1/commondir")).expect("rm commondir");
        assert_eq!(
            repo_of_cwd(&mut memo, &beside, 1).map(|id| id.linked),
            Some(false),
            "without commondir it reads as a submodule-shaped gitdir"
        );
    }

    #[test]
    fn a_repo_miss_is_memoised_then_expires() {
        let _g = super::super::probe::serial();
        let dir = tmpdir("repo-memo-neg");
        let mut memo = RepoMemo::new();
        assert_eq!(repo_of_cwd(&mut memo, &dir, 0), None);
        write(&dir.join(".git/HEAD"), b"ref: refs/heads/main\n");
        super::super::probe::reset_all();
        assert_eq!(repo_of_cwd(&mut memo, &dir, NEG_MEMO_TTL_TICKS - 1), None);
        assert_eq!(
            super::super::probe::GIT_STAT.get(),
            0,
            "fresh miss: no walk"
        );
        assert_eq!(
            repo_of_cwd(&mut memo, &dir, NEG_MEMO_TTL_TICKS),
            main_of(&dir, false)
        );
    }

    #[test]
    fn the_repo_memo_reclaims_by_disuse_and_is_capped() {
        let _g = super::super::probe::serial();
        let a = tmpdir("repo-reclaim-a");
        let b = tmpdir("repo-reclaim-b");
        let mut memo = RepoMemo::new();
        repo_of_cwd(&mut memo, &a, 0);
        repo_of_cwd(&mut memo, &b, 0);
        assert_eq!(memo.len(), 2);
        repo_of_cwd(&mut memo, &a, MEMO_TTL_TICKS - 1);
        assert_eq!(memo.len(), 2, "b is one tick short of its TTL");
        repo_of_cwd(&mut memo, &a, MEMO_TTL_TICKS);
        assert_eq!(memo.len(), 1, "the untouched entry was reclaimed");
        let mut memo = RepoMemo::new();
        for i in 0..(MEMO_MAX + 5) {
            repo_of_cwd(&mut memo, &tmpdir(&format!("repo-cap{i}")), i as u64);
        }
        assert_eq!(memo.len(), MEMO_MAX, "capped, oldest evicted");
        assert!(memo
            .entries
            .contains_key(&tmpdir_path(&format!("repo-cap{}", MEMO_MAX + 4))));
        assert!(!memo.entries.contains_key(&tmpdir_path("repo-cap0")));
    }

    fn tmpdir_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tabmeta-git-{}-{}", std::process::id(), tag))
    }
}
