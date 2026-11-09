//! Public command entrypoint for persistent foreign-harness workers.
fn main() {
    std::process::exit(agentctl::foreign::cli::main_entry());
}
