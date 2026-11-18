//! The control endpoint between the driver and the soak worker: one JSON object per line over a
//! local endpoint of its own (named like a sync endpoint, in a directory of its own, so it never
//! meets a real one). It plays the role the OS keyboard and the probe window play in the lab:
//! the driver types through it and reads the fields back.
//!
//! What crosses it (typed keys, canaries included) is the driver's own input. It is never
//! written to a file and never goes through the service endpoint.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use iamb_platform::{
    EndpointKind, EndpointName, LocalEndpoint, LocalListener, LocalStream, PeerPolicy,
};
use serde::{Deserialize, Serialize};

/// The app of the ordinary editor window.
pub const EDITOR_APP: &str = "org.iamb.soak.editor";
/// The app of the window with a password field.
pub const SECURE_APP: &str = "org.iamb.soak.bank";
/// The app the driver puts on the exclusion list.
pub const EXCLUDED_APP: &str = "org.iamb.soak.excluded";

/// One of the mock desktop's windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Window {
    /// An ordinary text field ([`EDITOR_APP`]).
    Editor,
    /// A password field ([`SECURE_APP`]).
    Secure,
    /// An ordinary field of the excluded app ([`EXCLUDED_APP`]).
    Excluded,
}

/// What the driver asks. `Debug` leaves typed text out.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    /// Who are you. → [`Reply::Hello`]
    Hello,
    /// Focus `window` and wait until the engine has seen the change. → [`Reply::Ok`]
    Focus {
        /// The window.
        window: Window,
    },
    /// Click in the focused field (a boundary for the engine), then empty every field and
    /// forget the mock injector's log. → [`Reply::Ok`]
    Reset,
    /// Type `text` on the mock keyboard, one real key per character. → [`Reply::Ok`]
    Type {
        /// The keys.
        text: String,
    },
    /// Wait until the engine is idle and no injection is in flight, then read `window`'s
    /// field. → [`Reply::Text`]
    Check {
        /// The window.
        window: Window,
    },
    /// Stop the worker normally (exit status 0, so the supervisor stops too). → [`Reply::Ok`]
    Stop,
}

/// What the worker answers. `Debug` leaves field text out.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    /// Done.
    Ok,
    /// The worker process.
    Hello {
        /// Its process id.
        pid: u32,
    },
    /// A field's text.
    Text {
        /// The text.
        text: String,
    },
    /// The command failed.
    Error {
        /// Why.
        message: String,
    },
}

// Typed keys and field text may hold canaries: error messages and reports print these types
// with `{:?}`, so `Debug` gives lengths only.
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hello => f.write_str("Hello"),
            Self::Focus { window } => f.debug_struct("Focus").field("window", window).finish(),
            Self::Reset => f.write_str("Reset"),
            Self::Type { text } => f
                .debug_struct("Type")
                .field("chars", &text.chars().count())
                .finish_non_exhaustive(),
            Self::Check { window } => f.debug_struct("Check").field("window", window).finish(),
            Self::Stop => f.write_str("Stop"),
        }
    }
}

impl std::fmt::Debug for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ok => f.write_str("Ok"),
            Self::Hello { pid } => f.debug_struct("Hello").field("pid", pid).finish(),
            Self::Text { text } => f
                .debug_struct("Text")
                .field("chars", &text.chars().count())
                .finish_non_exhaustive(),
            Self::Error { message } => f.debug_struct("Error").field("message", message).finish(),
        }
    }
}

/// The control endpoint's name for `dir`.
pub fn endpoint_name(dir: &Path) -> EndpointName {
    EndpointName::new(EndpointKind::Sync, dir)
}

/// The driver's side.
pub struct ControlClient {
    reader: BufReader<Box<dyn LocalStream>>,
    writer: Box<dyn LocalStream>,
}

impl std::fmt::Debug for ControlClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlClient").finish_non_exhaustive()
    }
}

impl ControlClient {
    /// Connects to the control endpoint in `dir`.
    pub fn connect(
        endpoint: &dyn LocalEndpoint,
        dir: &Path,
        timeout: Duration,
    ) -> Result<Self, String> {
        let stream = endpoint
            .connect(&endpoint_name(dir), PeerPolicy::SameUser, timeout)
            .map_err(|e| format!("control endpoint: {e}"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(|e| format!("control endpoint: {e}"))?;
        let writer = stream
            .try_clone()
            .map_err(|e| format!("control endpoint: {e}"))?;
        Ok(Self {
            reader: BufReader::new(stream),
            writer,
        })
    }

    /// Sends `command` and waits for the reply.
    pub fn call(&mut self, command: &Command) -> Result<Reply, String> {
        let mut line = serde_json::to_string(command).map_err(|e| e.to_string())?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .and_then(|()| self.writer.flush())
            .map_err(|e| format!("control endpoint: {e}"))?;
        let mut answer = String::new();
        match self.reader.read_line(&mut answer) {
            Ok(0) => Err("control endpoint: the worker closed the connection".to_owned()),
            Ok(_) => serde_json::from_str(&answer).map_err(|e| format!("control reply: {e}")),
            Err(e) => Err(format!("control endpoint: {e}")),
        }
    }

    /// Sends `command` and expects [`Reply::Ok`].
    pub fn ok(&mut self, command: &Command) -> Result<(), String> {
        match self.call(command)? {
            Reply::Ok => Ok(()),
            Reply::Error { message } => Err(message),
            other => Err(format!("unexpected control reply {other:?}")),
        }
    }

    /// The text of `window` once the engine is idle.
    pub fn text(&mut self, window: Window) -> Result<String, String> {
        match self.call(&Command::Check { window })? {
            Reply::Text { text } => Ok(text),
            Reply::Error { message } => Err(message),
            other => Err(format!("unexpected control reply {other:?}")),
        }
    }

    /// The worker's process id.
    pub fn pid(&mut self) -> Result<u32, String> {
        match self.call(&Command::Hello)? {
            Reply::Hello { pid } => Ok(pid),
            other => Err(format!("unexpected control reply {other:?}")),
        }
    }
}

/// Serves `listener` on background threads, one per connection, answering with `handler`.
pub fn serve(
    listener: Box<dyn LocalListener>,
    handler: Arc<dyn Fn(Command) -> Reply + Send + Sync>,
) -> std::io::Result<()> {
    thread::Builder::new()
        .name("soak-control".to_owned())
        .spawn(move || {
            while let Ok(stream) = listener.accept() {
                let handler = Arc::clone(&handler);
                let _ = thread::Builder::new()
                    .name("soak-control-conn".to_owned())
                    .spawn(move || connection(stream, &*handler));
            }
        })
        .map(|_| ())
}

fn connection(stream: Box<dyn LocalStream>, handler: &(dyn Fn(Command) -> Reply + Send + Sync)) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let reply = match serde_json::from_str::<Command>(&line) {
            Ok(command) => handler(command),
            // Not the serde message: it can quote the line, which may hold typed keys.
            Err(e) => Reply::Error {
                message: format!("bad command ({:?} at column {})", e.classify(), e.column()),
            },
        };
        let Ok(mut out) = serde_json::to_string(&reply) else {
            return;
        };
        out.push('\n');
        if writer
            .write_all(out.as_bytes())
            .and_then(|()| writer.flush())
            .is_err()
        {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iamb_platform::mock::MockPlatform;

    #[test]
    fn commands_round_trip_over_an_endpoint() {
        let os = MockPlatform::new();
        let dir = Path::new("/mock/control");
        let listener = os
            .bind(&endpoint_name(dir), PeerPolicy::SameUser)
            .expect("bind");
        serve(
            listener,
            Arc::new(|command| match command {
                Command::Hello => Reply::Hello { pid: 42 },
                Command::Check { window } => Reply::Text {
                    text: format!("{window:?}"),
                },
                Command::Stop => Reply::Error {
                    message: "no".to_owned(),
                },
                _ => Reply::Ok,
            }),
        )
        .expect("serve");
        let mut client = ControlClient::connect(&os, dir, Duration::from_secs(5)).expect("connect");
        assert_eq!(client.pid(), Ok(42));
        assert_eq!(client.text(Window::Secure).as_deref(), Ok("Secure"));
        assert_eq!(
            client.ok(&Command::Type {
                text: "a b\n".to_owned()
            }),
            Ok(())
        );
        assert_eq!(client.ok(&Command::Stop), Err("no".to_owned()));
    }

    #[test]
    fn debug_never_shows_typed_text() {
        let typed = Command::Type {
            text: "qzsecretletters".to_owned(),
        };
        let read = Reply::Text {
            text: "qzsecretletters".to_owned(),
        };
        assert!(!format!("{typed:?}").contains("qzsecret"));
        assert!(!format!("{read:?}").contains("qzsecret"));
        assert!(format!("{typed:?}").contains("15"));
    }
}
