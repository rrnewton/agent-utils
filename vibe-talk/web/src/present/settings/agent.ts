// The voice agent's Settings: what the send switch, prefix and prompt editors say.
// Portable presentation decisions, moved from voice.js for #624 voice-page-modularity.

import type { SessionPrompt, VoicePostPrefix, VoicePromptSettings } from "../../contract/index.ts";

/** How the last call's prompt follows the server's send switch. */
export type AgentPromptFollowing = "bridge" | "replaced" | "kept" | "unknown";

/** What the last call was given, with whether an edited prompt was saved when it opened. */
export type AgentPromptLastCall = Readonly<SessionPrompt & { editedWhenOpened: boolean | null }>;

/** The characters in a box, counted as the server counts them: by code point, after trimming. */
export const promptChars = (text: unknown): number => [...String(text).trim()].length;

/** One honest line per state: what the agent does, and for off, what that exposes. */
export const VOICE_SEND_STATE = {
  on: "The agent reads the message back and asks 'Send it?'; say yes or tap Send.",
  off: "The agent sends as soon as you ask. A message it reads could also talk it into sending.",
} as const;

/**
 * Beside the switch, what it does not reach on a hosted agent. The server obeys the switch at once,
 * but the agent's send rules are in its prompt, and its approval for post_reply is in its
 * provider's console: the prompt follows only when this page may replace it, and the approval
 * never does. Without this, an owner who unticked the box would hear the agent go on reading back
 * messages the server had already posted.
 */
export const VOICE_SEND_HOSTED = {
  unknown:
    "The server changes at once. The agent's own instructions follow at its next call only if its provider lets this page replace its prompt; if not, paste the new prompt into the provider's console by hand. Change post_reply's approval there to match, either way.",
  kept:
    "The server changes at once, but the last call's agent does not let this page replace its prompt: paste the new prompt into its provider's console by hand, and change post_reply's approval there to match.",
  replaced:
    "The server changes at once, and the last call's agent was given this server's prompt, so its instructions follow from its next call. Change post_reply's approval in its provider's console to match.",
} as const;

/**
 * Whether the agent's own instructions follow the switch, as far as this page knows. `#272
 * voice-agent-instructions`. "bridge": a bridge fetches its prompt as each call opens. "replaced":
 * the last call's hosted agent was handed this server's prompt. "kept": the last call's hosted
 * agent does not let its prompt be replaced, so it keeps the one pasted into its console.
 * "unknown": a hosted agent, and no call yet that said which — or an older server that does not say
 * how the prompt is delivered.
 */
export function agentPromptFollowsSwitch(voicePromptSettings: Readonly<VoicePromptSettings> | null, voicePromptLastCall: AgentPromptLastCall | null): AgentPromptFollowing {
  if (voicePromptSettings !== null && voicePromptSettings.delivery === "profile") return "bridge";
  const last = voicePromptLastCall;
  if (last !== null && last.transport === "prompt_override") return "replaced";
  if (last !== null && last.override_allowed === false) return "kept";
  return "unknown";
}

/** The end of the sentence a saved switch reports: when, and whether, the agent's instructions follow. */
export function voiceSendSavedClause(follows: AgentPromptFollowing): string {
  switch (follows) {
    case "bridge":
      return "its instructions change from its next call.";
    case "replaced":
      return "its instructions follow from its next call; change post_reply's approval in its provider's console to match.";
    case "kept":
      return "this agent keeps the prompt pasted into its provider's console, so paste the new one there by hand.";
    default:
      return "its instructions follow at its next call only if its provider lets this page replace its prompt; otherwise paste the new one into its console by hand.";
  }
}

/**
 * A tag as the server keeps it (`post_prefix::clean`): one line, any control character a space, no
 * space in front, and nothing at all when it is nothing but space. Trailing space stays: it is what
 * keeps the tag off the first word.
 */
export function cleanPostPrefix(text: unknown): string {
  const oneLine = String(text).replace(/[\u0000-\u001f\u007f-\u009f]/gu, " ").replace(/^\s+/u, "");
  return oneLine.trim() === "" ? "" : oneLine;
}

/** The line under the box: what is in force, or why the box cannot be saved. */
export function voicePostPrefixLine(settings: Readonly<VoicePostPrefix>, dirty: boolean, length: number): string {
  if (length > settings.max_chars) {
    return `${length} characters; at most ${settings.max_chars} are kept. Shorten it to save.`;
  }
  if (dirty) return "Not saved yet.";
  if (!settings.custom) return "The default.";
  return settings.prefix ? "Saved. Every message the agent posts starts with it." : "Off: the agent's messages carry no prefix.";
}

/** Why the boxes cannot be changed here, or "" when they can. */
export function voicePromptLock(voicePromptSettings: Readonly<VoicePromptSettings>, writable: boolean): string {
  if (!writable) return "Only the write token can change these.";
  if (!voicePromptSettings.changeable) return "This server keeps no settings, so these cannot be changed here.";
  return "";
}

/** The line under the instructions box. */
export function voiceInstructionsLine(voicePromptSettings: Readonly<VoicePromptSettings>, text: string, dirty: boolean): string {
  const length = promptChars(text);
  const limit = voicePromptSettings.max_instructions_chars;
  if (length > limit) return `${length} characters; at most ${limit} are kept. Shorten them to save.`;
  if (dirty) return "Not saved yet.";
  return voicePromptSettings.instructions ? "Saved. Added to every call." : "None.";
}

/** The line under the system prompt, which says whose prompt it is and how old. */
export function voiceSystemPromptLine(voicePromptSettings: Readonly<VoicePromptSettings>, text: string, dirty: boolean): string {
  const length = promptChars(text);
  const limit = voicePromptSettings.max_system_prompt_chars;
  const version = voicePromptSettings.prompt_version;
  if (length > limit) return `${length} characters; at most ${limit} are kept. Shorten it to save.`;
  if (dirty) return "Not saved yet. Saving an empty box restores the shipped prompt.";
  if (!voicePromptSettings.system_prompt_edited) return `The shipped prompt, version ${version}.`;
  const from = voicePromptSettings.edited_from_version;
  if (typeof from === "number" && from < version) {
    return (
      `Your copy, saved when the shipped prompt was version ${from}. It is now version ${version}; ` +
      "restore it to see what changed."
    );
  }
  return "Your copy, in place of the shipped prompt.";
}

/** How the words reach this deployment's agent. */
export function voicePromptDeliveryLine(voicePromptSettings: Readonly<VoicePromptSettings>): string {
  if (voicePromptSettings.delivery === "profile") return "Both reach the agent at its next call.";
  // `#272`: the replaced prompt is always the write token's, because this server cannot see the
  // token the agent was given in its provider's console.
  return (
    "This page hands them to the agent as each call starts; the prompt only if its provider lets it be replaced. " +
    "A replaced prompt tells the agent it may send, so that agent needs the write token."
  );
}

/** What the last call was given, said plainly — above all an edited prompt that did not reach it. */
export function voicePromptLastCallLine(voicePromptSettings: Readonly<VoicePromptSettings>, voicePromptLastCall: AgentPromptLastCall | null): string {
  const last = voicePromptLastCall;
  if (last === null) return "";
  const refused =
    last.override_allowed === false
      ? "The last call's agent does not let its prompt be replaced"
      : "Whether the last call's agent lets its prompt be replaced could not be checked";
  if (last.transport === "prompt_override") {
    if (last.system_prompt_applied) {
      return last.instructions_applied
        ? "The last call was given your prompt and instructions."
        : "The last call was given your prompt.";
    }
    // The whole prompt was replaced all the same: with no copy of his, by the shipped one, and the
    // prompt pasted into the provider's console did not reach the agent.
    return last.instructions_applied
      ? "The last call was given the shipped prompt with your instructions, in place of the prompt in its provider's console."
      : "The last call was given the shipped prompt, in place of the prompt in its provider's console.";
  }
  if (last.transport === "contextual_update") {
    return `${refused}, so only your instructions went, as a note at the start.`;
  }
  // Nothing went: an edited prompt the agent would not take, or nothing of the owner's at all —
  // as saved when the call opened, not as saved since. Either way the agent used the prompt in its
  // provider's console.
  const edited = last.editedWhenOpened ?? voicePromptSettings.system_prompt_edited;
  return edited
    ? `${refused}, so your edited prompt did not reach it.`
    : `${refused}, so it kept the prompt in its provider's console.`;
}
