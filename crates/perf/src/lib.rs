//! `perf` — the bench-owning crate for the performance-harness feature.
//!
//! This crate has no runtime code; it exists only to register the Criterion
//! benchmarks under the workspace-root `benches/` directory (see `Cargo.toml`'s
//! `[[bench]]` targets). Run them with `cargo bench -p perf`.
