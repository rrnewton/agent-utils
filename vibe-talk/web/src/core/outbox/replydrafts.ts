// What the reader has written in Reply to each message and not sent (`#193 compose-draft-loss`, `#185
// reply-new-thread`; `#624 voice-page-modularity`, moved from the legacy script into core/outbox): by
// message id, saved on the device so a reload does not lose it, and, for this session only, which of
// them were left with "Start a new thread" ticked. Drafts are per target message; once Send is pressed
// the reply's text moves into the outgoing queue. The reply box, when a draft is written, the rows' Draft
// buttons and saying a refusal are the page's.

import type { StoragePort } from "../../ports/index.ts";
import { storedExactly as storedExactlyIn } from "./storage.ts";

/** Where the device keeps them, by message id. */
const DRAFTS_KEY = "vibe-talk.voice.drafts";

/** What the owner asks of the page. */
export interface ReplyDraftsDependencies {
  readonly storage: StoragePort;
  /** The device's quota is full: give up the saved channel rows first (`dropMessageCache`). */
  makeRoom(): void;
}

/** The owner's named commands. */
export interface ReplyDrafts {
  /** Whether message `id` has a draft. */
  has(id: string): boolean;
  /** Message `id`'s draft, if it has one. */
  get(id: string): string | undefined;
  /** Message `id`'s draft is `text` now, here; `save` writes it to the device. */
  set(id: string, text: string): void;
  /** Message `id` has no draft now, here. */
  delete(id: string): void;
  /**
   * Written back with a read-after-write check, as the microphone settings are; and, where the quota is
   * full, given the saved channel rows' space, as the channel's drafts are: saved messages can be
   * fetched again, and what the reader typed cannot. Answers whether the device kept them.
   */
  save(): boolean;
  /** Take back what the device kept; storage absent or a record that does not parse leaves none. */
  load(): void;
  /**
   * Whether message `id`'s kept draft was left with "Start a new thread" ticked, so that the box comes
   * back ticked with the words: a reader who put a reply down to look at something else and picked it
   * up again should not find it bound for the main channel because the box forgot. Only with the draft
   * — a reply sent or emptied takes its tick with it — and never for another message. For this session:
   * the words outlive a reload, and the tick beside them does not.
   */
  startsThread(id: string): boolean;
  /** Message `id`'s draft is, or is not, bound for a new thread. */
  setStartsThread(id: string, on: boolean): void;
}

/** An owner holding no drafts. */
export function createReplyDrafts(deps: ReplyDraftsDependencies): ReplyDrafts {
  /** message id -> the unsent text. Mirrored to storage so a reload does not lose it. */
  const drafts = new Map<string, string>();
  /** The messages whose kept draft was left with "Start a new thread" ticked, by id as a string. */
  const draftStartsThread = new Set<string>();
  const storedExactly = (encoded: string): boolean => storedExactlyIn(deps.storage, DRAFTS_KEY, encoded);
  return {
    has: (id) => drafts.has(id),
    get: (id) => drafts.get(id),
    set(id, text) {
      drafts.set(id, text);
    },
    delete(id) {
      drafts.delete(id);
    },
    save() {
      const encoded = JSON.stringify(Object.fromEntries(drafts));
      if (storedExactly(encoded)) return true;
      deps.makeRoom();
      return storedExactly(encoded);
    },
    load() {
      let stored: unknown = null;
      try {
        stored = JSON.parse(deps.storage.getItem(DRAFTS_KEY) || "null");
      } catch (_error) {
        stored = null;
      }
      if (!stored || typeof stored !== "object") {
        return;
      }
      for (const [id, text] of Object.entries(stored as object)) {
        if (typeof text === "string" && text) {
          drafts.set(id, text);
        }
      }
    },
    startsThread: (id) => draftStartsThread.has(id),
    setStartsThread(id, on) {
      if (on) draftStartsThread.add(id);
      else draftStartsThread.delete(id);
    },
  };
}

// What the page and the tests use of it.
export { DRAFTS_KEY };
