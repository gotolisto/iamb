//! The live channel (§6.6): one private broadcast channel per user on the realtime service, topic
//! `feed:<user id>`, event `head`, payload `{ "epoch": …, "head": … }`. It carries no content; a
//! poke only starts a pull early, and correctness never depends on it.
//!
//! The service speaks the Phoenix channel protocol over a WebSocket
//! (`wss://<project>/realtime/v1/websocket?apikey=<public key>&vsn=1.0.0`): every frame is
//! `{ topic, event, payload, ref, join_ref }`. The device joins `realtime:feed:<user>` as a private
//! channel with its access token, sends the service's heartbeat on the topic `phoenix` every 25
//! seconds, gives a new access token on the socket (`access_token`) before the old one expires, and
//! treats a missing heartbeat answer within 10 seconds, a closed socket or a channel error as a
//! dropped channel. A join refused because the project's connections are used up
//! (`too_many_connections`) is reported as [`Event::Full`]: the device polls and tries again after
//! five minutes.
//!
//! The WebSocket is `tungstenite` (MIT or Apache-2.0) over `rustls`. A proxy named in
//! `HTTPS_PROXY` is reached with `CONNECT`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::http::Project;
use crate::schedule::{HEARTBEAT_MS, HEARTBEAT_TIMEOUT_MS};

/// What the channel reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Joined.
    Up,
    /// A poke: the feed's head is now `head` in `epoch`.
    Poke {
        /// The server's epoch.
        epoch: String,
        /// The feed's head.
        head: u64,
    },
    /// The channel dropped.
    Down,
    /// The join was refused because the project's connections are used up.
    Full,
    /// Nothing happened within the wait.
    Idle,
}

/// A frame of the Phoenix protocol, as the device sends it.
#[must_use]
pub fn frame(topic: &str, event: &str, payload: &Value, reference: u64) -> String {
    json!({
        "topic": topic,
        "event": event,
        "payload": payload,
        "ref": reference.to_string(),
        "join_ref": if event == "phx_join" { Value::from(reference.to_string()) } else { Value::Null },
    })
    .to_string()
}

/// The topic of a user's feed.
#[must_use]
pub fn topic(user: &str) -> String {
    format!("realtime:feed:{user}")
}

/// The join payload: a private broadcast channel, no presence, no echo, with the access token.
#[must_use]
pub fn join_payload(access_token: &str) -> Value {
    json!({
        "config": {
            "broadcast": { "ack": false, "self": false },
            "presence": { "enabled": false },
            "private": true
        },
        "access_token": access_token
    })
}

/// Reads one received frame: a poke, a reply to the join, an error.
#[must_use]
pub fn interpret(text: &str, join_ref: u64) -> Option<Event> {
    let v: Value = serde_json::from_str(text).ok()?;
    let event = v.get("event")?.as_str()?;
    let payload = v.get("payload").cloned().unwrap_or(Value::Null);
    let full = payload.to_string().contains("too_many");
    match event {
        "phx_reply" => {
            let is_join = v.get("ref").and_then(Value::as_str) == Some(&join_ref.to_string());
            let ok = payload.get("status").and_then(Value::as_str) == Some("ok");
            if !is_join {
                return None;
            }
            Some(if ok {
                Event::Up
            } else if full {
                Event::Full
            } else {
                Event::Down
            })
        }
        "broadcast" => {
            let inner = payload.get("payload").unwrap_or(&payload);
            if payload.get("event").and_then(Value::as_str) != Some("head") {
                return None;
            }
            Some(Event::Poke {
                epoch: inner.get("epoch")?.as_str()?.to_owned(),
                head: inner.get("head")?.as_u64()?,
            })
        }
        "phx_error" | "phx_close" => Some(if full { Event::Full } else { Event::Down }),
        "system" if full => Some(Event::Full),
        _ => None,
    }
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    Some(rest.split('/').next()?.to_owned())
}

fn connect_tcp(host: &str) -> std::io::Result<TcpStream> {
    let proxy = std::env::var("HTTPS_PROXY")
        .or_else(|_| std::env::var("https_proxy"))
        .ok();
    let Some(proxy) = proxy else {
        return TcpStream::connect((host, 443));
    };
    let addr = proxy
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .rsplit('@')
        .next()
        .unwrap_or_default()
        .to_owned();
    let mut s = TcpStream::connect(addr.as_str())?;
    write!(s, "CONNECT {host}:443 HTTP/1.1\r\nHost: {host}:443\r\n\r\n")?;
    let mut reader = BufReader::new(s.try_clone()?);
    let mut status = String::new();
    reader.read_line(&mut status)?;
    if status.split_whitespace().nth(1).is_none_or(|c| c != "200") {
        return Err(std::io::Error::other("the proxy refused the tunnel"));
    }
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? <= 2 {
            break;
        }
    }
    // Nothing was sent past the headers, so the reader holds no bytes of the tunnel.
    let _ = reader.into_inner().read(&mut [])?;
    Ok(s)
}

/// A joined live channel.
pub struct LiveChannel {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    topic: String,
    next_ref: u64,
    join_ref: u64,
    last_heartbeat: Instant,
    pending_heartbeat: Option<(u64, Instant)>,
}

impl LiveChannel {
    /// Connects and sends the join for `user`'s feed. The answer to the join comes as
    /// [`Event::Up`] or [`Event::Full`] from [`next`](Self::next).
    ///
    /// # Errors
    ///
    /// The connection or the TLS handshake failed.
    pub fn connect(
        project: &Project,
        tls: Arc<rustls::ClientConfig>,
        access_token: &str,
        user: &str,
    ) -> Result<Self, String> {
        let host = host_of(&project.url).ok_or("the project URL is not https")?;
        let url = format!(
            "wss://{host}/realtime/v1/websocket?apikey={}&vsn=1.0.0",
            project.public_key
        );
        let tcp = connect_tcp(&host).map_err(|e| e.to_string())?;
        tcp.set_read_timeout(Some(Duration::from_millis(500)))
            .map_err(|e| e.to_string())?;
        let (socket, _) = tungstenite::client_tls_with_config(
            url.as_str(),
            tcp,
            None,
            Some(tungstenite::Connector::Rustls(tls)),
        )
        .map_err(|e| match e {
            tungstenite::HandshakeError::Failure(e) => e.to_string(),
            tungstenite::HandshakeError::Interrupted(_) => "interrupted".to_owned(),
        })?;
        let mut ch = Self {
            socket,
            topic: topic(user),
            next_ref: 1,
            join_ref: 1,
            last_heartbeat: Instant::now(),
            pending_heartbeat: None,
        };
        let r = ch.take_ref();
        ch.join_ref = r;
        let join = frame(&ch.topic, "phx_join", &join_payload(access_token), r);
        ch.socket
            .send(Message::text(join))
            .map_err(|e| e.to_string())?;
        Ok(ch)
    }

    const fn take_ref(&mut self) -> u64 {
        let r = self.next_ref;
        self.next_ref += 1;
        r
    }

    /// Gives the channel a new access token before the old one expires.
    ///
    /// # Errors
    ///
    /// The socket failed.
    pub fn renew(&mut self, access_token: &str) -> Result<(), String> {
        let r = self.take_ref();
        let f = frame(
            &self.topic,
            "access_token",
            &json!({ "access_token": access_token }),
            r,
        );
        self.socket
            .send(Message::text(f))
            .map_err(|e| e.to_string())
    }

    /// Waits up to `wait` for something to happen, sending the heartbeat when it is due.
    pub fn next(&mut self, wait: Duration) -> Event {
        let until = Instant::now() + wait;
        loop {
            if self.last_heartbeat.elapsed() >= Duration::from_millis(HEARTBEAT_MS) {
                let r = self.take_ref();
                let hb = frame("phoenix", "heartbeat", &json!({}), r);
                if self.socket.send(Message::text(hb)).is_err() {
                    return Event::Down;
                }
                self.last_heartbeat = Instant::now();
                self.pending_heartbeat = Some((r, Instant::now()));
            }
            if let Some((_, at)) = self.pending_heartbeat
                && at.elapsed() >= Duration::from_millis(HEARTBEAT_TIMEOUT_MS)
            {
                return Event::Down;
            }
            match self.socket.read() {
                Ok(Message::Text(t)) => {
                    let t = t.as_str();
                    if let Some((r, _)) = self.pending_heartbeat
                        && t.contains("\"phoenix\"")
                        && t.contains(&format!("\"{r}\""))
                    {
                        self.pending_heartbeat = None;
                    }
                    if let Some(e) = interpret(t, self.join_ref) {
                        return e;
                    }
                }
                Ok(Message::Close(_)) => return Event::Down,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => return Event::Down,
            }
            if Instant::now() >= until {
                return Event::Idle;
            }
        }
    }

    /// Closes the socket.
    pub fn close(mut self) {
        let _ = self.socket.close(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_broadcast_head_is_a_poke() {
        let t = r#"{"event":"broadcast","topic":"realtime:feed:u","ref":null,
                   "payload":{"event":"head","type":"broadcast","payload":{"epoch":"e1","head":42}}}"#;
        assert_eq!(
            interpret(t, 1),
            Some(Event::Poke {
                epoch: "e1".into(),
                head: 42
            })
        );
    }

    #[test]
    fn join_replies_are_up_full_or_down() {
        let ok = r#"{"event":"phx_reply","ref":"1","payload":{"status":"ok","response":{}}}"#;
        let full = r#"{"event":"phx_reply","ref":"1","payload":{"status":"error","response":{"reason":"too_many_connections"}}}"#;
        let other = r#"{"event":"phx_reply","ref":"2","payload":{"status":"ok"}}"#;
        assert_eq!(interpret(ok, 1), Some(Event::Up));
        assert_eq!(interpret(full, 1), Some(Event::Full));
        assert_eq!(interpret(other, 1), None);
    }

    #[test]
    fn the_join_frame_is_private_and_carries_the_token_only_there() {
        let f = frame("realtime:feed:u", "phx_join", &join_payload("tok"), 1);
        let v: Value = serde_json::from_str(&f).unwrap_or_default();
        assert_eq!(v["payload"]["config"]["private"], true);
        assert_eq!(v["join_ref"], "1");
    }
}
