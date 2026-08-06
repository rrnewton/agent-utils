//! `herdr-run` executable entry point.

fn main() {
    std::process::exit(herdr_run::cli::main(std::env::args_os().skip(1)));
}
