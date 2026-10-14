//! The chat bridge's two built-in reactions, defined once for the bridge and every plugin.
//!
//! A chat bridge marks each request's chat message twice. The **acknowledgement**
//! goes on once the request is durably admitted; the **receipt** goes on once the request's
//! prompt is verified in the coordinator agent's pane. A bridge configuration may override
//! either emoji or disable either stage; one that says nothing inherits the defaults here.
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

/// Every built-in reaction, for a provider mapping that must cover them all.
pub const DEFAULT_REACTIONS: [Reaction; 2] = [DEFAULT_ACK_REACTION, DEFAULT_RECEIPT_REACTION];

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
}
