fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(tick_hub::cli::run_from_env() as u8)
}
