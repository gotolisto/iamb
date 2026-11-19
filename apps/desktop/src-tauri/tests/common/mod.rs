//! A service worker on the mock platform, in this process, and a recording bridge sink.
//!
//! The worker is the real one (`iamb_service::worker::Worker`: listener, engine, injector,
//! focus, store and IPC threads) against `iamb_platform::mock`, as in the service's own tests
//! (`crates/iamb-service/tests/common/mod.rs`). Its endpoint is the mock's in-memory one, or
//! the operating system's for tests that put the UI host in another process.

#![allow(dead_code, reason = "each test file uses a different part")]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use iamb_core::model::{Abbreviation, Id, Snippet, SyncMeta, Timestamp};
use iamb_desktop::bridge::{Bridge, BridgeConfig, BridgeSink};
use iamb_desktop::contract::{ConnectionState, StatusView, UiEvent};
use iamb_ipc::{Client, Role};
use iamb_platform::mock::{MockPlatform, MockWindow};
use iamb_platform::{FocusToken, LocalEndpoint, PeerPolicy};
use iamb_service::library::{build_view, ensure_personal_profile};
use iamb_service::log::Logger as ServiceLogger;
use iamb_service::paths::DataDirs;
use iamb_service::worker::{Worker, WorkerConfig};
use iamb_store::Store;
use tempfile::TempDir;

pub const T: Duration = Duration::from_secs(5);

pub fn id(n: u8) -> Id {
    Id::from_bytes([n; 16])
}

/// id byte, abbreviation, body.
pub const LIBRARY: &[(u8, &str, &str)] = &[
    (1, "brb", "be right back"),
    (2, "omw", "on my way"),
    (3, "hi", "Hello {field name=Name}!"),
];

fn seed(root: &Path) {
    let mut store = Store::open(root).expect("store opens");
    let mut n = 100_u8;
    let mut new_id = || {
        n += 1;
        Id::from_bytes([n; 16])
    };
    let profile = ensure_personal_profile(&mut store, &mut new_id, Timestamp(1)).expect("profile");
    let folder = build_view(&store, profile, 1).expect("view").folders[0]
        .meta
        .id;
    for (i, (byte, abbr, body)) in LIBRARY.iter().enumerate() {
        let s = Snippet::new_defaults(
            SyncMeta::new(id(*byte), Timestamp(10 + i64::try_from(i).expect("small"))),
            folder,
            Abbreviation::new(abbr).expect("abbreviation"),
            format!("label {abbr}"),
            *body,
            None,
        );
        store.put_snippet(&s, Timestamp(10)).expect("snippet");
    }
}

/// Which endpoint the worker serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    /// The mock platform's in-memory endpoint (every OS).
    Mock,
    /// The operating system's (`iamb_ipc::native_endpoint`), for a UI host in another process.
    Native,
}

pub struct Service {
    pub dir: TempDir,
    pub os: MockPlatform,
    pub editor: FocusToken,
    pub endpoint: Endpoint,
    pub worker: Option<Worker>,
}

impl Service {
    /// A machine with a focused editor and a running worker over [`LIBRARY`].
    pub fn start(endpoint: Endpoint) -> Self {
        let dir = TempDir::new().expect("temp dir");
        seed(dir.path());
        let os = MockPlatform::new();
        let editor = os.open_window(MockWindow::new("org.example.editor", 100));
        os.focus(editor).expect("window is open");
        let mut service = Self {
            dir,
            os,
            editor,
            endpoint,
            worker: None,
        };
        service.restart();
        service
    }

    /// Starts the worker (again) over the same data root and machine.
    pub fn restart(&mut self) {
        if let Some(w) = self.worker.take() {
            w.stop();
        }
        let dirs = self.dirs();
        let endpoint: Arc<dyn LocalEndpoint> = match self.endpoint {
            Endpoint::Mock => Arc::new(self.os.clone()),
            Endpoint::Native => iamb_ipc::native_endpoint(),
        };
        let mut config = WorkerConfig::new(dirs.clone(), Arc::new(self.os.clone()), endpoint);
        config.policy = PeerPolicy::SameUser;
        config.logger = Arc::new(ServiceLogger::open(&dirs.logs(), "worker"));
        let worker = Worker::start(config).expect("worker starts");
        worker.settle();
        self.worker = Some(worker);
    }

    /// Stops the worker (the service is gone).
    pub fn stop_worker(&mut self) {
        if let Some(w) = self.worker.take() {
            w.stop();
        }
    }

    pub fn worker(&self) -> &Worker {
        self.worker.as_ref().expect("running")
    }

    pub fn dirs(&self) -> DataDirs {
        DataDirs::new(self.dir.path())
    }

    pub fn text(&self) -> String {
        self.os.text(self.editor).expect("window is open")
    }

    /// Types `text` into the focused window and waits for the expansions to finish.
    pub fn type_text(&self, text: &str) {
        self.os.type_text(text);
        self.worker().settle();
    }

    /// The endpoint a UI host uses to reach this worker.
    pub fn client_endpoint(&self) -> Arc<dyn LocalEndpoint> {
        match self.endpoint {
            Endpoint::Mock => Arc::new(self.os.clone()),
            Endpoint::Native => iamb_ipc::native_endpoint(),
        }
    }

    /// A plain `iamb_ipc` client of another role (the command-line tool's view).
    pub fn client(&self, role: Role) -> Client {
        let stream = self
            .client_endpoint()
            .connect(&self.dirs().service_endpoint(), PeerPolicy::SameUser, T)
            .expect("connects");
        Client::handshake(stream, role, "test", T).expect("handshake")
    }

    /// A bridge to this worker, reporting to a new [`Recorder`], with a fast backoff.
    pub fn bridge(&self) -> (Bridge, Arc<Recorder>) {
        let recorder = Arc::new(Recorder::default());
        let mut cfg = BridgeConfig::new(
            self.client_endpoint(),
            self.dirs().service_endpoint(),
            Arc::new(iamb_desktop::log::Logger::open(
                &self.dirs().logs(),
                "desktop",
            )),
        );
        cfg.policy = PeerPolicy::SameUser;
        cfg.backoff = vec![Duration::from_millis(50), Duration::from_millis(200)];
        let sink: Arc<dyn BridgeSink> = recorder.clone();
        let bridge = Bridge::start(cfg, sink).expect("bridge starts");
        (bridge, recorder)
    }

    /// The worker's log so far.
    pub fn worker_log(&self) -> String {
        fs::read_to_string(self.dirs().logs().join("worker.log")).unwrap_or_default()
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

/// What a bridge reported.
#[derive(Default)]
pub struct Recorder {
    seen: Mutex<(Vec<StatusView>, Vec<UiEvent>)>,
    changed: Condvar,
}

impl BridgeSink for Recorder {
    fn status(&self, view: &StatusView) {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .push(view.clone());
        self.changed.notify_all();
    }

    fn event(&self, event: &UiEvent) {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .1
            .push(event.clone());
        self.changed.notify_all();
    }
}

impl Recorder {
    /// Waits up to `T` for a status report matching `pred`; returns it.
    pub fn wait_status(&self, pred: impl Fn(&StatusView) -> bool) -> StatusView {
        let deadline = Instant::now() + T;
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(v) = seen.0.iter().rev().find(|v| pred(v)) {
                return v.clone();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "no matching status within {T:?}: {:?}",
                seen.0
            );
            seen = self
                .changed
                .wait_timeout(seen, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Waits up to `T` for a connected status.
    pub fn wait_connected(&self) -> StatusView {
        self.wait_status(|v| v.connection == ConnectionState::Connected && v.service.is_some())
    }

    /// Waits up to `T` for an event matching `pred`; returns it.
    pub fn wait_event(&self, pred: impl Fn(&UiEvent) -> bool) -> UiEvent {
        let deadline = Instant::now() + T;
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(e) = seen.1.iter().find(|e| pred(e)) {
                return e.clone();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "no matching event within {T:?}: {:?}",
                seen.1
            );
            seen = self
                .changed
                .wait_timeout(seen, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Forgets what was seen.
    pub fn clear(&self) {
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        seen.0.clear();
        seen.1.clear();
    }

    pub fn statuses(&self) -> Vec<StatusView> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .clone()
    }
}

/// Lines of `log` at WARN or ERROR level.
pub fn problems(log: &str) -> Vec<String> {
    log.lines()
        .filter(|l| l.contains(" WARN ") || l.contains(" ERROR "))
        .map(str::to_owned)
        .collect()
}

/// Every file under `dir` whose name starts with `prefix`.
pub fn files_named(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default()
}
