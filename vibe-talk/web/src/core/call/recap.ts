// What a voice call's seams and the line under it say about how the call before ended and what the
// new one was handed (`#54 resume-recovery`, `#268 voice-call-drops`, `#270 voice-transcript-markers`,
// `#331 resume-on-websocket`, `#352 voice-reconnect-on-bridge-error`, `#512 voice-recap-private-stack`;
// `#624 voice-page-modularity`, moved from the legacy script; plan step 13). Words only: where they are
// drawn, and what the page knows of resuming, stay with the page.

/** A recap a call was handed, or why it was handed none, as the page and the stored marker hold it. */
export interface Recap {
  readonly state: string;
  readonly why?: string | null;
  readonly words?: unknown;
  readonly lines?: unknown;
  readonly dropped?: number;
  readonly transport?: string | null;
  readonly reconnected?: boolean;
}

/** Everything a "new conversation" seam says. */
export interface Boundary {
  readonly word: string;
  readonly chip: string | null;
  readonly detail: string;
  readonly recapDetail: string | null;
}

/** The entry of `table` under `key`, read as the legacy script read it (`table[key]`). */
function wordsFor(table: object, key: unknown): string | undefined {
  return (table as { readonly [key: string]: string | undefined })[String(key)];
}

// What the seam says INSIDE its disclosure, per cause. The LABEL is the same three words in every
// case, deliberately: the boundary is the same boundary — the agent below it has never seen
// anything above it — however the socket happened to close. Only the explanation differs, because
// only the cause differs, and each of these is a couple of clauses because the suite measures
// them. `#54 resume-recovery`.
const SEAM_DETAILS = {
  ended:
    "Anything below this line goes to an agent that has never seen anything above it. " +
    "Mute, not Hang up, keeps its memory.",
  suspended:
    "The call dropped while this page was in the background. Resuming starts a NEW " +
    "conversation: the agent below this line remembers nothing above it.",
  failed:
    "The connection dropped. Anything below this line goes to an agent that has never seen " +
    "anything above it.",
  // `#268 voice-call-drops`. A call the reader did not end says who did.
  closed:
    "The voice service ended this call; you did not hang up. Anything below this line goes to an " +
    "agent that has never seen anything above it.",
  error:
    "The voice service reported an error and the call ended. Anything below this line goes to an " +
    "agent that has never seen anything above it.",
  // `#270 voice-transcript-markers`. Not an ending: the hosted service began a new session on the
  // same call, and that session was handed nothing.
  replaced:
    "The voice service started a new session inside this call. Anything below this line goes to an " +
    "agent that has never seen anything above it.",
};

// ...and what it says when resuming is armed, because every sentence above asserts that the agent
// below the line remembers nothing above it — which stops being true the moment `#46
// conversation-replay` is switched on. A seam that kept the old wording would be the feature
// lying in the one place the reader looks to find out what just happened.
const RESUME_SEAM_DETAILS = {
  ended:
    "This is where the conversation broke. The next call is NEW, but this server will read the " +
    "lines above back to it: a reconstruction, not the same conversation.",
  suspended:
    "The call dropped while this page was in the background. Resuming opens a NEW conversation " +
    "and reads the lines above back to it, so it can carry on.",
  failed:
    "The connection dropped. The next call is a NEW one, and this server will read the lines " +
    "above back to it so it can carry on.",
  closed:
    "The voice service ended this call, not you. Next call is NEW; this server will read the " +
    "lines above back to it: a reconstruction, not the same conversation.",
  error:
    "The voice service reported an error, ending the call. This server will read the lines above " +
    "back to the next one: a reconstruction, not the same conversation.",
  // A replacement session is never handed the lines above, resuming or not.
  replaced:
    "The voice service started a new session inside this call. Anything below this line goes to an " +
    "agent that has never seen anything above it.",
};

/** The recap sentence, per state. `{n}` is the words sent, `{d}` the older lines that did not fit. */
// `#649 voice-agent-tools`. The owner asked whether the recap was only the call just before or
// cumulative, and decided (2026-10-28 11:07 ET): cumulative across recent calls within a size budget,
// over budget the most recent part, never summarised. The server builds it so (`recent_turns` in
// src/replay.rs: the calls of the last 12 hours, newest kept), and these sentences say so.
const RECAP_DETAILS = {
  sent:
    "This call was sent {n} words as a recap: the newest of what was said in your calls of the " +
    "last 12 hours, word for word.",
  dropped:
    "This call was sent the last {n} words of your calls of the last 12 hours; {d} older lines did " +
    "not fit, so it cannot see those.",
  client_data:
    "This call was sent {n} words above as a recap with its start request; the agent's own " +
    "settings may ignore a recap sent that way.",
  unsupported:
    "Nothing was recapped: this server's voice service has no way to take a recap, so the agent " +
    "below remembers nothing above.",
  // `#331 resume-on-websocket`. The bridge took the frame and answered that it would not use it.
  refused:
    "Nothing was recapped: the voice service refused the recap it was sent, so the agent below " +
    "remembers nothing above.",
  // `#512 voice-recap-private-stack`. Armed in the instructions the voice service fetches, and not
  // fetched with them before the page stopped asking.
  not_taken:
    "Nothing was recapped: the voice service did not take the recap armed for its instructions, so " +
    "the agent below remembers nothing above.",
  empty:
    "No recap was sent: the conversation above held nothing said aloud to replay, so the agent " +
    "below starts from nothing.",
  too_long:
    "No recap was sent: not even the last line above fit the recap budget, so the agent below " +
    "starts from nothing.",
  failed:
    "No recap was sent: this page could not read the conversation above from the server, so the " +
    "agent below starts from nothing.",
  replaced_session:
    "The voice service replaced this call's session, and the new session was sent no recap, so it " +
    "remembers nothing above.",
};

/** Why a call started fresh, per recap `why`, as the end of "Started fresh: …". */
const CALL_FRESH = {
  empty: "nothing was said aloud earlier",
  too_long: "the earlier conversation was too long to send",
  failed: "the earlier conversation could not be read",
  refused: "the voice service refused the recap",
  not_taken: "the voice service did not take the recap",
  replaced_session: "the voice service replaced the session",
};

/** Why a reconnected call's new session was handed nothing, as the end of its line under the call. */
const RECONNECT_FRESH = {
  ...CALL_FRESH,
  empty: "nothing said in this call was recorded to send",
  failed: "this call's record could not be read",
};

// One wording for every boundary in the stored record, because it is the same boundary each time:
// above the rule is a call that has ended and that nothing below it remembers. Held to the same
// word budget as the other seams the page draws.
const STORED_SEAM_DETAIL =
  "These lines are from a conversation that has already ended, restored from this server. " +
  "The agent has no memory of them: a new call starts from nothing.";

/** The seam's sentence for how a call ended (`cause`), by whether resuming is armed (`resuming`). */
export function seamDetailFor(cause: string, resuming: boolean): string {
  const table = resuming ? RESUME_SEAM_DETAILS : SEAM_DETAILS;
  return wordsFor(table, cause) || table.ended;
}

/**
 * How many words a sent recap held, or null when nobody can say: an older server's replay names no
 * count, and a recap of lines with no words in them is one whose count was not known when it was filed.
 */
export function recapWords(recap: Recap): number | null {
  const words = recap.words;
  return typeof words === "number" && !(words === 0 && Number(recap.lines) > 0) ? words : null;
}

/** The chip a seam shows for `recap`, or null for none. */
export function recapChip(recap: Recap | null | undefined): string | null {
  if (!recap || (recap.state === "none" && recap.why === "off")) return null;
  if (recap.state === "sent") {
    const words = recapWords(recap);
    return words === null ? "recap sent" : `${words.toLocaleString("en-US")}-word recap sent`;
  }
  if (recap.why === "unsupported") return "recap unsupported";
  if (recap.why === "not_taken") return "recap not taken";
  return recap.why === "refused" ? "recap refused" : "no recap sent";
}

/** The recap sentence for `recap`, or null for none. */
export function recapSentence(recap: Recap | null | undefined): string | null {
  if (!recap || (recap.state === "none" && recap.why === "off")) return null;
  if (recap.state === "sent") {
    const which = recap.transport === "client_data" ? "client_data" : (recap.dropped as number) > 0 ? "dropped" : "sent";
    // A count nobody can state is said as lines, not as a number of words.
    const words = recapWords(recap);
    const count = words !== null ? `${words.toLocaleString("en-US")} words` : which === "dropped" ? "lines" : "the lines";
    const older = `${recap.dropped} older line${recap.dropped === 1 ? "" : "s"}`;
    return RECAP_DETAILS[which].replace("{n} words", count).replace("{d} older lines", older);
  }
  return wordsFor(RECAP_DETAILS, recap.why) || null;
}

/**
 * Everything a "new conversation" seam says, from the recap the new call was handed and how the call
 * before it ended — the one source for the live seam and the restored one, so a reload says the same.
 * A seam never promises a read-back that did not happen, nor denies one that did: the cause's
 * sentence comes from the resuming wording only when a recap was sent. With no cause (the first call
 * after a load), the stored record's own sentence, or, when a recap went, the resuming one.
 */
export function boundaryFor(recap: Recap | null | undefined, cause: string | null | undefined): Boundary {
  const sent = Boolean(recap && recap.state === "sent");
  const table = sent ? RESUME_SEAM_DETAILS : SEAM_DETAILS;
  // A replacement session ended no call, so its marker names no cause; its recap says it was a
  // replacement, and the restored line says what the live one did (`noteConversationNamed`).
  const replaced = Boolean(recap && recap.state === "none" && recap.why === "replaced_session");
  const why = cause || (replaced ? "replaced" : null);
  const detail = why ? wordsFor(table, why) || table.ended : sent ? RESUME_SEAM_DETAILS.ended : STORED_SEAM_DETAIL;
  return { word: "new conversation", chip: recapChip(recap), detail, recapDetail: recapSentence(recap) };
}

/**
 * The line under a call for `recap`, given whether the voice service acknowledged it (`ack`). Words
 * from the server's own count of what its replay held; "the earlier conversation" where an older
 * server's replay named none. `freshBecause` is the page's reason for a fresh start no table names.
 */
export function callRecapLine(recap: Recap, ack: string | null, freshBecause: (why: unknown) => string): string {
  if (recap.reconnected) return reconnectedRecapLine(recap, ack, freshBecause);
  if (recap.state === "sent") {
    const words = recapWords(recap);
    const amount = words === null
      ? "the earlier conversation"
      : `${words.toLocaleString("en-US")} word${words === 1 ? "" : "s"} from earlier`;
    const older = (recap.dropped as number) > 0
      ? `; ${recap.dropped} older line${recap.dropped === 1 ? "" : "s"} did not fit`
      : "";
    return `${recapVerb(recap, ack, "Caught the agent up with", "Handing the agent", "Sent the agent")} ${amount}${older}`;
  }
  return `Started fresh: ${wordsFor(CALL_FRESH, recap.why) || freshBecause(recap.why)}`;
}

/**
 * `#352 voice-reconnect-on-bridge-error`. The line under a call the page carried on a new session:
 * that it did, and what the new session was handed of this call, or why nothing.
 */
function reconnectedRecapLine(recap: Recap, ack: string | null, freshBecause: (why: unknown) => string): string {
  if (recap.state === "sent") {
    const words = recapWords(recap);
    const amount = words === null
      ? "this call so far"
      : `${words.toLocaleString("en-US")} word${words === 1 ? "" : "s"} from this call`;
    const older = (recap.dropped as number) > 0
      ? `; ${recap.dropped} older line${recap.dropped === 1 ? "" : "s"} did not fit`
      : "";
    return `Reconnected after a lost turn: ${recapVerb(recap, ack, "caught the agent up with", "handing the agent", "sent the agent")} ` +
      `${amount}${older}`;
  }
  return `Reconnected after a lost turn: started fresh, as ${wordsFor(RECONNECT_FRESH, recap.why) || freshBecause(recap.why)}`;
}

/**
 * What the line under a call says was done with a recap that went: taken, once the voice service
 * said so (`accepted`); being handed, while a recap armed in the profile waits to be asked about
 * (`#512 voice-recap-private-stack`); otherwise sent.
 */
function recapVerb(recap: Recap, ack: string | null, taken: string, handing: string, sent: string): string {
  if (ack === "accepted") return taken;
  return recap.transport === "profile" && ack === "waiting" ? handing : sent;
}

/** Why a reconnected call's new session was handed nothing, as a table entry. */
export function reconnectFreshWords(why: unknown): string | undefined {
  return wordsFor(RECONNECT_FRESH, why);
}

export { STORED_SEAM_DETAIL };
