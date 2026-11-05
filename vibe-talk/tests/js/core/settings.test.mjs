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
    onOff("readerHidden", "vibe-talk.voice.reader-hidden", false, false),
    onOff("autoplay", "vibe-talk.voice.autoplay", false, false),
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
    ["quoteReadPercent", "vibe-talk.voice.quote-read-percent", 5, [[0, "0"], [100, "100"]], 5],
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

test("`#728 vt-quote-read-threshold`: the share a quote must cover is whole percentages from 0 to 100, 5 for anything unusable, kept synced", () => {
  const { clampQuoteReadPercent, DEFAULT_QUOTE_READ_PERCENT, DEVICE_SETTINGS, SYNCED_SETTING_KEYS } = SETTINGS;
  assert.equal(DEFAULT_QUOTE_READ_PERCENT, 5);
  for (const [given, want] of [[null, 5], ["", 5], ["x", 5], [" 12 ", 12], [2.4, 2], [-3, 0], [250, 100], [0, 0], ["100", 100]]) {
    assert.equal(clampQuoteReadPercent(given), want, `${JSON.stringify(given)}`);
  }
  const setting = DEVICE_SETTINGS.quoteReadPercent;
  assert.equal(setting.read(null), 5);
  assert.equal(setting.read("40"), 40);
  assert.equal(setting.write(7), "7");
  assert.equal(setting.refused.value, 5);
  assert.ok(SYNCED_SETTING_KEYS.includes(setting.key));
});

test("`#624 voice-page-modularity`: the message limits in force start at their defaults, apply clamped, draw what they apply, and keep only through persist", () => {
  const { createCardMaxLines, createCollapsedLines, createDeviceSettings, createQuoteReadPercent } = SETTINGS;
  const kept = new Map();
  const device = createDeviceSettings({ getItem: (k) => kept.get(k) ?? null, setItem: (k, v) => kept.set(k, v), removeItem: (k) => kept.delete(k) });
  const refused = () => { throw new Error("SecurityError"); };
  const off = createDeviceSettings({ getItem: refused, setItem: refused, removeItem: refused });
  // A browser that accepts a write and keeps nothing: only the read-back can tell.
  const forgetful = createDeviceSettings({ getItem: () => null, setItem: () => {}, removeItem: () => {} });
  const drawn = [];

  // `#425 collapsed-lines`.
  const lines = createCollapsedLines({ deviceSettings: () => device, render: (n) => drawn.push(["lines", n]) });
  assert.deepStrictEqual([lines.current(), lines.stored()], [3, 3], "the default, and nothing kept reads as it");
  assert.deepStrictEqual(drawn, [], "nothing is drawn until the page applies a count");
  assert.equal(lines.apply("99"), 8, "the count applied is the clamped one");
  assert.deepStrictEqual([lines.current(), drawn, kept.size], [8, [["lines", 8]], 0], "applying draws the count and keeps nothing");
  assert.equal(lines.persist(8), true);
  assert.deepStrictEqual([kept.get("vibe-talk.voice.collapsed-lines"), lines.stored()], ["8", 8]);
  assert.equal(createCollapsedLines({ deviceSettings: () => forgetful, render: () => {} }).persist(5), false, "a write kept nowhere was taken as kept");
  assert.equal(createCollapsedLines({ deviceSettings: () => off, render: () => {} }).persist(5), false, "a refused write was taken as kept");

  // `#522 thread-card-max-lines`: K, and the cards drawn again only when it changes.
  const redrawn = [];
  const cards = createCardMaxLines({ deviceSettings: () => device, render: (n) => drawn.push(["cards", n]), redraw: () => redrawn.push(cards.current()) });
  assert.equal(cards.current(), 5);
  assert.equal(cards.apply(5), 5);
  assert.deepStrictEqual(redrawn, [], "the K already in force drew the cards again");
  assert.equal(cards.apply(0), 1, "K applied is the clamped one");
  assert.deepStrictEqual(redrawn, [1], "a changed K draws the cards again, once it is in force");
  assert.equal(cards.apply(7, false), 7);
  assert.deepStrictEqual([cards.current(), redrawn], [7, [1]], "at load K is put on and nothing is drawn again");
  assert.deepStrictEqual(drawn.filter(([what]) => what === "cards").map(([, n]) => n), [5, 1, 7], "every K applied is drawn");
  assert.equal(kept.has("vibe-talk.voice.thread-card-max-lines"), false, "applying K kept it");
  assert.equal(cards.persist(7), true);
  assert.deepStrictEqual([kept.get("vibe-talk.voice.thread-card-max-lines"), cards.stored()], ["7", 7]);
  assert.equal(createCardMaxLines({ deviceSettings: () => off, render: () => {}, redraw: () => {} }).stored(), 5, "a refused read is the default");
  assert.equal(createCardMaxLines({ deviceSettings: () => off, render: () => {}, redraw: () => {} }).persist(5), false);

  // `#728 vt-quote-read-threshold`.
  const share = createQuoteReadPercent({ deviceSettings: () => device, render: (n) => drawn.push(["share", n]) });
  assert.deepStrictEqual([share.current(), share.stored()], [5, 5]);
  assert.equal(share.apply(250), 100, "the share applied is the clamped one");
  assert.equal(share.apply("x"), 5, "a share that is not a number is the default");
  assert.deepStrictEqual([share.current(), drawn.filter(([what]) => what === "share").map(([, n]) => n)], [5, [100, 5]]);
  assert.equal(kept.has("vibe-talk.voice.quote-read-percent"), false, "applying the share kept it: keeping is Settings'");
  kept.set("vibe-talk.voice.quote-read-percent", "40");
  assert.equal(share.stored(), 40);
  assert.equal(createQuoteReadPercent({ deviceSettings: () => off, render: () => {} }).stored(), 5, "a refused read is the default");

  // Two owners hold two values.
  const other = createCollapsedLines({ deviceSettings: () => device, render: () => {} });
  assert.deepStrictEqual([other.current(), lines.current()], [3, 8], "a second owner shares the first one's count");
});

test("`#624 voice-page-modularity`: Message links, Enter to send and the single-key shortcuts are on until turned off, kept as they are turned, and honoured where storage is refused", () => {
  const { createDeviceSettings, createEnterSends, createInAppLinks, createSingleKeys } = SETTINGS;
  const kept = new Map();
  const device = createDeviceSettings({ getItem: (k) => kept.get(k) ?? null, setItem: (k, v) => kept.set(k, v), removeItem: (k) => kept.delete(k) });
  const refused = () => { throw new Error("SecurityError"); };
  const off = createDeviceSettings({ getItem: refused, setItem: refused, removeItem: refused });
  const said = [];

  // `#543 in-app-message-links`: drawn, then kept.
  const LINKS = "vibe-talk.voice.in-app-links";
  const links = createInAppLinks({ deviceSettings: () => device, render: (on) => said.push(["links", on, kept.get(LINKS) ?? null]) });
  assert.deepStrictEqual([links.current(), links.stored(), said], [true, true, []], "absent means on, and nothing is drawn yet");
  links.apply(0);
  assert.deepStrictEqual([links.current(), said, kept.get(LINKS), links.stored()], [false, [["links", false, null]], "0", false],
    "the switch is drawn before it is kept, and kept as turned");

  // `#225 enter-to-send`: kept, then said.
  const ENTER = "vibe-talk.voice.enter-sends";
  const enter = createEnterSends({ deviceSettings: () => device, render: () => said.push(["enter", enter.current(), kept.get(ENTER) ?? null]) });
  assert.equal(enter.current(), true);
  enter.apply("");
  assert.deepStrictEqual([said.at(-1), enter.stored()], [["enter", false, "0"], false], "Enter to send is kept, then said");
  enter.apply(true);
  assert.deepStrictEqual([said.at(-1), enter.stored()], [["enter", true, "1"], true]);

  // `#224 keyboard-shortcuts`: kept, then the keys said; restored at load without either.
  const KEYS = "vibe-talk.voice.single-keys";
  const keys = createSingleKeys({ deviceSettings: () => device, render: () => said.push(["keys", keys.current(), kept.get(KEYS) ?? null]) });
  keys.apply(false);
  assert.deepStrictEqual([said.at(-1), keys.stored()], [["keys", false, "0"], false], "the switch is kept, then the keys said");
  const fresh = createSingleKeys({ deviceSettings: () => device, render: () => said.push(["restored"]) });
  assert.equal(fresh.current(), true, "on until restored");
  const before = said.length;
  fresh.restore();
  assert.deepStrictEqual([fresh.current(), said.length, kept.get(KEYS)], [false, before, "0"], "restoring takes what was kept, and neither says nor keeps it");

  // Storage refused: each choice still holds for the session.
  const offLinks = createInAppLinks({ deviceSettings: () => off, render: () => {} });
  const offEnter = createEnterSends({ deviceSettings: () => off, render: () => {} });
  const offKeys = createSingleKeys({ deviceSettings: () => off, render: () => {} });
  offLinks.apply(false);
  offEnter.apply(false);
  offKeys.apply(false);
  assert.deepStrictEqual([offLinks.current(), offEnter.current(), offKeys.current()], [false, false, false],
    "a browser that refuses storage did not honour the choice for the session");
  offKeys.restore();
  assert.equal(offKeys.current(), true, "a refused read of the single keys is on");
  assert.throws(() => offLinks.stored(), /SecurityError/, "a refused read of Message links was swallowed, which its reader never did");
  assert.throws(() => offEnter.stored(), /SecurityError/, "a refused read of Enter to send was swallowed, which its reader never did");
});

test("`#624 voice-page-modularity`: a switch kept on the device is on at first or as kept, drawn before it is kept, and holds for the session where storage is refused", () => {
  const { createKeptSwitch, createDeviceSettings, DEVICE_SETTINGS } = SETTINGS;
  const kept = new Map();
  const device = createDeviceSettings({ getItem: (k) => kept.get(k) ?? null, setItem: (k, v) => kept.set(k, v), removeItem: (k) => kept.delete(k) });
  const seen = [];
  const combine = createKeptSwitch({ setting: DEVICE_SETTINGS.combineMessages, deviceSettings: () => device, initial: true,
    render: (on) => seen.push([on, kept.get("vibe-talk.voice.combine") ?? null]) });
  assert.equal(combine.on(), true);
  assert.deepStrictEqual([seen, kept.size], [[], 0], "made, it drew and kept nothing");
  assert.equal(combine.stored(), true, "absent means on");
  combine.apply(0);
  assert.equal(combine.on(), false);
  assert.deepStrictEqual(seen, [[false, null]], "it was not drawn before it was kept");
  assert.deepStrictEqual([kept.get("vibe-talk.voice.combine"), combine.stored()], ["0", false]);
  combine.apply("yes");
  assert.deepStrictEqual([combine.on(), kept.get("vibe-talk.voice.combine")], [true, "1"]);
  // With no first value: what the device keeps, read once as it is made. Set and keep are apart.
  kept.set("vibe-talk.voice.threads-main", "on");
  let reads = 0;
  const main = createKeptSwitch({ setting: DEVICE_SETTINGS.threadsMain, deviceSettings: () => { reads += 1; return device; } });
  assert.deepStrictEqual([main.on(), reads], [true, 1]);
  main.set(false);
  assert.deepStrictEqual([main.on(), kept.get("vibe-talk.voice.threads-main")], [false, "on"], "set kept something");
  main.keep(false);
  assert.equal(kept.get("vibe-talk.voice.threads-main"), "off");
  // A refusing device: the choice holds for the session, and the stored read is the setting's to answer.
  const refused = () => { throw new Error("SecurityError"); };
  const off = createDeviceSettings({ getItem: refused, setItem: refused, removeItem: refused });
  const own = createKeptSwitch({ setting: DEVICE_SETTINGS.markOwnRead, deviceSettings: () => off, initial: true, render: () => {} });
  own.apply(false);
  assert.equal(own.on(), false, "a refused keep undid the choice");
  assert.throws(() => own.stored(), /SecurityError/, "a setting with no refused value lets the refusal be thrown, as before");
  const fold = createKeptSwitch({ setting: DEVICE_SETTINGS.threadCardFolded, deviceSettings: () => off });
  assert.equal(fold.on(), false, "a refused read of the fold is open");
  fold.keep(true);
  assert.equal(fold.on(), false, "keeping it changed it");
});
