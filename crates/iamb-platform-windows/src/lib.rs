//! Windows implementations of the Iamb platform traits (architecture §5, ADR-0005).
//!
//! On Windows this crate exposes [`Platform`], which implements every trait of
//! `iamb-platform`. On every other system the crate is empty, so the workspace builds
//! everywhere and `cargo clippy --target x86_64-pc-windows-msvc` can check it from Linux
//! (`docs/ci.md`).
//!
//! # Layout
//!
//! Each module implements its traits for [`Platform`] and keeps its own state in its own
//! `State` struct, so the tasks of Phase 2 do not edit the same lines.
//!
//! | Module | Trait | Task |
//! | --- | --- | --- |
//! | `listener` | `KeyListener` | 2.1 |
//! | `hotkeys` | `Hotkeys` | 2.1 |
//! | `injector` | `Injector` | 2.2 |
//! | `clipboard` | `ClipboardAccess` | 2.2 |
//! | `focus` | `FocusInspector` | 2.5 |
//! | `login_item` | `LoginItem` | 2.6 |
//! | `memory` | `MemoryLock` | 2.6 |
//! | `endpoint` | `LocalEndpoint` | 2.6 |
//! | `permissions` | `Permissions` | done: Windows has none |
//!
//! Until its task lands, a module answers [`Unsupported`](iamb_platform::PlatformError).
//!
//! # Rules for this crate
//!
//! - This is one of the two crates where `unsafe` is allowed. Keep each `unsafe` block as
//!   small as the call it wraps, with a `// SAFETY:` comment that says why the call is sound
//!   (the lint `clippy::undocumented_unsafe_blocks` is an error here). Nothing outside the
//!   traits is public.
//! - Bindings come from `windows-sys`: plain function declarations with no wrapper code, and
//!   only the namespaces listed in `Cargo.toml`. No networking namespace and no `std::net`:
//!   this crate is linked into the service (architecture §2).
//! - `windows-sys` has no COM interface types. UI Automation (task 2.5) therefore declares
//!   the few interfaces it calls by hand, next to the code that uses them.
//! - Shared decisions are not made here: marker values, named-key classification, the
//!   overflow rule, compensation and peer verification are functions of `iamb-platform`.

mod clipboard;
#[cfg(windows)]
mod endpoint;
#[cfg(windows)]
mod focus;
mod hotkeys;
mod injector;
mod listener;
#[cfg(windows)]
mod login_item;
#[cfg(windows)]
mod memory;
#[cfg(windows)]
mod permissions;

/// The Windows platform: implements every trait of `iamb-platform`
/// (and so [`iamb_platform::Platform`]).
///
/// Create one per process and share it (`Arc<dyn iamb_platform::Platform>`).
#[cfg(windows)]
#[derive(Debug, Default)]
pub struct Platform {
    listener: listener::State,
    hotkeys: hotkeys::State,
    injector: injector::State,
    clipboard: clipboard::State,
    focus: focus::State,
    login_item: login_item::State,
    memory: memory::State,
    endpoint: endpoint::State,
}

#[cfg(windows)]
impl Platform {
    /// A platform with nothing started: no hook, no hotkey, no endpoint.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::Platform;

    #[test]
    fn implements_every_platform_trait() {
        fn assert_platform<T: iamb_platform::Platform + 'static>() {}
        assert_platform::<Platform>();
        let shared: std::sync::Arc<dyn iamb_platform::Platform> =
            std::sync::Arc::new(Platform::new());
        assert!(shared.permission_status().all_satisfied());
    }
}
