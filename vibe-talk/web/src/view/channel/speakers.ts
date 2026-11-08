// Who is speaking in the channel, the page's half (`#624 voice-page-modularity`, moved from the
// legacy script into view/channel): the redraw a newly learned account of ours needs, and which
// channel a message is from. Who is who itself is core/channels' (`createIdentities`),
// instantiated by the page and handed in; Settings' list stays mounted by the page.

import type { Message } from "../../contract/index.ts";

/** Who is who, as far as noting our own account reads it (core/channels owns it). */
export interface IdentitiesLike {
  noteSelfAuthor(id: unknown): boolean;
}

/** What noting a speaker asks of the page. */
export interface SpeakersDependencies {
  readonly whoIsWho: IdentitiesLike;
  renderChannelRows(): void;
  renderIdentityList(): void;
  selectedChannelId(): string;
}

/** The speakers' commands and questions. */
export interface Speakers {
  noteSelfAuthor(id: unknown): void;
  messageChannel(message: Message | null | undefined): string;
}

/** Nothing noted. */
export function createSpeakers(deps: SpeakersDependencies): Speakers {
  function noteSelfAuthor(id: unknown): void {
    if (!deps.whoIsWho.noteSelfAuthor(id)) return;
    deps.renderChannelRows();
    deps.renderIdentityList();
  }

  function messageChannel(message: Message | null | undefined): string {
    return String((message && (message as { channel_id?: unknown }).channel_id) || deps.selectedChannelId() || "");
  }

  return { noteSelfAuthor, messageChannel };
}
