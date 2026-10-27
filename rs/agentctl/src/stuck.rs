//! Typed harness failures. Only transient service failures may be retried; never approve a
//! command or countermand a refusal. Detection reads the current response, not quoted history.
use serde::{Deserialize, Serialize};

/// A reason a harness cannot continue.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StuckCause {
    /// Provider has no capacity for the selected model.
    ModelCapacity,
    /// Provider rejected a request temporarily (HTTP 429).
    ApiRateLimit,
    /// A command needs operator attention.
    UnsafeCommand,
    /// A safety policy refused the request.
    SafetyRefusal,
    /// Account credits or subscription quota are exhausted.
    OutOfQuota,
    /// Native goal stopped without a more specific visible reason.
    GoalStalled,
}

impl StuckCause {
    /// Report spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ModelCapacity => "model-capacity",
            Self::ApiRateLimit => "api-rate-limit",
            Self::UnsafeCommand => "unsafe-command",
            Self::SafetyRefusal => "safety-refusal",
            Self::OutOfQuota => "out-of-quota",
            Self::GoalStalled => "goal-stalled",
        }
    }

    /// Only availability failures are safe to retry without an operator decision.
    pub const fn recoverable(self) -> bool {
        matches!(self, Self::ModelCapacity | Self::ApiRateLimit)
    }
}

/// Read current, anchored error rows; ordinary prose mentioning these phrases is not a fault.
/// A new user turn or tool/assistant block clears prior errors. An empty composer does not.
pub fn classify(screen: &str) -> Option<StuckCause> {
    let mut error_text = String::new();
    let mut active_error = false;
    let mut goal_stalled = false;
    let mut tool_output = false;
    for raw in screen.lines() {
        let line = raw.trim();
        let user_turn = crate::harness::user_turn_glyphs().iter().any(|prefix| {
            line.strip_prefix(*prefix).is_some_and(|rest| {
                let rest = rest.trim();
                !rest.is_empty() && !rest.chars().next().is_some_and(|c| c.is_ascii_digit())
            })
        });
        if user_turn
            || (line.starts_with(crate::harness::reply_bullets())
                || line.starts_with(crate::harness::tool_output_glyphs()))
        {
            error_text.clear();
            active_error = false;
            goal_stalled = false;
            tool_output = line.starts_with(crate::harness::tool_output_glyphs());
        }
        // Error-looking rows copied from command output are not harness errors.
        if tool_output && raw.starts_with(char::is_whitespace) {
            continue;
        }
        let error = line
            .strip_prefix('■')
            .map(str::trim)
            .or_else(|| line.strip_prefix("API Error:").map(str::trim))
            .or_else(|| line.strip_prefix("Error:").map(str::trim));
        if let Some(error) = error {
            error_text.clear();
            error_text.push_str(error);
            active_error = true;
            tool_output = false;
        } else if active_error {
            error_text.push(' ');
            error_text.push_str(line);
        }
        goal_stalled |= crate::harness::all()
            .iter()
            .any(|adapter| adapter.stalled_goal_resume(line).is_some());
    }
    classify_error(&error_text).or(goal_stalled.then_some(StuckCause::GoalStalled))
}

fn classify_error(error: &str) -> Option<StuckCause> {
    let text = error.to_ascii_lowercase();
    // Quota, permissions and refusals win over availability hints in a compound error.
    if [
        "insufficient_quota",
        "out of quota",
        "usage limit",
        "credit balance is too low",
        "quota exceeded",
        "you've hit your limit",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        Some(StuckCause::OutOfQuota)
    } else if text.contains("unsafe bash command") {
        Some(StuckCause::UnsafeCommand)
    } else if [
        "cyber safety",
        "cyber filter",
        "safety filter",
        "content_policy_violation",
        "blocked by safety",
        "safety policy",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        Some(StuckCause::SafetyRefusal)
    } else if text.contains("selected model is at capacity") {
        Some(StuckCause::ModelCapacity)
    } else if [
        "rate_limit_error",
        "rate limit exceeded",
        "rate limit reached",
        "too many requests",
        "429",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        Some(StuckCause::ApiRateLimit)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const CASES: &[(&str, StuckCause)] = &[
        (
            include_str!("../testdata/stuck/codex-capacity.txt"),
            StuckCause::ModelCapacity,
        ),
        (
            include_str!("../testdata/stuck/codex-rate-limit.txt"),
            StuckCause::ApiRateLimit,
        ),
        (
            include_str!("../testdata/stuck/claude-rate-limit.txt"),
            StuckCause::ApiRateLimit,
        ),
        (
            include_str!("../testdata/stuck/claude-unsafe.txt"),
            StuckCause::UnsafeCommand,
        ),
        (
            include_str!("../testdata/stuck/codex-safety.txt"),
            StuckCause::SafetyRefusal,
        ),
        (
            include_str!("../testdata/stuck/claude-safety.txt"),
            StuckCause::SafetyRefusal,
        ),
        (
            include_str!("../testdata/stuck/codex-quota.txt"),
            StuckCause::OutOfQuota,
        ),
        (
            include_str!("../testdata/stuck/claude-quota.txt"),
            StuckCause::OutOfQuota,
        ),
        (
            include_str!("../testdata/stuck/goal-stalled.txt"),
            StuckCause::GoalStalled,
        ),
    ];

    #[test]
    fn screen_fixtures_and_history_mutations() {
        for (screen, cause) in CASES {
            assert_eq!(classify(screen), Some(*cause), "{screen}");
            assert_eq!(
                classify(&format!("{screen}\n› Continue\n• Working on it.\n")),
                None
            );
            let prose = screen
                .lines()
                .map(|line| format!("    documentation: {line}\n"))
                .collect::<String>();
            assert_eq!(classify(&prose), None);
            assert_eq!(
                classify(&format!("{screen}\n• Ran cat log\n  └ no fault\n")),
                None
            );
        }
    }

    #[test]
    fn wrapped_quota_wins_over_http_429_and_tool_logs_do_not_count() {
        assert_eq!(
            classify("API Error: 429 {\n  \"type\": \"insufficient_quota\"\n}\n❯"),
            Some(StuckCause::OutOfQuota)
        );
        assert_eq!(
            classify(
                "● Bash(cat log)\n  ⎿ previous response:\n    API Error: 429 rate_limit_error\n❯"
            ),
            None
        );
    }

    #[test]
    fn refusal_and_permission_errors_override_availability_hints() {
        assert_eq!(
            classify("API Error: 429 content_policy_violation"),
            Some(StuckCause::SafetyRefusal)
        );
        assert_eq!(
            classify("■ Selected model is at capacity; blocked by cyber filter"),
            Some(StuckCause::SafetyRefusal)
        );
        assert_eq!(
            classify("Error: 429 unsafe bash command"),
            Some(StuckCause::UnsafeCommand)
        );
    }

    #[test]
    fn quota_and_refusals_never_become_retryable() {
        for cause in [
            StuckCause::OutOfQuota,
            StuckCause::UnsafeCommand,
            StuckCause::SafetyRefusal,
            StuckCause::GoalStalled,
        ] {
            assert!(!cause.recoverable());
        }
        assert_eq!(
            classify("API Error: 429 insufficient_quota rate limit exceeded"),
            Some(StuckCause::OutOfQuota)
        );
        assert_eq!(
            classify("Approaching rate limits\n› 1. Switch models\n  2. Keep current model"),
            None
        );
    }
}
