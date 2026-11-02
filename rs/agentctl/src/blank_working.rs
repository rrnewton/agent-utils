//! Blank-working health check: an agent Herdr still reports `working` from
//! its OSC title while its terminal grid stays empty.
//!
//! A hung harness TUI can leave exactly that shape for an hour: the title
//! keeps claiming work, the grid shows nothing, and no other watcher fires
//! because the screen never changes in a way its rules read as stalled.
//! This check turns the shape into one coordinator alert per episode.
//!
//! An agent counts only when both facts hold on the same pass:
//!
//! * `title_working`: Herdr's `agent explain` verdict is `working` under a
//!   rule whose region is the OSC title. Screen-derived states (Herdr's
//!   screen rules, agentctl's screen overrides for Muse, misdetected or
//!   relayed panes) never count: a blank grid cannot corroborate them, and
//!   the title is the one signal a hung TUI still emits.
//! * `blank`: the visible grid, once rendered, is whitespace in every cell
//!   ([`crate::submission::is_ansi_blank`], the same test the
//!   `composer_unreadable` classification uses).
//!
//! Episode state lives in `REGISTRY/.blank-working/NAME.json` (schema
//! `agentctl-blank-working/v1`), beside the records and outside every
//! agent's own directory, like `REGISTRY/.progress/NAME.json`. The first
//! blank pass records `since`; later passes update `last_blank_at`, so the
//! duration accumulates across separate check invocations (a tick or cron
//! caller runs one pass and exits). One successful alert sets `alerted`;
//! the episode alerts again only after a pass sees the agent recovered
//! (grid no longer blank, or title no longer working), which removes the
//! file and ends the episode. A pass that cannot probe an agent leaves its
//! episode untouched: no evidence is not recovery.
//!
//! The alert goes to the registry's coordinator through the same durable
//! queue as `agentctl send` (the caller supplies that send as a
//! [`CoordinatorAlert`], so tests substitute a fake). A failed alert is
//! logged and reported, never fatal, and `alerted` stays unset so the next
//! pass retries; the queue message ID is derived from the agent and
//! `since`, so a retry after a crash queues nothing twice.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::AgentError;

/// Schema of one episode file and of the check report.
pub const SCHEMA: &str = "agentctl-blank-working/v1";
/// Default minutes an agent may stay blank while title-working before the
/// coordinator is told (`--blank-working-minutes` overrides it).
pub const DEFAULT_MINUTES: u64 = 5;

type Result<T> = std::result::Result<T, AgentError>;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

/// One agent's persisted blank-working episode.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Episode {
    /// Always [`SCHEMA`].
    schema: String,
    /// Unix seconds the current blank stretch was first seen.
    since: f64,
    /// Unix seconds of the latest pass that still saw it blank and working.
    last_blank_at: f64,
    /// The coordinator was told about this episode.
    #[serde(default)]
    alerted: bool,
}

/// Where `name`'s episode lives in `registry`.
#[must_use]
pub fn path(registry: &Path, name: &str) -> PathBuf {
    registry.join(".blank-working").join(format!("{name}.json"))
}

fn load(registry: &Path, name: &str) -> Result<Option<Episode>> {
    let path = path(registry, name);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(fail(format!("cannot read {}: {error}", path.display()))),
    };
    let episode: Episode = serde_json::from_str(&text).map_err(|error| {
        fail(format!(
            "{} is not a blank-working episode: {error}",
            path.display()
        ))
    })?;
    if episode.schema != SCHEMA {
        return Err(fail(format!(
            "{} has schema {:?}, expected {SCHEMA}",
            path.display(),
            episode.schema
        )));
    }
    Ok(Some(episode))
}

fn save(registry: &Path, name: &str, episode: &Episode) -> Result<()> {
    let path = path(registry, name);
    let directory = path.parent().expect("episode path has a directory");
    crate::agent::create_private_directory(directory, "blank-working episodes", false, false)?;
    let value = serde_json::to_value(episode).map_err(|error| fail(error.to_string()))?;
    crate::agent::atomic_json(&path, &value)
}

fn clear(registry: &Path, name: &str) -> Result<bool> {
    let path = path(registry, name);
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(fail(format!("cannot remove {}: {error}", path.display()))),
    }
}

/// The recorded episode of `name`, for `agentctl status`: when the blank
/// stretch was first seen, how many minutes the passes so far have seen it
/// span (`last_blank_at - since`, never wall-clock since the last pass),
/// and whether the coordinator was told. `None` when no episode is open.
#[must_use]
pub fn current(registry: &Path, name: &str) -> Option<Value> {
    let episode = load(registry, name).ok()??;
    Some(json!({
        "since": episode.since,
        "minutes": (episode.last_blank_at - episode.since) / 60.0,
        "alerted": episode.alerted,
    }))
}

/// What one pass observed about one agent. The episode logic reads only
/// this, so tests drive it with fixtures instead of a terminal server.
#[derive(Clone, Debug)]
pub struct Observation {
    /// Registered agent name.
    pub name: String,
    /// Herdr's `agent explain` says `working` under an OSC-title rule.
    pub title_working: bool,
    /// The rendered visible grid is whitespace in every cell.
    pub blank: bool,
    /// Why the agent could not be probed this pass. Such an agent starts no
    /// episode and ends none.
    pub probe_error: Option<String>,
}

impl Observation {
    /// A probed agent with its two facts.
    #[must_use]
    pub fn probed(name: &str, title_working: bool, blank: bool) -> Self {
        Self {
            name: name.to_owned(),
            title_working,
            blank,
            probe_error: None,
        }
    }

    /// An agent this pass could not probe.
    #[must_use]
    pub fn unprobed(name: &str, error: String) -> Self {
        Self {
            name: name.to_owned(),
            title_working: false,
            blank: false,
            probe_error: Some(error),
        }
    }

    fn affected(&self) -> bool {
        self.probe_error.is_none() && self.title_working && self.blank
    }
}

/// The coordinator alert for one episode, supplied by the caller: the
/// production send queues durably for the coordinator exactly as
/// `agentctl send` does; tests record or fail it. `Ok` carries the queue
/// outcome wording for the report.
pub trait CoordinatorAlert {
    /// Tell `coordinator` that `agent` has been blank while title-working
    /// for `minutes` minutes, since Unix seconds `since`.
    fn alert(
        &mut self,
        coordinator: &str,
        agent: &str,
        since: f64,
        minutes: f64,
    ) -> std::result::Result<String, String>;
}

/// One agent's outcome in one pass.
#[derive(Clone, Debug, Serialize)]
pub struct AgentReport {
    /// Registered agent name.
    pub name: String,
    /// The rendered visible grid was whitespace in every cell.
    pub blank: bool,
    /// Herdr's `agent explain` said `working` under an OSC-title rule.
    pub title_working: bool,
    /// Both facts held: an episode is open (or was already open).
    pub affected: bool,
    /// Unix seconds the open episode was first seen, when one is open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<f64>,
    /// Minutes the episode has spanned at this pass, when one is open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minutes: Option<f64>,
    /// The coordinator was told about this episode (this pass or earlier).
    pub alerted: bool,
    /// This pass sent the episode's one alert.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub alert_sent: bool,
    /// This pass is past the threshold and would alert, but `--check-only`
    /// held the alert back.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub would_alert: bool,
    /// Why this pass's alert did not queue, when it tried and failed. The
    /// episode stays unalerted, so the next pass retries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alert_error: Option<String>,
    /// This pass saw the agent recovered and ended its episode.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub recovered: bool,
    /// Why the agent could not be probed this pass, when it could not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe_error: Option<String>,
}

/// One pass over every observed agent.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    /// Always [`SCHEMA`].
    pub schema: String,
    /// Unix seconds of this pass.
    pub checked_at: f64,
    /// The coordinator alerts name.
    pub coordinator: String,
    /// Minutes an agent may stay blank while title-working before an alert.
    pub threshold_minutes: f64,
    /// Report only: no alert was sent this pass.
    pub check_only: bool,
    /// No agent is past its threshold and no alert failed.
    pub ok: bool,
    /// Per-agent outcomes, in observation order.
    pub agents: Vec<AgentReport>,
    /// Alerts this pass queued for the coordinator.
    pub alerts_sent: usize,
}

/// How one blank-working pass was asked to run.
#[derive(Clone, Debug)]
pub struct Options {
    /// The coordinator alerts name.
    pub coordinator: String,
    /// Minutes an agent may stay blank while title-working before an alert.
    pub threshold: Duration,
    /// Report only: send no alert (episode state still advances).
    pub check_only: bool,
}

/// Run one blank-working pass: update every observed agent's episode from
/// this pass's facts at `now` (Unix seconds), alerting the coordinator
/// once per episode that has spanned the threshold. `log` receives the
/// lines the caller prints (alert failures, state failures); nothing here
/// prints or exits, and one agent's failure never stops the others.
pub fn check(
    registry: &Path,
    observations: &[Observation],
    now: f64,
    options: &Options,
    alerts: &mut dyn CoordinatorAlert,
    log: &mut dyn FnMut(&str),
) -> Report {
    let coordinator = options.coordinator.as_str();
    let check_only = options.check_only;
    let threshold_minutes = options.threshold.as_secs_f64() / 60.0;
    let mut agents = Vec::with_capacity(observations.len());
    let mut alerts_sent = 0usize;
    let mut ok = true;
    for observation in observations {
        let mut report = AgentReport {
            name: observation.name.clone(),
            blank: observation.blank,
            title_working: observation.title_working,
            affected: observation.affected(),
            since: None,
            minutes: None,
            alerted: false,
            alert_sent: false,
            would_alert: false,
            alert_error: None,
            recovered: false,
            probe_error: observation.probe_error.clone(),
        };
        if observation.probe_error.is_some() {
            // No evidence: an open episode neither advances nor ends. Say
            // what is still recorded, so the report never hides it.
            if let Ok(Some(episode)) = load(registry, &observation.name) {
                report.since = Some(episode.since);
                report.minutes = Some((episode.last_blank_at - episode.since) / 60.0);
                report.alerted = episode.alerted;
            }
            agents.push(report);
            continue;
        }
        if !observation.affected() {
            match clear(registry, &observation.name) {
                Ok(true) => report.recovered = true,
                Ok(false) => {}
                Err(error) => log(&format!(
                    "blank-working: cannot end {}'s episode: {error}",
                    observation.name
                )),
            }
            agents.push(report);
            continue;
        }
        let mut episode = match load(registry, &observation.name) {
            Ok(Some(episode)) => episode,
            Ok(None) => Episode {
                schema: SCHEMA.to_owned(),
                since: now,
                last_blank_at: now,
                alerted: false,
            },
            Err(error) => {
                // A state file this build cannot read must not hide the
                // agent: report it unprobed and leave the file alone.
                log(&format!(
                    "blank-working: cannot read {}'s episode: {error}",
                    observation.name
                ));
                report.probe_error = Some(error.to_string());
                report.affected = false;
                agents.push(report);
                continue;
            }
        };
        // A clock that moved backwards never shortens an episode.
        episode.last_blank_at = episode.last_blank_at.max(now);
        let minutes = (episode.last_blank_at - episode.since) / 60.0;
        report.since = Some(episode.since);
        report.minutes = Some(minutes);
        report.alerted = episode.alerted;
        if minutes >= threshold_minutes {
            ok = false;
            if !episode.alerted {
                if check_only {
                    report.would_alert = true;
                } else {
                    match alerts.alert(coordinator, &observation.name, episode.since, minutes) {
                        Ok(_) => {
                            episode.alerted = true;
                            report.alerted = true;
                            report.alert_sent = true;
                            alerts_sent += 1;
                        }
                        Err(error) => {
                            // Logged, never fatal; `alerted` stays unset,
                            // so the next pass retries the one alert.
                            log(&format!(
                                "blank-working: cannot alert {coordinator} about {}: {error}",
                                observation.name
                            ));
                            report.alert_error = Some(error);
                        }
                    }
                }
            }
        }
        if let Err(error) = save(registry, &observation.name, &episode) {
            log(&format!(
                "blank-working: cannot record {}'s episode: {error}",
                observation.name
            ));
            if report.probe_error.is_none() {
                report.probe_error = Some(error.to_string());
            }
        }
        agents.push(report);
    }
    Report {
        schema: SCHEMA.to_owned(),
        checked_at: now,
        coordinator: coordinator.to_owned(),
        threshold_minutes,
        check_only,
        ok,
        agents,
        alerts_sent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Records every alert; `fail` makes each one fail the way a send can.
    #[derive(Default)]
    struct FakeAlert {
        sent: Vec<(String, String, f64)>,
        fail: bool,
    }

    impl CoordinatorAlert for FakeAlert {
        fn alert(
            &mut self,
            coordinator: &str,
            agent: &str,
            _since: f64,
            minutes: f64,
        ) -> std::result::Result<String, String> {
            if self.fail {
                return Err("queue unavailable (fixture)".to_owned());
            }
            self.sent
                .push((coordinator.to_owned(), agent.to_owned(), minutes));
            Ok("delivered".to_owned())
        }
    }

    fn registry(test: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "agentctl-blank-working-{test}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("fixture registry");
        directory
    }

    fn pass(
        registry: &Path,
        observations: &[Observation],
        now: f64,
        alert: &mut FakeAlert,
        logs: &RefCell<Vec<String>>,
    ) -> Report {
        check(
            registry,
            observations,
            now,
            &Options {
                coordinator: "coordinator".to_owned(),
                threshold: Duration::from_secs(DEFAULT_MINUTES * 60),
                check_only: false,
            },
            alert,
            &mut |line| logs.borrow_mut().push(line.to_owned()),
        )
    }

    #[test]
    fn blank_and_working_under_the_threshold_records_but_never_alerts() {
        let registry = registry("under");
        let mut alert = FakeAlert::default();
        let logs = RefCell::new(Vec::new());
        let blank = [Observation::probed("worker-a", true, true)];
        let first = pass(&registry, &blank, 1_000.0, &mut alert, &logs);
        assert!(first.ok);
        assert_eq!(first.agents[0].since, Some(1_000.0));
        assert_eq!(first.agents[0].minutes, Some(0.0));
        // Four more minutes across separate invocations: still under five.
        let later = pass(&registry, &blank, 1_240.0, &mut alert, &logs);
        assert!(later.ok);
        assert_eq!(later.agents[0].minutes, Some(4.0));
        assert!(alert.sent.is_empty(), "{:?}", alert.sent);
        let recorded = current(&registry, "worker-a").expect("episode recorded");
        assert_eq!(recorded["since"], json!(1_000.0));
        assert_eq!(recorded["alerted"], json!(false));
    }

    #[test]
    fn over_the_threshold_alerts_once_and_continued_blank_never_repeats_it() {
        let registry = registry("over");
        let mut alert = FakeAlert::default();
        let logs = RefCell::new(Vec::new());
        let blank = [Observation::probed("worker-a", true, true)];
        pass(&registry, &blank, 1_000.0, &mut alert, &logs);
        let due = pass(&registry, &blank, 1_300.0, &mut alert, &logs);
        assert!(!due.ok, "past the threshold is not ok: {due:?}");
        assert!(due.agents[0].alert_sent);
        assert_eq!(due.alerts_sent, 1);
        assert_eq!(
            alert.sent,
            vec![("coordinator".to_owned(), "worker-a".to_owned(), 5.0)]
        );
        // Still blank ten minutes on: the same episode, no second alert.
        let continued = pass(&registry, &blank, 1_900.0, &mut alert, &logs);
        assert!(!continued.agents[0].alert_sent);
        assert!(continued.agents[0].alerted);
        assert_eq!(alert.sent.len(), 1);
    }

    #[test]
    fn recovery_clears_the_episode_and_a_new_blank_stretch_may_alert_again() {
        let registry = registry("recovery");
        let mut alert = FakeAlert::default();
        let logs = RefCell::new(Vec::new());
        let blank = [Observation::probed("worker-a", true, true)];
        pass(&registry, &blank, 1_000.0, &mut alert, &logs);
        pass(&registry, &blank, 1_300.0, &mut alert, &logs);
        assert_eq!(alert.sent.len(), 1);
        // The grid shows content again: the episode ends.
        let visible = [Observation::probed("worker-a", true, false)];
        let recovered = pass(&registry, &visible, 1_400.0, &mut alert, &logs);
        assert!(recovered.ok);
        assert!(recovered.agents[0].recovered);
        assert!(current(&registry, "worker-a").is_none());
        // A fresh blank stretch is a new episode with its own one alert.
        pass(&registry, &blank, 2_000.0, &mut alert, &logs);
        let again = pass(&registry, &blank, 2_300.0, &mut alert, &logs);
        assert!(again.agents[0].alert_sent);
        assert_eq!(alert.sent.len(), 2);
    }

    #[test]
    fn non_blank_or_not_title_working_never_alerts() {
        let registry = registry("facts");
        let mut alert = FakeAlert::default();
        let logs = RefCell::new(Vec::new());
        // Blank, but the working verdict came from a screen rule (or any
        // non-title source): excluded, however long it holds.
        let screen_working = [Observation::probed("worker-a", false, true)];
        pass(&registry, &screen_working, 1_000.0, &mut alert, &logs);
        let late = pass(&registry, &screen_working, 9_000.0, &mut alert, &logs);
        assert!(late.ok);
        assert!(!late.agents[0].affected);
        assert!(current(&registry, "worker-a").is_none());
        // Title-working, but the grid shows a composer: no episode either.
        let visible = [Observation::probed("worker-b", true, false)];
        pass(&registry, &visible, 1_000.0, &mut alert, &logs);
        let late = pass(&registry, &visible, 9_000.0, &mut alert, &logs);
        assert!(late.ok);
        assert!(current(&registry, "worker-b").is_none());
        // Not working at all, blank grid: an idle empty pane is not a hang.
        let idle_blank = [Observation::probed("worker-c", false, true)];
        pass(&registry, &idle_blank, 9_000.0, &mut alert, &logs);
        assert!(alert.sent.is_empty(), "{:?}", alert.sent);
    }

    #[test]
    fn a_failed_alert_is_logged_never_fatal_and_retried_next_pass() {
        let registry = registry("send-failure");
        let mut alert = FakeAlert {
            fail: true,
            ..FakeAlert::default()
        };
        let logs = RefCell::new(Vec::new());
        let blank = [Observation::probed("worker-a", true, true)];
        pass(&registry, &blank, 1_000.0, &mut alert, &logs);
        let failed = pass(&registry, &blank, 1_300.0, &mut alert, &logs);
        assert!(!failed.ok);
        assert!(
            !failed.agents[0].alerted,
            "a failed alert is not recorded sent"
        );
        assert!(
            failed.agents[0]
                .alert_error
                .as_deref()
                .is_some_and(|error| error.contains("queue unavailable")),
            "{failed:?}"
        );
        assert!(
            logs.borrow()
                .iter()
                .any(|line| line.contains("cannot alert coordinator about worker-a")),
            "{:?}",
            logs.borrow()
        );
        // The send recovers: the same episode's one alert goes out.
        alert.fail = false;
        let retried = pass(&registry, &blank, 1_360.0, &mut alert, &logs);
        assert!(retried.agents[0].alert_sent);
        assert_eq!(alert.sent.len(), 1);
    }

    #[test]
    fn check_only_reports_the_due_alert_without_sending_or_spending_it() {
        let registry = registry("check-only");
        let mut alert = FakeAlert::default();
        let logs = RefCell::new(Vec::new());
        let blank = [Observation::probed("worker-a", true, true)];
        pass(&registry, &blank, 1_000.0, &mut alert, &logs);
        let report = check(
            &registry,
            &blank,
            1_300.0,
            &Options {
                coordinator: "coordinator".to_owned(),
                threshold: Duration::from_secs(DEFAULT_MINUTES * 60),
                check_only: true,
            },
            &mut alert,
            &mut |line| logs.borrow_mut().push(line.to_owned()),
        );
        assert!(!report.ok);
        assert!(report.agents[0].would_alert);
        assert!(!report.agents[0].alerted);
        assert!(alert.sent.is_empty());
        // A real pass afterwards still sends the episode's one alert.
        let sent = pass(&registry, &blank, 1_360.0, &mut alert, &logs);
        assert!(sent.agents[0].alert_sent);
    }

    #[test]
    fn an_unprobed_pass_neither_advances_nor_ends_an_episode() {
        let registry = registry("unprobed");
        let mut alert = FakeAlert::default();
        let logs = RefCell::new(Vec::new());
        let blank = [Observation::probed("worker-a", true, true)];
        pass(&registry, &blank, 1_000.0, &mut alert, &logs);
        let unknown = [Observation::unprobed(
            "worker-a",
            "pane read failed (fixture)".to_owned(),
        )];
        let report = pass(&registry, &unknown, 9_000.0, &mut alert, &logs);
        assert!(report.ok, "no evidence is not an alert: {report:?}");
        assert_eq!(report.agents[0].minutes, Some(0.0), "not advanced");
        // The next blank pass resumes the same episode from its first sight.
        let resumed = pass(&registry, &blank, 9_060.0, &mut alert, &logs);
        assert_eq!(resumed.agents[0].since, Some(1_000.0));
        assert!(resumed.agents[0].alert_sent);
    }
}
