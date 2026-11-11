// The channel's name for a picker and the summary seam at its head (`#624
// voice-page-modularity`, moved from the legacy script). Words and order are as they
// were in the script. The directory, the summary's words and the older pages are the
// page's, handed in.


/** A known channel, as its name is read here. */
export type KnownChannel = object;

/** What naming a channel and restating its seam ask of the page. */
export interface ChannelHeadDependencies {
  knownChannel(id: unknown): KnownChannel | null;
  channelName(channel: unknown): string;
  el(id: string): HTMLElement;
  readModes(): { hiding(): boolean };
  renderChannelSeam(summary: string): void;
  todoSummary(): string;
  channelSummary(loaded: number, whole: boolean, name: string): string;
  olderPager(): { loadedIsWhole(more: unknown): boolean };
  olderPages(): { moreAbove: unknown };
  selectedChannelId(): unknown;
}

/** A channel's picker name and the seam at its head restated. */
export interface ChannelHead {
  channelLabel(channelId: unknown): string;
  restateChannelSeam(): void;
}

/** The channel head over `deps`. */
export function createChannelHead(deps: ChannelHeadDependencies): ChannelHead {
  /**
   * The name the channel picker shows for a snowflake, or the snowflake itself for one no longer on
   * the list (an outbox entry may outlive its channel). From the list, not the picker's children:
   * those are service groups once there are several. `#355 provider-grouped-channels`.
   */
  function channelLabel(channelId: unknown): string {
    const channel = deps.knownChannel(channelId);
    return channel ? deps.channelName(channel) : String(channelId);
  }

  /**
   * Rewrite the line at the head of the channel, which is already on screen saying the old name.
   *
   * From what is on the screen rather than by re-reading: a rename is not a reason to spend a
   * request, and a seam left standing with the previous name is the most confident possible way of
   * being wrong. It does nothing before the first read, when there is no seam to correct.
   */
  function restateChannelSeam(): void {
    if (deps.el("channel-summary").children.length === 0) {
      return;
    }
    if (deps.readModes().hiding()) {
      deps.renderChannelSeam(deps.todoSummary());
      return;
    }
    deps.renderChannelSeam(
      deps.channelSummary(
        deps.el("discord-log").children.length,
        deps.olderPager().loadedIsWhole(deps.olderPages().moreAbove),
        deps.channelName(deps.knownChannel(deps.selectedChannelId()))
      )
    );
  }

  return { channelLabel, restateChannelSeam };
}
