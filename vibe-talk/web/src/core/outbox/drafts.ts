// What the reader has typed in each channel's and thread's message box, not yet sent (`#193
// compose-draft-loss`; `#624 voice-page-modularity`, moved from the legacy script; plan step 12):
// by box key, saved on the device at each keystroke, so a reload, a crash or another view puts the
// draft back. The boxes themselves, and when a draft is written, stay with the page.

import type { StoragePort } from "../../ports/index.ts";
import { storedExactly as storedExactlyIn } from "./storage.ts";

/** Where the device keeps them, by box key. */
const CHANNEL_DRAFTS_KEY = "vibe-talk.channel-drafts";

/**
 * `#819 voice-page-quoted-reply-and-draft-chip`. Where the device keeps the quick replies closed unsent, by
 * channel, message and quoted words: the same kind of store, under its own key.
 */
const QUICK_DRAFTS_KEY = "vibe-talk.quick-reply-drafts";

/** What the owner asks of the page. */
export interface ChannelDraftsDependencies {
  readonly storage: StoragePort;
  /** The device's quota is full: give up the saved channel rows first (`dropMessageCache`). */
  makeRoom(): void;
  /** Where the device keeps them; the message boxes' key unless said. */
  readonly key?: string;
}

/** The owner's named commands. */
export interface ChannelDrafts {
  /** The draft of box `key`, if it has one. */
  get(key: string): string | undefined;
  /** Box `key`'s draft is `text` now, here; `save` writes it to the device. */
  set(key: string, text: string): void;
  /** Box `key` has no draft now, here. */
  delete(key: string): void;
  /**
   * Write every draft to the device, read back to be sure, giving up the saved channel rows once
   * where the quota is full: saved messages can be fetched again, and what the reader typed cannot.
   * Answers whether the device kept them.
   */
  save(): boolean;
  /** Take back what the device kept; storage absent or an interrupted write leaves none. */
  load(): void;
  /** `#819`. The keys of the boxes with a draft. */
  keys(): string[];
}

/** An owner holding no drafts. */
export function createChannelDrafts(deps: ChannelDraftsDependencies): ChannelDrafts {
  const channelDrafts = new Map<string, string>();
  const storageKey = deps.key || CHANNEL_DRAFTS_KEY;
  const storedExactly = (encoded: string): boolean => storedExactlyIn(deps.storage, storageKey, encoded);
  return {
    keys: () => [...channelDrafts.keys()],
    get: (key) => channelDrafts.get(key),
    set(key, text) {
      channelDrafts.set(key, text);
    },
    delete(key) {
      channelDrafts.delete(key);
    },
    save() {
      try {
        const encoded = JSON.stringify(Object.fromEntries(channelDrafts));
        if (storedExactly(encoded)) return true;
        deps.makeRoom();
        return storedExactly(encoded);
      } catch (_error) {
        return false;
      }
    },
    load() {
      try {
        const saved: unknown = JSON.parse(deps.storage.getItem(storageKey) || "{}");
        for (const [key, value] of Object.entries((saved || {}) as object)) {
          if (typeof value === "string") channelDrafts.set(key, value);
        }
      } catch (_error) {
        // Storage may be absent or contain an interrupted write; composing still works in this page.
      }
    },
  };
}

// What the page and the tests use of it.
export { QUICK_DRAFTS_KEY };
