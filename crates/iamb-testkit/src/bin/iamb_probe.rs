//! `iamb-probe`: the probe window of the end-to-end latency measurement (task 2.7, test-only,
//! never shipped; see `docs/test-strategy.md` §6). Normally started by `iamb-compat latency`.

use std::process::ExitCode;

fn main() -> ExitCode {
    iamb_testkit::compat::probe::app::main_with(std::env::args_os())
}
