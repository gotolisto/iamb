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
| Sync conflict scenarios | TOML scenarios with a defined outcome each (`docs/sync-design.md` §12) | `spec/sync/scenarios.toml`; the format and its check in `iamb-testkit::sync_scenarios`, run by `crates/iamb-testkit/tests/sync_scenarios.rs`; replayed by the two-device simulation of task 4.4 | CI, 3 OSes, every PR | At least 30 scenarios; unique ids; every row stated for the server and every device; every case the task and the three reviews of the design ask for covered; each outcome agrees with the scenario's own steps and answers (the server's row is the last write that reached it; the history holds exactly what was replaced; a write over a version its device had not seen has its conflict, of the right kind, shown to its authors; a refused change is kept and a blocked one stays; a device that ends online holds what the server holds, and every live row of its user's own profile; a device holds a row of someone else's profile only where a membership or a share reaches that profile for its user, and a device that was signed out or revoked and not signed in again holds no version that reached the server afterwards; a stated field is what the steps left in that version, the author of a snippet's content is the one the steps give, a snippet that must be reviewed is held on its readers' devices and not on its author's, and the snippet stated to expand is not behind another as far as author, profile and the server's order decide; after a restore of the server, a version a device of the profile's owner held and the backup lacks ends as the row or in the history, one a device of its writer held ends there or among that device's kept changes, a device whose session the restore ended and that was not signed in again still holds it, the later of two lost versions of a row stays, and a removal the restore lost is made again, by the operator's replay or by the device that made it, before that device gives a share with the same group back); the check refuses files broken on purpose, among them the eight wrong outcomes of the first review (`docs/reviews/4.1-sync-design-review.md`, finding 15) eleven of the thirteen of the follow-up review (`docs/reviews/4.1-sync-design-followup.md`, §4), four of the eight the close-out review saw pass (`docs/reviews/4.1-sync-design-closeout.md`, §5) and that review's findings K1, K2 and K5 as the design then was; the design document writes out every scenario and defines every rule cited. The check takes the answers, the cascades and which folders of a profile a user may see from the scenario and does not derive them, and it does not know a mark only the server sets or whether a device should have added a link; three wrong outcomes that rest on the first three are in the tests as examples that pass. From task 4.4: every scenario replays to its outcome, derived from the rules by the real store and sync client and a model of the server |
| IPC contract | `cargo test` with golden JSON | `crates/iamb-ipc/tests/contract/` (one golden file per message) | CI, every PR | Every message round-trips byte-for-byte; role and version rules tested |
| Service vs. mock platform | `iamb-platform` mock + `iamb-testkit` | `crates/iamb-service/tests/` | CI, 3 OSes, every PR | Must pass |
| UI host: bridge, message contract, crash isolation | `cargo test -p iamb-desktop` (a worker on the mock platform in the test process) | `apps/desktop/src-tauri/tests/` (§8) | CI job `desktop`, 3 OSes, every PR | Must pass |
| UI host front end | `vitest` and `tsc` | `apps/desktop/src/**/*.test.ts(x)` | CI job `desktop`, 3 OSes, every PR | Must pass |
| OS layer | `cargo test -- --ignored` on runners | `crates/iamb-platform-{windows,macos}/tests/` | CI `windows-latest`, `macos-latest` | Must pass where the runner allows (see §4) |
| Compatibility matrix | `iamb-testkit::compat` + manual | [`compatibility-matrix.md`](compatibility-matrix.md) | Real machines, task 2.7 and every release candidate | ≥ 95% per OS |
| Benchmarks | criterion + `LatencyRecorder` | `crates/iamb-bench/benches/` | CI, every PR (two runs); real machines for KPIs | Regression rule against the CI baseline, §3 |
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
| UI cold open < 400 ms (3.2) | Library window (created on demand, so every open is cold): the tray's "Open library" action → the window's first paint, which the front end reports; `iamb-desktop --measure-open 20` with the UI host warm; the first open after the host started reported separately; max and median (§8) | Lab; CI every PR (Linux under Xvfb gated at 800 ms, twice the lab target as §2.2 sets for CI; Windows and macOS report only) | Lab max < 400 ms |
| Fill-in form visible, the UI host's part (3.5) | `FormRequested` reaching the UI host → the form panel's first paint after it was shown, which the front end reports as the cold open does; `iamb-desktop --measure-form 20` after the hidden panel painted once; the first form after start reported separately; max and median (§8.5). The service's part (the match to `FormRequested` and the round trips `FormAck`, `GetSnippet`, `PanelFocus`) is timed in `tests/form.rs` | Lab; CI every PR (Linux under Xvfb gated at 300 ms, twice the target as §2.2 sets; Windows and macOS report only) | Lab max < 150 ms; the end-to-end KPI of §2.1 stays the lab's |
| Palette results < 30 ms at 10,000 snippets (3.6) | Bench `search/query/10000` (`iamb_core::search`: query to the 50 best hits with their fields and ranges): 500 queries from a fixed mix (abbreviation prefixes, words, fragments, typos, two words, queries that match nothing), each timed, p95; and `iamb-service/tests/search_timing.rs`, the same through IPC and the worker (400 queries, release build); the rows are drawn from a list in memory, no request (§8.7) | CI Linux (the bench, `IAMB_SEARCH_BUDGET_MS=60`, and the release test, both gated at twice the target); lab (end to end) | Lab p95 < 30 ms; CI p95 < 60 ms |
| Search palette visible, the UI host's part (3.6) | `PaletteRequested` reaching the UI host → the palette panel's first paint after it was shown, which the front end reports as the form does; `iamb-desktop --measure-palette 20` after the hidden panel painted once; the first palette after start reported separately; max and median (§8.7). The service's part (the hotkey to `PaletteRequested`, `PanelFocus`) is timed in `tests/palette.rs` | Lab; CI every PR (Linux under Xvfb gated at 200 ms, twice the target as §2.2 sets; Windows and macOS report only) | Lab max < 100 ms; the end-to-end KPI of §2.1 stays the lab's |
| Library list scrolls at 60 frames per second with 10,000 snippets (3.3) | `apps/desktop/scripts/scroll-kpi.mjs`: the built front end in Chromium (Playwright) with a mocked command layer, 10,000 snippets, `requestAnimationFrame` gaps over 10 s of scrolling per driver; p95 and dropped frames (§8.4) | CI Linux (report); lab on each OS's web view | p95 frame time ≤ 16.8 ms and no frame over 25 ms at 1× CPU; lab on WebView2 and WKWebView |

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

**Regression rule.** CI fails a PR when a bench's gated statistic is **> 25% worse than baseline
and the absolute difference exceeds the noise floor** (twice the bench's measured spread, at least
1 µs), **on two consecutive runs**. The CI absolute thresholds in §2.2 (2× the lab target) apply at
the same time. Baselines change only in a PR that says why. Exactly (`xtask/src/bench.rs`):

| Term | Definition |
| --- | --- |
| Gated statistic | The record's `target_stat` when set, else `p50` |
| Baseline value | Median of the gated statistic over the baseline runs; for an even number of runs, the mean of the two middle values, rounded down to whole nanoseconds |
| Spread | Maximum minus minimum of the gated statistic over the baseline runs (the range, not a standard deviation) |
| Noise floor | `max(2 × spread, 1 µs)` |
| Regression in one run | `value > 1.25 × baseline` **and** `value − baseline > floor`, both strict, integer arithmetic |
| Two consecutive runs | On a pull request the `Bench` workflow runs the four quick benches (matcher, template, store, engine) twice, one after the other in the same job on the same runner, writing to `target/bench-results/run-1` and `run-2` (`IAMB_BENCH_RESULTS_DIR`). The gate fails only when the same bench regresses in both runs; a regression in one run only passes and is shown in the table |

`cargo xtask bench-compare --baseline crates/iamb-bench/baselines/ci-ubuntu.jsonl <run-1> <run-2>`
applies the rule, prints a table (bench, baseline, run 1, run 2, change, floor, verdict) and writes
it to the job summary. Benches not in the baseline are reported as new and never fail; a baseline
bench missing from either run is an error, so a bench that crashes fails the gate (the bench steps
themselves are report-only). Both runs must be of one commit and of the baseline's runner. While the
baseline file does not exist the gate prints "not armed yet" and passes. If a slow runner fails both
runs, re-running the job gets a fresh one. Pushes to `main` and manual runs run the benches once,
report only, and upload the result files; the baseline is built from those.

The baseline file has one JSON line per bench: `schema, bench, stat, unit, baseline, spread, runs,
git_sha, runner`. To create or refresh it (the first task of Phase 2 creates it; later refreshes
only in a PR that says why, e.g. a new bench, an accepted slowdown or a runner image change):

1. Start 10 manual runs of the `Bench` workflow on one commit of `main`; each gets its own
   concurrency group, so they run on separate runners:
   `for i in $(seq 10); do gh workflow run bench.yml --ref main; done`
2. Download each run's artifact into its own directory:
   `gh run list --workflow bench.yml --event workflow_dispatch --limit 10 --json databaseId --jq '.[].databaseId' | xargs -I{} gh run download {} -D runs/{}`
3. `cargo xtask bench-baseline runs/*` reads every `*.jsonl` under each directory, refuses runs of
   different commits or runners and names any bench missing from some runs, writes
   `crates/iamb-bench/baselines/ci-ubuntu.jsonl` (`--out <file>` to write elsewhere) and prints
   each bench's median, spread and floor.
4. Commit the file in a PR whose description says why and includes the printed table.

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
- Leak rule 2: within one worker life, mean memory of the life's last hour minus mean of its hour 2
  (the hour after its warm-up) is < 5 MB. The life is the last one if it holds both windows, else the
  longest that does; if none does, the rule is skipped, not passed. Never across lives: each worker
  settles at its own plateau.
- Handle and thread counts return to their hour-1 range after each kill and stay flat.
- Expansion correctness during the run ≥ 99.9%.

### 5.3 Canary privacy scan (also the method for the "0 typed keys" KPI)

1. The driver generates canaries per run: `qz` + 14 random lowercase letters (never an
   abbreviation). It types them as ordinary text, inside password fields, in excluded apps, and
   while paused.
2. During the run: capture all network traffic of the machine (`tcpdump` on macOS, `pktmon` on
   Windows) and all IPC frames (the soak harness serves the service endpoint through a tapping
   endpoint wrapper, §5.5; on CI a dependency check replaces the capture).
3. After the run: search for every canary, as a whole string (UTF-8, UTF-16 and UTF-32 in both
   byte orders, Base64, hex, escapes) and one character at a time (at a constant stride, or one
   character per log line), in the data root, logs, crash dumps, OS temp directories, the whole
   user profile, the IPC tap and the packet capture.
4. Pass: 0 hits outside the probe window's own text and the driver's own input file. A worker memory
   dump taken after a boundary event is also scanned; hits there are reported, not gating.

### 5.4 Running the harness (`iamb-soak`)

`iamb-soak` (in `crates/iamb-testkit`, feature `soak`, on by default) is the driver and also
starts the two service processes, so a run is three processes:

| Process | Command | Runs |
| --- | --- | --- |
| Driver | `iamb-soak run …` | Seeds the library, types, edits, samples, kills, types the canaries, stops the service, scans, applies the rules |
| Supervisor | `iamb-soak serve-supervisor` (started by the driver) | `iamb_service::supervisor::run` unchanged (lock, backoff 0/1/5/30 s, heartbeat 3 s), with `iamb-soak serve-worker` as its worker command |
| Worker | `iamb-soak serve-worker` (started by the supervisor) | `iamb_service::worker::Worker` unchanged on the mock platform: heartbeat on standard output, exit when standard input ends, `logs/worker.log`, the real service endpoint through the IPC tap, plus the control endpoint |

```sh
# Build (release for real runs; the CI job does this)
cargo build --release --locked -p iamb-testkit --bin iamb-soak

# CI form: what the nightly job runs (about 31 min)
target/release/iamb-soak run --duration 30m --out target/soak/nightly --net-check .

# Short local smoke run (about 15 s; the cargo test below runs the same)
target/debug/iamb-soak run --duration 12s --library 300 --keys-per-second 60 \
  --sample-every 1s --edit-every 2s --canary-rounds 1 --out target/soak/smoke

# Lab duration (72 h is the default); add the places only the lab can scan
target/release/iamb-soak run --out soak-72h --scan "$HOME" --scan /path/to/capture.pcapng

# Scan more places afterwards for the canaries of a run (exit status 1 on a hit)
target/release/iamb-soak scan --canaries soak-72h/input/canaries.json \
  --location profile="$HOME" --location capture=/path/to/capture.pcapng

# The networking-crate check alone
target/release/iamb-soak net-check --workspace .
```

`cargo test -p iamb-testkit --test soak` runs the 12-second smoke run through the binary, checks
its files, that the tap holds the service's frames, and that a canary planted afterwards is
found and reported by number, form and location only. It runs on Linux and is ignored on Windows
and macOS (the CI soak is Linux-only; the harness compiles everywhere).

| Flag of `run` | Default | Meaning |
| --- | --- | --- |
| `--duration` | `72h` | Run length (`30m`, `90s`, `1h30m`, …) |
| `--out` | `target/soak/<unix ms>` | The run's directory; must not hold an earlier run. Keep it short (or relative): the Unix socket paths inside it are limited to about 100 bytes |
| `--seed` | from the clock | Key stream, library and edits; printed and kept in the report. Canaries never come from it (OS generator) |
| `--library` | `10000` | Fixture snippets (at most 17,576) |
| `--keys-per-second` | `3` | Average typing rate; raise it for a short local run (60 for the smoke run) |
| `--sample-every` | `60s` | Sample interval |
| `--edit-every` | `60s` | One library edit through IPC (`GetSnippet`, `PutSnippet`, wait for the new revision) |
| `--kill-every` | `auto` | Worker hard-kills: `auto`, `off`, or a duration (see the decisions below) |
| `--canary-rounds` | `3` | Rounds at ⅙, ½ and ⅚ of the run; each types one canary per context |
| `--net-check WORKSPACE` | off | Also run the networking-crate check (the nightly job passes `.`) |
| `--scan PATH` | — | Another location to scan (repeatable) |
| `--quiet` | off | No progress lines on standard error |

Exit status: 0 every rule passed, 1 a rule failed, 2 the run could not be set up. Files under
`--out` (the nightly job uploads the first four, the logs and `service-stderr.log` as the
artifact `soak-<run id>`):

| File | Holds |
| --- | --- |
| `samples.jsonl` | One line per sample (`iamb-soak-sample/1`): `t_ms`, `unix_ms`, `supervisor` and `worker` (`pid, alive, rss_kb, private_kb, cpu_ms, handles, threads`), `worker_life`, `life_started_ms`, running totals of expansions, keys, edits, kills |
| `report.json` | `iamb-soak-report/1`: the commit (`git_sha`: `GITHUB_SHA`, else `git rev-parse HEAD` in the current directory or the soak binary's; `git_sha_source` says which, or why it is null), options, totals (keys, rate, expansions, edits, CPU, max memory, restarts the supervisor logged), every recovery (control, `Health` and probe times), unexpected events, the first 50 expansion failures (snippet number, shape, case mode; never field text), per canary its context and checks (never its text), the crash dumps found and where the search looked, the network check, every rule's verdict |
| `canary-scan.json` | Locations with files and bytes scanned; hits as canary number, context, form, location, file and byte offset; unreadable files |
| `summary.txt` | The verdicts in plain text (also printed) |
| `input/canaries.json` | The driver's own input file: the only file the harness writes canaries to; not uploaded |
| `work/data/` | The data root (`library.db`, `logs/supervisor.log`, `logs/worker.log`, `run/`) |
| `work/tmp/` | `TMPDIR`, `TMP` and `TEMP` of the service processes |
| `work/ipc-tap/` | `worker<pid>-conn<n>-in.bin` / `-out.bin`: every byte each service-endpoint connection read or wrote |
| `work/service-stderr.log` | Standard error of the supervisor and every worker |

### 5.5 Decisions in the harness

- **Service processes.** The shipped `iamb-service --platform mock` cannot be typed into from
  another process (its mock lives inside the worker) and its supervisor always starts its own
  executable, so the soak binary hosts the service's library code instead: the supervisor loop
  and the worker are the service's own, unchanged; only argument parsing and the two test hooks
  below are the harness's. Nothing in `iamb-service`, `iamb-ipc` or `iamb-platform` changed.
  Peers are checked with the same-user policy in every build (the soak binary is not the signed,
  installed binary).
- **IPC tap.** `MessageLog` sees only frame names and sizes, so the tap is a `LocalEndpoint`
  wrapper (`soak::tap`) around the native endpoint that the worker serves on (the worker takes
  any endpoint). It copies every byte of every accepted connection, one file per connection and
  direction, so a frame is contiguous in the file and a canary split across two reads is still
  found. It covers every frame of the service endpoint: hello, welcome, requests, responses,
  events.
- **Typing.** The worker also serves a control endpoint (`soak::control`, JSON lines; named like
  a sync endpoint in a directory of its own) through which the driver types into the mock's
  windows and reads their fields back: the stand-in for the OS keyboard and the probe window.
  Its traffic is the driver's input; it is never written to a file. The mock desktop has an
  ordinary editor, a password field and a window of an app the driver puts on the exclusion list
  (`PutExclusion`).
- **Key stream.** Units of 8 to 16 tokens in a cleared field, starting with a word; 30% of tokens
  are abbreviations in a case variant their mode expands, 70% ordinary words (some capitalised,
  some followed by `,` or `.`). One token in five is typed as a burst at five times the average
  rate (15 keys/s at the default 3), the others with exponential gaps of mean 0.4 s, so the mean
  is 3 keys/s; canaries are typed as bursts. After each abbreviation the driver waits for the
  engine to settle and compares the whole field with the expected text; a mismatch is a failed
  expansion and ends the unit.
- **Fixture library.** Three shapes so that no abbreviation can fire inside a word, another
  abbreviation or a canary: 60% `v` + three letters (delimiter, word start), 15% `v` + three
  letters + `9` (delimiter, no word start), 25% `;k` + three letters (immediate); case modes
  exact, insensitive and adaptive cycle within each shape; plain-text bodies of 20 to about 300
  characters, some with a line break. Ordinary words never start with `v`.
- **Window `W`.** The rules' "hour" is `W = D / 72`, at least five sample intervals and eight
  edit intervals, at most `D / 4`: 1 h for the lab run, 7.5 min for the CI run. The edit floor is
  measured, not guessed: with 10,000 snippets the worker's private memory is 37 MB before the
  first library edit, about 49 MB after it, and flat at 57–60 MB from the fourth edit on (each
  edit builds a new snapshot while the old one is in use; the allocator keeps the high-water
  mark). A 2-minute window reported that climb as a 20 MB leak.
- **Warm-up in both leak rules.** §5.2 calls hour 1 warm-up for rule 1. The harness applies the
  same to rule 2: it compares the last `V` of one worker life with the first `V` after that life's
  warm-up (`[start + W, start + W + V]`; hour 2 of the first life in the lab), not with hour 1,
  which would charge the climb above to the 5 MB budget. `V` is `W`, or half of the life's settled
  stretch (its length minus `W`) when that is shorter, as it is for every life of the CI run. Every
  worker life has its own warm-up `W`; samples inside it count for neither leak rule nor for the
  handle and thread ranges.
- **Rule 2 within one life.** Two lives settle at different plateaus (a worker started later holds
  a different allocator state), and a nightly run failed with no leak when rule 2 compared life 1
  with life 8. The windows therefore come from one life: the last, else the longest that holds both
  windows (each at least two samples, not overlapping), else the rule is reported skipped with the
  reason. A real leak in one life still fails it.
- **Leaks across kills.** A kill resets the worker's memory, which would hide a leak in a plain
  slope. Rule 1 therefore uses the least-squares slope pooled within worker lives (one intercept
  per life), times `D − W`.
- **Kills.** `auto` spreads up to 12 kills (§5.1: every 6 h of 72 h) evenly between `W` + 4
  samples after the start and `W` + 4 samples before the end, at least 65 s apart (a worker must
  run 60 s before the supervisor restarts it with no delay; a closer kill would measure the 1 s
  backoff). So the first and the last worker each keep a settled stretch for the leak rules. Lab:
  12 kills, about every 6.4 h. CI: 7 kills, 70 s apart, between minutes 11.5 and 18.5. A run too
  short for that stretch (the smoke run) gets one kill in the middle. §2.2 asks for 20 kills in
  the lab: pass `--kill-every 3h` there (23 kills).
- **Recovery time.** From the kill to: the control endpoint answering from a new process, then
  IPC `Health` answering on a new connection, then a probe expansion (`probe vaaa ` typed, the
  field checked). The probe time is the recovery time of the rule; all three are reported.
  "Unexpected exit": the supervisor exits, the worker's pid changes without a kill, a connection
  breaks outside a kill, or the supervisor's log counts more worker restarts than kills.
- **Handles and threads.** Each must stay within the range seen in the first `W`, give or take
  `max(2, 10%)`, except within `W` after a restart.
- **Canary contexts.** Each round types one canary per context: `ordinary` (in a sentence),
  `before-expansion` (directly followed by a no-word-start abbreviation, so the canary is in the
  engine's buffer when an expansion fires), `secure-field`, `excluded-app`, `paused` and `panel`
  (typed while the driver, as the UI host, says the palette has the keyboard with `PanelFocus`:
  the 2.8 review's finding 16). In the last four an abbreviation is typed after the canary and
  must not expand, and `GetStatus` must report the engine suspended (and paused); in the first
  two it must not be. A canary counts as
  typed only when the field then holds exactly the expected text.
- **Scan.** Whole-string forms (`soak::canary`): UTF-8, UTF-16 and UTF-32 in both byte orders,
  upper case, reversed, hex, JSON `\u` escapes and percent encoding (both cases of hex digit),
  the debug form of a byte slice (`113, 122, …`), and Base64 of UTF-8 and UTF-16LE (standard and
  URL-safe alphabets, at each of the three alignments, only the characters that do not depend on
  neighbouring bytes). Per-character forms (`soak::spread`, finding 2 of the 2.8 review): the
  canary's bytes at a constant stride from 2 to 512 bytes (UTF-16, UTF-32, arrays of `char`, the
  debug form `['q', 'z', …]`, fixed-size records such as a 96-byte queue slot, fixed-width log
  lines), and one character per line of one template after removing digits (a log that records
  every key: the lines are the same apart from one column, and the canary stands in that column
  in consecutive lines of that template, at most 16 other lines between two of them), and the
  same with each character written as its decimal code (`key 113 seq 46`: every run of digits in
  a line is read as a number and replaced by one marker, and the canary's codes stand at one
  number's place in consecutive lines of one template; the follow-up review's N3). All three need
  all 16 characters in order, so ordinary output does not trigger them (a random letter matches
  a fixed start and stride with probability 26^-15; repeated log lines stop a chain at the first
  wrong letter); the unit tests feed them random text, random bytes and real logs; on the 2.8 review's own
  runs the 3-minute run gives 0 hits, while the 40-second run of its build that logs every key
  gives a hit for each of its 10 canaries. Not found: characters written as hexadecimal numbers
  one per line, records of varying length that are not lines, compressed output, another
  order. Locations: the data root (library, WAL, logs, locks),
  the temp directory the service processes used, the IPC tap, their standard error, the control
  directory, the samples file, core files and crash dumps of the service processes (below), and
  any `--scan` path. A file that cannot be read fails the rule (0 hits cannot be claimed for
  it). The rule `canary-scanner-control` requires that scanning the driver's own input finds
  every canary and that every shape of every canary (30: each whole-string form inside other
  text, each Base64 alignment, 96-byte records, the debug form of a `char` slice, one key per log
  line in the service's log format, as a character and as its decimal code), planted in files of its own next to the input and removed
  again, is found, so a scanner that stops finding a form cannot pass. Hits are reported by
  canary number, context, form, location, file and byte offset (for `per-line` and
  `per-line-decimal`, of the line with the first character); no report, summary or progress line contains a canary or the bytes
  around it.
- **Crash dumps.** The two service processes opt out of crash dumps first, as the service's
  `main` does (architecture §8, "Crash handling"): on Linux a core file of the worker holds no
  memory. After the run the driver looks for core files and crash dumps changed during the run:
  `core` and `core.*` in the service processes' working directory (the driver's), the directory
  of `/proc/sys/kernel/core_pattern` when that names one (a pattern that hands cores to a
  program is reported as not searched), `/cores/core.<pid>` on macOS for the run's pids, and
  `iamb-soak.exe.*` in `%LOCALAPPDATA%\CrashDumps` on Windows. Each one found is scanned
  (location `crash-dump`) and fails the rule `no-crash-dumps`: no service process may crash.
- **Network.** CI cannot capture traffic. Instead the rule `no-networking-crate` runs
  `cargo tree -p iamb-service -e normal --target all` and checks every package against the deny
  list of `crates/iamb-service/tests/no_network.rs` (a unit test keeps the two lists equal). The
  lab run adds the packet capture (below).
- **Samples.** Linux: `/proc/<pid>/{status,stat,smaps_rollup,fd}`; private memory is
  `Private_Clean + Private_Dirty`, CPU time in `USER_HZ` = 100 ticks. Windows: PowerShell
  `Get-Process` (private bytes, handle count). macOS: `ps` and `lsof`; RSS only, so the lab
  takes `phys_footprint` with `footprint` itself. The mock keeps a log of every injection; the
  harness empties it at each unit so the mock's own bookkeeping does not grow the worker.

What the CI form leaves to the lab run:

| Lab only | Why | How |
| --- | --- | --- |
| Real OS layer typing into the probe window | The probe window exists (§6) but `iamb-soak` cannot drive it yet: its worker runs the mock platform in-process and the driver types and reads back through the worker's control endpoint. A `--target probe` needs a worker on the real platform (the service's own worker, with the tap), a typist fast enough for 15 keys/s bursts (the compat typist starts a process per call, 0.3–1 s), the probe's text check in place of the control endpoint's `Check` (a probe command that compares without returning the text), two more probe windows for the `secure-field` and `excluded-app` canary contexts (a password control and a second app id), and the recovery probe expansion through it | Until then a lab run is the mock-platform run on the lab machine (the service minus its OS layer) |
| Packet capture of the whole machine | Not possible on hosted runners | `tcpdump` (macOS) or `pktmon` (Windows) for the whole run; scan the capture with `iamb-soak scan --location capture=…` or `run --scan` |
| Crash dumps of other programs, the whole user profile | Too large and too shared on CI (the run's own core files and dumps are found and scanned automatically) | `--scan "$HOME"` (or `%USERPROFILE%`) plus the OS crash-dump directories by name: `%LOCALAPPDATA%\CrashDumps` and `C:\ProgramData\Microsoft\Windows\WER` (Windows), `/cores` and `~/Library/Logs/DiagnosticReports` (macOS) |
| UI host restart every 24 h | No UI host yet | Add to the driver with task 3.x |
| Disk quota of the data root full for 5 min | Needs a quota'd volume | Lab procedure |
| Worker memory dump after a boundary event (reported, not gating) | Needs an OS dump tool | Lab procedure; scan the dump with `iamb-soak scan` |
| Official memory numbers (private working set, `phys_footprint`) | Mock and shared runner | Lab |

Measured on the build machine (Linux, 4 cores, release build, mock platform, the nightly job's
command, seed 20261118): pass; 1801 s, 38 samples, 5,758 keys (3.2/s while typing), 318 of 318
expansions correct, 29 edits, 7 kills recovered in 48–75 ms (probe expansion), 15 canaries in 5
contexts typed and confirmed, 0 hits in 25 files (4.4 MB: data root, IPC tap, stderr, samples),
no networking crate among 47 packages. Worker private memory 37 MB at start, flat at 57–58 MB
from the fifth edit; leak slope 2.0 MB (limit 5), window difference 0.6 MB; supervisor 0.3 MB,
flat. The 2.0 MB slope comes from nine settled samples in two lives: the CI rule detects a gross
leak, not one near the limit, which is the lab's job. A 4-minute run with an edit every second
(224 edits) stayed between 51 and 59 MB.

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

### 6.1 Running it (`iamb-compat latency`)

Two processes: the driver `iamb-compat latency` and the probe `iamb-probe`, which the driver
starts. The service runs on its own data directory, as for the matrix
([`compatibility-run.md`](compatibility-run.md), "End-to-end latency"):

```sh
# Build: the service alone, then the harness and the probe (feature `probe`, off by default)
cargo build --release -p iamb-service
cargo build --release -p iamb-cli -p iamb-testkit
cargo build --release -p iamb-testkit --features probe --bin iamb-probe

# The 1,000 latency snippets next to the matrix's five; then start the service on that folder
target/release/iamb-compat setup --data-dir ~/iamb-compat --latency

# Check the probe on this machine without Iamb (types into itself, prints T1 - T0)
target/release/iamb-probe --self-test

# The KPI run: 1,000 trials into the native probe window (about half an hour)
IAMB_BENCH_RUNNER=lab-win11-01 target/release/iamb-compat latency

# A short trial run, and the browser variant for the browser rows
target/release/iamb-compat latency --trials 50
target/release/iamb-compat latency --target browser --open --trials 100
```

| Item | Decision |
| --- | --- |
| Where the code lives | Rules, protocol, browser page and driver: `iamb-testkit` (`compat::probe`, `compat::latency`), no OS code, tested on every system. The native windows: `iamb_platform_windows::probe` and `iamb_platform_macos::probe`, the only crates where `unsafe` may be, behind their `probe` feature. `iamb-testkit`'s `probe` feature turns those on and builds the `iamb-probe` binary; it is off by default and no shipped binary enables it (the service is built on its own) |
| Windows control | Rich Edit (`RICHEDIT50W` of `Msftedit.dll`), multi-line, plain-text mode, in a window of the system dialog class subclassed for its `EN_CHANGE` notifications. T0: `Instant::now()` (`QueryPerformanceCounter`) when the message loop takes the control's `WM_KEYDOWN` whose `WM_CHAR` completes the abbreviation. T1: `Instant::now()` on the `EN_CHANGE` after which the text equals the expected result |
| macOS control | An `NSTextView` subclass (plain text; quote, dash, spelling, replacement, completion and link substitution off) as the window's content, on the main thread under `run_with_main_loop`, activation policy `Regular`. T0: `Instant::now()` (`CLOCK_UPTIME_RAW`, the clock of `mach_absolute_time`) at the top of `keyDown:`. T1: at the top of `didChangeText` |
| Browser control | A `<textarea>` on a page the probe serves on `127.0.0.1` only (random port and per-run token in the path; requests with another `Host` or `Origin` are refused). T0 and T1 are the page's `performance.now()` at `keydown` and `input`, sent back over loopback; the page is served cross-origin isolated so browsers keep the timer fine-grained. The server is in `iamb-testkit`, never in the service, which stays network-free (`crates/iamb-service/tests/no_network.rs`) |
| Which key is "last" | The probe is told the keys about to be typed; T0 is the key event after which the characters received since arming end with them. With the fixture's immediate-trigger abbreviations that is the trigger key. Keys after T0 (the expander's deletions and text) are ignored |
| Matching rule | The control's whole text equals the expected text exactly; `\r\n` and `\r` read as `\n` on both sides (Rich Edit keeps line breaks as `\r`). Changes before T0 never match |
| Timeout | No match within 2 s after T0: failed, counted as 2 s. A change stamped more than 2 s after T0 fails even if it matches. The last key not arriving within `--key-wait` (10 s): failed (`no-key`), counted as 2 s. The browser gets 1 s of delivery slack before the probe gives up waiting, never in the stamps |
| Correct expansion | After a match the probe keeps watching for `--settle` (300 ms) and records whether the text is still the expected one (`stable`); a later change (an overshoot) shows there |
| Control channel | JSON lines on the probe's standard input and output: `hello`, `wait_ready`, `reset`, `expect` (cell, typed keys, expected text), `wait` (→ sample), `report` (→ every sample and the `LatencyRecorder` samples, ascending), `quit` (`compat::probe::protocol`). Only the driver's commands carry text, from its fixture; no reply, result or message does |
| Typing | The compat typist: `SendKeys` through PowerShell on Windows, System Events through `osascript` on macOS; ordinary unmarked keys, so the listener treats them as typing. Each trial: reset (empty and focus the control), expect, type the abbreviation, wait, then `--gap` (300 ms) |
| Fixture | `compat::latency::library`: 1,000 fixed plain snippets, 400 short (10–30 characters), 400 medium (31–200), 200 long (201–500), ASCII words, a quarter of the medium and long ones with a line break; abbreviations `;laaa` to `;lbml`, immediate trigger, exact case. `--trials N` keeps the 40/40/20 split; `--seed` only orders the trials |
| Result | `target/bench-results/<run id>.jsonl` (`--out` or `IAMB_BENCH_RESULTS_DIR` elsewhere) in the format of §3: `e2e/plain/<n>` with `kpi = "e2e-plain-p95"`, `target_stat = "p95"`, `target_ns = 50000000`, plus one line per length class (`e2e/plain-short/400`, …, no KPI); a browser run writes `e2e/plain-browser/<n>` lines without the KPI. Sorted samples in `<run id>.samples/`; `<run id>.trials.jsonl` has one line per trial: cell, snippet number, class, length, pass, stable, focus, latency, failure (never text) |
| Self-test | `iamb-probe --self-test` types a short string into its own window with synthetic keys (`SendInput` on Windows; key events posted to its own queue on macOS, which need no permission and never reach Iamb) and prints `T1 − T0` |

Tests: the rules, the protocol, the loop and the browser server with a simulated page in
`iamb-testkit` (every system; `cargo test -p iamb-testkit`, and the binary end to end with
`--features probe --test probe_browser`); a whole run against a simulated machine down to the
result files (`compat::latency`); the fixture through the engine's dry run (`tests/compat.rs`).
The native windows are tested on their OS (`cargo test -p iamb-platform-windows --features
probe --test probe`, which needs a desktop and fails without one when `IAMB_REQUIRE_DESKTOP`
is set; `cargo test -p iamb-platform-macos --features probe --test probe`, a `harness = false`
test that says `SKIPPED` when its window cannot become key): open the probe, type a short
abbreviation-free string with synthetic keys, check T0, T1 and the match. CI runs all of them.

Not measured here: the typist's own speed (it runs before T0) and the probe's paint; the
probe's T1 is the control's change, not the first frame on screen (the form KPI of §2.1 has its
own paint stamp).

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
| 4.1 | The sync conflict scenarios and their check: structure, and each outcome against its own steps and answers (`spec/sync/scenarios.toml`, `iamb-testkit::sync_scenarios`) |
| 4.2–4.4 | Server policy tests (`server/tests/`, the guarantees G1 to G23 of `sync-design.md` §11.5, with the two-connection tests of G21), the restore procedure with its script (what took access away is made again from the function log, the epoch begins above the operator's floor, and what the authentication service does with tokens issued after the backup, `sync-design.md` §11.6), sync convergence tests (the scenario replay and the 1,000-run simulation with the invariants I1 to I7 and the twelve places of a change of `sync-design.md` §1, a restore of the server among its events, with sessions that the restore ends), two-device latency |

## 8. UI host (task 3.2)

### 8.1 Cold open of the library window

**What is measured.** The library window is created on demand and destroyed when closed
(architecture §10), so every open is cold. The clock starts when the tray's "Open library"
action is handled, before the window is built, and stops at the window's first paint. The front
end reads the paint time in a task queued from a `requestAnimationFrame` callback (so after the
frame was presented) as `performance.timeOrigin + performance.now()` and reports it with the
`report_first_paint` command, together with when the page's navigation and its script began
(`apps/desktop/src/paint.ts`, `apps/desktop/src-tauri/src/timing.rs`). The result is the paint
time minus the wall-clock time of the action; the monotonic time until the report arrived is
printed beside it as an upper bound. Every open is also logged in `logs/desktop.log`.

**How.** `iamb-desktop --measure-open RUNS [--budget-ms MS]` waits 3 s for start-up to settle
(the pre-warmed form and palette load), then performs the tray action `RUNS` times, waiting for
each first paint and destroying the window in between, prints one line per open and a summary,
and exits with status 1 when an open was not painted or the slowest open exceeded the budget.
The binary must carry the front end (`pnpm build`, then a build with
`--features tauri/custom-protocol`); see `apps/desktop/README.md`. On Linux without a display:

    xvfb-run -a iamb-desktop --data-dir "$(mktemp -d)" --measure-open 20 --budget-ms 400

**Result here (2026-11-19).** Linux (Ubuntu 24.04.5, WebKitGTK 2.52.6, 4 vCPU Xeon 2.1 GHz
container, Xvfb with software rendering), release build, 5 processes of 10 opens each: first
open 59–101 ms, median of the later opens 59–81 ms, slowest of all 50 opens 139 ms; within the
400 ms budget. A debug build with the front end embedded gave the same (first 74–139 ms,
slowest 139 ms).

Where the time goes, and why the library window shares a web process: without that, an open
took 650–830 ms, of which about 600 ms passed before the page's navigation began. That is the
web view's engine starting a content process for the new view; loading the page, running its
script and painting took about 65 ms. On Linux the library window (and the form) are therefore
created related to the pre-warmed palette's web view and share its running content process
(`src/app/windows.rs`). Windows (WebView2) and macOS (`WKWebView`) start content processes
their own way; their numbers come from the `desktop` CI job on their runners (report only) and,
for the KPI, from the lab, and decide whether they need an equivalent.

**A fresh machine's first paint (CI).** On a machine where no window has painted before, Mesa's
software renderer (llvmpipe, under Xvfb) compiles its shaders during the first paint and caches
them in `~/.cache/mesa_shader_cache`. Measured here with the app on one busy core (2026-11-20):
the first open took 1,700–2,800 ms with an empty cache directory and about 540 ms with a warm one;
with only the shader cache kept it took about 610 ms, with only GStreamer's registry kept about
1,500 ms, so the shaders are the cost. Every CI runner is such a fresh machine, and the Linux gate
failed on it twice with a first open of 1.0–1.9 s while every later open took about 60 ms. The
`desktop` job therefore launches the measurement once, untimed, before the gated run, so the gate
measures opening the library and not the virtual display's one-time shader compilation. The
Windows and macOS numbers (report only) and the lab's are taken on machines with a GPU driver and
its own caches.

### 8.2 Idle memory of the UI host

ADR-0017 leaves the UI host's budget to this task. Measured here (Linux, Xvfb, release build,
idle with the hidden panels, 8 s after start): 246 MB proportional set size (PSS) over the UI
host process and its two web view processes (network and one shared content process), 398 MB
resident; with a content process per panel it was 290 MB PSS. Software rendering inflates both.
Provisional budget: 300 MB PSS over all of the UI host's processes, idle, to be confirmed or
reset with the Windows and macOS lab numbers (private working set, `phys_footprint`).

### 8.3 Message contract, bridge and crash isolation

- `tests/contract.rs`: the bridge's hello and requests against `iamb-ipc`'s golden files, and
  each golden response and event the bridge reads mapped to what the front end is promised;
  the `ui` role may call every request and hears every event. A protocol change breaks it.
- `tests/front_end_contract.rs` and `src/ipc.test.ts`: the command and event names agree
  between Rust and TypeScript, and the payloads Rust sends (written to
  `src/contract.fixtures.json`, `IAMB_BLESS=1` to rewrite) pass the front end's guards.
- `tests/bridge.rs`: the bridge against a real worker on the mock platform: status, pause,
  snippets, forwarded events, reconnecting when the service restarts, and no form value in any
  log.
- `tests/crash_isolation.rs`: a UI host whose connection ends abruptly mid-request with a form
  pending (every OS), and a UI host process killed with `SIGKILL` in the same state (Linux, the
  OS endpoint): the form is abandoned after the service's 2 s acknowledgement timeout, plain
  expansion continues, and the worker logs the connection and its end and nothing else (no
  warning, no error, no crash report). One case is ignored until the service changes: a UI host
  that dies after acknowledging a form leaves the form session open, and the engine drops every
  typed match while one is open, so expansion stays off until the worker restarts
  (`forms_and_expansion_recover_after_the_ui_host_died_with_a_form_open`).

### 8.4 Library window (task 3.3)

What runs where:

- **Component tests** (`pnpm test`, vitest with jsdom and Testing Library; files `src/library/*.test.ts(x)`).
  `testkit.tsx` is a fake service with the rules the interface relies on (abbreviations unique
  without regard to case, Undo by writing a deleted id again, tag names unique, four folder
  levels, `LibraryChanged` naming its rows). They cover the virtualised list (only a screenful
  of rows exists for 10,000 snippets; the window moves on scroll and renders again only when the
  first row changes), selection, keyboard movement and type-to-jump, the filter and its empty
  states, the editor (create, edit, explicit save, the leave dialog's three buttons, the On
  switch, options, tags), the abbreviation's messages with the specification's exact copy
  (duplicate blocks saving, shadowing warns), changes made elsewhere, folders (create, nest to
  the limit, rename, delete with its question) and tags (eight colours, unique names, delete),
  and that the stylesheet uses only tokens that exist.
- **Front-end and Rust contract**: `tests/library_contract.rs` runs the library's requests and
  response mapping against `iamb-ipc`'s golden files and writes
  `src/library/library.fixtures.json` (`IAMB_BLESS=1`), which `src/library/api.test.ts` runs
  through the guards; `tests/front_end_contract.rs` compares the command lists as sets.
- **End to end** (`tests/library.rs`): the library's operations through the bridge against a real
  worker on the mock platform: create (a draft does not expand, a saved snippet does), edit
  (the old abbreviation stops, tags are replaced), turn off and on, delete and Undo with the
  deleted snippet's id, `LibraryChanged` with its changed rows, the abbreviation check
  (conflict, hides, hidden by), folders to four levels, delete with cascade, tags (eight
  colours, unique names, delete), and no snippet text in either log.

#### Scroll measurement (KPI: 60 frames per second with 10,000 snippets)

Method. `node scripts/scroll-kpi.mjs` serves `dist/` (`pnpm build`) and opens it in Chromium with
`window.__TAURI_INTERNALS__.invoke` replaced by `scripts/tauri-mock.mjs`, which answers the
`library_*` commands from a generated library of 10,000 snippets (8 tags, nested folders, some
off, some broken). The real bundle, the real `api.ts` and the real list therefore run; only the
service is missing. A `requestAnimationFrame` loop records the gap between callbacks (the frame
time) for 10 s per driver; a frame over 25 ms is a dropped frame. Drivers: real wheel events
(120 px 60 times a second, 150 rows a second, scrolled by the compositor); `scrollTop` moved
from every frame at 6,000, 24,000 and 96,000 px/s (125, 500 and 2,000 rows a second; each frame
costs the main thread a scroll event, a state update and the render of the rows that came into
view); and the second driver at 24,000 px/s with the CPU slowed 4 and 6 times through the
DevTools protocol (headroom). Playwright is not a dependency of the app: `NODE_PATH` or
`PLAYWRIGHT_MODULE` points at an install.

Result (Chromium 153, headless, software rendering, 4 vCPUs of an Intel Xeon at 2.1 GHz,
1280 × 800, 28 rows in the page out of 10,000):

| Driver | Frames | fps | p50 | p95 | p99 | max | Dropped (> 25 ms) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Wheel, 7,200 px/s | 598 | 60.0 | 16.7 ms | 16.8 ms | 16.8 ms | 16.8 ms | 0 |
| `scrollTop`, 6,000 px/s | 599 | 60.0 | 16.7 ms | 16.8 ms | 16.8 ms | 16.8 ms | 0 |
| `scrollTop`, 24,000 px/s | 599 | 60.0 | 16.7 ms | 16.8 ms | 16.8 ms | 16.8 ms | 0 |
| `scrollTop`, 96,000 px/s | 598 | 59.9 | 16.7 ms | 16.8 ms | 16.8 ms | 33.4 ms | 1 of 598 |
| 24,000 px/s, CPU × 4 | 551 | 55.3 | 16.7 ms | 33.3 ms | 33.4 ms | 33.4 ms | 47 of 551 (8.5%) |
| 24,000 px/s, CPU × 6 | 425 | 42.7 | 16.7 ms | 33.4 ms | 50.0 ms | 50.1 ms | 161 of 425 |

The target holds at the machine's own speed (p95 16.8 ms, the 60 Hz frame, no dropped frame at
500 rows a second) and degrades only when the CPU is slowed 4 times or more. Caveats: the
headless browser's frame clock is pinned to 60 Hz, so a result at 16.7 ms says the work fits in
a frame, not by how much (the slowed runs are the measure of that); the web views of the
product (WebView2, WKWebView) are not Chromium under Playwright, so the lab repeats the run on
each OS's web view before the KPI is closed. The filter is not measured here: over 10,000 rows it
takes under 16 ms on the main thread (`model.test.ts`, asserted), so the worker the
specification asks for above 2,000 rows is not used.

### 8.5 Fill-in form (task 3.5)

What runs where:

- **Component tests** (`pnpm test`; `src/form/*.test.ts(x)`): every field type drawn with its
  control (one-line, several-line, a choice of up to six options as radio buttons and of seven
  or more as a select, a toggle), labelled, with autocomplete and spell checking off; focus in
  the first field with its content selected; Enter inserts from every field but a several-line
  one, ⌘⏎ / Ctrl+Enter from anywhere, Esc cancels, Space flips a toggle, first letters jump in
  a list; the buttons in each system's order; remembered answers pre-filled with "Your last
  answers. Start empty", and ⌘⌫ / Ctrl+Backspace or the link starting empty (and leaving the key
  to the text field when nothing was remembered); values corrected to fit their field as the
  template language does (nothing is validated or required); the "could not get back" banner
  with every answer kept; the answers forgotten when the form closes; the stylesheet uses only
  tokens and never animates.
- **Front-end and Rust contract**: the `form_*` commands in the three lists
  (`tests/front_end_contract.rs`), and the form `form_current` answers, its events and the
  `focus_lost` error, built from the golden `FormRequested` and written to
  `src/contract.fixtures.json`, through the guards of `src/form/api.ts` (`api.test.ts`).
- **End to end** (`tests/form.rs`): the real bridge and the form's flow against a real worker on
  the mock platform, with a recorded panel in place of the window: requested → acknowledged →
  shown → submitted, and the text lands in the mock's target window with the abbreviation
  replaced; cancel changes nothing and frees the session at once; keys typed while the panel
  holds the keyboard never reach the engine, and do again once it gives the keyboard back; a
  panel that loses the keyboard (a click into the app behind) gives it back to the app and
  takes it again; the `PanelFocus` lease renewed past its length; a focus that cannot be given
  back sends nothing and brings the panel back; a request the UI host took after 2 s is
  cancelled and never shown; `ReopenLastForm` brings the last answers; the UI host killed with
  the form open (the service records `form_ui_host_gone`, expansion resumes, the next UI host
  gets the next form); the service going away closes the form; no form value in either log.

#### Visible in under 150 ms

Method. The clock starts when `FormRequested` reaches the UI host (the bridge's reader thread
stamps it, monotonic and wall clock) and stops at the panel's first paint after it was shown,
which the front end reports with `form_painted` the way the cold open reports its paint (§8.1:
`performance.timeOrigin + performance.now()` in a task queued from a `requestAnimationFrame`
callback). In between: `FormAck`, `GetSnippet` for the header, the form handed to the hidden
panel, which renders it and reports its height (waited for up to 80 ms), `PanelFocus` and its
answer, then the window placed and shown. `iamb-desktop --measure-form RUNS [--budget-ms MS]`
waits for start-up and for the hidden panel's one paint (the panel is shown out of sight once
when its page reports it loaded, `form_ready`; without it the first form took about 400 ms
under Xvfb), then hands the flow `RUNS` requests for a form of six fields, one of each kind and
a choice of each size, every other one with remembered values, and cancels each. Its service
answers at once; the round trips to a real worker are measured by
`a_form_is_acknowledged_shown_and_its_text_lands_in_the_target_window` in `tests/form.rs`,
which prints them.

    xvfb-run -a iamb-desktop --data-dir "$(mktemp -d)" --measure-form 20 --budget-ms 150

Result here (2026-11-20; the machine of §8.1, Xvfb with software rendering, a debug build
loading the built front end from `vite preview` on the development URL, while other builds ran
on the machine): 5 processes of 10 forms each, first form after start 98–116 ms (shown after
37–52 ms), median of the later forms 16–21 ms, every later form under 40 ms; within 150 ms.
Before the hidden panel's paint waited for its page, one process under heavy load took 600 ms
for its first form. The round trips to a real worker on the mock platform took 0.6–1.1 ms
(`FormAck`, `GetSnippet`, `PanelFocus`).

What this does not measure: the trigger to `FormRequested` in the service (the engine share of
§2.1, measured with the probe in the lab), and WebView2 and `WKWebView`, whose numbers come
from the `desktop` CI job on their runners (report only) and, for the KPI, from the lab. On
macOS the panel is not yet the non-activating panel (`apps/desktop/README.md`), which may change
its show time.

#### Focus returns correctly across the matrix

Needs real machines: the steps are in `docs/compatibility-run.md`, "The fill-in form panel"
(F1 to F10 in each of the 20 apps of each OS). On the mock, `tests/form.rs` checks that the
hand-back (`restore_focus`) comes before `FormSubmit` and that a refused hand-back sends nothing.

### 8.6 Onboarding (task 3.7)

Tests: "Walkthroughs on fresh machines". KPI: "Install to first expansion under 2 minutes for 3 of
3 testers".

**What runs here.**

- `src/onboarding/*.test.ts(x)` (component tests over a fake service): every step and state of
  flow 1, on both systems. Welcome (the service not running, a restart this build cannot do);
  Permissions (nothing granted, one, both, the keyboard moving to Continue, the restart banner when
  both grants are there and the listener is still off, "Skip for now", the permissions step alone
  with "Done" for "Fix permissions"); First snippet (the three errors, the saved snippet's trigger,
  case and word start, skipped when the library has a snippet); Try it (the test field declared on
  entering and withdrawn on leaving and when the window closes, the expansion recognised, Enter
  and Tab, the switch, and each cause of "nothing happens" in its order); Done (the end, the login
  item, the shortcut the system refused, the library opening on the new snippet, Esc). One test
  fails when any command receives what was typed in the test field.
- `tests/onboarding.rs` (a real worker on the mock platform, through the bridge): permissions
  missing then granted; the settings page opening; typing the starter abbreviation into the
  declared test field expands it, and does not before it is declared or after it is cleared; a
  window of another process cannot be declared; another window of Iamb stays unexpanded; finishing
  sets `ui.onboarding_done`, stores the default shortcut or reports the refusal, and the window
  does not open at the next start (the worker restarted over the same data root); the login item;
  no typed text in any log.
- `tests/onboarding_contract.rs`: the requests against `iamb-ipc`'s golden files and the payloads
  against `src/onboarding/onboarding.fixtures.json` (`IAMB_BLESS=1` to rewrite).

**Scripted timing** (`scripted_first_run_timing`, on the mock platform): the steps automated
through the same functions the window's commands call, from the moment the window opens to the first
expansion in the test field, and on to the end. Result here (2026-11-20, debug build, the machine
of §8.1; it prints its marks with `--nocapture`):

| Mark | Time from the window opening |
| --- | --- |
| permissions read, both granted (macOS path) | 0.5 to 0.7 ms |
| first snippet saved | 3 to 4 ms |
| test field declared | 3 to 4 ms |
| first expansion (abbreviation and a space typed) | 3.5 to 4.5 ms |
| onboarding done (field withdrawn, shortcut and `ui.onboarding_done` stored) | 6 to 8 ms |

The test asserts that the first expansion comes within 5 s. This says that the service's side of the
steps costs nothing next to the 2 minutes. It does not time the window, the web view or a person.

**What needs real machines and testers.** The KPI itself, three people on each OS from a clean
machine: `docs/onboarding-walkthrough.md` (what to install, the two clocks and their marks, what
counts as a pass, the timing sheet). Also on real machines: the permission rows on macOS
(`GetPermissions` against the real grants, the helper listed as "Iamb" with its icon, the grants
taking effect without a restart or with the restart banner), `SetTestField` with the real focus
inspector (the window's token on each OS), the login item, and the window and tray menu entries on
a real desktop (the window itself was checked under Xvfb on Linux with the mock service: it opens
at start while `ui.onboarding_done` is not set).

### 8.7 Search palette (task 3.6)

What runs where:

- **Relevance fixtures** (`spec/search/relevance.toml`, run by `crates/iamb-core/tests/search_relevance.rs`):
  a workshop's 14 snippets and 28 queries with the results that must come first, in order, and the
  field and the marked text of the first hit. They cover an abbreviation typed whole, as a prefix, in
  the middle and as letters in order; a label word and several words in any order; content only the
  content has; a tag name; two letters swapped, a letter missing from a tag, a letter too many in a
  label; a query too short to forgive anything; "warranty", which the palette's own example says
  matches nothing; white space; letters beyond ASCII; and ties, which keep the library's order. The
  rules are in `crates/iamb-core/src/search.rs` (module docs). Unit tests there cover the tiers, the
  typo distance, UTF-16 ranges for surrogate pairs and for letters that fold to two, a body that is
  not ASCII, and the cut of a long query.
- **The service** (`crates/iamb-service/tests/editor.rs`, `end_to_end.rs`): `Search` answers with the
  field and the ranges, unchanged in shape.
- **Component tests** (`pnpm test`; `src/palette/*.test.ts(x)`): the dialog with its combobox and
  listbox and the hint; the query box focused and the paint reported on `shown`; search on every
  change with the count ("4 of 76"); an answer to an older query dropped; matches marked with
  `<mark>`; the arrows wrap, Page Up and Page Down jump; Enter on each kind of row (insert, fill in
  and insert, review, open, select in the library, open the form, create), with the footer saying
  which; Ctrl+1 to Ctrl+9 (⌘ on a Mac) and Ctrl+E (⌘E); Esc; the empty state with "Fill in “…”
  again"; "No snippet matches “…”." with "New snippet labelled “…”"; "Iamb could not get back to …";
  the dimmed list after 300 ms and "Iamb is not running." with "Start it again" after 2 s; the query
  gone when the palette closes and in no storage; the count announced; the stylesheet uses only
  tokens and never animates. `src/library/usePaletteRequests.test.tsx`: what the library does with
  the requests a pick leaves for it.
- **End to end** (`apps/desktop/src-tauri/tests/palette.rs`): the real bridge and the palette's flow
  (and the form's) against a real worker on the mock platform, with recorded panels in place of the
  windows: the shortcut registered through `PutSetting`, pressed on the mock, the palette shown,
  searched, and a pick types the text in the target window (nothing deleted); Esc changes nothing
  and the keyboard is the app's again; keys typed while the palette holds the keyboard never reach
  the engine; a pick for a snippet with blanks hides the palette and shows the form, which then
  holds the keyboard and submits (and the shortcut does nothing while the form is open); the
  shortcut again closes it; a focus that cannot be given back sends nothing and brings the panel
  back; a panel that loses the keyboard gives it back to the app and takes it again; the lease
  renewed past its length; the tray opens it for the application in front; opened from Iamb's own
  window a pick goes to the library and types nothing; "Fill in … again" after a submitted form;
  the service going away and the UI host killed with the palette open; a typo, an abbreviation
  first and a no-match through the rows; the list of snippets read again after `LibraryChanged`;
  the preview; the default shortcut offered once; no query in any log.
- **Contract** (`tests/front_end_contract.rs`): the `palette_*` commands and `library_take_request`
  in the three lists.

#### Results in under 30 ms at 10,000 snippets

Method. `cargo bench -p iamb-bench --bench search -- --quick` times 500 queries over the
10,000-snippet fixture (each snippet with a label of its first three words and up to two of forty
tags), `limit` 50, one at a time, p95 (`LatencyRecorder`); `IAMB_SEARCH_BUDGET_MS` makes it fail
over a budget (CI: 60). `cargo test --release -p iamb-service --test search_timing -- --nocapture`
imports 10,000 snippets into a worker and times 400 `Search` requests over IPC. The first version
of the search (substring matching, folding each body on every query) was not timed; the one here
does not allocate per snippet, searches an ASCII body in place and looks at the lower tiers of a
snippet only when no better tier matched.

Result here (2026-11-20, the machine of §8.1, a release build, while other builds ran):

| What | p50 | p95 | p99 | max |
| --- | --- | --- | --- | --- |
| `search/query/10000`, the ranking alone, 500 queries | 6.6 ms | 11.1 ms | 13.2 ms | 14.7 ms |
| `Search` over IPC through the worker, 400 queries | 5.3 ms | 6.9 ms | | 10.9 ms |

The slowest queries are the ones that match nothing (every tier and the typo tier for every
snippet); the floor of 3.2 ms is the scan of the bodies. In a debug build, through the bridge and
with the rows built from the list the UI host holds (`search_through_the_bridge_at_10000_snippets`,
100 queries): p50 93 ms, p95 107 ms, which is about ten times a release build and is only held to
600 ms. The rows are drawn from the list in memory; the list is read in pages of 1,000 the first
time the palette opens (`palette_current`, in the background) and again after a `LibraryChanged`;
a query that arrives before it is read waits for it (10,000 snippets: about 1 s in a debug build
through the mock).

What this does not measure: the key to the screen (the web view's paint of the rows, which the lab
measures with the palette's front end on WebView2 and `WKWebView`), and the preview's round trips
(`GetSnippet` and `RenderPreview`, 50 ms after the selection stops moving).

#### Visible in under 100 ms

Method. The clock starts when `PaletteRequested` reaches the UI host (the bridge's reader thread
stamps it, monotonic and wall clock) and stops at the panel's first paint after it was shown,
which the front end reports with `palette_painted` the way the form's is (§8.5). In between:
the palette handed to the hidden panel (which clears its query and draws the empty state),
`PanelFocus` and its answer, then the window placed and shown. `iamb-desktop --measure-palette RUNS
[--budget-ms MS]` waits for start-up and for the hidden panel's one paint (shown out of sight once
when its page reports it loaded, `palette_ready`), then hands the flow `RUNS` requests and closes
each. Its service answers at once; the round trip to a real worker (`PanelFocus`) is measured by
`the_shortcut_opens_the_palette_and_a_pick_types_where_the_cursor_was` in `tests/palette.rs`,
which prints it.

    xvfb-run -a iamb-desktop --data-dir "$(mktemp -d)" --measure-palette 20 --budget-ms 100

Result here (2026-11-20; the machine of §8.1, Xvfb with software rendering, a debug build with
the built front end embedded, `--features tauri/custom-protocol`): 3 processes of 10 palettes each,
first palette after start 37 to 52 ms (shown after 1 to 2 ms), median of the later ones 23 to 24
ms, every one within 52 ms; within 100 ms. The round trip to a real worker on the mock platform
took under 2 ms (`PanelFocus`).

What this does not measure: the shortcut to `PaletteRequested` in the service (the hotkey thread, the
focus token), and WebView2 and `WKWebView`, whose numbers come from the `desktop` CI job on their
runners (report only) and, for the KPI, from the lab. On macOS the panel is not yet the
non-activating panel (`apps/desktop/README.md`).

#### Focus returns correctly, and the keys stay out of the engine

Needs real machines: the steps are in `docs/compatibility-run.md`, "The search palette" (P1 to P16
in each of the 20 apps of each OS). On the mock, `tests/palette.rs` checks that the hand-back
(`restore_focus`) comes before `ExpandSnippet`, that a refused hand-back sends nothing, and that
what is typed while the palette holds the keyboard reaches neither the engine nor the hand-off.

### 8.8 Rich text snippets (task 3.8)

KPI: at least 90% of paste-fidelity cases pass; plain-text fallback everywhere. The matrix
(Gmail, Outlook, Word, Google Docs, Slack, Notes on the systems each runs on), the nine fixture
snippets, what counts as a pass and the 90% rule are in [`docs/paste-fidelity.md`](paste-fidelity.md);
running it needs real machines and real applications.

Method, without them:

- **Rendering and sanitising** (`iamb-core::rich`): the examples of `spec/rich/examples.toml`
  (Markdown to the clipboard's HTML and plain text, hostile input included) in
  `tests/rich_spec.rs`; property tests in `tests/rich_props.rs` (2,000 generated documents of
  Markdown and HTML fragments: every tag and attribute allowed, links only `http`, `https` and
  `mailto`, images only attachments, balanced, nesting within 32; escaped text renders as itself;
  pathological inputs of 20,000 to 40,000 characters each render in under 2 s in a debug build).
- **Templates in Markdown**: the typing sessions `072` and `119`–`121` of `iamb-testkit` (macro
  output escaped, `{cursor}` ignored, keys splitting the pastes, plain and rich snippets inside each
  other) and `rich_props.rs`.
- **Attachments** (`iamb-store`): `tests/attachments.rs` (content addressing, private files, types
  read from the bytes, size limits, damaged bytes read as missing, the snippet links, a JSON export
  and import carrying the bytes, an import with a wrong hash writing nothing).
- **The clipboard**: `iamb-service/tests/rich.rs` on the mock platform (a formatted snippet pastes
  HTML with its image embedded and the user's clipboard comes back without a history entry; a
  missing image and an application set to type key by key give plain text; the editor's
  `PutAttachment`, formatted `RenderPreview` and the links written on save);
  `tests/paste_fidelity.rs` (the fixture file current and importable, the clipboard content of each
  fixture current in `docs/paste-fidelity/clipboard.json`); the `HTML Format` offsets
  (`iamb-platform-windows`, `clipboard/html.rs`) and the macOS pasteboard item
  (`iamb-platform-macos`, `clipboard/layout.rs`), pure code that runs on every OS.
- **A browser's paste**: `docs/paste-fidelity/browser-paste.mjs` puts each fixture's clipboard
  content on headless Google Chrome's clipboard and pastes it with the shortcut into a rich-text
  editor and a plain text field (CI: `desktop` job on Linux). Result here (2026-11-20): 9 of 9
  fixtures pass, formatted and plain.
- **The editor**: `src/library/EditorFormatted.test.tsx` (the kind control, saving the kind, the
  question before formatting would be lost), `src/library/template/Formatted.test.tsx` (the
  formatting toolbar and its shortcuts, images from the picker, a paste and a drop, refusals, the
  formatted preview and its links that do not navigate), `src/library/template/markdown.test.ts`;
  in Rust, `src-tauri/tests/template_commands.rs` (the formatted preview with the image embedded
  from its file, `template_add_attachment` through the bridge, no temporary file left).

What this does not cover: what each application's own paste filter keeps (the matrix), the
system clipboards themselves (the platform crates' paste tests and the compatibility run), and
pasting large images into slow applications (the paste wait of each app's strategy).

### 8.9 Library health and usage statistics (task 3.10)

**KPI: library health detects 100% of planted problems.** Method: a library is written with the
problems put in on purpose, with the list of exactly what must be reported and as what, and the
test fails on a miss and on a false alarm.

- **Fixture libraries** (`crates/iamb-store/tests/fixtures/health/*.json`): one file per kind of
  problem (`missing_reference`, `cycle`, `depth`, `template_error` with a snippet that includes a
  broken one, `duplicate_abbreviation`, `shadowed`, `unused`, `unused_after_reset`) and
  `mixed.json`, a library of 284 snippets as a business might keep one, with 27 planted problems
  of every kind and decoys that must not be reported (a switched-off snippet with a broken
  reference, a long-unused switched-off one, a reference to a switched-off snippet, one used 89
  days ago, one made 20 days ago, a broken snippet that is also unused, reported once). Each file
  has `now_day`, its snippets (abbreviation, content, trigger, on or off, creation day, last use)
  and `expect`: one line per planted problem (`error <key> <CODE> ref=… in=…`,
  `taken <key> kept=<key>`, `never_reached <key> by=<key>`, `unused <key> last_day=<n>|never`).
- **The test** (`crates/iamb-store/tests/health.rs`) builds the engine's snapshot from each file,
  runs `iamb_store::health::analyse`, writes what it reports in the same lines and compares the
  two sets. It prints the detection rate of every file and the total (`cargo test -p iamb-store
  --test health -- --nocapture`). Result here (2026-11-20): 46 of 46 planted problems detected
  (100.0%), 0 false alarms; the sidebar count equals the snippets reported once.
- **Timing**: `health_over_ten_thousand_snippets_is_fast` makes 10,000 snippets with about a
  tenth in trouble (486 will not expand, 102 never reached, 986 unused) and times `analyse`:
  about 40 ms in a debug build on the development machine (the bound is 1 s). The snapshot's
  build, which happens once per library change already, takes about 160 ms in the same build;
  the `Health` request adds the read of the usage rows (10,000 small rows) on the store thread.
- **Usage**: `iamb-store/tests/crud.rs` (usage touches neither the snippet nor the outbox; a
  reset forgets every count, notes when and changes no snippet or outbox row; the most recently
  used come newest first and skip deleted snippets), `iamb-service/tests/usage_health.rs` (an
  expansion counts once; the list answer carries `uses` and `last_used`; `RecentSnippets` is
  newest first with no matched field; `ResetUsage` clears the counts, is refused to the
  command-line tool and leaves the outbox as an unused library has it; health through the
  endpoint names the missing snippet, the cycle, the shadowing and the unused snippets, and a
  reset starts the 90 days again), `iamb-ipc/tests/golden` (the new messages and fields).
- **The window** (Vitest): `src/library/health/Health.test.tsx` (the sections with the
  specification's titles and none that is empty, each entry's line and buttons, opening, turning
  off and deleting from an entry, the badge counting snippets once, the clean state, following a
  change while open, the failure and its retry, the sentences), the editor footer's usage line
  (`Editor.test.tsx`), "Most used on this computer" (`model.test.ts`), the recent list of the
  palette (`palette/PaletteView.test.tsx`: newest first, eight rows with "Fill in … again", the hint
  when there is nothing) and the reset in Settings (`settings/SettingsView.test.tsx`: it asks first,
  forgets only when confirmed, says when the service does not answer).

Not covered: usage recorded on a real platform (the mock's end-to-end test in
`iamb-service/tests/end_to_end.rs` records through the same store thread), and the reading of
"never used" for libraries made before the counts were reset by a build that did not record the
reset (they count from creation).

### 8.10 Accessibility and polish (task 3.11)

KPI: 0 critical accessibility issues, against WCAG 2.1 AA (`docs/ux/spec.md` §8). Three layers.

- **axe-core in the component tests** (`apps/desktop/src/a11y.test.tsx`, helper `src/a11y.ts`).
  `axe-core` 4.13.0 (MPL-2.0; a dev dependency, 2026-08-05) was chosen over `vitest-axe` and
  `jest-axe` because those are thin wrappers that pin their own copy of it; calling `axe.run`
  directly needs no matcher plumbing. It runs over 38 states: the library (list, no filter match,
  empty, service not running, editor plain with messages and options, formatted, preview, the
  unsaved-changes dialog, the new folder and new tag dialogs, a menu, the toast), Library health
  (clean, broken), the seven Settings sections and the service-down state, "Why didn't it expand?"
  (entries, empty), the form (every field type, remembered answers, the "could not get back"
  banner), the palette (nothing typed, recent, results, no match) and every onboarding step. The
  bar in jsdom is zero violations of any impact. Rules that need layout or style sheets
  (`color-contrast`, `target-size`, `scrollable-region-focusable`, the page-level `region`,
  `landmark-one-main`, `page-has-heading-one`) cannot answer in jsdom and run in layer 3. A
  check of 2.5.3 Label in name (`scripts/label-in-name.mjs`: a control with an `aria-label` and
  visible text must be named starting with that text) runs in both layers, because axe's own
  rule for it is experimental and stricter than the commitment. A canary test checks that the
  harness fails on a bad page.
  Counts before the fixes (jsdom): critical 1 in each of the 6 states that hold the template
  editor (`aria-expanded` on a `role="textbox"`), serious 1 in each of the 4 palette states
  (`aria-label` on a plain `div`), minor 1 in each of the 3 form states (`role="dialog"` on a
  `form`); the other 28 states 0. After: 0 in every impact in all 38 states.
- **Style sheets** (`src/styles/a11y.test.ts`): no colour value outside `tokens.css` (hex,
  functions, named colours; the test is shown to see one), dark by the system and dark chosen in
  Settings carry the same values, `base.css` zeroes every duration under
  `prefers-reduced-motion`, only transforms and opacity are transitioned and with the duration
  tokens, every window's sheet has a `forced-colors` block, and no rule removes the focus ring
  without another one around it.
- **The keyboard-only run** (`apps/desktop/scripts/keyboard-run.mjs`, mock
  `keyboard-mock.mjs`; `docs/accessibility-run.md`): the built front end in Chromium, keyboard
  only, in four settings (light, dark, reduced motion, forced colours). After each of 45 steps
  per setting it checks that something has the focus, that it is on screen and has a ring (2.4.7),
  runs axe-core with contrast and Label in name, and fails the step on critical or serious.
  Result here (2026-11-20): 4 settings x 45 steps, critical 0, serious 0, moderate 4 (the same
  one in each setting: `region`, a context menu is a popup outside the landmarks; a landmark
  around every popup would lengthen a screen reader's landmark list for no gain), minor 0.
  First run, before the fixes: critical 5 (the collapsed sidebar's buttons had no names at 640
  px), serious 0 once the experimental rule was left out, moderate 49 steps (no `main`, no `h1`,
  a skipped heading level, the menu), and these failures: no ring on the palette's query field,
  focus lost to the page after Ctrl+, from the editor and after Undo, Undo unreachable in time,
  the window 880 px wide at 200% zoom.
  Run it with `PLAYWRIGHT_MODULE=<playwright> IAMB_CHROME_PATH=<chrome> node scripts/keyboard-run.mjs`
  after `pnpm build`; CI runs it in the desktop job on Linux.
- **Not covered here**: real screen readers (NVDA, Narrator, VoiceOver), the focus handed to and
  taken back from the non-activating panels, Increase contrast on macOS, real 200% zoom and the
  zoom keys, and the native title bar; `docs/accessibility-run.md` lists the steps for a person.

## 9. Sync server (task 4.2)

The server's schema, row-level security and database functions (`server/`, built from
`docs/sync-design.md`) are tested by SQL tests that act as each kind of user of §4 of the design
(owner, admin, member with `edit`, member with `view`, former member, stranger, a revoked device, a
session that ended, a session opened with a password) and fail on any violation. `server/README.md`
has the layout, the table of which test covers which guarantee, and how to run them.

- **Where they run.** Locally and in CI (the `server` job, Linux) against a throwaway Postgres cluster
  with a stand-in for the platform's `auth`, `storage` and `realtime` schemas (`server/tests/run.sh`;
  the README lists what the stand-in cannot model), with the migrations applied twice. Against the
  hosted project through the Management API, each file inside a transaction that is rolled back
  (`server/tests/hosted.sh`), plus the authentication checks with throwaway accounts
  (`server/tests/hosted_auth_probe.py`).
- **What they cover.** The plan's three: an outsider reads nothing (`03_outsider`: every table, every
  reading function, the stored files, the live channel); a viewer cannot write (`04_viewer`: every op of
  §4.3 and every account call the role does not allow); a removed member loses access, including through
  the feed, the history, conflicts, attachments, the live channel and the rows he wrote (`05_removed_member`,
  also for an ended share and a deleted shared folder). Then every guarantee G1 to G23 of §11.5 that can
  be tested in the database, among them the two-connection tests of G21 (`20_concurrency`: each second
  request is seen waiting on the first's locks), the restore procedure against a real dump
  (`restore_test.sh`, G23 and the restore checks of §11.6), and a scan of the function log for row
  content (G20). After every step of every test, `folder_access` is compared with the slow reference of
  §5.4 (G1), the folder tree is checked (I7), and for every actor the rows the policies give are
  compared with the server's own rule for who may read a row.
- **Measured** (§11.6): a push of 200 ops and a folder delete that takes 2,000 rows, against push's own
  time-out of 20 s (`server/tests/perf.sql`).

Results (2026-11-21):

| KPI or check | Target | Result |
| --- | --- | --- |
| Row-level security on the tables of Iamb's schemas | 100% | 29 of 29, locally and on the hosted project; the exposed schemas `api` and `api_service` hold no table |
| Security advisor warnings | 0 | 24, all lint 0029 (a signed-in user can run a `security definer` function of `api`), which is the design's write path and which Supabase documents as intentional for per-user privileged operations; the one other warning (a mutable `search_path` on the retention job) was fixed. See `server/README.md`, "The hosted project" |
| Performance advisor warnings | 0 | 0 (4 findings at INFO level: indexes unused on a project without traffic) |
| Local tests | all pass | 21 of 21: 18 SQL files, the restore test, the measurement, the log scan; about 35 s |
| Hosted tests | all pass | 17 of 17 SQL files (all but the concurrency file, which needs a superuser), rolled back; the authentication checks as listed in the README |
| Push of 200 ops | < 20 s | 0.21-0.31 s locally, 0.44 s on the hosted project |
| Cascade of 2,000 rows | < 20 s | 1.4 s locally, 4.6 s on the hosted project |
