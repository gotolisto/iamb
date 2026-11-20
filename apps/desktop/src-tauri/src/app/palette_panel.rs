//! The search palette in the app (task 3.6): the [`PaletteHost`] over the pre-warmed `palette`
//! window, the platform layer that gives the focus back, the hand-over to the library window,
//! and the palette's Tauri commands. The flow itself is `crate::palette`;
//! `apps/desktop/src/palette/` is the front end.
//!
//! | Command | Does |
//! | --- | --- |
//! | `palette_ready` | The panel's page has loaded and listens: it paints once while hidden |
//! | `palette_current` | The open palette, for a panel whose page loaded after the request |
//! | `palette_painted` | The panel painted after it was shown (the 100 ms measurement) |
//! | `palette_search` | `Search`, with the rows of its hits |
//! | `palette_preview` | What the selected snippet would type now (`GetSnippet`, `RenderPreview`) |
//! | `palette_pick` | Insert, fill in again, hand to the library, or close (hides, hands the focus back first) |
//! | `palette_open` | ⌘K / Ctrl+K in the library window: the palette over it |
//! | `library_take_request` | The library window takes what a pick asked of it |
//!
//! The palette's commands answer the `palette` window only, `palette_open` and
//! `library_take_request` the `library` window only. None logs its arguments or its answer: the
//! query is the user's text (architecture §8, spec §9).

use std::sync::{Arc, Mutex, PoisonError};

use iamb_platform::FocusInspector;
use tauri::{AppHandle, Emitter, EventTarget, Manager, State, WebviewWindow};

use super::{Ui, windows};
use crate::bridge::Bridge;
use crate::contract::{CommandError, parse_id};
use crate::form::FormService;
use crate::log::Logger;
use crate::palette::{
    LibraryRequest, PALETTE_EVENT, PaletteConfig, PaletteController, PaletteEvent, PaletteHost,
    PaletteOpen, Pick,
};
use crate::palette_data::{PaletteData, PaletteResults};
use crate::template::PreviewView;

type Res<T> = Result<T, CommandError>;

/// The Tauri event that tells the library window a request waits for it (it has no payload: the
/// window takes the request with `library_take_request`, so that a request made before its page
/// loaded is not lost).
pub const LIBRARY_REQUEST_EVENT: &str = "iamb:library-request";

/// The palette's parts the app holds.
pub struct PalettePanel {
    /// The flow.
    pub controller: PaletteController,
    /// The snippet list and the searches.
    pub data: PaletteData,
    /// What the library window is to do when it next looks.
    inbox: Mutex<Option<LibraryRequest>>,
    /// Where the marker of the default shortcut lives.
    run_dir: std::path::PathBuf,
    /// The default shortcut was looked at in this run of the UI host.
    seeded: std::sync::atomic::AtomicBool,
}

impl PalettePanel {
    fn put(&self, request: LibraryRequest) {
        *self.inbox.lock().unwrap_or_else(PoisonError::into_inner) = Some(request);
    }

    fn take(&self) -> Option<LibraryRequest> {
        self.inbox
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

/// The `palette` window as the flow's [`PaletteHost`].
struct Panel {
    app: AppHandle,
}

impl PaletteHost for Panel {
    fn send(&self, event: &PaletteEvent) {
        // To the palette panel only: nobody else hears of what is searched.
        let _ = self.app.emit_to(
            EventTarget::webview_window(windows::PALETTE),
            PALETTE_EVENT,
            event,
        );
    }

    fn show(&self) -> Result<(), String> {
        windows::show_panel(
            &self.app,
            windows::PALETTE,
            windows::PALETTE_WIDTH,
            Some(windows::PALETTE_HEIGHT),
        )
        .map_err(|e| e.to_string())
    }

    fn hide(&self) {
        windows::hide_panel(&self.app, windows::PALETTE);
    }

    fn prepaint(&self) {
        windows::prepaint_panel(&self.app, windows::PALETTE);
    }

    fn to_library(&self, request: LibraryRequest) {
        let Some(ui) = self.app.try_state::<Arc<Ui>>() else {
            return;
        };
        ui.palette.put(request);
        let app = self.app.clone();
        let ui: Arc<Ui> = Arc::clone(&ui);
        // Off the flow's thread: opening a window can take a while (`windows::open_library`).
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(e) = windows::open_library(&app, &ui.timer) {
                ui.log
                    .warn(format_args!("cannot open the library window: {e}"));
                return;
            }
            let _ = app.emit_to(
                EventTarget::webview_window(windows::LIBRARY),
                LIBRARY_REQUEST_EVENT,
                (),
            );
        });
    }
}

/// The platform layer the UI host gives the focus back with and follows the front application
/// with; none on Linux, which is for development only.
#[cfg_attr(
    any(windows, target_os = "macos"),
    expect(
        clippy::unnecessary_wraps,
        reason = "the same signature returns `None` on Linux"
    )
)]
fn platform_focus() -> Option<Arc<dyn FocusInspector>> {
    #[cfg(windows)]
    {
        Some(Arc::new(iamb_platform_windows::Platform::new()))
    }
    #[cfg(target_os = "macos")]
    {
        Some(Arc::new(iamb_platform_macos::Platform::new()))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        None
    }
}

/// Starts the palette's flow over `bridge`; for the measurement (`measuring`), over a service
/// that answers at once and with no connection to follow (`crate::form::InstantService`).
///
/// # Errors
///
/// When its thread cannot be started.
pub fn start(
    app: &AppHandle,
    bridge: &Bridge,
    run_dir: std::path::PathBuf,
    measuring: bool,
    log: &Arc<Logger>,
) -> std::io::Result<PalettePanel> {
    let host = Arc::new(Panel { app: app.clone() });
    let service: Arc<dyn FormService> = if measuring {
        Arc::new(crate::form::InstantService)
    } else {
        Arc::new(bridge.clone())
    };
    let mut cfg = PaletteConfig::new(service, host, platform_focus(), Arc::clone(log));
    let handle = app.clone();
    cfg.form_open = Arc::new(move || {
        handle
            .try_state::<Arc<Ui>>()
            .is_some_and(|ui| ui.form.is_open())
    });
    let handle = app.clone();
    cfg.last_form = Arc::new(move || {
        handle
            .try_state::<Arc<Ui>>()
            .and_then(|ui| ui.form.last_submitted())
    });
    cfg.follows_connection = !measuring;
    Ok(PalettePanel {
        controller: PaletteController::start(cfg)?,
        data: PaletteData::default(),
        inbox: Mutex::new(None),
        run_dir,
        // A measurement has no service to offer a shortcut to.
        seeded: std::sync::atomic::AtomicBool::new(measuring),
    })
}

fn only(window: &WebviewWindow, label: &str, what: &str) -> Res<()> {
    if window.label() == label {
        Ok(())
    } else {
        Err(CommandError::new(
            "not_permitted",
            format!("only the {what} may do this"),
        ))
    }
}

fn only_the_palette(window: &WebviewWindow) -> Res<()> {
    only(window, windows::PALETTE, "search palette")
}

fn only_the_library(window: &WebviewWindow) -> Res<()> {
    only(window, windows::LIBRARY, "library window")
}

async fn blocking<T: Send + 'static>(
    ui: &Arc<Ui>,
    f: impl FnOnce(&Ui) -> Res<T> + Send + 'static,
) -> Res<T> {
    let ui = Arc::clone(ui);
    tauri::async_runtime::spawn_blocking(move || f(&ui))
        .await
        .map_err(|_| CommandError::new("internal", "the request did not finish"))?
}

/// The panel's page has loaded and listens: the panel paints once while hidden
/// (`windows::prepaint_panel`), so that the first palette does not pay for the first paint.
#[tauri::command]
pub async fn palette_ready(ui: State<'_, Arc<Ui>>, window: WebviewWindow) -> Res<()> {
    only_the_palette(&window)?;
    ui.palette.controller.prepaint();
    Ok(())
}

/// The open palette, if any.
#[tauri::command]
pub async fn palette_current(
    ui: State<'_, Arc<Ui>>,
    window: WebviewWindow,
) -> Res<Option<PaletteOpen>> {
    only_the_palette(&window)?;
    let open = ui.palette.controller.current();
    if open.is_some() && !ui.palette.data.is_fresh() {
        // Read the snippet list while the user starts typing, so that the first answer does not
        // wait for it (a library of 10,000 snippets is ten pages).
        let ui: Arc<Ui> = Arc::clone(&ui);
        tauri::async_runtime::spawn_blocking(move || {
            let _ = ui.palette.data.refresh(&ui.bridge);
        });
    }
    Ok(open)
}

/// The panel of `session` painted at `painted_at_ms` (epoch milliseconds) after it was shown.
#[tauri::command]
pub async fn palette_painted(
    ui: State<'_, Arc<Ui>>,
    window: WebviewWindow,
    session: u64,
    painted_at_ms: f64,
) -> Res<()> {
    only_the_palette(&window)?;
    if let Some(t) = ui.palette.controller.painted(session, painted_at_ms) {
        ui.log.info(format_args!(
            "palette {session}: first paint {:.1} ms after the request arrived (shown after \
             {:.1} ms, reported after {:.1} ms)",
            t.paint_ms, t.shown_ms, t.report_ms
        ));
    }
    Ok(())
}

/// The hits for `query` with their rows. The query is not logged and not kept.
#[tauri::command]
pub async fn palette_search(
    ui: State<'_, Arc<Ui>>,
    window: WebviewWindow,
    query: String,
) -> Res<PaletteResults> {
    only_the_palette(&window)?;
    blocking(&ui, move |ui| ui.palette.data.search(&ui.bridge, &query)).await
}

/// What snippet `id` would type now, for the strip under the list.
#[tauri::command]
pub async fn palette_preview(
    ui: State<'_, Arc<Ui>>,
    window: WebviewWindow,
    id: String,
) -> Res<PreviewView> {
    only_the_palette(&window)?;
    blocking(&ui, move |ui| ui.palette.data.preview(&ui.bridge, &id)).await
}

/// What the user did with the palette of `session`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PickAction {
    /// Type the snippet.
    Insert {
        /// The snippet's id.
        id: String,
    },
    /// Open the last form again.
    FillAgain,
    /// Show the snippet in the library window.
    Show {
        /// The snippet's id.
        id: String,
    },
    /// A new snippet in the library window.
    New {
        /// The draft's label.
        label: String,
    },
    /// Esc.
    Close,
}

impl PickAction {
    /// The flow's pick.
    ///
    /// # Errors
    ///
    /// `invalid` for an id that is none.
    pub fn pick(self) -> Res<Pick> {
        Ok(match self {
            Self::Insert { id } => Pick::Insert(parse_id(&id)?),
            Self::FillAgain => Pick::FillAgain,
            Self::Show { id } => Pick::Library(LibraryRequest::Show {
                id: parse_id(&id)?.to_string(),
            }),
            Self::New { label } => Pick::Library(LibraryRequest::New { label }),
            Self::Close => Pick::Close,
        })
    }
}

/// Carries out the user's pick: hides the panel, hands the focus back, then asks the service.
#[tauri::command]
pub async fn palette_pick(
    ui: State<'_, Arc<Ui>>,
    window: WebviewWindow,
    session: u64,
    action: PickAction,
) -> Res<()> {
    only_the_palette(&window)?;
    let pick = action.pick()?;
    blocking(&ui, move |ui| ui.palette.controller.pick(session, pick)).await
}

/// ⌘K / Ctrl+K in the library window: the palette over it.
#[tauri::command]
pub async fn palette_open(ui: State<'_, Arc<Ui>>, window: WebviewWindow) -> Res<()> {
    only_the_library(&window)?;
    ui.palette.controller.open_in_library();
    Ok(())
}

/// What a pick in the palette asked of the library window, once.
#[tauri::command]
pub async fn library_take_request(
    ui: State<'_, Arc<Ui>>,
    window: WebviewWindow,
) -> Res<Option<LibraryRequest>> {
    only_the_library(&window)?;
    Ok(ui.palette.take())
}

/// Runs the palette's part of a service event: a `PaletteRequested` opens it, a
/// `LibraryChanged` makes its snippet list read again. Never blocks.
pub fn service_event(ui: &Ui, event: &iamb_ipc::EventBody) {
    ui.palette.controller.service_event(event);
    if matches!(event, iamb_ipc::EventBody::LibraryChanged { .. }) {
        ui.palette.data.invalidate();
    }
}

/// The bridge connected: the first time in this run, offers the default shortcut to the service
/// (`crate::palette::seed_default_shortcut`). Never blocks.
pub fn connected(ui: &Arc<Ui>) {
    if ui
        .palette
        .seeded
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        return;
    }
    let ui = Arc::clone(ui);
    tauri::async_runtime::spawn_blocking(move || {
        crate::palette::seed_default_shortcut(&ui.bridge, &ui.palette.run_dir, &ui.log);
    });
}

/// The connection to the service ended: an open palette is over and the list is forgotten.
pub fn connection_lost(ui: &Ui) {
    ui.palette.controller.connection_lost();
    ui.palette.data.forget();
}
