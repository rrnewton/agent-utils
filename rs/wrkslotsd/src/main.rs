//! Executable entry point for the read-only observer slice.

fn main() -> std::process::ExitCode {
    // Rust ignores SIGPIPE, so a reader piped into `head` panics on
    // the closed stdout (exit 101). Restore the default disposition:
    // the process dies by SIGPIPE (shell status 141), as the Python
    // edition's callers expect.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    std::process::ExitCode::from(wrkslotsd::cli::run())
}
