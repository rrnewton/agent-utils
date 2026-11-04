// Every unsent draft the reader has, wherever it was written (`#819 voice-page-quoted-reply-and-draft-chip`):
// the main box and a thread's box (by box key), a reply in the stream (by message id), and a quick reply (by
// message and quoted words), each with the channel it was written in and when it was last written, kept on
// the device. The drafts' words are their own owners' (`createChannelDrafts`, `createReplyDrafts`); this is
// how the page finds them all again, newest first: the Draft chip. The owner lost a dictated reply.

import type { StoragePort } from "../../ports/index.ts";
import { storedExactly as storedExactlyIn } from "./storage.ts";

/** Where the device keeps it. */
const DRAFT_LEDGER_KEY = "vibe-talk.draft-ledger";

/** Where a draft is written: a message box, a reply in the stream, or a quick reply. */
export type DraftKind = "composer" | "reply" | "quick";

/** One draft: where it is, under which key its owner keeps it, its channel, and when it was last written (ms). */
export interface DraftEntry {
  readonly kind: DraftKind;
  readonly key: string;
  readonly channel: string;
  readonly at: number;
}

/** What the ledger asks of the page. */
export interface DraftLedgerDependencies {
  readonly storage: StoragePort;
  /** The device's quota is full: give up the saved channel rows first (`dropMessageCache`). */
  makeRoom(): void;
  now(): number;
}

/** The ledger's commands and questions. */
export interface DraftLedger {
  /** Draft `key` of kind `kind`, in channel `channel`, was written just now. */
  touch(kind: DraftKind, key: string, channel: string): void;
  /** Draft `key` of kind `kind` is gone: sent or emptied. */
  drop(kind: DraftKind, key: string): void;
  /** Whether draft `key` of kind `kind` is in the ledger. */
  has(kind: DraftKind, key: string): boolean;
  /** The drafts of channel `channel` that `live` says still hold words, newest first. */
  inChannel(channel: string, live: (entry: DraftEntry) => boolean): DraftEntry[];
  /** Let go of every draft `held` says its store no longer holds, and save what is left. */
  prune(held: (entry: DraftEntry) => boolean): void;
  /** Write it to the device, read back to be sure; whether the device kept it. */
  save(): boolean;
  /** Take back what the device kept; storage absent or a record that does not parse leaves none. */
  load(): void;
}

const KINDS: readonly DraftKind[] = ["composer", "reply", "quick"];

/** An empty ledger. */
export function createDraftLedger(deps: DraftLedgerDependencies): DraftLedger {
  const entries = new Map<string, DraftEntry>();
  const id = (kind: DraftKind, key: string): string => `${kind}\n${key}`;
  const storedExactly = (encoded: string): boolean => storedExactlyIn(deps.storage, DRAFT_LEDGER_KEY, encoded);

  return {
    touch(kind, key, channel) {
      entries.set(id(kind, key), { kind, key, channel, at: deps.now() });
    },
    drop(kind, key) {
      entries.delete(id(kind, key));
    },
    has: (kind, key) => entries.has(id(kind, key)),
    inChannel(channel, live) {
      return [...entries.values()].filter((entry) => entry.channel === channel && live(entry)).sort((a, b) => b.at - a.at);
    },
    prune(held) {
      let dropped = false;
      for (const [at, entry] of entries) {
        if (held(entry)) continue;
        entries.delete(at);
        dropped = true;
      }
      if (dropped) this.save();
    },
    save() {
      try {
        const encoded = JSON.stringify([...entries.values()]);
        if (storedExactly(encoded)) return true;
        deps.makeRoom();
        return storedExactly(encoded);
      } catch (_error) {
        return false;
      }
    },
    load() {
      try {
        const saved: unknown = JSON.parse(deps.storage.getItem(DRAFT_LEDGER_KEY) || "[]");
        if (!Array.isArray(saved)) return;
        for (const one of saved) {
          if (!one || typeof one !== "object") continue;
          const { kind, key, channel, at } = one as Record<string, unknown>;
          if (!KINDS.includes(kind as DraftKind) || typeof key !== "string" || typeof channel !== "string") continue;
          entries.set(id(kind as DraftKind, key), { kind: kind as DraftKind, key, channel, at: typeof at === "number" ? at : 0 });
        }
      } catch (_error) {
        // Storage may be absent or hold an interrupted write; the drafts themselves are still kept.
      }
    },
  };
}

/** A draft's identity: where it is written and its key there. */
export function draftIdentity(draft: { readonly kind: DraftKind; readonly key: string }): string {
  return `${draft.kind}\n${draft.key}`;
}

/** The Draft chip's round: which draft its next tap goes to. */
export interface DraftRound {
  /**
   * The draft a tap goes to among `drafts` (newest first) in channel `channel`: the one after the last it went
   * to there, round to the newest again; the newest where it last went elsewhere, or to a draft gone since.
   * `offers` (`#846 draft-chip-honest`) passes over the drafts the chip does not offer — those on screen —
   * without losing the round's place among them all; null when it offers none.
   */
  next<Entry extends { readonly kind: DraftKind; readonly key: string }>(
    drafts: readonly Entry[], channel: string, offers?: (draft: Entry) => boolean): Entry | null;
}

/** Gone nowhere yet. */
export function createDraftRound(): DraftRound {
  let last = { channel: "", draft: "" };
  return {
    next(drafts, channel, offers = () => true) {
      const at = last.channel === channel ? drafts.findIndex((draft) => draftIdentity(draft) === last.draft) : -1;
      for (let step = 1; step <= drafts.length; step += 1) {
        const next = drafts[(at + step) % drafts.length]!;
        if (!offers(next)) continue;
        last = { channel, draft: draftIdentity(next) };
        return next;
      }
      return null;
    },
  };
}

// What the page and the tests use of it.
export { DRAFT_LEDGER_KEY };
