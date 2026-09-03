#!/usr/bin/env python3
"""Gate 2: which of spec.md's sub-requirements are backed by an executable test.

**Scoped deliberately.** This repo numbers requirements per feature, so `R5.6` exists in
several specs and a bare name search happily matches another feature's test — which is
exactly how an earlier version of this script reported 65/67 while five sub-requirements
had no backing at all. So a match must come from a file this feature actually touched,
and only from a test name: a citation inside a non-test doc comment is prose, which is
the thing the gate exists to distinguish.
"""
import io, re, subprocess, sys, pathlib

ROOT = pathlib.Path(__file__).resolve().parents[3]

# The files this feature changed, read off the working tree rather than hand-listed — a
# hand-list drifts, and it drifted: `terminal_app.rs` (where the wheel routing lives) was
# missing, so a test added there would not have counted.
def _owned():
    """Files this feature touched — from the working tree, else from the branch."""
    porcelain = subprocess.run(
        ["git", "status", "--porcelain", "--", "crates"],
        capture_output=True, text=True, cwd=ROOT,
    ).stdout
    files = [l[3:].strip() for l in porcelain.split("\n") if l.strip().endswith(".rs")]
    if files:
        return files
    # A clean tree means the work is committed, and this gate should still be runnable
    # afterwards — otherwise it reports 0/67 and hard-fails for the first person to check it
    # after a merge.
    return [
        f
        for f in subprocess.run(
            ["git", "diff", "--name-only", "main...HEAD", "--", "crates"],
            capture_output=True, text=True, cwd=ROOT,
        ).stdout.split("\n")
        if f.strip().endswith(".rs")
    ]


OWNED = _owned()

spec = io.open(ROOT / "specs/ai-monitor-panel-scroll/spec.md", encoding="utf-8").read()
subs = re.findall(r"^#### (R\d+\.\d+):", spec, re.M)

# Test names THIS FEATURE ADDED, read off the diff.
#
# Scoping by file is not enough: `dashboard.rs` carries several features' tests, so a
# file-scoped search still matched `r5_6_the_file_verb_carries_the_designs_role` from the
# original ai-monitor spec. Only a name the diff introduced can honestly back a
# sub-requirement of this spec.
tracked = [p for p in OWNED if (ROOT / p).exists() and subprocess.run(
    ["git", "ls-files", "--error-unmatch", p],
    capture_output=True, cwd=ROOT).returncode == 0]
untracked = [p for p in OWNED if p not in tracked]
def _diff(paths):
    """The change this feature is: uncommitted if there is any, else the branch.

    Both halves are needed, and an earlier version had only the first. Before the commit
    the work lives in the working tree; after it, that diff is empty and a working-tree-only
    gate reports 0 backed and hard-fails for the first person to check it post-merge — while
    *looking* like a coverage collapse rather than a broken measurement.
    """
    if not paths:
        return ""
    worktree = subprocess.run(
        ["git", "diff", "--unified=0", "--"] + paths,
        capture_output=True, text=True, cwd=ROOT,
    ).stdout
    if worktree.strip():
        return worktree
    return subprocess.run(
        ["git", "diff", "--unified=0", "main...HEAD", "--"] + paths,
        capture_output=True, text=True, cwd=ROOT,
    ).stdout


diff = _diff(tracked)
# A file this feature CREATED is entirely new, and `git diff` says nothing about it.
for rel in untracked:
    diff += "".join("+" + l + "\n" for l in io.open(ROOT / rel, encoding="utf-8").read().split("\n"))
added = set(re.findall(r"^\+\s*fn ([a-z0-9_]+)\(\)", diff, re.M))
# …and it must actually be a runnable test. Matching any added zero-argument `fn` counted
# helpers (`scrollable`, `filled`) and would have counted an `#[ignore]`d test too — the gate
# is supposed to prove something RUNS, so intersect with what the harness reports.
runnable = {
    n.strip().split("::")[-1]
    for n in subprocess.run(
        ["bash", "-lc", "cargo test --workspace -- --list 2>/dev/null | grep ': test' | sed 's/: test//'"],
        capture_output=True, text=True, cwd=ROOT,
    ).stdout.split("\n")
    if n.strip()
}
tests = [(m, "diff") for m in sorted(added & runnable)]

def backed(sr):
    n, m = sr[1:].split(".")
    tok = f"r{n}_{m}"
    for name, rel in tests:
        if re.search(rf"(^|_){tok}(_|$)", name):
            return f"{rel}::{name}"
        rng = re.match(r"r(\d+)_(\d+)_to_r(\d+)_(\d+)", name)
        if rng and rng.group(1) == n == rng.group(3) and int(rng.group(2)) <= int(m) <= int(rng.group(4)):
            return f"{rel}::{name}"
    return None

missing = [s for s in subs if not backed(s)]
print(f"sub-requirements: {len(subs)}")
print(f"backed:           {len(subs) - len(missing)}")
print(f"unbacked:         {len(missing)}  {missing}")
allowed = {"R12.1", "R12.2"}
bad = [x for x in missing if x not in allowed]
if bad:
    print(f"GATE 2 FAIL — unbacked beyond the two declared diff-facts: {bad}")
    sys.exit(1)
print("GATE 2 PASS")
