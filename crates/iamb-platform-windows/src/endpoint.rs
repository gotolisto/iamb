//! The local endpoint: a named pipe. **Task 2.6** fills this file.
//!
//! What it must do (architecture §3.3, §6.1; ADR-0006; the trait docs of [`LocalEndpoint`],
//! `LocalListener` and `LocalStream`):
//!
//! - The pipe is `\\.\pipe\<stem>-<user SID>`, with the stem from
//!   [`EndpointName::pipe_stem`]; when the data root is not the default one, add a suffix
//!   derived from [`EndpointName::run_dir`], so tests never meet the installed service.
//! - `bind` creates the pipe as the first instance of its name (a second server gets
//!   [`EndpointInUse`](iamb_platform::PlatformError::EndpointInUse) and must not start), with
//!   a security descriptor that admits the current user's SID only, and rejects remote
//!   clients.
//! - `accept` and `connect` identify the peer (process id, the user of its token, the path
//!   of its executable; under the installed-binary policy also the signature of that
//!   executable, checked without revocation look-ups so nothing touches the network) and
//!   decide with [`PeerPolicy::check`]. A rejected client is disconnected before `accept`
//!   reports it; a rejected server is never written to.
//! - Streams use overlapped I/O, so a thread blocked in a read does not hold up another
//!   thread's write on a clone, and read timeouts and `shutdown` can cancel a blocked read.

use std::time::Duration;

use iamb_platform::{
    EndpointName, LocalEndpoint, LocalListener, LocalStream, PeerPolicy, PlatformError, Result,
};

use crate::Platform;

/// The endpoint's state.
#[derive(Debug, Default)]
pub(crate) struct State {}

impl LocalEndpoint for Platform {
    fn bind(&self, _name: &EndpointName, _policy: PeerPolicy) -> Result<Box<dyn LocalListener>> {
        let State {} = &self.endpoint;
        Err(PlatformError::Unsupported)
    }

    fn connect(
        &self,
        _name: &EndpointName,
        _policy: PeerPolicy,
        _timeout: Duration,
    ) -> Result<Box<dyn LocalStream>> {
        Err(PlatformError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use iamb_platform::EndpointKind;

    use super::*;

    #[test]
    fn not_implemented_yet() {
        let platform = Platform::new();
        let name = EndpointName::new(EndpointKind::Service, r"C:\data\run");
        assert!(matches!(
            platform.bind(&name, PeerPolicy::SameUser),
            Err(PlatformError::Unsupported)
        ));
        assert!(matches!(
            platform.connect(&name, PeerPolicy::SameUser, Duration::from_secs(1)),
            Err(PlatformError::Unsupported)
        ));
    }
}
