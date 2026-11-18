# iamb-service

The background service (architecture §3, ADR-0001, ADR-0011). One binary, two processes:

- `iamb-service --supervise`: the login item. Holds `run/supervisor.lock` (single instance),
  starts the worker, restarts it on abnormal exit (backoff 0 s, 1 s, 5 s, then every 30 s; reset
  after a worker ran for 60 s) and kills and restarts it when its heartbeat (one byte on the
  worker's standard output every 500 ms) stops for 3 s. A new worker gets 10 s for its first
  heartbeat, because it may wait up to 5 s for `run/store.lock`.
- `iamb-service --worker` (the default): listener → engine → injector, the focus and store
  threads, the service endpoint (§3.1). Exits with status 0 when its standard input ends (the
  supervisor is gone) and with status 75 when the listener failed its self-test twice. On macOS
  the native worker's main thread runs the application event loop
  (`iamb_platform_macos::run_with_main_loop`; no Dock icon, no menu bar), which the listener's
  keyboard-layout reads and the hotkeys need; the worker's own threads run beside it. The
  supervisor and `--platform mock` do not use the loop.

```text
iamb-service --supervise [--data-dir PATH] [--platform native|mock]
iamb-service [--worker] [--data-dir PATH] [--platform native|mock] [--heartbeat-stdout]
```

`--platform mock` runs the worker against the in-memory mock platform (development and CI only:
no real keyboard; one fake window with an ordinary text field). It is the only choice on Linux,
which has no OS crate. The data root is `--data-dir`, else `IAMB_DATA_DIR`, else the per-OS
default. Logs: `logs/supervisor.log` and `logs/worker.log`, plain text, 5 × 1 MB.

## Running it

Against the mock (any OS):

```sh
cargo build -p iamb-service
export IAMB_DATA_DIR=/tmp/iamb-dev
target/debug/iamb-service --supervise --platform mock &
cargo run -p iamb-cli -- list                    # goes through the service endpoint
cargo run -p iamb-cli -- add brb "be right back"
tail -f /tmp/iamb-dev/logs/worker.log
```

On Windows (from a normal, non-elevated prompt):

```bat
cargo build --release -p iamb-service
set IAMB_DATA_DIR=%TEMP%\iamb-soak
target\release\iamb-service.exe --supervise
```

On macOS the grants (Accessibility, Input Monitoring) belong to the signed helper app
(ADR-0021). Until packaging (Phase 5), sign a development build with the stable development
identity, grant both permissions to that binary in System Settings, then run
`iamb-service --supervise` from a terminal. `GetPermissions` (or the worker log) says what is
missing.

Build release binaries with `-p iamb-service` alone so feature unification with other workspace
members cannot add code to them (ADR-0008).

## Measured on the build machine (Linux, mock platform, debug build)

| What | Result | Test |
| --- | --- | --- |
| Restart after `kill -9` of the worker (backoff 0 s) | about 11 ms until the new worker answers | `tests/supervisor.rs` |
| Restart after a second `kill -9` (backoff 1 s) | about 1.01 s | same |
| Hung worker replaced (no heartbeat for 3 s) | 3.0 s after its last heartbeat | same |
| Backoff between crashing starts | 3 ms, 1002 ms, 5004 ms, then 30 s | same |
| Idle wake-ups over 3 s | 6 heartbeats; every other thread 0 | `tests/idle.rs` |

## Operations note for real machines (later)

These need the real OS crates and real hardware; nothing here has run on them yet.

### Idle CPU and memory (target: CPU under 0.1%, memory under 20 MB)

1. Release build, a library of about 1,000 snippets imported (`iamb import`), the supervisor
   started from a fresh login, a UI host not running.
2. Wait 5 minutes, then sample for 30 minutes without touching the keyboard:
   - Windows: `typeperf "\Process(iamb-service*)\% Processor Time" "\Process(iamb-service*)\Private Bytes" -si 10 -sc 180`
     (two instances: supervisor and worker; report both and their sum).
   - macOS: `top -l 180 -s 10 -stats pid,command,cpu,mem -pid <worker> -pid <supervisor>`, and
     `footprint <pid>` at the end for the dirty memory.
3. Expected wake-ups while idle: the worker's engine thread every 500 ms (heartbeat), the
   watchdog every 30 s (self-test), the supervisor's heartbeat reader every 500 ms. Anything
   more is a bug: compare with `Worker::wake_counts` (the counters `tests/idle.rs` checks).
4. Record: OS version, CPU, library size, both processes' CPU % (mean, max) and memory
   (private bytes / footprint) at the start and the end.

### 72-hour soak

1. Release build; supervisor started at login (Run key on Windows, login item on macOS).
2. Drive typing with the compatibility harness (task 2.7) against a text editor for 10 minutes
   every hour (mixed abbreviations, typing ahead during expansions, focus changes, a password
   field), idle in between. Lock and unlock the screen a few times a day; sleep and wake the
   machine at least once.
3. Once a day: `kill -9` (Windows: `taskkill /F /PID`) the worker and confirm it answers again in
   under 2 s (`iamb list` succeeds; the supervisor log shows the restart).
4. Collect at the end: `logs/` (scan them for the harness's canary strings: none may appear),
   the number of worker restarts and their causes (supervisor log), memory at hours 1, 24, 48
   and 72 (no growth beyond noise), expansion success rate from the harness, and
   `iamb list` round-trip time.
5. Pass: no unexplained restart, no memory growth, no canary in any log, every forced restart
   under 2 s, expansion success rate at the target of the reliability run.
