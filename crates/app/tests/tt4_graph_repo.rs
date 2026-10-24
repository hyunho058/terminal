//! TT4 — acceptance tests for `specs/session-flow-graph` R4 (D10, D16): repository
//! identity resolved from a session's cwd, and the worker's graph-readiness gate.
//!
//! R4.5 (memo `stat` cost) and R4.6 (field locks) are T2's in-crate unit tests, not here.
//!
//! Every git layout is written by hand under a unique temp directory — no `git` binary.
//! The temp root is canonicalised before anything is built under it (macOS `/var` is a
//! symlink to `/private/var`), so the expected paths are the same whether `repo_of`
//! normalises lexically or by `canonicalize`.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use app::{graph_pass, publish_graph_state, repo_of, PanePass, RepoId};

/// A unique, canonicalised temp directory, removed on drop.
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(tag: &str) -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let name = format!(
            "tt4-graph-repo-{tag}-{}-{nanos}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        );
        let raw = std::env::temp_dir().join(name);
        fs::create_dir_all(&raw).expect("create temp root");
        let canonical = fs::canonicalize(&raw).expect("canonicalise temp root");
        TempRoot(canonical)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        // Restore permissions first, in case a test made a file unreadable.
        restore_perms(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn restore_perms(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if let Ok(meta) = fs::symlink_metadata(&p) {
                if meta.is_dir() {
                    restore_perms(&p);
                } else if meta.is_file() {
                    let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o644));
                }
            }
        }
    }
}

fn mkdir(p: &Path) {
    fs::create_dir_all(p).expect("mkdir");
}

fn write(p: &Path, contents: &str) {
    if let Some(parent) = p.parent() {
        mkdir(parent);
    }
    fs::write(p, contents).expect("write");
}

/// A main repository at `r`: `r/.git/` as a directory with a HEAD.
fn main_repo(r: &Path) {
    write(&r.join(".git/HEAD"), "ref: refs/heads/main\n");
    mkdir(&r.join(".git/refs/heads"));
}

/// Register linked worktree `name` in main repo `r` and check out its directory at `wt`:
/// `r/.git/worktrees/<name>/{HEAD,commondir,gitdir}` and `wt/.git` = `gitdir: <abs>`.
fn linked_worktree(r: &Path, name: &str, wt: &Path) -> PathBuf {
    let g = r.join(".git/worktrees").join(name);
    write(&g.join("HEAD"), &format!("ref: refs/heads/{name}\n"));
    write(&g.join("commondir"), "../..\n");
    write(
        &g.join("gitdir"),
        &format!("{}\n", wt.join(".git").display()),
    );
    write(&wt.join(".git"), &format!("gitdir: {}\n", g.display()));
    g
}

fn has_no_parent_dir(p: &Path) -> bool {
    p.components().all(|c| c != Component::ParentDir)
}

fn assert_repo(cwd: &Path, got: Option<RepoId>, common: &Path, main_root: &Path, linked: bool) {
    let id = got.unwrap_or_else(|| panic!("repo_of({}) returned None", cwd.display()));
    assert_eq!(id.common, common, "common for {}", cwd.display());
    assert_eq!(id.main_root, main_root, "main_root for {}", cwd.display());
    assert_eq!(id.linked, linked, "linked for {}", cwd.display());
    assert!(
        has_no_parent_dir(&id.common),
        "common keeps a `..`: {}",
        id.common.display()
    );
    assert!(
        has_no_parent_dir(&id.main_root),
        "main_root keeps a `..`: {}",
        id.main_root.display()
    );
}

fn assert_none(cwd: &Path, got: Option<RepoId>, case: &str) {
    if let Some(id) = got {
        panic!(
            "{case}: repo_of({}) = Some(common: {}, main_root: {}, linked: {}), want None",
            cwd.display(),
            id.common.display(),
            id.main_root.display(),
            id.linked
        );
    }
}

// R4.1
#[test]
fn r4_1_main_worktree_resolves_to_its_git_dir_and_parent() {
    let tmp = TempRoot::new("r4-1");
    let r = tmp.path().join("r");
    main_repo(&r);
    let cwd = r.join("src/x");
    mkdir(&cwd);

    assert_repo(&cwd, repo_of(&cwd), &r.join(".git"), &r, false);
}

// R4.2 — the worktree beside the main repository.
#[test]
fn r4_2_linked_worktree_beside_main_resolves_to_main_repo() {
    let tmp = TempRoot::new("r4-2-beside");
    let r = tmp.path().join("r");
    main_repo(&r);
    let wt = tmp.path().join("r-w1");
    linked_worktree(&r, "w1", &wt);

    assert_repo(&wt, repo_of(&wt), &r.join(".git"), &r, true);
}

// R4.2 — the worktree inside the main repository (`r/.claude/worktrees/w1`).
#[test]
fn r4_2_linked_worktree_inside_main_resolves_to_main_repo() {
    let tmp = TempRoot::new("r4-2-inside");
    let r = tmp.path().join("r");
    main_repo(&r);
    let wt = r.join(".claude/worktrees/w1");
    linked_worktree(&r, "w1", &wt);

    assert_repo(&wt, repo_of(&wt), &r.join(".git"), &r, true);
}

// R4.2 — both worktrees share one `gitdir` target, as the Given states.
#[test]
fn r4_2_linked_worktrees_beside_and_inside_share_one_identity() {
    let tmp = TempRoot::new("r4-2-both");
    let r = tmp.path().join("r");
    main_repo(&r);
    let beside = tmp.path().join("r-w1");
    let inside = r.join(".claude/worktrees/w1");
    let g = linked_worktree(&r, "w1", &beside);
    // The second worktree's `.git` file reads the same `gitdir: <r>/.git/worktrees/w1`.
    write(&inside.join(".git"), &format!("gitdir: {}\n", g.display()));

    for wt in [&beside, &inside] {
        assert_repo(wt, repo_of(wt), &r.join(".git"), &r, true);
    }
}

// R4.3
#[test]
fn r4_3_submodule_is_its_own_repository() {
    let tmp = TempRoot::new("r4-3");
    let parent = tmp.path().join("p");
    main_repo(&parent);
    let modules_s = parent.join(".git/modules/s");
    write(&modules_s.join("HEAD"), "ref: refs/heads/main\n");
    assert!(
        !modules_s.join("commondir").exists(),
        "precondition: no commondir"
    );
    let s = parent.join("s");
    write(&s.join(".git"), "gitdir: ../.git/modules/s\n");

    assert_repo(&s, repo_of(&s), &modules_s, &s, false);
}

// R4.4 — no `.git` ancestor within 40 levels (the nearest `.git` is 41 levels above
// the cwd's parent, out of reach of the bounded walk).
#[test]
fn r4_4_no_git_ancestor_within_forty_levels_is_none() {
    let tmp = TempRoot::new("r4-4-deep");
    let top = tmp.path().join("top");
    main_repo(&top);
    let mut cwd = top.clone();
    for i in 0..41 {
        cwd.push(format!("d{i}"));
    }
    mkdir(&cwd);

    assert_none(&cwd, repo_of(&cwd), "no .git within 40 levels");
}

// R4.4 — a cwd that does not exist, even though an ancestor walk would reach a `.git`.
#[test]
fn r4_4_nonexistent_cwd_is_none() {
    let tmp = TempRoot::new("r4-4-gone");
    let r = tmp.path().join("r");
    main_repo(&r);
    let cwd = r.join(".claude/worktrees/removed");
    assert!(!cwd.exists(), "precondition: cwd does not exist");

    assert_none(&cwd, repo_of(&cwd), "nonexistent cwd");
}

// R4.4 — a `.git` file with no `gitdir:` line.
#[test]
fn r4_4_git_file_without_gitdir_line_is_none() {
    let tmp = TempRoot::new("r4-4-nogitdir");
    let d = tmp.path().join("d");
    write(&d.join(".git"), "this is not a gitdir file\n");

    assert_none(&d, repo_of(&d), ".git file with no gitdir: line");
}

// R4.4 — a linked worktree whose `commondir` exists but cannot be read.
#[test]
fn r4_4_linked_worktree_with_unreadable_commondir_is_none() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempRoot::new("r4-4-commondir");
    let r = tmp.path().join("r");
    main_repo(&r);
    let wt = tmp.path().join("r-w1");
    let g = linked_worktree(&r, "w1", &wt);
    let commondir = g.join("commondir");
    fs::set_permissions(&commondir, fs::Permissions::from_mode(0o000)).expect("chmod 000");
    assert!(
        fs::read(&commondir).is_err(),
        "precondition: commondir must be unreadable (running as root?)"
    );

    assert_none(&wt, repo_of(&wt), "unreadable commondir");
}

// R4.7 — `publish_graph_state` over all eight input combinations.
#[test]
fn r4_7_publish_graph_state_over_all_eight_combinations() {
    for wanted in [false, true] {
        for full_pass_done in [false, true] {
            for prev_ready in [false, true] {
                let p = publish_graph_state(wanted, full_pass_done, prev_ready);
                let at = format!(
                    "wanted={wanted}, full_pass_done={full_pass_done}, prev_ready={prev_ready}"
                );
                if !wanted {
                    assert!(!p.ready, "not wanted must not be ready ({at})");
                    assert!(p.clear, "not wanted must clear ({at})");
                } else if full_pass_done {
                    assert!(p.ready, "a full pass while wanted must be ready ({at})");
                    assert!(!p.clear, "a full pass while wanted must not clear ({at})");
                } else {
                    assert_eq!(p.ready, prev_ready, "a partial pass keeps readiness ({at})");
                    assert!(
                        !p.clear,
                        "a partial pass while wanted must not clear ({at})"
                    );
                }
            }
        }
    }
}

// R4.7 — `graph_pass` keeps session-flow-viewer R6.6's eight-row table.
#[test]
fn r4_7_graph_pass_keeps_the_eight_row_table() {
    for just_enabled in [false, true] {
        for due in [false, true] {
            assert_eq!(
                graph_pass(false, just_enabled, due),
                PanePass::Skip,
                "not wanted must skip (just_enabled={just_enabled}, due={due})"
            );
        }
    }
    for due in [false, true] {
        assert_eq!(
            graph_pass(true, true, due),
            PanePass::Derive,
            "just enabled must derive regardless of due={due}"
        );
    }
    assert_eq!(graph_pass(true, false, true), PanePass::Derive);
    assert_eq!(graph_pass(true, false, false), PanePass::Skip);
}
