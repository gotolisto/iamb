//! `--measure-palette RUNS`: the search palette's "visible in under 100 ms" harness
//! (`docs/test-strategy.md` §8.7), the same method as the fill-in form's (`measure_form`).
//!
//! After start-up has settled (the pre-warmed panels have loaded, and the palette panel has
//! painted once while hidden, `windows::prepaint_panel`), it hands the palette's flow a
//! `PaletteRequested` `RUNS` times, as the bridge does when the service hears the global
//! shortcut, stamped with the moment it is handed over. The flow asks for the keyboard and shows
//! the panel; the front end reports the panel's next paint after it was shown (`palette_painted`)
//! and the time from the request's arrival to that paint is the measurement. The palette is then
//! closed, which hides the panel, and the next one follows. The service is a stand-in that
//! answers every request at once ([`InstantService`](crate::form::InstantService)): the round
//! trip to a real worker (`PanelFocus`, about a millisecond on the same machine) is measured by
//! `tests/palette.rs` instead. Prints one line per palette and a summary, then exits: status 0,
//! or 1 when a palette was not painted within 10 s or any took longer than `--budget-ms`.

use std::io::Write as _;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use super::Ui;
use crate::palette::Pick;
use crate::timing::{Measurement, summarize, wall_ms};

/// How long start-up gets before the first palette.
const SETTLE: Duration = Duration::from_secs(3);

/// How long one palette may take to paint.
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

fn measure(app: &AppHandle, runs: usize, budget_ms: Option<f64>) -> i32 {
    thread::sleep(SETTLE);
    let Some(ui) = app.try_state::<Arc<Ui>>().map(|s| Arc::clone(&s)) else {
        say("palette-open: the app did not start");
        return 1;
    };
    // The panel paints once while hidden when its page reports it loaded (`palette_ready`); a
    // slow start-up gets longer, as a user's first palette would wait for its page too.
    let deadline = Instant::now() + PAINT_LIMIT;
    while !ui.palette.controller.prepainted() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    if !ui.palette.controller.prepainted() {
        say("palette-open: the panel's page did not report itself loaded; measuring anyway");
    }
    thread::sleep(Duration::from_millis(100));
    let mut results: Vec<Measurement> = Vec::new();
    for run in 1..=runs {
        ui.palette
            .controller
            .open_for(None, Instant::now(), wall_ms());
        let Some(t) = ui.palette.controller.timer().wait(PAINT_LIMIT) else {
            say(&format!(
                "palette-open: run {run}: no first paint within {PAINT_LIMIT:?}"
            ));
            return 1;
        };
        say(&format!(
            "palette-open: run {run}: first paint {:.1} ms after the request arrived (shown \
             after {:.1} ms, report {:.1} ms)",
            t.paint_ms, t.shown_ms, t.report_ms
        ));
        results.push(Measurement {
            paint_ms: t.paint_ms,
            report_ms: t.report_ms,
            navigation_ms: t.shown_ms,
            script_ms: t.shown_ms,
        });
        let Some(session) = ui.palette.controller.current().map(|o| o.session) else {
            say(&format!("palette-open: run {run}: the palette is not open"));
            return 1;
        };
        if let Err(e) = ui.palette.controller.pick(session, Pick::Close) {
            say(&format!(
                "palette-open: run {run}: close failed: {}",
                e.code
            ));
            return 1;
        }
        thread::sleep(Duration::from_millis(300));
    }
    let Some(s) = summarize(&results) else {
        say("palette-open: no runs");
        return 1;
    };
    let verdict = match budget_ms {
        Some(budget) if s.max_ms > budget => format!("over the budget of {budget} ms"),
        Some(budget) => format!("within the budget of {budget} ms"),
        None => "no budget given".to_owned(),
    };
    say(&format!(
        "palette-open: summary: first {:.1} ms, later median {:.1} ms, max {:.1} ms over {} \
         runs; {verdict}",
        s.first_ms, s.median_later_ms, s.max_ms, s.runs
    ));
    i32::from(budget_ms.is_some_and(|b| s.max_ms > b))
}
