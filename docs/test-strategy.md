# Iamb test strategy

Status: draft for approval · Task 0.5 · Builds on [`architecture.md`](architecture.md) and
[`compatibility-matrix.md`](compatibility-matrix.md)

This document says how every product KPI and component target is measured, where each kind of test
lives, and which task owns it. Rule of thumb: **CI detects regressions; official KPI numbers come
from real machines.** Shared CI runners are noisy (10–30% run-to-run variance, no GPU, no
interactive desktop on macOS), so CI uses generous thresholds and never reports a KPI as met.

## 1. Test layers

| Layer | Tool | Lives in | Runs | Gate |
| --- | --- | --- | --- | --- |
| Unit | `cargo test` | `#[cfg(test)]` modules in each crate | CI, 3 OSes, every PR | Must pass |
| Property | `proptest` | `crates/<crate>/tests/prop_*.rs` | CI, every PR (256 cases); nightly 10,000 cases | Must pass |
| Template spec examples | TOML cases | `spec/template/examples/*.toml`; run by `crates/iamb-core/tests/template_spec.rs`, later by the mobile bindings | CI, every PR | Every case passes |
| Fuzzing | `cargo-fuzz` (pinned nightly, the only nightly use) | `fuzz/` (its own workspace) target `template` (parse, resolve and render); later `rich_markdown`, `ipc_frame_decode`, `import_json` | Nightly, 10 min per target, toolchain pinned in `.github/workflows/nightly.yml`; corpus seeded from the spec examples and kept between nights | No crash, no hang over 1 s, no OOM over 512 MB |
| Scripted typing sessions | `iamb-testkit::session` (scripted key stream, fake clock, expected actions) | `crates/iamb-testkit/sessions/*.toml` (run by `crates/iamb-core/tests/engine_sessions.rs`), later `crates/iamb-service/tests/` | CI, every PR | Must pass |
| Mutation sample | `cargo-mutants` on `iamb-core::matcher` | Config in `crates/iamb-core/.cargo/mutants.toml` | Manual at tasks 1.2 and 1.9; weekly afterwards | ≥ 80% mutants caught; every missed mutant reviewed |
| IPC contract | `cargo test` with golden JSON | `crates/iamb-ipc/tests/contract/` (one golden file per message) | CI, every PR | Every message round-trips byte-for-byte; role and version rules tested |
| Service vs. mock platform | `iamb-platform` mock + `iamb-testkit` | `crates/iamb-service/tests/` | CI, 3 OSes, every PR | Must pass |
| OS layer | `cargo test -- --ignored` on runners | `crates/iamb-platform-{windows,macos}/tests/` | CI `windows-latest`, `macos-latest` | Must pass where the runner allows (see §4) |
| Compatibility matrix | `iamb-testkit::compat` + manual | [`compatibility-matrix.md`](compatibility-matrix.md) | Real machines, task 2.7 and every release candidate | ≥ 95% per OS |
| Benchmarks | criterion + `LatencyRecorder` | `crates/iamb-bench/benches/` | CI report-only; real machines for KPIs | §3 |
| Soak and privacy | `iamb-testkit::soak` + canary scan | `crates/iamb-testkit/src/soak/` | Real machine per OS (72 h); CI nightly (30 min) | §5 |
| Coverage | `cargo-llvm-cov` | CI job `coverage` (Linux) | Every PR from task 1.6 | `iamb-core` line coverage ≥ 85% |

Determinism rules: tests set `IAMB_DATA_DIR` to a temp directory; time comes from the fake clock
through `Environment`; randomness only from seeded generators (seed printed on failure).

## 2. KPI and target measurement

"Lab" means a dedicated real machine per OS (Windows 11 and macOS, current release, mid-range
hardware, power plugged in, no other user apps running). Every lab number is recorded in
`docs/results/<task>-<os>.md` with machine, OS build and commit.

### 2.1 Product KPIs

| KPI | Target | Method, sample, statistic | Where | Pass rule |
| --- | --- | --- | --- | --- |
| Last keystroke → expanded text, plain ≤ 500 chars | 95% < 50 ms | Probe window (§6); 1,000 expansions per OS: 400 short (≤ 30 chars), 400 medium (31–200), 200 long (201–500); nearest-rank p95 | Lab, tasks 1.6 (engine share, mock platform), 2.7 (end to end) | p95 < 50 ms on each OS. CI (mock platform): p95 of engine share < 5 ms, report-only |
| Fill-in form visible | < 150 ms | From probe's last-key timestamp to the form window's first painted frame (form logs its paint timestamp on the same monotonic clock); 100 trials, warm UI host | Lab, task 3.5 | p95 < 150 ms |
| Search palette opens | < 100 ms | Hotkey injected by driver → palette first paint; 100 trials, warm UI host | Lab, task 3.6 | p95 < 100 ms |
| Correct expansions in scripted run | ≥ 99.9% of 10,000 per OS | `iamb-testkit` script of 10,000 expansions (all 5 snippet types, mixed with non-matching typing) into the probe window through the real service and OS layer; correct = probe text equals expected exactly | Lab, tasks 2.1, 2.3 (listener), 2.7 (full) | ≤ 10 failures per OS; each failure logged with cause. CI mock run: 10,000/10,000 |
| 72-hour run | No crash; memory growth < 5 MB | §5 soak | Lab, task 2.6 | 0 unexpected exits; both leak rules in §5 hold |
| Service idle | CPU < 0.1%; memory < 20 MB | After 10 min settle, 60 min idle with 10,000-snippet library: CPU = process CPU time delta ÷ wall time; memory = max of 60 samples of private working set (Windows) / `phys_footprint` (macOS) | Lab, task 2.6; CI nightly reports | CPU < 0.1% (< 3.6 s CPU per hour); max memory < 20 MB |
| 20-app matrix per OS | ≥ 95% pass | [`compatibility-matrix.md`](compatibility-matrix.md): 100 cells per OS | Lab, task 2.7, every release candidate | ≥ 95 cells pass per OS; every failing cell has an exception record |
| Typed keys written to disk, logs or network | 0 | Canary scan (§5.3) after soak and after each matrix run | Lab, tasks 2.5, 2.8; CI nightly 30-min soak | 0 canary hits in any form |
| Line coverage of core engine | ≥ 85% | `cargo llvm-cov -p iamb-core --fail-under-lines 85` | CI Linux, task 1.6 onwards | Gate on every PR |
| Crash-free sessions in beta | ≥ 99.5% | Opt-in crash reports from `iamb-desktop`; session = one worker start to clean stop or crash; crash = abnormal exit of worker, supervisor or UI host | Beta telemetry, task 5.5 | ≥ 99.5% over the beta window with ≥ 500 sessions |
| Install to first expansion | < 2 min | Moderated session, screen-recorded; clock from installer launch to first correct expansion of a sample snippet; testers never used the product | 3 testers on own machines, task 3.7 | 3 of 3 under 2 min |
| Two online devices show same library | within 5 s | Device A writes a snippet (timestamp on A), device B's store publishes `LibraryChanged` (timestamp on B); clocks synced by NTP, offset measured before run and subtracted; 100 edits | Lab, two machines, task 4.4 | p95 < 5 s |
| Banned-term hits in repo | 0 | `cargo xtask banned-terms` | CI, every PR, task 0.4 | 0 hits |

### 2.2 Component targets

| Target | Method, sample, statistic | Where | Pass rule |
| --- | --- | --- | --- |
| Matcher lookup < 50 µs p99 at 10,000 snippets (1.2) | Bench `matcher/lookup/10000`: 10,000 individually timed lookups (90% hits, 10% misses) on the seeded fixture; `LatencyRecorder` p99 | CI Linux (report); lab | Lab p99 < 50 µs; CI p99 < 100 µs |
| 5 KB snippet renders < 1 ms (1.3) | Bench `template/render/5kb`: 5 KB template with fields, dates, conditionals; 1,000 timed renders; p99 | CI; lab | Lab p99 < 1 ms; CI p99 < 2 ms |
| 10,000 snippets load from SQLite < 100 ms (1.5) | Bench `store/load/10000`: open DB (warm OS cache), read all, build snapshot; 30 runs; p95 | CI; lab | Lab p95 < 100 ms; CI p95 < 200 ms |
| Hook callback < 1 ms p99 (2.1, 2.3) | In-process timing around the callback body (debug-only `latency-probe` feature), 100,000 keys during the scripted run; p99 | Lab only (hooks need a real desktop) | p99 < 1 ms |
| Service restart < 2 s (2.6) | Kill worker → supervisor restarts → IPC `Health` answers and a probe expansion succeeds; 20 kills during soak; max | Lab; CI nightly report | max < 2 s |
| UI cold open < 400 ms (3.2) | Library window: process start (or create, when host warm) → first paint; 20 trials with the UI host warm; first open after login reported separately; p95 | Lab | p95 < 400 ms |
| Palette results < 30 ms at 10,000 snippets (3.6) | Bench `store/search/10000` (query → ranked IDs) plus UI render timing in the palette; 500 queries from a fixed list; p95 | CI (store part); lab (end to end) | Lab p95 < 30 ms; CI store part p95 < 30 ms |

## 3. Benchmark harness (`crates/iamb-bench`)

| Item | Decision |
| --- | --- |
| Framework | criterion 0.8 with default features off (no plotters, rayon or HTML reports) for mean/median and change detection |
| Percentiles | `iamb_bench::LatencyRecorder`: keeps every nanosecond sample, sorts, reports min/p50/p95/p99/max/mean by nearest rank (`rank = ceil(q·n)`), integer arithmetic only. Every percentile KPI bench runs a recorder pass after criterion |
| Bench files | One file per area: `benches/matcher.rs`, `template.rs`, `store.rs`, `engine.rs`, `search.rs`; `benches/sample.rs` is the placeholder baseline until task 1.2 |
| Bench ids | `<area>/<operation>/<size>`, lower case, e.g. `matcher/lookup/10000`, `template/render/5kb`, `store/load/10000` |
| Fixtures | `iamb_bench::generate_library(seed, n)`: SplitMix64, unique abbreviations (prefix + 2–8 chars), ASCII bodies 20–500 chars with newlines. Committed seed `DEFAULT_SEED`, size 10,000. Same seed → same library on every OS (unit-tested) |
| Result format | One JSON-lines file per run: `target/bench-results/<unix_ms>-<bench file>.jsonl`; one line per measurement with `schema, run_id, timestamp_ms, git_sha, os, arch, runner, bench, kpi, unit, count, min, p50, p95, p99, max, mean, target_stat, target_ns, pass, note`. Raw sorted samples in `<run_id>.samples/<bench>.txt` |
| Run modes | CI: `cargo bench -p iamb-bench -- --quick`. Lab: full mode, `IAMB_BENCH_RUNNER=lab-<os>-<n>`, laptop on mains power, results committed under `docs/results/` |

**Regression rule.** Report-only for now: the `Bench` workflow runs the matcher and template
benches, uploads results and never blocks a merge. The plan put the baseline in task 1.9, but a
baseline has to come from runs of the real benches on `main`, and those only start once Phase 1
merges. So the first task of Phase 2 collects 10 runs on `main` to measure each bench's spread
and commits `crates/iamb-bench/baselines/ci-ubuntu.jsonl`.
From then on CI fails a PR when a bench's gated statistic is **> 25% worse than baseline and the
absolute difference exceeds the noise floor** (twice the bench's measured spread, at least 1 µs),
**on two consecutive runs**. The CI absolute thresholds in §2.2 (2× the lab target) apply at the
same time. Baselines change only in a PR that says why.

## 4. What CI runners can and cannot measure

| Can | Cannot |
| --- | --- |
| Unit, property, spec, session, IPC, service-vs-mock tests on Linux, Windows, macOS | Official latency KPIs (shared CPU, variable clocks) |
| Bench regressions with generous thresholds | macOS event tap and injection: hosted runners cannot grant Accessibility or Input Monitoring, so these tests are `#[ignore]` there and run in the lab |
| Coverage, fuzzing, banned terms, dependency deny list | Hook callback p99, UI paint timing, real app matrix |
| 30-minute soak with the mock platform; canary scan of disk and logs | Windows hook and `SendInput` tests run on `windows-latest` only when the runner has an interactive desktop; if not, they are skipped with a logged reason, never silently passed |

## 5. Long-run (soak) test

### 5.1 Set-up

| Item | Value |
| --- | --- |
| Driver | `iamb-testkit::soak`, a separate process |
| Target | Real `iamb-service` (supervisor + worker); lab uses the real OS layer typing into the probe window; CI uses the mock platform |
| Key stream | Seeded: 70% ordinary words, 30% abbreviations across all trigger and case modes; 3 keys/s average with 15 keys/s bursts; library of 10,000 fixture snippets; 1 library edit per minute through IPC |
| Sampling | Every 60 s: RSS / private working set, process CPU time, handle or file-descriptor count, thread count, expansion success count |
| Fault injection | Every 6 h: hard-kill the worker; every 24 h: restart the UI host; once: fill the disk quota of the data root for 5 min |
| Duration | Lab: 72 h per OS (task 2.6, then each release candidate). CI: 30 min nightly on Linux, same rules scaled |

### 5.2 Pass rules

- No unexpected exit of supervisor or worker (injected kills excluded); each injected kill recovers
  in < 2 s.
- Leak rule 1: least-squares slope of memory over hours 1–72 (first hour is warm-up), multiplied by
  71 h, is < 5 MB.
- Leak rule 2: mean memory of the last hour minus mean of hour 1 is < 5 MB.
- Handle and thread counts return to their hour-1 range after each kill and stay flat.
- Expansion correctness during the run ≥ 99.9%.

### 5.3 Canary privacy scan (also the method for the "0 typed keys" KPI)

1. The driver generates canaries per run: `qz` + 14 random lowercase letters (never an
   abbreviation). It types them as ordinary text, inside password fields, in excluded apps, and
   while paused.
2. During the run: capture all network traffic of the machine (`tcpdump` on macOS, `pktmon` on
   Windows) and all IPC frames (test build of `iamb-ipc` with a frame tap).
3. After the run: search for every canary, in UTF-8, UTF-16LE and Base64 forms, in the data root,
   logs, crash dumps, OS temp directories, the whole user profile, the IPC tap and the packet
   capture.
4. Pass: 0 hits outside the probe window's own text and the driver's own input file. A worker memory
   dump taken after a boundary event is also scanned; hits there are reported, not gating.

## 6. End-to-end latency measurement (lab)

The **probe window** (`iamb-probe`, a test-only app in `iamb-testkit::compat`, task 2.7; never
shipped) is a native window with one text control (Win32 Rich Edit on Windows, `NSTextView` on
macOS) plus a local web page variant for browser rows (`performance.now()` on `input` events).

| Step | Detail |
| --- | --- |
| Drive | The driver process injects the abbreviation as ordinary synthetic keys (unmarked, so the listener sees them like real typing) |
| T0 | Probe records the monotonic time (QueryPerformanceCounter / `mach_absolute_time`) when its control receives the last abbreviation key |
| T1 | Probe records the monotonic time of the first text-change event after which the control's text equals the expected result (abbreviation removed, expansion complete) |
| Result | Latency = T1 − T0; both stamps on one clock in one process, so no cross-process clock error |
| Report | Probe writes samples to the `LatencyRecorder` format; driver writes the JSON-lines result with `kpi = "e2e-plain-p95"` |
| Timeout | No match within 2 s counts as a failed expansion and as 2 s latency |

The same probe produces the "correct expansions" KPI and the type-1 timing cells of the matrix.

## 7. Task ownership (phases 1–4)

| Task | Owns |
| --- | --- |
| 1.1 | Model unit and property tests |
| 1.2 | Matcher unit, property and mutation tests; `matcher/lookup/10000`; replaces `benches/sample.rs` |
| 1.3 | Spec example suite and loader; fuzz targets; `template/render/5kb` |
| 1.4 | Date math property tests (month ends, leap years, DST) in spec examples |
| 1.5 | Store tests; `store/load/10000`, `store/search/10000` |
| 1.6 | Engine sessions; coverage gate; engine-share latency on mock |
| 1.7–1.8 | Import/export round-trip properties; CLI integration tests |
| 1.9 | Review; mutation re-run; coverage gate and nightly fuzz and mutation jobs wired up (baselines and the regression gate move to the start of Phase 2, see §3) |
| 2.1–2.4 | OS-layer tests on runners; lab 10,000-expansion runs; hook p99 |
| 2.5 | Secure field, exclusion and pause cases; canary scan |
| 2.6 | Soak harness, idle and restart measurements |
| 2.7 | Probe window, compat harness, matrix runs, e2e latency |
| 2.8 | Privacy review using the canary scan results |
| 3.2, 3.5, 3.6 | UI cold open, form and palette timings |
| 3.7 | Install-to-first-expansion sessions |
| 4.2–4.4 | Server policy tests (`server/tests/`), sync convergence tests, two-device latency |
