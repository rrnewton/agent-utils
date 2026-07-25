//! CLI entry point for safe-ci-dag-runner.

use std::process::ExitCode;

use safe_ci_dag_runner::cli;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ExitCode::from(cli::run(&args) as u8)
}
