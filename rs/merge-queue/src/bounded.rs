//! Child processes with a deadline. A network command that hangs (a stalled transport, a
//! credential helper waiting for budget) must not hold a run, a landing or the daemon's loop
//! for ever: past its deadline its whole process group is killed and the call fails, so the
//! caller's usual error path (a retry at the next cycle, a stopped run) takes over.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long a network command (`git fetch`, `ls-remote`, `push`, `gh`) may run by default
/// before it is killed and fails.
pub const NET_TIMEOUT: Duration = Duration::from_secs(300);

/// The end of the error [`output_within`] returns for a command it killed at its deadline.
const KILLED: &str = "(its process group was killed)";

/// Whether `error` is [`output_within`]'s report of a command killed at its deadline.
pub fn is_timeout(error: &str) -> bool {
    error.ends_with(KILLED)
}

/// Run `command` to completion within `timeout` and collect its output, with stdin closed.
/// Past `timeout` its process group is killed and the run fails, naming `what`.
pub fn output_within(
    mut command: Command,
    timeout: Duration,
    what: &str,
) -> Result<Output, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Its own process group, so a timeout kills everything it started (a wrapper, git's
        // transport helper, a credential helper).
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not run {what}: {error}"))?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                kill_group(&mut child);
                return Err(format!(
                    "{what} timed out after {}s {KILLED}",
                    timeout.as_secs_f64()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                kill_group(&mut child);
                return Err(format!("cannot wait for {what}: {error}"));
            }
        }
    };
    // A grandchild that keeps a pipe open must not hold the caller: the output is collected
    // with a bounded wait.
    let grace = Duration::from_secs(5);
    Ok(Output {
        status,
        stdout: stdout.recv_timeout(grace).unwrap_or_default(),
        stderr: stderr.recv_timeout(grace).unwrap_or_default(),
    })
}

/// Read a pipe to its end on a helper thread, so a chatty child never blocks on a full pipe.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    if let Some(mut pipe) = pipe {
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = pipe.read_to_end(&mut buffer);
            let _ = sender.send(buffer);
        });
    }
    receiver
}

/// Kill `child`'s whole process group (it was started as its own group leader), then the child
/// itself, and reap it. `kill(1)` is used because this crate forbids `unsafe` and so cannot call
/// `kill(2)` directly.
pub fn kill_group(child: &mut Child) {
    let group = format!("-{}", child.id());
    let _ = Command::new("kill")
        .args(["-KILL", "--", &group])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_that_finishes_in_time_returns_its_output() {
        let mut command = Command::new("sh");
        command.args(["-c", "echo out; echo err >&2; exit 3"]);
        let output = output_within(command, Duration::from_secs(10), "sh").unwrap();
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(String::from_utf8_lossy(&output.stdout), "out\n");
        assert_eq!(String::from_utf8_lossy(&output.stderr), "err\n");
    }

    #[test]
    fn a_command_past_its_deadline_is_killed_with_its_children() {
        let marker = std::env::temp_dir().join(format!("bounded-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        // The child starts a grandchild that would write the marker after 1 s.
        let mut command = Command::new("sh");
        command.args([
            "-c",
            &format!("(sleep 1; touch '{}') & sleep 30", marker.display()),
        ]);
        let started = Instant::now();
        let error = output_within(command, Duration::from_millis(300), "a sleeper").unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(8), "{error}");
        assert_eq!(
            error,
            "a sleeper timed out after 0.3s (its process group was killed)"
        );
        assert!(is_timeout(&error));
        assert!(!is_timeout(
            "git fetch failed (128): fatal: couldn't find remote ref"
        ));
        std::thread::sleep(Duration::from_millis(1500));
        assert!(!marker.exists(), "the grandchild outlived the timeout");
    }
}
