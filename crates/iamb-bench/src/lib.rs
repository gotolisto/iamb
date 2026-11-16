//! Benchmark harness for the Iamb product KPIs.
//!
//! - [`fixture`]: deterministic snippet libraries from a seed (no external RNG crate).
//! - [`latency`]: a recorder that keeps every sample and reports nearest-rank percentiles,
//!   which criterion does not report.
//! - [`results`]: writes one JSON-lines file per run under `target/bench-results/`.
//!
//! See `docs/test-strategy.md` for how these pieces are used.

pub mod fixture;
pub mod latency;
pub mod results;

pub use fixture::{FixtureSnippet, SplitMix64, generate_library};
pub use latency::{LatencyRecorder, LatencySummary};
pub use results::{ResultRecord, RunWriter};

/// Seed used by every committed benchmark, so all runs measure the same library.
pub const DEFAULT_SEED: u64 = 0x1A3B_2026;

/// Library size used by the KPI benchmarks (matcher, store load, palette search).
pub const KPI_LIBRARY_SIZE: usize = 10_000;
