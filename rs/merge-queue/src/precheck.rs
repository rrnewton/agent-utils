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
//!
//! Checks too slow to wait for go in `precheck.background`: a worker thread runs that command on
//! every queued head, one at a time, in its own checkout (`<state>/scratch/precheck-background/`)
//! and never in the run's way. Its verdicts are kept like the others. A change whose background
//! verdict is a failure is ejected when its batch is built; one not checked yet joins the batch,
//! and its verdict counts when it comes. A red batch whose every failure is a step the
//! precheck reproduces (`precheck.covers`) ejects the changes that failed their precheck, and
//! the rest go back as one batch, not one batch each.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex, OnceLock, PoisonError};
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

/// The checkout [`check`] runs in, under `<state>/scratch/`.
const CHECKOUT: &str = "precheck";

/// The checkout the background worker runs in, so it never shares a working tree with
/// [`check`].
const BACKGROUND_CHECKOUT: &str = "precheck-background";

/// How long a head whose background check could not run waits before it is checked again.
const RETRY_COULD_NOT_RUN: Duration = Duration::from_secs(30 * 60);

/// Set by [`stop_background`]: the worker kills its check and takes no more.
static BACKGROUND_STOP: AtomicBool = AtomicBool::new(false);

/// Whether the worker is running a check.
static BACKGROUND_BUSY: AtomicBool = AtomicBool::new(false);

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
    let target = Target {
        pr,
        head_ref,
        head,
        base,
    };
    Some(check_in(
        state_root,
        git,
        &target,
        command,
        config.precheck.timeout(),
        CHECKOUT,
    ))
}

/// The change one check is for.
struct Target<'a> {
    pr: i64,
    head_ref: &'a str,
    head: &'a str,
    base: &'a str,
}

/// Run `command` on `target` in the checkout `<state>/scratch/<checkout>`, under the deadline
/// `limit`, and keep a pass or a failure.
fn check_in(
    state_root: &Path,
    git: &Git,
    target: &Target<'_>,
    command: &str,
    limit: Duration,
    checkout: &str,
) -> Verdict {
    let started = Instant::now();
    let head = target.head;
    let checkout = state_root.join("scratch").join(checkout);
    let verdict = match prepare(git, &checkout, head) {
        Ok(()) => run(limit, &checkout, command, target),
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
    verdict
}

/// The kept verdict of the background command (`precheck.background`) on change `pr` at
/// `head`. Without one, the head is handed to this process's background worker (once: a head
/// already waiting there, or one that could not be checked by this process, is not handed over
/// again) and this says `None` until its verdict is kept. `None` too when no background command
/// is configured.
pub fn background(
    state_root: &Path,
    config: &Config,
    git: &Git,
    pr: i64,
    head_ref: &str,
    head: &str,
    base: &str,
) -> Option<Verdict> {
    let command = config.precheck.background.trim();
    if command.is_empty() {
        return None;
    }
    if let Some(verdict) = kept(state_root, head, command) {
        return Some(verdict);
    }
    worker().ask(Job {
        state_root: state_root.to_owned(),
        git: git.clone(),
        command: command.to_owned(),
        limit: config.precheck.background_timeout(),
        pr,
        head_ref: head_ref.to_owned(),
        head: head.to_owned(),
        base: base.to_owned(),
    });
    None
}

/// The kept failure of either precheck command on `head`, the quick one first.
pub fn kept_failure(state_root: &Path, config: &Config, head: &str) -> Option<Verdict> {
    [&config.precheck.command, &config.precheck.background]
        .into_iter()
        .map(|command| command.trim())
        .filter(|command| !command.is_empty())
        .find_map(|command| {
            kept(state_root, head, command).filter(|verdict| verdict.outcome == Outcome::Failed)
        })
}

/// One head for the background worker.
struct Job {
    state_root: PathBuf,
    git: Git,
    command: String,
    limit: Duration,
    pr: i64,
    head_ref: String,
    head: String,
    base: String,
}

/// Verdict paths handed to the worker and not kept yet: `None` while the check waits or runs,
/// and the time it ended for one that could not run.
type Asked = Mutex<BTreeMap<PathBuf, Option<Instant>>>;

/// This process's background worker: one thread, one check at a time, in the order asked.
struct Worker {
    jobs: Mutex<mpsc::Sender<Job>>,
    /// A head waiting or running is not handed over again, nor one that could not be checked
    /// in the last [`RETRY_COULD_NOT_RUN`].
    asked: &'static Asked,
}

fn worker() -> &'static Worker {
    static WORKER: OnceLock<Worker> = OnceLock::new();
    WORKER.get_or_init(|| {
        static ASKED: Asked = Mutex::new(BTreeMap::new());
        let (jobs, queue) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new()
            .name("precheck-background".to_owned())
            .spawn(move || work(&queue, &ASKED));
        if let Err(error) = spawned {
            eprintln!("merge-queue: cannot start the background precheck worker: {error}");
        }
        Worker {
            jobs: Mutex::new(jobs),
            asked: &ASKED,
        }
    })
}

impl Worker {
    fn ask(&self, job: Job) {
        let path = verdict_path(&job.state_root, &job.head, &job.command);
        {
            let mut asked = self.asked.lock().unwrap_or_else(PoisonError::into_inner);
            match asked.get(&path) {
                Some(None) => return,
                Some(Some(ended)) if ended.elapsed() < RETRY_COULD_NOT_RUN => return,
                _ => {}
            }
            asked.insert(path, None);
        }
        let _ = self
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(job);
    }
}

/// Stop the background worker before this process exits: its check (in a process group of its
/// own, which would outlive the process) is killed, and it takes no more. Waits up to two
/// seconds for that; returns at once when no check is running.
pub fn stop_background() {
    BACKGROUND_STOP.store(true, Ordering::SeqCst);
    let started = Instant::now();
    while BACKGROUND_BUSY.load(Ordering::SeqCst) && started.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Whether a check in progress should be killed now: the process is stopping.
fn stopping() -> bool {
    crate::interrupt::requested() || BACKGROUND_STOP.load(Ordering::SeqCst)
}

/// Check each head handed over, in order, until the process is asked to stop.
fn work(queue: &mpsc::Receiver<Job>, asked: &Asked) {
    while let Ok(job) = queue.recv() {
        if stopping() {
            return;
        }
        let path = verdict_path(&job.state_root, &job.head, &job.command);
        let target = Target {
            pr: job.pr,
            head_ref: &job.head_ref,
            head: &job.head,
            base: &job.base,
        };
        let outcome = match kept(&job.state_root, &job.head, &job.command) {
            Some(verdict) => verdict.outcome,
            None => {
                BACKGROUND_BUSY.store(true, Ordering::SeqCst);
                let outcome = check_in(
                    &job.state_root,
                    &job.git,
                    &target,
                    &job.command,
                    job.limit,
                    BACKGROUND_CHECKOUT,
                )
                .outcome;
                BACKGROUND_BUSY.store(false, Ordering::SeqCst);
                outcome
            }
        };
        // A pass or a failure is kept on disk, where the next ask finds it. One that could not
        // run is not handed over again for a while.
        let mut asked = asked.lock().unwrap_or_else(PoisonError::into_inner);
        if outcome == Outcome::CouldNotRun {
            asked.insert(path, Some(Instant::now()));
        } else {
            asked.remove(&path);
        }
    }
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

/// Run `command` for `target` in `checkout` under the deadline `limit`.
fn run(limit: Duration, checkout: &Path, command: &str, target: &Target<'_>) -> Verdict {
    let started = Instant::now();
    let mut verdict = Verdict {
        schema: VERDICT_SCHEMA.to_owned(),
        head: target.head.to_owned(),
        command: command.to_owned(),
        outcome: Outcome::CouldNotRun,
        exit: None,
        reason: String::new(),
        output: String::new(),
        elapsed_ms: 0,
        checked_at: String::new(),
    };
    let log = checkout.with_extension("log");
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
                .env("MQ_PR", target.pr.to_string())
                .env("MQ_HEAD", target.head)
                .env("MQ_HEAD_REF", target.head_ref)
                .env("MQ_BASE", target.base)
                .stdin(Stdio::null())
                .stdout(file)
                .stderr(err)
                .process_group(0)
                .spawn()
                .map_err(|error| format!("cannot start sh: {error}"))?
        };
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
            // A daemon asked to stop, or a process about to exit, does not wait out the deadline.
            if stopping() {
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
