//! Who the frontmost application is, and which window of it has the focus. Plain Rust, built
//! and tested on every system.
//!
//! # The application's name
//!
//! The name is the bundle identifier, or the executable's file name for a program without
//! one (a bare command-line binary that shows a window). It is never a window title: titles
//! carry document text (architecture §4.1).
//!
//! # Iamb's own processes
//!
//! `own` is true for [`APP_BUNDLE_ID`] and every identifier below it (the service helper
//! `org.iamb.Iamb.Service`, the UI host), compared without regard to ASCII case as macOS
//! does, and for the calling process itself, whatever it is called (a development binary has
//! no bundle identifier).
//!
//! # The window
//!
//! The focus token's window is the Core Graphics window number of the application's
//! frontmost ordinary window: the first window of that process, in the window server's
//! front-to-back order, on layer 0. Panels above the ordinary layer, menus, the Dock and the
//! desktop are other layers. 0 means the application shows no ordinary window.

use iamb_platform::AppIdentity;

/// The bundle identifier of `Iamb.app`. The helper inside it is `org.iamb.Iamb.Service`
/// (`login_item::HELPER_BUNDLE_ID`); a macOS test checks that it counts as Iamb's own.
pub(crate) const APP_BUNDLE_ID: &str = "org.iamb.Iamb";

/// The name of an application that has neither a bundle identifier nor an executable name
/// the system reports. It can still be judged by its focused field; it cannot be excluded.
pub(crate) const UNNAMED: &str = "unknown";

/// Whether `bundle_id` belongs to Iamb.
pub(crate) fn is_own_bundle(bundle_id: &str) -> bool {
    let Some((head, rest)) = bundle_id.split_at_checked(APP_BUNDLE_ID.len()) else {
        return false;
    };
    head.eq_ignore_ascii_case(APP_BUNDLE_ID) && (rest.is_empty() || rest.starts_with('.'))
}

/// The identity of the application the system reports as frontmost: its bundle identifier,
/// the file name of its executable and its process id. `None` when it has no process id (the
/// system reports -1 for an application that has ended).
pub(crate) fn identity(
    bundle_id: Option<&str>,
    executable: Option<&str>,
    pid: i32,
    own_pid: u32,
) -> Option<AppIdentity> {
    let pid = u32::try_from(pid).ok().filter(|&pid| pid != 0)?;
    let bundle_id = bundle_id.filter(|id| !id.is_empty());
    let name = bundle_id
        .or(executable.filter(|name| !name.is_empty()))
        .unwrap_or(UNNAMED);
    let app = AppIdentity::new(name, pid);
    if pid == own_pid || bundle_id.is_some_and(is_own_bundle) {
        Some(app.own())
    } else {
        Some(app)
    }
}

/// One window of the window server's list: the three numbers this crate reads from it. The
/// list's other fields (the window's name among them) are never read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowEntry {
    /// `kCGWindowOwnerPID`: the process that owns the window.
    pub(crate) owner: i64,
    /// `kCGWindowLayer`: 0 for ordinary windows.
    pub(crate) layer: i64,
    /// `kCGWindowNumber`: the window's id.
    pub(crate) number: i64,
}

/// The frontmost ordinary window of process `pid` in `windows`, which are in front-to-back
/// order; 0 when it shows none.
pub(crate) fn front_window(windows: impl IntoIterator<Item = WindowEntry>, pid: u32) -> u64 {
    windows
        .into_iter()
        .filter(|window| window.owner == i64::from(pid) && window.layer == 0)
        .find_map(|window| u64::try_from(window.number).ok().filter(|&id| id != 0))
        .unwrap_or(0)
}

/// Whether `windows` holds window `number` of process `pid`.
pub(crate) fn lists_window(
    windows: impl IntoIterator<Item = WindowEntry>,
    pid: u32,
    number: u64,
) -> bool {
    windows.into_iter().any(|window| {
        window.owner == i64::from(pid) && u64::try_from(window.number).is_ok_and(|n| n == number)
    })
}

#[cfg(test)]
mod tests {
    use iamb_platform::core_types::AppId;

    use super::*;

    const fn window(owner: i64, layer: i64, number: i64) -> WindowEntry {
        WindowEntry {
            owner,
            layer,
            number,
        }
    }

    #[test]
    fn the_name_is_the_bundle_identifier() {
        let app = identity(Some("com.apple.TextEdit"), Some("TextEdit"), 501, 7).expect("an app");
        assert_eq!(app.app, AppId("com.apple.TextEdit".to_owned()));
        assert_eq!(app.pid, 501);
        assert!(!app.own);
    }

    #[test]
    fn without_a_bundle_identifier_the_name_is_the_executable() {
        for bundle_id in [None, Some("")] {
            let app = identity(bundle_id, Some("some-tool"), 9, 7).expect("an app");
            assert_eq!(app.app, AppId("some-tool".to_owned()));
            assert!(!app.own);
        }
    }

    #[test]
    fn an_application_without_any_name_is_still_an_application() {
        for executable in [None, Some("")] {
            let app = identity(None, executable, 9, 7).expect("an app");
            assert_eq!(app.app, AppId(UNNAMED.to_owned()));
        }
    }

    #[test]
    fn an_application_without_a_process_is_no_application() {
        for pid in [-1, 0, i32::MIN] {
            assert_eq!(identity(Some("com.example.gone"), None, pid, 7), None);
        }
    }

    #[test]
    fn iambs_bundles_are_own_in_any_case() {
        for id in [
            "org.iamb.Iamb",
            "org.iamb.Iamb.Service",
            "org.iamb.iamb.desktop",
            "ORG.IAMB.IAMB",
        ] {
            assert!(is_own_bundle(id), "{id}");
            assert!(identity(Some(id), None, 3, 7).expect("an app").own, "{id}");
        }
    }

    #[test]
    fn other_bundles_are_not_own() {
        for id in [
            "",
            "org.iamb",
            "org.iamb.Iam",
            "org.iamb.Iambic",
            "org.iamb.IambService",
            "com.example.org.iamb.Iamb",
            "org.iamb.Iamé",
        ] {
            assert!(!is_own_bundle(id), "{id}");
        }
        assert!(
            !identity(Some("org.iamb.Iambic"), None, 3, 7)
                .expect("an app")
                .own
        );
    }

    #[test]
    fn the_calling_process_is_own_whatever_it_is_called() {
        let app = identity(None, Some("iamb-desktop"), 7, 7).expect("an app");
        assert!(app.own);
        let app = identity(Some("com.example.host"), None, 7, 7).expect("an app");
        assert!(app.own);
    }

    #[test]
    fn the_front_window_is_the_first_ordinary_window_of_the_process() {
        let windows = [
            window(1, 25, 90), // a menu-bar item of another process
            window(42, 3, 80), // a floating panel of the process
            window(7, 0, 70),  // another application's window, in front
            window(42, 0, 60), // the one
            window(42, 0, 50), // a window behind it
        ];
        assert_eq!(front_window(windows, 42), 60);
        assert_eq!(front_window(windows, 7), 70);
    }

    #[test]
    fn an_application_without_an_ordinary_window_has_window_zero() {
        let windows = [window(42, 25, 90), window(42, -2_147_483_603, 2)];
        assert_eq!(front_window(windows, 42), 0);
        assert_eq!(front_window(windows, 5), 0);
        assert_eq!(front_window([], 42), 0);
        // Numbers the system never gives are not a window.
        assert_eq!(front_window([window(42, 0, 0), window(42, 0, -3)], 42), 0);
    }

    #[test]
    fn a_window_is_listed_only_for_its_own_process() {
        let windows = [window(42, 0, 60), window(7, 0, 70)];
        assert!(lists_window(windows, 42, 60));
        assert!(!lists_window(windows, 42, 70));
        assert!(!lists_window(windows, 7, 60));
        assert!(!lists_window([], 42, 60));
    }
}
