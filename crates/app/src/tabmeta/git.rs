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

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
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
        self.entries
            .retain(|_, e| tick.saturating_sub(e.last_read_tick) < MEMO_TTL_TICKS);
        while self.entries.len() > MEMO_MAX {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_read_tick)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    self.entries.remove(&k);
                }
                None => break,
            }
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
}
