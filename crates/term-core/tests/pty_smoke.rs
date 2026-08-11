//! PTY integration smoke test — spawns a real zsh.
//!
//! Ignored by default (it depends on `/bin/zsh` and can block on shell startup),
//! so CI stays deterministic. Run explicitly:
//!
//! ```sh
//! cargo test -p term-core --test pty_smoke -- --ignored
//! ```

use std::io::{Read, Write};
use std::time::Duration;

use term_core::Pty;

#[test]
#[ignore = "spawns a real zsh; run with --ignored"]
fn zsh_echo_roundtrip() {
    let pty = Pty::spawn(80, 24).expect("spawn zsh");
    let mut reader = pty.reader().expect("reader");
    let mut writer = pty.writer().expect("writer");

    writer.write_all(b"printf MARKER42\n").expect("write");
    writer.flush().expect("flush");
    drop(writer); // EOF so the shell exits and the reader eventually stops.

    // Bounded read: give the shell a moment, then collect available output.
    std::thread::sleep(Duration::from_millis(300));
    let mut buf = vec![0u8; 4096];
    let mut seen = String::new();
    for _ in 0..8 {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => seen.push_str(&String::from_utf8_lossy(&buf[..n])),
            Err(_) => break,
        }
        if seen.contains("MARKER42") {
            break;
        }
    }
    assert!(
        seen.contains("MARKER42"),
        "shell output did not contain marker; got: {seen:?}"
    );
}

/// End-to-end shell integration: a real zsh started with our ZDOTDIR injection
/// must emit an OSC 133 command-end marker (`ESC ] 133 ; D ; <exit> ; <ms>`)
/// after a command runs. This exercises the emit → wire contract against zsh 5.9;
/// the parse side is covered by the unit tests in `parser.rs`.
#[test]
#[ignore = "spawns a real zsh + sources the user's rc; run with --ignored"]
fn osc133_marker_emitted_by_injected_zsh() {
    let pty = Pty::spawn(80, 24).expect("spawn zsh");
    let mut reader = pty.reader().expect("reader");
    let mut writer = pty.writer().expect("writer");

    // Let the interactive shell finish sourcing rc + print its first prompt.
    std::thread::sleep(Duration::from_millis(600));
    writer.write_all(b"true\n").expect("write"); // a command → preexec/precmd fire
    writer.flush().expect("flush");

    let mut seen: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 8192];
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(50));
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => seen.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
        // OSC 133 command-end for `true`: exit 0. Match the framed prefix.
        if seen.windows(8).any(|w| w == b"\x1b]133;D;") {
            return; // marker observed → contract holds
        }
    }
    panic!(
        "no OSC 133 command-end marker in {} bytes of shell output",
        seen.len()
    );
}

/// R1.6 (child boundary) — a real zsh spawned through `Pty::spawn` reports none
/// of the eleven agent session-identity markers, even when the test binary itself
/// carries them (which it does when `cargo test` runs inside a Claude session).
///
/// The unit tests in `pty.rs` assert the `CommandBuilder`'s state; this one is the
/// only check that the removal survives portable-pty's `env_clear`-then-reapply,
/// which is a 0.9.0 implementation detail rather than a published contract.
/// Ignored by default like its neighbours — outside a Claude session it passes
/// trivially, so it is a manual verification step, not a CI gate.
#[test]
#[ignore = "spawns a real zsh; run with --ignored"]
fn agent_session_markers_absent_from_the_child() {
    // Kept in sync by hand with `pty::AGENT_SESSION_VARS`, which is private (R6.2).
    const PATTERN: &str = "^(AI_AGENT|CLAUDECODE|CLAUDE_CODE_CHILD_SESSION\
        |CLAUDE_CODE_ENTRYPOINT|CLAUDE_CODE_EXECPATH|CLAUDE_CODE_MESSAGING_SOCKET\
        |CLAUDE_CODE_SESSION_ID|CLAUDE_CODE_SSE_PORT|CLAUDE_CODE_VERSION\
        |CLAUDE_EFFORT|CLAUDE_PID)=";

    let pty = Pty::spawn(80, 24).expect("spawn zsh");
    let mut reader = pty.reader().expect("reader");
    let mut writer = pty.writer().expect("writer");

    // Let the interactive shell finish sourcing rc + print its first prompt.
    std::thread::sleep(Duration::from_millis(600));
    // Report only a *count*, so no value of any environment variable is ever
    // echoed. The marker is assembled from `$m` so the terminal's echo of this
    // command line cannot itself contain `AGENTVARS<` — otherwise the read loop
    // would stop on the echo and never see the reply.
    let cmd =
        format!("m=AGENTVARS; printf '%s<%s>\\n' \"$m\" \"$(env | grep -c -E '{PATTERN}')\"\n");
    writer.write_all(cmd.as_bytes()).expect("write");
    writer.flush().expect("flush");

    let mut seen = String::new();
    let mut buf = vec![0u8; 8192];
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(50));
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => seen.push_str(&String::from_utf8_lossy(&buf[..n])),
            Err(_) => break,
        }
        if seen.contains("AGENTVARS<") {
            break;
        }
    }
    assert!(
        seen.contains("AGENTVARS<0>"),
        "child still carries agent session markers (or no reply arrived); got: {seen:?}"
    );
}
