//! A TUI agentctl has no adapter for, driven by harness-neutral rules (`--harness unknown`,
//! [`crate::unknown_harness`]). It knows no phrase, glyph, launch argument or transcript; the one
//! fact it supplies is that whatever program leads the pane's foreground process group is the
//! harness, so adoption can pin it. It is not in [`super::all`]: it adds nothing to the
//! cross-harness tables.

use super::{FollowupPolicy, HarnessAdapter, ScreenMarkers};

/// A TUI driven by the generic rules.
pub struct Unknown;

static MARKERS: ScreenMarkers = ScreenMarkers {
    queue_hints: &[],
    approval: &[],
    reply_bullets: &[],
    user_turn_glyphs: &[],
    tool_output_glyphs: &[],
    running_glyphs: &[],
    composer_placeholders: &[],
};

impl HarnessAdapter for Unknown {
    fn name(&self) -> &'static str {
        crate::unknown_harness::HARNESS
    }

    fn display_name(&self) -> &'static str {
        "unknown harness"
    }

    fn markers(&self) -> &'static ScreenMarkers {
        &MARKERS
    }

    fn native_group_leader(&self, _executable: &std::path::Path) -> bool {
        true
    }

    fn followup_policy(&self) -> FollowupPolicy {
        FollowupPolicy::Never
    }
}
