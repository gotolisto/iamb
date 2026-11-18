//! Talking to the running service (architecture §6.3): the command-line tool first tries the
//! service endpoint with the role `cli`; only if nothing answers does it open the store itself.

use std::path::Path;
use std::time::Duration;

use iamb_core::model::{Folder, Id, Snippet, Timestamp};
use iamb_ipc::{Client, IpcError, RequestBody, ResponseBody, Role, SnippetQuery, native_endpoint};
use iamb_platform::{EndpointKind, EndpointName, PeerPolicy};

use crate::error::CliError;
use crate::ids;

/// How long the tool waits for the service to take the connection and answer the hello.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

/// A connection to the service, with the personal profile.
pub struct Remote {
    client: Client,
    /// The personal profile's id.
    pub profile: Id,
    /// Time of this command.
    pub now: Timestamp,
}

impl From<IpcError> for CliError {
    fn from(e: IpcError) -> Self {
        match e {
            IpcError::Remote(body) => Self::new(body.message),
            other => Self::new(format!("lost the connection to the service: {other}")),
        }
    }
}

impl Remote {
    /// Connects to the service of `data_dir`. `Ok(None)` when no service answers (or the one
    /// that does fails verification): the caller then opens the store under its lock.
    pub fn connect(data_dir: &Path) -> Result<Option<Self>, CliError> {
        let name = EndpointName::new(EndpointKind::Service, data_dir.join("run"));
        let client = match Client::connect(
            native_endpoint().as_ref(),
            &name,
            PeerPolicy::for_build(),
            Role::Cli,
            iamb_ipc::VERSION,
            CONNECT_TIMEOUT,
        ) {
            Ok(client) => client,
            // A service of another protocol version runs: say so instead of waiting for a
            // lock it holds.
            Err(IpcError::Remote(body)) => return Err(CliError::new(body.message)),
            Err(_) => return Ok(None),
        };
        let mut remote = Self {
            client,
            profile: Id::from_bytes([0; 16]),
            now: ids::now(),
        };
        match remote.call(RequestBody::ListFolders {})? {
            ResponseBody::Folders { profile, .. } => remote.profile = profile,
            _ => return Err(unexpected()),
        }
        Ok(Some(remote))
    }

    /// Sends a request.
    pub fn call(&mut self, body: RequestBody) -> Result<ResponseBody, CliError> {
        Ok(self.client.call(body)?)
    }

    /// The personal profile's live folders.
    pub fn folders(&mut self) -> Result<Vec<Folder>, CliError> {
        match self.call(RequestBody::ListFolders {})? {
            ResponseBody::Folders { folders, .. } => Ok(folders),
            _ => Err(unexpected()),
        }
    }

    /// The personal profile's live snippets.
    pub fn snippets(&mut self) -> Result<Vec<Snippet>, CliError> {
        match self.call(RequestBody::ListSnippets {
            query: SnippetQuery::default(),
        })? {
            ResponseBody::Snippets { snippets } => Ok(snippets),
            _ => Err(unexpected()),
        }
    }
}

/// The service answered with the wrong kind of response.
pub fn unexpected() -> CliError {
    CliError::new("the service gave an unexpected answer")
}

/// Where a command runs: through the service, or on the store directly.
pub enum Backend {
    /// The service answered.
    Service(Remote),
    /// No service: the store, opened under its lock.
    Local(crate::session::Session),
}

/// Tries the service, then the store (waiting up to 3 s for its lock); if neither is
/// available the error says "the service is starting, try again" (§6.3).
pub fn open(data_dir: &Path) -> Result<Backend, CliError> {
    if let Some(remote) = Remote::connect(data_dir)? {
        return Ok(Backend::Service(remote));
    }
    crate::session::Session::open(data_dir).map(Backend::Local)
}
