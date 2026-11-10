// A read landed: how fresh the rows are said to be, the quick retry after a stand-in page,
// and whether a message is one this page sent (`#624 voice-page-modularity`, moved from the
// legacy script). Words and order are as they were in the script. The timeline reader, the
// stand-in retries and the own-posts record are core's, made in the script and handed in.

/** How fresh a landed read says the rows are (core/sync `FreshDecision`). */
export interface FreshDecisionLike {
  readonly freshAt: number;
  readonly failed: boolean;
  readonly freshness: "failed" | "fresh" | "slow";
}

/** A landed read, as this owner reads it. */
export interface ReadPayload { pins_revision?: unknown }

/** What noting a fresh read and standing by own posts asks of the page. */
export interface FreshReadDependencies {
  timelineReader: { freshDecision(payload: unknown): FreshDecisionLike };
  standInRetries: { noteRead(failed: boolean): boolean };
  ownPosts: { sentFromHere(id: string): boolean; load(): void; stands(id: string): boolean };
  loadState(): { freshAt: number };
  channelPoll(): { armed(): boolean; dueAt(): number };
  scheduleDiscordPoll(ms: number): void;
  setChannelFreshness(state: string): void;
  notePinsRevision(revision: unknown): void;
  noteBookmark(payload: ReadPayload): void;
  providerOfChannel(id: unknown): { threading_supported?: unknown } | null;
  providerFacts: { threading(): boolean };
  selectedChannelId(): unknown;
}

/** Noting fresh reads and standing by this page's own posts. */
export interface FreshRead {
  noteFreshRead(payload: ReadPayload): void;
  sentFromThisPage(message: { id: unknown }): boolean;
  loadOwnPosts(): void;
  ownPostStands(id: unknown): boolean;
  storeKeepsOwnPosts(channel: string): boolean;
}

/** The fresh-read noting and own-post standing over `deps`. */
export function createFreshRead(deps: FreshReadDependencies): FreshRead {
  /**
   * A read landed: say how fresh the rows are — now, or as of what the server could vouch for.
   *
   * `#617 cold-load-first-paint`: a page from the server's own copy says "Refresh failed" only when
   * that copy is older than `STALE_AS_OF_MS`; a younger one is as current as the pill says ("Updated"
   * at the copy's time). The next read comes at the normal poll cadence either way
   * (`#1227 poll-storm-stale-thread`): a stale copy never makes the page ask sooner.
   */
  function noteFreshRead(payload: ReadPayload): void {
    const decision = deps.timelineReader.freshDecision(payload);
    deps.loadState().freshAt = decision.freshAt;
    const retryWanted = deps.standInRetries.noteRead(decision.failed);
    // A poll already armed comes forward; one armed after this read takes it up as it is armed.
    if (decision.failed && retryWanted && deps.channelPoll().armed()) {
      deps.scheduleDiscordPoll(Math.max(0, deps.channelPoll().dueAt() - Date.now()));
    }
    deps.setChannelFreshness(decision.freshness);
    // `#206 pin-message`. Every landed read, delta included, says what revision the pins are at.
    deps.notePinsRevision(payload.pins_revision);
    // `#240 reading-bookmark`. ...and where the bookmark is, which another device may have moved.
    deps.noteBookmark(payload);
  }


  /** Whether `message` was sent from this page (`ownPosts`). */
  const sentFromThisPage = (message: { id: unknown }): boolean => deps.ownPosts.sentFromHere(String(message.id));

  /** Take back what earlier pages of the device sent in the grace (`ownPosts`). */
  function loadOwnPosts(): void {
    deps.ownPosts.load();
  }

  /**
   * Whether message `id` is one this page posted within OWN_POST_GRACE_MS: a page that does not carry
   * it is not evidence that it is gone.
   *
   * THE OWNER'S REPLY VANISHED. Sent to a Slack message nobody had answered, it was drawn, then gone
   * from every view, and the message it answered went back to "start a thread": "my sent message
   * disappeared, and I didn't see it anywhere on the screen." Reproduced: the stream's copy of the
   * reply asks for a newest page of All (`reason=live`), sent after the post, and the chat service
   * answered it from state that did not have the reply yet — a bridge's lag, Slack's own Flat memory of
   * a thread that had no replies (`#282 slack-flat-skip`). That page carries the message answered with
   * a count of none, so it answers for its whole thread (`allPageAnswersFor`) and took the reply out as
   * deleted upstream; nothing was left to draw it, the receipt having gone as the reply was drawn. The
   * page orders copies by when its reads were sent, which is sound only while every answer is at
   * least as new as every earlier one (replication model, R9 and F3). Until the server says how new
   * each copy is (C7), this page's own fresh post is taken out only once it is told of its removal
   * (`noteRemoved`), never by its absence from a page alone.
   *
   * For copies without the server's versions only. Where a page says its `observed_ms` and a newest
   * page's `settled_below_ms` and the store's copy says its own `observed_ms` (`#473
   * replication-c7-server-versions`; the answer to a post carries it), no stale page can drop a copy
   * observed after it began, and `pageFold` decides by those before this is asked. Either way the
   * page's own post is kept, so its receipt is a display aid (`sentReceiptRetires`, `#500
   * sent-receipts-pile-up`).
   */
  function ownPostStands(id: unknown): boolean {
    return deps.ownPosts.stands(String(id));
  }

  /**
   * Whether the page's store keeps its own posts in channel `channel` against a page from older state
   * (`ownPostStands`, and the server's versions where said, `pageFold`): a channel with threads. Without
   * them a read replaces the rows, and the receipt is what draws a confirmed post a stale read left out.
   */
  function storeKeepsOwnPosts(channel: string): boolean {
    const provider = deps.providerOfChannel(channel);
    return provider ? provider.threading_supported === true
      : deps.providerFacts.threading() && channel === String(deps.selectedChannelId());
  }

  return { noteFreshRead, sentFromThisPage, loadOwnPosts, ownPostStands, storeKeepsOwnPosts };
}
