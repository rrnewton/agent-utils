//! Push notifications for item state changes, sent by the `serve` daemon.
//!
//! An item submitted with `--notify <name>` owes exactly one message per
//! state change — `landed`, `ejected`, `refusal-deferred`, `withdrawn` —
//! delivered as:
//!
//! ```text
//! agentctl send <name> --deliver-while-working --file <report>
//! ```
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
//! Tests inject a fake command through any of these seams.

use std::path::PathBuf;
use std::process::Command;

use crate::model::{now, Item, ItemStatus, NotifyDelivery};
use crate::state::{Config, State};

/// Most send attempts for one transition, across all daemon cycles.
pub const MAX_ATTEMPTS: u32 = 3;

/// Environment supplement naming the agentctl command (shell-style words).
pub const AGENTCTL_ENV: &str = "MERGE_QUEUE_AGENTCTL";

/// One owed transition: a stable dedup key plus its verdict word.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Transition {
    /// Dedup key persisted on the item (`verdict:head:discriminator`).
    pub key: String,
    /// `landed`, `ejected`, `refusal-deferred` or `withdrawn`.
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

/// The configured agentctl-compatible sender.
#[derive(Clone, Debug)]
pub struct Notifier {
    /// Command words; the first is the program.
    pub command: Vec<String>,
}

impl Notifier {
    /// Build from the flag override, the environment supplement, and config.
    pub fn resolve(flag: Option<&[String]>, config: &Config) -> Self {
        let env = std::env::var(AGENTCTL_ENV).ok();
        Self {
            command: resolve_agentctl(flag, env.as_deref(), config),
        }
    }

    /// Send `report` to `name`. Any non-zero exit — including 126 (cannot
    /// execute) and 127 (no such command) — and any spawn failure (a missing
    /// binary) is a failed send, never a success.
    pub fn send(&self, name: &str, report: &std::path::Path) -> Result<(), String> {
        let mut command = Command::new(&self.command[0]);
        command
            .args(&self.command[1..])
            .arg("send")
            .arg(name)
            .arg("--deliver-while-working")
            .arg("--file")
            .arg(report)
            .stdin(std::process::Stdio::null());
        let output = command
            .output()
            .map_err(|error| format!("cannot run {}: {error}", self.command[0]))?;
        if output.status.success() {
            Ok(())
        } else {
            let code = output
                .status
                .code()
                .map_or_else(|| "signal".to_owned(), |code| code.to_string());
            let stderr = String::from_utf8_lossy(&output.stderr);
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
        "refusal-deferred" => {
            text.push_str(&format!("refusal: {}\n", item.last_refusal));
            text.push_str(&format!("retry_after: {}\n", item.next_retry_at));
            text.push_str(&format!("refusals: {}\n", item.validation_refusals));
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

/// Deliver every owed notification once. Returns how many were delivered.
///
/// Each item is handled independently: a failure is persisted (attempt
/// count and error), logged to stderr and the item's history, and never
/// stops the queue or the other notifications. Attempts for a transition
/// stop after [`MAX_ATTEMPTS`]; the give-up is logged the same way.
pub fn sweep(state: &State, config: &Config, notifier: &Notifier) -> Result<usize, String> {
    let _ = config;
    let mut delivered = 0usize;
    for item in state.items()? {
        if item.notify.is_empty() {
            continue;
        }
        let Some(transition) = desired_transition(&item) else {
            continue;
        };
        let prior = &item.notify_delivery;
        if prior.key == transition.key && (prior.delivered || prior.attempts >= MAX_ATTEMPTS) {
            continue;
        }
        let attempts = if prior.key == transition.key {
            prior.attempts + 1
        } else {
            1
        };
        // Persist the attempt before sending: a daemon crash mid-send can
        // then only lose the delivery mark, never the retry bound.
        let mut attempt_item = item.clone();
        attempt_item.notify_delivery = NotifyDelivery {
            key: transition.key.clone(),
            attempts,
            delivered: false,
            last_error: String::new(),
        };
        if let Err(error) = state.save_item(&attempt_item) {
            eprintln!(
                "merge-queue notify: cannot record attempt for #{}: {error}",
                item.pr
            );
            continue;
        }
        let path = report_path(state, &item, &transition);
        let outcome = std::fs::create_dir_all(reports_dir(state))
            .map_err(|error| format!("cannot create notifications dir: {error}"))
            .and_then(|()| {
                std::fs::write(&path, render_report(state, &item, &transition))
                    .map_err(|error| format!("cannot write {}: {error}", path.display()))
            })
            .and_then(|()| notifier.send(&item.notify, &path));
        // Reload before persisting the outcome: the item may have moved on
        // (resubmitted, withdrawn) while agentctl ran. Only mark the key we
        // actually attempted, and only if it is still the owed transition.
        let mut current = match state.item(item.pr) {
            Ok(Some(current)) => current,
            Ok(None) => continue,
            Err(error) => {
                eprintln!("merge-queue notify: cannot reload #{}: {error}", item.pr);
                continue;
            }
        };
        if desired_transition(&current).as_ref() != Some(&transition) {
            continue;
        }
        match outcome {
            Ok(()) => {
                current.notify_delivery.delivered = true;
                current.notify_delivery.last_error.clear();
                current.log(
                    "notified",
                    format!(
                        "{} sent to {} (report {})",
                        transition.verdict,
                        item.notify,
                        path.display()
                    ),
                );
                if let Err(error) = state.save_item(&current) {
                    eprintln!(
                        "merge-queue notify: cannot record delivery for #{}: {error}",
                        item.pr
                    );
                    continue;
                }
                delivered += 1;
            }
            Err(error) => {
                current.notify_delivery.last_error = error.clone();
                let gave_up = attempts >= MAX_ATTEMPTS;
                current.log(
                    "notify-failed",
                    format!(
                        "{} to {} failed (attempt {attempts}/{MAX_ATTEMPTS}){}: {error}",
                        transition.verdict,
                        item.notify,
                        if gave_up { "; giving up" } else { "" }
                    ),
                );
                let _ = state.save_item(&current);
                eprintln!(
                    "merge-queue notify: #{} {} to {} failed (attempt {attempts}/{MAX_ATTEMPTS}){}: {error}",
                    item.pr,
                    transition.verdict,
                    item.notify,
                    if gave_up { "; giving up" } else { "" }
                );
            }
        }
    }
    Ok(delivered)
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
            exact: false,
            requires: vec![],
            claim: None,
            source: String::new(),
            close_pending: false,
            validation_refusals: 0,
            next_retry_at: String::new(),
            last_refusal: String::new(),
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
    fn missing_and_refused_sends_are_failures_never_success() {
        let missing = Notifier {
            command: vec!["/definitely/missing/agentctl".to_owned()],
        };
        let report = std::env::temp_dir().join("merge-queue-notify-unit.md");
        std::fs::write(&report, "x\n").unwrap();
        assert!(missing.send("lane-fictional", &report).is_err());
        for code in [1, 75, 126, 127] {
            let refused = Notifier {
                command: vec![
                    "sh".to_owned(),
                    "-c".to_owned(),
                    format!("exit {code}"),
                    "sh".to_owned(),
                ],
            };
            assert!(
                refused.send("lane-fictional", &report).is_err(),
                "exit {code} must be a failed send"
            );
        }
        let ok = Notifier {
            command: vec![
                "sh".to_owned(),
                "-c".to_owned(),
                "exit 0".to_owned(),
                "sh".to_owned(),
            ],
        };
        assert!(ok.send("lane-fictional", &report).is_ok());
    }
}
