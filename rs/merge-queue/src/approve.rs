//! The submit approval gate: a shell command that must approve every
//! direct submission before it is queued (#1381 mq-submit-approval-gate).
//!
//! `submit.approval_command` runs fresh on every direct submit (sync
//! submissions are exempt; they answer to `require_approval_at_head`
//! instead), in the repository directory, with `MQ_PR`, `MQ_HEAD` (the
//! resolved head commit) and `MQ_BY` (the submitter, as given to
//! `submit --by`) set. Exit 0 approves. Any other exit refuses the
//! submission before anything is queued, naming the command's first
//! non-empty output line (or its exit status). A command that cannot
//! run at all (cannot start, or still running at
//! `submit.approval_timeout_secs`) also refuses, naming why: a gate
//! that cannot run must not silently queue. The verdict is never
//! cached per head, so a head refused before its approval arrives is
//! approved by a later submit once the command approves. This is a
//! cooperative guard (every submitter shares one Unix user), not a
//! security boundary.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::state::Config;

/// How one approval run ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Verdict {
    /// Exit 0.
    Approved,
    /// Refused, naming why.
    Refused(String),
}

/// Run the configured approval command for one direct submission.
/// `None` when no gate is configured (empty command).
pub fn check(config: &Config, state_root: &Path, pr: i64, head: &str, by: &str) -> Option<Verdict> {
    let command = config.submit.approval_command.trim();
    if command.is_empty() {
        return None;
    }
    Some(run(config, state_root, command, pr, head, by))
}

fn run(
    config: &Config,
    state_root: &Path,
    command: &str,
    pr: i64,
    head: &str,
    by: &str,
) -> Verdict {
    let scratch = state_root.join("scratch");
    if let Err(error) = std::fs::create_dir_all(&scratch) {
        return Verdict::Refused(format!(
            "approval command could not run: cannot create {}: {error}",
            scratch.display()
        ));
    }
    let log_path = scratch.join("approval.log");
    let log_file = match std::fs::File::create(&log_path) {
        Ok(file) => file,
        Err(error) => {
            return Verdict::Refused(format!(
                "approval command could not run: cannot create {}: {error}",
                log_path.display()
            ));
        }
    };
    let stderr_file = match log_file.try_clone() {
        Ok(file) => file,
        Err(error) => {
            return Verdict::Refused(format!(
                "approval command could not run: cannot clone its log file: {error}"
            ));
        }
    };
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(&config.repo)
        .env("MQ_PR", pr.to_string())
        .env("MQ_HEAD", head)
        .env("MQ_BY", by)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return Verdict::Refused(format!("approval command could not run: {error}"));
        }
    };
    let timeout = config.submit.approval_timeout();
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() <= timeout => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Verdict::Refused(format!(
                    "approval command could not run: still running after {}s (killed)",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                return Verdict::Refused(format!("approval command could not run: {error}"));
            }
        }
    };
    if status.success() {
        return Verdict::Approved;
    }
    let text = std::fs::read_to_string(&log_path).unwrap_or_default();
    let reason = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| crate::validate::cut_line(line).to_owned())
        .unwrap_or_else(|| {
            format!(
                "approval command exited {}",
                status
                    .code()
                    .map_or_else(|| "on a signal".to_owned(), |code| code.to_string())
            )
        });
    Verdict::Refused(reason)
}

#[cfg(test)]
mod unit {
    use super::*;
    use std::path::PathBuf;

    fn config(command: &str) -> (Config, PathBuf) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "merge-queue-approve-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut config = Config {
            repo: root.clone(),
            ..Config::default()
        };
        config.submit.approval_command = command.to_owned();
        (config, root)
    }

    #[test]
    fn no_command_is_no_gate() {
        let (config, root) = config("");
        assert_eq!(check(&config, &root, 1, "abc", "lane"), None);
    }

    #[test]
    fn exit_zero_approves_and_sees_pr_head_and_by() {
        let (config, root) =
            config("test \"$MQ_PR\" = 7 && test \"$MQ_HEAD\" = abc && test \"$MQ_BY\" = lane-1");
        assert_eq!(
            check(&config, &root, 7, "abc", "lane-1"),
            Some(Verdict::Approved)
        );
        // A different submitter is refused by the same command.
        match check(&config, &root, 7, "abc", "other") {
            Some(Verdict::Refused(reason)) => {
                assert!(reason.contains("exited"), "{reason}")
            }
            other => panic!("expected refusal, got {other:?}"),
        }
    }

    #[test]
    fn refusal_names_the_first_output_line() {
        let (config, root) = config("echo 'no approval for this head'; exit 3");
        assert_eq!(
            check(&config, &root, 7, "abc", "lane-1"),
            Some(Verdict::Refused("no approval for this head".to_owned()))
        );
    }

    #[test]
    fn a_command_that_cannot_run_refuses() {
        let (config, root) = config("exec /definitely/not/a/real/command-xyz");
        match check(&config, &root, 7, "abc", "lane-1") {
            Some(Verdict::Refused(reason)) => {
                assert!(reason.contains("not/a/real/command-xyz"), "{reason}")
            }
            other => panic!("expected refusal, got {other:?}"),
        }
    }
}
