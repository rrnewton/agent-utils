//! Public command entrypoint for persistent foreign-harness workers.
fn main() {
    let arguments: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    // The runner spawns every harness through this binary's inert
    // launch gate (`SpawnOwnedHarnessOptions::default` uses
    // `<current_exe> --chat-launch-gate`), exactly as the agentctl
    // binary does in its CLI entry.
    if arguments.first().map(String::as_str) == Some("--chat-launch-gate") {
        std::process::exit(agentctl::chat_launch::gate_main(&arguments[1..]));
    }
    std::process::exit(agentctl::foreign::cli::main_entry());
}
