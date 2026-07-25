//! CLI entry point for safe-ci-dag-runner.

use std::process::ExitCode;

use safe_ci_dag_runner::{help_text, PROG, VERSION};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--version") {
        println!("{PROG} {VERSION}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{}", help_text());
        return ExitCode::SUCCESS;
    }

    // No subcommand yet: the runner is still being ported.
    eprintln!("{PROG} {VERSION}: no command given (runner port in progress).");
    ExitCode::SUCCESS
}
