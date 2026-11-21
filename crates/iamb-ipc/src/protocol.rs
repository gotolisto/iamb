//! The envelope every frame carries, the handshake and the roles (architecture §6.1, §6.2).
//!
//! On the wire every frame is one JSON object with a `kind`:
//!
//! | `kind` | Direction | Fields |
//! | --- | --- | --- |
//! | `hello` | client → server, first frame | `protocol`, `role`, `client_version` |
//! | `welcome` | server → client, answer to `hello` | `protocol`, `server_version` |
//! | `refused` | server → client, answer to a bad `hello`; the server then closes | `error` |
//! | `request` | client → server | `id`, `body` (a [`RequestBody`], tagged by `type`) |
//! | `response` | server → client | `id`, and either `result` (a [`ResponseBody`], tagged by `type`) or `error` |
//! | `event` | server → client, pushed | `topic`, `body` (an [`EventBody`]) |

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::messages::{EventBody, RequestBody, ResponseBody};

/// The protocol major version this build speaks. A `hello` with another major version is
/// refused with [`ErrorCode::UnsupportedProtocol`].
///
/// # When it is raised (architecture §6.1)
///
/// By one whenever a build could misread what a build of the previous version sends or
/// answers: a request, response or event removed or renamed, a field removed, renamed or given
/// another meaning or type, a new required field, or a request answered with another response
/// type. Additions an older peer can ignore do not raise it: a new request (an older service
/// answers `unknown_request`), a new event, a new optional field. The golden files in
/// `tests/golden/` change with every such step, which is where a review sees it.
///
/// | Version | Change |
/// | --- | --- |
/// | 1 | The first protocol. |
/// | 2 | `PutExclusion` answers `Exclusions` (the rows written or removed) instead of `Done` (the 2.8 privacy review's finding 6; its follow-up's N7). |
/// | 3 | `WireDiagnostic.at` is required (every diagnostic carries its time). `Preview` carries `parts`, `cursor` and `fields` instead of `text`. `TemplateErrorInfo.position` is `start` (with `end` and `snippet` beside it), and template positions are UTF-16 offsets. Additions in the same step, which alone would not raise it: `ListTags`, `DeleteTag`, `ListSnippetSummaries`, `CheckAbbreviation`, `PanelFocus`; tags in `PutSnippet`, `Snippet` and `Snippets`; `SnippetQuery.tag`; the matched field and ranges of a `SearchHit`; the token attributes of `TemplateTokens`; the changed rows of `LibraryChanged`. |
/// |  | Task 3.10 added, without a new version (an older peer ignores them): `RecentSnippets` and `ResetUsage`; `uses` and `last_used` of a `SnippetSummary`; `reference` and `included` of an `ExcludedEntry`; `HealthReport.unused`. |
/// |  | Task 3.12 added, without a new version (an older peer ignores it): `has_fields` of a `SnippetSummary`. |
pub const PROTOCOL_VERSION: u32 = 3;

/// Who a client is (architecture §6.2). Fixed by the `hello` for the life of the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// `iamb-desktop`, the UI host.
    Ui,
    /// `iamb-cli`, the command-line tool.
    Cli,
    /// `iamb-sync`, the sync agent.
    Sync,
}

impl Role {
    /// Every role.
    pub const ALL: [Self; 3] = [Self::Ui, Self::Cli, Self::Sync];

    /// The role's name on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Cli => "cli",
            Self::Sync => "sync",
        }
    }

    /// Whether this role may call requests of `group` and hears its events (§6.2):
    ///
    /// | Role | Groups |
    /// | --- | --- |
    /// | `ui` | Library, Engine control, Forms, Templates, Diagnostics, Settings |
    /// | `cli` | Library, Engine control |
    /// | `sync` | Sync only |
    #[must_use]
    pub const fn may_call(self, group: Group) -> bool {
        match self {
            Self::Ui => !matches!(group, Group::Sync),
            Self::Cli => matches!(group, Group::Library | Group::EngineControl),
            Self::Sync => matches!(group, Group::Sync),
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A message group of the service endpoint (architecture §6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    /// Snippets, folders, tags, search, import and export, health, reviews.
    Library,
    /// Status, pause, palette expansion, dry run, permissions, the onboarding test field.
    EngineControl,
    /// Fill-in forms.
    Forms,
    /// Template parsing and preview for the editor.
    Templates,
    /// The diagnostics ring.
    Diagnostics,
    /// Settings and app exclusions.
    Settings,
    /// The sync agent's messages.
    Sync,
}

impl Group {
    /// Every group.
    pub const ALL: [Self; 7] = [
        Self::Library,
        Self::EngineControl,
        Self::Forms,
        Self::Templates,
        Self::Diagnostics,
        Self::Settings,
        Self::Sync,
    ];
}

/// The first frame a client sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// The protocol major version the client speaks.
    pub protocol: u32,
    /// Who the client is.
    pub role: Role,
    /// The client's build version (for logs and diagnostics).
    pub client_version: String,
}

/// The server's answer to an acceptable [`Hello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    /// The protocol major version the server speaks.
    pub protocol: u32,
    /// The server's build version.
    pub server_version: String,
}

/// A stable error code, for clients to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The `hello` names a protocol major version this server does not speak.
    UnsupportedProtocol,
    /// The first frame was not a `hello`.
    BadHello,
    /// The frame was not valid JSON or not a request.
    BadRequest,
    /// The request's `type` is unknown to this server.
    UnknownRequest,
    /// The client's role may not call this request's group.
    NotPermitted,
    /// The row or snippet named does not exist.
    NotFound,
    /// The change conflicts with the library (an abbreviation in use).
    Conflict,
    /// A value in the request is not acceptable.
    Invalid,
    /// This build has no handler for the request yet (its counterpart process does not exist).
    NotImplemented,
    /// The service cannot do it now (no UI host connected, the store is busy, …).
    Unavailable,
    /// Something failed inside the service.
    Internal,
}

impl ErrorCode {
    /// The code's name on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedProtocol => "unsupported_protocol",
            Self::BadHello => "bad_hello",
            Self::BadRequest => "bad_request",
            Self::UnknownRequest => "unknown_request",
            Self::NotPermitted => "not_permitted",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Invalid => "invalid",
            Self::NotImplemented => "not_implemented",
            Self::Unavailable => "unavailable",
            Self::Internal => "internal",
        }
    }
}

/// An error answer: a code and a sentence for people.
///
/// The message is written by the service; it may name library content (an abbreviation in
/// use) but never typed text or form values. Servers log the code, never the message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// What went wrong.
    pub code: ErrorCode,
    /// A sentence for the user.
    pub message: String,
}

impl ErrorBody {
    /// An error with this code and message.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for ErrorBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// One request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Chosen by the client; echoed in the response. Unique per connection.
    pub id: u64,
    /// What is asked.
    pub body: RequestBody,
}

/// The answer to one request: `result` or `error`, never both.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// The request's id (0 when the request could not be read far enough to know it).
    pub id: u64,
    /// The result, on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ResponseBody>,
    /// The error, on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

impl Response {
    /// A response for request `id` with this outcome.
    #[must_use]
    pub fn new(id: u64, outcome: Result<ResponseBody, ErrorBody>) -> Self {
        match outcome {
            Ok(result) => Self {
                id,
                result: Some(result),
                error: None,
            },
            Err(error) => Self {
                id,
                result: None,
                error: Some(error),
            },
        }
    }

    /// The outcome. A response with both or neither field is a protocol violation, reported
    /// as an [`ErrorCode::BadRequest`] error.
    ///
    /// # Errors
    ///
    /// The server's error.
    pub fn into_result(self) -> Result<ResponseBody, ErrorBody> {
        match (self.result, self.error) {
            (Some(result), None) => Ok(result),
            (None, Some(error)) => Err(error),
            _ => Err(ErrorBody::new(
                ErrorCode::BadRequest,
                "the response has neither or both of result and error",
            )),
        }
    }
}

/// One frame, as it is on the wire. See the [module docs](self).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Wire {
    /// The client's first frame.
    Hello(Hello),
    /// The server accepted the hello.
    Welcome(Welcome),
    /// The server refused the hello and closes the connection.
    Refused {
        /// Why.
        error: ErrorBody,
    },
    /// A request.
    Request(Request),
    /// A response.
    Response(Response),
    /// An event pushed by the server.
    Event(EventBody),
}

impl Wire {
    /// The frame's name for logs: its kind, and for requests, responses and events the message
    /// name. Never content.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Hello(_) => "hello",
            Self::Welcome(_) => "welcome",
            Self::Refused { .. } => "refused",
            Self::Request(r) => r.body.name(),
            Self::Response(r) => match (&r.result, &r.error) {
                (Some(body), _) => body.name(),
                _ => "error",
            },
            Self::Event(e) => e.name(),
        }
    }

    /// Encodes the frame's JSON payload.
    ///
    /// # Errors
    ///
    /// Never for these types in practice; a serialiser failure is reported as
    /// [`ErrorCode::Internal`].
    pub fn encode(&self) -> Result<Vec<u8>, ErrorBody> {
        serde_json::to_vec(self)
            .map_err(|_| ErrorBody::new(ErrorCode::Internal, "the message could not be encoded"))
    }

    /// Decodes a frame's JSON payload.
    ///
    /// # Errors
    ///
    /// [`DecodeError`]: where the JSON is malformed, or that it is not a frame of this
    /// protocol. The error never quotes the payload.
    pub fn decode(payload: &[u8]) -> Result<Self, DecodeError> {
        serde_json::from_slice(payload).map_err(|e| DecodeError {
            line: e.line(),
            column: e.column(),
            syntax: e.is_syntax() || e.is_eof(),
        })
    }
}

/// Why a payload is not a frame of this protocol. Never holds payload content (serde's own
/// messages can quote values, so they are dropped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError {
    /// Line of the failure (1-based).
    pub line: usize,
    /// Column of the failure (1-based).
    pub column: usize,
    /// True when the payload is not JSON at all; false when it is JSON of the wrong shape (an
    /// unknown `kind` or `type`, a missing or mistyped field).
    pub syntax: bool,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = if self.syntax {
            "malformed JSON"
        } else {
            "not a message of this protocol"
        };
        write!(f, "{what} at line {}, column {}", self.line, self.column)
    }
}

impl std::error::Error for DecodeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_call_their_groups_only() {
        use Group::*;
        let table = [
            (Role::Ui, [true, true, true, true, true, true, false]),
            (Role::Cli, [true, true, false, false, false, false, false]),
            (Role::Sync, [false, false, false, false, false, false, true]),
        ];
        for (role, allowed) in table {
            for (group, ok) in Group::ALL.into_iter().zip(allowed) {
                assert_eq!(role.may_call(group), ok, "{role} {group:?}");
            }
        }
        assert!(Role::Ui.may_call(Library));
        assert_eq!(Role::ALL.map(Role::as_str), ["ui", "cli", "sync"]);
    }

    #[test]
    fn a_response_has_exactly_one_outcome() {
        let ok = Response::new(3, Ok(ResponseBody::Done {}));
        assert_eq!(ok.clone().into_result(), Ok(ResponseBody::Done {}));
        let err = Response::new(3, Err(ErrorBody::new(ErrorCode::NotFound, "gone")));
        assert_eq!(
            err.into_result().map_err(|e| e.code),
            Err(ErrorCode::NotFound)
        );
        let broken = Response {
            id: 1,
            result: None,
            error: None,
        };
        assert_eq!(
            broken.into_result().map_err(|e| e.code),
            Err(ErrorCode::BadRequest)
        );
    }

    #[test]
    fn decode_errors_never_quote_the_payload() {
        let e = Wire::decode(br#"{"kind":"request","id":1,"body":{"type":"secret-words"}}"#)
            .expect_err("unknown type");
        assert!(!e.syntax);
        assert!(!e.to_string().contains("secret"));
        let e = Wire::decode(b"{not json").expect_err("syntax");
        assert!(e.syntax);
    }
}
