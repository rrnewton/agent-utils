// What the channel box's Clear unread says and decides (`#232 upstream-mark-read`, `#253
// upstream-clear-polish`, `#349 channel-settings-all`; `#624 voice-page-modularity`, moved from the
// legacy script into present/settings). The box itself is web/src/view/settings's; the status lines for
// one channel are web/src/present/notices's (`upstreamClearSentence`, `upstreamClearNowSentence`).

import type { UpstreamClearAttemptView, UpstreamClearMode, UpstreamClearNowOutcome, UpstreamClearNowResponse, UpstreamClearState } from "../../contract/index.ts";

/**
 * How often, after a "Mark read now" the server stopped waiting on, the box asks again what the
 * push came to. `#253 upstream-clear-polish`.
 */
const UPSTREAM_CLEAR_RECHECK_MS = 4000;
/**
 * How long the box keeps asking before it stops and says the push is still unanswered: asked every
 * UPSTREAM_CLEAR_RECHECK_MS, so seven times. `#253 upstream-clear-polish`.
 */
const UPSTREAM_CLEAR_RECHECK_LIMIT_MS = 30000;

/** The modes, as the status line names them. */
const UPSTREAM_CLEAR_MODE_WORDS: { readonly [mode: string]: string | undefined } =
  Object.freeze({ on: "On", dry_run: "Dry run", off: "Off" });

/**
 * A press the server answered `waiting`, as the box follows it: `mode` is the mode it was pressed in,
 * `messageAtMs` the instant of the message it asked to push, and `before` the attempt the server had
 * last recorded when it answered, so a newer one is recognised.
 */
interface UpstreamClearPress {
  readonly mode: UpstreamClearMode;
  readonly messageAtMs: number | null;
  readonly before: string;
}

/** What the server said about each channel All channels reaches, as the box holds it. */
interface UpstreamClearAllHeld {
  readonly ids: readonly string[];
  readonly views: ReadonlyMap<string, UpstreamClearState>;
  readonly failures: ReadonlyMap<string, string>;
}

/** One recorded attempt, as a key that changes when a newer attempt replaces it. */
function upstreamClearAttemptKey(attempt: UpstreamClearAttemptView | null | undefined): string {
  return attempt ? `${attempt.at_ms} ${attempt.message_id} ${attempt.result}` : "";
}

/**
 * What a press the server answered `waiting` has come to, read from the channel's state as the
 * server holds it now: the answer the server would have given had it waited this long, or null
 * while nothing has answered the press yet. The same rules as the server's own wait: an attempt
 * newer than the one recorded at the answer that reaches the pressed message answers it, whatever
 * it came to; a newer failure short of it means the message waits for the retry; a refusal or
 * another mode ends it. `#253 upstream-clear-polish`.
 */
function upstreamClearPressSettled(press: UpstreamClearPress, view: UpstreamClearState): UpstreamClearNowResponse | null {
  if (view.mode !== press.mode) return { outcome: "changed", state: view };
  const last = view.last || null;
  const fresh = Boolean(last) && upstreamClearAttemptKey(last) !== press.before;
  if (last && fresh && (press.messageAtMs === null || last.message_at_ms >= press.messageAtMs)) {
    const outcomes: { readonly [result: string]: UpstreamClearNowOutcome | undefined } = { failed: "failed", skipped: "skipped", unavailable: "unavailable" };
    return { outcome: outcomes[last.result] || "pushed", attempt: last, state: view };
  }
  if (view.unavailable) return { outcome: "unavailable", state: view };
  if (last && fresh && last.result === "failed") return { outcome: "retry_waiting", state: view };
  return null;
}

/** The status line while a press waits on the server: what it is doing, in the chat service. */
const upstreamClearPushingWords = (mode: UpstreamClearMode, service: string): string => mode === "dry_run"
  ? `Trying it on the newest message in ${service}…`
  : `Marking the newest message read in ${service}…`;

/** The status line once the box has stopped asking about a press nothing has answered. */
const upstreamClearUnansweredWords = (service: string): string => `${service} has not answered yet. The server carries on ` +
  "without this page; open Settings again later to see what it came to.";

/** Why the box has no answer for its channel. */
const upstreamClearReadFailure = (why: string): string => `Could not read this setting: ${why}`;

/**
 * The mode every channel All channels reaches has: the mode, null when they differ (Mixed), or
 * undefined while one of them has not been read.
 */
function upstreamClearAllCommon(held: UpstreamClearAllHeld | null): UpstreamClearMode | null | undefined {
  if (!held || held.ids.some((id) => !held.views.has(id))) return undefined;
  const modes = [...new Set(held.ids.map((id) => (held.views.get(id) as UpstreamClearState).mode))];
  return modes.length === 1 ? modes[0] : null;
}

/**
 * The status line for All channels: what they are set to, or why that is not known yet. `channelCount`
 * is how many channels the picker offers, the ones it leaves out among them; `nameOf` names a channel
 * by id.
 */
function upstreamClearAllSentence(held: UpstreamClearAllHeld | null, channelCount: number, nameOf: (id: string) => string): string {
  if (!held) return "";
  const others = channelCount - held.ids.length;
  const tail = others === 0 ? ""
    : others === 1 ? " 1 other channel's chat service cannot clear unread."
      : ` ${others} other channels' chat services cannot clear unread.`;
  if (held.failures.size > 0) {
    const [id, why] = [...held.failures][0] as [string, string];
    const more = held.failures.size > 1 ? ` (and ${held.failures.size - 1} more)` : "";
    return `Could not read this setting for ${nameOf(id)}${more}: ${why}`;
  }
  const common = upstreamClearAllCommon(held);
  if (common === undefined) return "Checking with the server…";
  if (common !== null) {
    const where = held.ids.length === 1 ? nameOf(held.ids[0] as string) : `all ${held.ids.length} channels`;
    return `${UPSTREAM_CLEAR_MODE_WORDS[common]} in ${where}.${tail}`;
  }
  const counts = (["on", "dry_run", "off"] as UpstreamClearMode[])
    .map((mode): [UpstreamClearMode, number] => [mode, held.ids.filter((id) => (held.views.get(id) as UpstreamClearState).mode === mode).length])
    .filter(([, count]) => Number(count) > 0);
  return `Mixed: ${counts.map(([mode, count]) => `${UPSTREAM_CLEAR_MODE_WORDS[mode]} in ${count}`).join(", ")}.${tail}`;
}

/** What a Save on All channels came to: every channel, and what they are set to now; or which refused. */
const upstreamClearAllSavedWords = (refused: readonly string[], total: number, sentence: () => string): string => refused.length === 0
  ? `Saved. ${sentence()}`
  : `Saved in ${total - refused.length} of ${total} channels; not in ${refused.join(", ")}.`;

export {
  UPSTREAM_CLEAR_MODE_WORDS,
  UPSTREAM_CLEAR_RECHECK_LIMIT_MS,
  UPSTREAM_CLEAR_RECHECK_MS,
  upstreamClearAllCommon,
  upstreamClearAllSavedWords,
  upstreamClearAllSentence,
  upstreamClearAttemptKey,
  upstreamClearPressSettled,
  upstreamClearPushingWords,
  upstreamClearReadFailure,
  upstreamClearUnansweredWords,
};
export type { UpstreamClearAllHeld, UpstreamClearPress };
