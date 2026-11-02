// The Settings channel box: service choice, add/rename/remove, hidden channels and the picker.
// Moved from voice.js for #624 voice-page-modularity. Reading selection and provider grouping stay
// with the channel view; all channel facts come from the injected directory owner.

import type { ChannelInfo } from "../../contract/index.ts";
import type { ChannelDirectory as Directory } from "../../core/channels/index.ts";
import type { ElementById } from "../dom/index.ts";
import { addChannelForm, allChannelsFacts, emptyServiceWords, serviceFacts, STRAY_SERVICE_FACTS, writeSourceWords } from "../../present/settings/index.ts";
import type { ChannelDirectory } from "./directory.ts";
import type { SettingsFolds } from "./folds.ts";
import type { IdentityList } from "./identities.ts";
import type { UpstreamClearBox } from "./upstream.ts";
import type { ChannelHistoryBox } from "./history.ts";

type ChannelAnswer = { channel?: ChannelInfo; alias_notice?: string } & Record<string, unknown>;

export interface ChannelSettingsDependencies {
  lookup(id: string): HTMLElement;
  readonly directory: Directory;
  readonly allChannels: string;
  readonly undescribedService: string;
  tokenScope(): string;
  selectedChannelId(): string;
  serviceLabel(key: string): string;
  chatServiceName(): string;
  channelName(channel: ChannelInfo | { label: string }): string;
  pickerName(name: string): string;
  browser(): ChannelDirectory;
  upstream(): UpstreamClearBox;
  history(): ChannelHistoryBox;
  identities(): IdentityList;
  folds(): SettingsFolds;
  request(path: string, options: { method: string; body?: unknown }): Promise<ChannelAnswer>;
  guard(run: () => Promise<void>): () => void;
  adoptChannelList(payload: unknown): void;
  changeSelectedChannel(): Promise<void>;
  forgetChannelScopes(matches: (channel: string) => boolean): void;
  saveCacheChannels(): void;
  fillReadingSelect(): void;
  restateChannelSeam(): void;
  setStatus(text: string): void;
}

export function mountChannelSettings(deps: ChannelSettingsDependencies) {
  const el = <K extends keyof ElementById>(id: K): ElementById[K] => deps.lookup(id) as ElementById[K];
  const EMPTY_CHANNEL_LIST = "Your channel list is empty. Add a channel, or show a hidden one again below.";
  // --- Settings, one service at a time -------------------------------------------------------------
  //
  // `#355 provider-grouped-channels`. The owner: "under the Channels setting ... drop-down for
  // Service=Slack/etc and then within that a list of subscribed channels". With two or more services,
  // Settings > Channels starts with a Service picker; the channel picker lists that service's channels,
  // a line says what the service does here, and Add, Browse and the hidden channels act on it. Settings
  // opens on the service of the channel being read. With one service none of this shows, and every
  // request is what it was.

  /** The service Settings is showing, by key, or null with one service. */
  let settingsService: string | null = null;

  /**
   * The Service picker's entry for the channels of no service the server describes (an added channel
   * whose bridge is no longer configured), shown only while there are some: every channel on the bar
   * can be reached from Settings, to be renamed or looked at.
   */
  const NO_SERVICE = "";

  /** The channels of no service the server describes: the bar's "Not connected" group. */
  function strayChannels() {
    return deps.directory.channels().filter(
      (channel) => typeof channel.provider !== "string" || !deps.directory.providers().has(channel.provider)
    );
  }

  /** Whether `key` is an entry of the Service picker now: a described service, or "Not connected" while it holds channels. */
  function settingsServiceOffered(key: string) {
    return deps.directory.providers().has(key as string) || (key === NO_SERVICE && strayChannels().length > 0);
  }

  /** Whether Settings chooses a service first: the server describes two or more. */
  function servicesInSettings() {
    return deps.directory.providers().size >= 2;
  }

  /** The description of the service Settings is showing, or null with one service. */
  function settingsServiceDescription() {
    return servicesInSettings() && settingsService !== null ? deps.directory.providers().get(settingsService) || null : null;
  }

  /** The service Settings is showing, as a noun phrase: its name, or the one service's. */
  function settingsServiceName() {
    return settingsServiceDescription() ? deps.serviceLabel((settingsService as string)) : deps.chatServiceName();
  }

  /** The channels Settings' picker lists: the chosen service's, or every one with one service. */
  function settingsChannels() {
    if (!servicesInSettings() || settingsService === null) return deps.directory.channels();
    if (settingsService === NO_SERVICE) return strayChannels();
    return deps.directory.channels().filter((channel) => channel.provider === settingsService);
  }

  /** The service Settings opens on with none chosen yet: the channel being read's, else the first with channels. */
  function defaultSettingsService() {
    const reading = deps.directory.known(deps.selectedChannelId());
    if (reading) return deps.directory.providers().has(reading.provider as string) ? reading.provider as string : NO_SERVICE;
    const keys = [...deps.directory.providers().keys()];
    return keys.find((key) => deps.directory.channels().some((channel) => channel.provider === key)) || keys[0] || null;
  }

  /** Whether the service Settings is showing takes a link or reference to add a channel, rather than an id. */
  function addsByLink() {
    const chosen = settingsServiceDescription();
    return chosen ? chosen.channel_registration_supported === true : deps.directory.registrationSupported();
  }

  /** Whether Browse is offered: the service can list its channels and add one by reference. */
  function browseOffered() {
    const chosen = settingsServiceDescription();
    return chosen
      ? chosen.channel_registration_supported === true && chosen.channel_discovery_supported === true
      : deps.directory.discoverySupported();
  }

  // What the chosen service does here, the line under the Service picker, is web/src/present/settings's
  // (`serviceFacts`, `#624 voice-page-modularity`).

  /**
   * Draw the Service picker: one entry per service, in the server's order, shown with two or more,
   * when it takes the place of the "Chat service" line.
   */
  function fillServiceSelect() {
    const shown = servicesInSettings();
    el("settings-service-row").hidden = !shown;
    el("chat-provider-line").hidden = shown;
    const select = el("settings-service");
    if (!shown) {
      settingsService = null;
      select.replaceChildren();
      return;
    }
    if (settingsService === null || !settingsServiceOffered(settingsService)) settingsService = defaultSettingsService();
    const keys = [...deps.directory.providers().keys()];
    if (strayChannels().length > 0) keys.push(NO_SERVICE);
    select.replaceChildren(...keys.map((key) => {
      const option = document.createElement("option");
      option.value = key;
      option.textContent = key === NO_SERVICE ? deps.undescribedService : deps.serviceLabel(key);
      return option;
    }));
    select.value = settingsService as string;
    const chosen = settingsServiceDescription();
    el("service-facts").textContent = chosen
      ? serviceFacts(chosen, deps.serviceLabel(chosen.key))
      : STRAY_SERVICE_FACTS;
  }

  /**
   * Show the service `key` in Settings, and the channel `channelId` in it when it is one of its own.
   * Another service than the one showing closes any open panel and empties the browse list: both
   * belonged to the service they were opened for.
   */
  function chooseSettingsService(key: string, channelId: string | null = null) {
    if (!servicesInSettings() || !settingsServiceOffered(key)) return;
    if (key !== settingsService) {
      settingsService = key;
      showChannelPanel(null);
      deps.browser().reset();
    }
    fillServiceSelect();
    fillSettingsSelect();
    if (channelId !== null && settingsChannels().some((channel) => String(channel.id) === String(channelId))) {
      el("settings-channel").value = String(channelId);
    }
    renderAddChannelForm();
    renderHiddenChannels();
    renderChannelBox();
  }

  /** Point Settings at the channel `channelId`: its service, then it. */
  function pointSettingsAt(channelId: string) {
    const listed = deps.directory.known(channelId) || deps.directory.hidden().find((channel) => String(channel.id) === String(channelId));
    if (listed && servicesInSettings()) {
      chooseSettingsService(deps.directory.providers().has(listed.provider as string) ? listed.provider as string : NO_SERVICE, channelId);
    } else {
      el("settings-channel").value = String(channelId);
    }
  }

  /**
   * Settings, opened, shows the service of the channel being read, and that channel when it is
   * another service than the one showing. Not on the way back from Help, which is the same visit.
   */
  function pointSettingsAtReading() {
    if (!servicesInSettings()) return;
    const reading = deps.directory.known(deps.selectedChannelId());
    if (!reading) return;
    const service = deps.directory.providers().has(reading.provider as string) ? reading.provider as string : NO_SERVICE;
    if (service !== settingsService) chooseSettingsService(service, reading.id);
  }

  /**
   * The add form as the service Settings shows takes a channel: a link or reference, or an id. With
   * several services an id is not always a snowflake (a Slack that does not register channels takes
   * its own ids), so the field asks for "the channel's ID" there.
   */
  function renderAddChannelForm() {
    const byLink = addsByLink();
    const service = settingsServiceName();
    const anyId = !byLink && settingsServiceDescription() !== null;
    // Nothing can be added to "Not connected": it is the channels of no service this server reads.
    const stray = servicesInSettings() && settingsService === NO_SERVICE;
    el("open-add-channel").hidden = stray;
    el("open-browse-channels").hidden = stray || !browseOffered();
    // The words and the field's shape are web/src/present/settings's (`#624 voice-page-modularity`).
    const { help, label: fieldName, inputMode, maxLength, placeholder } = addChannelForm(byLink, anyId, service);
    el("new-channel-help").textContent = help;
    el("new-channel-id-label").textContent = fieldName;
    el("new-channel-id").inputMode = inputMode;
    el("new-channel-id").maxLength = maxLength;
    el("new-channel-id").placeholder = placeholder;
    el("new-channel-writable-row").hidden = byLink;
    if (byLink) el("new-channel-writable").checked = false;
  }

  /**
   * Remove is offered for every channel on the list. `#199 removable-config-channels`.
   *
   * It used to be offered only for a channel added here, because the server refused the others: a
   * configured channel comes from a file it never writes. The server now HIDES one of those instead,
   * durably, so the owner's one verb works on whatever he is looking at, and what it did is said
   * after he presses it rather than as a reason the button is missing.
   */
  function renderRemoveChannel(channel: ChannelInfo | null | undefined) {
    el("remove-channel").hidden = channel === null || channel === undefined;
    el("remove-channel-state").textContent = "";
  }

  /**
   * Draw the hidden configured channels, each with Show again, or take the list away when there
   * are none. Names go through `deps.channelName` like every picker, so a channel he renamed is offered
   * back under his name for it.
   *
   * `#401 hidden-channels-fold`. The list is a fold at the foot of the Channels section, and its
   * heading says how many it holds, so a folded list still says whether there is anything in it.
   * Whether it is open is the reader's (`deps.folds().set`), never this redraw's. The group shown or hidden
   * brings a fold into sight or takes it out, which Expand all counts (`deps.folds().renderAll`).
   */
  function renderHiddenChannels() {
    // `#355 provider-grouped-channels`. With several services, the chosen service's hidden channels.
    const shown = !servicesInSettings() || settingsService === null
      ? deps.directory.hidden()
      : settingsService === NO_SERVICE
        ? deps.directory.hidden().filter((channel) => typeof channel.provider !== "string" || !deps.directory.providers().has(channel.provider))
        : deps.directory.hidden().filter((channel) => channel.provider === settingsService);
    const rows = shown.map((channel) => {
      const row = document.createElement("li");
      const name = document.createElement("span");
      name.className = "directory-name";
      name.textContent = deps.channelName(channel);
      const show = document.createElement("button");
      show.type = "button";
      show.className = "secondary";
      show.textContent = "Show again";
      show.setAttribute("aria-label", `Show ${deps.channelName(channel)} again`);
      show.addEventListener("click", deps.guard(() => showChannelAgain(channel)));
      row.append(name, show);
      return row;
    });
    el("hidden-channel-list").replaceChildren(...rows);
    el("fold-hidden-channels").textContent = `Hidden channels (${rows.length})`;
    el("hidden-channels").hidden = rows.length === 0;
    deps.folds().renderAll();
  }

  /** Put a hidden configured channel back on the list, and point the channel box at it. */
  async function showChannelAgain(channel: ChannelInfo) {
    const said = el("remove-channel-state");
    let payload: ChannelAnswer | null = null;
    try {
      payload = await deps.request(`/api/v1/channels/${encodeURIComponent(channel.id)}/hidden`, {
        method: "DELETE",
      });
    } catch (caught) {
      said.textContent = (caught as { message: string }).message;
      return;
    }
    const previousChannel = deps.selectedChannelId();
    deps.adoptChannelList(payload);
    pointSettingsAt(channel.id);
    renderChannelBox();
    said.textContent = `Shown again. "${deps.channelName(channel)}" is back in the picker.`;
    if (deps.selectedChannelId() !== previousChannel) await deps.changeSelectedChannel();
  }

  /** Add the channel named in the form, and put the answer where it can be read. */
  async function addChannel() {
    const id = el("new-channel-id").value.trim();
    const label = el("new-channel-label").value.trim();
    const said = el("add-channel-state");
    const byLink = addsByLink();
    if (!id || !label) {
      said.textContent = byLink
        ? "Both the channel reference and a name are needed."
        : "Both the id and a name are needed.";
      return;
    }
    said.textContent = byLink
      ? "Registering and checking that it can be read…"
      : "Checking that the bot can read it…";
    let payload: ChannelAnswer | null = null;
    /** @type {Record<string, unknown>} */
    const body: Record<string, unknown> = byLink
      ? { source: id, label }
      : { id, label, writable: el("new-channel-writable").checked };
    // `#355 provider-grouped-channels`. With several services, the one Settings is showing.
    if (settingsServiceDescription()) body.provider = settingsService;
    try {
      payload = await deps.request("/api/v1/channels", {
        method: "POST",
        body,
      });
    } catch (caught) {
      // THE SERVER'S OWN SENTENCE, not a rewrite of it. It is the one that knows whether this was a
      // typo, a channel the bot was never added to, or a token that has stopped working, and it says
      // which — a second explanation written here could only be vaguer.
      said.textContent = (caught as { message: string }).message;
      return;
    }
    // Redrawn from the answer rather than from a re-read: the server hands back the whole list
    // precisely so the two pickers and the editor cannot disagree about what exists.
    deps.adoptChannelList(payload);
    const addedId = String((payload.channel && payload.channel.id) || id);
    pointSettingsAt(addedId);
    renderChannelBox();
    el("new-channel-id").value = "";
    el("new-channel-label").value = "";
    el("new-channel-writable").checked = false;
    showChannelPanel(null);
    // Named from the answer: adding a configured channel the owner hid shows it again under the
    // file's label or his alias for it, not the name typed here. `#199 removable-config-channels`.
    const added = deps.channelName(payload.channel || { label });
    said.textContent = `Added. "${added}" is now in the channel picker.`;
  }

  /**
   * Take a channel off the list: forgotten when it was added here, hidden when it came from the
   * configuration file. The server decides which from the channel itself; the page only says which
   * one happened.
   */
  async function removeChannel() {
    const id = el("settings-channel").value;
    const channel = deps.directory.known(id);
    if (!channel) {
      return;
    }
    const said = el("remove-channel-state");
    let payload: ChannelAnswer | null = null;
    try {
      payload = await deps.request(`/api/v1/channels/${encodeURIComponent(id)}`, { method: "DELETE" });
    } catch (caught) {
      said.textContent = (caught as { message: string }).message;
      return;
    }
    const previousChannel = deps.selectedChannelId();
    deps.forgetChannelScopes((channel) => channel === String(id));
    deps.adoptChannelList(payload);
    // The directory is cached while Settings stays open. Put a removed source back into its real
    // state immediately; otherwise reopening Browse in the same page still said "Already added"
    // and offered no way to add it again.
    deps.browser().forget(id);
    deps.browser().render();
    renderChannelBox();
    said.textContent =
      channel.added === true
        ? "Removed. It is no longer in the picker."
        : `Removed from your list. "${deps.channelName(channel)}" is still in this server's ` +
          "configuration file, so it can be shown again below.";
    if (deps.selectedChannelId() !== previousChannel) await deps.changeSelectedChannel();
  }

  /**
   * Redraw everything in the channel box for the channel its picker is on, or for all of them.
   *
   * ONE CALL, because the box holds several things scoped by that picker — what this channel is called
   * and what it offers, whether its unread is cleared upstream, and who is in it — and every caller
   * needs all of them. `#349 channel-settings-all` took the list of who is who out of the box and
   * `#402 who-is-who-channel-box` put it back, so it is redrawn here again. The hidden channels under
   * the box are the service's, not the channel's (`renderHiddenChannels`), and are not redrawn here.
   */
  function renderChannelBox() {
    renderAliasEditor();
    deps.upstream().render();
    deps.history().render();
    deps.identities().render();
  }

  /** Whether Settings' channel box is on All channels. `#349 channel-settings-all`. */
  const allChannelsChosen = () => el("settings-channel").value === deps.allChannels;

  /** `#355 provider-grouped-channels`. What Settings says for a service with no channel on the list. */
  function emptyServiceSentence() {
    // Worded by web/src/present/settings (`emptyServiceWords`, `#624 voice-page-modularity`).
    return emptyServiceWords(browseOffered(), deps.directory.hidden().some((channel) => channel.provider === settingsService),
      settingsServiceName());
  }

  /** What the editor says about the channel it is pointed at, including the label underneath. */
  function renderAliasEditor() {
    // `#349 channel-settings-all`. Rename names one channel, so it steps aside for All channels, as
    // Remove does below; Add and Browse are not about the channel chosen, and stay.
    const all = allChannelsChosen();
    el("rename-channel").hidden = all;
    if (all) {
      el("channel-alias").value = "";
      // `#355 provider-grouped-channels`: the chosen service's, with several.
      const count = settingsChannels().length;
      el("channel-facts").textContent = allChannelsFacts(count, settingsServiceDescription() ? settingsServiceName() : null);
      el("alias-state").textContent = "";
      renderRemoveChannel(null);
      return;
    }
    const channel = deps.directory.known(el("settings-channel").value);
    if (!channel) {
      el("channel-alias").value = "";
      el("channel-facts").textContent = settingsServiceDescription() ? emptyServiceSentence() : EMPTY_CHANNEL_LIST;
      el("alias-state").textContent = "";
      renderRemoveChannel(null);
      return;
    }
    el("channel-alias").value = channel.alias || "";
    renderRemoveChannel(channel);
    // ONE LINE, ALWAYS VISIBLE, saying what this channel is. The configured label is named either
    // way: it is what clearing a name goes back to, and without it on screen the owner cannot tell
    // what he would be returning to. Where the channel CAME FROM is here too, because it is what
    // decides whether Remove is offered, and a missing button explains itself badly.
    const named = channel.alias
      ? `Called "${channel.alias}" here. The configured label is "${channel.label}".`
      : `No name of your own yet. The configured label is "${channel.label}".`;
    const origin = channel.added
      ? "Added in this app."
      : "From the server's configuration file.";
    const posting = writeSourceSentence(channel);
    el("channel-facts").textContent = posting ? `${named} ${origin} ${posting}` : `${named} ${origin}`;
    // The same sentence inside the rename panel, where it is what the input is described by.
    el("alias-state").textContent = named;
  }

  /**
   * Why a channel may not be posted to, or that posting follows a service's policy, by whose setting
   * `writable` is. `#374 registered-writable-follows-policy`. An older server sends no `write_source`,
   * and then nothing is said beyond what was said before.
   */
  function writeSourceSentence(channel: ChannelInfo) {
    const described = deps.directory.providerOf(channel.id);
    const service = (described && typeof described.name === "string" && described.name.trim()) || deps.directory.providerName() || "the chat service";
    // Worded by web/src/present/settings (`writeSourceWords`, `#624 voice-page-modularity`).
    return writeSourceWords(channel, service);
  }

  // --- clearing unread in the chat service's own app ----------------------------------------------
  //
  // `#232 upstream-mark-read`. With this on for a channel, the SERVER marks every message it receives
  // there read in the chat service's own app too, so that app's unread list keeps the people and
  // vibe-talk keeps the agents; `#349 channel-settings-all` sets it for every channel at once. The box is
  // web/src/view/settings `mountUpstreamClear`, in the words of web/src/present/settings (`#624
  // voice-page-modularity`); which channels it is offered for is decided here, with the channel list.

  /** Whether the box offers the control for the channel `id`. */
  function upstreamClearOffered(id: string) {
    if (deps.tokenScope() !== "write" || !deps.directory.known(id)) return false;
    const provider = deps.directory.providerOf(id);
    return provider ? provider.upstream_clear_supported === true : deps.directory.capabilities().upstreamClear;
  }

  /**
   * The channels All channels reaches: those whose chat service can clear unread, for this token. With
   * several services, only the chosen service's: `#355 provider-grouped-channels`.
   */
  function upstreamClearAllChannels() {
    return settingsChannels().map((channel) => String(channel.id)).filter((id) => upstreamClearOffered(id));
  }

  /**
   * Open one of the channel box's two panels, or neither.
   *
   * ONE AT A TIME. Both are text entry and the box lives on a phone; two open at once is the
   * full-height form this replaced. Passing `null` closes both, which is what changing channel does —
   * a half-typed name for the channel you just navigated away from is not a draft worth keeping.
   */
  function showChannelPanel(which: string | null) {
    for (const [panel, opener] of [
      ["rename-fields", "rename-channel"],
      ["add-channel-fields", "open-add-channel"],
      ["browse-channel-fields", "open-browse-channels"],
    ] as const) {
      const open = panel === which;
      el(panel).hidden = !open;
      el(opener).setAttribute("aria-expanded", open ? "true" : "false");
    }
    // `#371 channel-search-hash`. A directory read still on its way when the list is closed is given up
    // on: its count stops, and its answer is not drawn into a list nobody is looking at. Opening the list
    // again reads it again (the server answers from what it kept).
    if (which !== "browse-channel-fields") deps.browser().giveUp();
  }

  /**
   * Take the server's answer as the truth about the name, and carry its notice through unchanged.
   *
   * The server is what decides what got stored — it trims, and it refuses what it will not keep —
   * so the field is refilled from the answer rather than from what was typed.
   */
  function adoptChannel(payload: ChannelAnswer | null) {
    const updated = payload && payload.channel;
    if (updated) {
      deps.directory.replaceChannel(updated);
      deps.saveCacheChannels();
    }
    el("alias-note").textContent = (payload && payload.alias_notice) || "";
    deps.fillReadingSelect();
    fillSettingsSelect();
    renderChannelBox();
    deps.restateChannelSeam();
  }

  function aliasPath() {
    const channel = el("settings-channel").value;
    return `/api/v1/channels/${encodeURIComponent(channel)}/alias`;
  }

  async function saveAlias() {
    if (!deps.directory.known(el("settings-channel").value)) {
      return;
    }
    const payload = await deps.request(aliasPath(), {
      method: "PUT",
      body: { alias: el("channel-alias").value },
    });
    adoptChannel(payload);
    deps.setStatus("Saved. This app calls it that from now on.");
  }

  async function clearChannelAlias() {
    if (!deps.directory.known(el("settings-channel").value)) {
      return;
    }
    const payload = await deps.request(aliasPath(), { method: "DELETE" });
    adoptChannel(payload);
    deps.setStatus("Cleared. The configured label is back.");
  }


  function fillSettingsSelect() {
    const select = el("settings-channel");
    // Restore only an ID still present in the server's current channel list. Settings has its own
    // picker and must not change the remembered reading destination merely by editing a channel.
    // The bar's picker shows the selection (`navigation`), so its channel is the selection's.
    const chosen = select.value || "";
    select.replaceChildren();
    // `#349 channel-settings-all`. Settings' picker opens with All channels where there is more than
    // one: Clear unread set for every channel at once. Never the reading picker's: a view reads one.
    // `#351 settings-polish`: and only where one of them, at least, can take Clear unread with this
    // token. All channels is for that one setting, and Rename and Remove step aside for it, so with none
    // that can (no chat service that clears unread, or a read token) it was a choice that left nothing to do.
    // `#355 provider-grouped-channels`: with several services, all of the chosen service's channels,
    // "All Slack channels", offered on the same terms within that service.
    const all = settingsChannels().length >= 2 && upstreamClearAllChannels().length > 0;
    if (all) {
      const option = document.createElement("option");
      option.value = deps.allChannels;
      option.textContent = settingsServiceDescription() ? `All ${settingsServiceName()} channels` : "All channels";
      select.append(option);
    }
    const optionFor = (channel: ChannelInfo) => {
      const option = document.createElement("option");
      option.value = channel.id;
      option.textContent = deps.pickerName(deps.channelName(channel));
      return option;
    };
    for (const channel of settingsChannels()) select.append(optionFor(channel));
    const order = settingsChannels();
    const showing = all && chosen === deps.allChannels
      ? deps.allChannels
      : order.some((channel) => String(channel.id) === String(chosen))
        ? chosen
        : order.length > 0
          ? order[0]!.id
          : "";
    select.value = showing;
    // `#351 settings-polish`, `#402 who-is-who-channel-box`. Settings' picker redrawn onto another
    // channel than the one it showed (the box's channel removed from under it) moves who is who with it.
    if (String(select.value) !== String(chosen)) deps.identities().render();
  }

  return {
    settingsChannels,
    settingsServiceDescription,
    settingsServiceName,
    fillServiceSelect,
    chooseSettingsService,
    pointSettingsAt,
    pointSettingsAtReading,
    renderAddChannelForm,
    renderHiddenChannels,
    addChannel,
    removeChannel,
    renderChannelBox,
    allChannelsChosen,
    renderAliasEditor,
    upstreamClearOffered,
    upstreamClearAllChannels,
    showChannelPanel,
    saveAlias,
    clearChannelAlias,
    fillSettingsSelect,
    service: () => settingsService,
  };
}

export type ChannelSettings = ReturnType<typeof mountChannelSettings>;
