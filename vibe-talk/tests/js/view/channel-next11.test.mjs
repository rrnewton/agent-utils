// Opening and closing the search bar, the words for a slow service, a channel's picker
// name and the seam at its head: their owners, over fakes (`#624 voice-page-modularity`).

import assert from "node:assert/strict";
import { test } from "node:test";
import { createChannelHead } from "../../../web/src/view/channel/channelhead.ts";
import { createSearchOpenControl } from "../../../web/src/view/channel/searchopen.ts";
import { createServiceWords } from "../../../web/src/view/channel/servicewords.ts";

test("closing the search bar clears its filters inside the kept place", () => {
  const record = [];
  const elements = {
    "search-field": { value: "x", focus() { record.push("focus"); } },
    "search-toggle": { setAttribute(name, value) { record.push(`toggle ${name}=${value}`); } },
    "screen-main": {
      setAttribute(name) { record.push(`main+${name}`); },
      removeAttribute(name) { record.push(`main-${name}`); },
    },
  };
  const owner = createSearchOpenControl({
    keepingFilterPlace: (act) => { record.push("keep"); act(); },
    searchField: {
      setOpen: (open) => record.push(`open=${open}`),
      setQuery: (query) => record.push(`query=${JSON.stringify(query)}`),
      setMarked: (marked) => record.push(`marked=${JSON.stringify(marked)}`),
    },
    markSearchSoon: () => record.push("mark"),
    el: (id) => elements[id],
    setLinksOnly: (on) => record.push(`links=${on}`),
    setPinnedOnly: (on) => record.push(`pinned=${on}`),
    setAuthorFilter: (author) => record.push(`author=${author}`),
    hidingRead: () => false,
    holdingReader: (act) => { record.push("hold"); act(); },
    renderControlBar: () => record.push("bar"),
    renderChannelFreshness: () => record.push("pill"),
    eachColumn: (act) => act(),
    renderCachedTimeline: () => record.push("timeline"),
    renderScrollTools: () => record.push("tools"),
    sideColumnUp: () => false,
    refilterSideCards: () => record.push("side"),
  });
  owner.setSearchOpen(false);
  assert.equal(
    record.join(" "),
    "keep open=false query=\"\" marked=\"\" mark links=false pinned=false author=everyone toggle aria-pressed=false hold main-data-searching bar pill tools",
  );
  assert.equal(elements["search-field"].value, "");
});

test("a channel's service is its provider's name, else the page's", () => {
  const words = createServiceWords({
    readableFailure: (error, service) => `${service}: ${error.message}`,
    providerOfChannel: (id) => (id === "a" ? { name: "Slack" } : null),
    selectedChannelId: () => "b",
    chatServiceName: () => "the chat service",
  });
  assert.equal(words.chatServiceOf("a"), "Slack");
  assert.equal(words.readableChannelFailure({ message: "slow" }), "the chat service: slow");
});

test("a channel's head names it and restates its seam", () => {
  const seams = [];
  const head = createChannelHead({
    knownChannel: (id) => (id === "1" ? { name: "general" } : null),
    channelName: (channel) => channel.name,
    el: (id) => ({ children: id === "channel-summary" ? [1] : [1, 2] }),
    readModes: () => ({ hiding: () => false }),
    renderChannelSeam: (summary) => seams.push(summary),
    todoSummary: () => "to do",
    channelSummary: (loaded, whole, name) => `${loaded}:${whole}:${name}`,
    olderPager: () => ({ loadedIsWhole: (more) => !more }),
    olderPages: () => ({ moreAbove: false }),
    selectedChannelId: () => "1",
  });
  assert.equal(head.channelLabel("1"), "general");
  assert.equal(head.channelLabel("9"), "9");
  head.restateChannelSeam();
  assert.deepEqual(seams, ["2:true:general"]);
});
