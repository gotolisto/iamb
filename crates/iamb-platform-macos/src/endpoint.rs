//! The local endpoint: a Unix domain socket. **Task 2.6** fills this file.
//!
//! What it must do (architecture §3.3, §6.1; ADR-0006; the trait docs of [`LocalEndpoint`],
//! `LocalListener` and `LocalStream`):
//!
//! - The socket is [`EndpointName::socket_path`], in the data root's `run` directory.
//! - `bind` makes sure that directory is owned by the current user with mode `0700`
//!   (creating it so, and refusing a directory that is not), and binds exclusively: when a
//!   live server already answers on the path, the result is
//!   [`EndpointInUse`](iamb_platform::PlatformError::EndpointInUse) and the caller must not
//!   start; only a stale socket file that nothing answers on may be replaced.
//! - `accept` and `connect` identify the peer from the socket's peer credentials (user id,
//!   process id, the path of its executable; under the installed-binary policy also its code
//!   signature, checked against Iamb's designated requirement) and decide with
//!   [`PeerPolicy::check`]. A rejected client is closed before `accept` reports it; a
//!   rejected server is never written to.
//! - Streams are the socket itself: clones are duplicated descriptors, the read timeout is
//!   the socket's, `shutdown` shuts both directions down.

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
        let name = EndpointName::new(EndpointKind::Service, "/tmp/iamb-test/run");
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
