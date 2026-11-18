//! macOS implementations of the Iamb platform traits (architecture §5, ADR-0005, ADR-0021).
//!
//! On macOS this crate exposes [`Platform`], which implements every trait of
//! `iamb-platform`, and [`run_with_main_loop`], which the worker's `main` calls so that its
//! main thread runs the application event loop the listener and the hotkeys need. On every
//! other system the crate is empty, so the workspace builds everywhere and
//! `cargo clippy --target aarch64-apple-darwin` can check it from Linux (`docs/ci.md`).
//!
//! # Layout
//!
//! Each module implements its traits for [`Platform`] and keeps its own state in its own
//! `State` struct, so the tasks of Phase 2 do not edit the same lines.
//!
//! | Module | Trait | Task |
//! | --- | --- | --- |
//! | `listener` | `KeyListener` | 2.3 |
//! | `hotkeys` | `Hotkeys` | 2.3 |
//! | `permissions` | `Permissions` | 2.3 |
//! | `injector` | `Injector` | 2.4 |
//! | `clipboard` | `ClipboardAccess` | 2.4 |
//! | `focus` | `FocusInspector` | 2.5 |
//! | `login_item` | `LoginItem` | 2.6 |
//! | `memory` | `MemoryLock` | 2.6 |
//! | `endpoint` | `LocalEndpoint` | 2.6 |
//!
//! Until its task lands, a module answers [`Unsupported`](iamb_platform::PlatformError).
//!
//! One module implements no trait: `main_loop` ([`run_with_main_loop`]) gives the worker's
//! main thread to the application event loop, because the listener's keyboard-layout reads
//! and the hotkeys must happen there (`listener`, `hotkeys`).
//!
//! # Bindings
//!
//! The crate uses the `objc2` family (`objc2`, `objc2-foundation`, `objc2-app-kit`,
//! `objc2-core-foundation`, `objc2-core-graphics`, `objc2-application-services`,
//! `objc2-service-management`, `objc2-security`) and `libc`, not the older
//! `core-foundation` and `core-graphics` crates:
//!
//! - one family covers every framework this crate calls, including the Accessibility API,
//!   the pasteboard, the login-item service and code-signature checks, which the older
//!   crates do not bind;
//! - the bindings are generated from the system headers, one Cargo feature per header, so
//!   only what is listed in `Cargo.toml` is compiled, and no networking framework is pulled
//!   in;
//! - reference counting is in the types (`Retained`, `CFRetained`), which removes most of
//!   the manual retain and release calls that hand-written bindings need;
//! - none of them needs the macOS SDK at build time, so the crate type-checks on Linux.
//!
//! Carbon is the exception: the family's Carbon crate is empty. The few Carbon functions
//! this crate needs (hotkey registration, the secure-input state, keyboard-layout
//! translation) are declared by hand, each next to the code that calls it.
//!
//! # Rules for this crate
//!
//! - This is one of the two crates where `unsafe` is allowed. Keep each `unsafe` block as
//!   small as the call it wraps, with a `// SAFETY:` comment that says why the call is sound
//!   (the lint `clippy::undocumented_unsafe_blocks` is an error here). Nothing outside the
//!   traits is public, with one agreed exception: [`run_with_main_loop`], because what the
//!   main thread runs is decided by the process's `main`, which no trait method can reach.
//! - No networking framework and no `std::net`: this crate is linked into the service
//!   (architecture §2).
//! - The grants (Accessibility, Input Monitoring) belong to the process that creates the
//!   event tap: the worker, a signed helper app with a stable bundle identifier (ADR-0021).
//!   Nothing here may assume another process's grants.
//! - Shared decisions are not made here: marker values, named-key classification, the
//!   overflow rule, compensation and peer verification are functions of `iamb-platform`.

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod clipboard;
#[cfg(target_os = "macos")]
mod endpoint;
// `clipboard`, `focus`, `hotkeys`, `injector`, `listener` and `permissions` keep their
// decisions in plain Rust submodules that are built and tested on every system; their macOS
// code is gated inside. Off macOS nothing but the tests uses them.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod focus;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod hotkeys;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod injector;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod listener;
#[cfg(target_os = "macos")]
mod login_item;
#[cfg(target_os = "macos")]
mod main_loop;
#[cfg(target_os = "macos")]
mod memory;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod permissions;

#[cfg(target_os = "macos")]
pub use main_loop::run_with_main_loop;

/// The macOS platform: implements every trait of `iamb-platform`
/// (and so [`iamb_platform::Platform`]).
///
/// Create one per process and share it (`Arc<dyn iamb_platform::Platform>`).
#[cfg(target_os = "macos")]
#[derive(Debug, Default)]
pub struct Platform {
    listener: listener::State,
    hotkeys: hotkeys::State,
    permissions: permissions::State,
    injector: injector::State,
    clipboard: clipboard::State,
    focus: focus::State,
    login_item: login_item::State,
    memory: memory::State,
    endpoint: endpoint::State,
}

#[cfg(target_os = "macos")]
impl Platform {
    /// A platform with nothing started: no event tap, no hotkey, no endpoint.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::Platform;

    #[test]
    fn implements_every_platform_trait() {
        fn assert_platform<T: iamb_platform::Platform + 'static>() {}
        assert_platform::<Platform>();
        let shared: std::sync::Arc<dyn iamb_platform::Platform> =
            std::sync::Arc::new(Platform::new());
        // What the status is depends on the machine; that it can be asked through the trait
        // object is the point.
        let _ = shared.permission_status();
    }
}
