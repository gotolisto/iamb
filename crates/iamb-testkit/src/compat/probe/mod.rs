//! The probe of the end-to-end latency measurement (`docs/test-strategy.md` §6): the
//! `iamb-probe` app (task 2.7, test-only, never shipped).
//!
//! The probe is a process of its own with one text control: a native window (a Win32 Rich Edit
//! control on Windows, an `NSTextView` on macOS) or a local web page with a `<textarea>`. The
//! driver (`iamb-compat latency`) tells it over standard input what it is about to type and what
//! the control must then hold, types the abbreviation as ordinary unmarked keys, and asks for the
//! sample. Both stamps are taken in the process that owns the control, on one clock:
//!
//! | Surface | T0 | T1 | Clock |
//! | --- | --- | --- | --- |
//! | Windows | The control's `WM_KEYDOWN` taken from the queue, for the key whose `WM_CHAR` completes the abbreviation | `EN_CHANGE` after which the text equals the expected result | `QueryPerformanceCounter` (`Instant`) |
//! | macOS | `keyDown:` of the key that completes the abbreviation | `didChangeText` after which the text equals the expected result | `CLOCK_UPTIME_RAW`, the clock of `mach_absolute_time` (`Instant`) |
//! | Browser | `keydown` of that key | `input` after which the value equals the expected result | the page's `performance.now()` |
//!
//! | Module | Does |
//! | --- | --- |
//! | [`session`] | The rules: T0, T1, the matching rule, the 2 s timeout, the samples (tested everywhere) |
//! | [`protocol`] | The JSON-lines control channel |
//! | [`serve`] | The shared probe state, the [`Surface`](serve::Surface) trait and the control loop |
//! | [`browser`] | The page and its loopback server |
//! | `app` (feature `probe`) | The `iamb-probe` program: arguments, the native window, the self-test |
//!
//! The native windows live in the two OS crates (`iamb_platform_windows::probe`,
//! `iamb_platform_macos::probe`) behind their `probe` feature, because only those crates may
//! hold `unsafe` code; this crate's `probe` feature turns them on. The probe never logs or writes
//! the text it holds: replies, results and error messages carry cell ids, latencies, verdicts and
//! lengths only.

pub mod browser;
pub mod protocol;
pub mod serve;
pub mod session;

#[cfg(feature = "probe")]
pub mod app;
