//! `--measure-form RUNS`: the fill-in form's "visible in under 150 ms" harness
//! (`docs/test-strategy.md` §8.5).
//!
//! After start-up has settled (the pre-warmed panels have loaded, and the form panel has
//! painted once while hidden, `windows::prepaint_panel`), it hands the form's flow a
//! `FormRequested` `RUNS` times, as the bridge does when the service sends one, stamped with
//! the moment it is handed over. The flow acknowledges it, sends the form to the panel, asks for
//! the keyboard, shows the panel; the front end reports the panel's next paint after it was
//! shown (`form_painted`), and the time from the request's arrival to that paint is the
//! measurement (`crate::form::FormTiming`). The form is then cancelled, which hides the panel,
//! and the next one follows. The service is a stand-in that answers every request at once
//! ([`InstantService`](crate::form::InstantService)): the round trips to a real worker
//! (`FormAck`, `GetSnippet`, `PanelFocus`, about a millisecond together on the same machine) are
//! measured by `tests/form.rs` instead. Prints one line per form and a summary, then exits:
//! status 0, or 1 when a form was not painted within 10 s or any took longer than
//! `--budget-ms`.
//!
//! Each form is a different session of the same snippet with six fields, one of each kind and
//! a choice of each size; every other one carries remembered values, as `ReopenLastForm` does.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use iamb_core::model::Id;
use iamb_ipc::{EventBody, Secret, WireField};
use tauri::{AppHandle, Manager};

use super::Ui;
use crate::timing::{Measurement, summarize, wall_ms};

/// How long start-up gets before the first form.
const SETTLE: Duration = Duration::from_secs(3);

/// How long one form may take to paint.
const PAINT_LIMIT: Duration = Duration::from_secs(10);

fn say(line: &str) {
    let _ = writeln!(std::io::stdout().lock(), "{line}");
}

pub(super) fn spawn(app: AppHandle, runs: usize, budget_ms: Option<f64>) {
    let _ = thread::Builder::new()
        .name("iamb-measure".to_owned())
        .spawn(move || {
            let code = measure(&app, runs, budget_ms);
            app.exit(code);
        });
}

fn field(name: &str, kind: &str, options: &[&str], default: &str) -> WireField {
    WireField {
        name: name.to_owned(),
        kind: kind.to_owned(),
        options: options.iter().map(|o| (*o).to_owned()).collect(),
        default: default.to_owned(),
    }
}

/// The measured form: six fields, one of each kind (and a choice of each size).
fn request(session: u64, remembered: bool) -> EventBody {
    let spec = vec![
        field("First name", "text", &[], ""),
        field("Order number", "text", &[], ""),
        field("Bike", "choice", &["Road", "Gravel", "City"], "Road"),
        field(
            "Rim",
            "choice",
            &[
                "26",
                "27.5",
                "29",
                "650b",
                "700c",
                "700c, 32 holes",
                "Other",
            ],
            "700c",
        ),
        field("Work needed", "area", &[], ""),
        field("Add voucher", "toggle", &[], "no"),
    ];
    let last_values = remembered.then(|| {
        Secret::new(BTreeMap::from([
            ("First name".to_owned(), "Sample".to_owned()),
            ("Bike".to_owned(), "City".to_owned()),
            ("Add voucher".to_owned(), "yes".to_owned()),
        ]))
    });
    EventBody::FormRequested {
        session,
        snippet: Id::from_bytes([7; 16]),
        spec,
        focus: None,
        last_values,
    }
}

fn measure(app: &AppHandle, runs: usize, budget_ms: Option<f64>) -> i32 {
    thread::sleep(SETTLE);
    let Some(ui) = app.try_state::<Arc<Ui>>().map(|s| Arc::clone(&s)) else {
        say("form-open: the app did not start");
        return 1;
    };
    // The panel paints once while hidden when its page reports it loaded (`form_ready`); a
    // slow start-up gets longer, as a user's first form would wait for its page too.
    let deadline = Instant::now() + PAINT_LIMIT;
    while !ui.form.prepainted() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    if !ui.form.prepainted() {
        say("form-open: the panel's page did not report itself loaded; measuring anyway");
    }
    thread::sleep(Duration::from_millis(100));
    let mut results: Vec<Measurement> = Vec::new();
    for run in 1..=runs {
        let session = u64::try_from(run).unwrap_or(u64::MAX);
        ui.form
            .service_event_at(&request(session, run % 2 == 0), Instant::now(), wall_ms());
        let Some(t) = ui.form.timer().wait(PAINT_LIMIT) else {
            say(&format!(
                "form-open: run {run}: no first paint within {PAINT_LIMIT:?}"
            ));
            return 1;
        };
        say(&format!(
            "form-open: run {run}: first paint {:.1} ms after the request arrived (shown after \
             {:.1} ms, report {:.1} ms)",
            t.paint_ms, t.shown_ms, t.report_ms
        ));
        results.push(Measurement {
            paint_ms: t.paint_ms,
            report_ms: t.report_ms,
            navigation_ms: t.shown_ms,
            script_ms: t.shown_ms,
        });
        if let Err(e) = ui.form.cancel(session) {
            say(&format!("form-open: run {run}: cancel failed: {}", e.code));
            return 1;
        }
        thread::sleep(Duration::from_millis(300));
    }
    let Some(s) = summarize(&results) else {
        say("form-open: no runs");
        return 1;
    };
    let verdict = match budget_ms {
        Some(budget) if s.max_ms > budget => format!("over the budget of {budget} ms"),
        Some(budget) => format!("within the budget of {budget} ms"),
        None => "no budget given".to_owned(),
    };
    say(&format!(
        "form-open: summary: first {:.1} ms, later median {:.1} ms, max {:.1} ms over {} runs; {verdict}",
        s.first_ms, s.median_later_ms, s.max_ms, s.runs
    ));
    i32::from(budget_ms.is_some_and(|b| s.max_ms > b))
}
