//! The two service processes of a soak run, started from the `iamb-soak` binary.
//!
//! - `iamb-soak serve-supervisor`: [`iamb_service::supervisor::run`] with the service's
//!   defaults (backoff 0 s, 1 s, 5 s, 30 s; heartbeat timeout 3 s; start-up grace 10 s) and
//!   `iamb-soak serve-worker …` as the worker command. What `iamb-service --supervise` does,
//!   except for the worker executable.
//! - `iamb-soak serve-worker`: what `iamb-service --worker --platform mock --heartbeat-stdout`
//!   does ([`iamb_service::worker::Worker`] on a [`MockPlatform`], heartbeat on standard output,
//!   exit when standard input ends, log in `logs/worker.log`), with three windows instead of
//!   one (an ordinary editor, a password field and a window of the excluded app), its service
//!   endpoint served through the [`tap`](super::tap), and the [`control`](super::control)
//!   endpoint for the driver.
//!
//! Peers are checked with [`PeerPolicy::SameUser`] in every build: the soak binary is not the
//! signed, installed Iamb binary, so the release policy would refuse the driver.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Weak};
use std::thread;

use iamb_platform::mock::{MockPlatform, MockWindow};
use iamb_platform::{FocusToken, LocalEndpoint, PeerPolicy};
use iamb_service::EXIT_RESTART;
use iamb_service::log::Logger;
use iamb_service::paths::{DataDirs, create_private_dir};
use iamb_service::supervisor::{self, SupervisorConfig};
use iamb_service::worker::{ExitReason, Worker, WorkerConfig};

use super::control::{self, Command, EDITOR_APP, EXCLUDED_APP, Reply, SECURE_APP, Window};
use super::tap::TapEndpoint;

/// Panics are logged by location only, as the service does: a message could quote data.
fn log_panics(log: &Arc<Logger>) {
    let log = Arc::clone(log);
    std::panic::set_hook(Box::new(move |info| {
        let at = info
            .location()
            .map_or_else(String::new, |l| format!(" at {}:{}", l.file(), l.line()));
        log.error(format_args!(
            "panic in thread {}{at}",
            thread::current().name().unwrap_or("?")
        ));
    }));
}

/// The directories of one run's service processes.
#[derive(Debug, Clone)]
pub struct HostDirs<'a> {
    /// The data root.
    pub data: &'a Path,
    /// Where the control endpoint lives.
    pub control: &'a Path,
    /// Where the IPC tap writes.
    pub tap: &'a Path,
}

impl HostDirs<'_> {
    fn args(&self) -> Vec<OsString> {
        vec![
            "--data-dir".into(),
            self.data.as_os_str().to_owned(),
            "--control-dir".into(),
            self.control.as_os_str().to_owned(),
            "--tap-dir".into(),
            self.tap.as_os_str().to_owned(),
        ]
    }

    /// The arguments that start the supervisor from the soak binary.
    pub fn supervisor_args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec!["serve-supervisor".into()];
        args.extend(self.args());
        args
    }
}

/// `iamb-soak serve-supervisor`.
pub fn serve_supervisor(dirs: &HostDirs<'_>) -> ExitCode {
    let data = DataDirs::new(dirs.data);
    let log = Arc::new(Logger::open(&data.logs(), "supervisor"));
    log_panics(&log);
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("iamb-soak: cannot find this executable: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut args: Vec<OsString> = vec!["serve-worker".into()];
    args.extend(dirs.args());
    let cfg = SupervisorConfig::new(data, exe, args);
    match supervisor::run(&cfg, &log) {
        Ok(supervisor::Outcome::Stopped) => ExitCode::SUCCESS,
        Ok(supervisor::Outcome::AlreadyRunning) => {
            eprintln!("iamb-soak: another supervisor runs on this data root");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("iamb-soak: supervisor failed: {e}");
            ExitCode::FAILURE
        }
    }
}

struct Desk {
    os: MockPlatform,
    editor: FocusToken,
    secure: FocusToken,
    excluded: FocusToken,
}

impl Desk {
    fn open() -> Self {
        let os = MockPlatform::new();
        let editor = os.open_window(MockWindow::new(EDITOR_APP, 4101));
        let secure = os.open_window(MockWindow::new(SECURE_APP, 4102).secure());
        let excluded = os.open_window(MockWindow::new(EXCLUDED_APP, 4103));
        let _ = os.focus(editor);
        Self {
            os,
            editor,
            secure,
            excluded,
        }
    }

    const fn token(&self, window: Window) -> FocusToken {
        match window {
            Window::Editor => self.editor,
            Window::Secure => self.secure,
            Window::Excluded => self.excluded,
        }
    }

    fn handle(&self, worker: &Weak<Worker>, command: Command) -> Reply {
        let Some(worker) = worker.upgrade() else {
            return Reply::Error {
                message: "the worker is stopping".to_owned(),
            };
        };
        match command {
            Command::Hello => Reply::Hello {
                pid: std::process::id(),
            },
            Command::Focus { window } => match self.os.focus(self.token(window)) {
                Ok(()) => {
                    worker.sync();
                    Reply::Ok
                }
                Err(e) => Reply::Error {
                    message: e.to_string(),
                },
            },
            Command::Reset => {
                self.os.click();
                for token in [self.editor, self.secure, self.excluded] {
                    self.os.set_text(token, "");
                }
                // The mock keeps a log of every injection; a soak run must not grow it.
                drop(self.os.take_injected());
                worker.sync();
                Reply::Ok
            }
            Command::Type { text } => {
                self.os.type_text(&text);
                Reply::Ok
            }
            Command::Check { window } => {
                worker.settle();
                Reply::Text {
                    text: self.os.text(self.token(window)).unwrap_or_default(),
                }
            }
            Command::Stop => {
                worker.request_exit(ExitReason::Stop);
                Reply::Ok
            }
        }
    }
}

/// `iamb-soak serve-worker`; returns like the service's worker (0 stop, 75 restart me).
pub fn serve_worker(dirs: &HostDirs<'_>) -> ExitCode {
    let data = DataDirs::new(dirs.data);
    let log = Arc::new(Logger::open(&data.logs(), "worker"));
    log_panics(&log);
    let _ = create_private_dir(&data.run());
    log.warn(format_args!(
        "running against the mock platform (soak harness): no real keyboard"
    ));
    let desk = Desk::open();
    let endpoint: Arc<dyn LocalEndpoint> = iamb_ipc::native_endpoint();
    let tap = match TapEndpoint::new(
        Arc::clone(&endpoint),
        dirs.tap,
        &format!("worker{}", std::process::id()),
    ) {
        Ok(tap) => tap,
        Err(e) => {
            eprintln!("iamb-soak: cannot open the IPC tap: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut config = WorkerConfig::new(data, Arc::new(desk.os.clone()), Arc::new(tap));
    config.policy = PeerPolicy::SameUser;
    config.logger = Arc::clone(&log);
    config.heartbeat = Box::new(|| {
        let mut out = io::stdout().lock();
        let _ = out.write_all(b".");
        let _ = out.flush();
    });
    let worker = match Worker::start(config) {
        Ok(w) => Arc::new(w),
        Err(e) => {
            log.error(format_args!("worker did not start: {e}"));
            eprintln!("iamb-soak: worker did not start: {e}");
            return ExitCode::FAILURE;
        }
    };
    worker.settle();

    // The supervisor never writes to our input: its end means the supervisor is gone.
    let exit = worker.exit_handle();
    let watcher = Arc::clone(&log);
    let _ = thread::Builder::new()
        .name("iamb-stdin".to_owned())
        .spawn(move || {
            let mut buf = [0_u8; 64];
            let mut input = io::stdin().lock();
            while matches!(input.read(&mut buf), Ok(n) if n > 0) {}
            watcher.info(format_args!("the supervisor is gone; stopping"));
            exit.request(ExitReason::Stop);
        });

    let listener = match endpoint.bind(&control::endpoint_name(dirs.control), PeerPolicy::SameUser)
    {
        Ok(l) => l,
        Err(e) => {
            log.error(format_args!("cannot serve the soak control endpoint: {e}"));
            eprintln!("iamb-soak: cannot serve the control endpoint: {e}");
            return ExitCode::FAILURE;
        }
    };
    let weak = Arc::downgrade(&worker);
    let desk = Arc::new(desk);
    if let Err(e) = control::serve(
        listener,
        Arc::new(move |command| desk.handle(&weak, command)),
    ) {
        eprintln!("iamb-soak: cannot start the control thread: {e}");
        return ExitCode::FAILURE;
    }

    let reason = worker.wait_exit(None).unwrap_or(ExitReason::Stop);
    // A control call in flight holds the worker for a moment; wait for it, then stop.
    let mut worker = worker;
    for _ in 0..200 {
        match Arc::try_unwrap(worker) {
            Ok(w) => {
                w.stop();
                break;
            }
            Err(again) => {
                worker = again;
                thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    match reason {
        ExitReason::Stop => ExitCode::SUCCESS,
        ExitReason::ListenerDead => ExitCode::from(EXIT_RESTART),
    }
}
