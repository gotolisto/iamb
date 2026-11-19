//! Keys and focus changes that overtake each other (the 2.8 review's findings 5 and 18; the
//! follow-up review's N1 and N5).
//!
//! A key reaches the engine thread straight from the hook, a focus change through the OS
//! watcher and the focus thread. The other service tests wait for the engine after every focus
//! change; these do not. The mock holds its focus reports back
//! ([`hold_focus_reports`](iamb_platform::mock::MockPlatform::hold_focus_reports)) the way a
//! real watcher delivers them late, and scripts a focus change or a pause between a trigger and
//! the injector's start marker
//! ([`script_at`](iamb_platform::mock::MockPlatform::script_at)).

mod common;

use std::sync::atomic::Ordering;
use std::sync::mpsc::channel;
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use common::{Harness, LIBRARY, T, diagnostic_codes};
use iamb_ipc::{RequestBody, ResponseBody, Role};
use iamb_platform::mock::{Injected, InjectionPoint, MockWindow};
use iamb_platform::{CancelReason, FocusInspector, FocusToken, InjectionOutcome};

fn bank(h: &Harness) -> FocusToken {
    h.os.open_window(MockWindow::new("org.example.bank", 200).secure())
}

/// Nothing was deleted or typed by the injector.
fn nothing_injected(h: &Harness) -> bool {
    !h.os.injected().iter().any(|i| {
        matches!(
            i,
            Injected::DeleteBack(_) | Injected::TypeText(_) | Injected::Retype(_)
        )
    })
}

#[test]
fn a_key_typed_before_the_focus_change_is_reported_is_not_matched() {
    let h = Harness::start(LIBRARY);
    let bank = bank(&h);
    h.os.type_text("brb");
    h.settle();

    // The password field takes the focus; the watcher has not told the service yet when the
    // next key arrives there.
    h.os.hold_focus_reports(true);
    h.os.focus(bank).expect("window is open");
    h.os.type_text(" ");
    h.settle();
    assert_eq!(h.text(bank), " ", "nothing expanded in the password field");
    assert_eq!(h.text(h.editor), "brb");
    assert!(nothing_injected(&h), "{:?}", h.os.injected());

    // The report arrives: the field is judged secure and stays untouched.
    h.os.hold_focus_reports(false);
    h.settle();
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(bank), " brb ");
    assert!(nothing_injected(&h));
}

#[test]
fn keys_that_went_elsewhere_leave_the_previous_field_as_it_was() {
    let h = Harness::start(LIBRARY);
    let bank = bank(&h);
    h.os.type_text("br");
    h.settle();
    h.os.hold_focus_reports(true);
    // A few keys go to the password field and the focus comes back, all before the watcher
    // reports anything: those keys are not part of the editor's text, and the abbreviation
    // the user goes on typing in the editor still expands there.
    h.os.focus(bank).expect("window is open");
    h.os.type_text("x b");
    h.os.focus(h.editor).expect("window is open");
    h.os.type_text("b ");
    h.settle();
    assert_eq!(h.text(bank), "x b");
    assert_eq!(h.text(h.editor), "be right back ");
    h.os.hold_focus_reports(false);
    h.settle();
}

/// Starts `brb ` in the editor and runs `between` on the test thread while the injector waits
/// right before its start marker; returns the injector's log afterwards.
fn between_trigger_and_start_marker(h: &Harness, between: impl FnOnce()) -> Vec<Injected> {
    let (at, arrived) = channel();
    let (go, wait) = channel::<()>();
    h.os.script_at(InjectionPoint::BeforeStartMarker, move |_| {
        let _ = at.send(());
        let _ = wait.recv_timeout(T);
    });
    h.os.type_text("brb ");
    arrived
        .recv_timeout(T)
        .expect("the injector reached its start marker");
    between();
    // Everything sent so far, focus changes included, has reached the engine.
    h.worker().sync();
    go.send(()).expect("the injector waits");
    h.settle();
    h.os.injected()
}

fn cancelled_as_disturbed(log: &[Injected]) -> bool {
    log.iter()
        .any(|i| matches!(i, Injected::Cancelled(_, CancelReason::Disturbed)))
}

/// The service's own cancel. The focus comes back before the injector looks, so its start
/// check (N1) passes and only the hand-off can stop the expansion.
#[test]
fn a_focus_change_before_the_start_marker_cancels_the_expansion() {
    let h = Harness::start(LIBRARY);
    let other = h.os.open_window(MockWindow::new("org.example.chat", 300));
    let log = between_trigger_and_start_marker(&h, || {
        h.os.focus(other).expect("window is open");
        h.os.focus(h.editor).expect("window is open");
    });
    assert!(cancelled_as_disturbed(&log), "{log:?}");
    assert_eq!(
        h.text(other),
        "",
        "nothing typed into the window that took the focus"
    );
    assert_eq!(h.text(h.editor), "brb ");
}

#[test]
fn a_pause_before_the_start_marker_cancels_the_expansion() {
    let h = Harness::start(LIBRARY);
    let mut c = h.client(Role::Cli);
    let log = between_trigger_and_start_marker(&h, || {
        c.call(RequestBody::SetPaused { paused: true })
            .expect("paused");
    });
    assert!(cancelled_as_disturbed(&log), "{log:?}");
    assert_eq!(h.text(h.editor), "brb ");
}

#[test]
fn a_key_typed_into_another_window_before_the_start_marker_cancels_the_expansion() {
    let h = Harness::start(LIBRARY);
    let bank = bank(&h);
    let log = between_trigger_and_start_marker(&h, || {
        h.os.hold_focus_reports(true);
        h.os.focus(bank).expect("window is open");
        h.os.type_text("x");
        // Back before the injector looks: only the key's stamp can cancel (as above).
        h.os.focus(h.editor).expect("window is open");
    });
    assert!(cancelled_as_disturbed(&log), "{log:?}");
    assert_eq!(
        h.text(bank),
        "x",
        "nothing deleted or typed in the password field"
    );
    assert_eq!(h.text(h.editor), "brb ");
    h.os.hold_focus_reports(false);
    h.settle();
}

/// Follow-up review N1: a window takes the focus between the trigger key and the injector's
/// first look, and the watcher has not told the service yet. The request names the
/// trigger's window, so the injector does not start: nothing deleted or typed, no marker.
#[test]
fn a_window_that_takes_the_focus_before_the_injector_looks_gets_nothing() {
    let h = Harness::start(LIBRARY);
    let bank = h.os.open_window(
        MockWindow::new("org.example.bank", 200)
            .secure()
            .text("hunter22"),
    );
    h.os.hold_focus_reports(true);
    h.os.script_at(InjectionPoint::BeforeStartMarker, move |os| {
        os.focus(bank).expect("window is open");
    });
    h.os.type_text("brb ");
    h.settle();
    let log = h.os.injected();
    assert_eq!(h.text(bank), "hunter22", "{log:?}");
    assert_eq!(h.text(h.editor), "brb ");
    assert_eq!(
        log,
        [Injected::Cancelled(
            iamb_platform::core_types::ExpansionId::from_raw(1),
            CancelReason::FocusMoved
        )],
        "no marker, no step"
    );
    let mut c = h.client(Role::Ui);
    assert!(diagnostic_codes(&mut c).contains(&"injection_focus_moved".to_owned()));
    h.os.hold_focus_reports(false);
    h.settle();
}

/// Follow-up review N5: the watcher never reports that another window took the focus. The
/// keys typed there are not matched, but after a few of them the service says so and reads
/// the focus itself, and expansion works in that window again.
#[test]
fn keys_for_a_window_the_watcher_never_reported_bring_a_diagnostic_and_a_new_look() {
    let h = Harness::start(LIBRARY);
    let mut c = h.client(Role::Ui);
    let notes = h.os.open_window(MockWindow::new("org.example.notes", 300));
    h.os.hold_focus_reports(true);
    h.os.focus(notes).expect("window is open");
    h.os.type_text("brb ");
    h.settle();
    h.worker().sync();
    assert!(
        diagnostic_codes(&mut c).contains(&"keys_for_unreported_window".to_owned()),
        "a diagnostic, without anything typed"
    );
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(notes), "brb be right back ");
    assert!(
        !h.os
            .injected()
            .iter()
            .any(|i| matches!(i, Injected::Cancelled(..))),
        "{:?}",
        h.os.injected()
    );
    h.os.hold_focus_reports(false);
    h.settle();
}

/// Follow-up review N5 and finding 5 on macOS: a listener whose stamp names another process
/// than the inspector (a key window that belongs to a helper process, a field that holds
/// something else than expected). Its keys are dropped while that may be a late report; once
/// the inspector has said for half a second that the focus did not move, they are handled as
/// keys without a target, as before stamps existed, so expansion does not stop there for good.
#[test]
fn a_stamp_the_inspector_keeps_contradicting_stops_being_trusted() {
    let (h, wrapper) = wrapped_harness();
    let mut c = h.client(Role::Ui);
    *wrapper
        .restamp
        .lock()
        .unwrap_or_else(PoisonError::into_inner) =
        Some(iamb_platform::KeyTarget::process(4_000_000));
    h.os.type_text("brb ");
    h.settle();
    h.worker().sync();
    assert!(nothing_injected(&h), "{:?}", h.os.injected());
    assert!(diagnostic_codes(&mut c).contains(&"keys_for_unreported_window".to_owned()));
    std::thread::sleep(iamb_service::worker::RESYNC_AFTER + Duration::from_millis(50));
    h.os.type_text("x");
    h.settle();
    h.worker().sync();
    h.os.type_text(" brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "brb x be right back ");
}

/// The mock with a few behaviours a test can change: an inspection that waits until the test
/// lets it go on (the focus thread has then sent `FocusChanged` and the verdict is not there
/// yet; a real inspection may take up to 100 ms), an injector outcome forced on the next
/// expansion, a focus watcher that cannot be started, and key stamps replaced by another.
mod wrapped {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{Receiver, Sender};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Duration;

    use iamb_platform::core_types::Sensitive;
    use iamb_platform::mock::MockPlatform;
    use iamb_platform::{
        AppIdentity, Chord, ClipboardAccess, ClipboardContent, ClipboardSnapshot, EndpointName,
        EventSink, FieldSecurity, FocusInspector, FocusSink, FocusToken, HotkeyId, Hotkeys,
        InjectionOutcome, InjectionRequest, Injector, KeyListener, KeyTarget, ListenerEvent,
        LocalEndpoint, LocalListener, LocalStream, LoginItem, LoginItemStatus, MemoryLock,
        MemoryRegion, PeerPolicy, PermissionKind, PermissionStatus, Permissions, PlatformError,
        Result, TypedHandoff,
    };

    pub struct Wrapped {
        pub os: MockPlatform,
        /// When set: the next inspection says so on the sender and waits for a message on the
        /// receiver.
        pub gate: Mutex<Option<(Sender<()>, Receiver<()>)>>,
        /// When set: the next expansion is not sent, and this is its outcome.
        pub outcome: Mutex<Option<InjectionOutcome>>,
        /// While true, starting the focus watcher fails.
        pub refuse_watch: AtomicBool,
        /// When set: every stamped key is delivered with this target instead.
        pub restamp: Arc<Mutex<Option<KeyTarget>>>,
    }

    /// The listener's sink with the stamps of [`Wrapped::restamp`].
    struct Restamp {
        inner: Box<dyn EventSink>,
        to: Arc<Mutex<Option<KeyTarget>>>,
    }

    impl EventSink for Restamp {
        fn deliver(&mut self, event: ListenerEvent) {
            self.inner.deliver(event);
        }
        fn deliver_to(&mut self, event: ListenerEvent, target: KeyTarget) {
            let target =
                (*self.to.lock().unwrap_or_else(PoisonError::into_inner)).unwrap_or(target);
            self.inner.deliver_to(event, target);
        }
    }

    pub fn around(os: &MockPlatform) -> Arc<Wrapped> {
        Arc::new(Wrapped {
            os: os.clone(),
            gate: Mutex::new(None),
            outcome: Mutex::new(None),
            refuse_watch: AtomicBool::new(false),
            restamp: Arc::new(Mutex::new(None)),
        })
    }

    fn take<T>(slot: &Mutex<Option<T>>) -> Option<T> {
        slot.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    impl FocusInspector for Wrapped {
        fn focused_app(&self) -> Result<AppIdentity> {
            self.os.focused_app()
        }
        fn secure_state(&self, target: &FocusToken, timeout: Duration) -> FieldSecurity {
            if let Some((inspecting, release)) = take(&self.gate) {
                let _ = inspecting.send(());
                let _ = release.recv_timeout(super::T);
            }
            self.os.secure_state(target, timeout)
        }
        fn focus_token(&self) -> Result<FocusToken> {
            self.os.focus_token()
        }
        fn restore_focus(&self, token: &FocusToken) -> Result<()> {
            self.os.restore_focus(token)
        }
        fn allow_foreground(&self, pid: u32) -> Result<()> {
            self.os.allow_foreground(pid)
        }
        fn watch(&self, sink: Box<dyn FocusSink>) -> Result<()> {
            if self.refuse_watch.load(Ordering::Relaxed) {
                return Err(PlatformError::os(1, "test: the watcher cannot start"));
            }
            self.os.watch(sink)
        }
        fn unwatch(&self) -> Result<()> {
            self.os.unwatch()
        }
        fn is_watching(&self) -> bool {
            self.os.is_watching()
        }
    }

    impl KeyListener for Wrapped {
        fn start(&self, sink: Box<dyn EventSink>) -> Result<()> {
            self.os.start(Box::new(Restamp {
                inner: sink,
                to: Arc::clone(&self.restamp),
            }))
        }
        fn stop(&self) -> Result<()> {
            self.os.stop()
        }
        fn self_test(&self, timeout: Duration) -> Result<()> {
            self.os.self_test(timeout)
        }
    }

    impl Injector for Wrapped {
        fn inject(&self, request: &InjectionRequest, typed: &TypedHandoff) -> InjectionOutcome {
            match take(&self.outcome) {
                Some(outcome) => outcome,
                None => self.os.inject(request, typed),
            }
        }
        fn configure(&self, settings_json: &str) -> Result<()> {
            self.os.configure(settings_json)
        }
    }

    impl ClipboardAccess for Wrapped {
        fn read_text(&self) -> Result<Option<Sensitive<String>>> {
            self.os.read_text()
        }
        fn snapshot(&self) -> Result<ClipboardSnapshot> {
            self.os.snapshot()
        }
        fn write(&self, content: &ClipboardContent, transient: bool) -> Result<()> {
            self.os.write(content, transient)
        }
        fn restore(&self, snapshot: ClipboardSnapshot) -> Result<()> {
            self.os.restore(snapshot)
        }
    }

    impl Hotkeys for Wrapped {
        fn register(&self, id: HotkeyId, chord: Chord) -> Result<()> {
            self.os.register(id, chord)
        }
        fn unregister(&self, id: HotkeyId) -> Result<()> {
            self.os.unregister(id)
        }
    }

    impl Permissions for Wrapped {
        fn permission_status(&self) -> PermissionStatus {
            self.os.permission_status()
        }
        fn open_settings(&self, kind: PermissionKind) -> Result<()> {
            self.os.open_settings(kind)
        }
    }

    impl LoginItem for Wrapped {
        fn enable(&self) -> Result<()> {
            self.os.enable()
        }
        fn disable(&self) -> Result<()> {
            self.os.disable()
        }
        fn login_item_status(&self) -> Result<LoginItemStatus> {
            self.os.login_item_status()
        }
    }

    impl MemoryLock for Wrapped {
        fn lock(&self, region: MemoryRegion) -> Result<()> {
            self.os.lock(region)
        }
        fn unlock(&self, region: MemoryRegion) -> Result<()> {
            self.os.unlock(region)
        }
    }

    impl LocalEndpoint for Wrapped {
        fn bind(&self, name: &EndpointName, policy: PeerPolicy) -> Result<Box<dyn LocalListener>> {
            self.os.bind(name, policy)
        }
        fn connect(
            &self,
            name: &EndpointName,
            policy: PeerPolicy,
            timeout: Duration,
        ) -> Result<Box<dyn LocalStream>> {
            self.os.connect(name, policy, timeout)
        }
    }
}

/// A harness whose worker runs on [`wrapped::Wrapped`], with a fast heartbeat.
fn wrapped_harness() -> (Harness, Arc<wrapped::Wrapped>) {
    let mut wrapper = None;
    let h = Harness::start_wrapped(
        LIBRARY,
        |os| {
            let w = wrapped::around(os);
            wrapper = Some(Arc::clone(&w));
            w as Arc<dyn iamb_platform::Platform>
        },
        |c| c.heartbeat_interval = Duration::from_millis(10),
    );
    (h, wrapper.expect("wrapped"))
}

/// Waits until `done` holds.
fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + T;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_focus_change_cancels_before_its_verdict_is_known() {
    let (h, wrapper) = wrapped_harness();
    let other = h.os.open_window(MockWindow::new("org.example.chat", 300));
    let (inspecting, inspected) = channel();
    let (release, released) = channel();
    *wrapper.gate.lock().unwrap_or_else(PoisonError::into_inner) = Some((inspecting, released));

    let (at, arrived) = channel();
    let (go, wait) = channel::<()>();
    h.os.script_at(InjectionPoint::BeforeStartMarker, move |_| {
        let _ = at.send(());
        let _ = wait.recv_timeout(T);
    });
    h.os.type_text("brb ");
    arrived
        .recv_timeout(T)
        .expect("the injector reached its start marker");
    h.os.focus(other).expect("window is open");
    // The focus thread has sent `FocusChanged` and waits for the inspection; a status request
    // goes to the engine behind it.
    inspected.recv_timeout(T).expect("inspecting");
    // The focus comes back before the injector looks, so its start check (N1) passes and
    // only the service's cancel can stop the expansion.
    h.os.focus(h.editor).expect("window is open");
    let mut c = h.client(Role::Cli);
    assert!(matches!(
        c.call(RequestBody::GetStatus {}),
        Ok(ResponseBody::Status { .. })
    ));
    go.send(()).expect("the injector waits");
    wait_until("the injection ends", || {
        h.os.injected()
            .iter()
            .any(|i| matches!(i, Injected::Cancelled(..) | Injected::EndMarker(_)))
    });
    let log = h.os.injected();
    let _ = release.send(());
    h.settle();
    assert!(cancelled_as_disturbed(&log), "{log:?}");
    assert_eq!(h.text(other), "");
    assert_eq!(h.text(h.editor), "brb ");
}

#[test]
fn a_focus_move_after_the_start_marker_has_its_own_diagnostic() {
    let h = Harness::start(LIBRARY);
    let other = h.os.open_window(MockWindow::new("org.example.chat", 300));
    h.os.script_at(InjectionPoint::AfterStartMarker, move |os| {
        os.focus(other).expect("window is open");
    });
    h.os.type_text("brb ");
    h.settle();
    assert!(
        h.os.injected()
            .iter()
            .any(|i| matches!(i, Injected::Cancelled(_, CancelReason::FocusMoved))),
        "{:?}",
        h.os.injected()
    );
    let mut c = h.client(Role::Ui);
    let codes = diagnostic_codes(&mut c);
    assert!(
        codes.contains(&"injection_focus_moved".to_owned()),
        "{codes:?}"
    );
    assert!(
        !codes.contains(&"injection_cancelled".to_owned()),
        "{codes:?}"
    );
}

#[test]
fn a_clipboard_that_cannot_be_restored_has_its_own_diagnostic() {
    let (h, wrapper) = wrapped_harness();
    *wrapper
        .outcome
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(InjectionOutcome::Cancelled(
        CancelReason::ClipboardNotRestorable,
    ));
    h.os.type_text("brb ");
    h.settle();
    let mut c = h.client(Role::Ui);
    let codes = diagnostic_codes(&mut c);
    assert!(
        codes.contains(&"injection_clipboard_not_restorable".to_owned()),
        "{codes:?}"
    );
    assert!(
        !codes.contains(&"injection_cancelled".to_owned()),
        "{codes:?}"
    );
    // Any other reason keeps the general code.
    *wrapper
        .outcome
        .lock()
        .unwrap_or_else(PoisonError::into_inner) =
        Some(InjectionOutcome::Cancelled(CancelReason::Boundary));
    h.os.type_text(" omw ");
    h.settle();
    assert!(diagnostic_codes(&mut c).contains(&"injection_cancelled".to_owned()));
}

#[test]
fn a_stopped_focus_watcher_suspends_until_it_runs_again() {
    let (h, wrapper) = wrapped_harness();
    let mut c = h.client(Role::Ui);
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(h.text(h.editor), "be right back ", "watching: expands");

    // The watcher's thread ends by itself, and cannot be started again for now.
    wrapper.refuse_watch.store(true, Ordering::Relaxed);
    h.os.unwatch().expect("stops");
    wait_until("the engine notices", || {
        diagnostic_codes(&mut c).contains(&"suspended_unknown_focus".to_owned())
    });
    h.os.type_text("brb ");
    h.settle();
    assert_eq!(
        h.text(h.editor),
        "be right back brb ",
        "unknown focus: nothing expands"
    );
    // A verdict that still comes for the old focus (an exclusion change judges it again) does
    // not lift the suspension.
    c.call(RequestBody::PutExclusion {
        app_id: "org.example.other".to_owned(),
        display_name: String::new(),
        excluded: true,
    })
    .expect("stored");
    h.worker().sync();
    h.os.type_text("omw ");
    h.settle();
    assert_eq!(h.text(h.editor), "be right back brb omw ");

    // Once it can, the worker starts it again; its first report brings a verdict.
    wrapper.refuse_watch.store(false, Ordering::Relaxed);
    wait_until("the watcher runs again", || h.os.is_watching());
    h.worker().sync();
    h.os.click();
    h.os.type_text("omw ");
    h.settle();
    assert_eq!(h.text(h.editor), "be right back brb omw on my way ");
}
