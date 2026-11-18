// Tests for synced composer drafts (`#1840 vibe-talk-draft-sync-across-devices`):
// the module built as the page build builds it, two fake devices over one fake
// server, an injected timer — no wall-clock anywhere.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const OUTBOX = await moduleForTests("src/core/outbox/index.ts");
const {
  createDraftSync,
  decideDraftOffer,
  draftAgeWords,
  DRAFT_AUTOSYNC_IDLE_MS,
} = OUTBOX;

/** One fake server: the drafts routes over a map, as the Rust store keeps them. */
function fakeServer() {
  const rows = new Map();
  const idOf = (channel, thread, scope) => `${channel}\n${thread}\n${scope}`;
  return {
    rows,
    transport: {
      async getDrafts(channel) {
        return {
          drafts: [...rows.values()].filter((row) => row.channel_id === channel),
        };
      },
      async putDraft(channel, body) {
        const key = idOf(channel, body.thread_id ?? "", body.scope);
        const previous = rows.get(key) ?? null;
        if (!body.text.trim()) {
          rows.delete(key);
          return { draft: null, replaced: previous };
        }
        const draft = {
          channel_id: channel,
          thread_id: body.thread_id ?? "",
          scope: body.scope,
          text: body.text,
          device_id: body.device_id,
          device_label: body.device_label,
          updated_ms: (previous?.updated_ms ?? 0) + 1,
          version: (previous?.version ?? 0) + 1,
        };
        rows.set(key, draft);
        return {
          draft,
          replaced: previous && previous.text !== body.text ? previous : null,
        };
      },
      async deleteDraft(channel, body) {
        rows.delete(idOf(channel, body.thread_id ?? "", body.scope));
        return {};
      },
    },
  };
}

/** A device: a sync with a hand-driven timer queue over the shared server. */
function fakeDevice(server, deviceId, label, autosync = true) {
  const timers = new Map();
  let nextHandle = 0;
  const saved = [];
  const sync = createDraftSync({
    transport: server.transport,
    deviceId: () => deviceId,
    deviceLabel: () => label,
    autosync: () => autosync,
    schedule(run, ms) {
      const handle = (nextHandle += 1);
      timers.set(handle, { run, ms });
      return handle;
    },
    cancel(handle) {
      timers.delete(handle);
    },
    onSaved(key, replaced) {
      saved.push({ key, replaced });
    },
  });
  return {
    sync,
    saved,
    timers,
    runTimers() {
      const due = [...timers.values()];
      timers.clear();
      for (const timer of due) timer.run();
    },
  };
}

const KEY = { channel: "c1", thread: "", scope: "main" };

test("#1840: a draft typed on device A is the continue offer on device B, and sending clears it", async () => {
  const server = fakeServer();
  const phone = fakeDevice(server, "device-a", "phone");
  const laptop = fakeDevice(server, "device-b", "laptop");
  // A types and sits idle; nothing syncs before the idle wait runs out.
  phone.sync.noteTyped(KEY, "a long half-written message");
  assert.equal(server.rows.size, 0);
  assert.equal([...phone.timers.values()][0].ms, DRAFT_AUTOSYNC_IDLE_MS);
  phone.runTimers();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(server.rows.size, 1);
  assert.deepEqual(phone.saved.length, 1);
  // B opens the channel: the words arrive as an offer beside its empty box.
  const drafts = await laptop.sync.fetchChannel("c1");
  assert.equal(drafts.length, 1);
  const decision = decideDraftOffer(drafts[0], "", "device-b");
  assert.equal(decision.kind, "adopt");
  assert.equal(decision.draft.text, "a long half-written message");
  assert.equal(decision.draft.device_label, "phone");
  // B sends: the server copy goes with the send.
  await laptop.sync.clear(KEY);
  assert.equal(server.rows.size, 0);
  assert.deepEqual(await laptop.sync.fetchChannel("c1"), []);
});

test("#1840: two devices editing never silently lose words", async () => {
  const server = fakeServer();
  const phone = fakeDevice(server, "device-a", "phone");
  await phone.sync.saveNow(KEY, "words from the phone");
  const remote = (await phone.sync.fetchChannel("c1"))[0];
  // The laptop holds different unsent words: an offer, never an overwrite.
  assert.equal(decideDraftOffer(remote, "my own half sentence", "device-b").kind, "offer");
  // Its own echo and identical words are nothing at all.
  assert.equal(decideDraftOffer(remote, "words from the phone", "device-b").kind, "none");
  assert.equal(decideDraftOffer(remote, "", "device-a").kind, "none");
  // The offer's age reads off the server stamp.
  assert.equal(draftAgeWords(0, 3 * 60000), "3 min ago");
});

test("#1840: auto-sync off still saves by button, and a failed sync retries", async () => {
  const server = fakeServer();
  const quiet = fakeDevice(server, "device-a", "phone", false);
  quiet.sync.noteTyped(KEY, "typed with auto-sync off");
  quiet.runTimers();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(server.rows.size, 0, "idle typing did not sync");
  await quiet.sync.saveNow(KEY, "typed with auto-sync off");
  assert.equal(server.rows.size, 1, "the button synced at once");
  // A refused write keeps the latest words for the next flush.
  const realPut = server.transport.putDraft;
  server.transport.putDraft = () => Promise.reject(new Error("offline"));
  await quiet.sync.saveNow(KEY, "newer words while offline");
  server.transport.putDraft = realPut;
  await quiet.sync.flushPending();
  const held = (await quiet.sync.fetchChannel("c1"))[0];
  assert.equal(held.text, "newer words while offline");
});
