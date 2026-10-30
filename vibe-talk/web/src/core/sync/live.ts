// How many live events have arrived for each channel (`remote-ops-audit` W7, `#308
// live-reply-in-main`; `#624 voice-page-modularity`, moved from the legacy script; plan step 14): every
// event, the news among them (not the stream's replay tail), and the removals. A read whose answer lands
// after one may be older than what the event brought, so a read records them as it is sent and compares
// when it lands.

/** The owner's read surface and named commands. */
export interface LiveEvents {
  /** How many live events have arrived for `channel`. */
  seen(channel: string): number;
  /** How many of those were news: one that arrived after a read was sent says the service is answering. */
  news(channel: string): number;
  /** How many were removals: a placing read out as one arrived may carry the message removed. */
  removals(channel: string): number;
  /** A live event for `channel` arrived. */
  noteEvent(channel: string): void;
  /** ...and it was news, not the replay tail. */
  noteNews(channel: string): void;
  /** A removal for `channel` arrived. */
  noteRemoval(channel: string): void;
}

/** None arrived yet. */
export function createLiveEvents(): LiveEvents {
  const liveEventsSeen = new Map<string, number>();
  const liveNewsSeen = new Map<string, number>();
  const liveRemovalsSeen = new Map<string, number>();
  const count = (counts: Map<string, number>, channel: string): number => counts.get(channel) || 0;
  return {
    seen: (channel) => count(liveEventsSeen, channel),
    news: (channel) => count(liveNewsSeen, channel),
    removals: (channel) => count(liveRemovalsSeen, channel),
    noteEvent(channel) {
      liveEventsSeen.set(channel, count(liveEventsSeen, channel) + 1);
    },
    noteNews(channel) {
      liveNewsSeen.set(channel, count(liveNewsSeen, channel) + 1);
    },
    noteRemoval(channel) {
      liveRemovalsSeen.set(channel, count(liveRemovalsSeen, channel) + 1);
    },
  };
}
