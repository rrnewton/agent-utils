//! The precheck: a quick command every submitted head must pass before it is queued.
//!
//! A change that fails a cheap, deterministic check (a formatter check, say) sinks every batch
//! it joins, and the batches built on that batch, for the length of a validation. The precheck
//! runs such a check once, on the submitted head alone, when the submission is applied: a head
//! that fails it is ejected at once with the command's output, before any batch is built, and
//! its submitter is notified. A head that passes is queued as usual.
//!
//! ```text
//! <state>/scratch/precheck/                       the checkout every precheck runs in, reused
//! <state>/scratch/precheck.log                    the output of the last one
//! <state>/precheck/verdicts/<head>-<digest>.json  a verdict per head and command
//! ```
//!
//! `precheck.command` is a shell command run in that checkout with `MQ_PR`, `MQ_HEAD`,
//! `MQ_HEAD_REF` and `MQ_BASE` (the base commit) set. Exit 0 passes and any other exit fails,
//! except the exits that say the check could not run at all: 75 (a temporary refusal), 126 and
//! 127 (a command that cannot execute or is not found), death by a signal, a command that cannot
//! be started, one still running at `precheck.timeout_secs` (its process group is killed), and
//! one a stop request interrupts. A check that could not run says nothing about the change, so
//! the submission is queued anyway and the change's history says why the precheck was skipped.
//!
//! A pass or a failure is kept per head and command, so resubmitting a head costs nothing and
//! gets the same answer; a check that could not run is tried again next time. `submit
//! --skip-precheck` queues a head without the check, for a refusal that is wrong, and keeps
//! that as the head's verdict.
//!
//! A run checks every change again before it joins a batch, which answers at once from the kept
//! verdict for a head checked at submission, and catches a change queued before the precheck
//! was configured: one that fails is ejected there, before the batch is built.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::git::Git;
use crate::model::now;
use crate::plan::short;
use crate::state::{write_json, Config};

/// [`Verdict::schema`].
pub const VERDICT_SCHEMA: &str = "merge-queue-precheck/v1";

/// Bytes of a check's output kept with its verdict (its end, where a failure is reported).
const OUTPUT_KEPT: usize = 16 * 1024;

/// Lines of a failed check's output quoted in the refusal.
const LINES_QUOTED: usize = 40;

/// Days a verdict is kept.
const VERDICTS_KEEP_DAYS: i64 = 14;

/// How one precheck ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// Exit 0.
    Passed,
    /// Any other exit that is not one of the could-not-run exits.
    Failed,
    /// The check could not run (see the module documentation); it says nothing about the change.
    CouldNotRun,
    /// Not run: the head was submitted with `--skip-precheck`, which is kept like a verdict so a
    /// later check of the same head (before a batch is built) does not refuse it either.
    Skipped,
}

/// The result of one precheck of one head.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    /// [`VERDICT_SCHEMA`].
    pub schema: String,
    /// The head checked.
    pub head: String,
    /// The command that checked it.
    pub command: String,
    /// How it ended.
    pub outcome: Outcome,
    /// Its exit status, when it exited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    /// For [`Outcome::CouldNotRun`], why.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// The end of its output (stdout and stderr together), checkout paths shortened to
    /// repository-relative ones.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
    /// Wall time, in milliseconds.
    pub elapsed_ms: u64,
    /// RFC 3339 UTC time it finished.
    pub checked_at: String,
}

impl Verdict {
    /// Why the change at this head is refused: a summary line, then the end of the output.
    pub fn refusal(&self, pr: i64, kept: &Path) -> String {
        let lines: Vec<&str> = self.output.lines().collect();
        let quoted = &lines[lines.len().saturating_sub(LINES_QUOTED)..];
        let mut text = format!(
            "#{pr}: the precheck failed on {} ({} after {:.1}s): {}",
            short(&self.head),
            self.exit.map_or_else(
                || "no exit status".to_owned(),
                |code| format!("exit {code}")
            ),
            self.elapsed_ms as f64 / 1000.0,
            self.command
        );
        for line in quoted {
            text.push_str("\n    ");
            text.push_str(line);
        }
        if quoted.len() < lines.len() {
            text.push_str(&format!(
                "\n    (the last {} of {} lines kept; the rest is in {})",
                quoted.len(),
                lines.len(),
                kept.display()
            ));
        }
        text.push_str(
            "\nFix the head and submit it again; `submit --skip-precheck` queues it without the \
             check if this refusal is wrong.",
        );
        text
    }
}

/// The precheck directory in the state directory `state_root`.
pub fn dir(state_root: &Path) -> PathBuf {
    state_root.join("precheck")
}

/// Where the verdict of `command` on `head` is kept.
pub fn verdict_path(state_root: &Path, head: &str, command: &str) -> PathBuf {
    dir(state_root)
        .join("verdicts")
        .join(format!("{head}-{:016x}.json", digest(command)))
}

/// FNV-1a: a stable digest of a command, for verdict file names (not for security).
fn digest(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The kept verdict of `command` on `head`, if there is one.
pub fn kept(state_root: &Path, head: &str, command: &str) -> Option<Verdict> {
    let text = std::fs::read_to_string(verdict_path(state_root, head, command)).ok()?;
    let verdict: Verdict = serde_json::from_str(&text).ok()?;
    (verdict.schema == VERDICT_SCHEMA && verdict.head == head && verdict.command == command)
        .then_some(verdict)
}

/// Keep a [`Outcome::Skipped`] verdict for `head` under the configured command: `submit
/// --skip-precheck` by `by`. Nothing when no precheck is configured.
pub fn skip(state_root: &Path, config: &Config, head: &str, by: &str) -> Result<(), String> {
    let command = config.precheck.command.trim();
    if command.is_empty() {
        return Ok(());
    }
    let verdict = Verdict {
        schema: VERDICT_SCHEMA.to_owned(),
        head: head.to_owned(),
        command: command.to_owned(),
        outcome: Outcome::Skipped,
        exit: None,
        reason: if by.is_empty() {
            "skipped at submission (--skip-precheck)".to_owned()
        } else {
            format!("skipped at submission by {by} (--skip-precheck)")
        },
        output: String::new(),
        elapsed_ms: 0,
        checked_at: now(),
    };
    write_json(&verdict_path(state_root, head, command), &verdict)
}

/// Check change `pr` at `head` (a commit in `git`'s repository) with the configured precheck.
/// `None` when no precheck is configured; otherwise the kept verdict for this head and command,
/// or a new one. A checkout that cannot be made is a check that could not run.
pub fn check(
    state_root: &Path,
    config: &Config,
    git: &Git,
    pr: i64,
    head_ref: &str,
    head: &str,
    base: &str,
) -> Option<Verdict> {
    let command = config.precheck.command.trim();
    if command.is_empty() {
        return None;
    }
    if let Some(verdict) = kept(state_root, head, command) {
        return Some(verdict);
    }
    let started = Instant::now();
    let checkout = state_root.join("scratch").join("precheck");
    let verdict = match prepare(git, &checkout, head) {
        Ok(()) => run(config, &checkout, command, pr, head_ref, head, base),
        Err(error) => Verdict {
            schema: VERDICT_SCHEMA.to_owned(),
            head: head.to_owned(),
            command: command.to_owned(),
            outcome: Outcome::CouldNotRun,
            exit: None,
            reason: format!("cannot check out {}: {error}", short(head)),
            output: String::new(),
            elapsed_ms: elapsed_ms(started),
            checked_at: now(),
        },
    };
    if verdict.outcome != Outcome::CouldNotRun {
        // A verdict that cannot be kept is only checked again next time.
        let _ = write_json(&verdict_path(state_root, head, command), &verdict);
        prune(state_root);
    }
    Some(verdict)
}

/// Check out `head` in `checkout`, reusing the worktree there.
fn prepare(git: &Git, checkout: &Path, head: &str) -> Result<(), String> {
    if checkout.join(".git").exists() {
        let worktree = Git::new(checkout);
        if worktree
            .run(&["checkout", "--quiet", "--detach", "--force", head])
            .is_ok()
            && worktree.run(&["clean", "-ffdq"]).is_ok()
        {
            return Ok(());
        }
    }
    git.add_worktree(checkout, head)
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Run `command` in `checkout` under the configured deadline.
fn run(
    config: &Config,
    checkout: &Path,
    command: &str,
    pr: i64,
    head_ref: &str,
    head: &str,
    base: &str,
) -> Verdict {
    let started = Instant::now();
    let mut verdict = Verdict {
        schema: VERDICT_SCHEMA.to_owned(),
        head: head.to_owned(),
        command: command.to_owned(),
        outcome: Outcome::CouldNotRun,
        exit: None,
        reason: String::new(),
        output: String::new(),
        elapsed_ms: 0,
        checked_at: String::new(),
    };
    let log = checkout.with_file_name("precheck.log");
    let outcome = (|| -> Result<(Outcome, Option<i32>, String), String> {
        let file = std::fs::File::create(&log)
            .map_err(|error| format!("cannot create {}: {error}", log.display()))?;
        let err = file
            .try_clone()
            .map_err(|error| format!("cannot clone the output handle: {error}"))?;
        let mut child = {
            use std::os::unix::process::CommandExt;
            Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(checkout)
                .env("MQ_PR", pr.to_string())
                .env("MQ_HEAD", head)
                .env("MQ_HEAD_REF", head_ref)
                .env("MQ_BASE", base)
                .stdin(Stdio::null())
                .stdout(file)
                .stderr(err)
                .process_group(0)
                .spawn()
                .map_err(|error| format!("cannot start sh: {error}"))?
        };
        let limit = config.precheck.timeout();
        loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("cannot wait for the precheck: {error}"))?
            {
                return Ok(match status.code() {
                    Some(0) => (Outcome::Passed, Some(0), String::new()),
                    Some(code @ (75 | 126 | 127)) => (
                        Outcome::CouldNotRun,
                        Some(code),
                        format!("it exited {code}, which says it could not run"),
                    ),
                    Some(code) => (Outcome::Failed, Some(code), String::new()),
                    None => (
                        Outcome::CouldNotRun,
                        None,
                        "it was killed by a signal".to_owned(),
                    ),
                });
            }
            // A daemon asked to stop does not wait out the deadline.
            if crate::interrupt::requested() {
                crate::bounded::kill_group(&mut child);
                return Ok((
                    Outcome::CouldNotRun,
                    None,
                    "a stop request interrupted it".to_owned(),
                ));
            }
            if started.elapsed() > limit {
                crate::bounded::kill_group(&mut child);
                return Ok((
                    Outcome::CouldNotRun,
                    None,
                    format!(
                        "it was still running after {}s (its process group was killed)",
                        limit.as_secs()
                    ),
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    })();
    match outcome {
        Ok((outcome, exit, reason)) => {
            verdict.outcome = outcome;
            verdict.exit = exit;
            verdict.reason = reason;
        }
        Err(error) => verdict.reason = error,
    }
    verdict.output = read_output(&log, checkout);
    verdict.elapsed_ms = elapsed_ms(started);
    verdict.checked_at = now();
    verdict
}

/// The end of the output in `log`, at most [`OUTPUT_KEPT`] bytes, with `checkout`'s path
/// taken out of the paths it names.
fn read_output(log: &Path, checkout: &Path) -> String {
    let bytes = std::fs::read(log).unwrap_or_default();
    let start = bytes.len().saturating_sub(OUTPUT_KEPT);
    let text = String::from_utf8_lossy(&bytes[start..]);
    // A cut in the middle of a line leaves its tail: drop it.
    let text = if start > 0 {
        text.split_once('\n').map_or(&*text, |(_, rest)| rest)
    } else {
        &text
    };
    let prefix = format!("{}/", checkout.display());
    text.replace(&prefix, "").trim_end().to_owned()
}

/// Forget verdicts older than [`VERDICTS_KEEP_DAYS`].
fn prune(state_root: &Path) {
    let Ok(entries) = std::fs::read_dir(dir(state_root).join("verdicts")) else {
        return;
    };
    let keep = Duration::from_secs(u64::try_from(VERDICTS_KEEP_DAYS).unwrap_or(14) * 86_400);
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > keep);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(output: &str) -> Verdict {
        Verdict {
            schema: VERDICT_SCHEMA.to_owned(),
            head: "0123456789abcdef".to_owned(),
            command: "fmt --check".to_owned(),
            outcome: Outcome::Failed,
            exit: Some(1),
            reason: String::new(),
            output: output.to_owned(),
            elapsed_ms: 12_345,
            checked_at: now(),
        }
    }

    #[test]
    fn a_refusal_quotes_the_end_of_the_output_and_says_where_the_rest_is() {
        let short_output = verdict("Diff in src/a.rs:3:\n-x\n+y");
        let text = short_output.refusal(7, Path::new("/state/precheck/verdicts/v.json"));
        assert!(
            text.starts_with(
                "#7: the precheck failed on 0123456789 (exit 1 after 12.3s): fmt --check"
            ),
            "{text}"
        );
        assert!(
            text.contains("\n    Diff in src/a.rs:3:\n    -x\n    +y\n"),
            "{text}"
        );
        assert!(!text.contains("lines kept"), "{text}");
        assert!(text.contains("--skip-precheck"), "{text}");

        let long: Vec<String> = (1..=100).map(|n| format!("line {n}")).collect();
        let text = verdict(&long.join("\n")).refusal(7, Path::new("/kept.json"));
        assert!(!text.contains("\n    line 60\n"), "{text}");
        assert!(text.contains("\n    line 61\n"), "{text}");
        assert!(text.contains("\n    line 100\n"), "{text}");
        assert!(
            text.contains("(the last 40 of 100 lines kept; the rest is in /kept.json)"),
            "{text}"
        );
    }

    #[test]
    fn output_keeps_the_end_and_drops_checkout_paths() {
        let root = std::env::temp_dir().join(format!("mq-precheck-out-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let log = root.join("output.log");
        let checkout = root.join("checkout");
        let mut text = String::new();
        for n in 0..2000 {
            text.push_str(&format!("noise {n:04} {}\n", "x".repeat(20)));
        }
        text.push_str(&format!("Diff in {}/src/lib.rs:12:\n", checkout.display()));
        std::fs::write(&log, &text).unwrap();
        let output = read_output(&log, &checkout);
        assert!(output.len() <= OUTPUT_KEPT, "{}", output.len());
        assert!(output.ends_with("Diff in src/lib.rs:12:"), "{output}");
        // The first kept line is a whole line, not the tail of one cut in the middle.
        assert!(output.starts_with("noise "), "{}", &output[..40]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_digest_names_a_command_stably() {
        assert_eq!(digest("cargo fmt --check"), digest("cargo fmt --check"));
        assert_ne!(digest("cargo fmt --check"), digest("cargo fmt --check "));
        assert_eq!(
            verdict_path(Path::new("/s"), "abc", "x"),
            PathBuf::from(format!(
                "/s/precheck/verdicts/abc-{:016x}.json",
                digest("x")
            ))
        );
    }
}
