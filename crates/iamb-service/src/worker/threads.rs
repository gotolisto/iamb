//! The injector, focus and watchdog threads.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use iamb_platform::{
    EventSink, FocusChange, InjectionRequest, PlatformError, SECURE_STATE_TIMEOUT, TypedHandoff,
    verdict,
};

use super::{
    EngineMsg, ExitReason, FocusMsg, SELF_TEST_TIMEOUT, Shared, WakeCounters, recv_counted,
};

/// The injector thread: one expansion at a time, each answered with its outcome (§4.6).
pub(crate) fn injector(
    shared: &Shared,
    handoff: &Arc<TypedHandoff>,
    rx: &Receiver<InjectionRequest>,
) {
    while let Ok(request) = rx.recv() {
        WakeCounters::bump(&shared.wakes.injector);
        let outcome = shared.platform.inject(&request, handoff);
        if !shared.to_engine(EngineMsg::Injected {
            expansion: request.expansion,
            outcome,
        }) {
            break;
        }
    }
}

/// The focus thread: turns each focus change into `FocusChanged` and, after asking the OS
/// whether the field is secure (at most 100 ms), the verdict. Iamb's own windows, exclusions
/// and the onboarding test field are applied by [`verdict`]; anything the OS cannot answer in
/// time is `Unknown`, which keeps the engine suspended (§4.1: fail closed).
pub(crate) fn focus(shared: &Shared, rx: &Receiver<FocusMsg>) {
    let mut current: Option<FocusChange> = None;
    while let Ok(msg) = rx.recv() {
        WakeCounters::bump(&shared.wakes.focus);
        let change = match msg {
            FocusMsg::Change(change) => change,
            FocusMsg::Rejudge => match &current {
                Some(change) => change.clone(),
                None => continue,
            },
            FocusMsg::Forward(msg) => {
                if shared.to_engine(msg) {
                    continue;
                }
                break;
            }
            FocusMsg::Stop => break,
        };
        current = Some(change.clone());
        let token = change.token;
        let app = change.app.clone();
        if !shared.to_engine(EngineMsg::FocusChanged(change)) {
            break;
        }
        let field = shared.platform.secure_state(&token, SECURE_STATE_TIMEOUT);
        let excluded = shared.view.get().is_excluded(&app.app.0);
        let test_field = *shared
            .test_field
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            == Some(token);
        let state = verdict(&app, field, excluded, test_field);
        if !shared.to_engine(EngineMsg::Verdict(state)) {
            break;
        }
    }
}

/// The watchdog thread: every `interval` the listener's self-test (§3.2). The first failure
/// reinstalls the hook; a second one in a row asks the worker to exit, so the supervisor
/// restarts it.
pub(crate) fn watchdog(
    shared: &Shared,
    stop: &Receiver<()>,
    interval: Duration,
    sink: &dyn Fn() -> Box<dyn EventSink>,
) {
    let mut failures = 0;
    while let Err(RecvTimeoutError::Timeout) = recv_counted(stop, interval, &shared.wakes.watchdog)
    {
        match shared.platform.self_test(SELF_TEST_TIMEOUT) {
            Ok(()) => failures = 0,
            Err(e) => {
                failures += 1;
                shared
                    .logger
                    .warn(format_args!("listener self-test failed ({failures}): {e}"));
                if failures >= 2 {
                    shared
                        .logger
                        .error(format_args!("the listener is dead; exiting for a restart"));
                    shared.exit.request(ExitReason::ListenerDead);
                    break;
                }
                let _ = shared.platform.stop();
                let started = shared.platform.start(sink());
                shared
                    .listener_ok
                    .store(started.is_ok(), std::sync::atomic::Ordering::Relaxed);
                match started {
                    Ok(()) => shared.logger.info(format_args!("listener reinstalled")),
                    Err(PlatformError::PermissionDenied(kind)) => shared
                        .logger
                        .warn(format_args!("cannot reinstall the listener: no {kind}")),
                    Err(e) => shared
                        .logger
                        .warn(format_args!("cannot reinstall the listener: {e}")),
                }
            }
        }
    }
}
