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
    /// Ordered recovery profiles file, when the team configured one.
    pub profile_file: Option<PathBuf>,
    /// Incident claim file to heartbeat while running, when launched
    /// through `start-recovery`.
    pub claim_file: Option<PathBuf>,
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
            profile_file: None,
            claim_file: None,
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

/// Which profile answered, from the wrapper's stderr attempt lines
/// (`run-agent-prompt: profile=NAME harness=muse status=answered ...`).
/// Without profiles, the harness name is the judge identity.
pub fn judge_of(stderr: &str) -> Option<String> {
    stderr.lines().find_map(|line| {
        if !line.contains("status=answered") {
            return None;
        }
        let word = |prefix: &str| {
            line.split_whitespace()
                .find_map(|word| word.strip_prefix(prefix))
                .map(str::to_owned)
        };
        word("profile=").or_else(|| word("harness="))
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
        // With a profile file, judgement runs only on a current
        // all-pass smoke record; deterministic recovery never waits
        // on it.
        let judgement = match &options.profile_file {
            Some(file) => {
                if judgement_allowed(options.state_dir.as_deref(), own, file) {
                    Some(file.clone())
                } else {
                    append_log(
                        &dir,
                        &format!("round {round}: judgement unavailable (smoke)"),
                    );
                    None
                }
            }
            None => None,
        };
        let prompt = brief(own_round, judged_rounds, round);
        let prompt_path = dir.join(format!("prompt-{round}.txt"));
        std::fs::write(&prompt_path, &prompt)
            .map_err(|error| fail(format!("cannot write the round brief: {error}")))?;
        let mut wrapper_args = vec![
            "--prompt-file".to_owned(),
            prompt_path.display().to_string(),
            "--timeout".to_owned(),
            options.call_timeout_seconds.to_string(),
        ];
        if let Some(file) = &judgement {
            wrapper_args.extend(["--profile-file".to_owned(), file.display().to_string()]);
        }
        let answer = if options.profile_file.is_some() && judgement.is_none() {
            Err(fail("judgement unavailable"))
        } else {
            run_bounded(
                &options.run_agent_prompt,
                &wrapper_args,
                Duration::from_secs(options.call_timeout_seconds * 3 + 60),
            )
        };
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
        if let Some(claim) = &options.claim_file {
            heartbeat_claim(claim, &incident, &boot_id);
        }
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

/// Schema of a recovery profile file.
pub const PROFILES_SCHEMA: &str = "agentctl-recovery-profiles/v1";
/// Schema of the recorded install-smoke results.
pub const SMOKE_SCHEMA: &str = "agentctl-recovery-smoke/v1";
/// Schema of a recovery incident claim.
pub const CLAIM_SCHEMA: &str = "agentctl-recovery-claim/v1";

/// One ordered harness/model profile (`[[profile]]` in the file).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RecoveryProfile {
    /// Unique profile name; stamps and smoke records use it.
    pub name: String,
    /// `muse`, `claude`, or `codex`.
    pub harness: String,
    /// Requested model, when pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Requested effort, when pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Per-attempt seconds budget for this profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<f64>,
    /// Absolute harness executable, when not resolved by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Explicitly out of the chain (install records the reason in prose).
    #[serde(default)]
    pub disabled: bool,
}

/// Parse the strict v1 profile-file subset: a top-level `schema`
/// string plus ordered `[[profile]]` tables whose only keys are the
/// [`RecoveryProfile`] fields with string, number, or boolean values.
/// This is the whole v1 schema, so the subset loses nothing; anything
/// outside it is refused rather than guessed at.
pub fn parse_profiles(text: &str) -> Result<Vec<RecoveryProfile>> {
    let mut schema: Option<String> = None;
    let mut profiles: Vec<RecoveryProfile> = Vec::new();
    let mut current: Option<RecoveryProfile> = None;
    let mut in_profile = false;
    let unquote = |value: &str| -> Result<String> {
        let value = value.trim();
        value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .map(str::to_owned)
            .ok_or_else(|| fail(format!("profile value must be a quoted string: {value}")))
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[profile]]" {
            if let Some(done) = current.take() {
                profiles.push(done);
            }
            current = Some(RecoveryProfile::default());
            in_profile = true;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(fail(format!("invalid profile line: {line}")));
        };
        let (key, value) = (key.trim(), value.trim());
        if !in_profile {
            if key != "schema" {
                return Err(fail(format!("unknown top-level profile key {key}")));
            }
            schema = Some(unquote(value)?);
            continue;
        }
        let profile = current.as_mut().expect("profile table open");
        match key {
            "name" => profile.name = unquote(value)?,
            "harness" => profile.harness = unquote(value)?,
            "model" => profile.model = Some(unquote(value)?),
            "effort" => profile.effort = Some(unquote(value)?),
            "command" => profile.command = Some(unquote(value)?),
            "timeout_seconds" => {
                profile.timeout_seconds = Some(
                    value
                        .parse::<f64>()
                        .map_err(|_| fail("profile timeout_seconds must be a number"))?,
                );
            }
            "disabled" => {
                profile.disabled = match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err(fail("profile disabled must be true or false")),
                };
            }
            other => return Err(fail(format!("unknown profile key {other}"))),
        }
    }
    if let Some(done) = current.take() {
        profiles.push(done);
    }
    if schema.as_deref() != Some(PROFILES_SCHEMA) {
        return Err(fail(format!("profile schema must be {PROFILES_SCHEMA}")));
    }
    if profiles.is_empty() {
        return Err(fail("at least one [[profile]] is required"));
    }
    let mut names = std::collections::BTreeSet::new();
    for profile in &profiles {
        if profile.name.is_empty() || profile.harness.is_empty() {
            return Err(fail("each profile needs name and harness"));
        }
        if !names.insert(profile.name.clone()) {
            return Err(fail(format!("duplicate profile name {}", profile.name)));
        }
        if !matches!(profile.harness.as_str(), "muse" | "claude" | "codex") {
            return Err(fail(format!("unknown harness {}", profile.harness)));
        }
        if let Some(command) = &profile.command {
            if !Path::new(command).is_absolute() {
                return Err(fail(format!(
                    "profile {} command must be absolute",
                    profile.name
                )));
            }
        }
        if profile
            .timeout_seconds
            .is_some_and(|timeout| timeout <= 0.0)
        {
            return Err(fail(format!(
                "profile {} timeout_seconds must be positive",
                profile.name
            )));
        }
    }
    Ok(profiles)
}

/// Load and validate the profile file at `path`.
pub fn load_profiles(path: &Path) -> Result<Vec<RecoveryProfile>> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| fail(format!("cannot read {}: {error}", path.display())))?;
    parse_profiles(&text)
}

/// SHA-256 of a profile file's bytes; smoke records bind to it.
pub fn profiles_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The team recovery state directory: `<state root>/recovery/<key>/`.
pub fn recovery_dir(state_dir: Option<&Path>, registry: &Path) -> Option<PathBuf> {
    Some(
        state_root(state_dir)?
            .join("recovery")
            .join(crate::restore::team_key(registry)),
    )
}

/// Whether judgement is currently trusted for this profile file: a
/// smoke record exists, binds to the file's current hash, and every
/// enabled profile passed. Anything else leaves judgement unavailable.
pub fn judgement_allowed(state_dir: Option<&Path>, registry: &Path, profile_file: &Path) -> bool {
    let Ok(bytes) = std::fs::read(profile_file) else {
        return false;
    };
    let Ok(profiles) = parse_profiles(&String::from_utf8_lossy(&bytes)) else {
        return false;
    };
    let Some(dir) = recovery_dir(state_dir, registry) else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(dir.join("install-smoke.json")) else {
        return false;
    };
    let Ok(record) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    if record["schema"] != SMOKE_SCHEMA || record["config_hash"] != profiles_hash(&bytes) {
        return false;
    }
    profiles
        .iter()
        .filter(|profile| !profile.disabled)
        .all(|profile| {
            record["profiles"].as_array().is_some_and(|rows| {
                rows.iter()
                    .any(|row| row["name"] == profile.name && row["status"] == "pass")
            })
        })
}

/// What claiming an incident should do, decided purely.
#[derive(Clone, Debug, PartialEq)]
pub enum ClaimDecision {
    /// No live claimant: take the incident.
    Take,
    /// A previous-boot claim: it never blocks a new boot.
    TakeOverPreviousBoot,
    /// A dead or stale claimant in this boot: take over, recorded.
    TakeOverStale,
    /// A live claimant in this boot owns the incident.
    AlreadyRunning,
}

/// Decide from a claim record. A claim from a different boot is stale
/// by definition, whatever its heartbeat says (the reboot rule).
pub fn claim_decision(
    claim: Option<&Value>,
    boot_id: &str,
    now: f64,
    stale_seconds: f64,
    pid_alive: bool,
) -> ClaimDecision {
    let Some(claim) = claim else {
        return ClaimDecision::Take;
    };
    if claim["boot_id"].as_str() != Some(boot_id) {
        return ClaimDecision::TakeOverPreviousBoot;
    }
    let heartbeat = claim["heartbeat_at"].as_f64().unwrap_or(0.0);
    if pid_alive && now - heartbeat < stale_seconds {
        ClaimDecision::AlreadyRunning
    } else {
        ClaimDecision::TakeOverStale
    }
}

/// Refresh a claim's heartbeat under its short flock: constant-time
/// file work only, never across a round's long operations.
pub fn heartbeat_claim(path: &Path, incident: &str, boot_id: &str) {
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))
    else {
        return;
    };
    if file.lock_exclusive().is_err() {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d: Duration| d.as_secs_f64())
        .unwrap_or(0.0);
    let mut record: Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(
            || json!({"schema": CLAIM_SCHEMA, "incident": incident, "boot_id": boot_id}),
        );
    record["heartbeat_at"] = json!(now);
    // The claim's pid names the incident owner (the supervisor), set
    // by whoever claimed it; a heartbeat refresh never replaces it.
    if record["pid"].is_null() {
        record["pid"] = json!(std::process::id());
    }
    let _ = crate::agent::atomic_json(path, &record);
}

/// The fixed harmless smoke prompt: an exact final JSON marker, so a
/// pass proves executable resolution, credentials, model/effort
/// selection, timeout handling, and the structured reply shape.
pub const SMOKE_PROMPT: &str = "You are smoke-testing a recovery profile. \
Reply with exactly one final line and nothing after it:\n\
{\"marker\": \"agentctl-recovery-smoke\"}";

/// Whether an answer passes smoke: its last JSON line is the marker.
pub fn smoke_passed(answer: &str) -> bool {
    answer.lines().rev().find_map(|line| {
        let value: Value = serde_json::from_str(line.trim()).ok()?;
        Some(value["marker"] == "agentctl-recovery-smoke")
    }) == Some(true)
}

/// Run `recovery smoke`: every enabled profile (or one named profile)
/// plus one whole-chain run, recording `install-smoke.json`.
pub fn smoke_profiles(
    registry: &Path,
    profile_file: &Path,
    wrapper: &Path,
    only: Option<&str>,
    state_dir: Option<&Path>,
) -> Result<(Value, i32)> {
    let profiles = load_profiles(profile_file)?;
    let bytes = std::fs::read(profile_file)
        .map_err(|error| fail(format!("cannot read {}: {error}", profile_file.display())))?;
    let dir = recovery_dir(state_dir, registry)
        .ok_or_else(|| fail("cannot determine the agentctl state directory"))?;
    std::fs::create_dir_all(&dir)
        .map_err(|error| fail(format!("cannot create {}: {error}", dir.display())))?;
    let prompt_path = dir.join("smoke-prompt.txt");
    std::fs::write(&prompt_path, SMOKE_PROMPT)
        .map_err(|error| fail(format!("cannot write the smoke prompt: {error}")))?;
    let mut rows: Vec<Value> = Vec::new();
    let mut all_pass = true;
    for profile in profiles.iter().filter(|profile| !profile.disabled) {
        if let Some(name) = only {
            if profile.name != name {
                continue;
            }
        }
        let started = Instant::now();
        let timeout = profile.timeout_seconds.unwrap_or(120.0);
        let result = run_bounded(
            wrapper,
            &[
                "--profile-file".to_owned(),
                profile_file.display().to_string(),
                "--profile".to_owned(),
                profile.name.clone(),
                "--prompt-file".to_owned(),
                prompt_path.display().to_string(),
                "--timeout".to_owned(),
                timeout.to_string(),
            ],
            Duration::from_secs_f64(timeout * 3.0 + 30.0),
        );
        let (status, detail) = match result {
            Ok((0, answer, _)) if smoke_passed(&answer) => {
                ("pass", "answered with the smoke marker")
            }
            Ok((0, _, _)) => ("fail", "answered without the smoke marker"),
            Ok((69, _, _)) => ("fail", "every harness failed"),
            Ok((code, _, _)) => {
                all_pass = false;
                rows.push(json!({"name": profile.name, "harness": profile.harness,
                    "model": profile.model, "effort": profile.effort,
                    "status": "fail", "detail": format!("wrapper exit {code}"),
                    "duration_s": started.elapsed().as_secs_f64()}));
                continue;
            }
            Err(_error) => ("fail", "wrapper could not run"),
        };
        if status != "pass" {
            all_pass = false;
        }
        let _ = detail;
        rows.push(json!({"name": profile.name, "harness": profile.harness,
            "model": profile.model, "effort": profile.effort,
            "status": status,
            "duration_s": started.elapsed().as_secs_f64()}));
    }
    // One whole-chain run proves fall-through order end to end.
    let chain_started = Instant::now();
    let chain = run_bounded(
        wrapper,
        &[
            "--profile-file".to_owned(),
            profile_file.display().to_string(),
            "--prompt-file".to_owned(),
            prompt_path.display().to_string(),
            "--timeout".to_owned(),
            "120".to_owned(),
        ],
        Duration::from_secs(390),
    );
    let (chain_status, judge) = match &chain {
        Ok((0, answer, log)) if smoke_passed(answer) => {
            ("pass", judge_of(log).unwrap_or_else(|| "none".to_owned()))
        }
        _ => ("fail", "none".to_owned()),
    };
    if chain_status != "pass" {
        all_pass = false;
    }
    let record = json!({
        "schema": SMOKE_SCHEMA,
        "at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d: Duration| d.as_secs_f64())
            .unwrap_or(0.0),
        "config_hash": profiles_hash(&bytes),
        "profiles": rows,
        "chain": {"status": chain_status, "judge": judge,
                  "duration_s": chain_started.elapsed().as_secs_f64()},
    });
    crate::agent::atomic_json(&dir.join("install-smoke.json"), &record)
        .map_err(|error| fail(error.to_string()))?;
    Ok((record, if all_pass { 0 } else { 75 }))
}

/// Options for the one `start-recovery` entry point.
#[derive(Clone, Debug)]
pub struct StartOptions {
    /// Keep the supervisor in the foreground (container/manual use).
    pub foreground: bool,
    /// Seconds before a same-boot claim with no heartbeat is stale.
    pub stale_seconds: f64,
    /// The `run-agent-prompt` executable for the supervisor child.
    pub run_agent_prompt: PathBuf,
}

/// Claim or join the incident, then launch restore and the recovery
/// supervisor concurrently. This is the single trigger-neutral entry
/// point: a service unit, a script, a remote shell, and a human all
/// call it, and a second live call joins instead of starting a
/// second restorer. The claim flock is held only for the claim
/// read/write itself.
pub fn start_recovery(
    registry: &Path,
    host: &dyn Host,
    inputs: &ShepherdInputs,
    state_dir: Option<&Path>,
    options: &StartOptions,
) -> Result<Value> {
    crate::team_driver::require_host_namespace(host)?;
    if !registry.is_dir() {
        return Ok(json!({"schema": "agentctl-start-recovery/v1",
            "status": "BLOCKED_PREREQUISITE",
            "detail": format!("registry {} is not a directory", registry.display()),
            "exit_code": 1}));
    }
    let boot_id = host.boot_id()?;
    let epoch = RestoreEpoch {
        boot_id: boot_id.clone(),
        user_manager: host.user_manager_id(),
    };
    let incident = crate::team_driver::incident_id(registry, &epoch);
    let dir = recovery_dir(state_dir, registry)
        .ok_or_else(|| fail("cannot determine the agentctl state directory"))?;
    std::fs::create_dir_all(&dir)
        .map_err(|error| fail(format!("cannot create {}: {error}", dir.display())))?;
    let claim_path = dir.join("claim.json");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d: Duration| d.as_secs_f64())
        .unwrap_or(0.0);
    // Claim: short flock, constant-time file work only.
    let decision = {
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("claim.lock"))
            .map_err(|error| fail(format!("cannot open the claim lock: {error}")))?;
        lock.lock_exclusive()
            .map_err(|error| fail(format!("cannot take the claim lock: {error}")))?;
        let existing: Option<Value> = std::fs::read_to_string(&claim_path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());
        let pid_alive = existing
            .as_ref()
            .and_then(|claim| claim["pid"].as_u64())
            .is_some_and(|pid| Path::new(&format!("/proc/{pid}")).exists());
        let decision = claim_decision(
            existing.as_ref(),
            &boot_id,
            now,
            options.stale_seconds,
            pid_alive,
        );
        if decision != ClaimDecision::AlreadyRunning {
            let claim = json!({
                "schema": CLAIM_SCHEMA,
                "registry": registry,
                "incident": incident,
                "boot_id": boot_id,
                "epoch": epoch.as_json(),
                "pid": std::process::id(),
                "heartbeat_at": now,
                "started_at": now,
                "takeover": format!("{decision:?}"),
            });
            crate::agent::atomic_json(&claim_path, &claim)
                .map_err(|error| fail(error.to_string()))?;
        }
        decision
    };
    if decision == ClaimDecision::AlreadyRunning {
        return Ok(json!({"schema": "agentctl-start-recovery/v1",
            "status": "already-running", "incident": incident,
            "claim": claim_path, "exit_code": 75}));
    }
    // Launch restore and the supervisor concurrently; neither waits
    // for the other, and both are idempotent underneath (restore's
    // replay guard, team act request ids).
    let base = vec![
        "--registry".to_owned(),
        registry.display().to_string(),
        "--herdr-bin".to_owned(),
        inputs.herdr_bin.display().to_string(),
    ];
    let spawn_logged = |args: &[String], log_name: &str| -> Result<std::process::Child> {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(log_name))
            .map_err(|error| fail(format!("cannot open the recovery log: {error}")))?;
        let err = log
            .try_clone()
            .map_err(|error| fail(format!("cannot clone the recovery log: {error}")))?;
        Command::new(&inputs.agentctl)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err))
            .env("AGENTCTL_RECOVERY_CLAIM_FILE", &claim_path)
            .spawn()
            .map_err(|error| fail(format!("cannot launch a recovery child: {error}")))
    };
    let mut restore_args = base.clone();
    restore_args.push("restore".to_owned());
    let mut restore_child = spawn_logged(&restore_args, "restore.log")?;
    let mut shepherd_args = base;
    shepherd_args.extend([
        "team".to_owned(),
        "shepherd".to_owned(),
        "--run-agent-prompt".to_owned(),
        options.run_agent_prompt.display().to_string(),
    ]);
    if let Some(state) = state_dir {
        shepherd_args.extend(["--state-dir".to_owned(), state.display().to_string()]);
    }
    let mut supervisor = spawn_logged(&shepherd_args, "supervisor.log")?;
    // The claim belongs to the supervisor, not this short-lived
    // starter: rewrite its pid under the short flock.
    {
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("claim.lock"))
            .map_err(|error| fail(format!("cannot open the claim lock: {error}")))?;
        lock.lock_exclusive()
            .map_err(|error| fail(format!("cannot take the claim lock: {error}")))?;
        if let Ok(text) = std::fs::read_to_string(&claim_path) {
            if let Ok(mut claim) = serde_json::from_str::<Value>(&text) {
                claim["pid"] = json!(supervisor.id());
                let _ = crate::agent::atomic_json(&claim_path, &claim);
            }
        }
    }
    heartbeat_claim(&claim_path, &incident, &boot_id);
    if options.foreground {
        let status = supervisor
            .wait()
            .map_err(|error| fail(format!("cannot wait on the supervisor: {error}")))?;
        let _ = restore_child.wait();
        return Ok(json!({"schema": "agentctl-start-recovery/v1",
            "status": "finished", "incident": incident, "claim": claim_path,
            "exit_code": status.code().unwrap_or(1)}));
    }
    Ok(json!({"schema": "agentctl-start-recovery/v1",
        "status": "started", "incident": incident, "claim": claim_path,
        "restore_pid": restore_child.id(), "supervisor_pid": supervisor.id(),
        "exit_code": 0}))
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    #[test]
    fn profiles_parse_in_file_order_and_refuse_guesses() {
        let text = "schema = \"agentctl-recovery-profiles/v1\"\n\
                    [[profile]]\nname = \"local-claude\"\nharness = \"claude\"\n\
                    [[profile]]\nname = \"local-codex\"\nharness = \"codex\"\nmodel = \"m\"\n";
        let profiles = parse_profiles(text).expect("profiles");
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].name, "local-claude");
        assert_eq!(profiles[1].model.as_deref(), Some("m"));
        assert!(parse_profiles(
            "schema = \"nope\"\n[[profile]]\nname = \"a\"\nharness = \"claude\"\n"
        )
        .is_err());
        assert!(parse_profiles(
            "schema = \"agentctl-recovery-profiles/v1\"\n[[profile]]\nname = \"a\"\nharness = \"claude\"\ncommand = \"relative\"\n"
        )
        .is_err());
    }

    #[test]
    fn previous_boot_claim_never_blocks() {
        let claim = json!({"boot_id": "old", "pid": 999999, "heartbeat_at": 10.0});
        assert_eq!(
            claim_decision(Some(&claim), "new", 20.0, 180.0, false),
            ClaimDecision::TakeOverPreviousBoot
        );
        let live = json!({"boot_id": "new", "pid": 1, "heartbeat_at": 19.0});
        assert_eq!(
            claim_decision(Some(&live), "new", 20.0, 180.0, true),
            ClaimDecision::AlreadyRunning
        );
        assert_eq!(
            claim_decision(Some(&live), "new", 20.0, 180.0, false),
            ClaimDecision::TakeOverStale
        );
        assert_eq!(
            claim_decision(None, "new", 20.0, 180.0, false),
            ClaimDecision::Take
        );
    }

    #[test]
    fn smoke_needs_the_exact_final_marker() {
        assert!(smoke_passed(
            "ok\n{\"marker\": \"agentctl-recovery-smoke\"}"
        ));
        assert!(!smoke_passed("{\"marker\": \"other\"}"));
        assert!(!smoke_passed("no marker"));
    }
}
