//! Tell about a provider gap the bridge kept running through (#2440 chat-bridge-never-halt, D5).
//!
//! Admission records each provider gap in `gap-reconcile.json` (see
//! [`crate::chat_runtime::gap_reconcile`]) and keeps the bridge running from the gap's far side.
//! This step, run with the reply-window checks every 30 seconds, puts ONE line in the bridge
//! agent's pane and posts ONE owner-alarm notice per gap. The pane line's message ID is the
//! gap's, so a restarted service never queues it twice; each is recorded as told in the record,
//! and a line that cannot be queued is tried again at the next check.

use super::owner_alarm::{self, Notice};
use crate::chat_runtime::BridgeState;

/// Tell the agent (`notify`, with a message ID) and the owner (`post_owner`, when the bridge has
/// an owner alarm) about the open gap reconcile, each once.
pub(crate) fn gap_reconcile_step(
    state: &BridgeState,
    now_millis: u64,
    notify: &mut dyn FnMut(&str, &str) -> Result<(), String>,
    post_owner: Option<&mut dyn FnMut(Notice)>,
    log: &mut dyn FnMut(String),
) {
    let Some(reconcile) = state.gap_reconcile() else {
        return;
    };
    if !reconcile.shown_at(now_millis) || (reconcile.agent_told && reconcile.owner_told) {
        return;
    }
    let mut agent_told = false;
    let mut owner_told = false;
    if !reconcile.agent_told {
        match notify(&reconcile.agent_notice(), &reconcile.notice_id()) {
            Ok(()) => {
                log(format!(
                    "agentctl: error: chat provider gap ({}); the bridge kept running from the \
                     gap's far side and reconciles the window {} from the provider's history; \
                     the agent was given the alarm",
                    reconcile.reasons.join("; "),
                    reconcile.window_text()
                ));
                agent_told = true;
            }
            Err(error) => log(format!(
                "agentctl: error: chat provider gap; the alarm could not be queued to the agent, \
                 and is tried again: {error}"
            )),
        }
    }
    if !reconcile.owner_told {
        if let Some(post) = post_owner {
            post(owner_alarm::gap_reconcile_notice(
                &reconcile,
                &state.config().agent_label,
            ));
            owner_told = true;
        }
    }
    if agent_told || owner_told {
        if let Err(error) =
            state.mark_gap_reconcile_told(reconcile.observed_at_millis, agent_told, owner_told)
        {
            log(format!(
                "agentctl: warning: the gap reconcile could not be recorded as told, so it may \
                 be told again: {error}"
            ));
        }
    }
}
