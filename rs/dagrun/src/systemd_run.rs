//! Keeping `systemd-run` from rewriting the command it starts (#2316).
//!
//! `systemd-run` expands `${NAME}` anywhere in an argument, a whole-word `$NAME`, and `$$` (to
//! `$`) in the command line it is given. With `--scope` it does so itself, from the CALLER's
//! environment (the default in newer systemd); without `--scope` the service manager does it from
//! the unit's environment. An unknown or invalid name becomes the empty string. A launcher that
//! hands over text it did not write (a shell script, a user's command, its own argv) must turn
//! that off with `--expand-environment=no`, or the command that runs is not the one it was given.
//!
//! systemd before 254 refuses the option as unknown, and on those versions a scope never expanded
//! anything, so there the launch goes ahead without it after one note on stderr.

use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

/// The option that passes the command's arguments through byte for byte.
pub const NO_EXPAND_ENVIRONMENT: &str = "--expand-environment=no";

/// Longest the option probe may take. It needs no bus and starts no unit, so this only bounds a
/// broken binary.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether `program` accepts [`NO_EXPAND_ENVIRONMENT`]: `Some(true)` when it does, `Some(false)`
/// when it refuses the option, `None` when it could not be run at all. `--version` after the
/// option makes a supporting `systemd-run` print its version and exit without touching the bus.
pub fn probe_no_expand(program: &str) -> Option<bool> {
    let spawn = || {
        Command::new(program)
            .args([NO_EXPAND_ENVIRONMENT, "--version"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };
    // ETXTBSY: a just-written executable can still be open in a sibling thread's forked child for
    // a moment; that is not a refusal, so try again briefly.
    let mut attempt = 0;
    let mut child = loop {
        match spawn() {
            Ok(child) => break child,
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && attempt < 50 => {
                attempt += 1;
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    };
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.success()),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Some(false);
            }
        }
    }
}

/// The arguments to put before `--` on every `systemd-run` call whose command carries text this
/// process did not write: `[NO_EXPAND_ENVIRONMENT]` when the `systemd-run` on `PATH` accepts it,
/// otherwise nothing. Asked once per process; a refusal is reported once on stderr under `prefix`.
pub fn no_expand_args(prefix: &str) -> Vec<String> {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    let supported = *SUPPORTED.get_or_init(|| match probe_no_expand("systemd-run") {
        Some(true) => true,
        Some(false) => {
            eprintln!("{}", unsupported_note(prefix));
            false
        }
        // Not runnable at all: the launch itself fails and says so.
        None => false,
    });
    if supported {
        vec![NO_EXPAND_ENVIRONMENT.to_string()]
    } else {
        Vec::new()
    }
}

/// The one line said when `systemd-run` refuses the option.
pub fn unsupported_note(prefix: &str) -> String {
    format!(
        "{prefix} note: this systemd-run refuses {NO_EXPAND_ENVIRONMENT} (systemd older than \
         254), so it is left out; scopes there do not expand ${{...}} in arguments (#2316)."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn shim(dir: &std::path::Path, body: &str) -> String {
        let path = dir.join("systemd-run");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dagrun_systemd_run_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn probe_reports_support_refusal_and_absence() {
        let dir = scratch("probe");
        // A new systemd-run: accepts the option, then --version exits 0.
        let new = shim(
            &dir,
            "[ \"$1\" = --expand-environment=no ] && [ \"$2\" = --version ] && exit 0; exit 9",
        );
        assert_eq!(probe_no_expand(&new), Some(true));
        // An old one: getopt refuses the unknown option.
        let old = shim(
            &dir,
            "echo \"systemd-run: unrecognized option '$1'\" >&2; exit 1",
        );
        assert_eq!(probe_no_expand(&old), Some(false));
        assert_eq!(probe_no_expand(dir.join("missing").to_str().unwrap()), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn note_names_the_option_and_the_issue() {
        let note = unsupported_note("[dagrun]");
        assert!(note.starts_with("[dagrun] note: "), "{note}");
        assert!(note.contains("--expand-environment=no"), "{note}");
        assert!(note.contains("${...}"), "{note}");
        assert!(note.contains("#2316"), "{note}");
    }
}
