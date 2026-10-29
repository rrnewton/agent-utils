// When a voice call carries on after the voice service loses a turn, and when it ends instead (`#352
// voice-reconnect-on-bridge-error`; `#624 voice-page-modularity`, moved from the legacy script; plan
// step 13). Why a call reconnects at all, and what it hands the new session, is told where the legacy
// script reconnects (`reconnectCall`).

/** The bridge's words for "this session is lost; open another": an `error` ending in "; reconnect". */
const RECONNECT_ASKED = /;\s*reconnect\s*\.?\s*$/iu;

/** "turn 4 never started": the turn the bridge says never began. */
const LOST_TURN = /\bturn\s+(\d+)\s+never\s+started\b/iu;

/**
 * How soon after one automatic reconnect another request ends the call instead: a bridge that keeps
 * losing turns is not hidden behind reconnects.
 */
const RECONNECT_MIN_GAP_MS = 60000;

/** The most automatic reconnects in one call: the request after them ends it. */
const RECONNECTS_PER_CALL_MAX = 3;

/**
 * How long a reconnect waits for this call's turns to reach the server before it asks for the record
 * it hands the new session, so that the record holds what was just said.
 */
const RECONNECT_RECORDS_WAIT_MS = 3000;

/** What of a call these decisions read. */
export interface ReconnectingCall {
  /** The session's protocol: only `vibe-talk-v1` can carry a call on to a new session. */
  readonly protocol: string | null | undefined;
  /** Whether that session started: before `session_started` there is no session to carry on from. */
  readonly v1Ready: boolean | undefined;
  /** How many times this call has reconnected automatically. */
  readonly reconnects: number;
  /** When it last did, by the wall clock; 0 or null when it never has. */
  readonly lastReconnectAt: number | null | undefined;
}

/**
 * Does the voice service's `error` (`said`) ask the client to reconnect, on a `vibe-talk-v1` session
 * that started?
 */
export function reconnectAsked(call: ReconnectingCall, said: string): boolean {
  return call.protocol === "vibe-talk-v1" && Boolean(call.v1Ready) && RECONNECT_ASKED.test(said);
}

/**
 * Whether a call asked to reconnect at `now` carries on (`reconnect`), or ends at the bound: past
 * RECONNECTS_PER_CALL_MAX reconnects (`count`), or within RECONNECT_MIN_GAP_MS of the last (`soon`).
 */
export function reconnectVerdict(call: ReconnectingCall, now: number): "count" | "soon" | "reconnect" {
  if (call.reconnects >= RECONNECTS_PER_CALL_MAX) return "count";
  if (call.lastReconnectAt && now - call.lastReconnectAt < RECONNECT_MIN_GAP_MS) return "soon";
  return "reconnect";
}

export { LOST_TURN, RECONNECT_ASKED, RECONNECT_MIN_GAP_MS, RECONNECT_RECORDS_WAIT_MS, RECONNECTS_PER_CALL_MAX };
