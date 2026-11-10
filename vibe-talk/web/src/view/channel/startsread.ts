// Whether a message starts read, drawing it so before the server's answer, and whether a
// message is still to do (`#624 voice-page-modularity`, moved from the legacy script).
// Words and order are as they were in the script. The switch, who-is-who and the ledgers
// are core's and the store's, made in the script and handed in.

import { isNoise } from "../../present/rows/index.ts";
import type { ChannelStore } from "../../core/channels/index.ts";
import type { ReadLedger } from "../../core/annotations/index.ts";
import type { Message } from "../../contract/index.ts";

/** Who is who, as starting read asks it (core/channels `Identities`). */
export interface StartsReadWho {
  choiceOf(id: string): string | undefined;
  isOwnerId(id: unknown): boolean;
  isSelfId(id: unknown): boolean;
  codingAgentsOf(channelId: unknown): { readonly ids: ReadonlySet<string> };
}

/** What starting read and still-to-do ask of the page. */
export interface StartsReadDependencies {
  ownReadSwitch: { on(): boolean };
  whoIsWho(): StartsReadWho;
  messageChannel(message: Message): string;
  providerFacts: { threading(): boolean };
  heldMessage(id: string): unknown;
  readLedger: ReadLedger;
  selectedChannelId(): unknown;
  channelStore: ChannelStore;
  doneState(): { archived: Set<string> };
  hiddenEmpty(message: Message): boolean;
}

/** Whether messages start read, noting it, and whether one is still to do. */
export interface StartsRead {
  startsRead(message: Message | null | undefined): boolean;
  noteStartsRead(messages: Message[]): void;
  stillToDo(message: Message, done?: ReadonlySet<string>): boolean;
}

/** The starts-read questions over `deps`. */
export function createStartsRead(deps: StartsReadDependencies): StartsRead {
  /**
   * Whether `message` is one of the reader's own whose read state starts read: "Mark my own messages as
   * read" on, not read as noise already, and his by the server's own order (src/own_read.rs
   * `Choice::owns`) — who-is-who first, then the configured accounts (`isOwnerId`), then this bridge's
   * account unless the configuration names it as a coding agent. That last is narrower than how a row
   * is drawn (`bucketFor` draws the bridge's account as "me" regardless), because a guess wider than the
   * server's would draw an agent's message read that the server leaves unread. `#549 own-read-stored`.
   */
  function startsRead(message: Message | null | undefined): boolean {
    if (!deps.ownReadSwitch.on() || !message || isNoise(message)) return false;
    const id = String(message.author_id || "");
    if (!id) return false;
    const chosen = deps.whoIsWho().choiceOf(id);
    if (chosen) return chosen === "me";
    if (deps.whoIsWho().isOwnerId(id)) return true;
    return deps.whoIsWho().isSelfId(id) && !deps.whoIsWho().codingAgentsOf(deps.messageChannel(message)).ids.has(id);
  }

  /**
   * Draw each of `messages` that `startsRead` read now, before the server's answer for it arrives: one
   * this page just sent, or one of his the stream brought. The server decided it at the same first
   * sight, and records it (`src/own_read.rs`), so this only spares a row of his a moment drawn unread
   * between its arrival and the read that follows. Held as a write made here, settled at once
   * (`localReadWrites`): a read sent before it does not undo it, and a read sent after it speaks for the
   * message, so the server's answer is the one that stands. A message the page already holds keeps
   * the state its reads gave it. `#549 own-read-stored`.
   *
   */
  function noteStartsRead(messages: Message[]): void {
    const fresh = messages.filter((message) => startsRead(message) &&
      !(deps.providerFacts.threading() && deps.heldMessage(String(message.id))) && !deps.readLedger.written(String(message.id)));
    if (fresh.length === 0) return;
    const epoch = deps.readLedger.bump();
    const here = String(deps.selectedChannelId());
    for (const message of fresh) {
      const id = String(message.id);
      deps.readLedger.seed(id, epoch);
      if (String(message.channel_id) !== here) continue;
      if (deps.providerFacts.threading() && deps.channelStore.current().channel === here) deps.channelStore.current().dismissed.add(id);
      deps.doneState().archived.add(id);
    }
  }

  /**
   * Does this message belong in the list behind Hide read?
   *
   * ONE definition, for both places the threaded view filters. Not archived, not read automatically,
   * and not the reader's own words when they have said those are read. And for what Fold read
   * collapses and what entering a thread lands on (`#221 read-modes`, `rowIsRead`): a refinement of
   * what counts as unread belongs here, and reaches every read mode at once.
   *
   * `done` is the read state asked: the marks for the rows on screen by default, and the store's for
   * a reply Main does not draw (`#222 unread-replies`, `replyTally`).
   */
  function stillToDo(message: Message, done: ReadonlySet<string> = deps.doneState().archived): boolean {
    return !done.has(String(message.id)) && !isNoise(message) && !deps.hiddenEmpty(message);
  }

  return { startsRead, noteStartsRead, stillToDo };
}
