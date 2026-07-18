//! Bench group 3 of 3: **frame time / sustained fps** (R1.2).
//!
//! Times a headless `Renderer` drawing a full-screen snapshot to an offscreen
//! texture; fps is derived as `1 / frame_time`. This is the CPU-submit portion of
//! the frame (instance building + pass recording + submit). Runs on a headless
//! wgpu device (C1) — no window. Skips gracefully if no GPU adapter is available
//! (e.g. a CI runner without one).

use criterion::{criterion_group, criterion_main, Criterion};

use common::config::Config;
use renderer::Renderer;
use term_core::{Color, Terminal};

fn frame(c: &mut Criterion) {
    let cfg = Config::default();
    let mut renderer = match Renderer::new_headless(2560, 1440, 2.0, &cfg) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("frame bench skipped (no headless GPU adapter): {e}");
            return;
        }
    };

    // A dense full-screen grid of text — the realistic worst case for the glyph
    // pass (every cell shaped + drawn).
    let (cols, rows) = renderer.grid_size();
    let mut term = Terminal::new(
        cols,
        rows,
        Color(cfg.theme.foreground),
        Color(cfg.theme.background),
    );
    for _ in 0..rows {
        term.advance(b"the quick brown fox jumps over the lazy dog 0123456789 != => <= |> ");
        term.advance(b"\r\n");
    }
    let snapshot = term.snapshot();

    // One reusable offscreen target (its allocation is outside the timed loop).
    let target = renderer.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("bench-offscreen"),
        size: wgpu::Extent3d {
            width: 2560,
            height: 1440,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: renderer.surface_format(),
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    c.bench_function("frame", |b| {
        b.iter(|| {
            renderer.draw_to(&view, &snapshot);
        })
    });
}

criterion_group!(benches, frame);
criterion_main!(benches);
