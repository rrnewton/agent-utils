//! `agent-teams` binary entry point.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(agent_teams::cli::run(&args));
}
