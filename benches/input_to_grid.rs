//! Bench group 1 of 3: **input→grid** (R1.1).
//!
//! Times the CPU portion of the input→screen budget: feeding a keystroke→echo
//! byte sequence through `Terminal::advance` (vte parse → grid mutation). Fully
//! headless — no GPU, no window (C1 / SD2).
//!
//! Provisional baseline (Apple M1, `--release`): median ≈ 2.1 µs / advance of a
//! 30-byte prompt-echo sequence. The committed gate lives in
//! `docs/performance.md` (pending G1 human confirmation — see build_order.md).
//!
//! NOTE: not yet registered as a `[[bench]]` target — that needs a bench-owning
//! package manifest (a workspace member), which is outside this feature's
//! declared surface (`benches/**`). Parked pending that decision.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use term_core::{Color, Terminal};

const FG: Color = Color(0xE6E1CF);
const BG: Color = Color(0x0F1419);

fn input_to_grid(c: &mut Criterion) {
    // A representative prompt echo: SGR-colored prompt + a typed command + CRLF.
    let seq: &[u8] = b"\x1b[32muser@host\x1b[0m:~$ ls -la\r\n";
    let mut term = Terminal::new(120, 40, FG, BG);
    c.bench_function("input_to_grid", |b| {
        b.iter(|| {
            term.advance(black_box(seq));
        })
    });
}

criterion_group!(benches, input_to_grid);
criterion_main!(benches);
