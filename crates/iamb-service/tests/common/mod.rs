//! A worker on the mock platform, in-process, with a seeded library.

#![allow(dead_code, reason = "each test file uses a different part")]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iamb_core::datetime::LocalDateTime;
use iamb_core::model::{Abbreviation, Id, Snippet, SyncMeta, Timestamp, Trigger};
use iamb_ipc::{Client, EventBody, ResponseBody, Role};
use iamb_platform::mock::{MockPlatform, MockWindow};
use iamb_platform::{FocusToken, LocalEndpoint, PeerPolicy};
use iamb_service::library::{build_view, ensure_personal_profile};
use iamb_service::log::Logger;
use iamb_service::paths::DataDirs;
use iamb_service::worker::{Worker, WorkerConfig};
use iamb_store::Store;
use tempfile::TempDir;

pub const T: Duration = Duration::from_secs(5);

pub fn id(n: u8) -> Id {
    Id::from_bytes([n; 16])
}

/// A library entry: id byte, abbreviation, body, immediate trigger.
pub type Entry = (u8, &'static str, &'static str, bool);

pub const LIBRARY: &[Entry] = &[
    (1, "brb", "be right back", false),
    (2, "omw", "on my way", false),
    (3, "hi", "Hello {field name=Name}!", false),
    (4, ";sig", "Kind regards", true),
];

/// Writes `entries` into the root folder of a fresh store in `root`.
pub fn seed(root: &Path, entries: &[Entry]) {
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
    for (i, (byte, abbr, body, immediate)) in entries.iter().enumerate() {
        let mut s = Snippet::new_defaults(
            SyncMeta::new(id(*byte), Timestamp(10 + i64::try_from(i).expect("small"))),
            folder,
            Abbreviation::new(abbr).expect("abbreviation"),
            "",
            *body,
            None,
        );
        if *immediate {
            s.trigger = Trigger::Immediate;
            s.word_start = false;
        }
        store.put_snippet(&s, Timestamp(10)).expect("snippet");
    }
}

pub struct Harness {
    pub dir: TempDir,
    pub os: MockPlatform,
    pub editor: FocusToken,
    pub worker: Option<Worker>,
}

pub fn fixed_clock() -> LocalDateTime {
    LocalDateTime::new(2026, 3, 18, 14, 5, 9).expect("valid date")
}

impl Harness {
    /// A machine with a focused editor window and a running worker over `entries`.
    pub fn start(entries: &[Entry]) -> Self {
        Self::start_with(entries, |_| {})
    }

    pub fn start_with(entries: &[Entry], tweak: impl FnOnce(&mut WorkerConfig)) -> Self {
        let dir = TempDir::new().expect("temp dir");
        seed(dir.path(), entries);
        let os = MockPlatform::new();
        let editor = os.open_window(MockWindow::new("org.example.editor", 100));
        os.focus(editor).expect("window is open");
        let dirs = DataDirs::new(dir.path());
        let mut config =
            WorkerConfig::new(dirs.clone(), Arc::new(os.clone()), Arc::new(os.clone()));
        config.policy = PeerPolicy::SameUser;
        config.clock = Arc::new(fixed_clock);
        config.logger = Arc::new(Logger::open(&dirs.logs(), "worker"));
        tweak(&mut config);
        let worker = Worker::start(config).expect("worker starts");
        worker.settle();
        Self {
            dir,
            os,
            editor,
            worker: Some(worker),
        }
    }

    pub fn worker(&self) -> &Worker {
        self.worker.as_ref().expect("running")
    }

    pub fn dirs(&self) -> DataDirs {
        DataDirs::new(self.dir.path())
    }

    pub fn settle(&self) {
        self.worker().settle();
    }

    pub fn text(&self, window: FocusToken) -> String {
        self.os.text(window).expect("window is open")
    }

    /// A client of the service endpoint, through the mock's endpoint.
    pub fn client(&self, role: Role) -> Client {
        let stream = self
            .os
            .connect(&self.dirs().service_endpoint(), PeerPolicy::SameUser, T)
            .expect("connects");
        Client::handshake(stream, role, "test", T).expect("handshake")
    }

    pub fn stop(mut self) -> TempDir {
        if let Some(w) = self.worker.take() {
            w.stop();
        }
        // TempDir is returned so the caller can inspect files after the stop.
        std::mem::replace(&mut self.dir, TempDir::new().expect("temp dir"))
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(w) = self.worker.take() {
            w.stop();
        }
    }
}

/// Waits for an event matching `pred`, up to `T`.
pub fn wait_event(client: &mut Client, pred: impl Fn(&EventBody) -> bool) -> EventBody {
    let deadline = Instant::now() + T;
    while Instant::now() < deadline {
        if let Some(event) = client
            .next_event(Some(Duration::from_millis(100)))
            .expect("event stream")
            && pred(&event)
        {
            return event;
        }
    }
    panic!("no matching event within {T:?}");
}

/// The diagnostic codes in the ring.
pub fn diagnostic_codes(client: &mut Client) -> Vec<String> {
    match client
        .call(iamb_ipc::RequestBody::RecentDiagnostics {})
        .expect("diagnostics")
    {
        ResponseBody::Diagnostics { entries } => entries.into_iter().map(|e| e.code).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

/// Every regular file under `dir`.
pub fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else {
            out.push(path);
        }
    }
    out
}
