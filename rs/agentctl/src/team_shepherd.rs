//! The always-on one-shot recovery shepherd (`#1559`, Phase 2): after a
//! boot, it judges recovery from `team manifest` facts with stateless
//! one-shot agent calls (`run-agent-prompt`: muse, then claude, then
//! codex) and effects only through `team act`.
//!
//! One incident per restore epoch (kernel boot plus user-manager
//! identity, the epoch the restore replay guard uses). Inside the
//! incident it loops bounded rounds until every judged team is a
//! deterministic YES (lead running, bridge healthy, handoff acked —
//! each an explicit pass; held/skipped never count) or bounded give-up
//! escalates and stamps. A mid-session death is not its job: watch,
//! `team kick` and the driver own that; the shepherd re-enters only on
//! a new epoch or an explicit manual run, which resumes or no-ops
//! against the existing stamp.
//!
//! Two restorers, mechanised from the team-restore skill's step 0: a
//! team whose lead is live is judged read-only and never acted on, and
//! only the invocation's own registry (the `--registry` team) is ever
//! acted on — every team in `--config` is judged read-only. The
//! shepherd state flock is held only for the constant-time load/save
//! of its small state and stamp files, never across a manifest build,
//! a restore, a one-shot call, or a `team act` wait.
//!
//! Persisted files live only under the shepherd state directory
//! (`<state root>/shepherd/<boot-id>/`): `state.json` (resumable round
//! bookkeeping) and `stamp.json` (the terminal verdict). No older
//! reader opens either path, and no driver record field changes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::agent::AgentError;
use crate::restore::{Host, RestoreConfig, RestoreEpoch};
use crate::subagents::ManagedApi;
use crate::team_manifest::manifest_team_with_verify;
use crate::team_verify::{explicit_yes, VerifyOptions};

/// Schema of the terminal stamp document.
pub const SHEPHERD_STAMP_SCHEMA: &str = "agentctl-team-shepherd-stamp/v1";
/// Schema of the resumable per-incident state document.
pub const SHEPHERD_STATE_SCHEMA: &str = "agentctl-team-shepherd-state/v1";
/// Schema of the `team shepherd` run report printed on stdout.
pub const SHEPHERD_REPORT_SCHEMA: &str = "agentctl-team-shepherd/v1";

type Result<T> = std::result::Result<T, AgentError>;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

/// Bounds and wiring for one shepherd run, from the CLI flags of the
/// same names (defaults are the approved phase-plan bounds).
#[derive(Clone, Debug)]
pub struct ShepherdOptions {
    /// Agentctl state root (default: `$XDG_STATE_HOME/agentctl`, else
    /// `~/.local/state/agentctl`).
    pub state_dir: Option<PathBuf>,
    /// Maximum judgement rounds in one incident.
    pub max_rounds: u32,
    /// Maximum wall minutes in one incident.
    pub max_minutes: u64,
    /// Hard seconds budget for one `run-agent-prompt` chain.
    pub call_timeout_seconds: u64,
    /// Seconds between rounds.
    pub round_delay_seconds: u64,
    /// Consecutive identical-manifest rounds that escalate early.
    pub no_progress_rounds: u32,
    /// The `run-agent-prompt` executable.
    pub run_agent_prompt: PathBuf,
    /// Seconds a `team act` execution may wait for its receipt.
    pub act_wait_seconds: u64,
}

impl Default for ShepherdOptions {
    fn default() -> Self {
        Self {
            state_dir: None,
            max_rounds: 20,
            max_minutes: 120,
            call_timeout_seconds: 300,
            round_delay_seconds: 30,
            no_progress_rounds: 2,
            run_agent_prompt: PathBuf::from("run-agent-prompt"),
            act_wait_seconds: 60,
        }
    }
}

/// Resumable bookkeeping for one incident, persisted between rounds so
/// a restarted shepherd resumes instead of repeating effects.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ShepherdState {
    /// Always [`SHEPHERD_STATE_SCHEMA`].
    pub schema: String,
    /// Kernel boot this incident belongs to.
    pub boot_id: String,
    /// Owning team's incident id (12 hex), once known.
    pub incident: String,
    /// Rounds completed so far.
    pub round: u32,
    /// SHA-256 of the owning team's last manifest, for no-progress.
    pub last_manifest_hash: String,
    /// Consecutive rounds with an identical owning-team manifest.
    pub no_progress: u32,
    /// The deterministic restore arm already ran for this incident.
    pub restore_done: bool,
    /// Harness that answered the latest one-shot (`none` until one does).
    pub judge: String,
}

/// Facts the loop gathered for one team in one round.
struct TeamRound {
    registry: PathBuf,
    manifest: Value,
    yes: bool,
    lead_live: bool,
}

/// The shepherd state directory for one boot:
/// `<state root>/shepherd/<boot-id>/`.
pub fn shepherd_dir(state_dir: Option<&Path>, boot_id: &str) -> Option<PathBuf> {
    Some(state_root(state_dir)?.join("shepherd").join(boot_id))
}

fn state_root(state_dir: Option<&Path>) -> Option<PathBuf> {
    match state_dir {
        Some(dir) => Some(dir.to_path_buf()),
        None => {
            crate::restore::default_report_dir().and_then(|dir| dir.parent().map(Path::to_path_buf))
        }
    }
}

/// SHA-256 of a manifest's canonical JSON text.
pub fn manifest_hash(manifest: &Value) -> String {
    let text = serde_json::to_string(manifest).unwrap_or_default();
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// The proposal a one-shot answer carries: its last JSON line with an
/// `action` field. Anything else is judgement prose, not a proposal.
pub fn parse_proposal(answer: &str) -> Option<Value> {
    answer.lines().rev().find_map(|line| {
        let line = line.trim();
        if !line.starts_with('{') {
            return None;
        }
        let value: Value = serde_json::from_str(line).ok()?;
        value.get("action")?.is_string().then_some(value)
    })
}

/// The `team act` argv (after `team act`) for a validated proposal, or
/// the refusal reason. Only allowlisted actions with their required
/// fields survive; a session gap's candidates stay report-only, so no
/// proposal can bind a session or rewrite a record.
pub fn proposal_argv(proposal: &Value) -> std::result::Result<Vec<String>, String> {
    let action = proposal["action"].as_str().unwrap_or_default();
    let field = |name: &str| -> std::result::Result<String, String> {
        proposal[name]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("proposal {action} missing {name}"))
    };
    let argv = match action {
        "facts" => vec!["facts".to_owned()],
        "revive" => vec!["revive".to_owned(), field("agent")?],
        "start-unit" => vec!["start-unit".to_owned(), field("unit")?],
        "reset-unit" => vec!["reset-unit".to_owned(), field("unit")?],
        "drain" => vec!["drain".to_owned(), field("agent")?],
        "declare-done" => vec!["declare-done".to_owned()],
        "escalate" => {
            let mut argv = vec!["escalate".to_owned()];
            if let Some(text) = proposal["text"].as_str().filter(|text| !text.is_empty()) {
                argv.push(text.to_owned());
            }
            argv
        }
        "kill-pid" => vec![
            "kill-pid".to_owned(),
            field("pid")?,
            "--expect-cmdline".to_owned(),
            field("expect_cmdline")?,
            "--expect-start".to_owned(),
            field("expect_start")?,
            "--boot-id".to_owned(),
            field("boot_id")?,
        ],
        other => return Err(format!("proposal action {other:?} not allowlisted")),
    };
    Ok(argv)
}

/// Run one bounded subprocess, killing it past `timeout`. Stdout and
/// stderr are returned on success or failure alike; the exit code
/// rides along.
fn run_bounded(
    program: &Path,
    args: &[String],
    timeout: Duration,
) -> Result<(i32, String, String)> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| fail(format!("cannot run {}: {error}", program.display())))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                break;
            }
            Err(error) => return Err(fail(format!("cannot wait on a subprocess: {error}"))),
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| fail(format!("cannot collect a subprocess: {error}")))?;
    Ok((
        output.status.code().unwrap_or(1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

/// Which harness answered, from the wrapper's stderr attempt lines
/// (`run-agent-prompt: harness=muse status=answered ...`).
pub fn judge_of(stderr: &str) -> Option<String> {
    stderr.lines().find_map(|line| {
        if !line.contains("status=answered") {
            return None;
        }
        line.split_whitespace()
            .find_map(|word| word.strip_prefix("harness="))
            .map(str::to_owned)
    })
}

/// Hold the incident flock only for one load or save of the small
/// state/stamp files: constant-time under the lock, never across a
/// manifest build, restore, one-shot call, or `team act` wait.
fn lock_state(dir: &Path) -> Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("shepherd.lock"))
        .map_err(|error| fail(format!("cannot open the shepherd lock: {error}")))?;
    file.lock_exclusive()
        .map_err(|error| fail(format!("cannot take the shepherd lock: {error}")))?;
    Ok(file)
}

fn load_state(dir: &Path) -> ShepherdState {
    let Ok(text) = std::fs::read_to_string(dir.join("state.json")) else {
        return ShepherdState::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_state(dir: &Path, state: &ShepherdState) -> Result<()> {
    let _lock = lock_state(dir)?;
    let value = serde_json::to_value(state).map_err(|error| fail(error.to_string()))?;
    crate::agent::atomic_json(&dir.join("state.json"), &value)
        .map_err(|error| fail(error.to_string()))
}

fn load_stamp(dir: &Path) -> Option<Value> {
    std::fs::read_to_string(dir.join("stamp.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

fn save_stamp(dir: &Path, stamp: &Value) -> Result<()> {
    let _lock = lock_state(dir)?;
    crate::agent::atomic_json(&dir.join("stamp.json"), stamp)
        .map_err(|error| fail(error.to_string()))
}

fn append_log(dir: &Path, line: &str) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("shepherd.log"))
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Whether the team's verified lead is live this round: the
/// desired-live row for the coordinator explicitly passes. A live lead
/// owns recovery (skill step 0), so the shepherd only judges it.
fn lead_live(verify: &Value, coordinator: &str) -> bool {
    verify["checks"].as_array().is_some_and(|checks| {
        checks.iter().any(|row| {
            row["name"] == "desired-live"
                && row["subject"] == coordinator
                && row["status"] == "pass"
        })
    })
}

/// The brief for one one-shot call: instructions, the owning team's
/// manifest (allowlist facts only, never raw records), and one-line
/// read-only verdicts for every judged team.
fn brief(own: &TeamRound, judged: &[TeamRound], round: u32) -> String {
    let mut lines = vec![
        "You are a stateless recovery shepherd in emergency recovery mode. You have no tools."
            .to_owned(),
        "Judge the facts below. Effects happen only through the shepherd, never by you.".to_owned(),
        format!("Round {round}. Owning team manifest:"),
        serde_json::to_string(&own.manifest).unwrap_or_default(),
        "Other teams (read-only, never act on them):".to_owned(),
    ];
    for team in judged {
        lines.push(format!(
            "- {}: YES={} lead_live={}",
            team.registry.display(),
            team.yes,
            team.lead_live
        ));
    }
    lines.push(
        "Reply with a short judgement, then a FINAL line that is one JSON object: \
         {\"action\": \"facts\"} or {\"action\": \"revive\", \"agent\": NAME} or \
         {\"action\": \"start-unit\", \"unit\": UNIT} or {\"action\": \"reset-unit\", \"unit\": UNIT} or \
         {\"action\": \"drain\", \"agent\": NAME} or {\"action\": \"declare-done\"} or \
         {\"action\": \"escalate\", \"text\": WHY}. Propose at most one action, for the owning team only."
            .to_owned(),
    );
    lines.join("\n")
}

/// Inputs the CLI resolved once per run.
pub struct ShepherdInputs {
    /// This run's own executable, for `restore` and `team act`.
    pub agentctl: PathBuf,
    /// The Herdr binary every subprocess and client uses.
    pub herdr_bin: PathBuf,
}

/// Run one shepherd incident to its stamp. `own` is the only registry
/// ever acted on; `judged` registries are read-only. Exit semantics
/// ride in the report's `exit_code`: 0 every team YES, 75 gave up with
/// the stamp written, 1 the run itself was refused.
#[allow(clippy::too_many_arguments)]
pub fn run_shepherd<A: ManagedApi + ?Sized>(
    client: &A,
    own: &Path,
    judged: &[PathBuf],
    host: &dyn Host,
    inputs: &ShepherdInputs,
    options: &ShepherdOptions,
) -> Result<Value> {
    crate::team_driver::require_host_namespace(host)?;
    let boot_id = host.boot_id()?;
    let epoch = RestoreEpoch {
        boot_id: boot_id.clone(),
        user_manager: host.user_manager_id(),
    };
    let dir = shepherd_dir(options.state_dir.as_deref(), &boot_id)
        .ok_or_else(|| fail("cannot determine the agentctl state directory"))?;
    std::fs::create_dir_all(&dir)
        .map_err(|error| fail(format!("cannot create {}: {error}", dir.display())))?;
    if let Some(stamp) = load_stamp(&dir) {
        // A terminal stamp for this boot: resume means report, never
        // repeat effects.
        return Ok(json!({
            "schema": SHEPHERD_REPORT_SCHEMA,
            "boot_id": boot_id,
            "verdict": stamp["verdict"],
            "judge": stamp["judge"],
            "rounds": stamp["rounds"],
            "stamp": dir.join("stamp.json"),
            "resumed": true,
            "exit_code": if stamp["verdict"] == "YES" { 0 } else { 75 },
        }));
    }
    let mut state = load_state(&dir);
    if state.schema.is_empty() {
        state.schema = SHEPHERD_STATE_SCHEMA.to_owned();
        state.boot_id = boot_id.clone();
        state.judge = "none".to_owned();
    }
    let incident = crate::team_driver::incident_id(own, &epoch);
    state.incident = incident.clone();
    save_state(&dir, &state)?;
    let started = Instant::now();
    let mut verdict = "NO";
    let mut summaries: BTreeMap<String, Value> = BTreeMap::new();
    let mut round = state.round;
    while round < options.max_rounds {
        round += 1;
        let mut rounds_teams: Vec<TeamRound> = Vec::new();
        for registry in std::iter::once(own).chain(judged.iter().map(PathBuf::as_path)) {
            let config = RestoreConfig::load(registry)?;
            let coordinator = config.coordinator.clone().unwrap_or_default();
            let (manifest, verify) = manifest_team_with_verify(
                client,
                registry,
                host,
                &config,
                &VerifyOptions {
                    state_dir: options.state_dir.clone(),
                    ack_timeout_seconds: 1800,
                    dismiss_allowlisted: false,
                },
            )?;
            rounds_teams.push(TeamRound {
                registry: registry.to_path_buf(),
                yes: explicit_yes(&verify, &coordinator),
                lead_live: lead_live(&verify, &coordinator),
                manifest,
            });
        }
        let (own_round, judged_rounds) = rounds_teams.split_first().unwrap();
        for team in &rounds_teams {
            summaries.insert(
                team.registry.display().to_string(),
                json!({"yes": team.yes, "lead_live": team.lead_live,
                       "record_gaps": team.manifest["record_gaps"].as_array().map_or(0, Vec::len)}),
            );
        }
        append_log(
            &dir,
            &format!(
                "round {round}: own YES={} lead_live={}",
                own_round.yes, own_round.lead_live
            ),
        );
        if rounds_teams.iter().all(|team| team.yes) {
            verdict = "YES";
            state.round = round;
            save_state(&dir, &state)?;
            break;
        }
        // Step 0 + the deterministic arm, own team only, and only while
        // its lead is down: dry run first, then real; --force only when
        // the dry run replayed a completed report yet agents are down
        // (the user-manager-restart case in the skill).
        let own_actionable = !own_round.lead_live;
        if own_actionable && !state.restore_done {
            let base = vec![
                "--registry".to_owned(),
                own.display().to_string(),
                "--herdr-bin".to_owned(),
                inputs.herdr_bin.display().to_string(),
            ];
            let mut dry_args = base.clone();
            dry_args.extend(["restore".to_owned(), "--dry-run".to_owned()]);
            let (_, dry_text, _) = run_bounded(
                &inputs.agentctl,
                &dry_args,
                Duration::from_secs(options.call_timeout_seconds.max(60)),
            )?;
            let dry: Value = serde_json::from_str(&dry_text).unwrap_or(Value::Null);
            let replayed = dry["replayed"] == json!(true)
                || dry["action"].as_str() == Some("already-restored");
            let mut real_args = base;
            real_args.push("restore".to_owned());
            if replayed {
                real_args.push("--force".to_owned());
            }
            let (code, _, _) = run_bounded(
                &inputs.agentctl,
                &real_args,
                Duration::from_secs(options.max_minutes.max(1) * 60),
            )?;
            append_log(
                &dir,
                &format!("round {round}: restore replayed={replayed} exit={code}"),
            );
            state.restore_done = true;
            save_state(&dir, &state)?;
        }
        // One judgement call per round; at most one own-team act.
        let prompt = brief(own_round, judged_rounds, round);
        let prompt_path = dir.join(format!("prompt-{round}.txt"));
        std::fs::write(&prompt_path, &prompt)
            .map_err(|error| fail(format!("cannot write the round brief: {error}")))?;
        let answer = run_bounded(
            &options.run_agent_prompt,
            &[
                "--prompt-file".to_owned(),
                prompt_path.display().to_string(),
                "--timeout".to_owned(),
                options.call_timeout_seconds.to_string(),
            ],
            Duration::from_secs(options.call_timeout_seconds * 3 + 60),
        );
        let mut proposal: Option<Value> = None;
        if let Ok((0, text, log)) = &answer {
            if !text.trim().is_empty() {
                if let Some(judge) = judge_of(log) {
                    state.judge = judge;
                }
                proposal = parse_proposal(text);
            }
        }
        if own_actionable {
            if let Some(proposal) = &proposal {
                match proposal_argv(proposal) {
                    Ok(action_argv) => {
                        let mut args = vec![
                            "--registry".to_owned(),
                            own.display().to_string(),
                            "--herdr-bin".to_owned(),
                            inputs.herdr_bin.display().to_string(),
                            "team".to_owned(),
                            "act".to_owned(),
                            "--incident".to_owned(),
                            incident.clone(),
                            "--request-id".to_owned(),
                            format!("shepherd-{incident}-r{round}"),
                            "--wait-seconds".to_owned(),
                            options.act_wait_seconds.to_string(),
                        ];
                        args.extend(action_argv);
                        let (code, _, _) = run_bounded(
                            &inputs.agentctl,
                            &args,
                            Duration::from_secs(options.act_wait_seconds + 120),
                        )?;
                        append_log(
                            &dir,
                            &format!("round {round}: team act {} exit={code}", proposal["action"]),
                        );
                    }
                    Err(reason) => {
                        append_log(&dir, &format!("round {round}: proposal refused: {reason}"));
                    }
                }
            }
        }
        // No-progress bookkeeping on the owning team's manifest.
        let hash = manifest_hash(&own_round.manifest);
        if hash == state.last_manifest_hash {
            state.no_progress += 1;
        } else {
            state.no_progress = 0;
            state.last_manifest_hash = hash;
        }
        state.round = round;
        save_state(&dir, &state)?;
        let elapsed = started.elapsed() >= Duration::from_secs(options.max_minutes * 60);
        if elapsed || state.no_progress >= options.no_progress_rounds {
            break;
        }
        if round < options.max_rounds && options.round_delay_seconds > 0 {
            std::thread::sleep(Duration::from_secs(options.round_delay_seconds));
        }
    }
    if verdict != "YES" {
        // Bounded give-up: escalate through the landed path (own team,
        // lead down) and stamp. A live lead owns escalation instead.
        let own_yes = summaries
            .get(&own.display().to_string())
            .is_some_and(|row| row["yes"] == json!(true));
        if !own_yes {
            let args = vec![
                "--registry".to_owned(),
                own.display().to_string(),
                "--herdr-bin".to_owned(),
                inputs.herdr_bin.display().to_string(),
                "team".to_owned(),
                "act".to_owned(),
                "--incident".to_owned(),
                incident.clone(),
                "--request-id".to_owned(),
                format!("shepherd-{incident}-giveup"),
                "--wait-seconds".to_owned(),
                options.act_wait_seconds.to_string(),
                "escalate".to_owned(),
                "shepherd gave up without a deterministic YES".to_owned(),
            ];
            let _ = run_bounded(
                &inputs.agentctl,
                &args,
                Duration::from_secs(options.act_wait_seconds + 120),
            );
        }
        verdict = "GAVE-UP";
    }
    let stamp = json!({
        "schema": SHEPHERD_STAMP_SCHEMA,
        "boot_id": boot_id,
        "epoch": epoch.as_json(),
        "incident": incident,
        "registry": own,
        "verdict": verdict,
        "judge": state.judge,
        "rounds": state.round,
        "teams": summaries,
    });
    save_stamp(&dir, &stamp)?;
    append_log(&dir, &format!("stamp: {verdict} judge={}", state.judge));
    Ok(json!({
        "schema": SHEPHERD_REPORT_SCHEMA,
        "boot_id": stamp["boot_id"],
        "verdict": verdict,
        "judge": state.judge,
        "rounds": state.round,
        "stamp": dir.join("stamp.json"),
        "resumed": false,
        "exit_code": if verdict == "YES" { 0 } else { 75 },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposal_parsing_takes_the_last_json_line() {
        let answer = "judging\n{\"action\": \"facts\"}\n{\"action\": \"revive\", \"agent\": \"lane\"}\ntrail";
        let proposal = parse_proposal(answer).expect("proposal");
        assert_eq!(proposal["action"], "revive");
        assert_eq!(proposal_argv(&proposal).unwrap(), ["revive", "lane"]);
        assert!(parse_proposal("no json here").is_none());
        assert!(proposal_argv(&json!({"action": "bind-session"})).is_err());
        assert!(proposal_argv(&json!({"action": "revive"})).is_err());
    }

    #[test]
    fn judge_comes_from_the_answered_attempt_line() {
        let log = "run-agent-prompt: harness=muse status=timeout exit=- duration_s=1.0 model=unknown effort=low\n\
                   run-agent-prompt: harness=claude status=answered exit=0 duration_s=0.5 model=unknown effort=low\n";
        assert_eq!(judge_of(log).as_deref(), Some("claude"));
        assert_eq!(judge_of("run-agent-prompt: every harness failed"), None);
    }

    #[test]
    fn manifest_hash_is_stable_and_sensitive() {
        let one = json!({"a": 1});
        let two = json!({"a": 2});
        assert_eq!(manifest_hash(&one), manifest_hash(&one));
        assert_ne!(manifest_hash(&one), manifest_hash(&two));
    }
}
