//! All platform traits as one.

use crate::clipboard::ClipboardAccess;
use crate::endpoint::LocalEndpoint;
use crate::focus::FocusInspector;
use crate::hotkeys::Hotkeys;
use crate::inject::Injector;
use crate::listener::KeyListener;
use crate::login_item::LoginItem;
use crate::memory::MemoryLock;
use crate::permissions::Permissions;

/// Everything Iamb needs from an operating system: every platform trait at once.
///
/// Implemented automatically by any type that implements them all: `Platform` in each OS crate
/// and [`MockPlatform`](crate::mock::MockPlatform). The service holds one `Arc<dyn Platform>`
/// and hands each thread the part it needs; an `Arc<dyn Platform>` coerces to an
/// `Arc<dyn KeyListener>`, an `Arc<dyn Injector>` and so on:
///
/// ```
/// use std::sync::Arc;
/// use iamb_platform::mock::MockPlatform;
/// use iamb_platform::{Injector, KeyListener, Platform};
///
/// let platform: Arc<dyn Platform> = Arc::new(MockPlatform::new());
/// let listener: Arc<dyn KeyListener> = platform.clone();
/// let injector: Arc<dyn Injector> = platform;
/// # let _ = (listener, injector);
/// ```
///
/// No two platform traits share a method name, so calls on a `Platform` are never ambiguous.
pub trait Platform:
    KeyListener
    + Injector
    + FocusInspector
    + ClipboardAccess
    + Hotkeys
    + Permissions
    + LoginItem
    + MemoryLock
    + LocalEndpoint
{
}

impl<T> Platform for T where
    T: KeyListener
        + Injector
        + FocusInspector
        + ClipboardAccess
        + Hotkeys
        + Permissions
        + LoginItem
        + MemoryLock
        + LocalEndpoint
{
}
