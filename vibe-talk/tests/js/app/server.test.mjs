// Tests for web/src/app/server.ts on its own (`#624 voice-page-modularity`): the ports,
// storages and server are each made on first use, the on-use storages make nothing early,
// and a request carries the token. The page over the same factory is in ../page/.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const APP = await moduleForTests("src/app/index.ts");

function fakeStorage() {
  const held = new Map();
  return {
    getItem: (key) => held.get(key) ?? null,
    setItem: (key, value) => held.set(key, String(value)),
    removeItem: (key) => held.delete(key),
  };
}

function makeAccess() {
  const device = fakeStorage();
  const windowStore = fakeStorage();
  const asked = [];
  globalThis.localStorage = device;
  globalThis.sessionStorage = windowStore;
  const access = APP.createServerAccess({
    token: () => "tok-123",
    readRefused: () => asked.push("refused"),
    noteAsked: () => asked.push("noteAsked"),
    chatServiceOf: () => "Slack",
    selectedChannelId: () => "c1",
  });
  return { access, device, windowStore, asked };
}

test("the ports, settings and server are made once, on first use", () => {
  const { access, device, windowStore } = makeAccess();
  const onUse = access.deviceStorageOnUse();
  onUse.setItem("k", "v");
  assert.equal(device.getItem("k"), "v", "the on-use storage made no port until used, then the device's");
  assert.equal(access.browserPorts(), access.browserPorts(), "the ports are one instance");
  assert.equal(access.deviceSettings(), access.deviceSettings(), "the settings are one instance");
  assert.equal(access.theServer(), access.theServer(), "the server is one instance");
  access.windowStorage().setItem("w", "1");
  assert.equal(windowStore.getItem("w"), "1");
  assert.equal(device.getItem("w"), null, "the window's storage is not the device's");
});

test("a request carries the token and hands back the answer", async () => {
  const { access } = makeAccess();
  const seen = [];
  globalThis.fetch = async (path, init) => {
    seen.push([path, init.headers.Authorization]);
    return {
      ok: true, status: 200,
      headers: { get: () => null },
      text: async () => JSON.stringify({ answer: 42 }),
    };
  };
  const answer = await access.api("/api/v1/thing");
  assert.deepEqual(answer, { answer: 42 });
  assert.deepEqual(seen, [["/api/v1/thing", "Bearer tok-123"]]);
  let ran = false;
  access.clockOnUse().setTimeout(() => { ran = true; }, 0);
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(ran, true, "the clock on use runs on the browser's timers");
});
