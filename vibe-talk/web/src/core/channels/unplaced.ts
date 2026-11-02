// What a channel's store says of the messages the stream left unplaced (`#308 live-reply-in-main`;
// `#624 voice-page-modularity`, moved from the legacy script into core/channels): which of them no read
// of All has answered for, a read of All answering for them, and whether the store still holds what it
// held when a read was sent. The placing read itself — when it goes, what it asks, folding its answer —
// is the page's, whose section on it says why each rule is as it is. `forgetUnplaced` is ./removals.ts's.

import type { Message } from "../../contract/index.ts";
import type { ChannelCanon } from "./canon.ts";

/** The ids store `canon`'s stream left unplaced that no read of All has answered for, nor removed upstream. */
export function unplacedToAsk(canon: ChannelCanon): string[] {
  return [...canon.unplaced].filter((id) => !canon.unplacedAnswered.has(id) && !canon.removed.has(id));
}

/**
 * A read of All — its newest page, or its changes to the last of them — sent when `unplacedArrivals`
 * stood at `seen`, landed in store `canon`: it answers for every message left unplaced there that had
 * arrived by then, whether it carried it or not. What has been placed since is forgotten here.
 */
export function answerUnplaced(canon: ChannelCanon, seen: number): void {
  for (const id of [...canon.unplacedSince.keys(), ...canon.unplacedAnswered]) {
    if (!canon.unplaced.has(id)) {
      canon.unplacedSince.delete(id);
      canon.unplacedAnswered.delete(id);
    }
  }
  for (const id of canon.unplaced) {
    if ((canon.unplacedSince.get(id) || 0) <= seen) canon.unplacedAnswered.add(id);
  }
}

/**
 * Whether store `canon` holds what it held when a read was sent, `held` being its copies then
 * (`heldNow`): no message added since, none taken out, and none replaced. A read that brings a copy
 * word for word keeps the store's (`mergeIntoCanon`, `foldTimelineDelta`); one that brings an edit, a
 * thread record or a count replaces it, and the read sent before is older than that copy, which it
 * would undo. `#308 live-reply-in-main`.
 */
export function heldAsSent(canon: ChannelCanon, held: ReadonlyMap<string, Message>): boolean {
  return canon.messages.length === held.size &&
    canon.messages.every((message) => held.get(String(message.id)) === message);
}
