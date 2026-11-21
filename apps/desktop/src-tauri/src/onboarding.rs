//! The onboarding window's side of the contract (task 3.7), independent of Tauri: permissions,
//! the test field, the end of onboarding. `app/onboarding_commands.rs` holds the window and the
//! commands; `apps/desktop/src/onboarding/api.ts` is the typed TypeScript side.
//!
//! | Command | Request sent | Response read |
//! | --- | --- | --- |
//! | `onboarding_permissions` | `GetPermissions` | `Permissions` |
//! | `onboarding_open_permission_settings` | `OpenPermissionSettings` | `Done` |
//! | `onboarding_test_field` | `SetTestField` | `Done` |
//! | `onboarding_finish` | `GetSettings`, `PutSetting` (`palette_hotkey` when unset, `ui.onboarding_done`) | `Settings`, `Done` |
//!
//! # Permissions
//!
//! The grants belong to the worker (ADR-0021), so the UI host never asks the system about
//! itself: [`permissions`] asks the service, and the front end asks again every second while the
//! permissions step is shown and every 30 s while something is missing (specification §11.2,
//! entry 9: the service has no event for it yet).
//!
//! # The test field
//!
//! Typing into Iamb's own windows never expands (architecture §4.5), except in the one window
//! declared with `SetTestField`. [`declare_test_field`] names the window that has the keyboard
//! now (the platform's focus inspector says which, as a [`FocusToken`]) and refuses a window
//! that belongs to another process, so this command can never open another application to
//! expansion. [`clear_test_field`] withdraws the declaration. What is typed in the field never
//! reaches this module: the front end holds it in its own state (architecture §8 rule 3).
//!
//! # The end
//!
//! [`finish`] runs when the last step is reached: it withdraws the test field, stores the
//! default search shortcut when none is set (a `conflict` is reported, not hidden, so the
//! front end can ask for another chord), and records `ui.onboarding_done`.
//!
//! None of these functions logs.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iamb_ipc::{FocusRef, GrantState, PermissionName, RequestBody, ResponseBody};
use iamb_platform::{FocusInspector, FocusToken, LoginItem, LoginItemStatus};
use serde::{Deserialize, Serialize};

use crate::bridge::{Bridge, command};
use crate::contract::{CommandError, Unexpected};
use crate::settings;

/// The requests the onboarding functions send, by wire name.
pub const REQUESTS: &[&str] = &[
    "get_permissions",
    "open_permission_settings",
    "set_test_field",
    "get_settings",
    "put_setting",
];

/// The setting that records that the last step was reached once.
pub const DONE_KEY: &str = "ui.onboarding_done";

/// The search shortcut stored at the end when none is set (specification §12, decision 2).
pub const DEFAULT_PALETTE_CHORD: &str = "Ctrl+Shift+Space";

type Res<T> = Result<T, CommandError>;

/// The state of one permission, as the front end reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grant {
    /// Granted.
    Granted,
    /// Not granted (never asked, or refused: macOS does not tell the two apart).
    Denied,
    /// This system has no such permission (Windows).
    NotRequired,
}

impl From<GrantState> for Grant {
    fn from(g: GrantState) -> Self {
        match g {
            GrantState::Granted => Self::Granted,
            GrantState::Denied => Self::Denied,
            GrantState::NotRequired => Self::NotRequired,
        }
    }
}

/// What `GetPermissions` said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsView {
    /// Accessibility.
    pub accessibility: Grant,
    /// Input Monitoring.
    pub input_monitoring: Grant,
}

impl PermissionsView {
    /// True when something is missing.
    #[must_use]
    pub fn missing(&self) -> bool {
        self.accessibility == Grant::Denied || self.input_monitoring == Grant::Denied
    }

    /// How many are missing (0, 1 or 2).
    #[must_use]
    pub fn missing_count(&self) -> u8 {
        u8::from(self.accessibility == Grant::Denied)
            + u8::from(self.input_monitoring == Grant::Denied)
    }
}

/// How the end of onboarding went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinishView {
    /// The search shortcut stored now or before; absent when the system refused the default
    /// (another app owns it) and none was stored.
    pub palette_hotkey: Option<String>,
    /// The system refused the default shortcut: the front end shows the recorder.
    pub hotkey_refused: bool,
}

/// What "Start Iamb when I log in" is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginView {
    /// Registered.
    Enabled,
    /// Not registered.
    Disabled,
    /// Registered; the user must approve it in the system settings (macOS).
    RequiresApproval,
    /// The helper to register is missing, or this system has no login items.
    Unavailable,
}

impl From<LoginItemStatus> for LoginView {
    fn from(s: LoginItemStatus) -> Self {
        match s {
            LoginItemStatus::Enabled => Self::Enabled,
            LoginItemStatus::Disabled => Self::Disabled,
            LoginItemStatus::RequiresApproval => Self::RequiresApproval,
            LoginItemStatus::NotFound => Self::Unavailable,
        }
    }
}

fn call(bridge: &Bridge, body: RequestBody) -> Res<ResponseBody> {
    bridge.call(body).map_err(command)
}

// ---------------------------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------------------------

/// `GetPermissions`.
#[must_use]
pub const fn get_permissions() -> RequestBody {
    RequestBody::GetPermissions {}
}

/// `OpenPermissionSettings`.
#[must_use]
pub const fn open_permission_settings_request(kind: PermissionName) -> RequestBody {
    RequestBody::OpenPermissionSettings { kind }
}

/// `SetTestField`; no token clears it.
#[must_use]
pub fn set_test_field_request(token: Option<FocusToken>) -> RequestBody {
    RequestBody::SetTestField {
        window: token.map(FocusRef::from),
    }
}

/// The permission the front end named (`input_monitoring` or `accessibility`).
///
/// # Errors
///
/// `invalid` for any other name.
pub fn permission_named(name: &str) -> Res<PermissionName> {
    match name {
        "input_monitoring" => Ok(PermissionName::InputMonitoring),
        "accessibility" => Ok(PermissionName::Accessibility),
        other => Err(CommandError::new(
            "invalid",
            format!("{other:?} is not a permission"),
        )),
    }
}

// ---------------------------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------------------------

/// The permissions in a `Permissions` response.
///
/// # Errors
///
/// [`Unexpected`] for any other response.
pub fn permissions_from(response: ResponseBody) -> Result<PermissionsView, Unexpected> {
    match response {
        ResponseBody::Permissions {
            accessibility,
            input_monitoring,
        } => Ok(PermissionsView {
            accessibility: accessibility.into(),
            input_monitoring: input_monitoring.into(),
        }),
        other => Err(Unexpected { got: other.name() }),
    }
}

fn done_from(response: ResponseBody) -> Res<()> {
    match response {
        ResponseBody::Done {} => Ok(()),
        other => Err(Unexpected { got: other.name() }.into()),
    }
}

// ---------------------------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------------------------

/// The worker's permissions.
///
/// # Errors
///
/// The service's error, or `disconnected`.
pub fn permissions(bridge: &Bridge) -> Res<PermissionsView> {
    Ok(permissions_from(call(bridge, get_permissions())?)?)
}

/// Opens the system settings page of the permission `kind` names.
///
/// # Errors
///
/// `invalid` for an unknown name; `unavailable` when the system refuses to open the page.
pub fn open_permission_settings(bridge: &Bridge, kind: &str) -> Res<()> {
    let kind = permission_named(kind)?;
    done_from(call(bridge, open_permission_settings_request(kind))?)
}

/// The window that has the keyboard now, if it is one of this process's.
///
/// # Errors
///
/// `not_permitted` when the focused window belongs to another process; the platform's error
/// (`unavailable`) when it cannot name the focused window.
pub fn own_focused_window(focus: &dyn FocusInspector, own_pid: u32) -> Res<FocusToken> {
    let token = focus.focus_token().map_err(|e| {
        CommandError::new("unavailable", format!("the focused window is unknown: {e}"))
    })?;
    if token.pid() == own_pid {
        Ok(token)
    } else {
        Err(CommandError::new(
            "not_permitted",
            "only Iamb's own window can be the test field",
        ))
    }
}

/// Declares the window that has the keyboard now as the test field (see the module docs).
///
/// # Errors
///
/// As [`own_focused_window`], and the service's error.
pub fn declare_test_field(bridge: &Bridge, focus: &dyn FocusInspector, own_pid: u32) -> Res<()> {
    let token = own_focused_window(focus, own_pid)?;
    done_from(call(bridge, set_test_field_request(Some(token)))?)
}

/// Withdraws the test field.
///
/// # Errors
///
/// The service's error, or `disconnected`.
pub fn clear_test_field(bridge: &Bridge) -> Res<()> {
    done_from(call(bridge, set_test_field_request(None))?)
}

/// The end of onboarding (module docs). Safe to repeat.
///
/// # Errors
///
/// The service's error, or `disconnected`. A refused shortcut is not an error: it is in the
/// answer.
pub fn finish(bridge: &Bridge) -> Res<FinishView> {
    // A refusal here must not keep the window from finishing: the field may already be gone.
    let _ = clear_test_field(bridge);
    let values: BTreeMap<String, String> = settings::settings(bridge)?;
    let mut view = FinishView {
        palette_hotkey: values.get("palette_hotkey").cloned(),
        hotkey_refused: false,
    };
    if view.palette_hotkey.is_none() {
        match settings::put_setting(bridge, "palette_hotkey", Some(DEFAULT_PALETTE_CHORD)) {
            Ok(()) => view.palette_hotkey = Some(DEFAULT_PALETTE_CHORD.to_owned()),
            Err(e) if e.code == "conflict" => view.hotkey_refused = true,
            Err(e) => return Err(e),
        }
    }
    settings::put_setting(bridge, DONE_KEY, Some("1"))?;
    Ok(view)
}

/// Whether the window opens by itself when the service answers: onboarding was never finished,
/// and it was not already offered at this start (a service that restarts does not bring the
/// window back after the person closed it).
#[must_use]
pub const fn should_open_at_start(done: bool, already_offered: bool) -> bool {
    !done && !already_offered
}

/// Whether the menu offers "Set up Iamb…": only while setup is unfinished (specification §3).
#[must_use]
pub const fn menu_offers_setup(done: bool) -> bool {
    !done
}

/// Whether the menu offers "Fix permissions…": setup is done and a permission is missing (while
/// it is not done, the window itself is the way to them).
#[must_use]
pub fn menu_offers_fix(done: bool, permissions: &PermissionsView) -> bool {
    done && permissions.missing()
}

/// Whether onboarding was finished (the last step reached once).
///
/// # Errors
///
/// The service's error, or `disconnected`.
pub fn is_done(bridge: &Bridge) -> Res<bool> {
    Ok(settings::settings(bridge)?.contains_key(DONE_KEY))
}

// ---------------------------------------------------------------------------------------------
// The host's side: login item and supervisor
// ---------------------------------------------------------------------------------------------

/// Registers or removes the login item, and says what it is now.
///
/// # Errors
///
/// `unavailable` when the system refuses.
pub fn set_login_item(item: &dyn LoginItem, enabled: bool) -> Res<LoginView> {
    let result = if enabled {
        item.enable()
    } else {
        item.disable()
    };
    result.map_err(|e| CommandError::new("unavailable", format!("the login item: {e}")))?;
    login_item(item)
}

/// What the login item is now.
///
/// # Errors
///
/// `unavailable` when the system cannot say.
pub fn login_item(item: &dyn LoginItem) -> Res<LoginView> {
    item.login_item_status()
        .map(LoginView::from)
        .map_err(|e| CommandError::new("unavailable", format!("the login item: {e}")))
}

/// Asks the supervisor to stop and start the worker: "Restart the helper" and "Start it again"
/// (UX specification §4.2, §4.6, §11.1). See [`LaunchSupervisor`].
pub trait Supervisor: Send + Sync {
    /// Stops the worker and starts it again.
    ///
    /// # Errors
    ///
    /// `unavailable` when the supervisor cannot be reached.
    fn restart_worker(&self) -> Res<()>;
}

/// The file in `run/` that asks the supervisor for a worker started afresh: the same name as
/// `iamb_service::supervisor::RESTART_REQUEST` (the UI host does not depend on the service crate;
/// a test compares the two).
pub const RESTART_REQUEST: &str = "restart-worker";

/// How the UI host starts the supervisor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// Runs the executable, `iamb-service --supervise --no-ui-host --data-dir …` (Windows, Linux, and
    /// development builds on macOS).
    Direct(PathBuf),
    /// Opens the helper app inside `Iamb.app` through the system (`open -g`, macOS), so that it
    /// runs as itself, as it does at login, and the permissions it was given stay its own
    /// (ADR-0021), rather than as a child of the UI host.
    Open(PathBuf),
}

/// The supervisor's file name next to the UI host.
const SERVICE_EXE: &str = if cfg!(windows) {
    "iamb-service.exe"
} else {
    "iamb-service"
};

/// Where the supervisor may be for a UI host running from `ui_exe`, best first: on macOS the
/// helper app of the installed layout (`Iamb.app/Contents/Library/LoginItems/Iamb Service.app`,
/// ADR-0021), then, everywhere, `iamb-service` next to the UI host (the Windows installer's
/// layout, `/usr/bin` on Linux, and `target/` while developing).
#[must_use]
pub fn launch_candidates(ui_exe: &Path, macos: bool) -> Vec<Launch> {
    let mut out = Vec::new();
    let Some(dir) = ui_exe.parent() else {
        return out;
    };
    if macos
        && dir.ends_with("Contents/MacOS")
        && let Some(contents) = dir.parent()
    {
        out.push(Launch::Open(
            contents.join("Library/LoginItems/Iamb Service.app"),
        ));
    }
    out.push(Launch::Direct(dir.join(SERVICE_EXE)));
    out
}

/// The program and the arguments that start the supervisor for the data root `root`.
#[must_use]
pub fn launch_command(launch: &Launch, root: &Path) -> (PathBuf, Vec<OsString>) {
    // `--no-ui-host`: this UI host is the one; the supervisor is not to start another.
    let service_args = [
        OsString::from("--supervise"),
        OsString::from("--no-ui-host"),
        OsString::from("--data-dir"),
        root.as_os_str().to_owned(),
    ];
    match launch {
        Launch::Direct(exe) => (exe.clone(), service_args.to_vec()),
        Launch::Open(app) => {
            let mut args = vec![
                OsString::from("-g"),
                app.as_os_str().to_owned(),
                OsString::from("--args"),
            ];
            args.extend(service_args);
            (PathBuf::from("/usr/bin/open"), args)
        }
    }
}

/// The supervisor as the UI host reaches it (`iamb-service`'s `supervisor` module, "Restart
/// requests"): it leaves the restart request in `run/` and starts the supervisor. A supervisor
/// that already runs takes the request (the new one exits at once, finding the lock taken); one
/// that does not run starts with a new worker.
#[derive(Debug, Clone)]
pub struct LaunchSupervisor {
    /// How the supervisor is started; `None` when it was not found.
    pub launch: Option<Launch>,
    /// The data root (passed to the supervisor) and its `run/`.
    pub root: PathBuf,
}

impl LaunchSupervisor {
    /// The supervisor installed with the running UI host, for the data root `root`.
    #[must_use]
    pub fn installed(root: &Path) -> Self {
        let launch = std::env::current_exe().ok().and_then(|exe| {
            launch_candidates(&exe, cfg!(target_os = "macos"))
                .into_iter()
                .find(|l| match l {
                    Launch::Direct(p) => p.is_file(),
                    Launch::Open(p) => p.is_dir(),
                })
        });
        Self {
            launch,
            root: root.to_owned(),
        }
    }
}

impl Supervisor for LaunchSupervisor {
    fn restart_worker(&self) -> Res<()> {
        let Some(launch) = &self.launch else {
            return Err(CommandError::new(
                "unavailable",
                "Iamb cannot find its background service; reinstalling Iamb puts it back",
            ));
        };
        let run = self.root.join("run");
        crate::paths::create_private_dir(&run)
            .and_then(|()| std::fs::write(run.join(RESTART_REQUEST), b""))
            .map_err(|e| {
                CommandError::new("unavailable", format!("cannot ask for a restart: {e}"))
            })?;
        let (program, args) = launch_command(launch, &self.root);
        let mut command = std::process::Command::new(program);
        command
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP: no console window, and the supervisor
            // is not part of the UI host's group.
            command.creation_flags(0x0800_0000 | 0x0000_0200);
        }
        let mut child = command.spawn().map_err(|e| {
            CommandError::new(
                "unavailable",
                format!("cannot start the background service: {e}"),
            )
        })?;
        // Reaped on a thread of its own: the supervisor may run as long as the session does.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

/// The supervisor of a host that has no way to reach one (tests, and a host whose data root is
/// not known).
pub struct NoSupervisor;

impl Supervisor for NoSupervisor {
    fn restart_worker(&self) -> Res<()> {
        Err(CommandError::new(
            "unavailable",
            "this build cannot restart the background service",
        ))
    }
}

/// What the host gives the onboarding commands to reach the system: the focus inspector (none on
/// Linux, which is for development only), the login item and the supervisor.
#[derive(Clone)]
pub struct Host {
    /// Names the window that has the keyboard.
    pub focus: Option<Arc<dyn FocusInspector>>,
    /// Start at login.
    pub login: Option<Arc<dyn LoginItem>>,
    /// Restarts the worker.
    pub supervisor: Arc<dyn Supervisor>,
    /// This process's id (the owner of the windows the test field may be).
    pub pid: u32,
}

impl Host {
    /// [`declare_test_field`] with this host's inspector.
    ///
    /// # Errors
    ///
    /// `unavailable` when there is no inspector; otherwise as [`declare_test_field`].
    pub fn declare(&self, bridge: &Bridge) -> Res<()> {
        let focus = self.focus.as_deref().ok_or_else(|| {
            CommandError::new("unavailable", "this system cannot name the focused window")
        })?;
        declare_test_field(bridge, focus, self.pid)
    }

    /// [`set_login_item`] with this host's login item.
    ///
    /// # Errors
    ///
    /// `unavailable` when there is none or the system refuses.
    pub fn set_login(&self, enabled: bool) -> Res<LoginView> {
        match self.login.as_deref() {
            Some(item) => set_login_item(item, enabled),
            None => Err(CommandError::new(
                "unavailable",
                "this system has no login item",
            )),
        }
    }

    /// What the login item is now.
    ///
    /// # Errors
    ///
    /// As [`Host::set_login`].
    pub fn login_status(&self) -> Res<LoginView> {
        match self.login.as_deref() {
            Some(item) => login_item(item),
            None => Ok(LoginView::Unavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_names_are_the_wire_names() {
        assert_eq!(
            permission_named("input_monitoring").expect("known"),
            PermissionName::InputMonitoring
        );
        assert_eq!(
            permission_named("accessibility").expect("known"),
            PermissionName::Accessibility
        );
        assert_eq!(
            permission_named("camera").expect_err("unknown").code,
            "invalid"
        );
    }

    #[test]
    fn missing_means_denied_not_unrequired() {
        let v = |a, i| PermissionsView {
            accessibility: a,
            input_monitoring: i,
        };
        assert!(v(Grant::Granted, Grant::Denied).missing());
        assert!(!v(Grant::NotRequired, Grant::NotRequired).missing());
        assert!(!v(Grant::Granted, Grant::Granted).missing());
        assert_eq!(v(Grant::Denied, Grant::Denied).missing_count(), 2);
        assert_eq!(v(Grant::Granted, Grant::Denied).missing_count(), 1);
        assert_eq!(v(Grant::NotRequired, Grant::Granted).missing_count(), 0);
    }

    #[test]
    fn the_supervisor_is_looked_for_where_the_installers_put_it() {
        let mac = Path::new("/Applications/Iamb.app/Contents/MacOS/iamb-desktop");
        assert_eq!(
            launch_candidates(mac, true),
            [
                Launch::Open(PathBuf::from(
                    "/Applications/Iamb.app/Contents/Library/LoginItems/Iamb Service.app"
                )),
                Launch::Direct(PathBuf::from(format!(
                    "/Applications/Iamb.app/Contents/MacOS/{SERVICE_EXE}"
                ))),
            ]
        );
        let dev = Path::new("/home/ada/iamb/target/debug/iamb-desktop");
        assert_eq!(
            launch_candidates(dev, true),
            [Launch::Direct(PathBuf::from(format!(
                "/home/ada/iamb/target/debug/{SERVICE_EXE}"
            )))]
        );
        assert_eq!(launch_candidates(mac, false).len(), 1);
        assert!(launch_candidates(Path::new(""), false).is_empty());
    }

    #[test]
    fn the_supervisor_is_started_for_the_same_data_root() {
        let root = Path::new("/data/iamb");
        let (program, args) = launch_command(&Launch::Direct("/x/iamb-service".into()), root);
        assert_eq!(program, Path::new("/x/iamb-service"));
        assert_eq!(
            args,
            ["--supervise", "--no-ui-host", "--data-dir", "/data/iamb"]
        );
        let (program, args) = launch_command(&Launch::Open("/A/Iamb Service.app".into()), root);
        assert_eq!(program, Path::new("/usr/bin/open"));
        assert_eq!(
            args,
            [
                "-g",
                "/A/Iamb Service.app",
                "--args",
                "--supervise",
                "--no-ui-host",
                "--data-dir",
                "/data/iamb"
            ]
        );
    }

    #[test]
    fn without_a_supervisor_a_restart_says_why() {
        let none = LaunchSupervisor {
            launch: None,
            root: PathBuf::from("/nowhere"),
        };
        assert_eq!(
            none.restart_worker().expect_err("not found").code,
            "unavailable"
        );
    }

    #[test]
    fn the_default_chord_is_the_one_the_specification_names() {
        assert_eq!(DEFAULT_PALETTE_CHORD, "Ctrl+Shift+Space");
    }
}
