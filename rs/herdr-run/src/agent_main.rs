//! `herdr-agent` executable entry point.

fn main() {
    std::process::exit(herdr_run::agent_cli::main(std::env::args_os().skip(1)));
}
