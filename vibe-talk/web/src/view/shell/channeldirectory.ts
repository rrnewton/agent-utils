// What to call a channel, the channels grouped by service, the bar's picker and the active
// channel remembered (`#624 voice-page-modularity`, moved from the legacy script). Words and
// order are as they were in the script. What the server said is core/channels'
// `createChannelDirectory`, made in the script and handed in; Settings' boxes stay there too.

import type { ChannelInfo, ProviderDescription } from "../../contract/index.ts";
/** The directory of channels and services, as this owner uses it (core/channels `ChannelDirectory`). */
export interface DirectoryLike {
  providerName(): string;
  learnProviders(providers: unknown): ProviderDescription[];
  providerOf(id: unknown): ProviderDescription | null;
  providers(): Map<string, ProviderDescription>;
  channels(): ChannelInfo[];
  known(id: unknown): ChannelInfo | null;
  capabilities(): unknown;
  setChannels(channels: ChannelInfo[]): void;
  setHidden(channels: ChannelInfo[]): void;
}

/** Settings' channel box and browser, as this owner uses them. */
export interface ChannelBoxes {
  fillServiceSelect(): void;
  fillSettingsSelect(): void;
  renderHiddenChannels(): void;
}

/** What naming and grouping a channel asks of the page. */
export interface ChannelDirectoryViewDependencies {
  directory(): DirectoryLike;
  activeChannelKey: string;
  el(id: string): HTMLElement;
  selectedChannelId(): string;
  selectionOwner(): { selectChannel(id: string): void };
  renderLiveState(): void;
  keepWindowItem(key: string, value: string | null): void;
  windowItem(key: string): string | null;
  deviceStorage(): { getItem(key: string): string | null; setItem(key: string, value: string): void; removeItem(key: string): void };
  fromProvider(provider: ProviderDescription | null, capabilities: unknown): void;
  saveCacheChannels(): void;
  learnProviderAccounts(providers: ProviderDescription[]): void;
  boxes(): ChannelBoxes;
}

/** A channel as anything the server calls one may be handed to `channelName`. */
interface NamedLike { alias?: unknown; label?: unknown; id?: unknown }

/** Channel names, groups, the picker and the remembered channel. */
export interface ChannelDirectoryView {
  NO_CHANNEL_TO_READ: string;
  UNDESCRIBED_SERVICE: string;
  channelName(channel: unknown): string;
  chatServiceName(): string;
  learnProviders(providers: unknown): void;
  providerOfChannel(id: unknown): ProviderDescription | null;
  serviceLabel(key: string): string;
  channelGroups(channels?: ChannelInfo[]): { provider: string | null; heading: string; channels: ChannelInfo[] }[];
  channelsGrouped(): boolean;
  channelsInPickerOrder(): ChannelInfo[];
  applyChannelProvider(): void;
  pickerName(name: string): string;
  knownChannel(id: unknown): ChannelInfo | null;
  storedActiveChannel(): string;
  rememberActiveChannel(chosen?: boolean): void;
  fillChannelSelect(id: string): void;
  adoptChannelList(payload: { channels?: ChannelInfo[]; hidden_channels?: ChannelInfo[] } | null | undefined): void;
}

/** The channel directory's drawing over `deps`. */
export function createChannelDirectoryView(deps: ChannelDirectoryViewDependencies): ChannelDirectoryView {
  // --- what to call a channel ----------------------------------------------------------------------
  //
  // `#39 channel-alias`. A name of the OWNER's own for a channel, because saying which channel he
  // means has to be possible out loud: `1532416065114607829` is unsayable, and a configured label
  // like "build noise" is not what anyone says either.
  //
  // THREE things this deliberately is not, each of them said on the screen as well as here:
  //
  //   * It is NOT a rename in Discord. The server keeps the name and never tells Discord; the
  //     channel keeps whatever it is called there. The server states that on every answer and this
  //     page shows THAT sentence rather than writing its own — same rule `#50 todo-view` follows
  //     with `read_state_notice`, and for the same reason: a second copy of a policy is a second
  //     thing that can go stale.
  //   * It is NOT visible to anyone outside this deployment.
  //   * It is NOT the agent's to choose. The agent is HANDED it — `list_channels` and the digest
  //     header carry it — and the server offers no tool that sets one. This editor is the only way
  //     in, and it is behind the write token, which is the operator's.
  //
  // ONE RULE, IN ONE PLACE: the alias when there is one, the configured label otherwise. Every
  // render of a channel's name on this page goes through `channelName`, so the picker, the head of
  // the channel and the head of the to-do list cannot come to disagree about what it is called.

  /**
   * The name to show for a channel, from anything the server calls a channel.
   *
   * Trimmed and emptiness-checked rather than a bare `alias || label`: the server refuses a blank
   * one, so an empty string arriving here means something upstream is wrong, and a picker with a
   * blank entry in it is worse than one showing the configured label.
   */
  function channelName(incoming: unknown): string {
    const channel = incoming as NamedLike | null | undefined;
    if (!channel) {
      return "this channel";
    }
    const alias = typeof channel.alias === "string" ? channel.alias.trim() : "";
    return alias || String(channel.label || channel.id || "this channel");
  }


  const NO_CHANNEL_TO_READ =
    "no channel to read — your channel list is empty. " +
    "Add one, or show a hidden one again, in Settings.";

  /** The chat service as a noun phrase: its configured name, or a generic one. */
  function chatServiceName(): string {
    return deps.directory().providerName() || "the chat service";
  }

  /** Remember every provider's description, and the accounts each one speaks as. */
  function learnProviders(providers: unknown): void {
    deps.learnProviderAccounts(deps.directory().learnProviders(providers));
  }

  /** The provider description for the channel `id`, or null when the server did not say. */
  function providerOfChannel(id: unknown): ProviderDescription | null {
    return deps.directory().providerOf(id);
  }

  // --- channels grouped by service -----------------------------------------------------------------
  //
  // `#355 provider-grouped-channels`. One server may read several chat services, and a flat list of
  // their channels hides which is which: two called "general" are one name. So a channel picker
  // groups its channels under a header per service, in the order the server configures them, once
  // two or more services have channels. With one, or with a server that describes no services, the
  // list is exactly the flat one it always was. The closed picker shows the channel's name alone;
  // the open list carries the headers, so the bar keeps its width on a phone.

  /** The header a group shows: the service's name, with its key where two services share it. */
  function serviceLabel(key: string): string {
    const described = deps.directory().providers().get(key);
    const name = described && typeof described.name === "string" && described.name.trim()
      ? described.name.trim()
      : String(key);
    const shared = [...deps.directory().providers().values()].filter(
      (other) => other !== described && typeof other.name === "string" && other.name.trim() === name
    ).length > 0;
    return shared ? `${name} (${key})` : name;
  }

  /** The header of the group of channels whose service this server no longer describes. */
  const UNDESCRIBED_SERVICE = "Not connected";

  /**
   * `channels` by service: one group per described service that has channels, in the server's
   * order, then the channels of no described service (an added channel whose bridge is no longer
   * configured), never dropped.
   *
   * @returns {{provider: string | null, heading: string, channels: any[]}[]}
   */
  function channelGroups(channels: ChannelInfo[] = deps.directory().channels()): { provider: string | null; heading: string; channels: ChannelInfo[] }[] {
    const groups: { provider: string | null; heading: string; channels: ChannelInfo[] }[] = [...deps.directory().providers().keys()].map((key) => ({
      provider: /** @type {string | null} */ (key),
      heading: serviceLabel(key),
      channels: channels.filter((channel) => channel.provider === key),
    }));
    const rest = channels.filter(
      (channel) => typeof channel.provider !== "string" || !deps.directory().providers().has(channel.provider)
    );
    groups.push({ provider: null, heading: UNDESCRIBED_SERVICE, channels: rest });
    return groups.filter((group) => group.channels.length > 0);
  }

  /** Whether the pickers show service groups: two or more services have channels. */
  function channelsGrouped(): boolean {
    return channelGroups().length >= 2;
  }

  /**
   * The channels in the order the bar's picker shows them: grouped by service when it is, else the
   * server's own order. Alt+↑ and Alt+↓ step through this order, and a picker with nothing to keep
   * opens on its first.
   */
  function channelsInPickerOrder(): ChannelInfo[] {
    return channelsGrouped() ? channelGroups().flatMap((group) => group.channels) : deps.directory().channels();
  }


  /**
   * Act on the selected channel's own provider: its threads, its read cursor, its live delivery.
   *
   * Called before anything reads the new channel, because whether to read a timeline or a flat
   * history is exactly the kind of decision that must not be made with another service's answer.
   */
  function applyChannelProvider(): void {
    const provider = providerOfChannel(deps.selectedChannelId());
    deps.fromProvider(provider, deps.directory().capabilities());
    deps.renderLiveState();
  }

  // `sentenceStart` is web/src/foundation's (`#624 voice-page-modularity`).


  /**
   * A channel's name in a picker: its name without a leading '#'. `#371 channel-search-hash`.
   *
   * A native <select> finds an option by the first letters of its text as they are typed, so a Slack
   * channel listed as `#team-builds` was never reached by typing `team`. The '#' says only that it is
   * a Slack channel, which the picker's service header (or Settings' Service row) says already.
   * Everywhere else a channel keeps its name as written.
   */
  function pickerName(name: string): string {
    return name.replace(/^#+\s*/, "") || name;
  }

  function knownChannel(id: unknown): ChannelInfo | null {
    return deps.directory().known(id);
  }

  /**
   * The channel to open on: the one this window was on, which a reload of it comes back to, and for a
   * brand-new window, which has none, the one chosen last in any window. `#265 window-channel-memory`.
   * One the server no longer lists gives way to its first, in `fillChannelSelect`, as it always has.
   */
  function storedActiveChannel(): string {
    const own = deps.windowItem(deps.activeChannelKey);
    if (own) return own;
    try {
      return deps.deviceStorage().getItem(deps.activeChannelKey) || "";
    } catch (_error) {
      return "";
    }
  }

  /**
   * Keep the channel on the bar as this window's. `chosen`, when the reader changed to it: then it is
   * also the device's, the one a brand-new window opens on. A channel only put on the bar — the one a
   * new window opened on, a list redrawn — is this window's alone, so another window's choice is not
   * taken back by a window that merely reloaded. `#265 window-channel-memory`.
   */
  function rememberActiveChannel(chosen = false): void {
    const id = deps.selectedChannelId();
    deps.keepWindowItem(deps.activeChannelKey, id || null);
    if (!chosen) return;
    try {
      if (id) deps.deviceStorage().setItem(deps.activeChannelKey, id);
      else deps.deviceStorage().removeItem(deps.activeChannelKey);
    } catch (_error) {
      // Switching spaces still works when this browser cannot retain the preference.
    }
  }

  /**
   * Redraw one channel picker from the directory's channels (`channelDirectory`), keeping it on the
   * channel it was showing.
   *
   * A rename is a redraw, NOT a change of channel. Rebuilding the options moves a `select` to its
   * first entry unless the value is put back, and doing that to `#discord-channel` would silently
   * move the Discord view to a different channel because a name changed somewhere else.
   */
  function fillChannelSelect(id: string): void {
    if (id === "settings-channel") return deps.boxes().fillSettingsSelect();
    const select = deps.el(id);
    // Restore only an ID still present in the server's current channel list. Settings has its own
    // picker and must not change the remembered reading destination merely by editing a channel.
    // The bar's picker shows the selection (`navigation`), so its channel is the selection's.
    const chosen = deps.selectedChannelId() || storedActiveChannel();
    select.replaceChildren();
    const optionFor = (channel: ChannelInfo): HTMLOptionElement => {
      const option = document.createElement("option");
      option.value = channel.id as unknown as string;
      option.textContent = pickerName(channelName(channel));
      return option;
    };
    // `#355 provider-grouped-channels`. The bar's picker groups its channels by service once two or
    // more have channels; each group is an <optgroup>, so the open list has a header per service.
    const grouped = channelsGrouped();
    if (grouped) {
      for (const group of channelGroups()) {
        const header = document.createElement("optgroup");
        // The attribute, not the property of that name: reading or writing a `label` property is
        // what the server's `#39 channel-alias` guard counts, as a channel named past its alias.
        header.setAttribute("label", group.heading);
        header.append(...group.channels.map(optionFor));
        select.append(header);
      }
    } else {
      for (const channel of deps.directory().channels()) select.append(optionFor(channel));
    }
    const order = grouped ? channelsInPickerOrder() : deps.directory().channels();
    const showing = order.some((channel) => String(channel.id) === String(chosen))
        ? chosen
        : order.length > 0
          ? order[0]!.id
          : "";
    // The bar's picker is redrawn onto the selection, which this keeps on its channel or moves to the
    // first: through its owner, whose picker then shows it.
    deps.selectionOwner().selectChannel(String(showing));
    rememberActiveChannel();
  }

  /** Take the server's whole answer about the list: both pickers, the hidden list, the cache. */
  function adoptChannelList(payload: { channels?: ChannelInfo[]; hidden_channels?: ChannelInfo[] } | null | undefined): void {
    if (payload && Array.isArray(payload.channels)) deps.directory().setChannels(payload.channels);
    if (payload && Array.isArray(payload.hidden_channels)) deps.directory().setHidden(payload.hidden_channels);
    deps.saveCacheChannels();
    deps.boxes().fillServiceSelect();
    fillChannelSelect("discord-channel");
    fillChannelSelect("settings-channel");
    deps.boxes().renderHiddenChannels();
  }


  return {
    NO_CHANNEL_TO_READ, UNDESCRIBED_SERVICE, channelName, chatServiceName, learnProviders,
    providerOfChannel, serviceLabel, channelGroups, channelsGrouped, channelsInPickerOrder,
    applyChannelProvider, pickerName, knownChannel, storedActiveChannel, rememberActiveChannel,
    fillChannelSelect, adoptChannelList,
  };
}
