//! `--measure-open RUNS`: the cold-open harness (see `timing` for what is measured).
//!
//! After start-up has settled (the pre-warmed panels have loaded: they are part of start-up,
//! not of opening the library), it performs the tray's "Open library" action `RUNS` times,
//! waits for each first paint, destroys the window, and prints one line per open and a summary
//! to standard output, then exits: status 0, or 1 when an open was not painted within 10 s or
//! any open took longer than `--budget-ms` (every open is cold: the window is destroyed in
//! between; the first is the coldest, being the process's first web view after the panels).

use std::io::Write as _;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use super::{Ui, windows};
use crate::timing::{Measurement, summarize};

/// How long start-up gets before the first open.
const SETTLE: Duration = Duration::from_secs(3);

/// How long one open may take to paint.
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
        say("cold-open: the app did not start");
        return 1;
    };
    let mut results: Vec<Measurement> = Vec::new();
    for run in 1..=runs {
        if let Err(e) = windows::open_library(app, &ui.timer) {
            say(&format!("cold-open: cannot open the library window: {e}"));
            return 1;
        }
        let Some(m) = ui.timer.wait(PAINT_LIMIT) else {
            say(&format!(
                "cold-open: run {run}: no first paint within {PAINT_LIMIT:?}"
            ));
            return 1;
        };
        say(&format!(
            "cold-open: run {run}: first paint {:.1} ms after the action (navigation {:.1} ms, \
             script {:.1} ms, report {:.1} ms)",
            m.paint_ms, m.navigation_ms, m.script_ms, m.report_ms
        ));
        results.push(m);
        if let Some(window) = app.get_webview_window(windows::LIBRARY) {
            let _ = window.destroy();
        }
        let gone = Instant::now();
        while app.get_webview_window(windows::LIBRARY).is_some()
            && gone.elapsed() < Duration::from_secs(2)
        {
            thread::sleep(Duration::from_millis(20));
        }
        thread::sleep(Duration::from_millis(300));
    }
    let Some(s) = summarize(&results) else {
        say("cold-open: no runs");
        return 1;
    };
    let verdict = match budget_ms {
        Some(budget) if s.max_ms > budget => format!("over the budget of {budget} ms"),
        Some(budget) => format!("within the budget of {budget} ms"),
        None => "no budget given".to_owned(),
    };
    say(&format!(
        "cold-open: summary: first {:.1} ms, later median {:.1} ms, max {:.1} ms over {} runs; {verdict}",
        s.first_ms, s.median_later_ms, s.max_ms, s.runs
    ));
    i32::from(budget_ms.is_some_and(|b| s.max_ms > b))
}
