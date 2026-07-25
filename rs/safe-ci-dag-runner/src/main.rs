//! CLI entry point for safe-ci-dag-runner (placeholder; full CLI lands in the cli module).

use std::process::ExitCode;

use safe_ci_dag_runner::{PROG, VERSION};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--version") {
        println!("{PROG} {VERSION}");
        return ExitCode::SUCCESS;
    }

    // Full subcommand surface (run/list/ascii/dot/json/quickstart) is being ported.
    eprintln!("{PROG} {VERSION}: no command given (runner port in progress).");
    ExitCode::SUCCESS
}
