//! Agent presence + identity: a bounded breadth-first walk of a tab's descendant
//! processes (D6), each node classified by executable basename first, then its leading
//! dot-component (an update-renamed binary like `agy.<id>.old`), then the
//! `<agent>/versions` native-install signature, then — *only* for known interpreters —
//! its argv (D5).
//!
//! [`walk`] is generic over a children-provider and a classifier closure (D13(2)) so
//! every bound is tested against hand-built fake trees with no syscall. The two
//! classifiers ([`classify_exe`], [`classify_argv`]) and the interpreter gate are pure
//! (D13(3)). Only [`agent_of_pid`] and [`classify_pid`] touch the real kernel, through
//! T2's `proc` primitives — so the `sysctl` in `proc_args` fires only when the gate
//! opens (R4.4), which is what keeps a build tree of `rustc` from costing 64 sysctls.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Component, Path};

use super::AgentKind;

/// Levels below the tab's zsh the walk descends (D6). `zsh → claude` is depth 1;
/// `zsh → node → claude` and `zsh → npx → claude` are depth 2; a shell-function
/// wrapper adds one more. At depth 4+ you are inside the agent's own subprocess tree.
const MAX_DEPTH: u32 = 3;

/// Hard cap on pids classified per walk (D6). The realistic worst case (an agent plus
/// a build) terminates well inside it; a pathological tree cannot make one tick unbounded.
const MAX_NODES: usize = 64;

/// Stage 1: executable basename → `AgentKind`, exact and case-insensitive (D5). A
/// closed table (R4.6): a process is an agent iff it is here.
const AGENT_TABLE: &[(&str, AgentKind)] = &[
    ("claude", AgentKind::Claude),
    ("antigravity", AgentKind::Antigravity),
    // `agy` is Antigravity's own short launcher — the name it actually runs under in a
    // terminal (`~/.local/bin/agy`), so it must be recognised like the long form.
    ("agy", AgentKind::Antigravity),
    ("codex", AgentKind::Codex),
    ("gemini", AgentKind::Gemini),
    ("aider", AgentKind::Aider),
];

/// The only basenames for which stage 2 reads argv (D5). Gating on this table is the
/// D6 × D5 resolution: a non-interpreter descendant costs zero `sysctl` calls.
const INTERP_TABLE: &[&str] = &["node", "bun", "deno", "python3", "python"];

/// Stage 2: a path component of an interpreter's argv matching one of these means that
/// agent (D5) — e.g. `node …/@anthropic-ai/claude-code/cli.js` is `Claude`.
const ARGV_TABLE: &[(&str, AgentKind)] = &[
    ("claude", AgentKind::Claude),
    ("claude-code", AgentKind::Claude),
    ("antigravity", AgentKind::Antigravity),
    ("agy", AgentKind::Antigravity),
    ("codex", AgentKind::Codex),
    ("gemini", AgentKind::Gemini),
    ("aider", AgentKind::Aider),
];

/// Stage 1 (pure): an executable basename → its `AgentKind`, or `None` for a near-miss
/// like `claude-foo` (R4.2). Case-insensitive, exact.
pub(crate) fn classify_exe(basename: &str) -> Option<AgentKind> {
    AGENT_TABLE
        .iter()
        .find(|(name, _)| basename.eq_ignore_ascii_case(name))
        .map(|(_, kind)| *kind)
}

/// Whether a basename is an interpreter whose argv is worth reading (R4.4's gate).
fn is_interpreter(basename: &str) -> bool {
    INTERP_TABLE
        .iter()
        .any(|n| basename.eq_ignore_ascii_case(n))
}

/// Stage 2 (pure): scan an argv for a path component naming a known agent (R4.3). First
/// match wins, scanning arguments in order and components left to right.
pub(crate) fn classify_argv(args: &[OsString]) -> Option<AgentKind> {
    for arg in args {
        let s = arg.to_string_lossy();
        for comp in s.split('/') {
            if let Some((_, kind)) = ARGV_TABLE
                .iter()
                .find(|(name, _)| comp.eq_ignore_ascii_case(name))
            {
                return Some(*kind);
            }
        }
    }
    None
}

/// Stage 1b (pure): recognise a native **versioned install**, where `proc_pidpath` reports
/// the versioned binary so the basename is a version string (e.g. `2.1.215`) rather than
/// the agent name. Matches an agent-name path component immediately followed by a
/// `versions` component — the layout the `claude` native installer uses
/// (`~/.local/share/claude/versions/<ver>`). Kept specific to `<agent>/versions` so a home
/// directory named after an agent (`/Users/claude/…`) is NOT mistaken for a running agent,
/// and needs no extra syscall (the path is already in hand).
fn classify_install_path(path: &Path) -> Option<AgentKind> {
    let comps: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    comps.windows(2).find_map(|w| {
        w[1].eq_ignore_ascii_case("versions")
            .then(|| classify_exe(&w[0]))
            .flatten()
    })
}

/// The staged classifier, pure over its inputs so the `sysctl` gate is testable without a
/// process (R4.4): stage 1 is the executable basename, stage 1a its leading component (for
/// an update-renamed binary), stage 1b the `<agent>/versions` install signature — all
/// syscall-free — and only then, for a known interpreter basename, is `read_argv` invoked.
/// A failed argv read (empty) means "not an agent" (R4.5).
pub(crate) fn classify_with(
    exe_path: Option<&Path>,
    read_argv: impl FnOnce() -> Vec<OsString>,
) -> Option<AgentKind> {
    let path = exe_path?;
    let base = path.file_name().map(|s| s.to_string_lossy());
    // Stage 1: exact executable basename.
    if let Some(kind) = base.as_deref().and_then(classify_exe) {
        return Some(kind);
    }
    // Stage 1a: an in-place auto-updater renames the running binary to `<name>.<id>.old`
    // (e.g. `agy.1784614720971820000.old`), so a long-running session's `proc_pidpath`
    // basename carries a suffix. The agent name survives as the leading dot-component.
    if let Some(kind) = base
        .as_deref()
        .and_then(|b| b.split_once('.'))
        .and_then(|(lead, _)| classify_exe(lead))
    {
        return Some(kind);
    }
    // Stage 1b: the versioned-install signature (basename is a version string).
    if let Some(kind) = classify_install_path(path) {
        return Some(kind);
    }
    // Stage 2: only a known interpreter's argv is read (the `sysctl` gate).
    if base.as_deref().is_some_and(is_interpreter) {
        return classify_argv(&read_argv());
    }
    None
}

/// Breadth-first, bounded, early-exit walk (D6). Generic over `children` and `classify`
/// so R5.1–R5.4 test every bound against a fake tree. The root (the tab's zsh) is not
/// itself classified — only its descendants, from depth 1. Returns the first match.
pub(crate) fn walk(
    root: u32,
    children: impl Fn(u32) -> Vec<u32>,
    classify: impl Fn(u32) -> Option<AgentKind>,
) -> Option<AgentKind> {
    let mut queue: VecDeque<(u32, u32)> = VecDeque::new();
    for child in children(root) {
        queue.push_back((child, 1));
    }
    let mut visited = 0usize;
    while let Some((pid, depth)) = queue.pop_front() {
        if visited >= MAX_NODES {
            break; // node budget exhausted (R5.2)
        }
        visited += 1;
        if let Some(kind) = classify(pid) {
            return Some(kind); // first match short-circuits (R5.3)
        }
        // A dead pid's `children` is empty; the queue simply carries on (R5.4).
        if depth < MAX_DEPTH {
            for child in children(pid) {
                queue.push_back((child, depth + 1));
            }
        }
    }
    None
}

/// Production entry point: classify the descendants of `pid` using T2's real syscalls.
/// A dead root pid yields `None` (its child list is empty), never a panic (R5.6).
pub(crate) fn agent_of_pid(pid: u32) -> Option<AgentKind> {
    walk(pid, super::proc::children_of, classify_pid)
}

/// Classify one live pid: `proc_pidpath` (basename + `<agent>/versions` signature) first,
/// then — only for an interpreter — its argv via `sysctl`. The `sysctl` (in `proc_args`)
/// is reached solely through the `is_interpreter` gate inside `classify_with` (R4.4).
fn classify_pid(pid: u32) -> Option<AgentKind> {
    let path = super::proc::exe_path(pid);
    classify_with(path.as_deref(), || super::proc::proc_args(pid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    // ---- stage 1 / stage 2 pure classifiers (R4.1–R4.3, R11.4) ----

    #[test]
    fn classify_exe_is_case_insensitive_and_exact() {
        // R4.2 — the first two match; the last three are near-misses, not agents.
        assert_eq!(classify_exe("Claude"), Some(AgentKind::Claude));
        assert_eq!(classify_exe("CLAUDE"), Some(AgentKind::Claude));
        assert_eq!(classify_exe("claude-foo"), None);
        assert_eq!(classify_exe("myclaude"), None);
        assert_eq!(classify_exe("node"), None);
        // Every table entry resolves.
        assert_eq!(classify_exe("antigravity"), Some(AgentKind::Antigravity));
        assert_eq!(classify_exe("agy"), Some(AgentKind::Antigravity));
        assert_eq!(classify_exe("AGY"), Some(AgentKind::Antigravity));
        assert_eq!(classify_exe("codex"), Some(AgentKind::Codex));
        assert_eq!(classify_exe("gemini"), Some(AgentKind::Gemini));
        assert_eq!(classify_exe("aider"), Some(AgentKind::Aider));
    }

    #[test]
    fn classify_argv_finds_packaged_agent() {
        // R4.3 — the npm install of Claude Code, launched via node.
        let argv = vec![
            OsString::from("node"),
            OsString::from("/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js"),
        ];
        assert_eq!(classify_argv(&argv), Some(AgentKind::Claude));
        // A bare component also matches.
        let bare = vec![OsString::from("python3"), OsString::from("aider")];
        assert_eq!(classify_argv(&bare), Some(AgentKind::Aider));
        // No known component → None.
        let none = vec![OsString::from("node"), OsString::from("server.js")];
        assert_eq!(classify_argv(&none), None);
    }

    // ---- the two-stage gate (R4.1, R4.4, R4.5) ----

    #[test]
    fn stage1_hit_never_reads_argv() {
        // R4.1 — a direct `claude` is classified without touching argv.
        let mut called = false;
        let got = classify_with(Some(Path::new("/usr/bin/claude")), || {
            called = true;
            Vec::new()
        });
        assert_eq!(got, Some(AgentKind::Claude));
        assert!(!called, "stage 1 hit must not read argv");
    }

    #[test]
    fn native_versioned_install_is_detected_without_argv() {
        // Bug 2 — the `.local` installer runs `.../claude/versions/<ver>`, so proc_pidpath's
        // basename is a version string; stage 1b recognises it with no argv read.
        let mut called = false;
        let got = classify_with(
            Some(Path::new("/Users/x/.local/share/claude/versions/2.1.215")),
            || {
                called = true;
                Vec::new()
            },
        );
        assert_eq!(got, Some(AgentKind::Claude));
        assert!(!called, "the install signature must not read argv");
    }

    #[test]
    fn antigravity_agy_launcher_is_detected_including_update_rename() {
        // The Antigravity CLI runs as `agy` (`~/.local/bin/agy`) — a fresh session's
        // basename is `agy`.
        assert_eq!(
            classify_with(Some(Path::new("/Users/x/.local/bin/agy")), Vec::new),
            Some(AgentKind::Antigravity),
        );
        // A long-running session whose binary was renamed in place by the auto-updater
        // (`agy.<id>.old`) is still detected via the leading dot-component (stage 1a) —
        // this is the exact `proc_pidpath` shape observed for the live process.
        assert_eq!(
            classify_with(
                Some(Path::new("/Users/x/.local/bin/agy.1784614720971820000.old")),
                Vec::new
            ),
            Some(AgentKind::Antigravity),
        );
        // A near-miss leading component is not an agent (exactness preserved).
        assert_eq!(
            classify_with(Some(Path::new("/usr/bin/agyx.7.old")), Vec::new),
            None,
        );
    }

    #[test]
    fn install_signature_is_specific_to_agent_slash_versions() {
        // `/Users/claude/…` has a `claude` component NOT followed by `versions` → no match,
        // so a home dir named after an agent is never mistaken for a running agent.
        assert_eq!(
            classify_with(
                Some(Path::new("/Users/claude/proj/target/debug/tool")),
                Vec::new
            ),
            None,
        );
        // The `<agent>/versions` adjacency matches even when deeper in the path…
        assert_eq!(
            classify_install_path(Path::new("/opt/antigravity/versions/9/bin/x")),
            Some(AgentKind::Antigravity),
        );
        // …but a bare `versions` dir with no agent name before it does not.
        assert_eq!(classify_install_path(Path::new("/x/versions/1")), None);
    }

    #[test]
    fn sysctl_gate_closed_for_non_interpreters() {
        let _g = super::super::probe::serial();
        // R4.4 — non-interpreter basenames never invoke the argv reader, so the real
        // path never calls `proc_args` and `probe::PROC_ARGS` stays 0.
        super::super::probe::reset_all();
        for base in ["rustc", "cc", "ld"] {
            // The closure would bump PROC_ARGS if the gate opened — it must not.
            let got = classify_with(Some(Path::new(base)), || super::super::proc::proc_args(0));
            assert_eq!(got, None);
        }
        assert_eq!(
            super::super::probe::PROC_ARGS.get(),
            0,
            "no sysctl for non-interpreters"
        );
    }

    #[test]
    fn interpreter_with_failed_argv_is_not_an_agent() {
        // R4.5 — a `node` whose argv read fails (empty) yields None.
        let got = classify_with(Some(Path::new("/usr/local/bin/node")), Vec::new);
        assert_eq!(got, None);
    }

    // ---- the bounded walk, syscall-free (R5.1–R5.4, R11.1) ----

    fn children_of(map: &HashMap<u32, Vec<u32>>) -> impl Fn(u32) -> Vec<u32> + '_ {
        move |pid| map.get(&pid).cloned().unwrap_or_default()
    }

    #[test]
    fn walk_stops_at_max_depth() {
        // R5.1 — a match 4 levels below the root is never reached (MAX_DEPTH = 3).
        let tree: HashMap<u32, Vec<u32>> = [(1, vec![2]), (2, vec![3]), (3, vec![4]), (4, vec![5])]
            .into_iter()
            .collect();
        let matches: HashMap<u32, AgentKind> = [(5, AgentKind::Claude)].into_iter().collect();
        let got = walk(1, children_of(&tree), |pid| matches.get(&pid).copied());
        assert_eq!(got, None);
    }

    #[test]
    fn walk_stops_at_max_nodes() {
        // R5.2 — 200 non-matching children with a match beyond the 64th → None.
        let level1: Vec<u32> = (2..202).collect();
        let tree: HashMap<u32, Vec<u32>> = [(1, level1)].into_iter().collect();
        // Match at the 71st child (pid 72), beyond the 64-node budget.
        let matches: HashMap<u32, AgentKind> = [(72, AgentKind::Claude)].into_iter().collect();
        let got = walk(1, children_of(&tree), |pid| matches.get(&pid).copied());
        assert_eq!(got, None);
    }

    #[test]
    fn walk_short_circuits_on_match() {
        // R5.3 — a match at the root's first child returns before the second subtree.
        let tree: HashMap<u32, Vec<u32>> = [(1, vec![2, 3]), (3, vec![100]), (100, vec![101])]
            .into_iter()
            .collect();
        let matches: HashMap<u32, AgentKind> = [(2, AgentKind::Claude)].into_iter().collect();
        let visited = RefCell::new(Vec::new());
        let got = walk(1, children_of(&tree), |pid| {
            visited.borrow_mut().push(pid);
            matches.get(&pid).copied()
        });
        assert_eq!(got, Some(AgentKind::Claude));
        assert!(
            !visited.borrow().contains(&100),
            "second subtree must not be visited"
        );
    }

    #[test]
    fn walk_skips_dead_node_and_continues() {
        // R5.4 — the second sibling has no children (a dead pid); the third matches.
        let tree: HashMap<u32, Vec<u32>> = [(1, vec![2, 3, 4])].into_iter().collect();
        // pid 3 intentionally absent from the tree → empty child list.
        let matches: HashMap<u32, AgentKind> = [(4, AgentKind::Codex)].into_iter().collect();
        let got = walk(1, children_of(&tree), |pid| matches.get(&pid).copied());
        assert_eq!(got, Some(AgentKind::Codex));
    }

    #[test]
    fn walk_with_no_children_is_none() {
        // R4.6 — a plain zsh with no children reports no agent.
        let got = walk(1, |_| Vec::new(), |_| None);
        assert_eq!(got, None);
    }

    // ---- R4.7: the real syscalls resolve against our own process ----

    #[test]
    fn agent_of_self_does_not_panic() {
        let _g = super::super::probe::serial();
        // The test binary is not an agent; the point is the walk runs the real syscalls
        // against a live pid without panicking (R5.6).
        let _ = agent_of_pid(std::process::id());
    }
}
