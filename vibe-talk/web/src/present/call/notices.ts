// What a call says to its agent and to the reader (`#624 voice-page-modularity`, moved from the legacy
// script into present/call): the fixed sentence the agent is told about a draft's outcome, and the
// status line for a call that ended.

import type { PendingPost } from "../../contract/index.ts";

/**
 * The fixed sentence the agent is told about `outcome`, starting "Voice page:" — what its
 * instructions tell it to wait for — or null when it needs none. It is never told SENT for anything
 * this page did not see the server accept. `how` is "voice" for a spoken yes and "tap" for the card.
 */
function outcomeNotice(outcome: { readonly kind: string; readonly proposal?: PendingPost | null; readonly reason?: string }, how: "voice" | "tap"): string | null {
  const channel = (outcome.proposal && outcome.proposal.channel_name) || "the channel";
  switch (outcome.kind) {
    case "sent":
      return how === "tap"
        ? `Voice page: the user tapped Send, and the message was SENT to ${channel}. ` +
            "Tell the user in a few words that it was sent. Then stop."
        : `Voice page: the message you read back was SENT to ${channel}. ` +
            "Tell the user in a few words that it was sent. Then stop.";
    case "partial":
      return `Voice page: only part of the message reached ${channel}; the rest was NOT sent. ` +
        "Tell the user that in one short sentence, and that the screen has the rest. Then stop.";
    case "unknown":
    case "unconfirmed":
      return "Voice page: I could not confirm whether the message was sent. Tell the user to " +
        "check the channel before sending it again. Then stop.";
    case "refused":
      return `Voice page: the message was NOT sent (${outcome.reason || "the server refused it"}). ` +
        "Tell the user in one short sentence that it was not sent. Then stop.";
    case "withdrawn":
      return "Voice page: the user tapped Don't send, so the draft was withdrawn and NOT sent. " +
        "Say so in a few words. Then stop.";
    case "lapsed":
      return `Voice page: the draft to ${channel} lapsed: nobody tapped Send or said yes in time, ` +
        "so it was NOT sent and the card is gone. Tell the user in one short sentence, and offer to " +
        "draft it again. Then stop.";
    case "declined":
      // The agent heard the no itself, and will offer new words or drop it; telling it again would
      // only make it speak twice.
      return null;
    case "gone":
      return "Voice page: no draft is waiting to be sent; the last one lapsed or was already " +
        "settled, so that answer sent nothing. If the user meant to send something, tell them in " +
        "one short sentence, and offer to draft it again. Otherwise carry on.";
    default:
      return `Voice page: the message was NOT sent: ${outcome.reason || "it could not be confirmed by voice"}. ` +
        "If the user meant to send it, tell them in one short sentence that it was not sent, and " +
        "ask whether to read it back again. Otherwise carry on.";
  }
}

/**
 * A close code, in plain words.
 *
 * The owner's screen read "conversation closed (code 1005)". 1005 means the connection gave no
 * reason at all, so the page was reporting the ABSENCE of information as though it were
 * information, in a vocabulary only a WebSocket implementer has. Say what happened, or say
 * nothing; the number is still recorded in the connection details for whoever is debugging.
 */
function closeReason(code: number): string {
  if (code === 1000) {
    return "Call ended.";
  }
  if (code === 1001 || code === 1005 || code === 1006) {
    return "The call ended — the connection dropped.";
  }
  return "The call ended unexpectedly. Settings has the details.";
}

export { closeReason, outcomeNotice };
