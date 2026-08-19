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
 * Every id the served page defines, with the label and the initial `hidden` state the MARKUP gives
 * it. Taking these from web/voice.html rather than restating them keeps the fixture honest: if the
 * page stops shipping the error panel, or ships it visible, the tests below change answer.
 */
const PAGE_ELEMENTS = new Map(
  [...HTML.matchAll(/<(\w+)([^>]*\bid="([^"]+)"[^>]*)>([^<]*)/g)].map((m) => [
    m[3],
    { hidden: /\bhidden\b/.test(m[2]), text: m[4].trim() },
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
    this.disabled = false;
    this.children = [];
    this.listeners = new Map();
  }

  addEventListener(type, fn) {
    const existing = this.listeners.get(type) || [];
    existing.push(fn);
    this.listeners.set(type, existing);
  }

  async click() {
    for (const fn of this.listeners.get("click") || []) {
      await fn();
    }
  }

  append(...kids) {
    this.children.push(...kids);
  }

  scrollIntoView() {}
}

function newPage() {
  const elements = new Map();
  for (const [id, markup] of PAGE_ELEMENTS) {
    const element = new FakeElement(id);
    element.hidden = markup.hidden;
    element.textContent = markup.text;
    elements.set(id, element);
  }
  const store = new Map();
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
  const context = {
    document,
    localStorage,
    window: {},
    navigator: {},
    console,
    setTimeout,
    clearTimeout,
    atob,
    btoa,
    WebSocket: class {},
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
