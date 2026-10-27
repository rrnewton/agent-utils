//! agentcloud: a remote session agentctl reaches through `agentcloudctl` and `agentterm`, not a
//! local screen. It takes prompts through its journal, so it has no composer, no turn to
//! interrupt, and no local transcript; `agentctl watch-agents` skips it. The driver that runs
//! inside the session (Claude Code, Codex, Muse) is chosen at creation from
//! [`crate::subagents::cloud`]'s driver list.

use super::{FollowupPolicy, HarnessAdapter, ScreenMarkers};

/// A remote agentcloud session.
pub struct AgentCloud;

static MARKERS: ScreenMarkers = ScreenMarkers {
    queue_hints: &[],
    approval: &[],
    reply_bullets: &[],
    user_turn_glyphs: &[],
    tool_output_glyphs: &[],
    running_glyphs: &[],
    composer_placeholders: &[],
};

impl HarnessAdapter for AgentCloud {
    fn name(&self) -> &'static str {
        "agentcloud"
    }

    fn display_name(&self) -> &'static str {
        "agentcloud"
    }

    fn markers(&self) -> &'static ScreenMarkers {
        &MARKERS
    }

    fn followup_policy(&self) -> FollowupPolicy {
        FollowupPolicy::Never
    }
}
