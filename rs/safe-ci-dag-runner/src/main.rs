//! CLI entry point for safe-ci-dag-runner.

use std::process::ExitCode;

use safe_ci_dag_runner::{cli, cpuset_allocator};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = if matches!(args.first().map(String::as_str), Some("__hold" | "__probe")) {
        cpuset_allocator::run(&args)
    } else {
        cli::run(&args)
    };
    ExitCode::from(code as u8)
}
