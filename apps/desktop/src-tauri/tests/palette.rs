//! The search palette end to end (task 3.6): a real worker on the mock platform, in this process,
//! the real bridge, the palette's flow (`iamb_desktop::palette`) and the fill-in form's, with
//! recording panels in place of the windows. The panels' front ends are played by the test: it
//! searches, picks or closes as the user would.
//!
//! On the mock a key always reaches the focused window's field, as on macOS a key typed into a
//! non-activating panel leaves the frontmost application the focused one: only `PanelFocus`
//! tells the service that a panel has the keyboard. So "the keys typed into the panel never reach
//! the engine" is checked by typing while the panel is shown and seeing nothing expand.

mod common;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use common::{Endpoint, Service, T, id};
use iamb_desktop::bridge::{Bridge, BridgeConfig, BridgeSink};
use iamb_desktop::contract::{ConnectionState, StatusView, UiEvent};
use iamb_desktop::form::{FormConfig, FormController, FormEvent, PanelHost};
use iamb_desktop::log::Logger;
use iamb_desktop::notice::{Notice, NoticeHost, Notices};
use iamb_desktop::palette::{
    LibraryRequest, NO_PALETTE, PaletteConfig, PaletteController, PaletteEvent, PaletteHost, Pick,
};
use iamb_desktop::palette_data::PaletteData;
use iamb_desktop::timing::wall_ms;
use iamb_ipc::{EventBody, RequestBody, ResponseBody, Role};
use iamb_platform::mock::MockWindow;
use iamb_platform::{Chord, FocusInspector, PeerPolicy};

const CHORD: &str = "Ctrl+Shift+Space";

/// What a panel was asked to do.
#[derive(Default)]
struct Seen<E> {
    events: Vec<E>,
    shows: usize,
    hides: usize,
    visible: bool,
    library: Vec<LibraryRequest>,
}

/// A panel's window, recorded.
struct Recorded<E> {
    seen: Mutex<Seen<E>>,
    changed: Condvar,
}

impl<E> Default for Recorded<E> {
    fn default() -> Self {
        Self {
            seen: Mutex::new(Seen {
                events: Vec::new(),
                shows: 0,
                hides: 0,
                visible: false,
                library: Vec::new(),
            }),
            changed: Condvar::new(),
        }
    }
}

impl<E: PartialEq> Recorded<E> {
    fn lock(&self) -> std::sync::MutexGuard<'_, Seen<E>> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn edit(&self, f: impl FnOnce(&mut Seen<E>)) {
        f(&mut self.lock());
        self.changed.notify_all();
    }

    /// Waits up to `T` until `pred` holds.
    fn wait(&self, what: &str, pred: impl Fn(&Seen<E>) -> bool) {
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

    fn visible(&self) -> bool {
        self.lock().visible
    }

    fn shows(&self) -> usize {
        self.lock().shows
    }

    fn events(&self) -> usize {
        self.lock().events.len()
    }
}

type PalettePanel = Recorded<PaletteEvent>;
type FormPanel = Recorded<FormEvent>;

impl PaletteHost for PalettePanel {
    fn send(&self, event: &PaletteEvent) {
        self.edit(|s| s.events.push(event.clone()));
    }

    fn show(&self) -> Result<(), String> {
        self.edit(|s| {
            s.shows += 1;
            s.visible = true;
        });
        Ok(())
    }

    fn hide(&self) {
        self.edit(|s| {
            s.hides += 1;
            s.visible = false;
        });
    }

    fn to_library(&self, request: LibraryRequest) {
        self.edit(|s| s.library.push(request));
    }
}

impl PanelHost for FormPanel {
    fn send(&self, event: &FormEvent) {
        self.edit(|s| s.events.push(event.clone()));
    }

    fn show(&self, _height: Option<f64>) -> Result<(), String> {
        self.edit(|s| {
            s.shows += 1;
            s.visible = true;
        });
        Ok(())
    }

    fn hide(&self) {
        self.edit(|s| {
            s.hides += 1;
            s.visible = false;
        });
    }
}

impl PalettePanel {
    /// Waits for a new palette's panel to be shown; returns its session.
    fn wait_shown(&self, after: usize) -> u64 {
        self.wait("shown", |s| {
            s.visible
                && s.events
                    .iter()
                    .skip(after)
                    .any(|e| matches!(e, PaletteEvent::Shown { .. }))
        });
        self.lock()
            .events
            .iter()
            .skip(after)
            .find_map(|e| match e {
                PaletteEvent::Shown { session } => Some(*session),
                _ => None,
            })
            .expect("a shown event")
    }
}

impl FormPanel {
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
}

/// The notice window, recorded.
#[derive(Default)]
struct NoticeWindow {
    shown: Mutex<Vec<Notice>>,
    changed: Condvar,
}

impl NoticeHost for NoticeWindow {
    fn show(&self, notice: &Notice) -> Result<(), String> {
        self.shown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(notice.clone());
        self.changed.notify_all();
        Ok(())
    }

    fn hide(&self) {}
}

impl NoticeWindow {
    /// Waits up to `T` for the `n`th notice.
    fn wait(&self, n: usize) -> Notice {
        let deadline = Instant::now() + T;
        let mut shown = self.shown.lock().unwrap_or_else(PoisonError::into_inner);
        while shown.len() < n {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "no notice was shown");
            shown = self
                .changed
                .wait_timeout(shown, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        shown[n - 1].clone()
    }

    fn count(&self) -> usize {
        self.shown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

/// The bridge's sink in the app: service events go to both flows (`app::Sink`).
#[derive(Default)]
struct Sink {
    palette: OnceLock<Arc<PaletteController>>,
    form: OnceLock<Arc<FormController>>,
    notices: OnceLock<Arc<Notices>>,
    data: PaletteData,
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
        if !connected {
            if let Some(form) = self.form.get() {
                form.connection_lost();
            }
            if let Some(palette) = self.palette.get() {
                palette.connection_lost();
            }
            self.data.forget();
        }
    }

    fn event(&self, _event: &UiEvent) {}

    fn service_event(&self, event: &EventBody) {
        if let Some(form) = self.form.get() {
            form.service_event(event);
        }
        if let Some(palette) = self.palette.get() {
            palette.service_event(event);
        }
        if let Some(notices) = self.notices.get() {
            notices.service_event(event);
        }
        if matches!(event, EventBody::LibraryChanged { .. }) {
            self.data.invalidate();
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

/// A UI host without windows: the bridge, both flows and the recorded panels.
struct Host {
    bridge: Bridge,
    palette: Arc<PaletteController>,
    form: Arc<FormController>,
    panel: Arc<PalettePanel>,
    form_panel: Arc<FormPanel>,
    sink: Arc<Sink>,
    notice_window: Arc<NoticeWindow>,
}

impl Host {
    fn start(service: &Service) -> Self {
        Self::start_with(service, |_| {})
    }

    fn start_with(service: &Service, adjust: impl FnOnce(&mut PaletteConfig)) -> Self {
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
        let focus: Arc<dyn FocusInspector> = Arc::new(service.os.clone());
        let notice_window = Arc::new(NoticeWindow::default());
        let notices = Arc::new(
            Notices::start(
                Arc::new(bridge.clone()),
                notice_window.clone(),
                Arc::clone(&logger),
            )
            .expect("notices start"),
        );
        let told = Arc::clone(&notices);
        let on_request: iamb_desktop::form::OnRequest =
            Arc::new(move |asked, snippet| told.expect(asked, snippet));
        let form_panel = Arc::new(FormPanel::default());
        let mut form_cfg = FormConfig::new(
            Arc::new(bridge.clone()),
            form_panel.clone(),
            Some(Arc::clone(&focus)),
            Arc::clone(&logger),
        );
        form_cfg.layout_wait = Duration::ZERO;
        form_cfg.on_request = Arc::clone(&on_request);
        let form = Arc::new(FormController::start(form_cfg).expect("form starts"));
        let panel = Arc::new(PalettePanel::default());
        let mut cfg =
            PaletteConfig::new(Arc::new(bridge.clone()), panel.clone(), Some(focus), logger);
        let watched = Arc::clone(&form);
        cfg.form_open = Arc::new(move || watched.is_open());
        let asked = Arc::clone(&form);
        cfg.last_form = Arc::new(move || asked.last_submitted());
        cfg.on_request = on_request;
        adjust(&mut cfg);
        let palette = Arc::new(PaletteController::start(cfg).expect("palette starts"));
        let _ = sink.form.set(Arc::clone(&form));
        let _ = sink.palette.set(Arc::clone(&palette));
        let _ = sink.notices.set(notices);
        sink.wait_connected();
        Self {
            bridge,
            palette,
            form,
            panel,
            form_panel,
            sink,
            notice_window,
        }
    }

    /// The UI host goes away (its connection ends, as when the process is killed).
    fn stop(&self) {
        if let Some(notices) = self.sink.notices.get() {
            notices.stop();
        }
        self.palette.stop();
        self.form.stop();
        self.bridge.stop();
    }

    /// Registers the palette's shortcut with the service, and waits until the engine has
    /// handled what the settings change sent the focus thread (the focused app is judged
    /// again, which clears the engine's buffer): otherwise that can reach the engine after
    /// the keys or the pick that follow.
    fn register_shortcut(&self, service: &Service) {
        self.bridge
            .call(RequestBody::PutSetting {
                key: "palette_hotkey".to_owned(),
                value: Some(CHORD.to_owned()),
            })
            .expect("the shortcut is registered");
        service.worker().sync();
    }

    /// Presses the global shortcut and waits for the palette to be shown.
    fn open(&self, service: &Service) -> u64 {
        let before = self.panel.events();
        assert!(
            service.os.press_chord(Chord::parse(CHORD).expect("chord")),
            "the chord is registered"
        );
        self.panel.wait_shown(before)
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
fn the_shortcut_opens_the_palette_and_a_pick_types_where_the_cursor_was() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    host.register_shortcut(&service);
    let session = host.open(&service);

    // The panel was shown within the budget of its part (the paint is the window's, measured
    // by `--measure-palette`); the arrival and the show are this process's clock.
    let timing = host
        .palette
        .painted(session, wall_ms())
        .expect("the palette was timed");
    eprintln!(
        "palette: shown {:.2} ms after PaletteRequested arrived (PanelFocus to a real worker)",
        timing.shown_ms
    );
    assert!(timing.shown_ms < 100.0, "{timing:?}");
    let open = host.palette.current().expect("open");
    assert_eq!((open.session, open.own_window), (session, false));
    assert_eq!(open.last_form, None);

    // Search by content, as the panel's front end does.
    let results = host
        .sink
        .data
        .search(&host.bridge, "right")
        .expect("search");
    assert_eq!(results.total, 3);
    assert_eq!(results.hits.len(), 1);
    let found = &results.hits[0];
    assert_eq!(found.row.id, id(1).to_string());
    assert_eq!(found.row.abbreviation, "brb");
    assert_eq!(found.row.label, "label brb");
    assert_eq!(found.field.as_deref(), Some("body"));
    assert_eq!(found.ranges, [[3, 8]]);
    assert!(!found.row.folder.is_empty());

    host.palette
        .pick(session, Pick::Insert(id(1)))
        .expect("picked");
    wait_text(&service, "be right back");
    assert!(!host.panel.visible());
    assert!(
        host.palette.current().is_none(),
        "the query's session is gone"
    );
    assert_eq!(
        host.panel.lock().events,
        vec![
            PaletteEvent::Open { session },
            PaletteEvent::Shown { session },
            PaletteEvent::Closed { session },
        ]
    );
    host.stop();
}

#[test]
fn esc_changes_nothing_and_the_keyboard_is_the_apps_again() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    host.register_shortcut(&service);
    let session = host.open(&service);
    host.palette.pick(session, Pick::Close).expect("closed");
    service.worker().settle();
    assert_eq!(service.text(), "");
    assert!(service.os.injected().is_empty(), "nothing was typed");
    assert!(!host.panel.visible());
    assert!(host.palette.current().is_none());
    assert_eq!(
        host.palette.pick(session, Pick::Close).map_err(|e| e.code),
        Err(NO_PALETTE.to_owned()),
        "closing is over"
    );
    // The abbreviation expands again: the hold ended.
    service.type_text("brb ");
    wait_text(&service, "be right back ");
    host.stop();
}

#[test]
fn keys_typed_into_the_panel_never_reach_the_engine() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    host.register_shortcut(&service);
    let session = host.open(&service);
    // Typed into the panel (on the mock they land in the editor's text, as the window behind a
    // non-activating panel stays the focused one).
    service.type_text("brb ");
    assert_eq!(service.text(), "brb ", "nothing expanded");
    assert_eq!(
        service.worker().handoff().len(),
        0,
        "nothing was handed off"
    );
    host.palette.pick(session, Pick::Close).expect("closed");
    service.type_text("brb ");
    wait_text(&service, "brb be right back ");
    host.stop();
}

#[test]
fn a_pick_for_a_snippet_with_blanks_opens_the_form_and_the_panels_do_not_fight() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    host.register_shortcut(&service);
    let session = host.open(&service);
    let before = host.form_panel.events();
    host.palette
        .pick(session, Pick::Insert(id(3)))
        .expect("picked");
    let form = host.form_panel.wait_shown(before);
    assert!(!host.panel.visible(), "the palette made way");
    assert!(host.palette.current().is_none());
    let open = host.form.current().expect("the form is open");
    assert_eq!(open.label, "label hi");
    assert_eq!(
        service.text(),
        "",
        "nothing is typed before the form is filled in"
    );
    // The form holds the keyboard now: typing reaches neither the engine nor the palette.
    service.type_text("brb ");
    assert_eq!(service.text(), "brb ");
    // While the form is open the shortcut does nothing.
    let shows = host.panel.shows();
    let refused = service.log_count("desktop", "palette: not opened, a form is open");
    assert!(service.os.press_chord(Chord::parse(CHORD).expect("chord")));
    // The palette's flow says it saw the shortcut and refused it: wait for that, not for a time.
    service.wait_log_count("desktop", "palette: not opened, a form is open", refused);
    assert_eq!(
        host.panel.shows(),
        shows,
        "the palette did not open over the form"
    );
    host.form
        .submit(form, values("Name", "Ada"))
        .expect("submitted");
    wait_text(&service, "brb Hello Ada!");
    assert!(!diagnostic_codes(&service).contains(&"panel_focus_expired".to_owned()));
    host.stop();
}

#[test]
fn the_shortcut_again_closes_the_palette() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    host.register_shortcut(&service);
    let session = host.open(&service);
    assert!(service.os.press_chord(Chord::parse(CHORD).expect("chord")));
    host.panel.wait("closed", |s| {
        !s.visible && s.events.contains(&PaletteEvent::Closed { session })
    });
    assert!(host.palette.current().is_none());
    service.worker().settle();
    assert_eq!(service.text(), "");
    // And opens again, a new session.
    let again = host.open(&service);
    assert_ne!(again, session);
    host.palette.pick(again, Pick::Close).expect("closed");
    host.stop();
}

#[test]
fn when_the_focus_cannot_be_given_back_nothing_is_sent_and_the_panel_returns() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    host.register_shortcut(&service);
    let session = host.open(&service);
    service.os.refuse_focus_restore(true);
    let err = host
        .palette
        .pick(session, Pick::Insert(id(1)))
        .expect_err("the focus did not come back");
    assert_eq!(err.code, iamb_desktop::form::FOCUS_LOST);
    assert!(host.panel.visible(), "the panel is back");
    assert_eq!(host.panel.shows(), 2);
    service.worker().settle();
    assert_eq!(service.text(), "", "nothing was sent");
    // Still the panel's keyboard.
    service.type_text("omw ");
    assert_eq!(service.text(), "omw ");
    // Enter again, once the window takes the focus.
    service.os.refuse_focus_restore(false);
    host.palette
        .pick(session, Pick::Insert(id(1)))
        .expect("picked");
    wait_text(&service, "omw be right back");
    host.stop();
}

#[test]
fn a_panel_that_loses_the_keyboard_gives_it_back_to_the_app_and_takes_it_again() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    host.register_shortcut(&service);
    let session = host.open(&service);
    // A click into the app behind: the panel stays, and the keys are the app's again.
    host.palette.keyboard(false);
    let deadline = Instant::now() + T;
    loop {
        service.type_text("brb ");
        if service.text().contains("be right back") {
            break;
        }
        assert!(Instant::now() < deadline, "the keyboard never went back");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(host.panel.visible(), "the panel stays open");
    // Back into the panel: its keys are dropped again. Wait for the service to say it took the
    // panel's hold in (its log), not for a time.
    let holds = service.log_count("worker", "panel has the keyboard");
    host.palette.keyboard(true);
    service.wait_log_count("worker", "panel has the keyboard", holds);
    let text = service.text();
    service.type_text("omw ");
    assert_eq!(service.text(), format!("{text}omw "), "nothing expanded");
    host.palette.pick(session, Pick::Close).expect("closed");
    host.stop();
}

#[test]
fn the_lease_is_renewed_while_the_palette_is_open() {
    let service = Service::start_with_lease(Endpoint::Mock, Some(Duration::from_millis(600)));
    let host = Host::start_with(&service, |cfg| {
        cfg.lease_renewal = Duration::from_millis(150);
    });
    host.register_shortcut(&service);
    let session = host.open(&service);
    // Longer than twice the lease (600 ms): the hold must outlive it because it is renewed, so
    // the clock is what is tested, not a wait for an event.
    std::thread::sleep(Duration::from_millis(1500));
    service.type_text("brb ");
    assert_eq!(service.text(), "brb ", "still the panel's keyboard");
    assert!(!diagnostic_codes(&service).contains(&"panel_focus_expired".to_owned()));
    host.palette.pick(session, Pick::Close).expect("closed");
    host.stop();
}

#[test]
fn the_tray_opens_it_for_the_app_in_front_and_a_pick_types_there() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    host.palette.open_from_tray();
    let session = host.panel.wait_shown(0);
    host.palette
        .pick(session, Pick::Insert(id(2)))
        .expect("picked");
    wait_text(&service, "on my way");
    host.stop();
}

/// The tray menu opens: on Windows it brings Iamb's own (hidden) window to the front first
/// (the Phase 3 UI review's finding 4). The pointer reached the icon before that.
fn open_tray_menu(service: &Service, host: &Host) {
    host.palette.tray_pointer(true);
    let menu = service
        .os
        .open_window(MockWindow::new("iamb-desktop", std::process::id()).own());
    service.os.focus(menu).expect("open");
    service.worker().sync();
}

#[test]
fn the_tray_menu_taking_the_foreground_does_not_change_where_a_pick_types() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    open_tray_menu(&service, &host);
    assert_eq!(host.palette.tray_target(), Some(service.editor));
    host.palette.open_from_tray();
    let session = host.panel.wait_shown(0);
    assert!(
        !host.palette.current().expect("open").own_window,
        "the palette is for the editor, not for Iamb's own window"
    );
    host.palette
        .pick(session, Pick::Insert(id(2)))
        .expect("picked");
    wait_text(&service, "on my way");
    host.stop();
}

#[test]
fn the_tray_s_fill_in_again_goes_to_the_window_that_was_in_front() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    service.type_text("hi ");
    let form = host.form_panel.wait_shown(0);
    host.form
        .submit(form, values("Name", "Ada"))
        .expect("submitted");
    wait_text(&service, "Hello Ada! ");
    open_tray_menu(&service, &host);
    let before = host.form_panel.events();
    host.form
        .reopen_last_for(host.palette.tray_target())
        .expect("asked");
    let again = host.form_panel.wait_shown(before);
    assert_eq!(
        host.form
            .current()
            .expect("open")
            .last_values
            .map(iamb_ipc::Secret::into_inner),
        Some(values("Name", "Ada"))
    );
    assert!(!diagnostic_codes(&service).contains(&"refused_own_window".to_owned()));
    host.form.cancel(again).expect("cancelled");
    host.stop();
}

#[test]
fn a_palette_whose_window_is_gone_or_that_the_system_closes_closes() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    host.register_shortcut(&service);
    // Closed by the system (Alt+F4): as Esc.
    let session = host.open(&service);
    host.palette.close_requested();
    host.panel.wait("closed", |s| {
        !s.visible && s.events.contains(&PaletteEvent::Closed { session })
    });
    assert!(host.palette.current().is_none());
    // Its window goes while it stepped aside: closed too.
    let mail = service
        .os
        .open_window(MockWindow::new("org.example.mail", 300));
    service.os.focus(mail).expect("open");
    service.worker().sync();
    let session = host.open(&service);
    service.os.focus(service.editor).expect("open");
    host.palette.keyboard(false);
    service.os.close_window(mail);
    host.panel.wait("closed", |s| {
        !s.visible && s.events.contains(&PaletteEvent::Closed { session })
    });
    assert!(host.palette.current().is_none());
    host.stop();
}

#[test]
fn opened_from_iamb_s_own_window_a_pick_goes_to_the_library_and_types_nothing() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    host.palette.open_in_library();
    let session = host.panel.wait_shown(0);
    assert!(host.palette.current().expect("open").own_window);
    host.palette
        .pick(
            session,
            Pick::Library(LibraryRequest::Show {
                id: id(2).to_string(),
            }),
        )
        .expect("handed over");
    assert_eq!(
        host.panel.lock().library,
        vec![LibraryRequest::Show {
            id: id(2).to_string()
        }]
    );
    service.worker().settle();
    assert_eq!(service.text(), "");
    assert!(service.os.injected().is_empty());
    assert!(!host.panel.visible());
    host.stop();
}

#[test]
fn the_last_form_is_offered_again_and_reopens_with_its_answers() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    host.register_shortcut(&service);
    service.type_text("hi ");
    let form = host.form_panel.wait_shown(0);
    host.form
        .submit(form, values("Name", "Ada"))
        .expect("submitted");
    wait_text(&service, "Hello Ada! ");
    let session = host.open(&service);
    assert_eq!(
        host.palette.current().expect("open").last_form.as_deref(),
        Some("label hi")
    );
    let before = host.form_panel.events();
    host.palette.pick(session, Pick::FillAgain).expect("picked");
    let again = host.form_panel.wait_shown(before);
    let reopened = host.form.current().expect("open");
    assert_eq!(reopened.session, again);
    assert_eq!(
        reopened.last_values.map(iamb_ipc::Secret::into_inner),
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
fn a_palette_whose_panel_loses_the_service_closes() {
    let mut service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    host.register_shortcut(&service);
    let session = host.open(&service);
    service.stop_worker();
    host.panel.wait("closed", |s| {
        !s.visible && s.events.contains(&PaletteEvent::Closed { session })
    });
    assert!(host.palette.current().is_none());
    host.stop();
}

#[test]
fn a_ui_host_killed_with_the_palette_open_recovers() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    host.register_shortcut(&service);
    host.open(&service);
    host.stop();
    // Typing expands at once: the hold ended with the connection.
    let deadline = Instant::now() + T;
    while !service.text().contains("be right back") {
        service.type_text("brb ");
        assert!(Instant::now() < deadline, "the service did not notice");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn search_forgives_typos_ranks_and_says_where() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    let data = &host.sink.data;
    // A typo in the label's word plus an exact abbreviation: only one snippet has both.
    let found = data.search(&host.bridge, "lable brb").expect("search");
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.hits[0].row.id, id(1).to_string());
    // The abbreviation first, whatever else the letters match.
    let found = data.search(&host.bridge, "hi").expect("search");
    assert_eq!(found.hits[0].row.id, id(3).to_string());
    assert_eq!(found.hits[0].field.as_deref(), Some("abbreviation"));
    // The row says whether picking it asks for values: from the service, not the first line.
    assert!(found.hits[0].row.has_fields, "\"Hello {{field}}!\" asks");
    let found = data.search(&host.bridge, "brb").expect("search");
    assert!(!found.hits[0].row.has_fields, "\"be right back\" does not");
    // Nothing matches: no rows, and the total is still there for "0 of 3".
    let found = data.search(&host.bridge, "warranty").expect("search");
    assert_eq!((found.hits.len(), found.total), (0, 3));
    // An empty query has no hits.
    assert!(
        data.search(&host.bridge, "  ")
            .expect("search")
            .hits
            .is_empty()
    );
    host.stop();
}

#[test]
fn the_preview_of_a_formatted_snippet_is_its_text_not_its_markdown() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    let mut cli = service.client(Role::Ui);
    let folder = match cli.call(RequestBody::ListFolders {}).expect("folders") {
        ResponseBody::Folders { folders, .. } => folders[0].meta.id,
        other => panic!("unexpected {other:?}"),
    };
    let mut added = iamb_core::model::Snippet::new_defaults(
        iamb_core::model::SyncMeta::new(id(9), iamb_core::model::Timestamp(99)),
        folder,
        iamb_core::model::Abbreviation::new(";bold").expect("abbreviation"),
        "Bold".to_owned(),
        "Hello **world**",
        None,
    );
    added.kind = iamb_core::model::SnippetKind::Rich;
    cli.call(RequestBody::PutSnippet {
        snippet: added,
        tags: None,
    })
    .expect("saved");
    let preview = host
        .sink
        .data
        .preview(&host.bridge, &id(9).to_string())
        .expect("preview");
    let strip = preview
        .parts
        .iter()
        .find_map(|p| match p {
            iamb_desktop::template::PartView::Formatted { plain, .. } => Some(plain.clone()),
            _ => None,
        })
        .expect("a formatted part: the snippet is previewed as formatted");
    assert_eq!(strip, "Hello world");
    host.stop();
}

#[test]
fn the_list_follows_the_library_and_the_preview_is_what_would_be_typed() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    let data = &host.sink.data;
    assert_eq!(data.search(&host.bridge, "omw").expect("search").total, 3);
    assert!(data.is_fresh());
    let mut cli = service.client(Role::Ui);
    let folder = match cli.call(RequestBody::ListFolders {}).expect("folders") {
        ResponseBody::Folders { folders, .. } => folders[0].meta.id,
        other => panic!("unexpected {other:?}"),
    };
    let added = iamb_core::model::Snippet::new_defaults(
        iamb_core::model::SyncMeta::new(id(9), iamb_core::model::Timestamp(99)),
        folder,
        iamb_core::model::Abbreviation::new(";fresh").expect("abbreviation"),
        "Added later".to_owned(),
        "brand new text",
        None,
    );
    cli.call(RequestBody::PutSnippet {
        snippet: added,
        tags: None,
    })
    .expect("saved");
    // The event reaches the sink; the next query reads the list again.
    let deadline = Instant::now() + T;
    while data.is_fresh() {
        assert!(Instant::now() < deadline, "no LibraryChanged arrived");
        std::thread::sleep(Duration::from_millis(10));
    }
    let found = data.search(&host.bridge, "brand").expect("search");
    assert_eq!(found.total, 4);
    assert_eq!(found.hits[0].row.label, "Added later");
    let preview = data
        .preview(&host.bridge, &id(1).to_string())
        .expect("preview");
    let text: String = preview
        .parts
        .iter()
        .filter_map(|p| match p {
            iamb_desktop::template::PartView::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "be right back");
    host.stop();
}

#[test]
fn what_is_searched_reaches_no_log_through_the_palette_flow() {
    const CANARY: &str = "zqxpalettecanary";
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    host.register_shortcut(&service);
    let session = host.open(&service);
    host.sink.data.search(&host.bridge, CANARY).expect("search");
    // The one place a query is used for anything else: handed to the library, once.
    host.palette
        .pick(
            session,
            Pick::Library(LibraryRequest::New {
                label: CANARY.to_owned(),
            }),
        )
        .expect("handed over");
    assert_eq!(
        host.panel.lock().library,
        vec![LibraryRequest::New {
            label: CANARY.to_owned()
        }]
    );
    assert!(
        !format!("{:?} {:?}", host.palette, host.panel.lock().library).contains(CANARY),
        "nothing prints it"
    );
    host.stop();
    for entry in std::fs::read_dir(service.dirs().logs()).expect("logs") {
        let path = entry.expect("entry").path();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            !text.contains(CANARY),
            "{} holds a palette query",
            path.display()
        );
    }
}

#[test]
fn the_default_shortcut_is_offered_once_and_never_again_after_the_user_clears_it() {
    use iamb_desktop::palette::{DEFAULT_SHORTCUT, seed_default_shortcut};

    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    let logger = Logger::open(&service.dirs().logs(), "desktop");
    let run = tempfile::TempDir::new().expect("run dir");
    assert!(service.os.registered_hotkeys().is_empty());
    // A service with no shortcut gets the default; the shortcut then opens the palette.
    assert!(seed_default_shortcut(&host.bridge, run.path(), &logger));
    assert_eq!(
        service.os.registered_hotkeys().len(),
        1,
        "the default is registered"
    );
    assert_eq!(DEFAULT_SHORTCUT, CHORD);
    let session = host.open(&service);
    host.palette.pick(session, Pick::Close).expect("closed");
    // Once only: the user clears it in Settings, and it stays cleared.
    assert!(!seed_default_shortcut(&host.bridge, run.path(), &logger));
    host.bridge
        .call(RequestBody::PutSetting {
            key: "palette_hotkey".to_owned(),
            value: None,
        })
        .expect("cleared");
    assert!(!seed_default_shortcut(&host.bridge, run.path(), &logger));
    assert!(service.os.registered_hotkeys().is_empty());
    host.stop();
}

#[test]
fn a_shortcut_the_user_chose_is_left_alone_and_a_refused_default_is_tried_again_later() {
    use iamb_desktop::palette::seed_default_shortcut;

    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    let logger = Logger::open(&service.dirs().logs(), "desktop");
    // Another application holds the chord: refused, no marker, the next start tries again.
    service
        .os
        .reserve_chord(Chord::parse(CHORD).expect("chord"));
    let run = tempfile::TempDir::new().expect("run dir");
    assert!(!seed_default_shortcut(&host.bridge, run.path(), &logger));
    assert!(service.os.registered_hotkeys().is_empty());
    assert!(!run.path().join("palette-shortcut-seeded").exists());
    // The user chose another one (before): the default is not offered, and the choice stays.
    host.bridge
        .call(RequestBody::PutSetting {
            key: "palette_hotkey".to_owned(),
            value: Some("Ctrl+Shift+J".to_owned()),
        })
        .expect("chosen");
    assert!(!seed_default_shortcut(&host.bridge, run.path(), &logger));
    assert_eq!(service.os.registered_hotkeys().len(), 1);
    assert!(run.path().join("palette-shortcut-seeded").exists());
    host.stop();
}

/// 100 queries over 10,000 snippets through the whole path of a keystroke's search: the bridge,
/// the service's `Search` over its snapshot, and the rows drawn from the list the UI host holds
/// (`PaletteData::search`). A debug build is several times slower than the release one the KPI
/// is about, so the bound here is loose; `iamb-bench`'s `search/query/10000` is the gated
/// measurement of the search itself (`docs/test-strategy.md` §8.7).
#[test]
fn search_through_the_bridge_at_10000_snippets() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    let mut cli = service.client(Role::Ui);
    let words = [
        "thanks", "order", "invoice", "meeting", "tomorrow", "schedule", "address", "support",
        "ticket", "project", "review", "draft", "attached", "regards",
    ];
    let started = Instant::now();
    let mut n = 1_000_u64;
    let imported = tempfile::TempDir::new().expect("dir");
    let path = imported.path().join("library.csv");
    let mut csv = String::from("abbreviation,label,body\n");
    for i in 0..10_000_usize {
        n += 1;
        let _ = writeln!(
            csv,
            ";x{n:05},{} {} note,{} {} {} for {} number {i}",
            words[i % words.len()],
            words[(i / 7) % words.len()],
            words[(i / 3) % words.len()],
            words[(i / 11) % words.len()],
            words[(i / 5) % words.len()],
            words[(i / 13) % words.len()],
        );
    }
    std::fs::write(&path, csv).expect("csv");
    cli.call(RequestBody::Import {
        path: path.clone(),
        format: iamb_ipc::TransferFormat::Csv,
        on_conflict: iamb_ipc::ConflictPolicy::Skip,
    })
    .expect("imported");
    eprintln!(
        "palette: 10,000 snippets imported in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    let data = &host.sink.data;
    let first = data.search(&host.bridge, "thanks").expect("search");
    assert!(first.total >= 10_000, "{}", first.total);
    assert_eq!(first.hits.len(), 50);
    let mut times: Vec<f64> = Vec::new();
    for i in 0..100_usize {
        let query = match i % 5 {
            0 => format!(";x{:05}", 1_001 + (i * 17) % 10_000),
            1 => words[i % words.len()].to_owned(),
            2 => format!(
                "{} {}",
                words[i % words.len()],
                words[(i / 3) % words.len()]
            ),
            3 => words[i % words.len()].replace("ea", "ae"),
            _ => format!("zqx{i}"),
        };
        let t = Instant::now();
        let found = data.search(&host.bridge, &query).expect("search");
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        assert!(found.hits.len() <= 50);
    }
    times.sort_by(f64::total_cmp);
    let p95 = times[times.len() * 95 / 100];
    eprintln!(
        "palette: 100 searches at 10,000 snippets through the bridge: p50 {:.1} ms, p95 {p95:.1} ms, max {:.1} ms (debug build)",
        times[times.len() / 2],
        times[times.len() - 1]
    );
    // Debug build: about ten times the release build's figure; the bound is for a hang or a
    // quadratic mistake, not for the KPI.
    assert!(p95 < 600.0, "p95 {p95:.1} ms");
    host.stop();
}

#[test]
fn a_refused_pick_shows_a_notice_and_one_that_types_does_not() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    host.register_shortcut(&service);
    // A pick that types: no notice.
    let session = host.open(&service);
    host.palette
        .pick(session, Pick::Insert(id(2)))
        .expect("picked");
    wait_text(&service, "on my way");
    // A pick into a password field: refused, and said so (spec §4.11).
    service
        .os
        .set_security(service.editor, iamb_platform::FieldSecurity::Secure);
    service.worker().sync();
    let session = host.open(&service);
    host.palette
        .pick(session, Pick::Insert(id(1)))
        .expect("sent");
    let notice = host.notice_window.wait(1);
    assert_eq!(notice.code, "refused_secure_field");
    assert_eq!(notice.label, "label brb");
    assert!(!notice.form);
    assert_eq!(
        host.notice_window.count(),
        1,
        "one notice, for the refused pick"
    );
    assert_eq!(service.text(), "on my way", "nothing was typed");
    host.stop();
}

#[test]
fn a_refused_form_insert_shows_a_notice_with_the_shortcut() {
    let service = Service::start(Endpoint::Mock);
    let host = Host::start(&service);
    host.register_shortcut(&service);
    service.type_text("hi ");
    let session = host.form_panel.wait_shown(0);
    // The field turns into a password field before Insert.
    service
        .os
        .set_security(service.editor, iamb_platform::FieldSecurity::Secure);
    service.worker().sync();
    host.form
        .submit(session, values("Name", "Ada"))
        .expect("sent");
    let notice = host.notice_window.wait(1);
    assert_eq!(notice.code, "refused_secure_field");
    assert_eq!(notice.label, "label hi");
    assert!(notice.form, "the answers are kept: the sentence says so");
    assert_eq!(notice.shortcut.as_deref(), Some(CHORD));
    host.stop();
}
