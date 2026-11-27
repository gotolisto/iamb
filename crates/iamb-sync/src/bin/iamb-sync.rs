//! `iamb-sync`: the sync agent the supervisor starts (architecture §3). A thin binary over the
//! library: it connects to the service's endpoint with the role `sync`, reads the account's tokens
//! from the OS keychain (a private file on Linux), and runs the loop of §6.7 with the schedule of
//! §6.6 and §6.8 and the live channel.
//!
//! Configuration: `--data-dir` or `IAMB_DATA_DIR` (the service's data root); the project's URL and
//! public key in `IAMB_PROJECT_URL` and `IAMB_PROJECT_KEY`, or in `<data>/sync/project.json` as
//! `{ "url": …, "public_key": … }`. Signing in (ADR-0020) is the account endpoint's, task 4.5:
//! until tokens are there the agent waits, with no network connection.
//!
//! The log (standard error) carries message names, counts, durations and codes, never a token or
//! a row's content.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iamb_ipc::{Client, EventBody, Role, SyncStatus, native_endpoint};
use iamb_platform::{EndpointKind, EndpointName, PeerPolicy};
use iamb_sync::auth::{TokenStore, Tokens, platform_store};
use iamb_sync::device::Device;
use iamb_sync::http::{AuthClient, HttpTransport, Project, Refresh, http_agent, rustls_config};
use iamb_sync::realtime::{Event, LiveChannel};
use iamb_sync::schedule::{Schedule, SplitMix};
use iamb_sync::{Agent, AgentConfig, IpcDevice, RoundError};

fn log(what: &str) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    eprintln!("{secs} iamb-sync: {what}");
}

fn data_dir() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--data-dir" {
            return args.next().map(PathBuf::from);
        }
    }
    std::env::var_os("IAMB_DATA_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn project(dir: &Path) -> Option<Project> {
    if let (Ok(url), Ok(key)) = (
        std::env::var("IAMB_PROJECT_URL"),
        std::env::var("IAMB_PROJECT_KEY"),
    ) {
        return Some(Project {
            url,
            public_key: key,
        });
    }
    let text = std::fs::read_to_string(dir.join("sync").join("project.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(Project {
        url: v.get("url")?.as_str()?.to_owned(),
        public_key: v.get("public_key")?.as_str()?.to_owned(),
    })
}

fn connect(dir: &Path) -> Option<Client> {
    let name = EndpointName::new(EndpointKind::Service, dir.join("run"));
    Client::connect(
        native_endpoint().as_ref(),
        &name,
        PeerPolicy::for_build(),
        Role::Sync,
        iamb_ipc::VERSION,
        Duration::from_secs(5),
    )
    .ok()
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// The live channel in a thread of its own, reporting to the loop.
fn live_channel(
    project: Project,
    tokens: Arc<Mutex<Option<Tokens>>>,
    user: String,
) -> Receiver<Event> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let Ok(tls) = rustls_config() else {
            return;
        };
        let mut jitter = SplitMix(u64::try_from(now_secs()).unwrap_or(1));
        let mut sched = Schedule::new();
        let start = Instant::now();
        let ms = || u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        loop {
            let token = tokens
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .map(|t| t.access_token.clone());
            let Some(token) = token else {
                std::thread::sleep(Duration::from_secs(5));
                continue;
            };
            #[allow(
                clippy::single_match_else,
                reason = "a loop per connection reads best as a match"
            )]
            match LiveChannel::connect(&project, tls.clone(), &token, &user) {
                Ok(mut ch) => loop {
                    let e = ch.next(Duration::from_secs(1));
                    match e {
                        Event::Idle => {}
                        Event::Down | Event::Full => {
                            let full = e == Event::Full;
                            if tx.send(e).is_err() {
                                return;
                            }
                            sched.channel_down(ms(), full, &mut jitter);
                            break;
                        }
                        other => {
                            if tx.send(other).is_err() {
                                return;
                            }
                        }
                    }
                },
                Err(_) => {
                    if tx.send(Event::Down).is_err() {
                        return;
                    }
                    sched.channel_down(ms(), false, &mut jitter);
                }
            }
            let wait = sched.reconnect_at.unwrap_or(0).saturating_sub(ms());
            std::thread::sleep(Duration::from_millis(wait));
        }
    });
    rx
}

#[allow(clippy::too_many_lines, reason = "the agent's whole life in one place")]
fn main() {
    let Some(dir) = data_dir() else {
        log("no data directory (pass --data-dir or set IAMB_DATA_DIR)");
        std::process::exit(2);
    };
    let Some(project) = project(&dir) else {
        log("no project configured; not syncing");
        std::process::exit(0);
    };
    let store: Box<dyn TokenStore> = match platform_store(&dir) {
        Ok(s) => s,
        Err(e) => {
            log(&format!("the token store is not available: {}", e.kind()));
            std::process::exit(1);
        }
    };
    let Ok(agent_http) = http_agent() else {
        log("no trusted root certificates");
        std::process::exit(1);
    };
    let auth = AuthClient::new(agent_http.clone(), project.clone());
    let tokens: Arc<Mutex<Option<Tokens>>> = Arc::new(Mutex::new(None));
    // Signed out: no network connection until tokens are there (ADR-0020).
    loop {
        if let Ok(Some(t)) = store.load() {
            *tokens.lock().unwrap_or_else(PoisonError::into_inner) = Some(t);
            break;
        }
        std::thread::sleep(Duration::from_secs(10));
    }
    let client = loop {
        if let Some(c) = connect(&dir) {
            break c;
        }
        std::thread::sleep(Duration::from_secs(2));
    };
    let transport = HttpTransport::new(agent_http, project.clone(), tokens.clone());
    let mut agent = Agent::new(
        transport,
        IpcDevice::new(client),
        AgentConfig::default(),
        Box::new(iamb_sync::new_v7),
    );
    let start = Instant::now();
    let ms = || u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut sched = Schedule::new();
    let mut jitter = SplitMix(u64::try_from(now_secs()).unwrap_or(7));
    let mut channel: Option<Receiver<Event>> = None;
    loop {
        // Events: the outbox changed, a poke, the channel's state.
        let wait = sched.next_round(ms()).saturating_sub(ms()).min(1_000);
        if let Ok(Some(EventBody::OutboxChanged { .. })) =
            agent.device.next_event(Some(Duration::from_millis(wait)))
        {
            sched.outbox_changed(ms());
        }
        if let Some(rx) = &channel {
            while let Ok(e) = rx.try_recv() {
                match e {
                    Event::Up => sched.channel_up(ms()),
                    Event::Poke { .. } => sched.poked(ms()),
                    Event::Down => sched.channel_down(ms(), false, &mut jitter),
                    Event::Full => sched.channel_down(ms(), true, &mut jitter),
                    Event::Idle => {}
                }
            }
        }
        if sched.next_round(ms()) > ms() {
            continue;
        }
        let began = Instant::now();
        match agent.sync() {
            Ok(r) => {
                sched.synced(ms());
                log(&format!(
                    "round: {} pages, {} rows, {} pushed in {} ms",
                    r.pages,
                    r.rows,
                    r.pushed,
                    began.elapsed().as_millis()
                ));
                let _ = agent.device.set_status(&SyncStatus {
                    signed_in: true,
                    last_sync: Some(iamb_core::model::Timestamp(now_secs() * 1_000)),
                    error: None,
                });
                if channel.is_none()
                    && let Some(h) = agent.hello_info()
                {
                    channel = Some(live_channel(
                        project.clone(),
                        tokens.clone(),
                        h.user.to_string(),
                    ));
                }
            }
            Err(RoundError::Unauthorized) => {
                let refresh = tokens
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                    .map(|t| t.refresh_token.clone());
                match refresh.map(|r| auth.refresh(&r, now_secs())) {
                    Some(Refresh::Renewed(t)) => {
                        let _ = store.save(&t);
                        *tokens.lock().unwrap_or_else(PoisonError::into_inner) = Some(t);
                        sched.wait(ms(), 0);
                    }
                    Some(Refresh::Failed(_)) => sched.failed(ms(), &mut jitter),
                    Some(Refresh::Refused) | None => {
                        // The session ended without Iamb's word: set aside, keep what is the
                        // user's own (§6.1, D14).
                        log("the session ended; signed out");
                        let _ = agent.device.signed_out(false, true);
                        let _ = store.clear();
                        std::process::exit(0);
                    }
                }
            }
            Err(RoundError::Retry(e)) => {
                log(&format!("no answer ({}); retrying", short(&e.to_string())));
                sched.failed(ms(), &mut jitter);
            }
            Err(RoundError::RateLimited(wait)) => sched.wait(ms(), wait),
            Err(RoundError::SignedOut { reason, .. }) => {
                log(&format!("signed out: {reason}"));
                let _ = store.clear();
                std::process::exit(0);
            }
            Err(RoundError::Stopped(s)) => {
                log(&format!("stopped: {s:?}"));
                std::thread::sleep(Duration::from_hours(1));
                agent.clear_stop();
            }
            Err(e) => {
                log(&format!("round failed: {}", short(&e.to_string())));
                sched.failed(ms(), &mut jitter);
            }
        }
    }
}

/// The first words of an error, without anything that could be content.
fn short(s: &str) -> String {
    s.split(':')
        .next()
        .unwrap_or_default()
        .chars()
        .take(60)
        .collect()
}
