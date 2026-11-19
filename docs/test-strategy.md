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
- Leak rule 2: mean memory of the last hour minus mean of hour 1 is < 5 MB.
- Handle and thread counts return to their hour-1 range after each kill and stay flat.
- Expansion correctness during the run ≥ 99.9%.

### 5.3 Canary privacy scan (also the method for the "0 typed keys" KPI)

1. The driver generates canaries per run: `qz` + 14 random lowercase letters (never an
   abbreviation). It types them as ordinary text, inside password fields, in excluded apps, and
   while paused.
2. During the run: capture all network traffic of the machine (`tcpdump` on macOS, `pktmon` on
   Windows) and all IPC frames (the soak harness serves the service endpoint through a tapping
   endpoint wrapper, §5.5; on CI a dependency check replaces the capture).
3. After the run: search for every canary, in UTF-8, UTF-16LE and Base64 forms, in the data root,
   logs, crash dumps, OS temp directories, the whole user profile, the IPC tap and the packet
   capture.
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
| `report.json` | `iamb-soak-report/1`: options, totals (keys, rate, expansions, edits, CPU, max memory, restarts the supervisor logged), every recovery (control, `Health` and probe times), unexpected events, the first 50 expansion failures (snippet number, shape, case mode; never field text), per canary its context and checks (never its text), the network check, every rule's verdict |
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
  same to rule 2: it compares the last `W` with `[W, 2W]` (hour 2 in the lab), not with hour 1,
  which would charge the climb above to the 5 MB budget. Every worker life has its own warm-up
  `W`; samples inside it count for neither leak rule nor for the handle and thread ranges.
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
  engine's buffer when an expansion fires), `secure-field`, `excluded-app` and `paused`. In the
  last three an abbreviation is typed after the canary and must not expand, and `GetStatus` must
  report the engine suspended (and paused); in the first two it must not be. A canary counts as
  typed only when the field then holds exactly the expected text.
- **Scan.** Forms: UTF-8, UTF-16LE, and Base64 of both (standard and URL-safe alphabets, at each
  of the three alignments, only the characters that do not depend on neighbouring bytes).
  Locations: the data root (library, WAL, logs, locks), the temp directory the service processes
  used, the IPC tap, their standard error, the control directory, the samples file, and any
  `--scan` path. A file that cannot be read fails the rule (0 hits cannot be claimed for it).
  Scanning the driver's own input must find every canary (the rule `canary-scanner-control`), so
  a broken scanner cannot pass. Hits are reported by canary number, context, form, location, file
  and byte offset; no report, summary or progress line contains a canary or the bytes around it.
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
| Crash dumps, the whole user profile | Too large and too shared on CI | `--scan "$HOME"` (or `%USERPROFILE%`) plus the OS crash-dump directories |
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
| 4.2–4.4 | Server policy tests (`server/tests/`), sync convergence tests, two-device latency |
