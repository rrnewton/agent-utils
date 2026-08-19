// Tests for the /voice page's feedback, run against the REAL web/voice.js.
//
// Why this exists at all: the page's job in a failure is to SAY what went wrong, and the bug it
// was written for was that it said nothing visible — a 502 naming a missing ElevenLabs permission
// reached only the browser console. A test that greps voice.js for a string would pass on a page
// that still shows nothing. So this executes the script.
//
// The DOM here is deliberately small and deliberately strict:
//
//   * `getElementById` knows ONLY the ids that really appear in web/voice.html, and THROWS for
//     anything else. So a script that reaches for an element the page does not have fails loudly
//     here instead of silently doing nothing in a phone browser at the roadside.
//   * There is no innerHTML, so a test cannot accidentally certify markup injection as "rendered".
//
// No dependencies, no build step, no framework — same rule the page itself follows.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const WEB = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "web");
const SCRIPT = readFileSync(join(WEB, "voice.js"), "utf8");
const HTML = readFileSync(join(WEB, "voice.html"), "utf8");

/**
 * Every id the served page defines, with the label and the initial `hidden` / `checked` state the
 * MARKUP gives it. Taking these from web/voice.html rather than restating them keeps the fixture
 * honest: if the page stops shipping the error panel, or ships it visible, or ships a microphone
 * toggle off, the tests below change answer.
 */
const PAGE_ELEMENTS = new Map(
  [...HTML.matchAll(/<(\w+)([^>]*\bid="([^"]+)"[^>]*)>([^<]*)/g)].map((m) => [
    m[3],
    {
      hidden: /\bhidden\b/.test(m[2]),
      checked: /\bchecked\b/.test(m[2]),
      text: m[4].trim(),
    },
  ])
);
const PAGE_IDS = new Set(PAGE_ELEMENTS.keys());

/** The vendor's real refusal, quoted from the live 502 the owner hit. */
const CONVAI_WRITE =
  "The API key you used is missing the permission convai_write to execute this operation.";

class FakeElement {
  constructor(id) {
    this.id = id;
    this.textContent = "";
    this.value = "";
    this.className = "";
    this.hidden = false;
    this.checked = false;
    this.disabled = false;
    this.children = [];
    this.listeners = new Map();
  }

  addEventListener(type, fn) {
    const existing = this.listeners.get(type) || [];
    existing.push(fn);
    this.listeners.set(type, existing);
  }

  /** Fire the listeners the page really registered for `type`. No synthetic event object. */
  async dispatch(type) {
    for (const fn of this.listeners.get(type) || []) {
      await fn();
    }
  }

  async click() {
    await this.dispatch("click");
  }

  /** Flip a checkbox the way a finger does: set the property, then fire `change`. */
  async setChecked(value) {
    this.checked = value;
    await this.dispatch("change");
  }

  append(...kids) {
    this.children.push(...kids);
  }

  scrollIntoView() {}
}

/**
 * The audio graph `start()` and `startCapture()` build. Only the pieces the page really touches
 * exist here, so a page that starts reaching for something else fails loudly rather than quietly
 * getting `undefined`.
 */
class FakeAudioContext {
  constructor() {
    this.sampleRate = 48000;
    this.currentTime = 0;
    this.destination = { id: "destination" };
    this.closed = false;
  }

  async resume() {}

  close() {
    this.closed = true;
  }

  createMediaStreamSource() {
    return { connect() {}, disconnect() {} };
  }

  createScriptProcessor() {
    return { onaudioprocess: null, connect() {}, disconnect() {} };
  }

  createGain() {
    return { gain: { value: 1 }, connect() {} };
  }
}

/** A deep copy whose objects and arrays belong to THIS realm, keeping every own key. */
function sameRealm(value) {
  if (value === null || typeof value !== "object") {
    return value;
  }
  if (Array.isArray(value)) {
    return value.map(sameRealm);
  }
  const copy = {};
  for (const key of Object.keys(value)) {
    copy[key] = sameRealm(value[key]);
  }
  return copy;
}

/**
 * Build a page.
 *
 * `store` is the browser's localStorage. Passing an existing one is how a RELOAD is simulated:
 * same storage, brand new script execution.
 */
function newPage(store = new Map()) {
  const elements = new Map();
  for (const [id, markup] of PAGE_ELEMENTS) {
    const element = new FakeElement(id);
    element.hidden = markup.hidden;
    element.checked = markup.checked;
    element.textContent = markup.text;
    elements.set(id, element);
  }
  const page = {
    elements,
    el: (id) => {
      const found = elements.get(id);
      if (!found) {
        throw new Error(`voice.js asked for #${id}, which web/voice.html does not define`);
      }
      return found;
    },
    /** Everything the page displays. Input VALUES are excluded: those are what the owner typed. */
    renderedText() {
      const parts = [];
      const walk = (element) => {
        parts.push(element.textContent);
        for (const kid of element.children) {
          walk(kid);
        }
      };
      for (const element of elements.values()) {
        if (element.id !== "api-token") {
          walk(element);
        }
      }
      return parts.join("\n");
    },
    setFetch(fn) {
      page.fetch = fn;
    },
    /** Every constraint object the page has handed getUserMedia, in order. */
    micRequests: [],
    /** Every WebSocket the page has opened, in order. */
    sockets: [],
  };
  const document = {
    getElementById: (id) => elements.get(id) || null,
    createElement: () => new FakeElement(""),
  };
  const localStorage = {
    getItem: (k) => (store.has(k) ? store.get(k) : null),
    setItem: (k, v) => store.set(k, v),
    removeItem: (k) => store.delete(k),
  };
  page.storage = store;

  class FakeWebSocket {
    constructor(url) {
      this.url = url;
      this.readyState = FakeWebSocket.OPEN;
      this.sent = [];
      page.sockets.push(this);
    }

    send(data) {
      this.sent.push(data);
    }

    close() {
      this.readyState = 3;
      if (this.onclose) {
        this.onclose({ code: 1000, reason: "" });
      }
    }
  }
  FakeWebSocket.OPEN = 1;

  const navigator = {
    mediaDevices: {
      getUserMedia: async (constraints) => {
        // Rebuilt in THIS realm, key for key. The page's object is made inside the vm context, so
        // its prototype is that context's `Object` and `deepStrictEqual` rejects it against an
        // otherwise identical literal here — a confusing red that says nothing about the page. A
        // JSON round-trip would fix that too, but it would also silently drop a key whose value
        // is `undefined`, and "we started sending an extra key" is exactly what these tests
        // exist to catch. So: same keys, same order, host realm.
        page.micRequests.push(sameRealm(constraints));
        return { getTracks: () => [{ stop() {} }] };
      },
    },
  };

  const context = {
    document,
    localStorage,
    window: { AudioContext: FakeAudioContext },
    navigator,
    console,
    setTimeout,
    clearTimeout,
    atob,
    btoa,
    WebSocket: FakeWebSocket,
    fetch: (...args) => page.fetch(...args),
  };
  vm.createContext(context);
  vm.runInContext(SCRIPT, context, { filename: "voice.js" });
  return page;
}

/** A gent-talk error response, exactly as `ApiError` serializes one. */
function errorResponse(status, error, detail) {
  return async () => ({
    ok: false,
    status,
    text: async () => JSON.stringify({ error, detail }),
  });
}

/** A successful mint, exactly as `/api/v1/signed-url` serializes one. */
const MINTED = async () => ({
  ok: true,
  status: 200,
  text: async () =>
    JSON.stringify({
      signed_url: "wss://example.invalid/convai?sig=test",
      agent_id: "agent_test",
      valid_for_seconds: 900,
    }),
});

/** The three microphone toggles, and the constraint each one drives. */
const MIC_TOGGLE_IDS = ["mic-echo-cancellation", "mic-noise-suppression", "mic-auto-gain"];

/**
 * Drive the whole happy path: save a token, mint, open the microphone, open the socket.
 *
 * The assertions here are load-bearing rather than decorative. If `start()` threw anywhere along
 * the way, `teardown()` would clear the session, and every later assertion about a LIVE call would
 * silently be testing the IDLE branch instead — a green test for the opposite behaviour.
 */
async function startTalking(page) {
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";
  await page.el("save-token").click();
  page.setFetch(MINTED);

  await page.el("start").click();

  assert.equal(
    page.el("error").hidden,
    true,
    `start() failed, so this is not a live call: ${page.el("error").textContent}`
  );
  assert.equal(page.sockets.length, 1, "start() opened no websocket");
  page.sockets[0].onopen();
  assert.equal(page.el("conversation-state").textContent, "connected");
  return page.sockets[0];
}

test("the page starts with no error showing", () => {
  // From the markup: the panel ships hidden, so its appearance later is itself the signal.
  const page = newPage();
  assert.equal(page.el("error").hidden, true, "web/voice.html must ship the panel hidden");
  assert.equal(page.el("error").textContent, "");
});

test("a vendor 502 is shown IN THE PAGE, in the vendor's own words", async () => {
  // The live case: the owner's ElevenLabs key lacks `convai_write`. Nobody should need dev tools
  // to learn that.
  const page = newPage();
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";
  await page.el("save-token").click();
  page.setFetch(
    errorResponse(502, "elevenlabs_error", `elevenlabs returned HTTP 401: ${CONVAI_WRITE}`)
  );

  await page.el("start").click();

  const shown = page.el("error");
  assert.equal(shown.hidden, false, "the error panel stayed hidden");
  assert.match(shown.textContent, /missing the permission convai_write/);
  assert.ok(
    shown.textContent.includes(CONVAI_WRITE),
    `the vendor's sentence was not shown verbatim: ${shown.textContent}`
  );
  assert.match(shown.textContent, /502/, "the HTTP status is part of the diagnosis");
  assert.match(shown.textContent, /elevenlabs_error/, "the error code identifies which failure");
  assert.equal(page.el("conversation-state").textContent, "error");
});

test("a 503 names the exact setting the operator has not set", async () => {
  const page = newPage();
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";
  await page.el("save-token").click();
  page.setFetch(
    errorResponse(
      503,
      "elevenlabs_not_configured",
      "elevenlabs.api_key is not set, so no conversation can be minted"
    )
  );

  await page.el("start").click();

  assert.match(page.el("error").textContent, /elevenlabs\.api_key/);
  assert.match(page.el("error").textContent, /503/);
});

test("a missing token is reported in the page rather than silently doing nothing", async () => {
  const page = newPage();
  let called = false;
  page.setFetch(async () => {
    called = true;
    throw new Error("unreachable");
  });

  await page.el("start").click();

  assert.equal(called, false, "nothing should be requested without a token");
  assert.match(page.el("error").textContent, /token/i);
  assert.equal(page.el("error").hidden, false);
});

test("no credential ever reaches the rendered page, even if the server echoes one back", async () => {
  // The server redacts its own secrets, but the page holds one the server never sees. A hostile
  // or merely careless error body that quotes the request back must not become visible text.
  const TOKEN = "write-token-SECRET-do-not-render";
  const page = newPage();
  page.el("api-token").value = TOKEN;
  await page.el("save-token").click();
  page.setFetch(
    errorResponse(502, "elevenlabs_error", `upstream rejected Authorization: Bearer ${TOKEN}`)
  );

  await page.el("start").click();

  const rendered = page.renderedText();
  assert.ok(
    !rendered.includes(TOKEN),
    `the API token was rendered into the page: ${rendered}`
  );
  assert.match(rendered, /\[redacted\]/, "the redaction must be visible, not a silent deletion");
  assert.ok(
    rendered.includes("upstream rejected"),
    "redaction must not swallow the rest of the message"
  );
});

test("Save changes the button itself, not only a banner", async () => {
  const page = newPage();
  const button = page.el("save-token");
  assert.equal(
    button.textContent,
    "Save token",
    "the fixture takes the starting label from web/voice.html itself"
  );
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";

  await button.click();

  assert.equal(button.textContent, "Saved ✓", "the button did not change state on a save");
  assert.equal(button.className, "ok");
  assert.equal(page.storage.get("gent-talk.token"), "write-token-aaaaaaaaaaaaaaaa");
  assert.match(page.el("token-state").textContent, /token saved/);
});

test("saving nothing is an error, not a success banner", async () => {
  const page = newPage();
  page.el("api-token").value = "   ";

  await page.el("save-token").click();

  assert.equal(page.storage.has("gent-talk.token"), false, "whitespace was stored as a token");
  assert.equal(page.el("save-token").textContent, "Save token");
  assert.match(page.el("error").textContent, /nothing to save/i);
});

test("a browser that refuses to store the token says so instead of claiming success", async () => {
  const page = newPage();
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";
  // Private browsing: setItem is accepted and then does nothing.
  page.storage.set = () => page.storage;

  await page.el("save-token").click();

  assert.notEqual(page.el("save-token").textContent, "Saved ✓");
  assert.match(page.el("error").textContent, /NOT saved/);
});

test("Forget clears the stored token and says what is now true", async () => {
  const page = newPage();
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";
  await page.el("save-token").click();

  await page.el("forget-token").click();

  assert.equal(page.storage.has("gent-talk.token"), false);
  assert.equal(page.el("api-token").value, "");
  assert.match(page.el("token-state").textContent, /no token saved/);
});

test("the page and the script agree about which elements exist", () => {
  // `newPage`'s getElementById throws for an unknown id, so merely constructing the page proves
  // the script's startup lookups all resolve. This states the property it relies on.
  assert.ok(PAGE_IDS.has("error"), "web/voice.html must define the error panel");
  assert.ok(PAGE_IDS.has("token-state"));
  const page = newPage();
  assert.throws(() => page.el("no-such-element"), /does not define/);
});

// --- microphone settings -------------------------------------------------------------------------
//
// The settings menu is an EXPOSURE change: it hands the owner three knobs that were previously
// hard-coded or unstated, and it must not move any of them. The first test below is the one that
// enforces that, and it is written as an exact-object comparison on purpose — a `match` or a
// property-by-property check would let an extra constraint through, and "we started sending one
// more thing to getUserMedia" is precisely the regression that would be invisible from the page.

test("with every toggle untouched, the page asks for EXACTLY what it asked for before the menu existed", async () => {
  const page = newPage();

  await startTalking(page);

  assert.deepStrictEqual(page.micRequests, [
    { audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } },
  ]);
});

test("every microphone toggle starts on, in the markup and after the script has run", () => {
  const page = newPage();
  for (const id of MIC_TOGGLE_IDS) {
    assert.ok(PAGE_IDS.has(id), `web/voice.html does not define #${id}`);
    assert.equal(PAGE_ELEMENTS.get(id).checked, true, `web/voice.html ships #${id} switched OFF`);
    assert.equal(page.el(id).checked, true, `the script switched #${id} OFF at load`);
  }
});

test("turning echo cancellation off asks for it off, and moves nothing else", async () => {
  const page = newPage();

  await page.el("mic-echo-cancellation").setChecked(false);
  await startTalking(page);

  assert.deepStrictEqual(page.micRequests, [
    { audio: { channelCount: 1, echoCancellation: false, noiseSuppression: true } },
  ]);
});

test("turning noise suppression off asks for it off, and moves nothing else", async () => {
  const page = newPage();

  await page.el("mic-noise-suppression").setChecked(false);
  await startTalking(page);

  assert.deepStrictEqual(page.micRequests, [
    { audio: { channelCount: 1, echoCancellation: true, noiseSuppression: false } },
  ]);
});

test("automatic gain control is stated ONLY when it is switched off", async () => {
  // The careful one. Today AGC is not in the constraint object at all, so the browser's own
  // default applies. Sending `autoGainControl: true` would convert an implicit default into an
  // explicit request, and the spec does not promise those are the same thing. So the ON state
  // stays silent, and only OFF is spoken — which is what keeps today's behaviour bit-for-bit.
  const on = newPage();
  await startTalking(on);
  assert.ok(
    !("autoGainControl" in on.micRequests[0].audio),
    "the default (on) state must stay UNSTATED, exactly as it is today"
  );

  const off = newPage();
  await off.el("mic-auto-gain").setChecked(false);
  await startTalking(off);
  assert.equal(off.micRequests[0].audio.autoGainControl, false);
});

test("the choices survive a reload", async () => {
  const first = newPage();
  await first.el("mic-auto-gain").setChecked(false);
  await first.el("mic-noise-suppression").setChecked(false);

  // Same browser storage, a fresh execution of the script: a reload.
  const second = newPage(first.storage);

  assert.equal(second.el("mic-auto-gain").checked, false, "the toggle came back on after a reload");
  assert.equal(second.el("mic-noise-suppression").checked, false);
  assert.equal(second.el("mic-echo-cancellation").checked, true, "an untouched toggle moved");

  await startTalking(second);
  assert.deepStrictEqual(second.micRequests, [
    {
      audio: {
        channelCount: 1,
        echoCancellation: true,
        noiseSuppression: false,
        autoGainControl: false,
      },
    },
  ]);
});

test("a toggle flipped mid-call says it does NOT apply to the call in progress", async () => {
  // Constraints are read when the microphone opens. A control that looks like it did something
  // and did not is worse than one that is plainly disabled, so the page has to say which call the
  // change lands on.
  const page = newPage();
  await startTalking(page);
  assert.equal(page.micRequests.length, 1);

  await page.el("mic-auto-gain").setChecked(false);

  assert.match(page.el("mic-settings-state").textContent, /does NOT change the call in progress/);
  assert.match(page.el("mic-settings-state").textContent, /hang up/i);
  assert.equal(
    page.micRequests.length,
    1,
    "the page silently re-opened the microphone during a live call"
  );
});

test("a toggle flipped while idle says it applies to the next call", async () => {
  const page = newPage();

  await page.el("mic-noise-suppression").setChecked(false);

  const said = page.el("mic-settings-state").textContent;
  assert.match(said, /^Saved\b/, `a stored setting must report itself saved: ${said}`);
  assert.match(said, /next time you start talking/);
  assert.doesNotMatch(said, /call in progress/, "no call is in progress");
});

test("a browser that refuses to store a microphone setting says so instead of claiming saved", async () => {
  const page = newPage();
  // Private browsing: setItem is accepted and then does nothing.
  page.storage.set = () => page.storage;

  await page.el("mic-auto-gain").setChecked(false);

  const said = page.el("mic-settings-state").textContent;
  assert.doesNotMatch(said, /^Saved\b/, `the page claimed a save that did not happen: ${said}`);
  assert.match(said, /refused to store/);
  assert.match(said, /forgotten when you reload/);

  // It is still live for THIS page, and the page should not pretend otherwise either.
  delete page.storage.set;
  await startTalking(page);
  assert.equal(page.micRequests[0].audio.autoGainControl, false);
});

test("a corrupt stored setting falls back to today's behaviour rather than to nothing", async () => {
  const store = new Map([["gent-talk.voice.mic", "{not json"]]);
  const page = newPage(store);

  for (const id of MIC_TOGGLE_IDS) {
    assert.equal(page.el(id).checked, true, `#${id} did not fall back to its default`);
  }
  await startTalking(page);
  assert.deepStrictEqual(page.micRequests, [
    { audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } },
  ]);
});

test("a browser that THROWS on write is caught, not left as a rejected promise", async () => {
  // Two different browsers refuse storage two different ways: Chrome's private mode accepts
  // setItem and stores nothing (above), Safari's throws QuotaExceededError. The second one is the
  // nastier failure — an uncaught throw inside the change listener leaves the line under the
  // toggles saying whatever it said last, which is usually "Saved."
  const page = newPage();
  page.storage.set = () => {
    throw new Error("QuotaExceededError: the quota has been exceeded.");
  };

  await page.el("mic-auto-gain").setChecked(false);

  const said = page.el("mic-settings-state").textContent;
  assert.doesNotMatch(said, /^Saved\b/, `the page claimed a save that threw: ${said}`);
  assert.match(said, /refused to store/);
});
