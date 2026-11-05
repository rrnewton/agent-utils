//! Push notifications for item state changes, sent by the `serve` daemon.
//!
//! An item submitted with `--notify <name>` owes exactly one message per
//! state change — `landed`, `ejected`, `refusal-deferred`, `escalated`, `withdrawn` —
//! delivered as:
//!
//! ```text
//! agentctl send --deliver-while-working --file <report> -- <name>
//! ```
//!
//! (`--` ends the options, so a name that starts with `-` is never read as
//! one.) Each send runs in its own process group under a deadline
//! ([`DEFAULT_SEND_TIMEOUT`], `serve --notify-timeout-secs`); on expiry the
//! whole group is killed and the send counts as a failed attempt, so a hung
//! sink can never stall the daemon or `sync`.
//!
//! The sweep runs only under the queue writer lock ([`sweep`] takes it,
//! [`sweep_locked`] proves the caller holds it), and every write it makes
//! reloads the item from disk and changes only the notification fields
//! (`notify_delivery` plus one history event), so it never writes back a
//! stale snapshot over another writer's change.
//!
//! Dedup and retry state live on the item record itself
//! (`notify_delivery`): the transition key, the attempts made for it, and
//! whether it was delivered. The daemon derives the currently-owed
//! transition from the item's status every cycle and compares keys, so a
//! daemon restart or an extra cycle never resends a delivered transition,
//! and a failed send is retried on later cycles at most [`MAX_ATTEMPTS`]
//! times in total before it is logged and given up on. A notification
//! failure never fails the queue: the sweep logs it (daemon stderr plus the
//! item's history) and moves on.
//!
//! The agentctl invocation is configurable, mirroring how the validator and
//! `gh` commands are configured: `init --agentctl WORD...` (stored in
//! `config.notify.agentctl`) is primary, `serve --agentctl WORD...`
//! overrides it for one daemon, and the `MERGE_QUEUE_AGENTCTL` environment
//! variable (shell-style words) is a documented supplement between the two.
//! Tests inject a fake command through any of these seams. The registry the
//! sends address resolves the same way: `serve --agentctl-registry PATH`,
//! then `MERGE_QUEUE_AGENTCTL_REGISTRY`, then `config.notify.registry`
//! (`init --agentctl-registry PATH`); when one is set, sends run
//! `<command> --registry PATH send ...`. Without one, agentctl looks for
//! `.agentctl` in the daemon's working directory, which for a daemon started
//! in a bare queue repository is no registry at all.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::bounded::kill_group;
use crate::model::{now, Item, ItemStatus, NotifyDelivery};
use crate::state::{Config, Lock, State};

/// Most send attempts for one transition, across all daemon cycles.
pub const MAX_ATTEMPTS: u32 = 3;

/// Default deadline for one notification send.
pub const DEFAULT_SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// Environment supplement naming the agentctl command (shell-style words).
pub const AGENTCTL_ENV: &str = "MERGE_QUEUE_AGENTCTL";

/// Environment supplement naming the agentctl registry the sends address.
pub const AGENTCTL_REGISTRY_ENV: &str = "MERGE_QUEUE_AGENTCTL_REGISTRY";

/// One owed transition: a stable dedup key plus its verdict word.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Transition {
    /// Dedup key persisted on the item (`verdict:head:discriminator`).
    pub key: String,
    /// `landed`, `ejected`, `refusal-deferred`, `escalated` or `withdrawn`.
    pub verdict: &'static str,
}

fn history_count(item: &Item, kind: &str) -> usize {
    item.history.iter().filter(|e| e.kind == kind).count()
}

/// The transition `item` currently owes a notification for, if any.
///
/// Terminal statuses win over a stale refusal deferral. The key changes
/// whenever the underlying occurrence changes (a new head, a new landing
/// commit, another refusal, another withdrawal), so each occurrence is
/// notified once and only once.
pub fn desired_transition(item: &Item) -> Option<Transition> {
    match item.status {
        ItemStatus::Landed => Some(Transition {
            key: format!("landed:{}:{}", item.head, item.landed_as),
            verdict: "landed",
        }),
        ItemStatus::Ejected => Some(Transition {
            key: format!("ejected:{}:{}", item.head, history_count(item, "ejected")),
            verdict: "ejected",
        }),
        ItemStatus::Withdrawn => Some(Transition {
            key: format!(
                "withdrawn:{}:{}",
                item.head,
                history_count(item, "withdrawn")
            ),
            verdict: "withdrawn",
        }),
        ItemStatus::Queued if !item.flaky_hold_until.is_empty() => Some(Transition {
            key: format!(
                "{}:{}:{}",
                item.hold_kind(),
                item.head,
                history_count(item, item.hold_kind())
            ),
            verdict: item.hold_kind(),
        }),
        ItemStatus::Queued if !item.escalated.is_empty() => Some(Transition {
            key: format!(
                "escalated:{}:{}",
                item.head,
                history_count(item, "escalated")
            ),
            verdict: "escalated",
        }),
        ItemStatus::Queued if item.validation_refusals > 0 && !item.last_refusal.is_empty() => {
            Some(Transition {
                key: format!(
                    "refusal-deferred:{}:{}",
                    item.head, item.validation_refusals
                ),
                verdict: "refusal-deferred",
            })
        }
        ItemStatus::Queued => None,
    }
}

/// Resolve the agentctl command words: a non-empty flag override wins, then
/// the `MERGE_QUEUE_AGENTCTL` supplement, then the configured command, then
/// plain `agentctl` on `PATH`.
pub fn resolve_agentctl(
    flag: Option<&[String]>,
    env: Option<&str>,
    config: &Config,
) -> Vec<String> {
    if let Some(words) = flag.filter(|words| !words.is_empty()) {
        return words.to_vec();
    }
    if let Some(env) = env.filter(|env| !env.trim().is_empty()) {
        if let Ok(words) = shell_words::split(env) {
            if !words.is_empty() {
                return words;
            }
        }
    }
    if !config.notify.agentctl.is_empty() {
        return config.notify.agentctl.clone();
    }
    vec!["agentctl".to_owned()]
}

/// Resolve the agentctl registry: a non-empty flag override wins, then the
/// `MERGE_QUEUE_AGENTCTL_REGISTRY` supplement, then the configured registry;
/// `None` leaves agentctl's own default.
pub fn resolve_registry(
    flag: Option<&std::path::Path>,
    env: Option<&str>,
    config: &Config,
) -> Option<PathBuf> {
    if let Some(path) = flag.filter(|path| !path.as_os_str().is_empty()) {
        return Some(path.to_path_buf());
    }
    if let Some(env) = env.map(str::trim).filter(|env| !env.is_empty()) {
        return Some(PathBuf::from(env));
    }
    let configured = config.notify.registry.trim();
    (!configured.is_empty()).then(|| PathBuf::from(configured))
}

/// The configured agentctl-compatible sender.
#[derive(Clone, Debug)]
pub struct Notifier {
    /// Command words; the first is the program.
    pub command: Vec<String>,
    /// Registry passed as `--registry PATH` before `send`; `None` passes none.
    pub registry: Option<PathBuf>,
    /// Deadline for one send; on expiry the send's process group is killed
    /// and the send is a failed attempt.
    pub timeout: Duration,
}

impl Notifier {
    /// A sender running `command` with agentctl's default registry and the
    /// default deadline.
    pub fn new(command: Vec<String>) -> Self {
        Self {
            command,
            registry: None,
            timeout: DEFAULT_SEND_TIMEOUT,
        }
    }

    /// Build from the flag overrides (command words, registry), the
    /// environment supplements, and config.
    pub fn resolve(
        flag: Option<&[String]>,
        registry_flag: Option<&std::path::Path>,
        config: &Config,
    ) -> Self {
        let env = std::env::var(AGENTCTL_ENV).ok();
        let registry_env = std::env::var(AGENTCTL_REGISTRY_ENV).ok();
        Self::new(resolve_agentctl(flag, env.as_deref(), config)).with_registry(resolve_registry(
            registry_flag,
            registry_env.as_deref(),
            config,
        ))
    }

    /// The same sender addressing `registry` (`None`: agentctl's default).
    #[must_use]
    pub fn with_registry(mut self, registry: Option<PathBuf>) -> Self {
        self.registry = registry;
        self
    }

    /// The same sender with a different per-send deadline.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Send `report` to `name`. Any non-zero exit — including 126 (cannot
    /// execute) and 127 (no such command) — any spawn failure (a missing
    /// binary), and a send still running at [`Notifier::timeout`] (its
    /// process group is killed) is a failed send, never a success.
    pub fn send(&self, name: &str, report: &std::path::Path) -> Result<(), String> {
        let mut command = Command::new(&self.command[0]);
        command.args(&self.command[1..]);
        if let Some(registry) = &self.registry {
            command.arg("--registry").arg(registry);
        }
        command
            .arg("send")
            .arg("--deliver-while-working")
            .arg("--file")
            .arg(report)
            // Options end here: a name starting with `-` stays positional.
            .arg("--")
            .arg(name)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            // Own process group, so a timeout can kill everything it spawned.
            .process_group(0);
        let mut child = command
            .spawn()
            .map_err(|error| format!("cannot run {}: {error}", self.command[0]))?;
        // Drain stderr on a helper thread so a chatty sender never blocks on
        // a full pipe; a grandchild that keeps the pipe open cannot block us
        // either, because the result is collected with a bounded wait.
        let (sender, receiver) = std::sync::mpsc::channel::<Vec<u8>>();
        if let Some(mut pipe) = child.stderr.take() {
            std::thread::spawn(move || {
                let mut buffer = Vec::new();
                let _ = pipe.read_to_end(&mut buffer);
                let _ = sender.send(buffer);
            });
        }
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    kill_group(&mut child);
                    return Err(format!(
                        "{} send timed out after {}s (process group killed)",
                        self.command[0],
                        self.timeout.as_secs_f64()
                    ));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(error) => {
                    kill_group(&mut child);
                    return Err(format!("cannot wait for {}: {error}", self.command[0]));
                }
            }
        };
        if status.success() {
            Ok(())
        } else {
            let code = status
                .code()
                .map_or_else(|| "signal".to_owned(), |code| code.to_string());
            let stderr = receiver
                .recv_timeout(Duration::from_millis(500))
                .unwrap_or_default();
            let stderr = String::from_utf8_lossy(&stderr);
            let tail = stderr.trim().lines().last().unwrap_or_default();
            Err(format!(
                "{} send exited {code}{}",
                self.command[0],
                if tail.is_empty() {
                    String::new()
                } else {
                    format!(": {tail}")
                }
            ))
        }
    }
}

/// Newest run report (`runs/<id>/report.md`) in the state directory, if any.
fn latest_run_report(state: &State) -> Option<PathBuf> {
    let mut ids: Vec<PathBuf> = std::fs::read_dir(state.runs_dir())
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    ids.sort();
    ids.into_iter()
        .rev()
        .map(|dir| dir.join("report.md"))
        .find(|path| path.is_file())
}

/// Newest ejection report for `pr` (`runs/<id>/ejections/<pr>.md`), if any.
fn latest_ejection_report(state: &State, pr: i64) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(state.runs_dir())
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .rev()
        .map(|dir| dir.join("ejections").join(format!("{pr}.md")))
        .find(|path| path.is_file())
}

/// The report file a notification points agentctl at. It names the change,
/// the exact candidate (submitted head) SHA, the verdict, the landed base
/// commit or the failing tests / refusal detail, and the run report.
pub fn render_report(state: &State, item: &Item, transition: &Transition) -> String {
    let mut text = String::new();
    text.push_str("merge-queue notification\n");
    text.push_str(&format!("pr: #{}\n", item.pr));
    text.push_str(&format!("verdict: {}\n", transition.verdict));
    text.push_str(&format!("notify: {}\n", item.notify));
    text.push_str(&format!("candidate_sha: {}\n", item.head));
    text.push_str(&format!("head_ref: {}\n", item.head_ref));
    if !item.submitted_by.is_empty() {
        text.push_str(&format!("submitted_by: {}\n", item.submitted_by));
    }
    match transition.verdict {
        "landed" => {
            text.push_str(&format!("landed_main_sha: {}\n", item.landed_as));
        }
        "escalated" => {
            text.push_str(&format!("escalated: {}\n", item.escalated));
            text.push_str(&format!(
                "remedy: fix what kept validation from running, then `merge-queue retry {}`\n",
                item.pr
            ));
        }
        "refusal-deferred" => {
            text.push_str(&format!("refusal: {}\n", item.last_refusal));
            text.push_str(&format!("retry_after: {}\n", item.next_retry_at));
            text.push_str(&format!("refusals: {}\n", item.validation_refusals));
        }
        "held-green-flaky" => {
            text.push_str(&format!("flaky_tests: {}\n", item.flaky_nodes.join(", ")));
            text.push_str(&format!("hold_until: {}\n", item.flaky_hold_until));
            match &item.held_candidate {
                Some(candidate) => text.push_str(&format!(
                    "remedy: rule on the flaky pass: `merge-queue accept-flaky {} --by WHO --reason WHY` lands the validated commit {} if the base is still {}, and `merge-queue retry <pr>` validates it again; the hold expires and re-queues it at hold_until either way\n",
                    candidate
                        .prs()
                        .iter()
                        .map(i64::to_string)
                        .collect::<Vec<_>>()
                        .join(" "),
                    candidate.tip,
                    candidate.parent
                )),
                None => text.push_str(
                    "remedy: rule on the flaky pass, then `merge-queue retry <pr>` returns it to the queue; the hold expires and re-queues it at hold_until either way\n",
                ),
            }
        }
        "held-infra-blocked" => {
            text.push_str(&format!("blocked_tests: {}\n", item.flaky_nodes.join(", ")));
            text.push_str(&format!("hold_until: {}\n", item.flaky_hold_until));
            text.push_str(
                "remedy: the host refused every measurement of these tests on a run and on its rerun, so the change has no result for them; rule on it, then `merge-queue retry <pr>` returns it to the queue; the hold expires and re-queues it at hold_until either way\n",
            );
        }
        _ => {}
    }
    if !item.deciding_failed.is_empty() {
        text.push_str(&format!(
            "failed_tests: {}\n",
            crate::run::format_failed_results(&item.deciding_failed)
        ));
        let failed_ids: Vec<String> = item
            .deciding_failed
            .iter()
            .map(|r| r.id.to_string())
            .collect();
        text.push_str(&format!("failed: {}\n", failed_ids.join(", ")));
    }
    if let Some(event) = item
        .history
        .iter()
        .rev()
        .find(|event| event.kind == transition.verdict)
    {
        text.push_str(&format!("detail: {}\n", event.detail));
    }
    if transition.verdict == "ejected" {
        if let Some(path) = latest_ejection_report(state, item.pr) {
            text.push_str(&format!("ejection_report: {}\n", path.display()));
            if let Ok(body) = std::fs::read_to_string(&path) {
                text.push_str("\n--- ejection report ---\n");
                text.push_str(body.trim_end());
                text.push('\n');
            }
        }
    }
    // The deciding run's report, recorded on the item at its transition,
    // wins over whatever run is newest now.
    if !item.deciding_report.is_empty() {
        text.push_str(&format!("run_report: {}\n", item.deciding_report));
    } else {
        match latest_run_report(state) {
            Some(path) => text.push_str(&format!("run_report: {}\n", path.display())),
            None => text.push_str("run_report: none\n"),
        }
    }
    text.push_str(&format!("at: {}\n", now()));
    text
}

/// Directory holding notification report files.
pub fn reports_dir(state: &State) -> PathBuf {
    state.root.join("notifications")
}

fn report_path(state: &State, item: &Item, transition: &Transition) -> PathBuf {
    let safe = transition.key.replace([':', '/'], "-");
    reports_dir(state).join(format!("{}-{safe}.md", item.pr))
}

/// The transition `item` owes a send attempt for now, with that attempt's
/// number, or `None` when nothing is owed (no subscription, nothing to
/// report, already delivered, or out of attempts).
fn owed(item: &Item) -> Option<(Transition, u32)> {
    if item.notify.is_empty() {
        return None;
    }
    let transition = desired_transition(item)?;
    let prior = &item.notify_delivery;
    if prior.key == transition.key && (prior.delivered || prior.attempts >= MAX_ATTEMPTS) {
        return None;
    }
    let attempts = if prior.key == transition.key {
        prior.attempts + 1
    } else {
        1
    };
    Some((transition, attempts))
}

/// Why no send to `name` can ever succeed, if none can: agentctl names start with a lowercase
/// letter and hold 1-32 lowercase letters, digits or hyphens.
fn unaddressable(name: &str) -> Option<String> {
    let mut chars = name.chars();
    let valid = name.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    (!valid).then(|| {
        "it is not an agentctl agent name (a lowercase letter, then up to 31 lowercase letters, digits or hyphens)".to_owned()
    })
}

/// How many items owe a send attempt now. A lock-free read, so the daemon
/// can skip taking the writer lock when there is nothing to send.
pub fn pending(state: &State) -> Result<usize, String> {
    Ok(state
        .items()?
        .iter()
        .filter(|item| owed(item).is_some())
        .count())
}

/// Deliver every owed notification once, taking the queue writer lock for
/// the duration. Fails (without sending anything) when another writer holds
/// the lock; callers that already hold it use [`sweep_locked`]. Returns how
/// many notifications were delivered.
pub fn sweep(state: &State, config: &Config, notifier: &Notifier) -> Result<usize, String> {
    let lock = state.lock()?;
    sweep_locked(state, &lock, config, notifier)
}

/// Write the notification fields of item `pr` and nothing else.
///
/// The item is reloaded from disk (the caller holds the writer lock, so the
/// reload-modify-save is not interleaved with another lock-respecting
/// writer) and `update` sees the fresh record; it must only touch
/// `notify_delivery` and append history. Returns `Ok(None)` when the record
/// is gone or no longer owes `transition`, in which case nothing is written.
fn update_delivery(
    state: &State,
    pr: i64,
    transition: &Transition,
    update: impl FnOnce(&mut Item),
) -> Result<Option<Item>, String> {
    let Some(mut fresh) = state.item(pr)? else {
        return Ok(None);
    };
    if desired_transition(&fresh).as_ref() != Some(transition) {
        return Ok(None);
    }
    update(&mut fresh);
    state.save_item(&fresh)?;
    Ok(Some(fresh))
}

/// [`sweep`] under a writer lock the caller already holds (the `&Lock`
/// proves it).
///
/// Each item is handled independently: a failure is persisted (attempt
/// count and error), logged to stderr and the item's history, and never
/// stops the queue or the other notifications. Attempts for a transition
/// stop after [`MAX_ATTEMPTS`]; the give-up is logged the same way. Every
/// item is re-read from disk right before it is handled and again before
/// each write, and only its notification fields are written.
pub fn sweep_locked(
    state: &State,
    _lock: &Lock,
    config: &Config,
    notifier: &Notifier,
) -> Result<usize, String> {
    let _ = config;
    let mut delivered = 0usize;
    let numbers: Vec<i64> = state.items()?.into_iter().map(|item| item.pr).collect();
    for pr in numbers {
        if deliver_one(state, notifier, pr) {
            delivered += 1;
        }
    }
    Ok(delivered)
}

/// Deliver the notification item `pr` owes now, if it owes one, instead of at the daemon's next
/// sweep: a change refused at submission while a run holds the queue would otherwise reach its
/// submitter only when that run ends. The caller holds the writer lock, as for [`sweep_locked`]
/// (a submission is only ever applied under it). Returns whether it delivered one.
pub fn deliver_now(state: &State, config: &Config, pr: i64) -> bool {
    deliver_one(state, &Notifier::resolve(None, None, config), pr)
}

/// Handle item `pr` as [`sweep_locked`] handles each item. Returns whether it delivered a
/// notification.
fn deliver_one(state: &State, notifier: &Notifier, pr: i64) -> bool {
    // Fresh read: an earlier send in the same sweep may have taken a while.
    let item = match state.item(pr) {
        Ok(Some(item)) => item,
        Ok(None) => return false,
        Err(error) => {
            eprintln!("merge-queue notify: cannot read #{pr}: {error}");
            return false;
        }
    };
    let Some((transition, attempts)) = owed(&item) else {
        return false;
    };
    // A name agentctl can never accept (a hosting login such as `SomeUser`, say) is not
    // retried: one record says why, and the transition counts as given up.
    if let Some(why) = unaddressable(&item.notify) {
        let _ = update_delivery(state, pr, &transition, |fresh| {
            fresh.notify_delivery = NotifyDelivery {
                key: transition.key.clone(),
                attempts: MAX_ATTEMPTS,
                delivered: false,
                last_error: why.clone(),
            };
            fresh.log(
                "notify-skipped",
                format!("{} not sent to {}: {why}", transition.verdict, item.notify),
            );
        });
        eprintln!(
            "merge-queue notify: #{pr} {} not sent to {}: {why}",
            transition.verdict, item.notify
        );
        return false;
    }
    // Persist the attempt before sending: a daemon crash mid-send can
    // then only lose the delivery mark, never the retry bound.
    let recorded = update_delivery(state, pr, &transition, |fresh| {
        fresh.notify_delivery = NotifyDelivery {
            key: transition.key.clone(),
            attempts,
            delivered: false,
            last_error: String::new(),
        };
    });
    let item = match recorded {
        Ok(Some(item)) => item,
        Ok(None) => return false,
        Err(error) => {
            eprintln!("merge-queue notify: cannot record attempt for #{pr}: {error}");
            return false;
        }
    };
    let path = report_path(state, &item, &transition);
    let outcome = std::fs::create_dir_all(reports_dir(state))
        .map_err(|error| format!("cannot create notifications dir: {error}"))
        .and_then(|()| {
            std::fs::write(&path, render_report(state, &item, &transition))
                .map_err(|error| format!("cannot write {}: {error}", path.display()))
        })
        .and_then(|()| notifier.send(&item.notify, &path));
    // Record the outcome on a fresh read: only mark the key we actually
    // attempted, and only if it is still the owed transition.
    match outcome {
        Ok(()) => {
            let marked = update_delivery(state, pr, &transition, |fresh| {
                fresh.notify_delivery.delivered = true;
                fresh.notify_delivery.last_error.clear();
                fresh.log(
                    "notified",
                    format!(
                        "{} sent to {} (report {})",
                        transition.verdict,
                        item.notify,
                        path.display()
                    ),
                );
            });
            match marked {
                Ok(Some(_)) => return true,
                Ok(None) => {}
                Err(error) => {
                    eprintln!("merge-queue notify: cannot record delivery for #{pr}: {error}");
                }
            }
        }
        Err(error) => {
            let gave_up = attempts >= MAX_ATTEMPTS;
            let suffix = if gave_up { "; giving up" } else { "" };
            let _ = update_delivery(state, pr, &transition, |fresh| {
                fresh.notify_delivery.last_error = error.clone();
                fresh.log(
                    "notify-failed",
                    format!(
                        "{} to {} failed (attempt {attempts}/{MAX_ATTEMPTS}){suffix}: {error}",
                        transition.verdict, item.notify
                    ),
                );
            });
            eprintln!(
                "merge-queue notify: #{pr} {} to {} failed (attempt {attempts}/{MAX_ATTEMPTS}){suffix}: {error}",
                transition.verdict, item.notify
            );
        }
    }
    false
}

#[cfg(test)]
mod unit {
    use super::*;
    use crate::model::ItemStatus as Status;

    fn item(status: Status) -> Item {
        Item {
            pr: 7,
            head_ref: "feature/x".to_owned(),
            head: "a".repeat(40),
            priority: 100,
            submitted_at: "2026-01-01T00:00:00Z".to_owned(),
            submitted_by: "lane-fictional".to_owned(),
            notify: "lane-fictional".to_owned(),
            title: String::new(),
            after: vec![],
            solo: false,
            prioritized: false,
            exact: false,
            requires: vec![],
            claim: None,
            source: String::new(),
            close_pending: false,
            validation_refusals: 0,
            suspect_reruns: 0,
            next_retry_at: String::new(),
            last_refusal: String::new(),
            flaky_nodes: Vec::new(),
            flaky_hold_until: String::new(),
            hold_verdict: String::new(),
            held_candidate: None,
            accepted: None,
            escalated: String::new(),
            deciding_report: String::new(),
            deciding_failed: vec![],
            notify_delivery: NotifyDelivery::default(),
            status,
            landed_as: "b".repeat(40),
            history: vec![],
        }
    }

    #[test]
    fn desired_transition_covers_each_verdict() {
        assert_eq!(
            desired_transition(&item(Status::Landed)).unwrap().verdict,
            "landed"
        );
        let mut ejected = item(Status::Ejected);
        ejected.log("ejected", "failed-tests: boom");
        assert_eq!(desired_transition(&ejected).unwrap().verdict, "ejected");
        let mut withdrawn = item(Status::Withdrawn);
        withdrawn.log("withdrawn", "by submitter");
        assert_eq!(desired_transition(&withdrawn).unwrap().verdict, "withdrawn");
        assert!(desired_transition(&item(Status::Queued)).is_none());
        let mut deferred = item(Status::Queued);
        deferred.validation_refusals = 2;
        deferred.last_refusal = "exit 75 COULD_NOT_RUN".to_owned();
        let transition = desired_transition(&deferred).unwrap();
        assert_eq!(transition.verdict, "refusal-deferred");
        assert!(transition.key.ends_with(":2"), "{}", transition.key);
        // A later refusal is a new transition; the landed key differs too.
        deferred.validation_refusals = 3;
        assert_ne!(desired_transition(&deferred).unwrap().key, transition.key);
        // Escalation to the owner outranks the refusal it ended.
        deferred.escalated = "validation did not run 3 time(s)".to_owned();
        deferred.log("escalated", "validation did not run 3 time(s)");
        let escalated = desired_transition(&deferred).unwrap();
        assert_eq!(escalated.verdict, "escalated");
        assert!(escalated.key.starts_with("escalated:"), "{}", escalated.key);
    }

    #[test]
    fn notify_body_lists_failed_ids_and_links_deciding_run() {
        let mut ejected = item(Status::Ejected);
        ejected.deciding_report = "/state/runs/run-deciding/report.md".to_owned();
        ejected.deciding_failed = vec![crate::tests::TestResult {
            id: crate::tests::TestId::step("tests/a"),
            outcome: crate::tests::Outcome::Fail,
            duration_ms: Some(12),
            log: None,
        }];
        ejected.log("ejected", "failed-tests: boom");
        let transition = desired_transition(&ejected).unwrap();
        let state = crate::state::State::at("/nonexistent-notify-unit");
        let text = render_report(&state, &ejected, &transition);
        assert!(text.contains("failed: tests/a"), "{text}");
        assert!(text.contains("tests/a (fail, 12ms)"), "{text}");
        assert!(
            text.contains("run_report: /state/runs/run-deciding/report.md"),
            "{text}"
        );
    }

    #[test]
    fn agentctl_resolution_prefers_flag_then_env_then_config() {
        let mut config = Config::default();
        config.notify.agentctl = vec!["configured".to_owned()];
        let flag = vec!["flagged".to_owned(), "--fast".to_owned()];
        assert_eq!(
            resolve_agentctl(Some(&flag), Some("from-env"), &config),
            flag
        );
        assert_eq!(
            resolve_agentctl(None, Some("from-env --x"), &config),
            vec!["from-env".to_owned(), "--x".to_owned()]
        );
        assert_eq!(
            resolve_agentctl(None, None, &config),
            vec!["configured".to_owned()]
        );
        assert_eq!(
            resolve_agentctl(None, Some("  "), &Config::default()),
            vec!["agentctl".to_owned()]
        );
    }

    #[test]
    fn registry_resolution_prefers_flag_then_env_then_config() {
        use std::path::Path;
        let mut config = Config::default();
        config.notify.registry = "/team/.agentctl".to_owned();
        assert_eq!(
            resolve_registry(Some(Path::new("/flag/.agentctl")), Some("/env"), &config),
            Some(PathBuf::from("/flag/.agentctl"))
        );
        assert_eq!(
            resolve_registry(None, Some(" /env/.agentctl "), &config),
            Some(PathBuf::from("/env/.agentctl"))
        );
        assert_eq!(
            resolve_registry(Some(Path::new("")), Some("  "), &config),
            Some(PathBuf::from("/team/.agentctl"))
        );
        // Nothing set: no --registry, so agentctl keeps its own default.
        assert_eq!(resolve_registry(None, None, &Config::default()), None);
    }

    #[test]
    fn a_registry_goes_before_send_as_a_global_option() {
        let dir = std::env::temp_dir().join(format!(
            "merge-queue-notify-registry-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let report = dir.join("r.md");
        std::fs::write(&report, "x\n").unwrap();
        let log = dir.join("argv");
        let recorder = Notifier::new(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            format!("printf '%s\\n' \"$@\" > {}", log.display()),
            "sh".to_owned(),
        ])
        .with_registry(Some(PathBuf::from("/work/team/.agentctl")));
        recorder.send("lane", &report).unwrap();
        let argv: Vec<String> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        let report_arg = report.display().to_string();
        assert_eq!(
            argv,
            vec![
                "--registry",
                "/work/team/.agentctl",
                "send",
                "--deliver-while-working",
                "--file",
                report_arg.as_str(),
                "--",
                "lane"
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_and_refused_sends_are_failures_never_success() {
        let missing = Notifier::new(vec!["/definitely/missing/agentctl".to_owned()]);
        let report = std::env::temp_dir().join("merge-queue-notify-unit.md");
        std::fs::write(&report, "x\n").unwrap();
        assert!(missing.send("lane-fictional", &report).is_err());
        for code in [1, 75, 126, 127] {
            let refused = Notifier::new(vec![
                "sh".to_owned(),
                "-c".to_owned(),
                format!("exit {code}"),
                "sh".to_owned(),
            ]);
            assert!(
                refused.send("lane-fictional", &report).is_err(),
                "exit {code} must be a failed send"
            );
        }
        let ok = Notifier::new(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "exit 0".to_owned(),
            "sh".to_owned(),
        ]);
        assert!(ok.send("lane-fictional", &report).is_ok());
    }

    #[test]
    fn hung_send_times_out_and_kills_its_process_group() {
        let report =
            std::env::temp_dir().join(format!("merge-queue-notify-hang-{}.md", std::process::id()));
        std::fs::write(&report, "x\n").unwrap();
        // The shell's child `sleep` holds the stderr pipe too: only killing
        // the whole group (and a bounded stderr wait) keeps this prompt.
        let hung = Notifier::new(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "sleep 30; exit 0".to_owned(),
            "sh".to_owned(),
        ])
        .with_timeout(Duration::from_millis(300));
        let start = Instant::now();
        let error = hung.send("lane-fictional", &report).unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn name_follows_an_end_of_options_marker() {
        let dir =
            std::env::temp_dir().join(format!("merge-queue-notify-argv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let report = dir.join("r.md");
        std::fs::write(&report, "x\n").unwrap();
        let log = dir.join("argv");
        let recorder = Notifier::new(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            format!("printf '%s\\n' \"$@\" > {}", log.display()),
            "sh".to_owned(),
        ]);
        recorder.send("-lane", &report).unwrap();
        let argv: Vec<String> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        let report_arg = report.display().to_string();
        assert_eq!(
            argv,
            vec![
                "send",
                "--deliver-while-working",
                "--file",
                report_arg.as_str(),
                "--",
                "-lane"
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
