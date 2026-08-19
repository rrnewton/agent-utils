// Tests for the /voice page, run against the REAL web/voice.js, web/voice.html and web/voice.css.
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
//   * There is no innerHTML, no insertAdjacentHTML, and no HTML parser anywhere in the fixture. A
//     test therefore CANNOT accidentally certify markup injection as "rendered" — the only way a
//     `<script>` in a Discord message could become an element is if the page created that element
//     itself, and `page.createdTags` records every element the page creates so that is checked
//     directly.
//
// What this fixture CANNOT do, stated so nobody reads more into a green run than is there:
//
//   * It does not lay anything out. The frame assertions below read web/voice.css as text and
//     check that the specific properties which produce the app-like behaviour are present on the
//     specific selectors that need them. That catches a deletion or a moved rule. It does not and
//     cannot prove the page looks right on a phone.
//   * There is no real audio hardware, no real WebSocket, and no real ElevenLabs.
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
const CSS = readFileSync(join(WEB, "voice.css"), "utf8");

/**
 * The same files with their comments removed.
 *
 * Load-bearing, and the reason is a trap this suite fell into: several assertions below are of the
 * form "this string appears NOWHERE in the page". Those files explain at length WHY they avoid
 * `innerHTML` and WHY they declare no standalone mode — so the forbidden strings are all present,
 * in prose, and the assertions failed against a page that was entirely correct. Stripping comments
 * first is what makes those assertions about the CODE rather than about the commentary.
 */
const stripJsComments = (text) =>
  text.replace(/\/\*[\s\S]*?\*\//g, " ").replace(/(^|[^:"'`\\])\/\/[^\n]*/g, "$1");
const HTML_CODE = HTML.replace(/<!--[\s\S]*?-->/g, " ");
const SCRIPT_CODE = stripJsComments(SCRIPT);

/**
 * Every id the served page defines, with the label and the initial `hidden` / `checked` state the
 * MARKUP gives it. Taking these from web/voice.html rather than restating them keeps the fixture
 * honest: if the page stops shipping the error panel, or ships it visible, or ships a microphone
 * toggle off, the tests below change answer.
 */
const PAGE_ELEMENTS = new Map(
  [...HTML.matchAll(/<([\w-]+)([^>]*\bid="([^"]+)"[^>]*)>([^<]*)/g)].map((m) => [
    m[3],
    {
      hidden: /\bhidden\b/.test(m[2]),
      checked: /\bchecked\b/.test(m[2]),
      // The initial state the MARKUP gives the status line, so a test can read it before the page
      // has had any reason to change it.
      state: (/\bdata-state="([^"]+)"/.exec(m[2]) || [])[1],
      text: m[4].trim(),
    },
  ])
);
const PAGE_IDS = new Set(PAGE_ELEMENTS.keys());

/** The vendor's real refusal, quoted from the live 502 the owner hit. */
const CONVAI_WRITE =
  "The API key you used is missing the permission convai_write to execute this operation.";

/**
 * The declaration block(s) web/voice.css gives one selector.
 *
 * Matching whole rules, not grepping the file, so that a property landing on the WRONG selector
 * cannot satisfy an assertion about this one.
 */
function cssBlock(selector) {
  const blocks = [];
  for (const rule of CSS.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const selectors = rule[1]
      .split(",")
      .map((part) => part.split("\n").pop().trim())
      .filter(Boolean);
    if (selectors.includes(selector)) {
      blocks.push(rule[2]);
    }
  }
  assert.ok(blocks.length > 0, `web/voice.css has no rule for "${selector}"`);
  return blocks.join("\n");
}

class FakeElement {
  constructor(id, tagName = "") {
    this.id = id;
    this.tagName = tagName;
    this.textContent = "";
    this.value = "";
    this.className = "";
    this.hidden = false;
    this.checked = false;
    this.disabled = false;
    this.children = [];
    this.attributes = new Map();
    this.listeners = new Map();
    // The transcript is the one element that scrolls; the page pins it to the newest line.
    this.scrollTop = 0;
    this.scrollHeight = 0;
    this.scrolledIntoView = 0;
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

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
  }

  getAttribute(name) {
    return this.attributes.has(name) ? this.attributes.get(name) : null;
  }

  append(...kids) {
    this.children.push(...kids);
    // Growing content is what makes a scroll-to-bottom meaningful.
    this.scrollHeight += 10;
  }

  replaceChildren(...kids) {
    this.children = [...kids];
    this.scrollHeight = kids.length * 10;
  }

  scrollIntoView() {
    this.scrolledIntoView += 1;
  }

  focus() {}

  /** Every element at or under this one, so a test can ask what was really rendered. */
  descendants() {
    const out = [];
    const walk = (node) => {
      out.push(node);
      for (const kid of node.children) {
        walk(kid);
      }
    };
    for (const kid of this.children) {
      walk(kid);
    }
    return out;
  }

  /** The visible text of this subtree, concatenated in document order. */
  text() {
    return [this, ...this.descendants()].map((node) => node.textContent).join("");
  }
}

/**
 * The audio graph `start()` and `startCapture()` build. Only the pieces the page really touches
 * exist here, so a page that starts reaching for something else fails loudly rather than quietly
 * getting `undefined`.
 */
class FakeAudioContext {
  constructor(page) {
    this.page = page;
    this.sampleRate = 48000;
    this.currentTime = 0;
    this.destination = { id: "destination" };
    this.closed = false;
    // How many buffers were actually STARTED. This is what makes "the agent's voice is silenced"
    // an observation about playback rather than about a flag the page set on itself.
    this.played = 0;
    this.stopped = 0;
    page.audio = this;
  }

  createBuffer(channels, length, rate) {
    return { duration: length / rate, getChannelData: () => new Float32Array(length) };
  }

  createBufferSource() {
    const context = this;
    return {
      buffer: null,
      onended: null,
      connect() {},
      start() {
        context.played += 1;
      },
      stop() {
        context.stopped += 1;
      },
    };
  }

  async resume() {}

  close() {
    this.closed = true;
  }

  createMediaStreamSource() {
    return { connect() {}, disconnect() {} };
  }

  createScriptProcessor() {
    // Kept, because driving `onaudioprocess` by hand is how the mute tests observe whether audio
    // frames are actually reaching the socket.
    const processor = { onaudioprocess: null, connect() {}, disconnect() {} };
    this.page.processors.push(processor);
    return processor;
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

/** A JSON response, shaped exactly as the page's `api()` unpacks one. */
const json = (status, body) => ({
  ok: status < 400,
  status,
  text: async () => JSON.stringify(body),
});

/** A gent-talk error response, exactly as `ApiError` serializes one. */
function errorResponse(status, error, detail) {
  return async () => json(status, { error, detail });
}

/** A successful mint, exactly as `/api/v1/signed-url` serializes one. */
const MINTED = async () =>
  json(200, {
    signed_url: "wss://example.invalid/convai?sig=test",
    agent_id: "agent_test",
    valid_for_seconds: 900,
  });

const CHANNEL = { id: "1110000000000000001", label: "lead team", writable: true };

/** The three microphone toggles, and the constraint each one drives. */
const MIC_TOGGLE_IDS = ["mic-echo-cancellation", "mic-noise-suppression", "mic-auto-gain"];

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
    if (markup.state !== undefined) {
      element.setAttribute("data-state", markup.state);
    }
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
    /** Which screen is showing. Exactly one, or this throws — a page with none is a blank app. */
    screen() {
      const showing = ["signin", "main", "settings"].filter((s) => !page.el(`screen-${s}`).hidden);
      assert.equal(showing.length, 1, `exactly one screen must show, not ${showing.join("+")}`);
      return showing[0];
    },
    /** Which tab is showing on the main screen. */
    tab() {
      return page.el("pane-discord").hidden ? "voice" : "discord";
    },
    /** Let the page's own floating promises (sign-in on load) run to completion. */
    settle: () => new Promise((resolve) => setTimeout(resolve, 0)),
    /** The mint handler. `setFetch` replaces it; the other routes keep working. */
    mint: async () => {
      throw new Error("this test did not set a mint response");
    },
    clientConfig: async () =>
      json(200, { version: "test", channels: [CHANNEL], elevenlabs_agent_id: "agent_test" }),
    messages: [],
    channelMessages: async () => json(200, { channel: CHANNEL, messages: page.messages }),
    setFetch(fn) {
      page.mint = fn;
    },
    /** Every constraint object the page has handed getUserMedia, in order. */
    micRequests: [],
    /** Every microphone track it has been given, each counting its own `stop()` calls. */
    tracks: [],
    /** Every WebSocket the page has opened, in order. */
    sockets: [],
    /** Every ScriptProcessorNode it has built, in order. */
    processors: [],
    /** Every tag name the page has passed to createElement, in order. */
    createdTags: [],
  };

  const document = {
    getElementById: (id) => elements.get(id) || null,
    createElement: (tag) => {
      page.createdTags.push(String(tag).toLowerCase());
      return new FakeElement("", String(tag).toLowerCase());
    },
  };
  const localStorage = {
    getItem: (k) => (store.has(k) ? store.get(k) : null),
    setItem: (k, v) => store.set(k, v),
    removeItem: (k) => store.delete(k),
  };
  page.storage = store;
  page.timers = new Map();
  let nextTimer = 1;
  /** Fire — and forget — every pending timer registered for exactly this delay. */
  page.expireTimers = (ms) => {
    let fired = 0;
    for (const [id, timer] of [...page.timers]) {
      if (timer.ms === ms) {
        page.timers.delete(id);
        timer.fn();
        fired += 1;
      }
    }
    return fired;
  };

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
        const track = {
          stops: 0,
          stop() {
            this.stops += 1;
          },
        };
        page.tracks.push(track);
        return { getTracks: () => [track] };
      },
    },
  };

  // A clock the test controls, so a timestamp is a fact rather than a race, and so the
  // "arm, then act within a few seconds" behaviour can be walked past its own deadline.
  const HostDate = Date;
  let clockMs = HostDate.UTC(2026, 7, 19, 14, 5, 0);
  class FixedDate extends HostDate {
    constructor(...args) {
      super(...(args.length ? args : [clockMs]));
    }

    static now() {
      return clockMs;
    }
  }
  page.setClock = (ms) => {
    clockMs = ms;
  };
  page.clock = () => clockMs;

  const context = {
    document,
    localStorage,
    Date: FixedDate,
    window: { AudioContext: function () { return new FakeAudioContext(page); } },
    navigator,
    console,
    // Timers the TEST drives. Nothing is really scheduled, so the page's own 8-second banner timer
    // cannot hold the runner open — and, more usefully, a delay the page relies on stops being
    // untestable. `expireTimers` is what lets the "armed for a few seconds" behaviour below be
    // walked past its own deadline instead of documented and hoped for.
    setTimeout: (fn, ms) => {
      const id = nextTimer;
      nextTimer += 1;
      page.timers.set(id, { fn, ms });
      return id;
    },
    clearTimeout: (id) => {
      page.timers.delete(id);
    },
    atob,
    btoa,
    WebSocket: FakeWebSocket,
    fetch: async (path, options) => {
      if (String(path).startsWith("/api/v1/signed-url")) {
        return page.mint(path, options);
      }
      if (String(path).startsWith("/api/v1/client-config")) {
        return page.clientConfig(path, options);
      }
      if (/\/messages$/.test(String(path))) {
        return page.channelMessages(path, options);
      }
      throw new Error(`the page fetched ${path}, which this fixture does not serve`);
    },
  };
  vm.createContext(context);
  vm.runInContext(SCRIPT, context, { filename: "voice.js" });
  return page;
}

/**
 * Drive the whole happy path: sign in, mint, open the microphone, open the socket.
 *
 * The assertions here are load-bearing rather than decorative. If `start()` threw anywhere along
 * the way, `teardown()` would clear the session, and every later assertion about a LIVE call would
 * silently be testing the IDLE branch instead — a green test for the opposite behaviour.
 */
async function startTalking(page) {
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";
  await page.el("save-token").click();
  await page.settle();
  page.setFetch(MINTED);

  await page.el("talk").click();

  assert.equal(
    page.el("error").hidden,
    true,
    `start() failed, so this is not a live call: ${page.el("error").textContent}`
  );
  assert.equal(page.sockets.length, 1, "start() opened no websocket");
  page.sockets[0].onopen();
  assert.equal(state(page), "live");
  assert.equal(page.processors.length, 1, "capture never started");
  return page.sockets[0];
}

/** One buffer of microphone audio arriving from the browser's audio graph. */
function speakInto(page) {
  const processor = page.processors[page.processors.length - 1];
  assert.ok(processor && processor.onaudioprocess, "nothing is capturing the microphone");
  processor.onaudioprocess({
    inputBuffer: { getChannelData: () => new Float32Array(4096) },
  });
}

/** The words a seam puts ON THE SCREEN — not the ones it keeps inside its disclosure. */
const seamLabel = (li) =>
  li.descendants().find((node) => node.className === "seam-label").textContent;

/** The one status line's machine-readable state, which is what colours its dot. */
const state = (page) => page.el("status-line").getAttribute("data-state");

/** Just the audio frames, not the initiation message or a pong. */
const audioFrames = (socket) => socket.sent.filter((s) => s.includes("user_audio_chunk"));

/** Sign in and land on the main screen, without placing a call. */
async function signIn(page) {
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";
  await page.el("save-token").click();
  await page.settle();
  assert.equal(page.screen(), "main", "signing in did not reach the main interface");
}

// --- what the page says when things fail --------------------------------------------------------

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
  await signIn(page);
  page.setFetch(
    errorResponse(502, "elevenlabs_error", `elevenlabs returned HTTP 401: ${CONVAI_WRITE}`)
  );

  await page.el("talk").click();

  const shown = page.el("error");
  assert.equal(shown.hidden, false, "the error panel stayed hidden");
  assert.match(shown.textContent, /missing the permission convai_write/);
  assert.ok(
    shown.textContent.includes(CONVAI_WRITE),
    `the vendor's sentence was not shown verbatim: ${shown.textContent}`
  );
  assert.match(shown.textContent, /502/, "the HTTP status is part of the diagnosis");
  assert.match(shown.textContent, /elevenlabs_error/, "the error code identifies which failure");
  assert.equal(state(page), "error");
});

test("a 503 names the exact setting the operator has not set", async () => {
  const page = newPage();
  await signIn(page);
  page.setFetch(
    errorResponse(
      503,
      "elevenlabs_not_configured",
      "elevenlabs.api_key is not set, so no conversation can be minted"
    )
  );

  await page.el("talk").click();

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

  await page.el("talk").click();

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
  await page.settle();
  page.setFetch(
    errorResponse(502, "elevenlabs_error", `upstream rejected Authorization: Bearer ${TOKEN}`)
  );

  await page.el("talk").click();

  const rendered = page.renderedText();
  assert.ok(!rendered.includes(TOKEN), `the API token was rendered into the page: ${rendered}`);
  assert.match(rendered, /\[redacted\]/, "the redaction must be visible, not a silent deletion");
  assert.ok(
    rendered.includes("upstream rejected"),
    "redaction must not swallow the rest of the message"
  );
});

// --- the token, with a button that visibly reacts -----------------------------------------------

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
  await signIn(page);

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

// --- screens -------------------------------------------------------------------------------------

test("with no token the page IS the sign-in screen, not the interface behind a nag", async () => {
  const page = newPage();
  await page.settle();

  assert.equal(page.screen(), "signin");
  // The controls act on a call this visitor cannot place, so they are not on the screen.
  assert.equal(page.el("control-pane").hidden, true, "the control pane showed before sign-in");
  assert.equal(page.el("view-switch").hidden, true, "there is nothing to switch between yet");
  // The status line, on the other hand, stays: a refused token has to be able to say so.
  assert.equal(page.el("status-line").hidden, false);
});

test("the explanatory text lives on the sign-in screen, not over the transcript", () => {
  // The header the owner asked to be moved off the main view. Asserting on the MARKUP, because
  // the point is where it sits in the page, not what a variable says.
  const signin = HTML.slice(
    HTML.indexOf('id="screen-signin"'),
    HTML.indexOf('id="screen-main"')
  );
  assert.match(signin, /Your ElevenLabs agent has authentication enabled/);
  const main = HTML.slice(HTML.indexOf('id="screen-main"'), HTML.indexOf('id="screen-settings"'));
  assert.doesNotMatch(main, /Your ElevenLabs agent has authentication enabled/);
});

test("a token the server accepts leads to the main interface", async () => {
  const page = newPage();
  await signIn(page);

  assert.equal(page.screen(), "main");
  assert.equal(page.el("control-pane").hidden, false, "the controls must be reachable");
  assert.equal(page.el("view-switch").hidden, false);
});

test("a token the server REFUSES sends you back to sign in, and says which thing was wrong", async () => {
  const page = newPage();
  page.clientConfig = async () => json(401, { error: "unauthorized", detail: "unknown token" });
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";

  await page.el("save-token").click();
  await page.settle();

  assert.equal(page.screen(), "signin");
  assert.match(page.el("error").textContent, /refused that token/);
  assert.match(page.el("error").textContent, /unknown token/);
});

test("a server that is merely DOWN does not get mistaken for a bad token", async () => {
  // Bouncing to a sign-in form on a 503 would blame the owner's token for the server's outage,
  // and they would paste the token again and again while nothing changed.
  const page = newPage();
  page.clientConfig = async () => json(503, { error: "unavailable", detail: "still starting" });
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";

  await page.el("save-token").click();
  await page.settle();

  assert.equal(page.screen(), "main", "a server outage must not look like a sign-in failure");
  assert.match(page.el("error").textContent, /did not answer/);
  assert.equal(page.storage.get("gent-talk.token"), "write-token-aaaaaaaaaaaaaaaa");
});

test("settings is reachable from the main screen and comes back to it", async () => {
  const page = newPage();
  await signIn(page);

  await page.el("open-settings").click();
  assert.equal(page.screen(), "settings");
  assert.equal(page.el("control-pane").hidden, true, "the pane must not float over settings");

  await page.el("close-settings").click();
  assert.equal(page.screen(), "main");
  assert.equal(page.el("control-pane").hidden, false);
});

test("connection details appear once in a dismissable banner, and stay in settings", async () => {
  const page = newPage();
  assert.equal(
    PAGE_ELEMENTS.get("connection-banner").hidden,
    true,
    "the banner must ship hidden; it is a thing that ARRIVES"
  );

  await startTalking(page);

  const banner = page.el("connection-banner");
  assert.equal(banner.hidden, false, "the connection details never appeared");
  assert.match(page.el("connection-detail").textContent, /agent_test/);
  assert.match(page.el("settings-detail").textContent, /agent_test/);

  await page.el("dismiss-banner").click();

  assert.equal(banner.hidden, true, "Dismiss did not dismiss");
  // Dismissing loses nothing: the same text is still in settings, which is the whole reason the
  // banner is allowed to go away by itself.
  assert.match(page.el("settings-detail").textContent, /agent_test/);
});

// --- the frame: what makes it an app rather than a page -----------------------------------------

test("the document itself cannot scroll, and cannot rubber-band", () => {
  const body = cssBlock("body");
  assert.match(body, /height:\s*100dvh/, "the frame must use the DYNAMIC viewport unit");
  assert.match(body, /overflow:\s*hidden/, "the document must not scroll");
  assert.match(body, /overscroll-behavior:\s*none/, "the page must not rubber-band");
  assert.match(cssBlock("html"), /overscroll-behavior:\s*none/);
  assert.doesNotMatch(body, /height:\s*100vh\b/, "100vh is the unit that gets the toolbar wrong");
});

test("the transcript is the one thing that scrolls, and the control pane is a grid row", () => {
  const body = cssBlock("body");
  assert.match(
    body,
    /grid-template-rows:\s*auto 1fr auto/,
    "header / body / pane must be a three-row grid"
  );
  const scroll = cssBlock("#scroll-area");
  assert.match(scroll, /overflow-y:\s*auto/, "the transcript area must be the scrolling element");
  assert.match(scroll, /overscroll-behavior:\s*contain/, "a flick must not escape the transcript");
  assert.match(
    scroll,
    /min-height:\s*0/,
    "without min-height:0 the grid row refuses to shrink and the document scrolls again"
  );

  const pane = cssBlock("#control-pane");
  assert.doesNotMatch(
    pane,
    /position:\s*(fixed|absolute)/,
    "the pane holds its place by being a grid row, not by being pinned"
  );
  assert.doesNotMatch(pane, /overflow-y:\s*(auto|scroll)/, "the pane must never scroll");
  assert.match(
    pane,
    /padding-bottom:\s*calc\([^)]*env\(safe-area-inset-bottom\)/,
    "the controls must clear the home indicator"
  );
});

test("the controls are built to be tapped, not clicked", () => {
  const control = cssBlock(".control");
  assert.match(control, /touch-action:\s*manipulation/, "leaves the 300 ms tap delay in place");
  assert.match(control, /-webkit-tap-highlight-color:\s*transparent/, "grey flash on tap");
  assert.match(control, /user-select:\s*none/, "a long press would start selecting the label");
});

test("the page declares no standalone mode and no manifest", () => {
  // Deliberate, and easy to "helpfully" add back. Keeping the assertion next to the frame tests
  // so the reason travels with the rule.
  assert.doesNotMatch(HTML_CODE, /apple-mobile-web-app-capable/);
  assert.doesNotMatch(HTML_CODE, /rel="manifest"/);
  assert.doesNotMatch(HTML_CODE, /mobile-web-app-capable/);
  // The markup does not declare it, and the prose says why — so the comment must survive too.
  assert.match(HTML, /apple-mobile-web-app-capable/, "the reason it is absent must stay recorded");
});

test("the page fetches nothing from anywhere but this server", () => {
  // No framework, no CDN, no build step: the whole reason the frame is CSS.
  assert.doesNotMatch(HTML_CODE, /https?:\/\//, "web/voice.html reaches off-origin");
  assert.doesNotMatch(SCRIPT_CODE, /https?:\/\//, "web/voice.js reaches off-origin");
  assert.doesNotMatch(CSS, /@import|url\(\s*["']?https?:/, "web/voice.css reaches off-origin");
});

test("the control pane is one dense tile, and the two small controls stack to the height of the big ones", () => {
  // The shape the owner asked for:
  //
  //     +----+----------+----------+
  //     | () |          |          |
  //     +----+ Hang up  |   Talk   |
  //     | [] |          |          |
  //     +----+----------+----------+
  //
  // Checked as GEOMETRY rather than as pixels: both large controls span the same two rows the two
  // small ones occupy one each, so their heights cannot drift apart — there is nothing to keep in
  // sync. The two large columns are `1fr` each, so the pane fills the width rather than sitting in
  // it.
  const pane = cssBlock("#control-pane");
  assert.match(pane, /display:\s*grid/, "the pane is a tile, not a row of floating buttons");
  const columns = /grid-template-columns:\s*([^;]+);/.exec(pane);
  assert.ok(columns, "the pane must state its columns");
  assert.match(
    columns[1],
    /1fr\s+1fr\s*$/,
    `the two large controls must each take a share of the width, not ${columns[1]}`
  );
  assert.match(
    pane,
    /grid-template-rows:\s*1fr 1fr/,
    "two rows: one per small control, and both spanned by each large one"
  );

  assert.match(cssBlock("#speaker"), /grid-row:\s*1/);
  assert.match(cssBlock("#clear-view"), /grid-row:\s*2/);
  assert.match(cssBlock(".control-mini"), /grid-column:\s*1/, "the small controls are the left column");
  for (const selector of [".control-hangup", ".control-talk"]) {
    assert.match(
      cssBlock(selector),
      /grid-row:\s*1 \/ span 2/,
      `${selector} must span both rows, which is what makes the stack line up`
    );
  }
  assert.match(cssBlock(".control-hangup"), /grid-column:\s*2/);
  assert.match(cssBlock(".control-talk"), /grid-column:\s*3/);
});

test("the large controls are rectangular: nothing pins them to a square-ish fixed width", () => {
  // They were 8.5rem wide by 8rem tall — near-square slabs taking a third of a phone screen. The
  // owner asked for less tall and more rectangular. Width is now a share of the pane, so the only
  // way to regress this is to put a fixed width back.
  for (const selector of [".control-talk", ".control-hangup"]) {
    const block = cssBlock(selector);
    assert.doesNotMatch(block, /(^|[^-])width:\s*[\d.]+rem/, `${selector} pins its own width again`);
    assert.doesNotMatch(block, /min-height:\s*[\d.]+rem/, `${selector} pins its own height again`);
  }
  // Their height is two rows of the small control, and that is the only place a number appears.
  const mini = Number(/min-height:\s*([\d.]+)rem/.exec(cssBlock(".control-mini"))[1]);
  assert.ok(mini > 0 && mini * 2 < 6, `two stacked small controls are ${mini * 2}rem, which is not "less tall"`);
  assert.match(cssBlock(".control-talk.live .control-ring"), /animation:\s*talk-pulse/);
});

test("hang up comes before talk in the markup, so talk lands under the right thumb", () => {
  const pane = HTML.slice(HTML.indexOf('id="control-pane"'));
  const hangUp = pane.indexOf('id="hang-up"');
  const talk = pane.indexOf('id="talk"');
  assert.ok(hangUp > -1 && talk > -1, "the pane must hold both controls");
  assert.ok(hangUp < talk, "hang up must come before talk");
  // And the two small ones come before both, which is the left column.
  assert.ok(pane.indexOf('id="speaker"') < hangUp);
  assert.ok(pane.indexOf('id="clear-view"') < hangUp);
});

// --- the header ---------------------------------------------------------------------------------

test("the view control is ONE switch carrying the word for the view you are in", async () => {
  // What this replaces: two text labels, one outlined and one plain, which read as one item having
  // been disabled rather than as the unselected half of a pair.
  assert.equal(PAGE_IDS.has("tab-voice"), false, "the two-label pseudo-tabs are gone");
  assert.equal(PAGE_IDS.has("tab-discord"), false);

  const page = newPage();
  await signIn(page);
  const label = page.el("view-switch-label");
  assert.equal(label.textContent, "Voice", "the switch must name the view you are IN");
  assert.equal(page.el("view-switch").getAttribute("aria-checked"), "false");

  await page.el("view-switch").click();
  await page.settle();

  assert.equal(page.tab(), "discord");
  assert.equal(label.textContent, "Discord", "the word did not follow the switch");
  assert.equal(page.el("view-switch").getAttribute("aria-checked"), "true");

  await page.el("view-switch").click();
  assert.equal(page.tab(), "voice");
  assert.equal(label.textContent, "Voice");
});

test("the switch is a switch: a knob that moves, and a track that changes with it", () => {
  // Three signals for one state — word, knob position, track colour — so it survives being read
  // quickly, at arm's length, by somebody who is not looking for it.
  const html = HTML.slice(HTML.indexOf('id="view-switch"'), HTML.indexOf('id="open-settings"'));
  assert.match(html, /role="switch"/, "a switch has to announce itself as one");
  assert.match(html, /class="switch-track"/);
  assert.match(html, /class="switch-knob"/);
  assert.match(cssBlock('.switch[aria-checked="true"] .switch-knob'), /transform:\s*translateX/);
  assert.match(cssBlock('.switch[aria-checked="true"] .switch-track'), /background:\s*var\(--accent\)/);
});

test("settings is a gear, not the word Settings", () => {
  const button = HTML.slice(HTML.indexOf('id="open-settings"'), HTML.indexOf("</header>"));
  assert.match(button, /<svg/, "the settings control must be drawn, not spelled");
  assert.match(button, /aria-label="Settings"/, "an icon still has to say what it is");
  assert.equal(
    PAGE_ELEMENTS.get("open-settings").text,
    "",
    "the gear must carry no word; that was the thing being replaced"
  );
});

test("the header shows two things at a time, and which two depends on the screen", async () => {
  // It used to hold four controls in two styles and gave no readable model of which were tabs and
  // which were actions. Clear has moved to the control pane, where it is honestly grouped with the
  // other things you DO; the settings screen turns the same row into a title bar with a way back,
  // rather than leaving it empty and putting a Back button in the body.
  const row = HTML.slice(HTML.indexOf('class="topbar-row"'), HTML.indexOf("</header>"));
  const ids = [...row.matchAll(/<button[^>]*\bid="([^"]+)"/g)].map((m) => m[1]);
  assert.deepStrictEqual(ids, ["close-settings", "view-switch", "open-settings"]);

  const page = newPage();
  const showing = () =>
    ["close-settings", "topbar-title", "view-switch", "open-settings"].filter(
      (id) => !page.el(id).hidden
    );
  await signIn(page);
  assert.deepStrictEqual(showing(), ["view-switch", "open-settings"]);

  await page.el("open-settings").click();
  assert.deepStrictEqual(showing(), ["close-settings", "topbar-title"]);

  await page.el("close-settings").click();
  assert.deepStrictEqual(showing(), ["view-switch", "open-settings"]);
});

// --- the one status line -------------------------------------------------------------------------

test("state is reported in ONE place, above the controls", () => {
  // It used to be two: a small grey word under the header and a sentence at the foot. That split
  // is how the closed state managed to announce itself three times, in three vocabularies.
  assert.equal(
    PAGE_IDS.has("conversation-state"),
    false,
    "the second status element is still in the page"
  );
  const dock = HTML.slice(HTML.indexOf('id="dock"'));
  assert.ok(
    dock.indexOf('id="status-line"') < dock.indexOf('id="control-pane"'),
    "the status line must sit ABOVE the control pane, out of the corner curvature"
  );
  const header = HTML.slice(HTML.indexOf('id="topbar"'), HTML.indexOf("</header>"));
  assert.doesNotMatch(header, /id="status/, "the header must not report status too");
});

test("the status line is not pinned to the bottom edge of the screen", () => {
  // style.css pins #status to the viewport bottom for the OTHER page. Inherited here, that is what
  // put the line inside the corner curvature of the owner's phone, which ate its first word.
  assert.match(cssBlock("#status"), /position:\s*static/, "the shared fixed positioning is back");
});

test("the horizontal safe-area insets are applied, not only the bottom one", () => {
  // NOT VERIFIABLE BY SCREENSHOT: no browser automation can make Chromium report a non-zero inset,
  // and a headless browser renders a rectangle with no curved corners at all. This asserts the
  // declaration, which is the only part that can be checked here.
  for (const selector of ["#topbar", "#status-line", "#control-pane", "#scroll-area"]) {
    const block = cssBlock(selector);
    assert.match(block, /env\(safe-area-inset-left\)/, `${selector} ignores the left inset`);
    assert.match(block, /env\(safe-area-inset-right\)/, `${selector} ignores the right inset`);
  }
});

test("the controls SHOW their state, not merely hold it", () => {
  // Each of these was a mutation that behaviour tests alone let through: the page can be perfectly
  // correct about what it will do and still look identical while doing it. That is the whole class
  // of defect the owner photographed — a control that is dead and looks alive.

  // The status dot must actually differ between states, or the one status line is just a sentence.
  const dot = (state) => cssBlock(`#status-line[data-state="${state}"] .status-dot`);
  const colour = (block) => /background:\s*([^;]+);/.exec(block)[1].trim();
  const shades = new Set([colour(cssBlock(".status-dot")), colour(dot("live")), colour(dot("error"))]);
  assert.equal(shades.size, 3, `idle, live and error must look different: ${[...shades]}`);

  // An armed Clear that looks like an unarmed one is not a confirmation.
  const armed = cssBlock("#clear-view.armed");
  assert.match(armed, /var\(--warn\)/, "the armed state must be visibly different, not just named");

  // A control that cannot act must not keep the loudest fill on the screen.
  const off = cssBlock(".control[disabled]");
  assert.doesNotMatch(off, /background:\s*var\(--warn\)/, "a dead control kept its warm fill");
  assert.match(off, /background:\s*var\(--panel\)/);
});

test("content fades under the header rather than being sliced off square", () => {
  const scroll = cssBlock("#scroll-area");
  assert.match(scroll, /mask-image:\s*linear-gradient/, "the top edge has no fade");
  assert.match(scroll, /-webkit-mask-image:\s*linear-gradient/, "Safari needs the prefixed one");
});

// --- mute: the whole point ------------------------------------------------------------------------

test("MUTE DOES NOT STOP THE MICROPHONE STREAM", async () => {
  // The load-bearing assertion in this file.
  //
  // Mute exists so the conversation — and with it everything the agent has been told — survives a
  // pause. `track.stop()` ends the capture, which ends the call, which loses the context; the
  // vendor documents no way to resume a conversation once it has closed. So this is not a
  // preference about implementation: reaching `track.stop()` from the mute path destroys the only
  // behaviour mute has.
  const page = newPage();
  await startTalking(page);
  assert.equal(page.tracks.length, 1, "exactly one microphone stream should be open");

  await page.el("talk").click();
  // Drive the capture callback too. Muting sets a flag; the flag is read inside `onaudioprocess`,
  // so a `track.stop()` smuggled into THAT branch would never run in a test that only clicks.
  speakInto(page);
  speakInto(page);

  assert.equal(page.tracks[0].stops, 0, "mute stopped the microphone track");
  assert.equal(page.micRequests.length, 1, "mute re-acquired the microphone");
  assert.notEqual(page.sockets[0].readyState, 3, "mute closed the conversation");
  assert.equal(page.sockets.length, 1, "mute opened a second conversation");
});

test("mute stops audio frames reaching the agent", async () => {
  const page = newPage();
  const socket = await startTalking(page);

  speakInto(page);
  const heard = audioFrames(socket).length;
  assert.ok(heard > 0, "the page sent no audio at all, so this test proves nothing");

  await page.el("talk").click();
  speakInto(page);
  speakInto(page);

  assert.equal(audioFrames(socket).length, heard, "the agent kept hearing you while muted");
});

test("unmuting resumes the SAME conversation rather than starting a new one", async () => {
  const page = newPage();
  const socket = await startTalking(page);

  await page.el("talk").click();
  speakInto(page);
  await page.el("talk").click();
  speakInto(page);

  assert.equal(page.sockets.length, 1, "unmuting opened a new conversation");
  assert.equal(page.sockets[0], socket, "the conversation was replaced");
  assert.equal(page.micRequests.length, 1, "unmuting re-asked for the microphone");
  assert.equal(page.tracks[0].stops, 0, "the microphone was released somewhere in the round trip");
  assert.ok(audioFrames(socket).length > 0, "the agent cannot hear you after unmuting");
});

test("the talk control says which of its three states it is in", async () => {
  const page = newPage();
  await signIn(page);
  assert.equal(page.el("talk-label").textContent, "Talk");
  assert.doesNotMatch(page.el("talk").className, /\blive\b|\bmuted\b/);

  await startTalking(page);
  assert.equal(page.el("talk-label").textContent, "Listening");
  assert.match(page.el("talk").className, /\blive\b/, "no animation while the agent can hear you");

  await page.el("talk").click();
  assert.equal(page.el("talk-label").textContent, "Muted");
  assert.match(page.el("talk").className, /\bmuted\b/);
  assert.doesNotMatch(page.el("talk").className, /\blive\b/, "still animating while muted");
});

test("the muted status is ONE short line, and the explanation is in settings", async () => {
  // It used to be three sentences in the status strip. Multi-sentence explanation is not
  // user-interface text: the strip is one line tall, so most of it was simply unreadable. What is
  // left says the one thing a muted caller needs; the rest is under "What the controls do".
  const page = newPage();
  await startTalking(page);

  await page.el("talk").click();

  const said = page.el("status").textContent;
  assert.match(said, /still remembers/, "the one fact that matters while muted");
  assert.ok(said.length <= 60, `the status line is one line, not a paragraph: ${said}`);
  assert.ok(!/\.[^.]*\./.test(said.slice(0, -1)), `more than one sentence on the strip: ${said}`);

  // Not deleted — relocated. This is the whole progressive-disclosure move, so it is asserted.
  const note = HTML.slice(HTML.indexOf('id="continuity-note"'));
  assert.match(note, /microphone as in use/, "the phone's own indicator still needs explaining");
});

// --- hang up ------------------------------------------------------------------------------------

test("hang up DOES stop the microphone stream", async () => {
  const page = newPage();
  await startTalking(page);

  await page.el("hang-up").click();

  assert.equal(page.tracks[0].stops, 1, "hanging up left the microphone open");
  assert.equal(page.sockets[0].readyState, 3, "hanging up left the conversation open");
  assert.equal(state(page), "ended");
});

test("HANG UP IS NOT ON THE SCREEN WHEN THERE IS NO CALL", async () => {
  // The defect this replaces: the most prominent warm-coloured control on the screen, at full
  // saturation, doing nothing at all — while three other elements said the call had ended. Dimming
  // it was not enough; an orange rectangle at half opacity is still the loudest thing in a dark
  // interface. So there is no Hang up unless there is something to hang up.
  const page = newPage();
  await signIn(page);
  assert.equal(page.el("hang-up").hidden, true, "Hang up is on screen before any call");
  assert.equal(page.el("control-pane").className, "solo", "the one action must take both columns");

  await startTalking(page);
  assert.equal(page.el("hang-up").hidden, false, "Hang up vanished during a live call");
  assert.equal(page.el("control-pane").className, "");

  await page.el("hang-up").click();
  assert.equal(page.el("hang-up").hidden, true, "Hang up survived the call it ended");
  assert.equal(page.el("control-pane").className, "solo");
});

test("after a call the pane offers ONE action, and the memory caveat rides on it", async () => {
  // Two defects resolved as one moment: the dead control is gone, and the caveat that used to
  // sprawl as a paragraph across the transcript is a clause attached to the button it is about.
  const page = newPage();
  await signIn(page);
  assert.equal(page.el("talk-label").textContent, "Talk");
  assert.equal(page.el("talk-note").hidden, true, "nothing has ended yet, so there is no caveat");

  await startTalking(page);
  await page.el("hang-up").click();

  assert.equal(page.el("talk-label").textContent, "Start a new call");
  const note = page.el("talk-note");
  assert.equal(note.hidden, false, "the caveat is the reason this is a NEW call");
  assert.match(note.textContent, /starts fresh/);
  assert.ok(note.textContent.length <= 40, `a clause, not a paragraph: ${note.textContent}`);
});

test("hang up marks the break in the transcript instead of implying one long conversation", async () => {
  // The misleading thing the owner noticed: one continuous message stream over two conversations
  // that share no context. The stream stays — it is still the record of what was said — but it
  // now carries a visible seam saying the agent does not remember what is above it.
  const page = newPage();
  await startTalking(page);
  page.sockets[0].onmessage({
    data: JSON.stringify({
      type: "agent_response",
      agent_response_event: { agent_response: "hello" },
    }),
  });

  await page.el("hang-up").click();

  const lines = page.el("transcript").children;
  const last = lines[lines.length - 1];
  assert.equal(last.className, "seam", "the seam must not look like something somebody said");
  assert.equal(seamLabel(last), "new conversation");
  // Said once, not once per teardown path.
  assert.equal(
    lines.filter((li) => li.className === "seam").length,
    1,
    "the end-of-call seam was drawn more than once"
  );
});

test("THE SEAM IS A LABEL, NOT A PARAGRAPH — the explanation is one tap inside it", async () => {
  // The finding that started this: four sentences about what a vendor does and does not document,
  // printed into the transcript and then cut off mid-word behind the control pane. Being cut off
  // was the screen reporting that they did not belong on it.
  //
  // So this test is about what is ON THE SURFACE. The words are still available — they are inside
  // the <details>, and named by `title` for a pointer device — but they are not standing on the
  // screen, and no amount of rewriting them would satisfy this assertion.
  const page = newPage();
  await startTalking(page);
  await page.el("hang-up").click();

  const seamLine = page.el("transcript").children.at(-1);
  const label = seamLabel(seamLine);
  assert.ok(label.length <= 24, `the surface text must be a label: "${label}"`);
  assert.ok(label.split(/\s+/).length <= 3, `at most three words on the rule: "${label}"`);

  // The detail exists, is long, and is inside a disclosure — not beside the label.
  const details = seamLine.descendants().filter((node) => node.tagName === "details");
  assert.equal(details.length, 1, "the explanation must live in a disclosure");
  const detail = seamLine.descendants().find((node) => node.className === "seam-detail");
  assert.ok(detail.textContent.length > 120, "the explanation was deleted rather than disclosed");
  assert.match(detail.textContent, /has never seen anything above it/);
  assert.match(detail.textContent, /Mute rather than Hang up/, "name the control that does work");
  // A pointer device gets it on hover, without opening anything.
  const summary = seamLine.descendants().find((node) => node.className === "seam-summary");
  assert.equal(summary.getAttribute("title"), detail.textContent);
});

test("opening the disclosure brings it into view instead of unfolding it under the dock", async () => {
  // A disclosure at the bottom of a scrolled list expands DOWNWARD, behind the control pane, so
  // tapping it appears to do nothing at all. Caught by looking at a screenshot of it open.
  const page = newPage();
  await startTalking(page);
  await page.el("hang-up").click();

  const seamLine = page.el("transcript").children.at(-1);
  const details = seamLine.descendants().find((node) => node.tagName === "details");
  const before = seamLine.scrolledIntoView;

  details.open = true;
  await details.dispatch("toggle");
  assert.ok(seamLine.scrolledIntoView > before, "the opened explanation was left off-screen");

  // Closing it must not yank the list around.
  const after = seamLine.scrolledIntoView;
  details.open = false;
  await details.dispatch("toggle");
  assert.equal(seamLine.scrolledIntoView, after, "closing it moved the transcript");
});

test("the close code is not left showing in the banner on the call screen either", async () => {
  // The status line is not the only place a number could reach the eye: the connection banner sits
  // on the main screen and is written from the same string.
  const page = newPage();
  await startTalking(page);
  assert.equal(page.el("connection-banner").hidden, false, "the banner should be up mid-call");

  page.sockets[0].onclose({ code: 1005, reason: "" });

  assert.equal(page.el("connection-banner").hidden, true, "the close code stayed on the screen");
  assert.match(page.el("settings-detail").textContent, /code 1005/, "and it must not be lost");
});

test("a conversation the SERVER closes gets the same honest marker", async () => {
  const page = newPage();
  await startTalking(page);

  page.sockets[0].onclose({ code: 1006, reason: "gone" });

  const seamLine = page.el("transcript").children.at(-1);
  assert.equal(seamLabel(seamLine), "new conversation");
  assert.equal(state(page), "ended");
});

test("A RAW CLOSE CODE NEVER REACHES THE USER", async () => {
  // The owner's screen said "conversation closed (code 1005)". 1005 means the connection gave no
  // reason at all — so the page was reporting the ABSENCE of information as though it were
  // information, in a vocabulary only a WebSocket implementer has. Say what happened, or say
  // nothing.
  for (const [code, expected] of [
    [1000, /^Call ended\.$/],
    [1005, /connection dropped/],
    [1006, /connection dropped/],
    [4001, /ended unexpectedly/],
  ]) {
    const page = newPage();
    await startTalking(page);
    page.sockets[0].onclose({ code, reason: "" });

    const said = page.el("status").textContent;
    assert.match(said, expected, `close code ${code} was not put into words`);
    assert.doesNotMatch(said, /\d/, `the number ${code} reached the status line: ${said}`);
    // Not thrown away, though: whoever is debugging finds it in the connection details, which is
    // where numbers belong.
    assert.match(page.el("settings-detail").textContent, new RegExp(`code ${code}`));
  }
});

test("a call that never connected does not claim a conversation ended", async () => {
  const page = newPage();
  await signIn(page);
  page.setFetch(errorResponse(502, "elevenlabs_error", "nope"));

  await page.el("talk").click();
  await page.el("hang-up").click();

  assert.equal(
    page.el("transcript").children.length,
    0,
    "a failed mint produced an end-of-call marker for a call that never happened"
  );
});

// --- clearing the view ----------------------------------------------------------------------------

test("Clear sits in the control pane, not in the header beside the record it erases", () => {
  // In the header it stood next to a notice calling the transcript the only surviving record,
  // which invited exactly the reading it must not have: that the button destroys something
  // irreplaceable. In the pane it is honestly grouped with the other things you DO.
  const pane = HTML.slice(HTML.indexOf('id="control-pane"'));
  assert.ok(pane.includes('id="clear-view"'), "Clear is not in the control pane");
  const header = HTML.slice(HTML.indexOf('id="topbar"'), HTML.indexOf("</header>"));
  assert.doesNotMatch(header, /id="clear-view"/, "Clear is still in the header");
  assert.equal(PAGE_ELEMENTS.get("clear-view-label").text, "Clear");
});

test("CLEAR ASKS TWICE — one tap arms it, and only the second one erases anything", async () => {
  // It is destructive, it cannot be undone, and it now sits under a thumb, which makes an
  // accidental tap likelier rather than less likely. A label does not make that safe; a second tap
  // does.
  const page = newPage();
  await startTalking(page);
  page.sockets[0].onmessage({
    data: JSON.stringify({
      type: "agent_response",
      agent_response_event: { agent_response: "the secret is 12345" },
    }),
  });
  assert.equal(page.el("transcript").children.length, 1);

  await page.el("clear-view").click();

  assert.equal(page.el("transcript").children.length, 1, "the FIRST tap erased the transcript");
  assert.equal(page.el("clear-view-label").textContent, "Sure?", "the armed state must be visible");
  assert.match(page.el("clear-view").className, /\barmed\b/);
  assert.match(page.el("status").textContent, /again/i, "the status line must say what it wants");

  await page.el("clear-view").click();

  assert.equal(page.el("transcript").children.length, 1, "only the seam should remain");
  assert.equal(page.el("clear-view-label").textContent, "Clear", "it stayed armed after firing");
  assert.doesNotMatch(page.el("clear-view").className, /\barmed\b/);
});

test("an armed Clear disarms itself rather than lying in wait", async () => {
  // Armed forever would be worse than no confirmation at all: the second tap could arrive minutes
  // later, from a thumb aiming at something else entirely.
  const page = newPage();
  await startTalking(page);
  page.sockets[0].onmessage({
    data: JSON.stringify({ type: "agent_response", agent_response_event: { agent_response: "hi" } }),
  });

  await page.el("clear-view").click();
  assert.equal(page.el("clear-view-label").textContent, "Sure?");

  assert.equal(page.expireTimers(4000), 1, "nothing was scheduled to disarm it");
  assert.equal(page.el("clear-view-label").textContent, "Clear");
  assert.doesNotMatch(page.el("clear-view").className, /\barmed\b/);

  // And the next tap arms again rather than clearing.
  await page.el("clear-view").click();
  assert.equal(page.el("transcript").children.length, 1, "a lapsed arm still cleared");
});

test("leaving the main screen disarms Clear", async () => {
  const page = newPage();
  await signIn(page);

  await page.el("clear-view").click();
  await page.el("open-settings").click();
  await page.el("close-settings").click();

  assert.equal(page.el("clear-view-label").textContent, "Clear", "Clear came back still armed");
});

test("clearing during a call empties the screen and says the agent has not forgotten", async () => {
  // The ambiguous middle this must not be: the screen empties and the agent carries on with
  // everything the operator thought they had removed, with nothing on screen saying so.
  const page = newPage();
  await startTalking(page);
  page.sockets[0].onmessage({
    data: JSON.stringify({
      type: "agent_response",
      agent_response_event: { agent_response: "the secret is 12345" },
    }),
  });
  assert.match(page.el("transcript").children[0].text(), /the secret is 12345/);

  await page.el("clear-view").click();
  await page.el("clear-view").click();

  const remaining = page.el("transcript").children;
  assert.ok(
    !remaining.some((li) => li.text().includes("the secret is 12345")),
    "the view was not cleared"
  );
  assert.equal(remaining.length, 1, "exactly the seam should remain");
  assert.equal(remaining[0].className, "seam");
  // Two words on the surface; the sentences are inside it, exactly as at a conversation boundary.
  assert.equal(seamLabel(remaining[0]), "view cleared");
  const detail = remaining[0].descendants().find((node) => node.className === "seam-detail");
  assert.match(detail.textContent, /still has everything said before this point/);
  assert.match(page.el("status").textContent, /has not forgotten/);
  // A display action, so it must not have touched the call.
  assert.equal(page.tracks[0].stops, 0, "clearing the view hung up");
  assert.notEqual(page.sockets[0].readyState, 3);
});

test("clearing while idle just clears, with no claim about an agent that is not there", async () => {
  const page = newPage();
  await signIn(page);
  page.el("transcript").append(new FakeElement("", "li"));

  await page.el("clear-view").click();
  await page.el("clear-view").click();

  assert.equal(page.el("transcript").children.length, 0);
  assert.equal(page.el("status").textContent, "Transcript cleared.");
});

test("the newest line is pinned above the controls", async () => {
  const page = newPage();
  await startTalking(page);
  const area = page.el("scroll-area");
  // Stand in for layout the fixture cannot do: a transcript taller than its box, scrolled up.
  area.scrollHeight = 5000;
  area.scrollTop = 0;

  page.sockets[0].onmessage({
    data: JSON.stringify({
      type: "user_transcript",
      user_transcription_event: { user_transcript: "newest" },
    }),
  });

  assert.ok(area.scrollTop > 0, "the transcript did not follow the newest line");
  assert.equal(area.scrollTop, area.scrollHeight);
});

// --- the raw Discord view --------------------------------------------------------------------------

/** Load the Discord tab with a given set of channel messages. */
async function showDiscord(page, messages) {
  page.messages = messages;
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.tab(), "discord", "the switch did not reach the channel view");
  return page.el("discord-log").children;
}

test("switching views does not disturb a live call", async () => {
  const page = newPage();
  const socket = await startTalking(page);
  await page.el("talk").click(); // muted, so the mute state has something to be preserved
  page.messages = [message({ content: "hi" })];

  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.tab(), "discord");
  await page.el("view-switch").click();
  assert.equal(page.tab(), "voice");

  assert.equal(page.sockets.length, 1, "a tab switch opened a second conversation");
  assert.equal(page.sockets[0], socket);
  assert.notEqual(socket.readyState, 3, "a tab switch hung up");
  assert.equal(page.tracks[0].stops, 0, "a tab switch released the microphone");
  assert.equal(page.micRequests.length, 1, "a tab switch re-asked for the microphone");
  assert.equal(page.el("talk-label").textContent, "Muted", "a tab switch unmuted the call");
  assert.equal(page.el("control-pane").hidden, false, "the controls left with the transcript");
});

test("the header spends one row, because the transcript is what deserves the height", () => {
  const row = HTML.slice(HTML.indexOf('class="topbar-row"'), HTML.indexOf("</header>"));
  assert.equal((row.match(/class="topbar-row"/g) || []).length, 1, "the header grew a second row");
  for (const id of ["view-switch", "open-settings"]) {
    assert.ok(row.includes(`id="${id}"`), `#${id} is not in the top row`);
  }
});

function message(overrides) {
  return {
    id: "1122334455667788990",
    channel_id: CHANNEL.id,
    author: "ci-bot",
    author_id: "1000000000000000001",
    author_is_bot: true,
    timestamp: "2026-08-19T04:31:00.000Z",
    content: "hello",
    ...overrides,
  };
}

test("every raw message shows its author and its message id", async () => {
  // The entire value of this view: the agent has described messages that did not exist, and the
  // operator needs to be able to point at a real one — or show that there is none.
  const page = newPage();
  await signIn(page);

  const lines = await showDiscord(page, [message({ id: "999888777666555444", author: "rrnewton", author_is_bot: false })]);

  assert.equal(lines.length, 1);
  const text = lines[0].text();
  assert.match(text, /rrnewton/);
  assert.match(text, /999888777666555444/, "the message id is what makes this view worth having");
  assert.match(text, /2026-08-19 04:31/);
  assert.match(page.el("status").textContent, /1 message\(s\) from lead team/);
});

test("a bot author is labelled as one", async () => {
  const page = newPage();
  await signIn(page);
  const lines = await showDiscord(page, [message({ author: "deepscry-bot", author_is_bot: true })]);
  assert.match(lines[0].text(), /deepscry-bot \(bot\)/);
});

test("the small markdown subset renders, and renders as ELEMENTS the page made itself", async () => {
  const page = newPage();
  await signIn(page);

  const lines = await showDiscord(page, [
    message({ content: "**bold** and *italic* and `code` and ~~gone~~ and <@123> in <#456>" }),
  ]);

  const kinds = lines[0].descendants();
  const of = (tag) => kinds.filter((node) => node.tagName === tag).map((node) => node.textContent);
  assert.deepStrictEqual(of("strong"), ["bold"]);
  assert.deepStrictEqual(of("em"), ["italic"]);
  assert.deepStrictEqual(of("code"), ["code"]);
  assert.deepStrictEqual(of("s"), ["gone"]);
  // A mention is shown as the id it really is. Inventing a display name here would be the same
  // class of mistake this view exists to expose.
  assert.ok(lines[0].text().includes("@123"), "a user mention should render as its id");
  assert.ok(lines[0].text().includes("#456"), "a channel mention should render as its id");
});

test("a fenced code block is taken verbatim, not parsed", async () => {
  const page = newPage();
  await signIn(page);

  const lines = await showDiscord(page, [
    message({ content: "look:\n```\n**not bold** <b>not markup</b>\n```\ndone" }),
  ]);

  const pre = lines[0].descendants().filter((node) => node.tagName === "pre");
  assert.equal(pre.length, 1);
  assert.equal(pre[0].textContent, "**not bold** <b>not markup</b>");
  assert.equal(
    lines[0].descendants().filter((node) => node.tagName === "strong").length,
    0,
    "markdown inside a code fence was parsed"
  );
});

test("a blockquote renders as a quote rather than as a stray angle bracket", async () => {
  const page = newPage();
  await signIn(page);
  const lines = await showDiscord(page, [message({ content: "> quoted line\nplain line" })]);
  const quotes = lines[0].descendants().filter((node) => node.className === "md-quote");
  assert.equal(quotes.length, 1);
  assert.match(quotes[0].text(), /quoted line/);
  assert.doesNotMatch(quotes[0].text(), /plain line/);
});

test("HOSTILE MESSAGE TEXT NEVER BECOMES MARKUP", async () => {
  // Channel text is written by whoever is in the channel, including bots nobody here controls.
  // The payloads below are the ones that matter: a script tag, an event-handler attribute, and an
  // image that fires on error. All three must come out as the characters they are.
  const page = newPage();
  await signIn(page);
  const HOSTILE =
    '<script>alert("xss")</script> <img src=x onerror=alert(1)> ' +
    '<iframe src="javascript:alert(2)"></iframe> &lt;already escaped&gt;';

  const before = page.createdTags.length;
  const lines = await showDiscord(page, [message({ content: HOSTILE })]);

  // 1. It is all still there, verbatim, as text the operator can read.
  assert.ok(
    lines[0].text().includes('<script>alert("xss")</script>'),
    `the payload was mangled rather than shown: ${lines[0].text()}`
  );
  assert.ok(lines[0].text().includes("<img src=x onerror=alert(1)>"));
  assert.ok(
    lines[0].text().includes("&lt;already escaped&gt;"),
    "text that was already escaped must not be double-unescaped into live markup"
  );

  // 2. No element of a dangerous kind was ever created. The fixture has no HTML parser, so the
  //    ONLY way one could exist is if the page created it — and every createElement is recorded.
  const created = page.createdTags.slice(before);
  for (const tag of ["script", "img", "iframe", "style", "object", "embed", "link"]) {
    assert.ok(!created.includes(tag), `rendering a message created a <${tag}>`);
  }

  // 3. Nothing carried an event-handler or a src attribute out of the message.
  for (const node of lines[0].descendants()) {
    for (const name of node.attributes.keys()) {
      assert.doesNotMatch(name, /^on/i, `an ${name} attribute was set from channel text`);
      assert.notEqual(name, "src");
    }
  }

  // 4. And the page really does have no HTML sink to reach for.
  assert.doesNotMatch(SCRIPT_CODE, /innerHTML|outerHTML|insertAdjacentHTML|document\.write/);
});

test("a link is only a link when its scheme is one a tap can be trusted with", async () => {
  const page = newPage();
  await signIn(page);

  const lines = await showDiscord(page, [
    message({
      content:
        "[safe](https://example.com/x) [script](javascript:alert(1)) " +
        "[data](data:text/html,<script>1</script>) [relative](/admin/delete)",
    }),
  ]);

  const anchors = lines[0].descendants().filter((node) => node.tagName === "a");
  assert.equal(anchors.length, 1, "exactly one of those four is safe to make tappable");
  assert.equal(anchors[0].textContent, "safe");
  assert.equal(anchors[0].getAttribute("href"), "https://example.com/x");
  assert.match(anchors[0].getAttribute("rel"), /noopener/);

  // The rejected ones are not silently dropped: the operator gets to SEE what the message said,
  // which is the point of a verification view.
  const text = lines[0].text();
  assert.ok(text.includes("javascript:alert(1)"), "a refused URL must still be visible as text");
  assert.ok(text.includes("/admin/delete"), "a same-origin URL is not automatically safe");
});

test("a message with no content at all renders without throwing", async () => {
  const page = newPage();
  await signIn(page);
  const lines = await showDiscord(page, [message({ content: "" }), message({ content: null })]);
  assert.equal(lines.length, 2);
});

test("Refresh re-reads the channel rather than appending to it", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1", content: "one" })]);

  page.messages = [message({ id: "2", content: "two" })];
  await page.el("refresh-discord").click();
  await page.settle();

  const lines = page.el("discord-log").children;
  assert.equal(lines.length, 1, "Refresh appended instead of replacing");
  assert.match(lines[0].text(), /two/);
});

test("a Discord read that fails reports itself and does NOT hang up on you", async () => {
  const page = newPage();
  await startTalking(page);
  page.channelMessages = async () => json(502, { error: "discord_error", detail: "rate limited" });

  await page.el("view-switch").click();
  await page.settle();

  assert.match(page.el("error").textContent, /rate limited/);
  assert.equal(page.tracks[0].stops, 0, "a failed channel read tore down the call");
  assert.notEqual(page.sockets[0].readyState, 3, "a failed channel read hung up");
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
  // The careful one. AGC is not in the constraint object at all by default, so the browser's own
  // default applies. Sending `autoGainControl: true` would convert an implicit default into an
  // explicit request, and the spec does not promise those are the same thing. So the ON state
  // stays silent, and only OFF is spoken — which is what keeps the behaviour bit-for-bit.
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

test("the settings screen explains what each control does to the agent's memory", () => {
  // Every one of these sentences corrects a way the interface could otherwise be read as promising
  // continuity it does not have. They live HERE, one tap away, rather than on the call screen.
  const note = HTML.slice(HTML.indexOf('id="continuity-note"'));
  assert.match(note, /Mute<\/strong> keeps the call open and keeps the agent's context/);
  assert.match(note, /does not keep its context/);
  assert.match(note, /never makes the agent forget/);
  assert.match(note, /Sound<\/strong> silences the agent's voice without silencing the agent/);
});

// --- the agent's voice, without the agent ---------------------------------------------------------

test("SOUND OFF SILENCES THE VOICE AND KEEPS THE TEXT", async () => {
  // The whole point of the control, and the thing that would be silently lost by "simplifying" it
  // into a mute: the agent keeps talking, and you keep reading it. A test that only checked that
  // no audio played would pass on a control that dropped the replies entirely.
  const page = newPage();
  await startTalking(page);
  const speak = () =>
    page.sockets[0].onmessage({
      data: JSON.stringify({ type: "audio", audio_event: { audio_base_64: btoa("\u0000\u0001") } }),
    });
  const reply = (text) =>
    page.sockets[0].onmessage({
      data: JSON.stringify({ type: "agent_response", agent_response_event: { agent_response: text } }),
    });

  speak();
  reply("heard me");
  const played = page.audio.played;
  assert.ok(played > 0, "no audio played at all, so this test proves nothing");

  await page.el("speaker").click();
  speak();
  speak();
  reply("still talking");

  assert.equal(page.audio.played, played, "the agent's voice kept playing with sound off");
  const said = page.el("transcript").children.map((li) => li.text()).join(" ");
  assert.match(said, /still talking/, "sound off swallowed the agent's WORDS as well as its voice");
  assert.equal(page.el("speaker-label").textContent, "Silent");
  assert.equal(page.el("speaker").getAttribute("aria-pressed"), "true");
  assert.match(page.el("status").textContent, /still arrive as text/);

  await page.el("speaker").click();
  speak();
  assert.ok(page.audio.played > played, "the agent stayed silent after sound was turned back on");
  assert.equal(page.el("speaker-label").textContent, "Sound");
});

test("sound off stops the sentence that is ALREADY playing", async () => {
  // Found by mutation: dropping incoming frames alone leaves whatever is already scheduled on the
  // audio clock to finish, so the agent keeps talking for a second or two after being silenced —
  // which is exactly the moment you reached for the control.
  const page = newPage();
  await startTalking(page);
  const speak = () =>
    page.sockets[0].onmessage({
      data: JSON.stringify({ type: "audio", audio_event: { audio_base_64: btoa("\u0000\u0001") } }),
    });
  speak();
  speak();
  assert.equal(page.audio.stopped, 0, "nothing has been stopped yet");

  await page.el("speaker").click();

  assert.ok(page.audio.stopped > 0, "the agent finished its sentence after being silenced");
});

test("sound off touches neither the microphone, the call, nor mute", async () => {
  const page = newPage();
  const socket = await startTalking(page);
  speakInto(page);
  const heard = audioFrames(socket).length;

  await page.el("speaker").click();
  speakInto(page);

  assert.equal(page.tracks[0].stops, 0, "silencing the agent released the microphone");
  assert.equal(page.sockets.length, 1, "silencing the agent opened a second conversation");
  assert.notEqual(socket.readyState, 3, "silencing the agent hung up");
  assert.ok(audioFrames(socket).length > heard, "silencing the agent also muted YOU");
  assert.equal(page.el("talk-label").textContent, "Listening", "the talk control changed state");
});

// --- reading the transcript -----------------------------------------------------------------------

test("the two speakers are visibly different things, not one grey word apart", async () => {
  // They shared width, background and alignment, and were told apart only by a small grey label.
  // Now they differ in side, tint and corner — three signals, so the shape of the conversation is
  // readable before any of it is read.
  const page = newPage();
  await startTalking(page);
  page.sockets[0].onmessage({
    data: JSON.stringify({ type: "user_transcript", user_transcription_event: { user_transcript: "mine" } }),
  });
  page.sockets[0].onmessage({
    data: JSON.stringify({ type: "agent_response", agent_response_event: { agent_response: "theirs" } }),
  });

  const [mine, theirs] = page.el("transcript").children;
  assert.equal(mine.className, "mine");
  assert.equal(theirs.className, "theirs");

  const mineCss = cssBlock(".messages li.mine");
  const theirsCss = cssBlock(".messages li.theirs");
  const property = (block, name) => (new RegExp(`${name}:\\s*([^;]+);`).exec(block) || [])[1];
  assert.notEqual(property(mineCss, "background"), property(theirsCss, "background"), "same tint");
  assert.ok(/margin-left/.test(mineCss) && /margin-right/.test(theirsCss), "same alignment");
});

test("every line says when it was said", async () => {
  // The surface is also being used as a debugging record, and a record with no clock is hard to
  // line up against anything else that happened.
  const page = newPage();
  await startTalking(page);
  page.sockets[0].onmessage({
    data: JSON.stringify({ type: "agent_response", agent_response_event: { agent_response: "hello" } }),
  });

  const line = page.el("transcript").children[0];
  const at = line.descendants().find((node) => node.className === "at");
  assert.ok(at, "a message carries no timestamp");
  assert.match(at.textContent, /^\d{2}:\d{2}$/, `not a clock time: ${at.textContent}`);
});

test("the idle screen puts its invitation in the empty space, not in the bottom strip", async () => {
  // Most of an idle display was doing nothing while the one strip where a phone shows text worst
  // carried the instruction. The transcript area is the space that is free.
  const page = newPage();
  await signIn(page);
  assert.equal(page.el("empty-state").hidden, false, "the empty transcript says nothing at all");
  assert.match(page.el("empty-line").textContent, /Talk/, "the invitation must name the control");
  assert.ok(
    page.el("status").textContent.length <= 24,
    `the strip should be short now: ${page.el("status").textContent}`
  );

  await startTalking(page);
  page.sockets[0].onmessage({
    data: JSON.stringify({ type: "agent_response", agent_response_event: { agent_response: "hi" } }),
  });

  assert.equal(page.el("empty-state").hidden, true, "the invitation stayed under the conversation");
});
