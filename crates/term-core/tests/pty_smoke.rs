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
