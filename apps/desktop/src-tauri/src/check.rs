//! `iamb-desktop --check-service`: connects to the service of the data root as the UI host
//! does (role `ui`, [`PeerPolicy::for_build`]), asks for its status and reports, without a
//! window. The install smoke test runs it against the installed service, so that a service
//! that refuses the installed UI host fails CI (the 2.8 privacy review's finding 11; before it
//! was closed the library window of an installed release build could not reach the service,
//! and nothing caught it).

use std::time::{Duration, Instant};

use iamb_ipc::{Client, IpcError, RequestBody, ResponseBody, Role, ServiceStatus};
use iamb_platform::{EndpointName, LocalEndpoint, PeerPolicy, PlatformError};

/// How long one connection attempt may take.
const ATTEMPT: Duration = Duration::from_secs(5);

/// Connects to the service at `name` as the UI host, retrying while there is no service yet
/// until `wait` has passed (it may still be starting), and returns its status. A refusal, on
/// either side, is final at once.
///
/// # Errors
///
/// The last failure, for the user: no service, a refused connection, a refused role.
pub fn check_service(
    endpoint: &dyn LocalEndpoint,
    name: &EndpointName,
    policy: PeerPolicy,
    wait: Duration,
) -> Result<ServiceStatus, String> {
    let deadline = Instant::now() + wait;
    loop {
        let (failure, again) =
            match Client::connect(endpoint, name, policy, Role::Ui, crate::VERSION, ATTEMPT) {
                Ok(mut client) => match client.call(RequestBody::GetStatus {}) {
                    Ok(ResponseBody::Status { status }) => return Ok(status),
                    Ok(other) => (
                        format!("the service answered {} to get_status", other.name()),
                        false,
                    ),
                    Err(e) => (format!("the service refused get_status: {e}"), false),
                },
                Err(e) => (
                    format!("cannot connect to the service as the UI host: {e}"),
                    matches!(e, IpcError::Platform(PlatformError::EndpointNotFound)),
                ),
            };
        if !again || Instant::now() >= deadline {
            return Err(failure);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}
