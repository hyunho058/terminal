//! Agent presence + identity: a bounded breadth-first walk of a tab's descendant
//! processes (D6), each node classified by executable basename first, then its leading
//! dot-component (an update-renamed binary like `agy.<id>.old`), then the
//! `<agent>/versions` native-install signature, then — *only* for known interpreters —
//! its argv (D5).
//!
//! The basename itself has two sources, and that is deliberate: `proc_pidpath` when the
//! executable is still on disk, and the kernel's **exec-time name** when it is not. An
//! auto-updater that renames the running binary and later deletes it would otherwise take
//! the whole ladder down with the path (`agent-classifier-deleted-binary`).
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

/// [`classify_with_name`] with no name source — the pre-`agent-classifier-deleted-binary`
/// signature, kept at its original arity so the eight pre-existing call sites that use it
/// (spread over six tests) stay byte-identical. Only tests call it now.
///
/// `#[allow(dead_code)]` rather than `#[cfg(test)]`: `tmux.rs`'s doc comment carries an
/// intra-doc link to this function, which a `cfg(test)` gate would break in every non-test
/// build. This is one of the three allows in the crate that rustc still enforces — see the
/// visibility note in `lib.rs` for why the others were removed rather than kept.
///
/// Do **not** "simplify" this away by pointing `classify_pid` back here — that is precisely
/// the change that reintroduces `agent-classifier-deleted-binary`.
#[allow(dead_code)]
pub(crate) fn classify_with(
    exe_path: Option<&Path>,
    read_argv: impl FnOnce() -> Vec<OsString>,
) -> Option<AgentKind> {
    classify_with_name(exe_path, || None, read_argv)
}

/// The staged classifier, pure over its inputs so the `sysctl` gate is testable without a
/// process (R4.4): stage 1 is the basename, stage 1a its leading component (for an
/// update-renamed binary), stage 1b the `<agent>/versions` install signature — all
/// syscall-free — and only then, for a known interpreter basename, is `read_argv` invoked.
/// A failed argv read (empty) means "not an agent" (R4.5).
///
/// The basename comes from `exe_path` when there is one and from `read_name` when there is
/// not (`agent-classifier-deleted-binary` D1/D2) — the fallback that keeps a live agent whose
/// binary was deleted from vanishing out of the classifier entirely.
///
/// `read_name` is consulted **only** when `exe_path` is `None`, and it is a closure for the
/// same reason `read_argv` is: the syscall must not fire on the common path, and a test must
/// be able to prove it did not.
///
/// The ladder itself is unchanged — this restores its *input*, it does not add a stage; the
/// per-stage comments in the body say which is which. Two consequences are worth stating
/// here, because the body cannot:
///
/// - **stage 1b is unreachable from a recovered name** — the `<agent>/versions` signature
///   needs a path, and there is none. Synthesising one would fabricate a component the kernel
///   never reported, so it is deliberately not done.
/// - **stage 2 receives an input it never used to** — a pathless process whose *name* is an
///   interpreter. That is the gate behaving as specified on a new input rather than a widened
///   gate, and both directions of it are pinned by tests below.
pub(crate) fn classify_with_name(
    exe_path: Option<&Path>,
    read_name: impl FnOnce() -> Option<String>,
    read_argv: impl FnOnce() -> Vec<OsString>,
) -> Option<AgentKind> {
    // The basename to classify: the path's when there is a path, and otherwise the kernel's
    // exec-time name for the process. Which of the two it is does not change a single rule
    // below — that equivalence is the whole design.
    let (base, path) = match exe_path {
        Some(path) => (
            path.file_name().map(|s| s.to_string_lossy().into_owned()),
            Some(path),
        ),
        // `exe_path` is `None` for a deleted binary (the reported bug), for a pid that died
        // between the child listing and the probe, and for a process we may not inspect. This
        // branch does not ask which: the verdict depends only on the name.
        None => (read_name(), None),
    };
    let base = base.as_deref();
    // Stage 1: exact executable basename.
    if let Some(kind) = base.and_then(classify_exe) {
        return Some(kind);
    }
    // Stage 1a: an in-place auto-updater renames the running binary to `<name>.<id>.old`
    // (e.g. `agy.1784614720971820000.old`), so a long-running session's `proc_pidpath`
    // basename carries a suffix. The agent name survives as the leading dot-component.
    if let Some(kind) = base
        .and_then(|b| b.split_once('.'))
        .and_then(|(lead, _)| classify_exe(lead))
    {
        return Some(kind);
    }
    // Stage 1b: the versioned-install signature (basename is a version string). Reachable
    // only with a path — a recovered name carries no directory components to match.
    if let Some(kind) = path.and_then(classify_install_path) {
        return Some(kind);
    }
    // Stage 2: only a known interpreter's argv is read (the `sysctl` gate).
    if base.is_some_and(is_interpreter) {
        return classify_argv(&read_argv());
    }
    None
}

/// The agent a walk found: which one, **and which process it is**.
///
/// The pid used to be discarded — the walk visited it, classified it, and returned only the
/// kind. That was sufficient while the only question was "is an agent running here". It is
/// not sufficient for "which session is this tab showing": answering that means asking the
/// process itself (`~/.claude/sessions/<pid>.json`), which needs the pid the walk already
/// had in hand (tabmeta-session-keying D5). Returning it is what lets the card resolve its
/// own session **without** `tabsession::hint_for`, which forks `/bin/ps` and is far too
/// expensive for a per-tick worker.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Detected {
    pub(crate) kind: AgentKind,
    pub(crate) pid: u32,
}

/// Breadth-first, bounded, early-exit walk (D6). Generic over `children` and `classify`
/// so R5.1–R5.4 test every bound against a fake tree. The root (the tab's zsh) is not
/// itself classified — only its descendants, from depth 1. Returns the first match, as
/// [`Detected`] — the kind *and* the pid that produced it.
///
/// **Order is now load-bearing.** Depth ordering is this loop's own (breadth-first, so a
/// `claude` the user launched at depth 1–2 is always classified before one a tool call
/// spawned at depth 3). Order *within* a level comes from the `children` provider, and the
/// production provider (`proc::children_of` → `parse_childpids`) sorts ascending precisely
/// so that this first-match is reproducible.
pub(crate) fn walk(
    root: u32,
    children: impl Fn(u32) -> Vec<u32>,
    classify: impl Fn(u32) -> Option<AgentKind>,
) -> Option<Detected> {
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
            return Some(Detected { kind, pid }); // first match short-circuits (R5.3)
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
pub(crate) fn agent_of_pid(pid: u32) -> Option<Detected> {
    walk(pid, super::proc::children_of, classify_pid)
}

/// Classify one live pid: `proc_pidpath` (basename + `<agent>/versions` signature) first;
/// when it reports nothing, the kernel's exec-time name; then — only for an interpreter —
/// argv via `sysctl`. The `sysctl` (in `proc_args`) is reached solely through the
/// `is_interpreter` gate inside `classify_with_name` (R4.4), and the name read
/// (`proc_pidinfo`) solely on the pathless branch.
fn classify_pid(pid: u32) -> Option<AgentKind> {
    let path = super::proc::exe_path(pid);
    classify_with_name(
        path.as_deref(),
        || super::proc::name_of_pid(pid),
        || super::proc::proc_args(pid),
    )
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

    // ---- the deleted-binary fallback (agent-classifier-deleted-binary R2) ----

    /// A name closure that records whether it was called — the only way to assert the
    /// *absence* of a syscall on the common path.
    fn spy<'a>(
        name: Option<&str>,
        called: &'a std::cell::Cell<bool>,
    ) -> impl FnOnce() -> Option<String> + 'a {
        let name = name.map(str::to_string);
        move || {
            called.set(true);
            name
        }
    }

    #[test]
    fn a_deleted_binary_classifies_by_its_exec_time_name() {
        // R2.1 — the reported bug, at the unit level. `agy`'s updater renames the running
        // binary and later deletes it; `proc_pidpath` then reports nothing, while the
        // kernel's exec-time name is still the un-renamed `agy`. Stage 1 matches it exactly.
        let called = std::cell::Cell::new(false);
        assert_eq!(
            classify_with_name(None, spy(Some("agy"), &called), Vec::new),
            Some(AgentKind::Antigravity),
        );
        assert!(called.get(), "the name must be read when there is no path");

        // Every table entry resolves the same way — the rule is agent-agnostic.
        for (name, kind) in [
            ("claude", AgentKind::Claude),
            ("codex", AgentKind::Codex),
            ("gemini", AgentKind::Gemini),
            ("aider", AgentKind::Aider),
            ("antigravity", AgentKind::Antigravity),
        ] {
            let c = std::cell::Cell::new(false);
            assert_eq!(
                classify_with_name(None, spy(Some(name), &c), Vec::new),
                Some(kind),
                "{name} must classify from its name alone"
            );
        }
    }

    #[test]
    fn stage_1a_still_applies_to_a_recovered_name() {
        // R2.2 — a process exec'd directly from an already-renamed file reports THAT name,
        // so the leading dot-component rule has to survive the move to the name source…
        let c = std::cell::Cell::new(false);
        assert_eq!(
            classify_with_name(None, spy(Some("agy.1784614720971820000.old"), &c), Vec::new),
            Some(AgentKind::Antigravity),
        );
        // …and stay exact: a near-miss is still not an agent.
        let c = std::cell::Cell::new(false);
        assert_eq!(
            classify_with_name(None, spy(Some("agyx.7.old"), &c), Vec::new),
            None,
        );
    }

    #[test]
    fn the_sysctl_gate_stays_closed_on_the_pathless_branch() {
        let _g = super::super::probe::serial();
        // R2.3 — `sysctl_gate_closed_for_non_interpreters` passes a real path, so it does
        // not cover this branch at all. Without this test the fallback could quietly start
        // paying a KERN_ARGMAX-sized sysctl for every unnameable pid in a walk.
        super::super::probe::reset_all();
        for name in ["rustc", "cc", "ld"] {
            let c = std::cell::Cell::new(false);
            let got = classify_with_name(None, spy(Some(name), &c), || {
                super::super::proc::proc_args(0)
            });
            assert_eq!(got, None);
        }
        assert_eq!(
            super::super::probe::PROC_ARGS.get(),
            0,
            "a recovered non-interpreter name must not open the sysctl gate"
        );
    }

    #[test]
    fn a_recovered_interpreter_name_does_reach_argv() {
        // R2.4 — the one input the gate never used to receive, pinned as a fact rather than
        // conceded in prose: if this ever stops holding, it is a deliberate change.
        let c = std::cell::Cell::new(false);
        let argv = vec![
            OsString::from("node"),
            OsString::from("/opt/@anthropic-ai/claude-code/cli.js"),
        ];
        assert_eq!(
            classify_with_name(None, spy(Some("node"), &c), || argv),
            Some(AgentKind::Claude),
        );
    }

    #[test]
    fn a_readable_path_never_reads_the_name() {
        // R2.5/R4.1 — the laziness that keeps the common path free. If the name closure
        // fires here, every walk on the machine pays an extra proc_pidinfo per pid.
        let called = std::cell::Cell::new(false);
        assert_eq!(
            classify_with_name(
                Some(Path::new("/usr/bin/claude")),
                spy(Some("agy"), &called),
                Vec::new
            ),
            Some(AgentKind::Claude),
        );
        assert!(
            !called.get(),
            "a readable path must not trigger the name read"
        );

        // Also true when the path classifies to NOTHING — the fallback is for an absent
        // path, not for a disappointing one. (A path that named an agent would otherwise be
        // second-guessed by a name that disagrees.)
        let called = std::cell::Cell::new(false);
        assert_eq!(
            classify_with_name(
                Some(Path::new("/usr/bin/rustc")),
                spy(Some("agy"), &called),
                Vec::new
            ),
            None,
        );
        assert!(
            !called.get(),
            "still no name read when the path simply does not match"
        );
    }

    #[test]
    fn neither_source_means_not_an_agent() {
        let _g = super::super::probe::serial();
        // R2.6 — a dead pid reaches here: both reads fail, and the walk carries on (R5.4).
        super::super::probe::reset_all();
        let got = classify_with_name(None, || None, || super::super::proc::proc_args(0));
        assert_eq!(got, None);
        assert_eq!(
            super::super::probe::PROC_ARGS.get(),
            0,
            "no name, no argv read either"
        );
    }

    #[test]
    fn the_wrapper_is_the_old_behaviour_exactly() {
        // R2.7 — `classify_with` must keep answering what it always did, since eight
        // pre-existing call sites (over six tests) still use it. It supplies no name, so a
        // `None` path stays `None`.
        assert_eq!(classify_with(None, Vec::new), None);
        assert_eq!(
            classify_with(Some(Path::new("/usr/bin/agy")), Vec::new),
            Some(AgentKind::Antigravity),
        );
    }

    #[test]
    fn the_verdict_on_the_pathless_branch_is_a_function_of_the_name_alone() {
        // R2.8 — a deleted binary and a process we may not inspect arrive identically here:
        // `exe_path` returned `None`, and the verdict rests on the name.
        //
        // **The "no EPERM branch" half is enforced by the TYPE, not by this test**:
        // `classify_with_name` receives `Option<&Path>`, which carries no reason for the
        // absence, so there is no `errno` here to branch on even if someone wanted to.
        //
        // What IS testable, and what this pins, is the other half: the same name yields the
        // same verdict as the equivalent path would, and the verdict is a real value rather
        // than a shared `None`.
        for (name, expect) in [
            ("claude", Some(AgentKind::Claude)),
            ("agy", Some(AgentKind::Antigravity)),
            ("rustc", None),
        ] {
            let c = std::cell::Cell::new(false);
            let pathless = classify_with_name(None, spy(Some(name), &c), Vec::new);
            assert_eq!(
                pathless, expect,
                "pathless {name} must classify as {expect:?}"
            );
            assert_eq!(
                pathless,
                classify_with(Some(Path::new(name)), Vec::new),
                "a recovered name must reach the same verdict as the same basename in a path"
            );
        }
    }

    #[test]
    fn the_walk_finds_an_agent_that_only_the_name_can_identify() {
        // R0.2 — composition: a node that ONLY the name can identify must still come back
        // out of the walk, carrying its pid. (Depth ordering and first-match-wins are pinned
        // by `depth_outranks_pid_order` and `walk_short_circuits_on_match`; this tree has one
        // level and one match, so it pins neither — it pins that the fallback survives the
        // trip through `walk` at all.)
        let tree: HashMap<u32, Vec<u32>> = HashMap::from([(1, vec![10, 11])]);
        let got = walk(1, children_of(&tree), |pid| {
            // pid 11 is the pathless one — only a name identifies it.
            (pid == 11)
                .then(|| classify_with_name(None, || Some("agy".into()), Vec::new))
                .flatten()
        });
        assert_eq!(
            got,
            Some(Detected {
                kind: AgentKind::Antigravity,
                pid: 11
            }),
        );
    }

    // ---- the live end-to-end gate (agent-classifier-deleted-binary R0.1, R3, R4.1) ----

    /// The child of `a_live_agent_survives_its_binary_being_deleted` — not a test of
    /// anything itself.
    ///
    /// `#[ignore]` so a normal run never executes it, and named so its parent can ask for
    /// exactly it.
    ///
    /// The sleep is a bounded orphan window, not a promise of tidiness: `ChildGuard` reaps
    /// the child on every path `Drop` runs, and this bound covers the one path where it does
    /// not — a `kill -9` of the runner. (A Ctrl-C does not need it: nothing here calls
    /// `process_group`, so the child stays in the parent's group and takes the terminal's
    /// SIGINT directly.) So the bound is set as *low* as the parent's budget allows —
    /// comfortably above the 10s readiness poll plus assertions, and under the workspace
    /// run's own ~39s wall time rather than a multiple of it.
    #[test]
    #[ignore = "helper: spawned by a_live_agent_survives_its_binary_being_deleted"]
    fn deleted_binary_child_sleeper() {
        // Strictly longer than the parent's readiness budget (10s) plus its assertions, so a
        // slow runner cannot hand those assertions a child that is already on its way out —
        // which would surface as a confusing liveness failure rather than a timeout.
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    /// Kills **and reaps** the child, on the normal path and on unwind alike.
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait(); // without this the child lingers as a zombie
        }
    }

    #[test]
    fn a_live_agent_survives_its_binary_being_deleted() {
        // R0.1/R3 — the reported bug, end to end, against a real process and real syscalls.
        // No pure test can reach it: the failure is `proc_pidpath` returning *nothing*, and
        // the two previous instalments of this same bug (`antigravity-agy-detection`,
        // `agent-skin-detection-fix`) were both closed on a manual probe — which is exactly
        // how this defect then shipped to a user.
        //
        // The lock is held across the child's WHOLE lifetime, for two reasons that are not
        // about this test's own counters: `worker.rs`'s
        // `a_tab_with_no_claude_agent_pays_nothing_for_identity` asserts that this process
        // runs no agent, and a live child named `agy` would falsify it; and this test's
        // classification bumps PROC_EXE/PROC_NAME on a live pid, which uncoordinated could
        // land inside another cost test's counted window.
        let _g = super::super::probe::serial();

        // Canonicalize OUR side once. `std::env::temp_dir()` is `/var/folders/…` while
        // `proc_pidpath` reports the kernel's resolved `/private/var/…`; comparing the two
        // spellings would make the readiness poll below time out every single run. This is
        // the trap `presence_fd::conversation_of_pid_in` documents having shipped once.
        let tmp = std::fs::canonicalize(std::env::temp_dir()).expect("canonical temp dir");
        let dir = tmp.join(format!("agent-del-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        // The basename must be exactly `agy`: the whole point is that the EXEC-TIME name is
        // what the kernel keeps, so this stands in for the real `~/.local/bin/agy`.
        let exe = dir.join("agy");
        std::fs::copy(std::env::current_exe().expect("current_exe"), &exe).expect("copy");

        // Spawn ONE named ignored test. With no arguments this would run the entire suite —
        // including this test, which spawns another copy, recursively.
        let child = std::process::Command::new(&exe)
            .args([
                "--exact",
                "tabmeta::agent::tests::deleted_binary_child_sleeper",
                "--ignored",
                "--nocapture",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn the copied binary");
        let mut guard = ChildGuard(child);
        let pid = guard.0.id();

        // `spawn` returns once fork/exec is INITIATED. Unlinking before the child's `execve`
        // lands would leave it never starting — rarely here, more often on a loaded runner.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if super::super::proc::exe_path(pid).as_deref() == Some(exe.as_path()) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "child {pid} never exec'd {}; exe_path reads {:?}. This is the readiness \
                 step failing, not the classifier — the likeliest cause is the `--exact` \
                 filter naming no test, which makes libtest exit 0 immediately.",
                exe.display(),
                super::super::proc::exe_path(pid),
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        // The production sequence: the updater renames the running binary in place…
        let renamed = dir.join("agy.1784614720971820000.old");
        std::fs::rename(&exe, &renamed).expect("rename");
        assert_eq!(
            super::super::proc::exe_path(pid).as_deref(),
            Some(renamed.as_path()),
            "proc_pidpath follows a rename — this is why stage 1a exists"
        );
        // …and a later run deletes it. THIS is where the old code went blind.
        std::fs::remove_file(&renamed).expect("unlink");

        // The child was alive at readiness; this catches it dying in the window between
        // there and here (a killed process, an OOM), so a `None` verdict below is never
        // quietly the classifier being blamed for a corpse. A filter typo is caught earlier,
        // by the readiness poll — a process that exits at once never reports an exe path.
        assert!(
            guard.0.try_wait().expect("try_wait").is_none(),
            "the child died between readiness and the assertions"
        );

        assert_eq!(
            super::super::proc::exe_path(pid),
            None,
            "an unlinked executable must leave proc_pidpath with nothing to report"
        );
        assert_eq!(
            super::super::proc::name_of_pid(pid).as_deref(),
            Some("agy"),
            "the kernel's name is fixed at exec: not the renamed one, and not gone"
        );
        assert_eq!(
            classify_pid(pid),
            Some(AgentKind::Antigravity),
            "a live agent whose binary was deleted must still be an agent (this returns \
             None on the pre-fix code)"
        );

        drop(guard);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_readable_exe_path_costs_no_name_read() {
        let _g = super::super::probe::serial();
        // R4.1 — the cost promise, on the real syscall path rather than the injected one.
        // This process has a readable `exe_path`, so the fallback must never be consulted.
        //
        // ONE pid, not a live walk, and that is a deliberate narrowing of R4.1's "over a walk
        // of readable-`exe_path` processes". A walk of THIS process's real children cannot
        // carry an absolute `PROC_NAME == 0`: a child that dies between the child listing and
        // the probe has no readable path and legitimately pays one name read, which is the
        // same nondeterminism that forced `worker.rs`'s `PROC_EXE` assertion to a floor. A
        // floor would assert nothing here (0 is already the floor), so the honest pin is an
        // exact count over a pid whose state the test controls.
        super::super::probe::reset_all();
        let _ = classify_pid(std::process::id());
        assert_eq!(
            super::super::probe::PROC_NAME.get(),
            0,
            "a readable exe_path must not pay for a name read"
        );
        assert_eq!(
            super::super::probe::PROC_EXE.get(),
            1,
            "and it still pays exactly the one path read it always did"
        );
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
        assert_eq!(
            got,
            Some(Detected {
                kind: AgentKind::Claude,
                pid: 2
            })
        );
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
        assert_eq!(
            got,
            Some(Detected {
                kind: AgentKind::Codex,
                pid: 4
            })
        );
    }

    // ---- the walk reports an identity, not just a kind (R1.1, R1.3) ----

    #[test]
    fn walk_reports_the_pid_that_matched() {
        // R1.1 — the pid is the whole point: it is what `~/.claude/sessions/<pid>.json` is
        // keyed on, so discarding it is what forced the old cwd-ranking.
        let tree: HashMap<u32, Vec<u32>> = [(1, vec![2])].into_iter().collect();
        let matches: HashMap<u32, AgentKind> = [(2, AgentKind::Claude)].into_iter().collect();
        let got = walk(1, children_of(&tree), |pid| matches.get(&pid).copied());
        assert_eq!(
            got,
            Some(Detected {
                kind: AgentKind::Claude,
                pid: 2
            }),
            "the walk must report WHICH process it classified"
        );
    }

    #[test]
    fn depth_outranks_pid_order() {
        // R1.3 — `parse_childpids` sorts *within* a level; it must never reorder *across*
        // levels. A depth-1 `claude` with a high pid still beats a depth-2 one with a low
        // pid, because the shallower process is the one the user launched in this tab.
        //
        // Note this test injects its own children provider, so it deliberately bypasses the
        // production sort — which is exactly why the sort lives in `parse_childpids` and is
        // tested there: the two properties are independent and are pinned independently.
        let tree: HashMap<u32, Vec<u32>> = [(1, vec![50]), (50, vec![2])].into_iter().collect();
        let matches: HashMap<u32, AgentKind> = [(2, AgentKind::Claude), (50, AgentKind::Claude)]
            .into_iter()
            .collect();
        let got = walk(1, children_of(&tree), |pid| matches.get(&pid).copied());
        assert_eq!(
            got.map(|f| f.pid),
            Some(50),
            "depth 1 wins over depth 2 regardless of pid value"
        );
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
