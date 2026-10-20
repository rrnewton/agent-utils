//! The chat bridge's built-in reactions, defined once for the bridge and every plugin.
//!
//! A chat bridge marks each request's chat message twice. The **acknowledgement**
//! goes on once the request is durably admitted; the **receipt** goes on once the request's
//! prompt is verified in the coordinator agent's pane.
//! A rejected human message can instead receive the rejection reaction, without creating a
//! request or a reply route.
//! A bridge configuration may override or disable each reaction; omitted settings inherit
//! the defaults here.
//!
//! The bridge sends reactions as Unicode emoji. A provider that names reactions differently maps
//! them itself; the Slack name of each default lives beside it so that the Slack plugin does not
//! need a second definition.

/// One built-in reaction: the Unicode emoji the bridge sends, and Slack's name for it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reaction {
    /// The Unicode emoji, as the bridge sends it to an outbound helper.
    pub emoji: &'static str,
    /// Slack's reaction name (shortcode, without colons) for the same emoji.
    pub slack_name: &'static str,
}

/// The default first-stage acknowledgement: the request was durably admitted.
pub const DEFAULT_ACK_REACTION: Reaction = Reaction {
    emoji: "👀",
    slack_name: "eyes",
};

/// The default second-stage receipt: the request's prompt is verified in the agent's pane.
pub const DEFAULT_RECEIPT_REACTION: Reaction = Reaction {
    emoji: "✅",
    slack_name: "white_check_mark",
};

/// The default rejection: an eligible human message did not become a request.
pub const DEFAULT_REJECT_REACTION: Reaction = Reaction {
    emoji: "❌",
    slack_name: "x",
};

/// Every built-in reaction, for a provider mapping that must cover them all.
pub const DEFAULT_REACTIONS: [Reaction; 3] = [
    DEFAULT_ACK_REACTION,
    DEFAULT_RECEIPT_REACTION,
    DEFAULT_REJECT_REACTION,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_eyes_then_a_check_mark_and_differ() {
        assert_eq!(DEFAULT_ACK_REACTION.emoji, "👀");
        assert_eq!(DEFAULT_ACK_REACTION.slack_name, "eyes");
        assert_eq!(DEFAULT_RECEIPT_REACTION.emoji, "✅");
        assert_eq!(DEFAULT_RECEIPT_REACTION.slack_name, "white_check_mark");
        // The bridge refuses a state whose two stages would look the same.
        assert_ne!(DEFAULT_ACK_REACTION.emoji, DEFAULT_RECEIPT_REACTION.emoji);
    }

    #[test]
    fn the_rejection_default_is_a_cross_and_differs_from_both_request_stages() {
        assert_eq!(DEFAULT_REJECT_REACTION.emoji, "❌");
        assert_eq!(DEFAULT_REJECT_REACTION.slack_name, "x");
        assert_ne!(DEFAULT_REJECT_REACTION.emoji, DEFAULT_ACK_REACTION.emoji);
        assert_ne!(
            DEFAULT_REJECT_REACTION.emoji,
            DEFAULT_RECEIPT_REACTION.emoji
        );
    }
}
