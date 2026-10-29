//! `chat repoint`: point a bridge at another agent without restarting it (#677
//! chat-bridge-repoint). The durable request and its application live in
//! [`crate::chat_runtime`]; this module validates the target against the registry and Herdr,
//! wakes or stands in for the running service, and posts the channel notice.

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::chat_runtime::{
    self, BridgeState, CommandOutboundTransport, RepointOutcome, RootMessageSubmission,
    RootMessageTransport,
};
use crate::subagents::{ManagedAgents, ManagedApi};

use super::{service_log, ChatServiceError};

/// The datagram that tells a running service a repoint request is waiting.
pub(super) const REPOINT_WAKE: &str = "repoint";
/// How often `chat repoint` looks for the running service's outcome.
const OUTCOME_POLL: Duration = Duration::from_millis(100);
/// Exit status of `chat repoint` when the request is recorded but not applied yet.
pub const EXIT_PENDING: i32 = 75;
/// Exit status of `chat repoint` when the target was refused.
pub const EXIT_REFUSED: i32 = 1;

/// Check that `agent` can be the bridge's target, and give the pane it is in: registered, and
/// either a pane agent whose pane is alive with a harness Herdr recognizes (in `pane` when one is
/// named), or a headless agent whose wrapper answers. The error says why not.
pub fn validate_target<A: ManagedApi + ?Sized>(
    manager: &ManagedAgents<'_, A>,
    agent: &str,
    pane: Option<&str>,
) -> Result<Option<String>, String> {
    let headless = manager
        .is_headless(agent)
        .map_err(|error| format!("{agent} is not a usable registered agent: {error}"))?;
    if headless {
        if pane.is_some() {
            return Err(format!(
                "{agent} is headless: it has no pane, so --pane cannot apply"
            ));
        }
        let status = manager.status(agent).map_err(|error| {
            format!("the status of headless agent {agent} is unreadable: {error}")
        })?;
        let agent_status = status["agent_status"].as_str().unwrap_or("unknown");
        return match agent_status {
            "working" | "idle" | "starting" => Ok(None),
            other => Err(format!(
                "headless agent {agent} is {other}{}",
                status["probe_error"]
                    .as_str()
                    .map(|error| format!(": {error}"))
                    .unwrap_or_default()
            )),
        };
    }
    let info = manager
        .pane_info(agent)
        .map_err(|error| format!("{agent}'s pane is not alive: {error}"))?;
    if let Some(expected) = pane {
        if expected != info.pane_id {
            return Err(format!(
                "{agent} is registered in pane {}, not {expected}; re-register it with \
                 `agentctl adopt` first, or leave out --pane",
                info.pane_id
            ));
        }
    }
    match info.agent.as_deref() {
        Some(_) => Ok(Some(info.pane_id)),
        None => Err(format!(
            "Herdr detects no agent harness in {agent}'s pane {}; start the agent there first",
            info.pane_id
        )),
    }
}

/// Apply the pending repoint request of `state`, if any, through `delivery`, which reads and
/// edits the previous agent's queue, and post its notice through `transport`. Gives the outcome
/// and, when the target changed, the state serving the new agent.
pub(super) fn apply_pending<A: ManagedApi + ?Sized>(
    state: &BridgeState,
    manager: &ManagedAgents<'_, A>,
    delivery: &dyn chat_runtime::CoordinatorDelivery,
    transport: Option<&mut CommandOutboundTransport>,
    applied_by: &str,
) -> Result<Option<(RepointOutcome, Option<BridgeState>)>, ChatServiceError> {
    let Some(request) = state.pending_repoint()? else {
        return Ok(None);
    };
    let validate = |agent: &str, pane: Option<&str>| validate_target(manager, agent, pane);
    let outcome = state.apply_repoint(&request, delivery, &validate, applied_by)?;
    let summary = outcome.summary();
    service_log(format_args!("agentctl: {summary}"));
    for entry in &outcome.uncertain {
        service_log(format_args!(
            "agentctl: repoint: request {} is uncertain: {}",
            entry.key, entry.detail
        ));
    }
    if let Some(transport) = transport {
        post_notice(state, transport, &summary);
    }
    if !outcome.changed_target() {
        return Ok(Some((outcome, None)));
    }
    let headless = manager.is_headless(&outcome.agent_name)?;
    let next = state
        .retargeted(&outcome.agent_name)
        .with_command_only_replies(headless)
        .with_receipt_when_started(headless);
    Ok(Some((outcome, Some(next))))
}

/// Post `summary` as a new message in the bridge's first channel. A failure is logged only: the
/// outcome is recorded either way, and `chat repoint` prints it.
fn post_notice(state: &BridgeState, transport: &mut dyn RootMessageTransport, summary: &str) {
    let Some(channel) = state.config().channel_ids.first() else {
        return;
    };
    let request_id = match chat_runtime::random_operation_uuid() {
        Ok(id) => id,
        Err(error) => {
            service_log(format_args!("agentctl: repoint notice not posted: {error}"));
            return;
        }
    };
    let submission = RootMessageSubmission {
        channel_id: channel,
        body: summary,
        request_id: &request_id,
    };
    if let Err(error) = chat_runtime::validate_root_message_submission(&submission) {
        service_log(format_args!("agentctl: repoint notice refused: {error}"));
        return;
    }
    match transport.publish_root(submission) {
        Ok(message) => service_log(format_args!(
            "agentctl: repoint notice posted to {channel} as {message}"
        )),
        Err(error) => service_log(format_args!("agentctl: repoint notice not posted: {error}")),
    }
}

/// Run `chat repoint`: validate `agent`, record the request, and have it applied: by the running
/// service, which is woken and waited for up to `wait`, or by this command when no service holds
/// the state. Gives the command's JSON result and exit status: 0 once applied, [`EXIT_REFUSED`]
/// when the target was refused, [`EXIT_PENDING`] when the service has not applied it within
/// `wait`.
pub fn repoint<A: ManagedApi + ?Sized>(
    state_root: &Path,
    manager: &ManagedAgents<'_, A>,
    agent: &str,
    pane: Option<&str>,
    requested_by: &str,
    wait: Duration,
) -> Result<(Value, i32), ChatServiceError> {
    let state = BridgeState::inspect(state_root)?;
    if let Err(reason) = validate_target(manager, agent, pane) {
        return Ok((
            json!({"applied": false, "refused": reason, "agent_name": agent}),
            EXIT_REFUSED,
        ));
    }
    let request = state.request_repoint(agent, pane, requested_by)?;
    let (outcome, service, woken) = match state.acquire_runner_lease() {
        Ok(_lease) => {
            // No service runs on this state: apply the request here, as the service would. The
            // lease is held until the outcome is recorded.
            let mut transport = state.outbound_transport()?;
            let applied = apply_pending(&state, manager, manager, transport.as_mut(), "command")?;
            (applied.map(|(outcome, _)| outcome), false, false)
        }
        Err(_) => {
            let woken = super::wake_service_with(state_root, REPOINT_WAKE);
            let deadline = Instant::now() + wait;
            let outcome = loop {
                if let Some(outcome) = state.repoint_outcome(request.generation)? {
                    break Some(outcome);
                }
                if Instant::now() >= deadline {
                    break None;
                }
                std::thread::sleep(OUTCOME_POLL);
            };
            (outcome, true, woken)
        }
    };
    let mut result = json!({
        "request": request,
        "service_running": service,
        "service_woken": woken,
    });
    let code =
        match &outcome {
            Some(outcome) if outcome.generation != request.generation => {
                result["superseded_by"] = json!(outcome);
                result["applied"] = json!(false);
                EXIT_REFUSED
            }
            Some(outcome) => {
                result["applied"] = json!(outcome.refused.is_none());
                result["outcome"] = json!(outcome);
                result["summary"] = json!(outcome.summary());
                if outcome.refused.is_some() {
                    EXIT_REFUSED
                } else {
                    0
                }
            }
            None => {
                result["applied"] = json!(false);
                result["pending"] =
                    json!(format!(
                "the running service has not applied the request within {}s; it applies it \
                 after its current pass{}, and at its next reconciliation or start; check with \
                 `agentctl chat health --bridge-state {}`",
                wait.as_secs(),
                if woken { "" } else { " once woken (no wake socket answered)" },
                state_root.display()
            ));
                EXIT_PENDING
            }
        };
    Ok((result, code))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};

    use super::super::{wake_service, wake_service_with, ReplyWakeListener, REPLY_WAKE_SOCKET};
    use super::*;

    #[test]
    fn a_repoint_wake_sets_the_flag_and_is_not_taken_for_a_reply() {
        let root = std::env::temp_dir().join(format!("agentctl-rp-wake-{}", std::process::id()));
        crate::agent::create_private_directory(&root, "test state", false, true).expect("root");
        let (sender, wakes) = mpsc::sync_channel(4);
        let repoint = Arc::new(AtomicBool::new(false));
        let listener = ReplyWakeListener::bind(
            root.join(REPLY_WAKE_SOCKET),
            sender,
            Arc::default(),
            Arc::new(AtomicBool::new(false)),
            Arc::clone(&repoint),
        )
        .expect("bind the wake socket");
        assert!(wake_service_with(&root, REPOINT_WAKE));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !repoint.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            repoint.load(Ordering::SeqCst),
            "the repoint wake set the flag"
        );
        // A reply wake still reaches the reply queue, and the repoint wake did not.
        let key = "a".repeat(64);
        assert!(wake_service(&root, &key));
        assert_eq!(wakes.recv_timeout(Duration::from_secs(5)), Ok(key));
        assert!(wakes.try_recv().is_err());
        listener
            .stop(Instant::now() + Duration::from_secs(5))
            .expect("stop the listener");
        let _ = std::fs::remove_dir_all(&root);
    }
}
