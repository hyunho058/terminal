//! Bench group 2 of 3: **100k-line scroll** (R1.3).
//!
//! Builds a 100k-line scrollback (the `scrollback` feature's ring), then times a
//! `scroll_up` page + `snapshot` sweep. Proves the per-frame scroll work is
//! bounded (no allocation-driven blowup) as the "no frame drops scrolling 100k
//! lines" goal requires. Headless (C1 / SD2).
//!
//! Provisional baseline (Apple M1, `--release`): median ≈ 9.7 µs for
//! `scroll_up(40) + snapshot` over a ~100k-row history (built in ≈112 ms). The
//! committed gate lives in `docs/performance.md` (pending G1 confirmation).
//!
//! NOTE: not yet registered as a `[[bench]]` target (see input_to_grid.rs).

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use term_core::{Color, Terminal};

const FG: Color = Color(0xE6E1CF);
const BG: Color = Color(0x0F1419);
const COLS: u16 = 120;
const ROWS: u16 = 40;

/// A `Terminal` whose scrollback holds ~100k lines of varied text.
fn built_100k() -> Terminal {
    let mut term = Terminal::new(COLS, ROWS, FG, BG);
    for i in 0..100_000u32 {
        let line = format!("line {i:06} the quick brown fox jumps over 0123456789\r\n");
        term.advance(line.as_bytes());
    }
    term
}

fn scroll_100k(c: &mut Criterion) {
    let mut term = built_100k();
    term.scroll_to_bottom();
    c.bench_function("scroll_100k", |b| {
        b.iter(|| {
            term.scroll_up(black_box(ROWS as usize));
            let snap = term.snapshot();
            black_box(&snap);
            if term.at_top() {
                term.scroll_to_bottom();
            }
        })
    });
}

criterion_group!(benches, scroll_100k);
criterion_main!(benches);
