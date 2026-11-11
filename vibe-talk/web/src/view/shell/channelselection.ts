// The channel chosen to read, and the bar's picker as its projection (`#624
// voice-page-modularity`, moved from the legacy script). Words and order are as they
// were in the script. The selection's owner is core/navigation's (`createNavigation`),
// handed in as the prelude's factories are here, as in navigation.ts.

/** The selection's owner, as this owner uses it (core/navigation `Navigation`). */
export interface NavigationLike {
  snapshot(): { readonly channelId: string };
  selectChannel(channelId: string): void;
  subscribe(listener: (snapshot: { readonly channelId: string }) => void): () => void;
}

/** What choosing a channel asks of the page. */
export interface ChannelSelectionDependencies {
  createNavigation(): NavigationLike;
  picker(): { value: string };
  channels(): readonly { id: unknown }[];
}

/** The channel chosen to read, under its page names. */
export interface ChannelSelection {
  selectionOwner(): NavigationLike;
  selectedChannelId(): string;
  chooseChannel(id: string): void;
}

/** The channel selection over `deps`; its owner is made on first use. */
export function createChannelSelection(deps: ChannelSelectionDependencies): ChannelSelection {
  /**
   * The selection's one owner (web/src/core/navigation, `#624 voice-page-modularity`): the channel
   * chosen to read. Made on first use, when the bar's picker is subscribed to it as its projection; an owner already
   * made is never thrown away.
   *
   */
  let pageNavigation: NavigationLike | undefined;

  /** {@link pageNavigation}, made if it is not yet. */
  function selectionOwner(): NavigationLike {
    if (!pageNavigation) {
      pageNavigation = deps.createNavigation();
      // The bar's picker shows the selection, and nothing else: no reader asks the picker which
      // channel is chosen. It has no right-column stand-in, so `el` always finds the bar's.
      pageNavigation.subscribe((snapshot) => {
        deps.picker().value = snapshot.channelId;
      });
    }
    return pageNavigation;
  }

  /**
   * The id of the channel chosen to read ("" for none). Page-wide: both columns read one channel.
   *
   */
  function selectedChannelId(): string {
    return selectionOwner().snapshot().channelId;
  }

  /**
   * Choose channel `id` to read, as the bar's picker takes a value: one it does not list chooses no
   * channel at all, as setting a `<select>` to a value it has no option for leaves it on none. The
   * caller says what the choice means (`changeSelectedChannel`); this only records it.
   *
   */
  function chooseChannel(id: string): void {
    selectionOwner().selectChannel(deps.channels().some((channel) => String(channel.id) === String(id)) ? String(id) : "");
  }

  return { selectionOwner, selectedChannelId, chooseChannel };
}
