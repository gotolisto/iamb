//! Notices over other apps (`docs/ux/spec.md` §4.11 and owner decision 4, design system §4.9;
//! the Phase 3 UI review's finding 3).
//!
//! Iamb shows a notice for one thing only: an explicit request (a palette pick, a form's
//! Insert, "Fill in … again") that the service refused, so that nothing was typed. The service
//! has no outcome event for those requests yet (spec §11.2 entry 8); until it has, the rule of
//! that entry applies: a `DiagnosticAdded` with a refusal code for the same snippet within
//! [`MATCH_WITHIN`] of the request is the refusal. The request is noted before it is sent
//! ([`Notices::expect`]), so the diagnostic, which comes back on the bridge's reader thread, can
//! never overtake it.
//!
//! The notice names the snippet ("“Late delivery” was not inserted") and the reason; for a form
//! it adds where the answers went and the palette's shortcut. The words are the front end's
//! (`apps/desktop/src/notice/`): this module sends the code, the label, whether it was a form
//! and the shortcut. It reads the label (`GetSnippet`) and the shortcut (`GetSettings`) on its
//! own thread (`iamb-notice`), never on the bridge's reader. A notice leaves after [`SHOW_FOR`],
//! takes no focus, and a newer one replaces it (one at a time).
//!
//! Nothing here touches Tauri: the window is a [`NoticeHost`] (`app::notice_window` is the real
//! one, a small window in a corner of the screen), so the matching runs in tests against a real
//! worker (`tests/notice.rs`).
//!
//! # Privacy
//!
//! A diagnostic carries a code, a snippet and an app, never typed text or form values, and so
//! does a notice; the label is library content. Nothing here logs a label.

use std::io;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use iamb_core::model::Id;
use iamb_ipc::{EventBody, RequestBody};
use serde::Serialize;

use crate::form::{FormService, header_from};
use crate::log::Logger;

/// The Tauri event that tells the notice window to fetch the notice (`notice_current`).
pub const NOTICE_EVENT: &str = "iamb:notice";

/// How long after a request a refusal for its snippet still counts as its answer (spec §11.2
/// entry 8).
pub const MATCH_WITHIN: Duration = Duration::from_secs(3);

/// How long a notice stays (design system §4.9).
pub const SHOW_FOR: Duration = Duration::from_secs(6);

/// The codes a refused request is told with (spec §6.2, the "In a notice" column; and
/// `form_target_gone`, a form whose window was closed), besides the template errors (`E_…`).
pub const NOTICE_CODES: [&str; 10] = [
    "refused_secure_field",
    "refused_excluded_app",
    "refused_unknown_focus",
    "refused_composing",
    "request_cancelled",
    "request_focus_timeout",
    "injection_busy",
    "unknown_snippet",
    "no_last_form",
    "form_target_gone",
];

/// Whether a diagnostic with `code` answers a refused request with a notice.
#[must_use]
pub fn is_notice_code(code: &str) -> bool {
    NOTICE_CODES.contains(&code) || code.starts_with("E_")
}

/// What the user asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    /// A palette pick: type the snippet.
    Insert,
    /// A fill-in form's Insert (or its window closed: its answers handed in).
    Form,
    /// "Fill in … again" (the palette's row, or the tray's item).
    Reopen,
}

/// A notice, as the notice window fetches it (`notice_current`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    /// Counts up; the front end shows the newest.
    pub id: u64,
    /// The diagnostic's code: the reason.
    pub code: String,
    /// The snippet's label, or its abbreviation when it has none; empty when it could not be
    /// read (the snippet was deleted, or none was named).
    pub label: String,
    /// The request was a form's: the answers are kept, and the sentence says how to use them.
    pub form: bool,
    /// The palette's shortcut, as the setting writes it, when one is set.
    pub shortcut: Option<String>,
}

/// The notice window, as the notices see it.
pub trait NoticeHost: Send + Sync + 'static {
    /// Shows `notice` in a corner of the screen, without taking the focus.
    ///
    /// # Errors
    ///
    /// A sentence for the log when the window cannot be made or shown.
    fn show(&self, notice: &Notice) -> Result<(), String>;
    /// Hides the notice.
    fn hide(&self);
}

enum Msg {
    Expect {
        asked: Asked,
        snippet: Option<Id>,
        at: Instant,
    },
    Diagnostic {
        code: String,
        snippet: Option<Id>,
    },
}

struct Pending {
    asked: Asked,
    snippet: Option<Id>,
    at: Instant,
}

/// The notices' thread (module docs). Cheap to share; [`stop`](Self::stop) ends it.
pub struct Notices {
    tx: Mutex<Option<Sender<Msg>>>,
    current: Arc<Mutex<Option<Notice>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for Notices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notices")
            .field("shown", &lock(&self.current).as_ref().map(|n| n.id))
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Notices {
    /// Starts the thread.
    ///
    /// # Errors
    ///
    /// When the thread cannot be started.
    pub fn start(
        service: Arc<dyn FormService>,
        host: Arc<dyn NoticeHost>,
        logger: Arc<Logger>,
    ) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let current = Arc::new(Mutex::new(None));
        let mut flow = Flow {
            service,
            host,
            logger,
            current: Arc::clone(&current),
            pending: Vec::new(),
            shown_until: None,
            next_id: 0,
        };
        let thread = thread::Builder::new()
            .name("iamb-notice".to_owned())
            .spawn(move || flow.run(&rx))?;
        Ok(Self {
            tx: Mutex::new(Some(tx)),
            current,
            thread: Mutex::new(Some(thread)),
        })
    }

    fn send(&self, msg: Msg) {
        if let Some(tx) = lock(&self.tx).as_ref() {
            let _ = tx.send(msg);
        }
    }

    /// A request for `snippet` (none for "Fill in … again", whose snippet the service picks)
    /// is about to be sent. Call it before the request goes out. Never blocks.
    pub fn expect(&self, asked: Asked, snippet: Option<Id>) {
        self.send(Msg::Expect {
            asked,
            snippet,
            at: Instant::now(),
        });
    }

    /// A service event, from the bridge's reader thread: a `DiagnosticAdded` may answer a
    /// request. Never blocks.
    pub fn service_event(&self, event: &EventBody) {
        if let EventBody::DiagnosticAdded { entry } = event
            && is_notice_code(&entry.code)
        {
            self.send(Msg::Diagnostic {
                code: entry.code.clone(),
                snippet: entry.snippet,
            });
        }
    }

    /// The notice on screen, if any (the notice window's `notice_current`).
    #[must_use]
    pub fn current(&self) -> Option<Notice> {
        lock(&self.current).clone()
    }

    /// Stops the thread.
    pub fn stop(&self) {
        lock(&self.tx).take();
        if let Some(handle) = lock(&self.thread).take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Notices {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Flow {
    service: Arc<dyn FormService>,
    host: Arc<dyn NoticeHost>,
    logger: Arc<Logger>,
    current: Arc<Mutex<Option<Notice>>>,
    pending: Vec<Pending>,
    shown_until: Option<Instant>,
    next_id: u64,
}

impl Flow {
    fn run(&mut self, rx: &Receiver<Msg>) {
        loop {
            let msg = match self.shown_until {
                Some(until) => {
                    match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                        Ok(msg) => Some(msg),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                None => match rx.recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => break,
                },
            };
            match msg {
                Some(Msg::Expect { asked, snippet, at }) => {
                    self.prune();
                    self.pending.push(Pending { asked, snippet, at });
                }
                Some(Msg::Diagnostic { code, snippet }) => self.diagnostic(code, snippet),
                None => {}
            }
            if self
                .shown_until
                .is_some_and(|until| Instant::now() >= until)
            {
                self.shown_until = None;
                lock(&self.current).take();
                self.host.hide();
            }
        }
        if self.shown_until.take().is_some() {
            self.host.hide();
        }
    }

    fn prune(&mut self) {
        self.pending.retain(|p| p.at.elapsed() < MATCH_WITHIN);
    }

    /// The request a diagnostic answers: the newest one for its snippet; for one that names no
    /// snippet, or a request that named none ("Fill in … again"), the newest such.
    fn answered(&mut self, code: &str, snippet: Option<Id>) -> Option<Pending> {
        self.prune();
        let at = self
            .pending
            .iter()
            .rposition(|p| match (p.snippet, snippet) {
                (Some(asked), Some(got)) => asked == got,
                (None, _) => p.asked == Asked::Reopen,
                (Some(_), None) => code == "no_last_form",
            })?;
        Some(self.pending.remove(at))
    }

    fn diagnostic(&mut self, code: String, snippet: Option<Id>) {
        let Some(request) = self.answered(&code, snippet) else {
            return;
        };
        let label = snippet
            .or(request.snippet)
            .and_then(|id| self.service.call(crate::contract::get_snippet(id)).ok())
            .as_ref()
            .and_then(header_from)
            .map(|h| {
                if h.label.is_empty() {
                    h.abbreviation
                } else {
                    h.label
                }
            })
            .unwrap_or_default();
        let form = request.asked != Asked::Insert || code == "form_target_gone";
        let shortcut = if form {
            match self.service.call(RequestBody::GetSettings {}) {
                Ok(iamb_ipc::ResponseBody::Settings { settings }) => {
                    settings.get("palette_hotkey").cloned()
                }
                _ => None,
            }
        } else {
            None
        };
        self.next_id += 1;
        let notice = Notice {
            id: self.next_id,
            code,
            label,
            form,
            shortcut,
        };
        self.logger.info(format_args!(
            "notice {}: a request was refused ({})",
            notice.id, notice.code
        ));
        *lock(&self.current) = Some(notice.clone());
        match self.host.show(&notice) {
            Ok(()) => self.shown_until = Some(Instant::now() + SHOW_FOR),
            Err(e) => {
                self.logger
                    .warn(format_args!("notice {}: cannot be shown: {e}", notice.id));
                lock(&self.current).take();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_codes_that_make_a_notice_are_the_specification_s() {
        assert!(is_notice_code("refused_secure_field"));
        assert!(is_notice_code("E_SNIPPET_NOT_FOUND"));
        assert!(is_notice_code("form_target_gone"));
        // No notice for what the user did not ask for, or for Iamb's own window (the palette
        // opens the snippet instead).
        assert!(!is_notice_code("refused_own_window"));
        assert!(!is_notice_code("suspended_secure_field"));
        assert!(!is_notice_code("form_open"));
        assert!(!is_notice_code("injection_cancelled"));
        let json = serde_json::to_value(Notice {
            id: 1,
            code: "refused_secure_field".to_owned(),
            label: "Late delivery".to_owned(),
            form: true,
            shortcut: Some("Ctrl+Shift+Space".to_owned()),
        })
        .expect("json");
        assert_eq!(
            json,
            serde_json::json!({"id": 1, "code": "refused_secure_field", "label": "Late delivery",
                "form": true, "shortcut": "Ctrl+Shift+Space"})
        );
    }
}
