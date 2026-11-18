//! `iamb-soak`: the soak harness and canary privacy scan (see `docs/test-strategy.md` §5).

use std::process::ExitCode;

fn main() -> ExitCode {
    iamb_testkit::soak::cli::main_with(std::env::args_os())
}
