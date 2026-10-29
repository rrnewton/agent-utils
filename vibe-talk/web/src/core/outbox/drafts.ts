// What the reader has typed in each channel's and thread's message box, not yet sent (`#193
// compose-draft-loss`; `#624 voice-page-modularity`, moved from the legacy script; plan step 12):
// by box key, saved on the device at each keystroke, so a reload, a crash or another view puts the
// draft back. The boxes themselves, and when a draft is written, stay with the page.

import type { StoragePort } from "../../ports/index.ts";

/** Where the device keeps them, by box key. */
const CHANNEL_DRAFTS_KEY = "vibe-talk.channel-drafts";

/** What the owner asks of the page. */
export interface ChannelDraftsDependencies {
  readonly storage: StoragePort;
  /** The device's quota is full: give up the saved channel rows first (`dropMessageCache`). */
  makeRoom(): void;
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
}

/** An owner holding no drafts. */
export function createChannelDrafts(deps: ChannelDraftsDependencies): ChannelDrafts {
  const channelDrafts = new Map<string, string>();
  /** `encoded` kept under the key, read back to be sure: a refusing browser may drop a write silently. */
  const storedExactly = (encoded: string): boolean => {
    try {
      deps.storage.setItem(CHANNEL_DRAFTS_KEY, encoded);
      return deps.storage.getItem(CHANNEL_DRAFTS_KEY) === encoded;
    } catch (_error) {
      return false;
    }
  };
  return {
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
        const saved: unknown = JSON.parse(deps.storage.getItem(CHANNEL_DRAFTS_KEY) || "{}");
        for (const [key, value] of Object.entries((saved || {}) as object)) {
          if (typeof value === "string") channelDrafts.set(key, value);
        }
      } catch (_error) {
        // Storage may be absent or contain an interrupted write; composing still works in this page.
      }
    },
  };
}
