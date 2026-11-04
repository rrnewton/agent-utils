// Resuming an earlier conversation (`#46 conversation-replay`; `#624 voice-page-modularity`, moved from the
// legacy script into core/call): whether the server permits it, the conversation a new call would resume from,
// what the last attempt actually did, the reader's own switch, the clause under the large control, and the
// fetch of a new call's payload.
//
// THIS IS A RECONSTRUCTION AND THE SCREEN MUST NEVER SAY OTHERWISE. The vendor documents no way to resume a
// conversation once its socket closes; the server holds the transcript, so a new call can be handed the earlier
// exchange as text. A failed replay never aborts the call: it degrades to a fresh one and says so. Drawing what
// Settings says, the switch itself, and what a call does with the payload are the page's.

import type { StoragePort } from "../../ports/index.ts";

/**
 * What the last attempt actually did — not what it was asked to do.
 *
 * `armed` means a payload really went out on the socket. `dropped` is how many older turns the server's budget
 * left behind, which is the difference between "replayed" and "replayed in part". `failed` means the fetch did
 * not answer. And `attempted` without `armed` is the fourth case that is easy to miss: the fetch WORKED and came
 * back with nothing to replay — an earlier conversation in which nothing was actually said. That must not be
 * reported as a resumption, and it must not be reported as a failure either.
 *
 * `dropped` is kept in that fourth case too, and it is what splits it in two. Nothing went out either way, but
 * `dropped > 0` means the budget threw the WHOLE transcript away — a conversation too long to replay, not an
 * empty one. Saying "there was nothing to replay" about it asserts something false about what was said earlier,
 * which is the exact class of lie this feature is written against; it just happens to be a lie about the past
 * rather than about the present.
 *
 * `#331 resume-on-websocket` adds what a `vibe-talk-v1` bridge did with it, each of which takes `armed` back:
 * `unsupported`, the call's session did not list `replay` and was sent nothing; `refused`, it took the frame and
 * answered that it would not use it; `unsent`, the call ended before its session started. `acked` is the one
 * that keeps it: the bridge said it took the record. And `words`, the server's count of what the record held,
 * null where it named none. `replaced` keeps `armed` too, since the record did go: the voice service then
 * replaced the session that took it, and the one that finished the call was sent none (`noteSessionReplaced`).
 */
export interface ResumeLast {
  attempted: boolean;
  armed: boolean;
  dropped: number;
  failed: boolean;
  words: number | null;
  acked: boolean;
  refused: boolean;
  unsupported: boolean;
  unsent: boolean;
  replaced: boolean;
}

/** What `/api/v1/conversations/{id}/replay` answers, as far as the page reads it. */
export interface ReplayPayload {
  enabled?: boolean;
  text?: string;
  transport?: string;
  included?: number;
  dropped?: number;
  words?: number | null;
  profileRecap?: number;
}

/** The clause under the large control: one of these, and nothing else. */
export type ResumeNote =
  | "the agent starts fresh"
  | "the agent starts fresh — the earlier conversation could not be read"
  | "the agent starts fresh — the voice service took no recap"
  | "the earlier conversation is replayed in part"
  | "the earlier conversation is replayed"
  | "the agent starts fresh — the earlier conversation was too long to replay"
  | "the agent starts fresh — there was nothing to replay";

/** What resuming asks of the page. */
export interface CallResumeDependencies {
  /** Where the reader's switch is kept, under `key`. */
  readonly key: string;
  /** This device's storage, looked up at every use. */
  storage(): StoragePort;
  /** The server's answer at `path`; throws where there is none. */
  api(path: string): Promise<unknown>;
  /** A line in the connection details. */
  addDetail(text: string): void;
  /** What the last attempt did changed: say so wherever it is shown. */
  render(): void;
}

/** Resuming's state and commands. */
export interface CallResume {
  /** Whether the SERVER permits it at all, from `/api/v1/client-config`. */
  allowed(): boolean;
  setAllowed(allowed: boolean): void;
  /** The conversation a new call would resume from, or null. */
  conversationId(): string | null;
  setConversationId(id: string | null): void;
  /** What the last attempt actually did ({@link ResumeLast}). */
  last(): ResumeLast;
  setLast(last: ResumeLast): void;
  /** Whether the reader has asked for it: off unless asked for. */
  wanted(): boolean;
  /** Keep the reader's switch, where this device keeps anything. */
  persist(on: boolean): void;
  /** Will the NEXT call carry a replay? Everything the screen says is derived from this. */
  armed(): boolean;
  /** The clause under the large control, and the one place this feature could lie. */
  note(): ResumeNote;
  /**
   * Fetch the payload for a new call. NEVER throws, and never leaves the state stale. Answers the payload, or
   * null when there is nothing to send — which includes every failure. The caller does not branch on why; the
   * SCREEN does, off {@link CallResume.last}.
   */
  fetchPayload(): Promise<ReplayPayload | null>;
}

/** What the last attempt did, before it has done anything. */
export function blankResumeLast(): ResumeLast {
  return {
    attempted: false, armed: false, dropped: 0, failed: false, words: null,
    acked: false, refused: false, unsupported: false, unsent: false, replaced: false,
  };
}

/** Not permitted, no conversation to resume from, and no attempt made. */
export function createCallResume(deps: CallResumeDependencies): CallResume {
  let resumeAllowed = false;
  let resumeConversationId: string | null = null;
  let resumeLast = blankResumeLast();

  const resumeWanted = (): boolean => deps.storage().getItem(deps.key) === "on";

  function persistResume(on: boolean): void {
    try {
      deps.storage().setItem(deps.key, on ? "on" : "off");
    } catch (_error) {
      // Same as the relay toggle: honoured for this session, not remembered. Better than failing.
    }
  }

  const resumeArmed = (): boolean =>
    Boolean(resumeAllowed && resumeWanted() && resumeConversationId && !resumeLast.failed);

  /**
   * Five answers, and every one that begins "the agent starts fresh" carries a different reason: both the off
   * state and the failed one mean the agent starts fresh, and only one of them is what the reader asked for. The
   * two empty ones are split the same way — an earlier conversation that held nothing and one the budget could
   * not fit are the same silence with opposite meanings.
   */
  function resumeNote(): ResumeNote {
    if (!resumeAllowed || !resumeWanted() || !resumeConversationId) {
      return "the agent starts fresh";
    }
    if (resumeLast.failed) {
      return "the agent starts fresh — the earlier conversation could not be read";
    }
    // `#331 resume-on-websocket`. The voice service would not take the last one.
    if (resumeLast.refused || resumeLast.unsupported) {
      return "the agent starts fresh — the voice service took no recap";
    }
    // A call that ended before its session started was sent nothing, and the next one will be.
    if (resumeLast.unsent) {
      return resumeLast.dropped > 0
        ? "the earlier conversation is replayed in part"
        : "the earlier conversation is replayed";
    }
    if (resumeLast.attempted && !resumeLast.armed) {
      return resumeLast.dropped > 0
        ? "the agent starts fresh — the earlier conversation was too long to replay"
        : "the agent starts fresh — there was nothing to replay";
    }
    if (resumeLast.armed && resumeLast.dropped > 0) {
      return "the earlier conversation is replayed in part";
    }
    return "the earlier conversation is replayed";
  }

  async function fetchResume(): Promise<ReplayPayload | null> {
    resumeLast = blankResumeLast();
    if (!resumeAllowed || !resumeWanted() || !resumeConversationId) {
      deps.render();
      return null;
    }
    resumeLast.attempted = true;
    let payload: ReplayPayload | null = null;
    try {
      payload = (await deps.api(
        `/api/v1/conversations/${encodeURIComponent(resumeConversationId)}/replay`
      )) as ReplayPayload | null;
    } catch (error) {
      resumeLast = { ...blankResumeLast(), attempted: true, failed: true };
      // The connection details, not the error panel: a lost reconstruction must not look like a broken call,
      // and the call is about to open perfectly well.
      deps.addDetail(`the earlier conversation could not be replayed: ${(error as Error).message}`);
      deps.render();
      return null;
    }
    // `included > 0` is the gate, not `text`: an empty transcript answers 200 with an empty payload on purpose,
    // and sending a "you are resuming" preamble with no record behind it is the false continuity claim this
    // whole feature is written against.
    if (!payload || payload.enabled !== true || !((payload.included as number) > 0) || !payload.text) {
      // Nothing goes out — but `dropped` still has to be carried, because it is the only thing that separates
      // "the earlier conversation was empty" from "the budget dropped all of it". The screen says opposite
      // things about those two and only one of them can be true.
      resumeLast = {
        ...blankResumeLast(),
        attempted: true,
        dropped: Number((payload && payload.dropped) || 0),
      };
      deps.render();
      return null;
    }
    resumeLast = {
      ...blankResumeLast(),
      attempted: true,
      armed: true,
      dropped: Number(payload.dropped) || 0,
      words: typeof payload.words === "number" ? payload.words : null,
    };
    deps.render();
    return payload;
  }

  return {
    allowed: () => resumeAllowed,
    setAllowed(allowed) {
      resumeAllowed = allowed;
    },
    conversationId: () => resumeConversationId,
    setConversationId(id) {
      resumeConversationId = id;
    },
    last: () => resumeLast,
    setLast(last) {
      resumeLast = last;
    },
    wanted: resumeWanted,
    persist: persistResume,
    armed: resumeArmed,
    note: resumeNote,
    fetchPayload: fetchResume,
  };
}
