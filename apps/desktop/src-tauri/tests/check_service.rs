//! `iamb-desktop --check-service` (`iamb_desktop::check`): accepted by a running worker, and a
//! clear failure when there is none, so that the install smoke test can tell.

mod common;

use std::time::Duration;

use common::{Endpoint, Service};
use iamb_platform::PeerPolicy;

#[test]
fn the_check_is_accepted_by_a_running_service_and_fails_without_one() {
    let service = Service::start(Endpoint::Mock);
    let endpoint = service.client_endpoint();
    let name = service.dirs().service_endpoint();
    let status = iamb_desktop::check::check_service(
        endpoint.as_ref(),
        &name,
        PeerPolicy::SameUser,
        Duration::from_secs(5),
    )
    .expect("accepted");
    assert!(status.snippets > 0, "the worker's library: {status:?}");

    let other = tempfile::TempDir::new().expect("temp dir");
    let nobody = iamb_desktop::paths::DataDirs::new(other.path()).service_endpoint();
    let err = iamb_desktop::check::check_service(
        endpoint.as_ref(),
        &nobody,
        PeerPolicy::SameUser,
        Duration::from_millis(600),
    )
    .expect_err("no service there");
    assert!(err.contains("cannot connect"), "{err}");
}
