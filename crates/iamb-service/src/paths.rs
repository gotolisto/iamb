//! Files on disk (architecture §3.3).

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use iamb_platform::{EndpointKind, EndpointName};

/// The data root and the directories the service uses inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDirs {
    root: PathBuf,
}

impl DataDirs {
    /// The directories of the data root `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The data root: `--data-dir`, else `IAMB_DATA_DIR`, else the per-OS default
    /// (`~/Library/Application Support/Iamb`, `%APPDATA%\Iamb`, `$XDG_DATA_HOME/iamb`).
    ///
    /// # Errors
    ///
    /// When no location can be derived from the environment.
    pub fn resolve(flag: Option<&Path>) -> io::Result<Self> {
        if let Some(p) = flag {
            return Ok(Self::new(p));
        }
        default_root(|k| std::env::var_os(k))
            .map(Self::new)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "cannot find a data directory; pass --data-dir or set IAMB_DATA_DIR",
                )
            })
    }

    /// The data root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `run/`: locks and the socket.
    #[must_use]
    pub fn run(&self) -> PathBuf {
        self.root.join("run")
    }

    /// `logs/`.
    #[must_use]
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// `run/supervisor.lock`.
    #[must_use]
    pub fn supervisor_lock(&self) -> PathBuf {
        self.run().join("supervisor.lock")
    }

    /// `run/restart-worker`: the UI host's request that the supervisor restart the worker now
    /// (`crate::supervisor`, "Restart requests").
    #[must_use]
    pub fn restart_request(&self) -> PathBuf {
        self.run().join(crate::supervisor::RESTART_REQUEST)
    }

    /// `run/desktop.quit`: the UI host leaves it when it ends on purpose (the tray's "Quit", or
    /// another UI host already runs for this data root), so that the supervisor can tell a quit
    /// from a crash where the exit status does not reach it (`crate::supervisor`, "The UI host").
    #[must_use]
    pub fn ui_host_quit_marker(&self) -> PathBuf {
        self.run().join(UI_HOST_QUIT_MARKER)
    }

    /// The service endpoint's name.
    #[must_use]
    pub fn service_endpoint(&self) -> EndpointName {
        EndpointName::new(EndpointKind::Service, self.run())
    }
}

/// The file name of [`DataDirs::ui_host_quit_marker`]; the UI host writes the same name.
pub const UI_HOST_QUIT_MARKER: &str = "desktop.quit";

fn nonempty(v: Option<OsString>) -> Option<PathBuf> {
    v.filter(|s| !s.is_empty()).map(PathBuf::from)
}

/// The default data root, reading the environment through `get`.
pub(crate) fn default_root(get: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(p) = nonempty(get("IAMB_DATA_DIR")) {
        return Some(p);
    }
    system_root(get)
}

/// The per-OS data root, without the `IAMB_DATA_DIR` override.
fn system_root(get: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        nonempty(get("HOME")).map(|h| h.join("Library/Application Support/Iamb"))
    } else if cfg!(windows) {
        nonempty(get("APPDATA")).map(|a| a.join("Iamb"))
    } else {
        nonempty(get("XDG_DATA_HOME"))
            .filter(|p| p.has_root())
            .map(|x| x.join("iamb"))
            .or_else(|| nonempty(get("HOME")).map(|h| h.join(".local/share/iamb")))
    }
}

/// Creates `dir` (and its parents) readable by the user only, and makes an existing `dir` so:
/// mode `0700` on Unix; on Windows a protected DACL for the user's SID alone, inherited by
/// what is inside, set through the platform layer
/// ([`platform::restrict_to_user`](crate::platform::restrict_to_user)).
///
/// # Errors
///
/// I/O errors, and the system's error when the permissions cannot be set.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)?;
        crate::platform::restrict_to_user(dir)
    }
}

/// True when `exe` is the executable of a login-item helper app, the installed layout of the
/// supervisor on macOS (ADR-0021): `….app/Contents/Library/LoginItems/<name>.app/Contents/MacOS/<exe>`.
///
/// The system starts a login item without arguments, so `main` takes this layout to mean
/// `--supervise` when neither `--supervise` nor `--worker` is given. A development binary
/// (`target/…/iamb-service`) and the worker the supervisor starts (always with `--worker`) are
/// unaffected.
#[must_use]
pub fn is_login_item_helper(exe: &Path) -> bool {
    let Some(helper) = exe
        .parent()
        .filter(|p| p.ends_with("Contents/MacOS"))
        .and_then(Path::parent)
        .and_then(Path::parent)
    else {
        return false;
    };
    helper.extension().is_some_and(|ext| ext == "app")
        && helper
            .parent()
            .filter(|p| p.ends_with("Contents/Library/LoginItems"))
            .and_then(Path::parent)
            .and_then(Path::parent)
            .and_then(Path::parent)
            .and_then(Path::extension)
            .is_some_and(|ext| ext == "app")
}

/// The UI host's file name (`iamb-desktop`, `iamb-desktop.exe`).
pub const UI_HOST_EXE: &str = if cfg!(windows) {
    "iamb-desktop.exe"
} else {
    "iamb-desktop"
};

/// The UI host installed with the supervisor whose executable is `exe` (architecture §3.2): in
/// the macOS helper layout (ADR-0021) the main app's executable,
/// `Iamb.app/Contents/MacOS/iamb-desktop`; otherwise `iamb-desktop` next to the supervisor (the
/// Windows installer's layout, `/usr/bin` on Linux, `target/` while developing). Whether it exists
/// is the caller's question.
#[must_use]
pub fn ui_host_beside(exe: &Path) -> Option<PathBuf> {
    if is_login_item_helper(exe) {
        // …/Iamb.app/Contents/Library/LoginItems/<helper>.app/Contents/MacOS/<exe>
        let main_contents = exe.ancestors().nth(6)?;
        return Some(main_contents.join("MacOS").join(UI_HOST_EXE));
    }
    exe.parent().map(|dir| dir.join(UI_HOST_EXE))
}

/// This executable's path with every symbolic link and `..` resolved (the 2026-11 beta review's
/// finding 7): `current_exe()` can be the relative path or the link the program was started
/// with on macOS, and the UI host and the outer app bundle are found relative to it.
///
/// # Errors
///
/// When the path cannot be read or resolved, or the result is not absolute.
pub fn canonical_exe() -> io::Result<PathBuf> {
    canonical_absolute(&std::env::current_exe()?)
}

/// `path` with every symbolic link and `..` resolved; an error unless the result is absolute.
///
/// # Errors
///
/// As [`std::fs::canonicalize`], and `InvalidInput` for a result that is not absolute.
pub fn canonical_absolute(path: &Path) -> io::Result<PathBuf> {
    let resolved = std::fs::canonicalize(path)?;
    if resolved.is_absolute() {
        Ok(resolved)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the path does not resolve to an absolute path",
        ))
    }
}

/// The bundle identifier of the main app, `Iamb.app` (`apps/desktop/src-tauri/tauri.conf.json`).
pub const MAIN_APP_ID: &str = "org.iamb.Iamb";

/// The main app (`Iamb.app`) that contains the login-item helper whose executable is `exe`
/// (ADR-0021): `….app` in `….app/Contents/Library/LoginItems/<helper>.app/Contents/MacOS/<exe>`;
/// `None` outside that layout. `exe` should be [`canonical_exe`], so that the bundle is the one
/// this executable really lives in, not one a link points from.
#[must_use]
pub fn main_app_of_helper(exe: &Path) -> Option<PathBuf> {
    if !is_login_item_helper(exe) {
        return None;
    }
    exe.ancestors().nth(7).map(Path::to_path_buf)
}

/// Checks that `app` is the main app the UI host is started from (`open -a`, macOS; the 2026-11
/// beta review's finding 1): an absolute path that contains the helper executable `helper_exe`
/// itself, a real directory whose `Contents`, `Contents/MacOS` and the UI host's executable in it
/// are no symbolic links, and whose `Info.plist` names [`MAIN_APP_ID`].
///
/// The UI host started through the system is its own responsible process whatever this check
/// says, so it inherits none of the helper's permissions; the check keeps the helper from
/// starting a program a copy of the bundle was given. It is not a signature check: comparing
/// the outer bundle's code signature with the helper's team identifier is a follow-up for the
/// first signed builds (architecture §8, finding 11 of the Phase 2 privacy review).
///
/// # Errors
///
/// What did not hold, for the log.
pub fn check_main_app(app: &Path, helper_exe: &Path) -> Result<(), String> {
    if !app.is_absolute() || !helper_exe.starts_with(app) {
        return Err("the app is not the one that contains this helper".to_owned());
    }
    let real_dir = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_dir());
    let contents = app.join("Contents");
    let macos = contents.join("MacOS");
    if !real_dir(app) || !real_dir(&contents) || !real_dir(&macos) {
        return Err("the app's bundle is not a plain directory".to_owned());
    }
    let host = macos.join(UI_HOST_EXE);
    if !std::fs::symlink_metadata(&host).is_ok_and(|m| m.file_type().is_file()) {
        return Err("the app has no ui host executable, or it is a link".to_owned());
    }
    let plist = contents.join("Info.plist");
    let text = std::fs::symlink_metadata(&plist)
        .ok()
        .filter(|m| m.file_type().is_file() && m.len() <= 1 << 20)
        .and_then(|_| std::fs::read_to_string(&plist).ok())
        .unwrap_or_default();
    if plist_string(&text, "CFBundleIdentifier") != Some(MAIN_APP_ID) {
        return Err("the app's bundle identifier is not Iamb's".to_owned());
    }
    Ok(())
}

/// The string value of `key` in an XML property list (the form the bundler writes), `None`
/// when it is not there as a plain `<string>`.
fn plist_string<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let tag = format!("<key>{key}</key>");
    let rest = text[text.find(&tag)? + tag.len()..].trim_start();
    let rest = rest.strip_prefix("<string>")?;
    rest.find("</string>").map(|end| rest[..end].trim())
}

/// Whether the installed macOS helper may run for the data root `root` (the 2026-11 beta review,
/// note to finding 1; architecture §8): a release build of the helper holds the user's keyboard
/// permissions, so it serves the default data root only, not one another process of the user
/// names with `--data-dir` or `IAMB_DATA_DIR` and filled with a library of its choosing.
/// Development builds, and every executable outside the helper layout, may use any root.
#[must_use]
pub fn helper_may_use_root(
    helper: bool,
    release: bool,
    root: &Path,
    get: impl Fn(&str) -> Option<OsString>,
) -> bool {
    !(helper && release) || system_root(get).is_some_and(|default| default == root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_variable_wins() {
        let root = default_root(|k| (k == "IAMB_DATA_DIR").then(|| OsString::from("/x")));
        assert_eq!(root, Some(PathBuf::from("/x")));
        let dirs = DataDirs::new("/x");
        assert_eq!(dirs.run(), Path::new("/x/run"));
        assert_eq!(dirs.logs(), Path::new("/x/logs"));
        assert_eq!(dirs.supervisor_lock(), Path::new("/x/run/supervisor.lock"));
        assert_eq!(
            dirs.service_endpoint().socket_path(),
            Path::new("/x/run/service.sock")
        );
    }

    #[test]
    fn nothing_known_is_none() {
        assert_eq!(default_root(|_| None), None);
    }

    #[test]
    fn the_ui_host_is_found_where_the_installers_put_it() {
        assert_eq!(
            ui_host_beside(Path::new(
                "/Applications/Iamb.app/Contents/Library/LoginItems/Iamb Service.app/Contents/MacOS/iamb-service"
            )),
            Some(PathBuf::from(format!(
                "/Applications/Iamb.app/Contents/MacOS/{UI_HOST_EXE}"
            )))
        );
        assert_eq!(
            ui_host_beside(Path::new("/opt/iamb/iamb-service")),
            Some(PathBuf::from(format!("/opt/iamb/{UI_HOST_EXE}")))
        );
        assert_eq!(ui_host_beside(Path::new("")), None);
    }

    #[test]
    fn the_installed_helper_is_recognised() {
        assert!(is_login_item_helper(Path::new(
            "/Applications/Iamb.app/Contents/Library/LoginItems/Iamb Service.app/Contents/MacOS/iamb-service"
        )));
        // Not inside an outer app.
        assert!(!is_login_item_helper(Path::new(
            "/tmp/Contents/Library/LoginItems/Iamb Service.app/Contents/MacOS/iamb-service"
        )));
        // The main app's own executable, a sidecar next to it, a development build.
        assert!(!is_login_item_helper(Path::new(
            "/Applications/Iamb.app/Contents/MacOS/iamb-service"
        )));
        assert!(!is_login_item_helper(Path::new(
            "/Users/ada/iamb/target/release/iamb-service"
        )));
        assert!(!is_login_item_helper(Path::new("iamb-service")));
        // Windows and Linux installs.
        assert!(!is_login_item_helper(Path::new(
            r"C:\Users\Ada\AppData\Local\Iamb\iamb-service.exe"
        )));
        assert!(!is_login_item_helper(Path::new("/usr/bin/iamb-service")));
    }

    const HELPER: &str =
        "Iamb.app/Contents/Library/LoginItems/Iamb Service.app/Contents/MacOS/iamb-service";

    /// A main app with the helper inside, as the bundler lays it out, under `dir`.
    fn fake_main_app(dir: &Path, id: &str) -> (PathBuf, PathBuf) {
        let app = dir.join("Iamb.app");
        let helper = dir.join(HELPER);
        std::fs::create_dir_all(helper.parent().expect("parent")).expect("helper dir");
        std::fs::write(&helper, b"").expect("helper");
        std::fs::create_dir_all(app.join("Contents/MacOS")).expect("macos dir");
        std::fs::write(app.join("Contents/MacOS").join(UI_HOST_EXE), b"").expect("ui host");
        std::fs::write(
            app.join("Contents/Info.plist"),
            format!(
                "<?xml version=\"1.0\"?>\n<plist version=\"1.0\"><dict>\n\
                 <key>CFBundleExecutable</key>\n<string>iamb-desktop</string>\n\
                 <key>CFBundleIdentifier</key>\n  <string>{id}</string>\n</dict></plist>\n"
            ),
        )
        .expect("plist");
        (app, helper)
    }

    #[test]
    fn the_main_app_is_the_bundle_around_the_helper() {
        let exe = Path::new("/Applications").join(HELPER);
        assert_eq!(
            main_app_of_helper(&exe),
            Some(PathBuf::from("/Applications/Iamb.app"))
        );
        assert_eq!(
            main_app_of_helper(Path::new(
                "/Applications/Iamb.app/Contents/MacOS/iamb-service"
            )),
            None
        );
        assert_eq!(main_app_of_helper(Path::new("/usr/bin/iamb-service")), None);
    }

    #[test]
    fn only_the_main_app_that_holds_this_helper_is_started() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let root = canonical_absolute(dir.path()).expect("canonical");
        let (app, helper) = fake_main_app(&root, MAIN_APP_ID);
        assert_eq!(check_main_app(&app, &helper), Ok(()));
        // Not around this helper, or not absolute.
        assert!(check_main_app(&root.join("Other.app"), &helper).is_err());
        assert!(check_main_app(Path::new("Iamb.app"), Path::new(HELPER)).is_err());
        // Another bundle identifier.
        let other = tempfile::TempDir::new().expect("temp dir");
        let other_root = canonical_absolute(other.path()).expect("canonical");
        let (app2, helper2) = fake_main_app(&other_root, "com.example.Other");
        assert!(check_main_app(&app2, &helper2).is_err());
        // A planted link in place of the ui host.
        #[cfg(unix)]
        {
            let host = app.join("Contents/MacOS").join(UI_HOST_EXE);
            std::fs::remove_file(&host).expect("remove");
            std::os::unix::fs::symlink(root.join("elsewhere"), &host).expect("link");
            assert!(check_main_app(&app, &helper).is_err());
        }
    }

    #[test]
    fn a_plist_string_is_read_only_from_its_key() {
        let text = "<key>A</key><string>x</string><key>CFBundleIdentifier</key>\n\t<string> org.iamb.Iamb </string>";
        assert_eq!(plist_string(text, "CFBundleIdentifier"), Some(MAIN_APP_ID));
        assert_eq!(plist_string(text, "A"), Some("x"));
        assert_eq!(plist_string(text, "B"), None);
        assert_eq!(plist_string("<key>A</key><true/>", "A"), None);
    }

    #[test]
    fn the_released_helper_serves_the_default_data_root_only() {
        let env = |k: &str| match k {
            "HOME" => Some(OsString::from("/home/ada")),
            "IAMB_DATA_DIR" => Some(OsString::from("/tmp/elsewhere")),
            _ => None,
        };
        let default = system_root(env).expect("a default");
        assert!(helper_may_use_root(true, true, &default, env));
        // IAMB_DATA_DIR does not count for the released helper, nor does --data-dir.
        assert!(!helper_may_use_root(
            true,
            true,
            Path::new("/tmp/elsewhere"),
            env
        ));
        assert!(!helper_may_use_root(
            true,
            true,
            Path::new("/tmp/x"),
            |_| None
        ));
        // A development build, or not the helper: any root.
        assert!(helper_may_use_root(true, false, Path::new("/tmp/x"), env));
        assert!(helper_may_use_root(false, true, Path::new("/tmp/x"), env));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_the_executable_resolves_to_where_it_is() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let root = canonical_absolute(dir.path()).expect("canonical");
        std::fs::create_dir(root.join("real")).expect("dir");
        std::fs::write(root.join("real/iamb-service"), b"").expect("exe");
        std::os::unix::fs::symlink(root.join("real/iamb-service"), root.join("link"))
            .expect("link");
        assert_eq!(
            canonical_absolute(&root.join("link")).expect("resolves"),
            root.join("real/iamb-service")
        );
        assert_eq!(
            ui_host_beside(&canonical_absolute(&root.join("link")).expect("resolves")),
            Some(root.join("real").join(UI_HOST_EXE))
        );
    }
}
