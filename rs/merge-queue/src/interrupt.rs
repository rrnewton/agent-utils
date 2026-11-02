//! Stopping on SIGINT/SIGTERM without a verdict.
//!
//! A stop request is not evidence about any change. Once one arrives, a run cancels its
//! validations, starts no new landing or ejection, and returns with the changes still queued.
//! A process the queue started that dies of an outside kill signal (a service manager stopping
//! the whole control group, the OOM killer, an operator) is the same kind of non-evidence: it
//! stops the run with no verdict instead of reading as a red validation or a failed generator.

use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

fn flag() -> &'static Arc<AtomicBool> {
    static FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    FLAG.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

/// Route SIGINT and SIGTERM to the stop flag. A second signal while a stop is already under
/// way exits at once with status 1 (an operator who presses Ctrl-C twice means it).
pub fn install() {
    // `signal-hook` registers the handlers behind a safe API, so this crate keeps
    // `#![forbid(unsafe_code)]`. The conditional exit is registered first so it sees the flag
    // as it was before this signal set it.
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        let _ = signal_hook::flag::register_conditional_shutdown(signal, 1, Arc::clone(flag()));
        let _ = signal_hook::flag::register(signal, Arc::clone(flag()));
    }
}

/// Whether a stop has been requested.
pub fn requested() -> bool {
    flag().load(Ordering::SeqCst)
}

/// Clear the stop flag (a fresh daemon starts clear; test support).
pub fn reset() {
    flag().store(false, Ordering::SeqCst);
}

/// `Err` naming `what` when a stop has been requested: the caller must not go on to a verdict.
pub fn check(what: &str) -> Result<(), String> {
    if requested() {
        Err(format!(
            "interrupted by a stop request before {what}; no verdict, the changes stay queued"
        ))
    } else {
        Ok(())
    }
}

/// The signal that killed a process, when it is one an outside stop sends (SIGTERM, SIGINT,
/// SIGHUP, SIGKILL) rather than a crash of the process itself (SIGSEGV, SIGABRT, ...).
pub fn outside_kill(status: &ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal().filter(|signal| {
            [
                signal_hook::consts::SIGTERM,
                signal_hook::consts::SIGINT,
                signal_hook::consts::SIGHUP,
                signal_hook::consts::SIGKILL,
            ]
            .contains(signal)
        })
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}

/// The run-level error for a process killed by an outside signal.
pub fn killed(what: &str, signal: i32) -> String {
    format!(
        "{what} was killed by signal {signal}, which the queue did not send (a service stop, the \
         OOM killer or an operator); no verdict, the changes stay queued"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn only_outside_kill_signals_count() {
        let status = |script: &str| Command::new("sh").args(["-c", script]).status().unwrap();
        assert_eq!(
            outside_kill(&status("kill -TERM $$")),
            Some(signal_hook::consts::SIGTERM)
        );
        assert_eq!(
            outside_kill(&status("kill -KILL $$")),
            Some(signal_hook::consts::SIGKILL)
        );
        // A crash is the process's own failure, and an exit code is not a signal.
        assert_eq!(outside_kill(&status("kill -SEGV $$")), None);
        assert_eq!(outside_kill(&status("exit 143")), None);
    }
}
