//! `chat health`: whether a bridge is working, in one line, and why (#677 chat-bridge-repoint).
//! It reads the bridge's records, `/proc/locks` and the registry, and asks Herdr about the target
//! agent's pane; it writes nothing and never contacts the chat provider.

use std::fmt::Write as _;
use std::path::Path;
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

/// `chat health` settings.
#[derive(Clone, Copy, Debug)]
pub struct HealthOptions {
    /// Age of the last provider commit past which the event stream counts as stale.
    pub stale_after: Duration,
}

/// One `chat health` report.
#[derive(Debug)]
pub struct Health {
    /// `ok`, `degraded` or `down`.
    pub verdict: &'static str,
    /// The whole report, as `--json` prints it.
    pub document: Value,
    /// The report as text: the verdict line, then details.
    pub text: String,
}

impl Health {
    /// The command's exit status: 0 when ok, [`EXIT_DEGRADED`] or [`EXIT_DOWN`].
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self.verdict {
            "ok" => 0,
            "degraded" => EXIT_DEGRADED,
            _ => EXIT_DOWN,
        }
    }
}

#[derive(Default)]
struct Problems {
    down: Vec<String>,
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
    /// [`probe_target`].
    target: (Value, Option<String>),
    times: Vec<RequestTimes>,
    unsent: Vec<UnsentReply>,
    now: u64,
}

/// Build the `chat health` report for the bridge state at `state_root`.
pub fn health<A: ManagedApi + ?Sized>(
    state_root: &Path,
    manager: &ManagedAgents<'_, A>,
    options: HealthOptions,
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
        target: probe_target(manager, &state.config().agent_name),
        times: state.request_times()?,
        unsent: state.unsent_replies()?,
        configuration: state.config().clone(),
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
        target: (target, target_problem),
        times,
        unsent,
        now,
    } = observed;
    let configuration = &configuration;
    let mut problems = Problems::default();
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
        problems.down.push(problem);
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
        .filter(|request| request.phase == "delivery_uncertain")
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
    let stuck_millis = u64::try_from(UNSENT_REPLY_STUCK.as_millis()).unwrap_or(u64::MAX);
    let stuck_replies = unsent
        .iter()
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

    let verdict = if !problems.down.is_empty() {
        "down"
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
                .chain(&problems.degraded)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        ),
    };

    let mut text = headline;
    text.push('\n');
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
        "problems": {"down": problems.down, "degraded": problems.degraded},
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
        "retry_loop_attempts": RETRY_LOOP_ATTEMPTS,
        "reply_breaker": status["reply_breaker"],
        "service_health": status["service_health"],
        "repoint": status["repoint"],
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
            target: (
                json!({"agent": "coord", "headless": false, "pane_id": "w1:p2", "harness": "claude", "status": "idle", "alive": true}),
                None,
            ),
            times: vec![request("a", "delivered", 1)],
            unsent: Vec::new(),
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
        }
    }

    fn options() -> HealthOptions {
        HealthOptions {
            stale_after: Duration::from_secs(300),
        }
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
