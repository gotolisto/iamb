//! `iamb-compat`: the compatibility-matrix harness of task 2.7 (see `docs/compatibility-run.md`).

use std::process::ExitCode;

fn main() -> ExitCode {
    iamb_testkit::compat::cli::main_with(std::env::args_os())
}
