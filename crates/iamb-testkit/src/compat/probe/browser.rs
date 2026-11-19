//! The browser variant of the probe (`docs/test-strategy.md` §6): a page with one `<textarea>`,
//! served by the probe on the loopback interface, that stamps `keydown` and `input` events with
//! `performance.now()` and sends them back to the probe.
//!
//! This is test tooling in `iamb-testkit`, never part of the service: the service and every
//! crate it links stay free of networking (ADR-0008). The server only listens on `127.0.0.1`,
//! serves paths under a random per-run token only, refuses a request whose `Host` is not the
//! loopback address and port (no DNS rebinding) or whose `Origin` is another site, and keeps
//! nothing it receives beyond the trial in progress; it logs nothing.
//!
//! | Path (under `/<token>/`) | Does |
//! | --- | --- |
//! | `GET /<token>/` | The page |
//! | `GET /<token>/poll?seen=N` | Long poll (up to 15 s): answers `{"generation":G}` as soon as the probe's reset generation `G` is above `N`. The first poll marks the page connected |
//! | `POST /<token>/events` | A JSON array of page events: `{"k":"key","t":ms,"c":"x"}`, `{"k":"text","t":ms,"v":"…"}`, `{"k":"reset","g":G}` |
//!
//! The page clears and focuses its field when the generation grows and acknowledges with a
//! `reset` event; [`Surface::reset`] waits for that acknowledgement. Event stamps are the page's
//! `performance.now()` in milliseconds (fractions kept), turned into nanoseconds; both T0 and
//! T1 come from that one clock, in the browser's process.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::serve::{Probe, Surface};

/// How long events from the page may take to arrive, on top of the 2 s timeout.
pub const PAGE_SLACK: Duration = Duration::from_secs(1);
/// The longest long poll.
const POLL_HOLD: Duration = Duration::from_secs(15);
/// How long a reset waits for the page.
const RESET_TIMEOUT: Duration = Duration::from_secs(5);
/// Request size limits.
const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 1024 * 1024;

/// The page. It finds its own base path, so it holds no token.
pub const PAGE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Iamb probe</title>
<style>body{font:15px sans-serif;margin:16px}textarea{width:100%;height:70vh;font:15px monospace}</style>
</head><body>
<p>Iamb latency probe. Click into the field once; keep this tab in front while the run lasts.</p>
<textarea id="f" autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck="false"></textarea>
<p id="s">connecting</p>
<script>
"use strict";
const field = document.getElementById("f");
const state = document.getElementById("s");
const base = location.pathname.endsWith("/") ? location.pathname : location.pathname + "/";
let queue = [], sending = false, generation = 0;
function send(ev) { queue.push(ev); flush(); }
async function flush() {
  if (sending || queue.length === 0) return;
  sending = true;
  const batch = queue; queue = [];
  try {
    const r = await fetch(base + "events", {method: "POST", headers: {"Content-Type": "application/json"}, body: JSON.stringify(batch)});
    if (!r.ok) throw new Error(String(r.status));
  } catch (e) {
    queue = batch.concat(queue);
    await new Promise(r => setTimeout(r, 100));
  }
  sending = false;
  if (queue.length > 0) flush();
}
field.addEventListener("keydown", e => {
  const t = performance.now();
  if (e.key.length === 1) send({k: "key", t: t, c: e.key});
  else if (e.key === "Enter") send({k: "key", t: t, c: "\n"});
  else if (e.key === "Tab") send({k: "key", t: t, c: "\t"});
});
field.addEventListener("input", () => { send({k: "text", t: performance.now(), v: field.value}); });
async function poll() {
  for (;;) {
    try {
      const r = await fetch(base + "poll?seen=" + generation, {cache: "no-store"});
      const j = await r.json();
      state.textContent = "connected";
      if (j.generation > generation) {
        generation = j.generation;
        field.value = "";
        field.focus();
        send({k: "reset", g: generation});
      }
    } catch (e) {
      state.textContent = "probe not answering";
      await new Promise(r => setTimeout(r, 500));
    }
  }
}
field.focus();
poll();
</script>
</body></html>
"#;

#[derive(Debug, Default)]
struct PageState {
    generation: u64,
    acked: u64,
    connected: bool,
}

#[derive(Debug)]
struct Shared {
    probe: Arc<Probe>,
    token: String,
    port: u16,
    state: Mutex<PageState>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, PageState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wait_until(&self, timeout: Duration, done: impl Fn(&PageState) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        loop {
            if done(&state) {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// One event from the page.
#[derive(Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
enum PageEvent {
    Key { t: f64, c: String },
    Text { t: f64, v: String },
    Reset { g: u64 },
}

/// A page stamp in ms as ns.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=u64::MAX first; sub-nanosecond digits do not matter"
)]
fn page_ns(ms: f64) -> u64 {
    if ms.is_finite() {
        (ms * 1_000_000.0).round().clamp(0.0, u64::MAX as f64) as u64
    } else {
        0
    }
}

/// The probe served to a browser.
#[derive(Debug)]
pub struct BrowserSurface {
    shared: Arc<Shared>,
    url: String,
}

/// A request as the server needs it.
#[derive(Debug, Default, PartialEq, Eq)]
struct Request {
    method: String,
    target: String,
    host: Option<String>,
    origin: Option<String>,
    body: Vec<u8>,
}

struct Response {
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Response {
    fn text(status: &'static str, body: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: body.as_bytes().to_vec(),
        }
    }

    fn json(body: String) -> Self {
        Self {
            status: "200 OK",
            content_type: "application/json",
            body: body.into_bytes(),
        }
    }
}

fn read_request(stream: &mut impl Read) -> Result<Request, &'static str> {
    let mut reader = BufReader::new(stream);
    let mut head = Vec::new();
    loop {
        let mut line = Vec::new();
        let n = reader
            .by_ref()
            .take(u64::try_from(MAX_HEAD).unwrap_or(u64::MAX))
            .read_until(b'\n', &mut line)
            .map_err(|_| "read failed")?;
        if n == 0 {
            return Err("connection closed");
        }
        head.extend_from_slice(&line);
        if head.len() > MAX_HEAD {
            return Err("header too large");
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
    }
    let head = String::from_utf8(head).map_err(|_| "header not UTF-8")?;
    let mut lines = head.lines();
    let mut first = lines.next().ok_or("empty request")?.split_whitespace();
    let mut req = Request {
        method: first.next().ok_or("no method")?.to_owned(),
        target: first.next().ok_or("no target")?.to_owned(),
        ..Request::default()
    };
    let mut length = 0_usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "host" => req.host = Some(value.to_owned()),
            "origin" => req.origin = Some(value.to_owned()),
            "content-length" => length = value.parse().map_err(|_| "bad length")?,
            _ => {}
        }
    }
    if length > MAX_BODY {
        return Err("body too large");
    }
    req.body = vec![0; length];
    reader.read_exact(&mut req.body).map_err(|_| "short body")?;
    Ok(req)
}

fn write_response(stream: &mut impl Write, r: &Response) {
    let head = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         Cross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Embedder-Policy: require-corp\r\n\
         X-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        r.status,
        r.content_type,
        r.body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&r.body);
    let _ = stream.flush();
}

fn route(shared: &Shared, req: &Request) -> Response {
    let port = shared.port;
    let allowed_host =
        |h: &str| h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}");
    if !req.host.as_deref().is_some_and(allowed_host) {
        return Response::text("403 Forbidden", "wrong host");
    }
    if let Some(origin) = req.origin.as_deref() {
        let ok = origin.strip_prefix("http://").is_some_and(allowed_host);
        if !ok {
            return Response::text("403 Forbidden", "wrong origin");
        }
    }
    let base = format!("/{}/", shared.token);
    let Some(rest) = req.target.strip_prefix(&base) else {
        return Response::text("404 Not Found", "not found");
    };
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    match (req.method.as_str(), path) {
        ("GET", "") => Response {
            status: "200 OK",
            content_type: "text/html; charset=utf-8",
            body: PAGE.as_bytes().to_vec(),
        },
        ("GET", "poll") => {
            let seen: u64 = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("seen="))
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            {
                let mut state = shared.lock();
                state.connected = true;
            }
            shared.changed.notify_all();
            shared.wait_until(POLL_HOLD, |s| s.generation > seen);
            let generation = shared.lock().generation;
            Response::json(format!("{{\"generation\":{generation}}}"))
        }
        ("POST", "events") => {
            let Ok(events) = serde_json::from_slice::<Vec<PageEvent>>(&req.body) else {
                return Response::text("400 Bad Request", "events must be a JSON array");
            };
            for event in events {
                match event {
                    PageEvent::Key { t, c } => {
                        for ch in c.chars() {
                            shared.probe.key(page_ns(t), ch);
                        }
                    }
                    PageEvent::Text { t, v } => shared.probe.text(page_ns(t), &v),
                    PageEvent::Reset { g } => {
                        let mut state = shared.lock();
                        state.acked = state.acked.max(g);
                        drop(state);
                        shared.changed.notify_all();
                    }
                }
            }
            Response::json("{\"ok\":true}".to_owned())
        }
        _ => Response::text("404 Not Found", "not found"),
    }
}

fn token() -> String {
    let mut bytes = [0_u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // Not expected. The token only keeps other local pages away from the probe.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        bytes = nanos.to_le_bytes();
    }
    let mut text = String::with_capacity(32);
    for b in bytes {
        let _ = write!(text, "{b:02x}");
    }
    text
}

impl BrowserSurface {
    /// Starts the server on `127.0.0.1:port` (0: any free port). Events go into `probe`, which
    /// should have been made with [`PAGE_SLACK`].
    pub fn start(probe: Arc<Probe>, port: u16) -> Result<Self, String> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
            .map_err(|e| format!("cannot listen on 127.0.0.1:{port}: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("no local address: {e}"))?
            .port();
        let shared = Arc::new(Shared {
            probe,
            token: token(),
            port,
            state: Mutex::new(PageState::default()),
            changed: Condvar::new(),
        });
        let url = format!("http://127.0.0.1:{port}/{}/", shared.token);
        let accept = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("iamb-probe-http".to_owned())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let shared = Arc::clone(&accept);
                    let _ = std::thread::Builder::new()
                        .name("iamb-probe-request".to_owned())
                        .spawn(move || serve_connection(&shared, stream));
                }
            })
            .map_err(|e| format!("cannot start the server thread: {e}"))?;
        Ok(Self { shared, url })
    }

    /// The port the server listens on.
    pub fn port(&self) -> u16 {
        self.shared.port
    }
}

fn serve_connection(shared: &Shared, mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let response = match read_request(&mut stream) {
        Ok(req) => route(shared, &req),
        Err(why) => Response::text("400 Bad Request", why),
    };
    write_response(&mut stream, &response);
}

impl Surface for BrowserSurface {
    fn kind(&self) -> &'static str {
        "browser"
    }

    fn url(&self) -> Option<String> {
        Some(self.url.clone())
    }

    fn wait_ready(&self, timeout: Duration) -> bool {
        self.shared.wait_until(timeout, |s| s.connected)
    }

    fn reset(&self) -> Result<bool, String> {
        let generation = {
            let mut state = self.shared.lock();
            state.generation += 1;
            state.generation
        };
        self.shared.changed.notify_all();
        if self
            .shared
            .wait_until(RESET_TIMEOUT, |s| s.acked >= generation)
        {
            Ok(true)
        } else {
            Err(
                "the page did not answer the reset (is it open, and is its tab in front?)"
                    .to_owned(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]

    use super::*;
    use crate::compat::probe::session::Trial;

    fn http(port: u16, method: &str, target: &str, host: &str, body: &str) -> (String, String) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let req = format!(
            "{method} {target} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        let (head, body) = out.split_once("\r\n\r\n").unwrap();
        (head.lines().next().unwrap().to_owned(), body.to_owned())
    }

    #[test]
    fn page_stamps_become_nanoseconds() {
        assert_eq!(page_ns(1.5), 1_500_000);
        assert_eq!(page_ns(0.0001), 100);
        assert_eq!(page_ns(-3.0), 0);
        assert_eq!(page_ns(f64::NAN), 0);
    }

    #[test]
    fn requests_are_parsed_with_their_body() {
        let raw = b"POST /t/events HTTP/1.1\r\nHost: 127.0.0.1:9\r\nOrigin: http://127.0.0.1:9\r\nContent-Length: 2\r\n\r\n[]";
        let req = read_request(&mut &raw[..]).unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.target, "/t/events");
        assert_eq!(req.host.as_deref(), Some("127.0.0.1:9"));
        assert_eq!(req.origin.as_deref(), Some("http://127.0.0.1:9"));
        assert_eq!(req.body, b"[]");
        let big = format!("GET / HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert!(read_request(&mut big.as_bytes()).is_err());
    }

    #[test]
    fn a_simulated_page_drives_a_trial_over_loopback() {
        let probe = Arc::new(Probe::new(PAGE_SLACK));
        let surface = BrowserSurface::start(Arc::clone(&probe), 0).unwrap();
        let url = surface.url().unwrap();
        assert!(url.starts_with("http://127.0.0.1:"));
        let port = surface.port();
        let host = format!("127.0.0.1:{port}");
        let base = url.trim_start_matches(&format!("http://{host}")).to_owned();

        // Wrong host, wrong token, the page itself.
        assert!(
            http(port, "GET", &base, "evil.example:80", "")
                .0
                .contains("403")
        );
        assert!(http(port, "GET", "/nottoken/", &host, "").0.contains("404"));
        let (status, page) = http(port, "GET", &base, &host, "");
        assert!(status.contains("200"), "{status}");
        assert!(page.contains("performance.now()"));

        // The page connects (a poll that returns at once, since generation 0 > -1 is not asked).
        assert!(!surface.wait_ready(Duration::from_millis(10)));
        let poller = {
            let base = base.clone();
            let host = host.clone();
            std::thread::spawn(move || http(port, "GET", &format!("{base}poll?seen=0"), &host, ""))
        };
        assert!(surface.wait_ready(Duration::from_secs(5)));

        // A reset raises the generation; the waiting poll answers; the page acknowledges.
        let resetter = std::thread::spawn({
            let shared = Arc::clone(&surface.shared);
            move || {
                shared.wait_until(Duration::from_secs(5), |s| s.generation > 0);
                let g = shared.lock().generation;
                let body = format!("[{{\"k\":\"reset\",\"g\":{g}}}]");
                http(
                    port,
                    "POST",
                    &format!("/{}/events", shared.token),
                    &format!("127.0.0.1:{port}"),
                    &body,
                )
            }
        });
        assert_eq!(surface.reset(), Ok(true));
        assert!(poller.join().unwrap().1.contains("\"generation\":1"));
        assert!(resetter.join().unwrap().0.contains("200"));

        // A trial: keys and text stamped by the page's clock.
        probe
            .arm(
                Trial {
                    cell: "B1".to_owned(),
                    typed: ";lx".to_owned(),
                    expected: "hello\nworld".to_owned(),
                },
                Duration::from_secs(10),
            )
            .unwrap();
        let events = r#"[{"k":"key","t":1000.25,"c":";"},{"k":"key","t":1001,"c":"l"},
            {"k":"key","t":1002.5,"c":"x"},{"k":"text","t":1002.6,"v":";lx"},
            {"k":"text","t":1010,"v":""},{"k":"text","t":1014.75,"v":"hello\nworld"}]"#;
        let (status, _) = http(port, "POST", &format!("{base}events"), &host, events);
        assert!(status.contains("200"));
        let sample = probe.wait(Duration::ZERO).unwrap();
        assert!(sample.pass);
        assert_eq!(sample.latency_ns, 12_250_000);
        assert!(
            http(port, "POST", &format!("{base}events"), &host, "{}")
                .0
                .contains("400"),
            "a body that is not an array is refused"
        );
    }

    #[test]
    fn a_reset_without_a_page_fails() {
        let probe = Arc::new(Probe::new(PAGE_SLACK));
        let surface = BrowserSurface::start(probe, 0).unwrap();
        let started = Instant::now();
        assert!(surface.reset().is_err());
        assert!(started.elapsed() >= RESET_TIMEOUT);
    }
}
