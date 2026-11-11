// The words for a slow service and for a channel read's failure (`#624
// voice-page-modularity`, moved from the legacy script). Words and order are as they
// were in the script. The readable words themselves are core/api's, handed in; whose
// service a channel is on is the directory's, also handed in.

/** A failure refusing a message's direct read to the pillar, as `readableFailure` reads it. */
export interface FailureLike {
  network?: unknown;
  refused?: unknown;
  status?: unknown;
  code?: unknown;
  message?: string;
}

/** What naming a channel's service and a failure asks of the page. */
export interface ServiceWordsDependencies {
  readableFailure(error: FailureLike, service: string): string;
  providerOfChannel(id: unknown): { name?: unknown } | null;
  selectedChannelId(): unknown;
  chatServiceName(): string;
}

/** Which service a channel is on, and a read failure said for the reader. */
export interface ServiceWords {
  chatServiceOf(channel: unknown): string;
  readableChannelFailure(error: FailureLike): string;
}

/** The service words over `deps`. */
export function createServiceWords(deps: ServiceWordsDependencies): ServiceWords {
  // `SLOW_WORDS`, `providerWords`, `isSlowFailure` and `readableFailure` are web/src/core/api's
  // (`#624 voice-page-modularity`).

  /** The chat service a channel is read through, as a noun phrase for a sentence. */
  function chatServiceOf(channel: unknown): string {
    const provider = deps.providerOfChannel(channel);
    return (provider && typeof provider.name === "string" && provider.name.trim()) || deps.chatServiceName();
  }

  /** {@link readableFailure} for the channel on screen. */
  function readableChannelFailure(error: FailureLike): string {
    return deps.readableFailure(error, chatServiceOf(deps.selectedChannelId()));
  }

  return { chatServiceOf, readableChannelFailure };
}
