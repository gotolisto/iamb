//! The fill-in form end to end (task 3.5): a real worker on the mock platform, in this process,
//! the real bridge, and the form's flow (`iamb_desktop::form`) with a recording panel in place
//! of the window. The panel's front end is played by the test: it submits or cancels as the
//! user would.
//!
//! On the mock a key always reaches the focused window's field, as on macOS a key typed into a
//! non-activating panel leaves the frontmost application the focused one: only `PanelFocus`
//! tells the service that the panel has the keyboard. So "the keys typed into the panel never
//! reach the engine" is checked by typing while the panel is shown and seeing nothing expand.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use common::{Endpoint, Service, T, id};
use iamb_desktop::bridge::{Bridge, BridgeConfig, BridgeSink};
use iamb_desktop::contract::FieldView;
use iamb_desktop::contract::{ConnectionState, StatusView, UiEvent};
use iamb_desktop::form::{
    FOCUS_LOST, FORM_BUSY, FormConfig, FormController, FormEvent, PanelHost, TARGET_GONE,
    TRIAL_TARGET,
};
use iamb_desktop::log::Logger;
use iamb_desktop::timing::wall_ms;
use iamb_ipc::{EventBody, RequestBody, ResponseBody, Role};
use iamb_platform::mock::MockWindow;
use iamb_platform::{FocusInspector, FocusToken, PeerPolicy};

/// What the panel was asked to do.
#[derive(Default)]
struct Seen {
    events: Vec<FormEvent>,
    shows: usize,
    hides: usize,
    visible: bool,
}

/// The panel's window, recorded.
#[derive(Default)]
struct Panel {
    seen: Mutex<Seen>,
    changed: Condvar,
}

impl PanelHost for Panel {
    fn send(&self, event: &FormEvent) {
        self.lock().events.push(event.clone());
        self.changed.notify_all();
    }

    fn show(&self, _height: Option<f64>) -> Result<(), String> {
        let mut seen = self.lock();
        seen.shows += 1;
        seen.visible = true;
        self.changed.notify_all();
        Ok(())
    }

    fn hide(&self) {
        let mut seen = self.lock();
        seen.hides += 1;
        seen.visible = false;
        self.changed.notify_all();
    }
}

impl Panel {
    fn lock(&self) -> std::sync::MutexGuard<'_, Seen> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits up to `T` until `pred` holds; returns what it saw.
    fn wait(&self, what: &str, pred: impl Fn(&Seen) -> bool) {
        let deadline = Instant::now() + T;
        let mut seen = self.lock();
        while !pred(&seen) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "the panel never got to: {what}");
            seen = self
                .changed
                .wait_timeout(seen, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Waits for the panel of a new form to be shown; returns its session.
    fn wait_shown(&self, after: usize) -> u64 {
        self.wait("shown", |s| {
            s.visible
                && s.events
                    .iter()
                    .skip(after)
                    .any(|e| matches!(e, FormEvent::Shown { .. }))
        });
        self.lock()
            .events
            .iter()
            .skip(after)
            .find_map(|e| match e {
                FormEvent::Shown { session } => Some(*session),
                _ => None,
            })
            .expect("a shown event")
    }

    fn events(&self) -> usize {
        self.lock().events.len()
    }

    fn visible(&self) -> bool {
        self.lock().visible
    }

    fn shows(&self) -> usize {
        self.lock().shows
    }
}

/// The bridge's sink in the app: service events go to the form (`app::Sink`).
#[derive(Default)]
struct Sink {
    form: OnceLock<Arc<FormController>>,
    /// Stamps each request as having arrived 3 s ago (a UI host that could not take it).
    late: AtomicBool,
    connected: Mutex<bool>,
    changed: Condvar,
}

impl BridgeSink for Sink {
    fn status(&self, view: &StatusView) {
        let connected = view.connection == ConnectionState::Connected;
        *self
            .connected
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = connected;
        self.changed.notify_all();
        if !connected && let Some(form) = self.form.get() {
            form.connection_lost();
        }
    }

    fn event(&self, _event: &UiEvent) {}

    fn service_event(&self, event: &EventBody) {
        if let Some(form) = self.form.get() {
            let (at, wall) = if self.late.load(Ordering::SeqCst) {
                let now = Instant::now();
                (
                    now.checked_sub(Duration::from_secs(3)).unwrap_or(now),
                    wall_ms() - 3000.0,
                )
            } else {
                (Instant::now(), wall_ms())
            };
            form.service_event_at(event, at, wall);
        }
    }
}

impl Sink {
    fn wait_connected(&self) {
        let deadline = Instant::now() + T;
        let mut connected = self
            .connected
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        while !*connected {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "the bridge did not connect");
            connected = self
                .changed
                .wait_timeout(connected, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// A UI host without windows: the bridge, the form's flow and the recorded panel.
struct Host {
    bridge: Bridge,
    form: Arc<FormController>,
    panel: Arc<Panel>,
    sink: Arc<Sink>,
}

impl Host {
    fn start(service: &Service) -> Self {
        Self::start_with(service, |_| {})
    }

    fn start_with(service: &Service, adjust: impl FnOnce(&mut FormConfig)) -> Self {
        let logger = Arc::new(Logger::open(&service.dirs().logs(), "desktop"));
        let mut cfg = BridgeConfig::new(
            service.client_endpoint(),
            service.dirs().service_endpoint(),
            Arc::clone(&logger),
        );
        cfg.policy = PeerPolicy::SameUser;
        cfg.backoff = vec![Duration::from_millis(50), Duration::from_millis(200)];
        let sink = Arc::new(Sink::default());
        let sink_dyn: Arc<dyn BridgeSink> = sink.clone();
        let bridge = Bridge::start(cfg, sink_dyn).expect("bridge starts");
        let panel = Arc::new(Panel::default());
        let focus: Arc<dyn FocusInspector> = Arc::new(service.os.clone());
        let mut form_cfg =
            FormConfig::new(Arc::new(bridge.clone()), panel.clone(), Some(focus), logger);
        form_cfg.layout_wait = Duration::ZERO;
        adjust(&mut form_cfg);
        let form = Arc::new(FormController::start(form_cfg).expect("form starts"));
        let _ = sink.form.set(Arc::clone(&form));
        sink.wait_connected();
        Self {
            bridge,
            form,
            panel,
            sink,
        }
    }

    /// The UI host goes away (its connection ends, as when the process is killed).
    fn stop(&self) {
        self.form.stop();
        self.bridge.stop();
    }
}

fn values(name: &str, value: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(name.to_owned(), value.to_owned())])
}

/// Waits up to `T` for the editor to read `want`.
fn wait_text(service: &Service, want: &str) {
    let deadline = Instant::now() + T;
    while service.text() != want {
        assert!(
            Instant::now() < deadline,
            "the editor reads {:?}, not {want:?}",
            service.text()
        );
        service.worker().settle();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Types `brb ` into `window` until it expands (up to `T`): the engine expands typed
/// abbreviations again, no form keeps them refused with `form_open`.
fn wait_expands(service: &Service, window: FocusToken) {
    let deadline = Instant::now() + T;
    loop {
        let before = service.os.text(window).expect("open");
        service.type_text("brb ");
        if service.os.text(window).expect("open") == format!("{before}be right back ") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "typed abbreviations stay refused ({} form_open)",
            diagnostic_codes(service)
                .iter()
                .filter(|c| *c == "form_open")
                .count()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Opens a mail window, types `hi ` into it and waits for its form; returns the window and
/// the session.
fn form_in_mail(service: &Service, host: &Host) -> (FocusToken, u64) {
    let mail = service
        .os
        .open_window(MockWindow::new("org.example.mail", 300));
    service.os.focus(mail).expect("open");
    service.worker().sync();
    service.type_text("hi ");
    (mail, host.panel.wait_shown(0))
}

/// "Fill in … again" (here from the editor) opens the form with `Ada`.
fn reopens_with_ada(host: &Host) {
    assert_eq!(host.form.last_submitted().as_deref(), Some("label hi"));
    let before = host.panel.events();
    host.form.reopen_last().expect("asked");
    let again = host.panel.wait_shown(before);
    let form = host.form.current().expect("open");
    assert_eq!(
        form.last_values.map(iamb_ipc::Secret::into_inner),
        Some(values("Name", "Ada")),
        "the answers were kept"
    );
    host.form.cancel(again).expect("cancelled");
}

fn diagnostic_codes(service: &Service) -> Vec<String> {
    let mut cli = service.client(Role::Ui);
    match cli
        .call(RequestBody::RecentDiagnostics {})
        .expect("diagnostics")
    {
        ResponseBody::Diagnostics { entries } => entries.into_iter().map(|e| e.code).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn a_form_is_acknowledged_shown_and_its_text_lands_in_the_target_window() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    service.type_text("hi ");
    let session = host.panel.wait_shown(0);

    // The panel was shown within the budget of its part (the paint is the window's, measured
    // by `--measure-form`); the arrival and the show are this process's clock.
    let timing = host
        .form
        .painted(session, wall_ms())
        .expect("the form was timed");
    eprintln!(
        "form: shown {:.2} ms after FormRequested arrived (FormAck, GetSnippet, PanelFocus to a real worker)",
        timing.shown_ms
    );
    assert!(timing.shown_ms < 150.0, "{timing:?}");

    // What the panel's front end fetches: the fields, the header, no remembered values yet.
    let form = host.form.current().expect("a form is open");
    assert_eq!(form.session, session);
    assert_eq!(form.snippet, id(3).to_string());
    assert_eq!(form.label, "label hi");
    assert_eq!(form.abbreviation, "hi");
    assert_eq!(form.fields.len(), 1);
    assert_eq!(form.fields[0].name, "Name");
    assert_eq!(form.last_values, None);
    assert_eq!(
        service.text(),
        "hi ",
        "nothing is deleted before the submit"
    );

    host.form
        .submit(session, values("Name", "Ada"))
        .expect("submitted");
    wait_text(&service, "Hello Ada! ");
    assert!(!host.panel.visible());
    assert!(host.form.current().is_none(), "the values are gone");
    assert_eq!(
        host.panel.lock().events,
        vec![
            FormEvent::Open { session },
            FormEvent::Shown { session },
            FormEvent::Closed { session },
        ]
    );
    // Acknowledged: the service did not abandon it. What is tested is that a deadline (the
    // acknowledgement's 2 s) passes without effect, which no event announces: only the clock
    // can say it has passed.
    std::thread::sleep(Duration::from_millis(2200));
    assert!(!diagnostic_codes(&service).contains(&"form_not_acknowledged".to_owned()));
    host.stop();
}

#[test]
fn cancel_changes_nothing() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    service.type_text("hi ");
    let session = host.panel.wait_shown(0);
    host.form.cancel(session).expect("cancelled");
    service.worker().settle();
    assert_eq!(service.text(), "hi ");
    assert!(!host.panel.visible());
    assert!(host.form.current().is_none());
    assert!(service.os.injected().is_empty(), "nothing was typed");
    // The session is closed at once: the next abbreviation opens a new form.
    let before = host.panel.events();
    service.type_text("hi ");
    let next = host.panel.wait_shown(before);
    assert_ne!(next, session);
    host.form.cancel(next).expect("cancelled");
    assert_eq!(
        host.form.cancel(next).map_err(|e| e.code),
        Err("no_form".to_owned())
    );
    host.stop();
}

#[test]
fn keys_typed_while_the_panel_has_the_keyboard_never_reach_the_engine() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    service.type_text("hi ");
    let session = host.panel.wait_shown(0);
    // Typed into the panel (on the mock they land in the editor's text, as the window behind
    // a non-activating panel stays the focused one).
    service.type_text("brb ");
    assert_eq!(service.text(), "hi brb ", "nothing expanded");
    assert_eq!(
        service.worker().handoff().len(),
        0,
        "nothing was handed off"
    );
    host.form.cancel(session).expect("cancelled");
    // The keyboard is the app's again: typing expands as before.
    service.type_text("brb ");
    wait_text(&service, "hi brb be right back ");
    host.stop();
}

#[test]
fn when_the_focus_cannot_be_given_back_nothing_is_sent_and_the_panel_returns() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    service.type_text("hi ");
    let session = host.panel.wait_shown(0);
    service.os.refuse_focus_restore(true);
    let err = host
        .form
        .submit(session, values("Name", "Ada"))
        .expect_err("the focus did not come back");
    assert_eq!(err.code, FOCUS_LOST);
    assert!(host.panel.visible(), "the panel is back");
    assert_eq!(host.panel.shows(), 2);
    service.worker().settle();
    assert_eq!(service.text(), "hi ", "nothing was sent");
    // Still the panel's keyboard.
    service.type_text("omw ");
    assert_eq!(service.text(), "hi omw ");
    // Insert again, once the window takes the focus.
    service.os.refuse_focus_restore(false);
    host.form
        .submit(session, values("Name", "Ada"))
        .expect("submitted");
    // The engine deletes as many characters as the abbreviation had: the user moved the caret
    // (ADR-0013 accepts this).
    wait_text(&service, "hi oHello Ada! ");
    host.stop();
}

#[test]
fn a_panel_that_loses_the_keyboard_gives_it_back_to_the_app_and_takes_it_again() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    service.type_text("hi ");
    let session = host.panel.wait_shown(0);
    let form_open = || {
        diagnostic_codes(&service)
            .iter()
            .filter(|c| *c == "form_open")
            .count()
    };
    // A click into the app behind: the panel stays, and the keys are the app's again. While a
    // form is open the engine expands nothing (flow 4: a second abbreviation does nothing), but
    // it sees the keys: the match is recorded as `form_open`.
    host.form.keyboard(false);
    let deadline = Instant::now() + T;
    loop {
        service.type_text("brb ");
        if form_open() > 0 {
            break;
        }
        assert!(Instant::now() < deadline, "the keyboard never went back");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(host.panel.visible(), "the panel stays open");
    // Back into the panel: its keys are dropped again, before the engine sees them. Wait for the
    // service to say it took the panel's hold in (its log), not for a time.
    let holds = service.log_count("worker", "panel has the keyboard");
    host.form.keyboard(true);
    service.wait_log_count("worker", "panel has the keyboard", holds);
    let seen = form_open();
    service.type_text("brb ");
    assert_eq!(form_open(), seen, "the engine did not see the keys");
    host.form.cancel(session).expect("cancelled");
    host.stop();
}

#[test]
fn the_lease_is_renewed_while_the_panel_is_open() {
    let service = Service::start_with_lease(Endpoint::Mock, Some(Duration::from_millis(600)));
    let host = Host::start_with(&service, |cfg| {
        cfg.lease_renewal = Duration::from_millis(150);
    });
    service.type_text("hi ");
    let session = host.panel.wait_shown(0);
    // Longer than twice the lease (600 ms): what is tested is that the hold outlives it
    // because it is renewed, so the clock is the subject, not a wait for an event.
    std::thread::sleep(Duration::from_millis(1500));
    service.type_text("brb ");
    assert_eq!(service.text(), "hi brb ", "still the panel's keyboard");
    assert!(!diagnostic_codes(&service).contains(&"panel_focus_expired".to_owned()));
    host.form.cancel(session).expect("cancelled");
    host.stop();
}

#[test]
fn a_ui_host_killed_with_the_form_open_recovers() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    service.type_text("hi ");
    host.panel.wait_shown(0);
    // The UI host dies with the form open and the keyboard held.
    host.stop();
    let deadline = Instant::now() + T;
    while !diagnostic_codes(&service).contains(&"form_ui_host_gone".to_owned()) {
        assert!(Instant::now() < deadline, "the service did not notice");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Typing expands at once (the hold ended with the connection).
    service.type_text("brb ");
    wait_text(&service, "hi be right back ");
    // The next UI host gets the next form.
    let host = Host::start(&service);
    service.type_text("hi ");
    let session = host.panel.wait_shown(0);
    host.form
        .submit(session, values("Name", "Ada"))
        .expect("submitted");
    wait_text(&service, "hi be right back Hello Ada! ");
    host.stop();
}

#[test]
fn a_request_the_ui_host_took_too_late_is_cancelled_and_never_shown() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    host.sink.late.store(true, Ordering::SeqCst);
    service.type_text("hi ");
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        assert_eq!(host.panel.shows(), 0, "a late form is never shown");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Cancelled, not left to the service's 2 s: the next abbreviation opens a form at once.
    host.sink.late.store(false, Ordering::SeqCst);
    let before = host.panel.events();
    service.type_text("hi ");
    let session = host.panel.wait_shown(before);
    assert!(!diagnostic_codes(&service).contains(&"form_open".to_owned()));
    host.form.cancel(session).expect("cancelled");
    host.stop();
}

#[test]
fn the_last_form_reopens_with_its_answers() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    service.type_text("hi ");
    let session = host.panel.wait_shown(0);
    host.form
        .submit(session, values("Name", "Ada"))
        .expect("submitted");
    wait_text(&service, "Hello Ada! ");
    let before = host.panel.events();
    host.form.reopen_last().expect("asked");
    let again = host.panel.wait_shown(before);
    let form = host.form.current().expect("open");
    assert_eq!(form.session, again);
    assert_eq!(
        form.last_values.map(iamb_ipc::Secret::into_inner),
        Some(values("Name", "Ada"))
    );
    host.form.cancel(again).expect("cancelled");
    assert_eq!(
        service.text(),
        "Hello Ada! ",
        "a cancelled reopen types nothing"
    );
    host.stop();
}

#[test]
fn the_service_going_away_closes_the_form() {
    let mut service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    service.type_text("hi ");
    let session = host.panel.wait_shown(0);
    service.stop_worker();
    host.panel.wait("closed", |s| {
        !s.visible && s.events.contains(&FormEvent::Closed { session })
    });
    assert!(host.form.current().is_none());
    host.stop();
}

#[test]
fn form_values_reach_no_log_through_the_form_flow() {
    const CANARY: &str = "zqxformflowcanary";
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    service.type_text("hi ");
    let session = host.panel.wait_shown(0);
    host.form
        .submit(session, values("Name", CANARY))
        .expect("submitted");
    wait_text(&service, &format!("Hello {CANARY}! "));
    // The remembered values travel back to the UI host in the reopened form, and are refused
    // once by a focus that would not come back.
    let before = host.panel.events();
    host.form.reopen_last().expect("asked");
    let again = host.panel.wait_shown(before);
    service.os.refuse_focus_restore(true);
    let err = host
        .form
        .submit(again, values("Name", CANARY))
        .expect_err("refused");
    assert_eq!(err.code, FOCUS_LOST);
    service.os.refuse_focus_restore(false);
    host.form.cancel(again).expect("cancelled");
    assert!(
        !format!("{:?}", host.form).contains(CANARY),
        "nothing is held once the form closed"
    );
    host.stop();
    for entry in std::fs::read_dir(service.dirs().logs()).expect("logs") {
        let path = entry.expect("entry").path();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            !text.contains(CANARY),
            "{} holds a form value",
            path.display()
        );
    }
}

#[test]
fn a_form_whose_app_quits_while_it_stepped_aside_ends_and_keeps_its_answers() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    let (mail, session) = form_in_mail(&service, &host);
    // The user switches to the editor: the panel loses the keyboard and steps aside.
    service.os.focus(service.editor).expect("open");
    host.form.keyboard(false);
    host.panel.wait("stepped aside", |s| !s.visible);
    // The mail app quits meanwhile.
    service.os.close_window(mail);
    host.panel.wait("asked for the answers", |s| {
        s.events.contains(&FormEvent::TargetGone { session })
    });
    // The front end hands in what was typed so far.
    host.form
        .abandon(session, values("Name", "Ada"))
        .expect("abandoned");
    host.panel.wait("closed", |s| {
        s.events.contains(&FormEvent::Closed { session })
    });
    assert!(!host.panel.visible());
    assert!(host.form.current().is_none());
    wait_expands(&service, service.editor);
    assert_eq!(service.text(), "be right back ", "nothing else was typed");
    assert!(diagnostic_codes(&service).contains(&"form_target_gone".to_owned()));
    reopens_with_ada(&host);
    host.stop();
}

#[test]
fn a_form_whose_window_is_gone_ends_even_without_its_answers() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    let (mail, session) = form_in_mail(&service, &host);
    service.os.focus(service.editor).expect("open");
    host.form.keyboard(false);
    host.panel.wait("stepped aside", |s| !s.visible);
    service.os.close_window(mail);
    // The front end never answers (its page is gone): the form is cancelled.
    host.panel.wait("closed", |s| {
        s.events.contains(&FormEvent::Closed { session })
    });
    assert_eq!(
        host.form
            .abandon(session, values("Name", "Ada"))
            .map_err(|e| e.code),
        Err("no_form".to_owned()),
        "too late"
    );
    wait_expands(&service, service.editor);
    host.stop();
}

#[test]
fn insert_for_a_window_that_is_gone_ends_the_form_and_keeps_its_answers() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    let (mail, session) = form_in_mail(&service, &host);
    // The mail window closes while the user is in the form.
    service.os.close_window(mail);
    let err = host
        .form
        .submit(session, values("Name", "Ada"))
        .expect_err("nowhere to type");
    assert_eq!(err.code, TARGET_GONE);
    assert!(!host.panel.visible(), "the panel does not come back");
    assert!(host.form.current().is_none());
    service.os.focus(service.editor).expect("open");
    wait_expands(&service, service.editor);
    reopens_with_ada(&host);
    host.stop();
}

#[test]
fn the_panel_closed_by_the_system_cancels_the_form() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    service.type_text("hi ");
    let session = host.panel.wait_shown(0);
    // Alt+F4, or the window manager's close command, on the panel.
    host.form.close_requested();
    host.panel.wait("closed", |s| {
        !s.visible && s.events.contains(&FormEvent::Closed { session })
    });
    assert!(host.form.current().is_none());
    wait_expands(&service, service.editor);
    assert_eq!(
        service.text(),
        "hi be right back ",
        "cancel changed nothing"
    );
    host.stop();
}

#[test]
fn the_form_s_lease_is_renewed_while_it_waits() {
    let mut service = Service::start(Endpoint::Mock);
    service.form_lease = Some(Duration::from_millis(600));
    service.restart();
    let host = Host::start_with(&service, |cfg| {
        cfg.lease_renewal = Duration::from_millis(150);
    });
    service.type_text("hi ");
    let session = host.panel.wait_shown(0);
    // The user clicks into the app and leaves the form waiting, longer than the lease.
    host.form.keyboard(false);
    // Longer than twice the form's lease (600 ms): the clock is what is tested (see above).
    std::thread::sleep(Duration::from_millis(1500));
    service.type_text("brb ");
    assert_eq!(service.text(), "hi brb ", "the form is still open");
    assert!(!diagnostic_codes(&service).contains(&"form_expired".to_owned()));
    host.form.cancel(session).expect("cancelled");
    host.stop();
}

#[test]
fn try_the_form_shows_the_panel_and_hands_the_answers_to_the_preview() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    let fields = vec![FieldView {
        name: "Name".to_owned(),
        kind: "text".to_owned(),
        options: Vec::new(),
        default: String::new(),
    }];
    let tried = {
        let form = Arc::clone(&host.form);
        let fields = fields.clone();
        std::thread::spawn(move || form.try_form(fields, "Greeting".to_owned(), "hi".to_owned()))
    };
    let session = host.panel.wait_shown(0);
    let open = host.form.current().expect("open");
    assert_eq!(open.label, "Greeting");
    assert_eq!(
        open.app.as_deref(),
        Some(TRIAL_TARGET),
        "goes into the preview"
    );
    assert_eq!(open.fields, fields);
    // While it is open, another try is refused and the service's forms are not blocked by it.
    assert_eq!(
        host.form
            .try_form(fields.clone(), String::new(), String::new())
            .map_err(|e| e.code),
        Err(FORM_BUSY.to_owned())
    );
    host.form
        .submit(session, values("Name", "Ada"))
        .expect("inserted");
    assert_eq!(
        tried.join().expect("joined").expect("answered"),
        Some(values("Name", "Ada"))
    );
    assert!(!host.panel.visible());
    assert!(host.form.current().is_none());
    service.worker().settle();
    assert_eq!(service.text(), "", "typed nowhere");
    assert!(service.os.injected().is_empty());
    // Cancelled: no answers.
    let tried = {
        let form = Arc::clone(&host.form);
        std::thread::spawn(move || form.try_form(fields, String::new(), "hi".to_owned()))
    };
    let before = host.panel.events();
    let session = host.panel.wait_shown(before);
    host.form.cancel(session).expect("cancelled");
    assert_eq!(tried.join().expect("joined").expect("answered"), None);
    // The service's forms work as before.
    let before = host.panel.events();
    service.type_text("hi ");
    let real = host.panel.wait_shown(before);
    host.form
        .submit(real, values("Name", "Kim"))
        .expect("submitted");
    wait_text(&service, "Hello Kim! ");
    host.stop();
}

#[test]
fn the_form_offered_again_follows_the_service() {
    let mut service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    let submit_one = |host: &Host, service: &Service| {
        let before = host.panel.events();
        service.type_text("hi ");
        let session = host.panel.wait_shown(before);
        host.form
            .submit(session, values("Name", "Ada"))
            .expect("submitted");
        assert_eq!(host.form.last_submitted().as_deref(), Some("label hi"));
    };
    let wait_forgotten = |host: &Host| {
        let deadline = Instant::now() + T;
        while host.form.last_submitted().is_some() {
            assert!(Instant::now() < deadline, "still offered again");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    // The screen locks: the service forgets the answers, and says so to the next reopen; the
    // row is not offered any more.
    submit_one(&host, &service);
    assert!(
        service
            .os
            .session_event(iamb_platform::SessionEvent::Locked)
    );
    service.worker().settle();
    host.form.reopen_last().expect("asked");
    wait_forgotten(&host);
    // The service restarts: its answers are gone with it.
    submit_one(&host, &service);
    service.restart();
    wait_forgotten(&host);
    host.stop();
}
