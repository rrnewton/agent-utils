//! Executable entry point for the Rust edition of wrkviz.

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::ExitCode::from(wrkviz::cli::run(&args))
}
