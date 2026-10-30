// Tests for web/src/core/settings/ on its own: the module built as the page build builds it and
// held directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over
// the same module is tested in ../voice_page.test.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The device's settings and, on its own handle, its token (web/src/core/settings). */
const SETTINGS = await moduleForTests("src/core/settings/index.ts");

test("`#624 voice-page-modularity`: the token is kept, read and forgotten on its own handle, and a refusing device says so the way the page relies on", () => {
  const kept = new Map();
  const storage = { getItem: (key) => kept.get(key) ?? null, setItem: (key, value) => kept.set(key, value), removeItem: (key) => kept.delete(key) };
  const credentials = SETTINGS.createCredentials(storage);
  assert.equal(credentials.token(), "", "no token reads as none");
  assert.equal(credentials.signedOutByRename(), false);
  kept.set("gent-talk.token", "old-token-aaaaaaaaaaaa");
  assert.equal(credentials.signedOutByRename(), true, "a token under the old name is a sign-out by the rename");
  credentials.keep("write-token-aaaaaaaaaaaaaaaa");
  assert.deepStrictEqual([credentials.token(), kept.get("vibe-talk.token"), credentials.signedOutByRename()],
    ["write-token-aaaaaaaaaaaaaaaa", "write-token-aaaaaaaaaaaaaaaa", false]);
  credentials.forget();
  assert.deepStrictEqual([credentials.token(), kept.has("vibe-talk.token")], ["", false]);
  // Storage turned off: reading the token throws, as the page's callers expect; the sign-in path's
  // question does not.
  const refused = () => { throw new Error("SecurityError"); };
  const off = SETTINGS.createCredentials({ getItem: refused, setItem: refused, removeItem: refused });
  assert.throws(() => off.token(), /SecurityError/);
  assert.throws(() => off.keep("x"), /SecurityError/);
  assert.equal(off.signedOutByRename(), false);
});

test("`#624 voice-page-modularity`: the settings sync runs on its own ports: the first read, the seed, a local change pushed, a refusal kept local", async () => {
  const KEY = "vibe-talk.voice.read-mode";
  const kept = new Map([[KEY, "hide"], ["vibe-talk.voice.combine", "1"]]);
  const storage = { getItem: (k) => kept.get(k) ?? null, setItem: (k, v) => kept.set(k, v), removeItem: (k) => kept.delete(k) };
  const asked = [];
  const timers = [];
  let held = [{ key: KEY, value: "show", updated_ms: 7 }];
  let refuse = null;
  const applied = [];
  const statuses = [];
  const sync = SETTINGS.createSettingsSync({
    storage,
    clock: { setTimeout: (run, ms) => timers.push({ run, ms }), clearTimeout: () => {} },
    decoded: async (type, path, options) => {
      asked.push({ type, path, method: options?.method ?? "GET", body: options?.body });
      if (refuse) throw refuse;
      if (options?.method === "PUT") {
        for (const entry of [...options.body.seed, ...options.body.set]) held = held.filter((h) => h.key !== entry.key).concat([{ ...entry, updated_ms: 9 }]);
      }
      return { settings: held };
    },
    token: () => "tok",
    tokenScope: () => "write",
    apply: (key) => applied.push([key, kept.get(key) ?? null]),
    status: (text) => statuses.push(text),
    landed: () => {},
  });
  // Nothing goes out before the server is said to keep settings, nor before the token is proven.
  await sync.syncOnce();
  assert.equal(asked.length, 0, "asked before either");
  sync.setSupported(true);
  await sync.syncOnce();
  assert.equal(asked.length, 0, "asked with an unproven token");
  sync.forget();
  assert.equal(sync.start(), true, "the first start is the caller's to make the first pass of");
  await sync.syncOnce();
  assert.equal(asked.length, 0, "asked of a server not said to keep settings");
  sync.setSupported(true);
  await sync.syncOnce();
  // The server's held value wins over one that never moved; the one it does not hold is seeded.
  assert.deepStrictEqual(asked.map((a) => a.method), ["GET", "PUT"]);
  assert.deepStrictEqual(asked[1].body, { set: [], seed: [{ key: "vibe-talk.voice.combine", value: "1", updated_ms: 0 }] });
  assert.deepStrictEqual(applied, [[KEY, "show"]]);
  assert.deepStrictEqual(sync.agreed(KEY), { value: "show", updatedMs: 7 });
  // A change here is pushed on the next pass.
  kept.set(KEY, "hide");
  await sync.syncOnce();
  assert.deepStrictEqual(asked[2].body, { set: [{ key: KEY, value: "hide", updated_ms: 0 }], seed: [] });
  // The timer reschedules itself after each pass, on the clock port: armed once, and again only
  // once the pass it ran has ended.
  sync.keepPassing();
  assert.deepStrictEqual(timers.map((t) => t.ms), [1750]);
  timers[0].run();
  assert.equal(timers.length, 1, "the next pass was armed before this one ended");
  for (let i = 0; i < 5 && timers.length === 1; i += 1) await Promise.resolve();
  assert.deepStrictEqual(timers.map((t) => t.ms), [1750, 1750], "the timer did not re-arm after its pass");
  // A permanent refusal keeps everything local and stops asking.
  refuse = Object.assign(new Error("HTTP 404"), { status: 404 });
  kept.set(KEY, "show");
  await sync.syncOnce();
  assert.equal(statuses.at(-1), "This server does not keep settings, so yours stay on this device.");
  const before = asked.length;
  await sync.syncOnce();
  assert.equal(asked.length, before, "a server that keeps no settings was asked again");
  assert.equal(kept.get(KEY), "show", "a refusal cost this device its value");
});

test("`#624 voice-page-modularity`: each device setting reads, keeps and refuses as its own reader and writer always did", () => {
  const { DEVICE_SETTINGS, createDeviceSettings } = SETTINGS;
  const THROWN = Symbol("thrown");
  // [name, key, nothing kept, [[a value, what it keeps (null: forgotten)], ...], a refused read].
  const onOff = (name, key, absent, refused) => [name, key, absent, [[true, "1"], [false, "0"]], refused];
  const table = [
    onOff("inAppLinks", "vibe-talk.voice.in-app-links", true, THROWN),
    onOff("summarizeCollapsed", "vibe-talk.voice.summarize-collapsed", false, false),
    ["linksRaw", "vibe-talk.voice.link-raw-urls", false, [[true, "on"], [false, null]], false],
    onOff("composerGrow", "vibe-talk.voice.composer-grow", false, false),
    onOff("enterSends", "vibe-talk.voice.enter-sends", true, THROWN),
    onOff("singleKeys", "vibe-talk.voice.single-keys", true, true),
    onOff("richHeaders", "vibe-talk.voice.rich-headers", true, true),
    onOff("markOwnRead", "vibe-talk.voice.mark-own-read", true, THROWN),
    onOff("hideEmptyMessages", "vibe-talk.voice.hide-empty", true, THROWN),
    onOff("combineMessages", "vibe-talk.voice.combine", true, THROWN),
    ["openInView", "vibe-talk.voice.open-in-view", "flat", [["main", "main"], ["flat", "flat"]], THROWN],
    ["readAudioSource", "vibe-talk.voice.read-audio-source", null, [["agent", "agent"], ["device", "device"], ["wit", "wit"]], null],
    ["witVoice", "vibe-talk.voice.wit-voice", "", [["Charlie", "Charlie"], ["", null]], ""],
    ["readSteerOffset", "vibe-talk.voice.read-steer-offset", null, [[{ x: 3, y: -4 }, '{"x":3,"y":-4}'], [null, null]], null],
    ["slackOpen", "vibe-talk.voice.slack-open", "browser", [["app", "app"], ["browser", "browser"]], "browser"],
    ["threadsMain", "vibe-talk.voice.threads-main", false, [[true, "on"], [false, "off"]], false],
    ["threadCardFolded", "vibe-talk.voice.thread-card-folded", false, [[true, "on"], [false, "off"]], false],
    ["keyboardDebug", "vibe-talk.voice.keyboard-debug", false, [[true, "1"], [false, null]], false],
    ["msgScale", "vibe-talk.voice.msg-scale", 100, [[120, "120"], [80, "80"]], THROWN],
    ["readingWidth", "vibe-talk.voice.width", 72, [[90, "90"], [45, "45"]], THROWN],
    ["collapsedLines", "vibe-talk.voice.collapsed-lines", 3, [[0, "0"], [8, "8"]], THROWN],
    ["cardMaxLines", "vibe-talk.voice.thread-card-max-lines", 5, [[1, "1"], [12, "12"]], 5],
    ["readSpeed", "vibe-talk.voice.read-speed", null, [[150, "150"], [null, null]], THROWN],
  ];
  const refused = () => { throw new Error("SecurityError"); };
  const off = createDeviceSettings({ getItem: refused, setItem: refused, removeItem: refused });
  for (const [name, key, absent, values, whenRefused] of table) {
    const kept = new Map();
    const settings = createDeviceSettings({ getItem: (k) => kept.get(k) ?? null, setItem: (k, v) => kept.set(k, v), removeItem: (k) => kept.delete(k) });
    const setting = DEVICE_SETTINGS[name];
    assert.equal(setting.key, key, name);
    assert.deepStrictEqual(settings.get(setting), absent, `${name}: nothing kept`);
    for (const [value, keeps] of values) {
      settings.keep(setting, value);
      assert.deepStrictEqual([kept.get(key) ?? null, settings.get(setting)], [keeps, value], `${name}: ${JSON.stringify(value)}`);
      assert.equal(settings.keptAs(setting, value), true, `${name}: the read-back of ${JSON.stringify(value)}`);
    }
    if (whenRefused === THROWN) assert.throws(() => off.get(setting), /SecurityError/, `${name}: a refusal was swallowed`);
    else assert.deepStrictEqual(off.get(setting), whenRefused, `${name}: refused`);
    assert.throws(() => off.keep(setting, values[0][0]), /SecurityError/, `${name}: a refused write was swallowed`);
  }
  // What a reader made of a value it does not know, as before: the default, never the stray text.
  const stray = createDeviceSettings({ getItem: () => "{nonsense", setItem: () => {}, removeItem: () => {} });
  assert.deepStrictEqual([stray.get(DEVICE_SETTINGS.readAudioSource), stray.get(DEVICE_SETTINGS.openInView),
    stray.get(DEVICE_SETTINGS.slackOpen), stray.get(DEVICE_SETTINGS.readSteerOffset)], [null, "flat", "browser", null]);
  // ...and a number out of its range, or not a number at all, as the clamps always read it: held to
  // the range; the default, or for the pace no preference; a blank the default, never zero.
  const keptText = (text) => createDeviceSettings({ getItem: () => text, setItem: () => {}, removeItem: () => {} });
  assert.deepStrictEqual([keptText("4000").get(DEVICE_SETTINGS.readingWidth), keptText("1").get(DEVICE_SETTINGS.msgScale),
    keptText("99").get(DEVICE_SETTINGS.collapsedLines), keptText("0").get(DEVICE_SETTINGS.cardMaxLines),
    keptText("999").get(DEVICE_SETTINGS.readSpeed)], [120, 80, 8, 1, 200]);
  assert.deepStrictEqual([stray.get(DEVICE_SETTINGS.readingWidth), stray.get(DEVICE_SETTINGS.msgScale),
    stray.get(DEVICE_SETTINGS.collapsedLines), stray.get(DEVICE_SETTINGS.cardMaxLines), stray.get(DEVICE_SETTINGS.readSpeed)],
  [72, 100, 3, 5, null]);
  assert.deepStrictEqual([keptText("  ").get(DEVICE_SETTINGS.collapsedLines), keptText("").get(DEVICE_SETTINGS.readingWidth)], [3, 72]);
  assert.deepStrictEqual(Object.keys(DEVICE_SETTINGS).sort(), table.map(([name]) => name).sort(), "a setting with no row here");
});
