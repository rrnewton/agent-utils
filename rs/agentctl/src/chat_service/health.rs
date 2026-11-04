//! `chat health`: whether a bridge is working, in one line, and why (#677 chat-bridge-repoint).
//! It reads the bridge's records, `/proc/locks` and the registry, and asks Herdr about the target
//! agent's pane; it writes nothing and never contacts the chat provider.

use std::fmt::Write as _;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::chat_runtime::{self, BridgeState, RequestTimes, UnsentReply};
use crate::subagents::{ManagedAgents, ManagedApi};

use super::{age_text, channel_lock, ChatServiceError};

/// Send attempts at which an unsent reply counts as a retry loop.
pub const RETRY_LOOP_ATTEMPTS: u64 = 20;
/// Age at which a captured reply that is still not sent counts as stuck.
const UNSENT_REPLY_STUCK: Duration = Duration::from_secs(10 * 60);
/// Requests each list in the report names before it counts the rest.
const MAX_LISTED: usize = 10;
/// Exit status of `chat health` when the bridge is down.
pub const EXIT_DOWN: i32 = 1;
/// Exit status of `chat health` when the bridge works with problems.
pub const EXIT_DEGRADED: i32 = 3;
/// Exit status of `chat health` when the service runs but its owner loop has stalled: the same
/// as down, since nothing is delivered or sent (#783 chat-reply-capture-miss).
pub const EXIT_STALLED: i32 = EXIT_DOWN;

/// `chat health` settings.
#[derive(Clone, Copy, Debug)]
pub struct HealthOptions {
    /// Age of the last provider commit past which the event stream counts as stale.
    pub stale_after: Duration,
}

/// One `chat health` report.
#[derive(Debug)]
pub struct Health {
    /// `ok`, `degraded`, `stalled` or `down`.
    pub verdict: &'static str,
    /// The whole report, as `--json` prints it.
    pub document: Value,
    /// The report as text: the verdict line, then details.
    pub text: String,
}

impl Health {
    /// The command's exit status: 0 when ok, [`EXIT_DEGRADED`], [`EXIT_STALLED`] or
    /// [`EXIT_DOWN`].
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self.verdict {
            "ok" => 0,
            "degraded" => EXIT_DEGRADED,
            "stalled" => EXIT_STALLED,
            _ => EXIT_DOWN,
        }
    }
}

#[derive(Default)]
struct Problems {
    down: Vec<String>,
    stalled: Vec<String>,
    degraded: Vec<String>,
}

fn when(millis: Option<u64>, now: u64) -> String {
    millis.map_or_else(
        || "never".to_owned(),
        |at| {
            format!(
                "{} ({} ago)",
                chat_runtime::utc_timestamp(at),
                age_text(now.saturating_sub(at))
            )
        },
    )
}

fn short(key: &str) -> &str {
    key.get(..12).unwrap_or(key)
}

/// What Herdr and the registry say about the agent the bridge types into.
fn probe_target<A: ManagedApi + ?Sized>(
    manager: &ManagedAgents<'_, A>,
    agent: &str,
) -> (Value, Option<String>) {
    match manager.is_headless(agent) {
        Err(error) => (
            json!({"agent": agent, "alive": false, "problem": error.to_string()}),
            Some(format!(
                "target {agent} is not a usable registered agent: {error}"
            )),
        ),
        Ok(true) => match manager.status(agent) {
            Ok(status) => {
                let agent_status = status["agent_status"].as_str().unwrap_or("unknown");
                let alive = matches!(agent_status, "working" | "idle" | "starting");
                (
                    json!({"agent": agent, "headless": true, "status": agent_status, "alive": alive}),
                    (!alive).then(|| format!("headless target {agent} is {agent_status}")),
                )
            }
            Err(error) => (
                json!({"agent": agent, "headless": true, "alive": false, "problem": error.to_string()}),
                Some(format!(
                    "headless target {agent}'s status is unreadable: {error}"
                )),
            ),
        },
        Ok(false) if manager.is_cloud(agent).unwrap_or(false) => match manager.status(agent) {
            Ok(status) => {
                let agent_status = status["agent_status"].as_str().unwrap_or("unknown");
                let alive = matches!(agent_status, "working" | "idle" | "starting" | "waiting");
                (
                    json!({"agent": agent, "headless": false, "cloud": true, "status": agent_status, "alive": alive}),
                    (!alive).then(|| {
                        format!(
                            "agentcloud target {agent} is {agent_status}{}",
                            status["probe_error"]
                                .as_str()
                                .map(|error| format!(": {error}"))
                                .unwrap_or_default()
                        )
                    }),
                )
            }
            Err(error) => (
                json!({"agent": agent, "headless": false, "cloud": true, "alive": false, "problem": error.to_string()}),
                Some(format!(
                    "agentcloud target {agent}'s status is unreadable: {error}"
                )),
            ),
        },
        Ok(false) => match manager.pane_info(agent) {
            Ok(info) => {
                let problem = info.agent.is_none().then(|| {
                    format!(
                        "Herdr detects no agent harness in target {agent}'s pane {}",
                        info.pane_id
                    )
                });
                (
                    json!({
                        "agent": agent,
                        "headless": false,
                        "pane_id": info.pane_id,
                        "harness": info.agent,
                        "status": info.status,
                        "alive": true,
                    }),
                    problem,
                )
            }
            Err(error) => (
                json!({"agent": agent, "headless": false, "alive": false, "problem": error.to_string()}),
                Some(format!("target {agent}'s pane is not alive: {error}")),
            ),
        },
    }
}

/// Who holds each configured channel's lock.
fn channel_holders(configuration: &chat_runtime::BridgeConfiguration) -> Vec<Value> {
    let directory = channel_lock::lock_directory();
    configuration
        .channel_ids
        .iter()
        .map(|channel| {
            let path =
                channel_lock::lock_path(&directory, &configuration.subscription_plugin, channel);
            let holder = crate::lock_wait::holder_of(&path);
            let recorded: Option<Value> = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str(text.trim()).ok());
            json!({
                "channel_id": channel,
                "lock": path.display().to_string(),
                "holder_pid": holder,
                "holder_bridge_state": recorded.as_ref().and_then(|value| value["bridge_state"].as_str()),
            })
        })
        .collect()
}

/// The `event-sinks.json` of an older sibling state (`NAME.old-*`, the shape a
/// moved-aside state takes) when the live state configures no sinks: the sinks
/// were left behind by a re-initialisation, and the bridge pushes nothing.
fn stranded_sink_config(state_root: &Path) -> Option<PathBuf> {
    if state_root.join(crate::chat_event_sink::SINKS_FILE).exists() {
        return None;
    }
    let parent = state_root.parent()?;
    let live = state_root.file_name()?.to_str()?;
    let prefix = format!("{live}.old");
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(parent)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .map(|path| path.join(crate::chat_event_sink::SINKS_FILE))
        .filter(|path| path.is_file())
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

/// The registry a bridge runs with, and how `chat health` learned it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeRegistry {
    /// The registry directory.
    pub path: PathBuf,
    /// `service command line` or `bridge index`.
    pub source: &'static str,
}

/// The registry the bridge whose state is `state_root` runs with: the `--registry` argument of
/// its running `chat run` (resolved against that process's working directory), else the
/// registry that `chat run` recorded for the state in the bridge index; `None` when neither is
/// known (#791 chat-health-wrong-registry).
#[must_use]
pub fn bridge_registry(state_root: &Path) -> Option<BridgeRegistry> {
    let runner = BridgeState::inspect(state_root)
        .ok()
        .and_then(|state| state.runner_pid());
    if let Some(path) = runner.and_then(service_registry_argument) {
        return Some(BridgeRegistry {
            path,
            source: "service command line",
        });
    }
    crate::chat_lookup::registry_for_state(state_root).map(|path| BridgeRegistry {
        path,
        source: "bridge index",
    })
}

/// The `--registry` value of process `pid`'s command line, made absolute against its working
/// directory.
fn service_registry_argument(pid: u64) -> Option<PathBuf> {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let argv: Vec<&[u8]> = cmdline.split(|byte| *byte == 0).collect();
    let value = argv.windows(2).find_map(|pair| {
        if pair[0] == b"--registry" {
            Some(pair[1])
        } else {
            pair[0].strip_prefix(b"--registry=")
        }
    })?;
    let value = PathBuf::from(std::ffi::OsStr::from_bytes(value));
    if value.as_os_str().is_empty() {
        return None;
    }
    if value.is_absolute() {
        return Some(value);
    }
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    Some(cwd.join(value))
}

/// Which registry `chat health` probes the target in, and what it says about that.
#[derive(Clone, Debug)]
pub struct RegistryChoice {
    /// The registry to probe the target in.
    pub probe: PathBuf,
    /// The `registry` section of the report.
    pub document: Value,
    /// The report line, when the bridge's registry is not the one the command was given.
    pub line: Option<String>,
    /// Set when the bridge's registry differs and cannot be used: a target the given registry
    /// does not know is then reported as this mismatch, not as down.
    pub mismatch: Option<String>,
}

fn same_directory(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Probe the target in the bridge's own registry when it is known and differs from `given`, the
/// registry this command was given, so a caller in another project does not see a working
/// bridge as down; `given` otherwise.
#[must_use]
pub fn choose_registry(given: &Path, bridge: Option<BridgeRegistry>) -> RegistryChoice {
    let document = |probe: &Path| {
        json!({
            "given": given,
            "bridge": bridge.as_ref().map(|found| &found.path),
            "bridge_source": bridge.as_ref().map(|found| found.source),
            "probed": probe,
        })
    };
    match bridge.as_ref() {
        Some(found) if !same_directory(&found.path, given) => {
            if found.path.is_dir() {
                RegistryChoice {
                    probe: found.path.clone(),
                    document: document(&found.path),
                    line: Some(format!(
                        "registry: the bridge runs with {} (from its {}), not {}, which this \
                         command was given; the target was looked up in the bridge's",
                        found.path.display(),
                        found.source,
                        given.display()
                    )),
                    mismatch: None,
                }
            } else {
                let mismatch = format!(
                    "registry mismatch: the bridge runs with {} (from its {}), which cannot be \
                     read here; the target was looked up in {}, which this command was given",
                    found.path.display(),
                    found.source,
                    given.display()
                );
                RegistryChoice {
                    probe: given.to_path_buf(),
                    document: document(given),
                    line: Some(mismatch.clone()),
                    mismatch: Some(mismatch),
                }
            }
        }
        _ => RegistryChoice {
            probe: given.to_path_buf(),
            document: document(given),
            line: None,
            mismatch: None,
        },
    }
}

/// What `chat health` read, before it judges it.
struct Observed {
    configuration: chat_runtime::BridgeConfiguration,
    /// `chat status`'s document.
    status: Value,
    runner: Option<u64>,
    runner_command: Option<String>,
    /// [`channel_holders`].
    channels: Vec<Value>,
    checkpoint_age: Option<u64>,
    alive_age: Option<u64>,
    /// How long ago the owner loop last turned, with the stall bound it runs with.
    owner_loop: Option<(u64, Option<Duration>)>,
    /// [`probe_target`].
    target: (Value, Option<String>),
    times: Vec<RequestTimes>,
    unsent: Vec<UnsentReply>,
    /// [`stranded_sink_config`], as text.
    stranded_sinks: Option<String>,
    /// [`choose_registry`].
    registry: RegistryChoice,
    /// The target's harness and the conversation its record names, when its record is readable.
    binding: Option<(String, Option<String>)>,
    /// `reply-source.json`: where reply text comes from, and the capture misses.
    reply_source: Option<Value>,
    /// The replies a restart held back as stale.
    held: Vec<chat_runtime::HeldReply>,
    now: u64,
}

/// Build the `chat health` report for the bridge state at `state_root`, with `manager` on the
/// registry `registry` chose ([`choose_registry`]).
pub fn health<A: ManagedApi + ?Sized>(
    state_root: &Path,
    manager: &ManagedAgents<'_, A>,
    options: HealthOptions,
    registry: RegistryChoice,
) -> Result<Health, ChatServiceError> {
    let state = BridgeState::inspect(state_root)?;
    let now = chat_runtime::unix_millis();
    let runner = state.runner_pid();
    let observed = Observed {
        status: state.status()?,
        runner,
        runner_command: runner.and_then(crate::lock_wait::command_of),
        channels: channel_holders(state.config()),
        checkpoint_age: state.checkpoint_age_millis(now),
        alive_age: state.provider_alive_age_millis(now),
        owner_loop: state.owner_loop_alive(now),
        target: probe_target(manager, &state.config().agent_name),
        times: state.request_times()?,
        unsent: state.unsent_replies()?,
        stranded_sinks: stranded_sink_config(state_root).map(|path| path.display().to_string()),
        configuration: state.config().clone(),
        binding: crate::chat_transcript::recorded_conversation(
            &registry.probe,
            &state.config().agent_name,
        ),
        reply_source: state.reply_source_record(),
        held: state.held_replies()?,
        registry,
        now,
    };
    Ok(assess(observed, options))
}

/// Judge what `chat health` read.
fn assess(observed: Observed, options: HealthOptions) -> Health {
    let Observed {
        configuration,
        status,
        runner,
        runner_command,
        channels,
        checkpoint_age,
        alive_age,
        owner_loop,
        target: (target, target_problem),
        times,
        unsent,
        stranded_sinks,
        registry,
        binding,
        reply_source,
        held,
        now,
    } = observed;
    let configuration = &configuration;
    let mut problems = Problems::default();
    // A record written while the bridge served another agent says nothing about this one: the
    // service writes this agent's at its next scan (#852 unknown-tui-bridge).
    let stale_source_agent = reply_source
        .as_ref()
        .and_then(|record| record["agent"].as_str())
        .filter(|agent| *agent != configuration.agent_name)
        .map(str::to_owned);
    let reply_source = reply_source.filter(|_| stale_source_agent.is_none());
    let process = match runner {
        Some(pid) => json!({"running": true, "pid": pid, "command": runner_command}),
        None => {
            problems
                .down
                .push("no chat run holds this state (its runner lock is free)".to_owned());
            json!({"running": false})
        }
    };
    for channel in &channels {
        let id = channel["channel_id"].as_str().unwrap_or("?");
        let other = channel["holder_bridge_state"]
            .as_str()
            .map(|state| format!(", state {state}"))
            .unwrap_or_default();
        match (channel["holder_pid"].as_u64(), runner) {
            (Some(pid), Some(runner)) if pid != runner => problems.down.push(format!(
                "channel {id} is held by another bridge (process {pid}{other})"
            )),
            (Some(pid), None) => problems.down.push(format!(
                "channel {id} is held by another bridge (process {pid}{other}) while this state \
                 has no running service"
            )),
            _ => {}
        }
    }

    let stale_millis = u64::try_from(options.stale_after.as_millis()).unwrap_or(u64::MAX);
    if status["runtime_evidence"]["healthy"] == Value::Bool(false) {
        problems.down.push(format!(
            "the event stream stopped: {}",
            status["runtime_evidence"]["basis"]
                .as_str()
                .unwrap_or("see chat status")
        ));
    } else if runner.is_some() && alive_age.is_some_and(|age| age > stale_millis) {
        problems.degraded.push(format!(
            "the event stream has delivered nothing, not even a heartbeat, for {} (stale after \
             {})",
            age_text(alive_age.unwrap_or(0)),
            age_text(stale_millis)
        ));
    }
    for (field, label) in [
        ("subscription_down", "the chat subscription is down"),
        ("send_path_down", "the reply send path is down"),
    ] {
        let value = &status["delivery_alarm"][field];
        if !value.is_null() {
            problems.degraded.push(format!("{label}: {value}"));
        }
    }
    if !status["service_health"]["owner_loop"].is_null() {
        problems.degraded.push(format!(
            "the owner loop is failing: {}",
            status["service_health"]["owner_loop"]["last_error"]
                .as_str()
                .unwrap_or("see chat status")
        ));
    }

    if let Some(problem) = target_problem {
        // Looked up in a registry the bridge does not use, an unknown or dead target says
        // nothing about the bridge: report the mismatch, not down (#791).
        match registry.mismatch.as_ref() {
            Some(mismatch) => problems.degraded.push(format!("{mismatch} ({problem})")),
            None => problems.down.push(problem),
        }
    }
    if let Some(old) = &stranded_sinks {
        problems.degraded.push(format!(
            "event sinks are configured in the older state {old} but not in this state, so \
             posts are not pushed to them; carry them over with `chat init --from` on the old \
             state directory, or copy its event-sinks.json and token files here"
        ));
    }

    // A coordinator whose record names no conversation may have its transcript found only by
    // discovery, or not at all, which leaves reply text to its screen (#783
    // chat-reply-capture-miss).
    let headless = target["headless"].as_bool() == Some(true);
    let source = reply_source.as_ref().map(|record| &record["source"]);
    let binding_line = match (&binding, headless) {
        (Some((harness, None)), false) if agent_harness::transcript(harness).is_some() => {
            let agent = &configuration.agent_name;
            let found_by_record = source.is_some_and(|source| {
                !source["path"].is_null() && source["discovered"] != Value::Bool(true)
            });
            let reading = match source {
                None => "the running service records no reply source, so its transcript may not \
                         be found and reply text may come only from its screen"
                    .to_owned(),
                Some(source) if source["path"].is_null() => {
                    "no transcript of it was found, so reply text comes only from its screen"
                        .to_owned()
                }
                Some(source) => format!(
                    "the bridge reads its transcript {} ({})",
                    source["path"].as_str().unwrap_or("?"),
                    source["evidence"].as_str().unwrap_or("?")
                ),
            };
            let remedy = match source.and_then(|source| source["unbound_session"].as_str()) {
                Some(session) => format!(
                    "; the bridge binds conversation {session}, which a live process names, \
                     once the record's locks are free"
                ),
                None => format!("; bind it with `agentctl bind-session {agent} SESSION_ID`"),
            };
            let line = format!("target {agent} has no native session binding: {reading}{remedy}");
            if !found_by_record {
                problems.degraded.push(line.clone());
            }
            Some(line)
        }
        _ => None,
    };
    let misses: Vec<&Value> = reply_source
        .as_ref()
        .and_then(|record| record["capture_misses"].as_array())
        .map(|misses| misses.iter().collect())
        .unwrap_or_default();
    if runner.is_some() && !misses.is_empty() {
        problems.degraded.push(format!(
            "{} reply block(s) in {}'s transcript were never captured (reply ID {}); the owner \
             and {} were warned",
            misses.len(),
            configuration.agent_name,
            misses
                .iter()
                .take(MAX_LISTED)
                .map(|miss| miss["reply_id"].as_str().unwrap_or("?"))
                .collect::<Vec<_>>()
                .join(", "),
            configuration.agent_name
        ));
    }

    let last = |pick: fn(&RequestTimes) -> Option<u64>| times.iter().filter_map(pick).max();
    let last_admitted = last(|request| Some(request.admitted_at_millis));
    let last_delivered = last(|request| request.delivered_at_millis);
    let last_replied = last(|request| request.last_reply_sent_at_millis);

    // The alarm lists every request not typed, uncertain ones too; those are reported below.
    let stalled: Vec<Value> = status["delivery_alarm"]["stalled"]
        .as_array()
        .map(|stalled| {
            stalled
                .iter()
                .filter(|entry| entry["phase"] != "delivery_uncertain")
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if !stalled.is_empty() {
        problems.degraded.push(format!(
            "{} request(s) not typed into {} after {} or more",
            stalled.len(),
            configuration.agent_name,
            age_text(
                status["delivery_alarm"]["stall_after_seconds"]
                    .as_u64()
                    .unwrap_or(0)
                    .saturating_mul(1_000)
            )
        ));
    }
    let uncertain: Vec<&RequestTimes> = times
        .iter()
        .filter(|request| request.phase == "delivery_uncertain" && request.settlement.is_none())
        .collect();
    if !uncertain.is_empty() {
        problems.degraded.push(format!(
            "{} request(s) with uncertain delivery, not retyped",
            uncertain.len()
        ));
    }
    let awaiting: Vec<&RequestTimes> = times
        .iter()
        .filter(|request| {
            request.phase == "delivered" && request.replies_captured == 0 && !request.replies_closed
        })
        .collect();

    let loops: Vec<&UnsentReply> = unsent
        .iter()
        .filter(|reply| {
            reply
                .send_attempts
                .is_some_and(|count| count >= RETRY_LOOP_ATTEMPTS)
                || reply.send_permanent_failure.is_some()
        })
        .collect();
    for reply in &loops {
        problems.degraded.push(match &reply.send_permanent_failure {
            Some(reason) => format!(
                "reply {} of request {} will not be sent: {reason}",
                reply.ordinal,
                short(&reply.key)
            ),
            None => format!(
                "reply {} of request {} retried {} times (last failure: {})",
                reply.ordinal,
                short(&reply.key),
                reply.send_attempts.unwrap_or(0),
                reply.last_send_failure_code.as_deref().unwrap_or("unknown")
            ),
        });
    }
    // Held replies wait for the coordinator on purpose: they are reported below, not as stuck
    // (#781 chat-no-stale-replies-on-restart).
    if !held.is_empty() {
        problems.degraded.push(format!(
            "{} reply(ies) held back as stale after a restart, waiting for the coordinator to send \
             or drop each (`agentctl chat held`)",
            held.len()
        ));
    }
    let stuck_millis = u64::try_from(UNSENT_REPLY_STUCK.as_millis()).unwrap_or(u64::MAX);
    let stuck_replies = unsent
        .iter()
        .filter(|reply| {
            !held
                .iter()
                .any(|entry| entry.key == reply.key && entry.ordinal == reply.ordinal)
        })
        .filter(|reply| {
            !loops
                .iter()
                .any(|looping| looping.key == reply.key && looping.ordinal == reply.ordinal)
                && reply
                    .captured_at_millis
                    .is_some_and(|at| now.saturating_sub(at) > stuck_millis)
        })
        .count();
    if stuck_replies > 0 {
        problems.degraded.push(format!(
            "{stuck_replies} captured repl(ies) unsent for more than {}",
            age_text(stuck_millis)
        ));
    }
    let held_threads = status["reply_breaker"]["held_threads"]
        .as_array()
        .map_or(0, Vec::len);
    if held_threads > 0 {
        problems.degraded.push(format!(
            "the reply breaker holds {held_threads} thread(s) back"
        ));
    }
    if let Some(pending) = status["repoint"]["pending"].as_object() {
        problems.degraded.push(format!(
            "a repoint to {} requested by {} is not applied yet",
            pending
                .get("agent_name")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            pending
                .get("requested_by")
                .and_then(Value::as_str)
                .unwrap_or("?")
        ));
    }

    // A running service whose owner loop has not turned for its stall bound delivers and sends
    // nothing, whatever its saved records say.
    if let (Some(pid), Some((age, Some(bound)))) = (runner, owner_loop) {
        if u128::from(age) > bound.as_millis() {
            problems.stalled.push(format!(
                "the owner loop of service process {pid} has not turned for {} (bound {}); \
                 chat run stops and its unit starts it again",
                age_text(age),
                age_text(u64::try_from(bound.as_millis()).unwrap_or(u64::MAX))
            ));
        }
    }
    let verdict = if !problems.down.is_empty() {
        "down"
    } else if !problems.stalled.is_empty() {
        "stalled"
    } else if !problems.degraded.is_empty() {
        "degraded"
    } else {
        "ok"
    };
    let headline = match verdict {
        "ok" => {
            let into = target["pane_id"]
                .as_str()
                .map(|pane| format!(" in pane {pane}"))
                .unwrap_or_default();
            format!(
                "chat health: OK: typing into {}{into}; last request admitted {}",
                configuration.agent_name,
                last_admitted
                    .map(|at| format!("{} ago", age_text(now.saturating_sub(at))))
                    .unwrap_or_else(|| "never".to_owned())
            )
        }
        verdict => format!(
            "chat health: {}: {}",
            verdict.to_ascii_uppercase(),
            problems
                .down
                .iter()
                .chain(&problems.stalled)
                .chain(&problems.degraded)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        ),
    };

    let mut text = headline;
    text.push('\n');
    if let Some(line) = registry.line.as_ref() {
        let _ = writeln!(text, "{line}");
    }
    let _ = writeln!(
        text,
        "service: {}",
        match runner {
            Some(pid) => format!(
                "running, process {pid}{}",
                process["command"]
                    .as_str()
                    .map(|command| format!(" ({command})"))
                    .unwrap_or_default()
            ),
            None => "not running".to_owned(),
        }
    );
    for channel in &channels {
        let _ = writeln!(
            text,
            "channel {}: {}",
            channel["channel_id"].as_str().unwrap_or("?"),
            match channel["holder_pid"].as_u64() {
                Some(pid) if Some(pid) == runner => "served by this bridge".to_owned(),
                Some(pid) => format!("held by process {pid}"),
                None if runner.is_some() =>
                    "lock not held (the running build predates one bridge per channel)".to_owned(),
                None => "no bridge".to_owned(),
            }
        );
    }
    let _ = writeln!(
        text,
        "event stream: last item {}; checkpoint written {}; cursor {}",
        match alive_age {
            Some(age) => when(Some(now.saturating_sub(age)), now),
            None => "unknown (the service build records none)".to_owned(),
        },
        when(checkpoint_age.map(|age| now.saturating_sub(age)), now),
        if status["cursor_present"] == Value::Bool(true) {
            "saved"
        } else {
            "none yet"
        }
    );
    let _ = writeln!(
        text,
        "target: {}{}{}",
        configuration.agent_name,
        match (target["headless"].as_bool(), target["pane_id"].as_str()) {
            (Some(true), _) => " (headless)".to_owned(),
            (_, Some(pane)) => format!(
                " in pane {pane} ({}, {})",
                target["harness"].as_str().unwrap_or("no harness"),
                target["status"].as_str().unwrap_or("unknown")
            ),
            _ => String::new(),
        },
        if target["alive"] == Value::Bool(true) {
            ""
        } else {
            ": NOT ALIVE"
        }
    );
    if let Some(line) = &binding_line {
        let _ = writeln!(text, "{line}");
    }
    for entry in held.iter().take(MAX_LISTED) {
        let _ = writeln!(
            text,
            "  held: {} captured {}: {}",
            entry.handle(),
            when(Some(entry.captured_at_millis), now),
            entry.reason.describe()
        );
    }
    if let Some(source) = source {
        let _ = writeln!(
            text,
            "reply text: {}",
            match source["path"].as_str() {
                Some(path) => format!(
                    "transcript {path} ({}){}",
                    source["evidence"].as_str().unwrap_or("?"),
                    if source["discovered"] == Value::Bool(true) {
                        ", discovered"
                    } else {
                        ""
                    }
                ),
                None => format!(
                    "the screen only: {}",
                    source["evidence"]
                        .as_str()
                        .unwrap_or("no transcript was found")
                ),
            }
        );
    } else if let Some(previous) = &stale_source_agent {
        let _ = writeln!(
            text,
            "reply text: not looked for yet: the recorded source is the previous agent \
             {previous}'s, and the service records {}'s at its next scan",
            configuration.agent_name
        );
    }
    let _ = writeln!(text, "last admitted: {}", when(last_admitted, now));
    let _ = writeln!(text, "last delivered: {}", when(last_delivered, now));
    let _ = writeln!(text, "last reply sent: {}", when(last_replied, now));
    let _ = writeln!(
        text,
        "requests: {} retained; {} stalled; {} uncertain; {} delivered and unanswered; {} \
         unsent repl(ies)",
        times.len(),
        stalled.len(),
        uncertain.len(),
        awaiting.len(),
        unsent.len()
    );
    for request in uncertain.iter().take(MAX_LISTED) {
        let _ = writeln!(
            text,
            "  uncertain {}: {}",
            short(&request.key),
            request
                .delivery_error
                .as_deref()
                .map(|detail| detail.chars().take(240).collect::<String>())
                .unwrap_or_default()
        );
    }
    for reply in loops.iter().take(MAX_LISTED) {
        let _ = writeln!(
            text,
            "  retry loop: reply {} of {}: {} attempts, last failure {}",
            reply.ordinal,
            short(&reply.key),
            reply.send_attempts.unwrap_or(0),
            reply.last_send_failure_code.as_deref().unwrap_or("unknown")
        );
    }
    if let Some(last) = status["repoint"]["last"].as_object() {
        let _ = writeln!(
            text,
            "last repoint: {} -> {} by {}, {}",
            last.get("previous_agent")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            last.get("agent_name")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            last.get("requested_by")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            when(last.get("applied_at_millis").and_then(Value::as_u64), now)
        );
    }

    let document = json!({
        "schema": "agentctl-chat-health/v1",
        "verdict": verdict,
        "problems": {
            "down": problems.down,
            "stalled": problems.stalled,
            "degraded": problems.degraded,
        },
        "owner_loop": {
            "last_turn_age_seconds": owner_loop.map(|(age, _)| age / 1_000),
            "stall_after_seconds": owner_loop.and_then(|(_, bound)| bound).map(|bound| bound.as_secs()),
        },
        "service": process,
        "channels": channels,
        "event_stream": {
            "last_item_age_seconds": alive_age.map(|age| age / 1_000),
            "checkpoint_age_seconds": checkpoint_age.map(|age| age / 1_000),
            "stale_after_seconds": options.stale_after.as_secs(),
            "cursor_present": status["cursor_present"],
            "runtime_evidence": status["runtime_evidence"],
        },
        "target": target,
        "last_admitted_at_millis": last_admitted,
        "last_delivered_at_millis": last_delivered,
        "last_reply_sent_at_millis": last_replied,
        "requests": {
            "retained": times.len(),
            "stalled": stalled,
            "uncertain": uncertain.iter().map(|request| json!({
                "key": request.key,
                "admitted_at_millis": request.admitted_at_millis,
                "detail": request.delivery_error,
            })).collect::<Vec<_>>(),
            "delivered_unanswered": awaiting.iter().map(|request| json!({
                "key": request.key,
                "delivered_at_millis": request.delivered_at_millis,
            })).collect::<Vec<_>>(),
        },
        "unsent_replies": unsent,
        "event_sinks": {"stranded_config": stranded_sinks},
        "retry_loop_attempts": RETRY_LOOP_ATTEMPTS,
        "reply_breaker": status["reply_breaker"],
        "service_health": status["service_health"],
        "repoint": status["repoint"],
        "registry": registry.document,
        "binding": binding.as_ref().map(|(harness, session)| json!({
            "harness": harness,
            "native_session": session,
        })),
        "reply_source": reply_source,
        "held_replies": held,
    });
    Health {
        verdict,
        document,
        text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    fn configuration() -> chat_runtime::BridgeConfiguration {
        serde_json::from_value(json!({
            "subscription_plugin": "fixture",
            "channel_ids": ["C1"],
            "allowed_senders": ["U1"],
            "agent_name": "coord",
            "agent_label": "coord",
            "outbound_enabled": false,
            "ack_reaction": null
        }))
        .expect("configuration")
    }

    fn healthy() -> Observed {
        Observed {
            configuration: configuration(),
            status: json!({
                "cursor_present": true,
                "runtime_evidence": {"healthy": true, "basis": "fine"},
                "delivery_alarm": {
                    "stall_after_seconds": 60,
                    "stalled": [],
                    "subscription_down": null,
                    "send_path_down": null,
                },
                "service_health": {"owner_loop": null},
                "reply_breaker": {"held_threads": []},
                "repoint": {"pending": null, "last": null},
            }),
            runner: Some(42),
            runner_command: Some("agentctl chat run".to_owned()),
            channels: vec![
                json!({"channel_id": "C1", "lock": "/l", "holder_pid": 42, "holder_bridge_state": "/s"}),
            ],
            checkpoint_age: Some(5_000),
            alive_age: Some(10_000),
            owner_loop: Some((2_000, Some(Duration::from_secs(300)))),
            target: (
                json!({"agent": "coord", "headless": false, "pane_id": "w1:p2", "harness": "claude", "status": "idle", "alive": true}),
                None,
            ),
            times: vec![request("a", "delivered", 1)],
            unsent: Vec::new(),
            stranded_sinks: None,
            registry: choose_registry(Path::new("/registry"), None),
            binding: Some(("claude".to_owned(), Some("s-1".to_owned()))),
            reply_source: None,
            held: Vec::new(),
            now: NOW,
        }
    }

    fn request(key: &str, phase: &'static str, replies: u32) -> RequestTimes {
        RequestTimes {
            key: key.repeat(64),
            thread_id: "C1/threads/1".to_owned(),
            posted_at_millis: Some(NOW - 120_000),
            admitted_at_millis: NOW - 119_000,
            acknowledged_at_millis: Some(NOW - 118_000),
            delivered_at_millis: (phase == "delivered").then_some(NOW - 117_000),
            first_reply_sent_at_millis: (replies > 0).then_some(NOW - 100_000),
            last_reply_sent_at_millis: (replies > 0).then_some(NOW - 100_000),
            phase,
            delivery_error: (phase == "delivery_uncertain").then(|| "typed, then lost".to_owned()),
            replies_captured: replies,
            replies_closed: false,
            settlement: None,
        }
    }

    fn options() -> HealthOptions {
        HealthOptions {
            stale_after: Duration::from_secs(300),
        }
    }

    #[test]
    fn sinks_left_in_an_older_state_are_a_degraded_warning() {
        let mut observed = healthy();
        observed.stranded_sinks = Some("/state/coord.old-gap-20261102/event-sinks.json".to_owned());
        let health = assess(observed, options());
        assert_eq!(health.verdict, "degraded", "{}", health.text);
        assert_eq!(health.exit_code(), EXIT_DEGRADED);
        assert!(
            health
                .text
                .contains("event sinks are configured in the older state"),
            "{}",
            health.text
        );
        assert_eq!(
            health.document["event_sinks"]["stranded_config"],
            "/state/coord.old-gap-20261102/event-sinks.json"
        );
    }

    #[test]
    fn a_reply_source_recorded_for_the_previous_agent_is_not_shown() {
        // #852 unknown-tui-bridge: after a repoint, the record the service wrote while it served
        // the previous agent names that agent's transcript and misses; health shows neither.
        let health = assess(
            Observed {
                reply_source: Some(json!({
                    "agent": "coordb",
                    "source": {"path": "/t/old.jsonl", "evidence": "e", "discovered": false},
                    "capture_misses": [{"key": "a", "reply_id": "31", "block_at_millis": NOW}],
                })),
                ..healthy()
            },
            options(),
        );
        assert_eq!(health.verdict, "ok", "{}", health.text);
        assert!(!health.text.contains("/t/old.jsonl"), "{}", health.text);
        assert!(
            health.text.contains(
                "reply text: not looked for yet: the recorded source is the previous \
                           agent coordb's"
            ),
            "{}",
            health.text
        );
        // The same record written for this agent is shown.
        let health = assess(
            Observed {
                reply_source: Some(json!({
                    "agent": "coord",
                    "source": {"path": null, "evidence": "the target's harness (unknown) writes \
                               no transcript agentctl reads", "discovered": false},
                    "capture_misses": [],
                })),
                ..healthy()
            },
            options(),
        );
        assert!(
            health
                .text
                .contains("reply text: the screen only: the target's harness (unknown)"),
            "{}",
            health.text
        );
    }

    #[test]
    fn a_target_without_a_session_binding_or_with_uncaptured_replies_is_degraded() {
        let unbound = |source: Option<Value>| Observed {
            binding: Some(("claude".to_owned(), None)),
            reply_source: source.map(|source| json!({"source": source, "capture_misses": []})),
            ..healthy()
        };
        // No reply source recorded (an older service): it may read only the screen.
        let health = assess(unbound(None), options());
        assert_eq!(health.verdict, "degraded", "{}", health.text);
        assert!(
            health
                .text
                .contains("target coord has no native session binding"),
            "{}",
            health.text
        );
        assert!(health
            .text
            .contains("agentctl bind-session coord SESSION_ID"));
        // Nothing found: the screen only.
        let health = assess(
            unbound(Some(
                json!({"path": null, "evidence": "none", "discovered": false}),
            )),
            options(),
        );
        assert_eq!(health.verdict, "degraded", "{}", health.text);
        assert!(health
            .text
            .contains("reply text comes only from its screen"));
        assert!(health.text.contains("reply text: the screen only"));
        // Discovered through a live process: read, and about to be bound.
        let health = assess(
            unbound(Some(
                json!({"path": "/t/s-live.jsonl", "evidence": "process 7 names it",
                "discovered": true, "unbound_session": "s-live"}),
            )),
            options(),
        );
        assert_eq!(health.verdict, "degraded", "{}", health.text);
        assert!(
            health.text.contains("the bridge binds conversation s-live"),
            "{}",
            health.text
        );
        assert_eq!(health.document["binding"]["native_session"], Value::Null);
        // Found through the record's own process: said, but no problem.
        let health = assess(
            unbound(Some(
                json!({"path": "/t/s-1.jsonl", "evidence": "process 7 names it",
                "discovered": false}),
            )),
            options(),
        );
        assert_eq!(health.verdict, "ok", "{}", health.text);
        assert!(health.text.contains("has no native session binding"));
        // A headless target has no transcript reader: nothing to say.
        let mut headless = unbound(None);
        headless.target.0 =
            json!({"agent": "coord", "headless": true, "status": "idle", "alive": true});
        assert_eq!(assess(headless, options()).verdict, "ok");

        // A reply block the bridge never captured.
        let health = assess(
            Observed {
                reply_source: Some(json!({
                    "source": {"path": "/t/s-1.jsonl", "evidence": "e", "discovered": false},
                    "capture_misses": [{"key": "a", "reply_id": "31", "block_at_millis": NOW}],
                })),
                ..healthy()
            },
            options(),
        );
        assert_eq!(health.verdict, "degraded", "{}", health.text);
        assert!(
            health.text.contains("never captured (reply ID 31)"),
            "{}",
            health.text
        );
        assert!(health
            .text
            .contains("reply text: transcript /t/s-1.jsonl (e)"));
    }

    #[test]
    fn replies_held_after_a_restart_are_degraded_and_not_called_stuck() {
        let mut observed = healthy();
        observed.held = vec![chat_runtime::HeldReply {
            key: "a".repeat(64),
            ordinal: 1,
            captured_at_millis: NOW - 4 * 3_600_000,
            held_at_millis: NOW - 60_000,
            reason: chat_runtime::HoldReason::Stale,
        }];
        observed.unsent = vec![UnsentReply {
            key: "a".repeat(64),
            ordinal: 1,
            phase: "pending".to_owned(),
            captured_at_millis: Some(NOW - 4 * 3_600_000),
            send_attempts: Some(0),
            last_send_failure_code: None,
            last_send_failure_at_millis: None,
            send_permanent_failure: None,
        }];
        let health = assess(observed, options());
        assert_eq!(health.verdict, "degraded", "{}", health.text);
        assert!(
            health.text.contains("held back as stale"),
            "{}",
            health.text
        );
        assert!(
            !health.text.contains("unsent for more than"),
            "{}",
            health.text
        );
        assert!(health.text.contains(&format!("held: {}/1", "a".repeat(64))));
        assert_eq!(health.document["held_replies"][0]["reason"], "stale");
    }

    #[test]
    fn a_working_bridge_is_ok_in_one_line() {
        let health = assess(healthy(), options());
        assert_eq!(health.verdict, "ok", "{}", health.text);
        assert_eq!(health.exit_code(), 0);
        let first = health.text.lines().next().expect("verdict line");
        assert_eq!(
            first,
            "chat health: OK: typing into coord in pane w1:p2; last request admitted 2.0 min ago"
        );
        assert!(health.text.contains("channel C1: served by this bridge"));
        assert!(health
            .text
            .contains("target: coord in pane w1:p2 (claude, idle)"));
        assert_eq!(health.document["schema"], "agentctl-chat-health/v1");
    }

    #[test]
    fn a_running_service_whose_owner_loop_stopped_turning_is_stalled_not_degraded() {
        // #783: a loop hung on one delivery for 12 minutes read only as DEGRADED.
        let mut observed = healthy();
        observed.owner_loop = Some((12 * 60_000, Some(Duration::from_secs(300))));
        let health = assess(observed, options());
        assert_eq!(health.verdict, "stalled", "{}", health.text);
        assert_eq!(health.exit_code(), EXIT_STALLED);
        assert_eq!(health.exit_code(), EXIT_DOWN);
        let first = health.text.lines().next().expect("verdict line");
        assert!(
            first.starts_with("chat health: STALLED: the owner loop of service process 42 has not turned for 12.0 min"),
            "{first}"
        );
        assert_eq!(
            health.document["problems"]["stalled"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(health.document["owner_loop"]["last_turn_age_seconds"], 720);
        assert_eq!(health.document["owner_loop"]["stall_after_seconds"], 300);

        // Within its bound, without a bound, or with no service at all, it is not stalled.
        let mut observed = healthy();
        observed.owner_loop = Some((299_000, Some(Duration::from_secs(300))));
        assert_eq!(assess(observed, options()).verdict, "ok");
        let mut observed = healthy();
        observed.owner_loop = Some((12 * 60_000, None));
        assert_eq!(assess(observed, options()).verdict, "ok");
        let mut observed = healthy();
        observed.owner_loop = None;
        assert_eq!(assess(observed, options()).verdict, "ok");
        let mut observed = healthy();
        observed.runner = None;
        observed.owner_loop = Some((12 * 60_000, Some(Duration::from_secs(300))));
        assert_eq!(assess(observed, options()).verdict, "down");
    }

    #[test]
    fn the_target_is_looked_up_in_the_bridges_registry_and_a_mismatch_is_not_down() {
        // #791: run from another project, chat health looked the target up in that project's
        // registry and called a working bridge down.
        let root = std::env::temp_dir().join(format!("agentctl-791-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let given = root.join("caller/.agentctl");
        let bridge = root.join("team/.agentctl");
        std::fs::create_dir_all(&given).unwrap();
        std::fs::create_dir_all(&bridge).unwrap();

        let same = choose_registry(
            &given,
            Some(BridgeRegistry {
                path: given.clone(),
                source: "bridge index",
            }),
        );
        assert_eq!(same.probe, given);
        assert!(same.line.is_none() && same.mismatch.is_none());
        let unknown = choose_registry(&given, None);
        assert_eq!(unknown.probe, given);
        assert!(unknown.line.is_none());

        let other = choose_registry(
            &given,
            Some(BridgeRegistry {
                path: bridge.clone(),
                source: "service command line",
            }),
        );
        assert_eq!(other.probe, bridge);
        assert!(other.mismatch.is_none());
        let line = other.line.clone().unwrap();
        assert!(line.contains("from its service command line"), "{line}");
        let mut observed = healthy();
        observed.registry = other;
        let health = assess(observed, options());
        assert_eq!(health.verdict, "ok", "{}", health.text);
        assert!(
            health.text.contains("registry: the bridge runs with"),
            "{}",
            health.text
        );
        assert_eq!(
            health.document["registry"]["bridge_source"],
            "service command line"
        );

        // A bridge registry that cannot be read here: the target problem found in the given
        // registry is a registry mismatch, degraded, never down.
        let missing = choose_registry(
            &given,
            Some(BridgeRegistry {
                path: root.join("gone/.agentctl"),
                source: "bridge index",
            }),
        );
        assert_eq!(missing.probe, given);
        let mut observed = healthy();
        observed.registry = missing;
        observed.target = (
            json!({"agent": "coord", "alive": false}),
            Some("target coord is not a usable registered agent: unknown agent".to_owned()),
        );
        let health = assess(observed, options());
        assert_eq!(health.verdict, "degraded", "{}", health.text);
        assert!(health.text.contains("registry mismatch"), "{}", health.text);
        assert!(health.document["problems"]["down"]
            .as_array()
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_services_own_registry_argument_is_read_from_its_command_line() {
        let root = std::env::temp_dir().join(format!("agentctl-791-cmd-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for (arguments, expected) in [
            (
                vec!["--registry", "team/.agentctl"],
                root.join("team/.agentctl"),
            ),
            (
                vec!["--registry=/abs/.agentctl"],
                PathBuf::from("/abs/.agentctl"),
            ),
        ] {
            let mut child = std::process::Command::new("sh")
                .args(["-c", "sleep 30; :", "agentctl"])
                .args(&arguments)
                .args(["chat", "run"])
                .current_dir(&root)
                .spawn()
                .unwrap();
            let pid = u64::from(child.id());
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut found = None;
            while found.is_none() && std::time::Instant::now() < deadline {
                found = service_registry_argument(pid);
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
            let canonical = |path: &Path| {
                std::fs::canonicalize(path.parent().unwrap()).map_or(path.to_path_buf(), |parent| {
                    parent.join(path.file_name().unwrap())
                })
            };
            assert_eq!(
                found.as_deref().map(canonical),
                Some(canonical(&expected)),
                "{arguments:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_service_another_bridge_or_a_dead_target_is_down() {
        let mut observed = healthy();
        observed.runner = None;
        let health = assess(observed, options());
        assert_eq!(health.verdict, "down");
        assert_eq!(health.exit_code(), EXIT_DOWN);
        assert!(health
            .text
            .starts_with("chat health: DOWN: no chat run holds this state"));

        let mut observed = healthy();
        observed.channels[0]["holder_pid"] = json!(7);
        let health = assess(observed, options());
        assert!(health
            .text
            .contains("channel C1 is held by another bridge (process 7, state /s)"));

        let mut observed = healthy();
        observed.target = (
            json!({"agent": "coord", "alive": false}),
            Some("target coord's pane is not alive: gone".to_owned()),
        );
        let health = assess(observed, options());
        assert_eq!(health.verdict, "down");
        assert!(health.text.contains("NOT ALIVE"));
    }

    #[test]
    fn a_silent_stream_uncertain_requests_and_retry_loops_are_degraded() {
        let mut observed = healthy();
        observed.alive_age = Some(600_000);
        observed.times.push(request("b", "delivery_uncertain", 0));
        observed.status["delivery_alarm"]["stalled"] = json!([
            {"key": "b".repeat(64), "phase": "delivery_uncertain"},
            {"key": "c".repeat(64), "phase": "pending"},
        ]);
        observed.unsent.push(UnsentReply {
            key: "d".repeat(64),
            ordinal: 2,
            phase: "sending".to_owned(),
            captured_at_millis: Some(NOW - 1_000),
            send_attempts: Some(140),
            last_send_failure_code: Some("operation_conflict".to_owned()),
            last_send_failure_at_millis: Some(NOW - 500),
            send_permanent_failure: None,
        });
        observed.unsent.push(UnsentReply {
            key: "e".repeat(64),
            ordinal: 1,
            phase: "pending".to_owned(),
            captured_at_millis: Some(NOW - 3_600_000),
            send_attempts: None,
            last_send_failure_code: None,
            last_send_failure_at_millis: None,
            send_permanent_failure: None,
        });
        observed.status["repoint"]["pending"] =
            json!({"agent_name": "coord2", "requested_by": "ops"});
        let health = assess(observed, options());
        assert_eq!(health.verdict, "degraded", "{}", health.text);
        assert_eq!(health.exit_code(), EXIT_DEGRADED);
        let first = health.text.lines().next().expect("verdict line");
        for part in [
            "chat health: DEGRADED: ",
            "delivered nothing, not even a heartbeat, for 10.0 min",
            "1 request(s) not typed into coord",
            "1 request(s) with uncertain delivery",
            "reply 2 of request dddddddddddd retried 140 times (last failure: operation_conflict)",
            "1 captured repl(ies) unsent for more than 10.0 min",
            "a repoint to coord2 requested by ops is not applied yet",
        ] {
            assert!(first.contains(part), "{part:?} missing from {first}");
        }
        assert!(health
            .text
            .contains("  uncertain bbbbbbbbbbbb: typed, then lost"));
        assert!(health
            .text
            .contains("  retry loop: reply 2 of dddddddddddd: 140 attempts"));
    }

    #[test]
    fn an_older_service_without_a_liveness_record_is_not_called_stale() {
        let mut observed = healthy();
        observed.alive_age = None;
        let health = assess(observed, options());
        assert_eq!(health.verdict, "ok", "{}", health.text);
        assert!(health.text.contains("last item unknown"));
    }
}
