// Tests for web/src/view/shell/channeldirectory.ts on its own (`#624 voice-page-modularity`):
// what a channel is called, the grouping by service, the remembered channel, and taking the
// server's channel list in. The page's behaviour over the same factory is in ../page/.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const SHELL = await moduleForTests("src/view/shell/index.ts");

function fakeElement(tag) {
  return {
    tag, value: "", textContent: "", children: [],
    setAttribute(name, value) { this[name] = value; },
    append(...kids) { this.children.push(...kids); },
    replaceChildren() { this.children = []; },
  };
}
globalThis.document = { createElement: (tag) => fakeElement(tag) };

function makeView() {
  const calls = [];
  const state = {
    channels: [
      { id: "1", label: "#general", provider: "slack" },
      { id: "2", label: "random", alias: "  the lounge ", provider: "slack" },
      { id: "3", label: "dev", provider: "discord" },
      { id: "4", label: "orphan", provider: "gone" },
    ],
    providers: new Map([
      ["slack", { name: "Slack" }],
      ["discord", { name: "Slack" }],
    ]),
    selected: "2",
    windowItems: new Map(),
    device: new Map([["active-key", "3"]]),
    select: fakeElement("select"),
  };
  const directory = {
    providerName: () => "",
    learnProviders: (providers) => { calls.push(["learnProviders", providers]); return providers; },
    providerOf: (id) => state.providers.get(state.channels.find((c) => c.id === id)?.provider) ?? null,
    providers: () => state.providers,
    channels: () => state.channels,
    known: (id) => state.channels.find((c) => c.id === id) ?? null,
    capabilities: () => ({}),
    setChannels: (channels) => { state.channels = channels; },
    setHidden: (channels) => { calls.push(["setHidden", channels.length]); },
  };
  const view = SHELL.createChannelDirectoryView({
    directory: () => directory,
    activeChannelKey: "active-key",
    el: () => state.select,
    selectedChannelId: () => state.selected,
    selectionOwner: () => ({ selectChannel: (id) => { calls.push(["selectChannel", id]); state.selected = id; } }),
    renderLiveState: () => calls.push("renderLiveState"),
    keepWindowItem: (key, value) => { if (value === null) state.windowItems.delete(key); else state.windowItems.set(key, value); },
    windowItem: (key) => state.windowItems.get(key) ?? null,
    deviceStorage: () => ({
      getItem: (key) => state.device.get(key) ?? null,
      setItem: (key, value) => state.device.set(key, value),
      removeItem: (key) => state.device.delete(key),
    }),
    fromProvider: () => calls.push("fromProvider"),
    saveCacheChannels: () => calls.push("saveCacheChannels"),
    learnProviderAccounts: () => calls.push("learnProviderAccounts"),
    boxes: () => ({
      fillServiceSelect: () => calls.push("fillServiceSelect"),
      fillSettingsSelect: () => calls.push("fillSettingsSelect"),
      renderHiddenChannels: () => calls.push("renderHiddenChannels"),
    }),
  });
  return { view, state, calls };
}

test("a channel is called by its alias, trimmed, else its label or id", () => {
  const { view } = makeView();
  assert.equal(view.channelName({ id: "9", label: "builds", alias: "  ci " }), "ci");
  assert.equal(view.channelName({ id: "9", label: "builds", alias: "   " }), "builds");
  assert.equal(view.channelName({ id: "9" }), "9");
  assert.equal(view.channelName(null), "this channel");
  assert.equal(view.pickerName("#general"), "general");
  assert.equal(view.pickerName("##  spaced"), "spaced");
  assert.equal(view.chatServiceName(), "the chat service");
});

test("channels group by service in server order, the undescribed last", () => {
  const { view } = makeView();
  // Two services share the name Slack, so each heading carries its key.
  assert.equal(view.serviceLabel("slack"), "Slack (slack)");
  const groups = view.channelGroups();
  assert.deepEqual(groups.map((g) => [g.heading, g.channels.map((c) => c.id)]), [
    ["Slack (slack)", ["1", "2"]],
    ["Slack (discord)", ["3"]],
    ["Not connected", ["4"]],
  ]);
  assert.equal(view.channelsGrouped(), true);
  assert.deepEqual(view.channelsInPickerOrder().map((c) => c.id), ["1", "2", "3", "4"]);
  assert.equal(view.knownChannel("3")?.label, "dev");
  assert.equal(view.knownChannel("nope"), null);
});

test("the remembered channel is this window's first, the device's second", () => {
  const { view, state } = makeView();
  assert.equal(view.storedActiveChannel(), "3");
  view.rememberActiveChannel();
  assert.equal(view.storedActiveChannel(), "2");
  assert.equal(state.device.get("active-key"), "3");
  view.rememberActiveChannel(true);
  assert.equal(state.device.get("active-key"), "2");
});

test("adopting a list stores it, redraws both pickers, and keeps the showing channel", () => {
  const { view, state, calls } = makeView();
  view.adoptChannelList({ channels: state.channels, hidden_channels: [{ id: "8" }] });
  assert.deepEqual(calls, [
    ["setHidden", 1], "saveCacheChannels", "fillServiceSelect",
    ["selectChannel", "2"], "fillSettingsSelect", "renderHiddenChannels",
  ]);
  // Grouped: three optgroups, the showing channel's name without its '#'.
  assert.deepEqual(state.select.children.map((g) => g.label), ["Slack (slack)", "Slack (discord)", "Not connected"]);
  assert.equal(state.select.children[0].children[0].textContent, "general");
  assert.equal(state.select.children[0].children[1].value, "2");
});
