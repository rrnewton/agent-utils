//! `merge-queue` executable entry point.

fn main() {
    std::process::exit(merge_queue::cli::main(std::env::args_os().skip(1)));
}
