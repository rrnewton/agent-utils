// The app page's tests: the message boxes: Enter, the phone's keyboard, the box that grows, and the composer key.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/composer.mjs.

import assert from "node:assert/strict";

import { BOTTOM_SLACK_PX, CSS_CODE, DESKTOP_QUERY, HTML, HTML_CODE, PAGE_CODE, PAGE_IDS, SCRIPT,
  SCRIPT_CODE, THREADS_MAIN_STORAGE, atBottomOf, cardIds, cardOf, compose, cssBlock, cssBlockIn,
  cssBlockOutside, cssRulesElsewhere, enterKey, errorResponse, focusName, focusOn, focused,
  inTwoColumns, inlineAnchors, json, keyboardMedia, keyboardSaid, litCards, mainMessagesPage,
  markupHolds, message, newPage, nodeIn, openReplyOn, pickThread, press, reloadPage, replyBoxAt,
  replyButton, replyColumnsPage, replyInStream, rowWithId, sentWithoutKey, settingsGroup,
  showDiscord, sideRow, signIn, sourceConstant, startTalking, tallChannel, test, threadBadge,
  threadData, threadPage, twoColumnServer, typedFrames } from "./fixture.mjs";

// --- `#616 composer-no-autofill`: no password manager takes a send for a sign-in -----------------
//
// The owner, on Android: "I get this last pass suggestion every time I send a message". Every
// control outside a <form> is one form to Chrome, and that form held the token's password field,
// filled with the saved token, beside every message box; Chrome hands an inferred submission of it
// to the phone's autofill service. What can be pinned here is the page's half. Whether the phone
// stops asking is checked only on the owner's phone.

/** Every text field the markup ships: inputs a reader types in, and every textarea. */
const typedFields = () =>
  [...HTML_CODE.matchAll(/<(input|textarea)\b([^>]*)>/g)]
    .map(([, tag, attributes]) => ({
      tag,
      attributes,
      id: (/\bid="([^"]+)"/.exec(attributes) || [])[1],
      type: (/(?:^|\s)type="([^"]+)"/.exec(attributes) || [])[1] || (tag === "input" ? "text" : "textarea"),
    }))
    .filter((field) => !["checkbox", "radio", "range", "button", "submit", "reset", "file", "color", "image"].includes(field.type));

test("composer-no-autofill: every field a reader types in tells password managers to stay out", () => {
  const fields = typedFields();
  for (const id of ["channel-compose-text", "side-compose-text", "reply-text", "compose-text", "search-field",
    "channel-directory-search", "api-token", "pair-input"]) {
    assert.ok(fields.some((field) => field.id === id), `${id} is no longer a text field in web/voice.html`);
  }
  for (const field of fields) {
    // The opt-outs LastPass, 1Password and Dashlane read in a desktop browser.
    for (const attribute of ['data-lpignore="true"', 'data-1p-ignore="true"', 'data-form-type="other"']) {
      assert.ok(field.attributes.includes(attribute), `#${field.id} lacks ${attribute}`);
    }
    // A one-line field gets autocomplete="off" too, which Chrome hands Android's autofill service as
    // the field's hint.
    if (field.tag === "input") assert.ok(field.attributes.includes('autocomplete="off"'), `#${field.id} lacks autocomplete="off"`);
  }
  // ...but not a message box: Chrome passes autocomplete="off" to a phone's keyboard as "no
  // suggestions", and the owner writes there with the keyboard's word suggestions.
  for (const id of ["channel-compose-text", "side-compose-text", "reply-text"]) {
    const box = fields.find((field) => field.id === id);
    assert.doesNotMatch(box.attributes, /\bautocomplete=/, `#${id} would lose the keyboard's suggestions`);
  }
});

test("composer-no-autofill: the token's password field has a form of its own, and no message box is in one", () => {
  const forms = [...HTML_CODE.matchAll(/<form\b([^>]*)>([\s\S]*?)<\/form>/g)];
  assert.equal(forms.length, 1, "web/voice.html has exactly one <form>, the sign-in fields'");
  const [, formAttributes, inside] = forms[0];
  assert.match(formAttributes, /\bid="signin-form"/);
  // A dialog form outside a dialog submits nowhere: an Enter can never put the token in the address.
  assert.match(formAttributes, /\bmethod="dialog"/);
  assert.equal(inside.trim(), "", "the form wraps nothing; its fields join it by their form attribute");
  const joined = typedFields().filter((field) => /\bform="signin-form"/.test(field.attributes)).map((field) => field.id);
  assert.deepEqual(joined.sort(), ["api-token", "pair-input", "pair-transfer"]);
  for (const field of typedFields().filter((one) => one.type === "password")) {
    assert.match(field.attributes, /\bform="signin-form"/, `password field #${field.id} is in the message boxes' form`);
  }
  for (const field of typedFields().filter((one) => one.id !== undefined && !joined.includes(one.id))) {
    assert.doesNotMatch(field.attributes, /\bform="/, `#${field.id} names a form`);
  }
  // Send is a click or a key, never a form's submission: the one submit listener is the sign-in
  // form's, and all it does is refuse.
  const submits = [...SCRIPT_CODE.matchAll(/el\("([\w-]+)"\)\.addEventListener\("submit"/g)].map((m) => m[1]);
  assert.deepEqual(submits, ["signin-form"]);
  assert.doesNotMatch(PAGE_CODE, /createElement\("form"\)|\.submit\(\)|requestSubmit/);
});

test("composer-no-autofill: the sign-in form's submit goes nowhere", async () => {
  const page = newPage();
  let prevented = false;
  await page.el("signin-form").dispatch("submit", { preventDefault: () => { prevented = true; } });
  assert.equal(prevented, true);
  assert.equal(page.storage.has("vibe-talk.token"), false, "a submit saved a token");
});

test("composer-no-autofill: the password field never holds the saved token", async () => {
  const store = new Map();
  const first = newPage(store);
  await signIn(first);
  assert.equal(first.el("api-token").value, "", "the token stayed in the password field after Save");
  assert.equal(store.get("vibe-talk.token"), "write-token-aaaaaaaaaaaaaaaa");

  const reloaded = newPage(store);
  await reloaded.settle();
  assert.equal(reloaded.screen(), "main");
  assert.equal(reloaded.el("api-token").value, "", "a reload wrote the saved token into the password field");

  // A browser that will not store it keeps what was pasted, so the owner can try again without pasting twice.
  const unstored = newPage();
  unstored.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";
  unstored.storage.set = () => unstored.storage;
  await unstored.el("save-token").click();
  assert.equal(unstored.el("api-token").value, "write-token-aaaaaaaaaaaaaaaa");
});

test("composer-no-autofill: a token the server refuses stays in the box, and Save on an empty box retries the saved one", async () => {
  const page = newPage();
  page.clientConfig = async () => json(401, { error: "unauthorized", detail: "token revoked" });
  page.el("api-token").value = "write-token-aaaaaaaaaaaaaaaa";
  await page.el("save-token").click();
  await page.settle();
  assert.equal(page.screen(), "signin");
  assert.equal(page.el("api-token").value, "write-token-aaaaaaaaaaaaaaaa", "a refused token was taken out of the box");

  // A reload with that token saved: the box is empty, and Save tries the saved token again rather
  // than calling it "nothing to save".
  const store = new Map([["vibe-talk.token", "write-token-aaaaaaaaaaaaaaaa"]]);
  const reloaded = reloadPage(store, (p) => {
    p.onlineConfig = p.clientConfig;
    p.clientConfig = errorResponse(401, "unauthorized", "token revoked");
  });
  await reloaded.settle();
  assert.equal(reloaded.screen(), "signin");
  assert.equal(reloaded.el("api-token").value, "");
  reloaded.clientConfig = reloaded.onlineConfig;
  await reloaded.el("save-token").click();
  await reloaded.settle();
  assert.doesNotMatch(reloaded.el("error").textContent, /nothing to save/i);
  assert.equal(reloaded.screen(), "main", "Save on the empty box did not retry the saved token");
  assert.equal(store.get("vibe-talk.token"), "write-token-aaaaaaaaaaaaaaaa");
});

test("the page and the script agree about which elements exist", () => {
  // `newPage`'s getElementById throws for an unknown id, so merely constructing the page proves
  // the script's startup lookups all resolve. This states the property it relies on.
  assert.ok(PAGE_IDS.has("error"), "web/voice.html must define the error panel");
  assert.ok(PAGE_IDS.has("token-state"));
  const page = newPage();
  assert.throws(() => page.el("no-such-element"), /does not define/);
});

/**
 * Every Enter worth telling apart, and whether it sends with the setting on and with it off.
 *
 * Written out rather than computed, so the table is the specification and not a second copy of the
 * handler. Ctrl+Enter and Meta+Enter send either way; Shift+Enter and Alt+Enter never do; an input
 * method mid-word gets the key whatever is held with it, because Enter there accepts the word.
 */
const ENTER_KEYS = [
  { name: "Enter", fields: {}, on: true, off: false },
  { name: "Shift+Enter", fields: { shiftKey: true }, on: false, off: false },
  { name: "Alt+Enter", fields: { altKey: true }, on: false, off: false },
  { name: "Ctrl+Enter", fields: { ctrlKey: true }, on: true, off: true },
  { name: "Meta+Enter", fields: { metaKey: true }, on: true, off: true },
  { name: "Enter during IME composition", fields: { isComposing: true }, on: false, off: false, ime: true },
  { name: "Enter reported as keyCode 229", fields: { keyCode: 229 }, on: false, off: false, ime: true },
  { name: "Ctrl+Enter during IME composition", fields: { ctrlKey: true, isComposing: true },
    on: false, off: false, ime: true },
  // A held key's repeats never send: one press is one send (`#248 composer-key`, where a Ctrl+Enter
  // outside a box moves the focus into it and its repeats followed it there and sent the draft).
  // Where the press itself would have sent, the repeats still have the default taken (`taken`), so
  // a held key types no run of new lines either; where it would not, the box keeps its new lines.
  { name: "a held Enter's repeat", fields: { repeat: true }, on: false, off: false, takenOn: true },
  { name: "a held Ctrl+Enter's repeat", fields: { ctrlKey: true, repeat: true }, on: false, off: false,
    takenOn: true, takenOff: true },
  { name: "a held Meta+Enter's repeat", fields: { metaKey: true, repeat: true }, on: false, off: false,
    takenOn: true, takenOff: true },
  { name: "a held Shift+Enter's repeat", fields: { shiftKey: true, repeat: true }, on: false, off: false },
];

/**
 * Each message box, on a page of its own with "Enter to send" set through the Settings switch.
 *
 * `ready(text)` puts the box on screen holding `text`, as a finger would, whatever the last key
 * did (a sent reply closes its screen); `sent()` counts what has left the browser from it; and
 * `went(text)` checks the last of that is `text`, sent where this box sends — so a key routed to the
 * wrong box's send cannot pass.
 */
const ENTER_COMPOSERS = {
  async call(on) {
    const page = newPage();
    const socket = await startTalking(page);
    await page.el("enter-sends").setChecked(on);
    // A textarea since `#241 voice-composer`, as the channel's box is: with the setting off, an
    // Enter is a new line here too.
    return {
      page, field: "compose-text",
      ready: (text) => compose(page, text),
      sent: () => typedFrames(socket).length,
      went: (text) => assert.equal(JSON.parse(typedFrames(socket).at(-1)).text, text),
    };
  },
  async channel(on) {
    const page = newPage();
    await signIn(page);
    await showDiscord(page, [message({ id: "300", content: "history" })]);
    await page.el("enter-sends").setChecked(on);
    return {
      page, field: "channel-compose-text",
      ready: (text) => page.el("channel-compose-text").setValue(text),
      sent: () => page.repliesPosted.length,
      went: (text) => assert.deepEqual(sentWithoutKey(page.repliesPosted.at(-1).body), { text }),
    };
  },
  async thread(on) {
    const page = newPage();
    const data = threadData();
    page.threadingSupported = true;
    page.threads = data.threads;
    await signIn(page);
    await showDiscord(page, data.messages);
    await threadBadge(page.el("discord-log").children[1]).click();
    assert.equal(page.el("thread-heading").hidden, false, "the thread did not open");
    await page.el("enter-sends").setChecked(on);
    return {
      page, field: "channel-compose-text",
      ready: (text) => page.el("channel-compose-text").setValue(text),
      sent: () => page.repliesPosted.length,
      went: (text) =>
        assert.deepEqual(sentWithoutKey(page.repliesPosted.at(-1).body), { text, thread_id: page.threads[0].id }),
    };
  },
  async reply(on) {
    const page = newPage();
    await signIn(page);
    const messages = [message({ id: "111", content: "a question" })];
    await openReplyOn(page, messages);
    await page.el("enter-sends").setChecked(on);
    return {
      page, field: "reply-text",
      ready: async (text) => {
        if (!replyInStream(page)) await openReplyOn(page, messages);
        await page.el("reply-text").setValue(text);
      },
      sent: () => page.repliesPosted.length,
      went: (text) => assert.deepEqual(sentWithoutKey(page.repliesPosted.at(-1).body), { text, reply_to: "111" }),
    };
  },
};

for (const [name, open] of Object.entries(ENTER_COMPOSERS)) {
  for (const on of [true, false]) {
    test(`ENTER IN THE ${name.toUpperCase()} BOX with Enter to send ${on ? "ON" : "OFF"}: Enter, Shift, Alt, Ctrl, Meta, IME and held keys`, async () => {
      const box = await open(on);
      for (const key of ENTER_KEYS) {
        const text = `${key.name} in the ${name} box`;
        await box.ready(text);
        const before = box.sent();
        const event = enterKey(key.fields);
        await box.page.el(box.field).dispatch("keydown", event);
        await box.page.settle();
        if (on ? key.on : key.off) {
          assert.equal(box.sent(), before + 1, `${key.name} did not send from the ${name} box`);
          box.went(text);
          assert.equal(event.defaulted, false,
            `${key.name} sent and left the browser its default too — a stray new line, or a page reload`);
        } else {
          assert.equal(box.sent(), before, `${key.name} sent from the ${name} box`);
          assert.equal(box.page.el(box.field).value, text, `${key.name} emptied the ${name} box`);
          // A new line is the box's own default, so it is left alone. Every box is a textarea since
          // `#241 voice-composer`, the call's included. An input method's Enter is the method's, always.
          // A held send key's repeats are taken, though: see `taken` in the table.
          const taken = Boolean(on ? key.takenOn : key.takenOff);
          assert.equal(event.defaulted, !taken, taken
            ? `${key.name} in the ${name} box was handed to the browser: a run of new lines`
            : `${key.name} had its default taken away, so the ${name} box gets no new line`);
        }
      }
      // ...and an ordinary key is nobody's business but the box's.
      await box.ready("half a wor");
      const before = box.sent();
      const letter = enterKey({ key: "d" });
      await box.page.el(box.field).dispatch("keydown", letter);
      await box.page.settle();
      assert.equal(box.sent(), before, `a letter sent from the ${name} box`);
      assert.equal(letter.defaulted, true, `a letter had its default taken away in the ${name} box`);
    });
  }
}

// `paste-never-sends`. The owner, 2026-10-26, on Android: "as soon as I press paste on Android, it is
// sending it immediately without giving me a chance to edit ... Just pasting something should never
// immediately send it." A keyboard's clipboard hands Chrome its text without a composition, and
// Chrome on Android turns each newline the keyboard commits into a trusted Enter keydown. Every way a
// paste can arrive, in every box: the text goes in, the box keeps it, nothing is sent; and the
// reader's own Enter, a moment later, still sends.

/** Each way pasted text reaches a box before the Enter its newline becomes. */
const PASTES = [
  { name: "a paste event", type: "paste", fields: {} },
  { name: "beforeinput insertFromPaste", type: "beforeinput", fields: { inputType: "insertFromPaste", data: null } },
  { name: "a keyboard's clipboard commit (insertText, no composition)", type: "beforeinput",
    fields: { inputType: "insertText", data: "and the second half of", isComposing: false } },
  { name: "a commit ending in a newline", type: "beforeinput",
    fields: { inputType: "insertText", data: "copied from another app\n", isComposing: false } },
  { name: "a drop", type: "beforeinput", fields: { inputType: "insertFromDrop", data: null } },
  // The owner, later: copying from vibe-talk itself and pasting sent at once too, so it is the
  // keyboard's paste, not the app copied from. A browser that reports the commit on `input` alone.
  { name: "a commit reported on input alone", type: "input",
    fields: { inputType: "insertText", data: "a message copied in vibe-talk\n", isComposing: false } },
];

for (const [name, open] of Object.entries(ENTER_COMPOSERS)) {
  test(`PASTE NEVER SENDS from the ${name} box: the Enter a pasted newline becomes is a new line`, async () => {
    const box = await open(true);
    for (const paste of PASTES) {
      const text = `half a message, then ${paste.name}`;
      await box.ready(text);
      const before = box.sent();
      await box.page.el(box.field).dispatch(paste.type, { type: paste.type, ...paste.fields });
      const enter = enterKey({});
      await box.page.el(box.field).dispatch("keydown", enter);
      await box.page.settle();
      assert.equal(box.sent(), before, `${paste.name} sent from the ${name} box`);
      assert.equal(box.page.el(box.field).value, text, `${paste.name} emptied the ${name} box`);
      assert.equal(enter.defaulted, true, `the Enter after ${paste.name} was taken: the pasted new line is lost`);
      // The reader's own Enter, once the paste is done, sends as ever.
      box.page.setClock(box.page.clock() + sourceConstant("PASTE_ENTER_MS") + 1);
      const own = enterKey({});
      await box.page.el(box.field).dispatch("keydown", own);
      await box.page.settle();
      assert.equal(box.sent(), before + 1, `the reader's Enter after ${paste.name} did not send from the ${name} box`);
      box.went(text);
    }
    // A word committed through a composition is typing, not a paste: the Enter right after it sends.
    await box.ready("typed and composed");
    const before = box.sent();
    await box.page.el(box.field).dispatch("beforeinput",
      { type: "beforeinput", inputType: "insertText", data: "composed", isComposing: true });
    await box.page.el(box.field).dispatch("keydown", enterKey({}));
    await box.page.settle();
    assert.equal(box.sent(), before + 1, `an Enter after a composed word did not send from the ${name} box`);
    // ...and one character typed outside a composition is a key, not a paste.
    await box.ready("ends with a question?");
    const after = box.sent();
    await box.page.el(box.field).dispatch("beforeinput",
      { type: "beforeinput", inputType: "insertText", data: "?", isComposing: false });
    await box.page.el(box.field).dispatch("keydown", enterKey({}));
    await box.page.settle();
    assert.equal(box.sent(), after + 1, `an Enter after one typed character did not send from the ${name} box`);
  });
}

test("ENTER TO SEND is a switch on the Settings screen, and it is on as the page is served", () => {
  const group = settingsGroup("Typing");
  assert.match(group, /<input id="enter-sends" type="checkbox" checked aria-describedby="enter-sends-hint" \/>\s*Enter to send\s*</);
  assert.match(group, /<p id="enter-sends-hint" class="hint" aria-live="polite"><\/p>/);
  assert.ok(markupHolds("screen-settings", "enter-sends"), "the switch is not on the Settings screen");
});

test("ENTER TO SEND is on until the reader turns it off, and stays as they left it across a reload", async () => {
  const page = newPage();
  await signIn(page);
  assert.equal(page.el("enter-sends").checked, true, "a fresh device does not send on Enter");
  await page.el("open-settings").click();
  await page.el("enter-sends").setChecked(false);
  assert.equal(page.storage.get("vibe-talk.voice.enter-sends"), "0", "turning it off was not stored");

  const again = newPage(page.storage);
  await signIn(again);
  assert.equal(again.el("enter-sends").checked, false, "a reload turned Enter to send back on");
  // The keys follow what was stored, not merely the switch.
  await showDiscord(again, [message({ id: "300", content: "history" })]);
  await again.el("channel-compose-text").setValue("a first line");
  const enter = enterKey({});
  await again.el("channel-compose-text").dispatch("keydown", enter);
  await again.settle();
  assert.equal(again.repliesPosted.length, 0, "Enter sent after a reload that kept Enter to send off");
  assert.equal(enter.defaulted, true, "Enter after the reload did not get its new line");

  await again.el("enter-sends").setChecked(true);
  assert.equal(again.storage.get("vibe-talk.voice.enter-sends"), "1", "turning it back on was not stored");
  const third = newPage(again.storage);
  await signIn(third);
  assert.equal(third.el("enter-sends").checked, true, "turning it back on did not survive a reload");
});

test("THE HINT NAMES THE KEY THAT SENDS, in the words on the reader's keyboard", async () => {
  for (const [platform, chord, other] of [
    ["Linux x86_64", "Ctrl+Enter", "Cmd"],
    ["Win32", "Ctrl+Enter", "Cmd"],
    ["Linux armv8l", "Ctrl+Enter", "Cmd"],
    ["MacIntel", "Cmd+Enter", "Ctrl"],
    ["iPhone", "Cmd+Enter", "Ctrl"],
    ["iPad", "Cmd+Enter", "Ctrl"],
  ]) {
    const page = newPage(new Map(), SCRIPT, (p) => p.setPlatform(platform));
    await signIn(page);
    const hint = page.el("enter-sends-hint");
    assert.equal(hint.textContent, "Shift+Enter starts a new line, and on a phone's keyboard so does ↵ in the box's corner.", `${platform}: the hint with it on`);
    await page.el("enter-sends").setChecked(false);
    assert.equal(hint.textContent, `${chord} sends. Enter starts a new line.`,
      `${platform}: the hint with it off does not name ${chord}`);
    assert.doesNotMatch(hint.textContent, new RegExp(other), `${platform}: the hint names the other key`);
    await page.el("enter-sends").setChecked(true);
    assert.equal(hint.textContent, "Shift+Enter starts a new line, and on a phone's keyboard so does ↵ in the box's corner.", `${platform}: the hint stuck`);
  }
});

test("the platform words the hint and decides nothing: Ctrl+Enter and Cmd+Enter send on a Mac and off it", async () => {
  for (const platform of ["MacIntel", "Linux x86_64"]) {
    const page = newPage(new Map(), SCRIPT, (p) => p.setPlatform(platform));
    await signIn(page);
    await showDiscord(page, [message({ id: "300", content: "history" })]);
    await page.el("enter-sends").setChecked(false);
    for (const fields of [{ ctrlKey: true }, { metaKey: true }]) {
      const text = `${platform} ${Object.keys(fields)[0]}`;
      await page.el("channel-compose-text").setValue(text);
      await page.el("channel-compose-text").dispatch("keydown", enterKey(fields));
      await page.settle();
      assert.deepEqual(sentWithoutKey(page.repliesPosted.at(-1)?.body), { text }, `${text} did not send`);
    }
  }
});

test("a phone's own return key says what Enter will do, in every message box", async () => {
  // `enterkeyhint` is what an on-screen keyboard draws on its return key: a send arrow, or a return.
  const page = newPage();
  await signIn(page);
  const boxes = ["compose-text", "channel-compose-text", "reply-text"];
  for (const id of boxes) assert.equal(page.el(id).getAttribute("enterkeyhint"), "send", `#${id} with it on`);
  await page.el("enter-sends").setChecked(false);
  for (const id of boxes) assert.equal(page.el(id).getAttribute("enterkeyhint"), "enter", `#${id} with it off`);
  await page.el("enter-sends").setChecked(true);
  for (const id of boxes) assert.equal(page.el(id).getAttribute("enterkeyhint"), "send", `#${id} turned back on`);
});

// --- `#575 mobile-keyboard-compose` ------------------------------------------------------------------
//
// The owner, 2026-10-27, on Android and iOS: a tap in the box left Send and "some useless white space"
// between it and the keyboard, whose own send key makes Send needless; and "I thought previously there
// was a way to put in new lines by pressing shift ... It just stays a send button and I have no way to
// place a new line." A phone keyboard's Shift never reaches the page (neither Gboard nor iOS reports it
// on the return key), so the new line is a key of the page's, in the box's corner while the keyboard is
// up and Enter sends.

/** Each box with a new-line key: how to open it (`ENTER_COMPOSERS`), and its key. */
const NEWLINE_BOXES = [["channel", "channel-newline"], ["thread", "channel-newline"], ["reply", "reply-newline"]];

for (const [name, keyId] of NEWLINE_BOXES) for (const refused of [false, true]) {
  test(`MOBILE KEYBOARD COMPOSE in the ${name} box${refused ? ", the editor refusing" : ""}: the new-line key writes a new line at the caret, keeps the focus, sends nothing, and Enter then sends both lines`, async () => {
    const box = await ENTER_COMPOSERS[name](true);
    box.page.execLineBreakRefuses = refused;
    const field = box.page.el(box.field);
    const key = box.page.el(keyId);
    await box.ready("first linesecond line");
    field.focus();
    field.setSelectionRange(10, 10);
    const inputs = [];
    field.addEventListener("input", (event) => inputs.push(event));
    const before = box.sent();
    // The press keeps the focus in the box: a tap's `mousedown` would move it to the key and take the
    // keyboard down with it.
    let kept = false;
    await key.dispatch("mousedown", { type: "mousedown", preventDefault: () => { kept = true; } });
    assert.equal(kept, true, `the new-line key's press takes the focus from the ${name} box`);
    await key.click();
    await box.page.settle();
    assert.equal(field.value, "first line\nsecond line", `no new line at the caret in the ${name} box`);
    assert.equal(field.selectionStart, 11, "the caret is not on the new line");
    assert.equal(field.selectionEnd, 11);
    assert.equal(box.page.document.activeElement, field, `the ${name} box lost the focus`);
    assert.deepEqual(inputs.map((event) => event.inputType), ["insertLineBreak"],
      `the ${name} box was not told of its new line, so its draft would not follow`);
    assert.equal(box.sent(), before, `the new-line key sent from the ${name} box`);
    // Through the browser's editor, which keeps it in the box's undo, where the browser will.
    assert.equal(box.page.execLineBreaks, 1, "the new line was not asked of the browser's editor");
    // Over a selection, as typing over one does.
    field.setSelectionRange(0, 5);
    await key.click();
    assert.equal(field.value, "\n line\nsecond line", "the new line did not replace the selection");
    await box.ready("first line\nsecond line");
    // Not a paste (`paste-never-sends`): the reader's Enter straight after still sends, both lines once.
    await field.dispatch("keydown", enterKey({}));
    await box.page.settle();
    assert.equal(box.sent(), before + 1, `Enter after the new-line key did not send from the ${name} box`);
    box.went("first line\nsecond line");
  });
}

test("MOBILE KEYBOARD COMPOSE: the new line is never noted as a paste, so the next Enter is the reader's", async () => {
  const box = await ENTER_COMPOSERS.channel(true);
  const field = box.page.el(box.field);
  await box.ready("one");
  field.focus();
  field.setSelectionRange(3, 3);
  await box.page.el("channel-newline").click();
  // Typed on, with no input reported: the key's own input is the last the box was told of.
  field.value = "one\ntwo";
  const enter = enterKey({});
  await field.dispatch("keydown", enter);
  await box.page.settle();
  assert.equal(box.sent(), 1, "an Enter just after the new-line key was taken for a paste's");
  box.went("one\ntwo");
  assert.equal(enter.defaulted, false, "the Enter that sent also typed a new line");
});

/**
 * `#610 mobile-keyboard-compose`. The channel's box laid out as a phone lays it out with the keyboard coming up,
 * in the layout viewport's coordinates: the stylesheet saying the composer is laid out for the keyboard
 * (`--keyboard-fit`), #scroll-area ending at `listBottom()` (the layout's foot where Safari leaves the
 * layout alone, the keyboard's top where Chrome resizes it), and the box at the list's very end with the
 * list scrolled to `END_TOP`, under however much room (`--keyboard-inset`) the composer keeps below it.
 * The list scrolls freely between none and that end plus the room, as a browser's does; the box's
 * Send and state line are not drawn (Enter sends, nothing to say).
 */
const END_TOP = 1000;
const BOX_PX = 80;
function phoneComposer(page, listBottom) {
  page.sayStyle({ "--keyboard-fit": "on" });
  const area = page.el("scroll-area");
  const box = page.el("channel-compose-text");
  const room = () => Number.parseFloat(page.documentElement.style.getPropertyValue("--keyboard-inset")) || 0;
  let top = END_TOP;
  Object.defineProperty(area, "scrollTop", {
    configurable: true,
    get: () => top,
    set: (value) => { top = Math.max(0, Math.min(Number(value) || 0, END_TOP + room())); },
  });
  area.getBoundingClientRect = () => ({ top: 0, bottom: listBottom(), height: listBottom(), left: 0, right: 393, width: 393 });
  area.getClientRects = () => [{}];
  box.getClientRects = () => [{}];
  box.getBoundingClientRect = () => {
    const bottom = listBottom() - (top - END_TOP);
    return { top: bottom - BOX_PX, bottom, height: BOX_PX, left: 0, right: 393, width: 393 };
  };
  for (const id of ["channel-send", "channel-compose-state"]) page.el(id).getClientRects = () => [];
  return {
    area, box, room,
    /** Where the box's foot is, in the layout viewport's coordinates. */
    foot: () => box.getBoundingClientRect().bottom,
    /** The visual viewport's foot: the keyboard's top, with all Safari draws above the keys. */
    keysTop: () => page.viewport.offsetTop + page.viewport.visual,
  };
}

/** The reader's finger on the screen, as the page hears it: a `touchstart` on the document. */
const touch = (page) => page.documentEvent("touchstart", { type: "touchstart" });

test("MOBILE KEYBOARD COMPOSE, iOS: the visual viewport shrinks and pans, the layout does not: the box lands on the keys as the list's end, and the reader then scrolls freely", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  // Safari's layout stays 852px tall with its keyboard up: the list ends at the layout's foot.
  const phone = phoneComposer(page, () => 852);
  const atEnd = () => phone.area.scrollTop === END_TOP + phone.room();
  await focusOn(page, phone.box);
  // The keyboard, with its suggestion strip and the floating Done control, leaves 472px of the 852.
  await page.resizeViewport({ visual: 472 });
  assert.equal(keyboardSaid(page), "open");
  assert.equal(phone.room(), 380, "the composer was not given the room the keyboard covers of its list");
  assert.equal(phone.foot(), phone.keysTop(), "the box's foot is not on the keyboard's top");
  assert.ok(atEnd(), "the box on the keys is not the list's end, so going to the foot overshoots it");
  // Safari pans the visual viewport over the field after its resize: in the jump's moment the box
  // follows to the new top, and is still the list's end.
  await page.panViewport(200);
  assert.equal(phone.foot(), 672, "the box was left where the pan put it, off the keyboard's top");
  assert.equal(phone.room(), 180, "the room under the box was not measured from the panned keyboard's top");
  assert.ok(atEnd(), "after the pan, the box on the keys is not the list's end");
  // The reader's own scrolling: up to what they answer, with their own pans after; nothing takes them back.
  await touch(page);
  const flush = phone.area.scrollTop;
  phone.area.scrollTop = flush - 300;
  await phone.area.dispatch("scroll");
  await page.panViewport(120);
  await page.panViewport(260);
  assert.equal(phone.area.scrollTop, flush - 300, "a scroll or a pan after the reader's touch took them back to the box");
  assert.equal(phone.room(), 180, "the room changed under a reader panning");
  // Going to the foot (a send, Newest, a keeper holding a reader at the foot) leaves the box on the keys.
  await page.panViewport(200);
  phone.area.scrollTop = 1e9;
  assert.equal(phone.foot(), phone.keysTop(), "going to the list's foot left the box off the keys");
  // The keyboard's moment over, the reader's place is theirs even with no touch heard: a pan moves nothing.
  phone.area.scrollTop = flush - 300;
  page.expireTimers(sourceConstant("KEYBOARD_SETTLE_MS"));
  await page.panViewport(0);
  assert.equal(phone.area.scrollTop, flush - 300);
  // The keyboard growing (an emoji row) with the reader scrolled away: room for it, and they stay put.
  await page.resizeViewport({ visual: 428 });
  assert.equal(phone.room(), 424, "the room did not grow with the keyboard");
  assert.equal(phone.area.scrollTop, flush - 300, "a keyboard growing under a reader who had scrolled took them back");
  // Gone with the focus.
  await page.moveFocus(null);
  page.expireTimers(0);
  assert.equal(keyboardSaid(page), "closed");
  assert.equal(phone.room(), 0, "the room stayed after the box lost the focus");
});

test("MOBILE KEYBOARD COMPOSE, iOS: an innerHeight that already leaves out the keyboard still gives the box its room, where #575 gave it none and hid it", async () => {
  // WebKit sizes `innerHeight` to the keyboard for a page asking for `interactive-widget=resizes-content`,
  // as this one does, and still lays the page out at the full 852px. `#575` measured the room from
  // `innerHeight`, got none, and left the box at the end of a list behind the keyboard with no scroll left.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  const phone = phoneComposer(page, () => 852);
  await focusOn(page, phone.box);
  await page.resizeViewport({ visual: 472, layout: 472 });
  assert.equal(keyboardSaid(page), "open");
  assert.equal(phone.room(), 380, "no room under the box: it stays under the keyboard");
  assert.equal(phone.foot(), 472, "the box's foot is not on the keyboard's top");
  assert.ok(phone.foot() - BOX_PX >= 0, "the box's top is off screen");
});

test("MOBILE KEYBOARD COMPOSE, Android: the layout resizes with the keyboard: no room, the box already on the keys, nothing else moved", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  const phone = phoneComposer(page, () => page.viewport.layout);
  await focusOn(page, phone.box);
  await page.resizeViewport({ visual: 516, layout: 516 });
  assert.equal(keyboardSaid(page), "open");
  assert.equal(phone.room(), 0, "a keyboard that resized the layout was given room");
  assert.equal(phone.foot(), 516);
  assert.equal(phone.area.scrollTop, END_TOP, "the list was moved though the box was on the keys already");
  // The keyboard growing with the reader where the jump left them: the jump is made again.
  await page.resizeViewport({ visual: 472, layout: 472 });
  assert.equal(phone.room(), 0);
  assert.equal(phone.foot(), 472);
});

test("MOBILE KEYBOARD COMPOSE: the jump is made again only in its own moment, and a touch ends that moment", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  const phone = phoneComposer(page, () => 852);
  await focusOn(page, phone.box);
  await page.resizeViewport({ visual: 472 });
  assert.equal(phone.foot(), 472);
  // Safari's late pan, once the moment is over: the reader panning, not the keyboard settling.
  page.expireTimers(sourceConstant("KEYBOARD_SETTLE_MS"));
  await page.panViewport(150);
  assert.equal(phone.foot(), 472, "a pan after the keyboard settled moved the box");
  // The keyboard changing size with the reader where the jump left them: a new jump, and a new moment.
  await page.resizeViewport({ visual: 436 });
  assert.equal(phone.foot(), phone.keysTop(), "the keyboard changed size and the box was left off its top");
  // The browser scrolling the list in that moment (Safari showing the field), with no touch: made again.
  phone.area.scrollTop -= 50;
  await phone.area.dispatch("scroll");
  assert.equal(phone.foot(), phone.keysTop(), "a scroll of the browser's in the jump's moment left the box off the keys");
  await touch(page);
  await page.panViewport(60);
  assert.notEqual(phone.foot(), phone.keysTop(), "a pan after the reader touched the screen moved the box to the keys");
  // ...and a scroll after the touch is the reader's: not a cap.
  const theirs = phone.area.scrollTop - 50;
  phone.area.scrollTop = theirs;
  await phone.area.dispatch("scroll");
  assert.equal(phone.area.scrollTop, theirs, "a scroll after the reader's touch was taken back to the box");
  // The keyboard going down and coming up again is a new jump, whatever the reader did before.
  await page.panViewport(0);
  await page.resizeViewport({ visual: 852 });
  assert.equal(keyboardSaid(page), "closed");
  await page.resizeViewport({ visual: 472 });
  assert.equal(phone.foot(), 472, "a keyboard opened again did not bring the box to its top");
});

test("MOBILE KEYBOARD COMPOSE: Keyboard layout debug draws the page's numbers over the top of the page, off by default, kept on this device", async () => {
  const store = new Map();
  const page = newPage(store);
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  const readout = page.el("keyboard-debug-readout");
  const toggle = page.el("keyboard-debug");
  assert.equal(readout.hidden, true, "the readout is drawn as served");
  assert.equal(toggle.checked, false);
  const phone = phoneComposer(page, () => 852);
  readout.getClientRects = () => [{}];
  toggle.checked = true;
  await toggle.dispatch("change");
  assert.equal(readout.hidden, false, "the switch did not draw the readout");
  assert.equal(store.get("vibe-talk.voice.keyboard-debug"), "1", "the choice was not kept");
  await focusOn(page, phone.box);
  await page.resizeViewport({ visual: 472, layout: 472 });
  await page.panViewport(100);
  const text = readout.textContent;
  for (const said of ["keyboard open", "innerHeight 472", "visualViewport height 472  offsetTop 100", "keys top 572",
    "box top 492 bottom 572", "foot - keys 0", "room 280",
    "last jump channel-compose-text: target 572, list moved 280 in all"]) {
    assert.ok(text.includes(said), `the readout does not say "${said}":\n${text}`);
  }
  assert.equal(readout.style.transform, "translateY(100px)", "the readout was left above the panned screen");
  toggle.checked = false;
  await toggle.dispatch("change");
  assert.equal(readout.hidden, true);
  assert.equal(store.has("vibe-talk.voice.keyboard-debug"), false);
  // Markup: out of the reader's way and a screen reader's, and nothing positioned outside the scrollers'
  // containment (`position: absolute` against the layout viewport, which never scrolls).
  assert.match(HTML, /<pre id="keyboard-debug-readout" aria-hidden="true" hidden><\/pre>/);
  assert.match(cssBlock("#keyboard-debug-readout"), /pointer-events:\s*none/);
  assert.match(cssBlock("#keyboard-debug-readout"), /position:\s*absolute/);
});

test("MOBILE KEYBOARD COMPOSE: with the keyboard up, the box's foot is on it, Send goes only while the keyboard's key sends, and the new-line key comes only then", () => {
  const { body } = keyboardMedia();
  const P = ':root[data-keyboard="open"]:has(#pane-discord:not([hidden]))';
  const rule = (selector) => {
    const at = body.indexOf(`${P} ${selector} {`);
    assert.ok(at >= 0, `no keyboard rule for ${selector}`);
    return body.slice(body.indexOf("{", at) + 1, body.indexOf("}", at));
  };
  // The composer of the box being written in keeps no room under itself but what the keyboard covers,
  // and says so to the script that brings its foot to the keyboard's top.
  const composer = rule(":is(#channel-composer, #side-composer):has(> textarea:focus)");
  assert.match(composer, /padding-bottom:\s*var\(--keyboard-inset, 0px\);/);
  assert.match(composer, /--keyboard-fit:\s*on;/);
  assert.match(rule("#side-scroll:has(:is(#channel-compose-text, #side-compose-text):focus)"),
    /padding-bottom:\s*0;/, "the list keeps its own room under the composer at its foot");
  // Send goes while the keyboard's key sends (`enterkeyhint="send"`), and only for the box with the focus.
  assert.match(rule(':is(#channel-compose-text, #side-compose-text)[enterkeyhint="send"]:focus ~ :is(#channel-send, #side-send)'),
    /display:\s*none;/);
  for (const hidden of body.matchAll(/([^{}]*#(?:channel|side)-send[^{}]*)\{([^{}]*)\}/g)) {
    if (/display:\s*none/.test(hidden[2])) {
      assert.match(hidden[1], /\[enterkeyhint="send"\]:focus/, `Send goes with the keyboard's key a return: ${hidden[1].trim()}`);
    }
  }
  // An empty state line is no row under the box; one with something to say stays.
  assert.match(rule(":is(#channel-compose-text, #side-compose-text):focus ~ :is(#channel-compose-state, #side-compose-state):empty"),
    /display:\s*none;/);
  // The new-line key: none anywhere but here, and here only while Enter sends.
  assert.match(CSS_CODE, /(?:^|\})\s*\.newline-key\s*\{\s*display:\s*none;\s*\}/, "the new-line key is drawn without a keyboard");
  const shown = rule(':is(#channel-compose-text, #side-compose-text, #reply-text)[enterkeyhint="send"]:focus ~ .newline-key');
  assert.match(shown, /display:\s*inline-flex;/);
  assert.match(shown, /align-self:\s*end;/);
  assert.match(shown, /justify-self:\s*end;/);
  // ...in the box's own grid cell, its corner, with room kept so no word is written under it.
  assert.match(rule(':is(#channel-compose-text, #side-compose-text)[enterkeyhint="send"]:focus'), /padding-right:\s*3\.25rem;/);
  assert.match(rule('#reply-text[enterkeyhint="send"]:focus'), /padding-right:\s*3\.25rem;/);
});

test("MOBILE KEYBOARD COMPOSE: each box's new-line key follows it, a button out of the tab order named for what it does", () => {
  for (const [boxId, keyId] of [["channel-compose-text", "channel-newline"], ["side-compose-text", "side-newline"],
    ["reply-text", "reply-newline"]]) {
    const after = HTML.slice(HTML.indexOf(`<textarea id="${boxId}"`));
    const next = after.slice(after.indexOf("</textarea>")).match(/<(?!!--)[a-z]+[^>]*>/)[0];
    assert.match(next, new RegExp(`^<button id="${keyId}" class="newline-key" type="button"`), `#${keyId} does not follow #${boxId}`);
    assert.match(next, /aria-label="New line"/);
    assert.match(next, /tabindex="-1"/, "a keyboard's Tab would stop on a key a keyboard has its own Shift+Enter for");
  }
});

test("the field states its own layout rule: the size iOS will not zoom", () => {
  // NOT VERIFIABLE BY SCREENSHOT. Chromium under automation has no iOS text-zoom behaviour at all,
  // so the declaration is the only part checkable here.
  //
  // Never under 16px, not a rem alone: iOS Safari zooms the whole frame when a smaller input takes
  // focus, and this page is a fixed 100dvh grid — the zoom pushes the dock off a viewport that
  // cannot scroll back.
  assert.match(
    cssBlock("#compose-text"),
    /font-size:\s*max\(1rem,\s*16px\)/,
    "the field is small enough that iOS Safari will zoom the frame when it takes focus"
  );
  // `#241 voice-composer`. The SAME box as the channel's — one rule, so the two cannot come to look
  // different — and the same Send.
  assert.equal(cssBlock("#compose-text"), cssBlock("#channel-compose-text"), "the call's box is drawn its own way");
  assert.equal(cssBlock("#send-text"), cssBlock("#channel-send"), "the call's Send is drawn its own way");
  assert.match(cssBlock("#voice-composer"), /padding:[^;]*var\(--scroll-tools-clearance, 0px\)/,
    "the floating chips can cover the call's box at the end of the list");
  // ...and room for the status pill too, which the call's view puts up at every turn and which
  // floats 0.5rem off the list's foot at least 1.9rem tall. tests/voice_composer_browser.py checks
  // in Chromium that it is not drawn over Send.
  assert.match(cssBlock("#voice-composer"), /padding-bottom:[^;]*var\(--scroll-tools-clearance, 0px\)[^;]*\+ 2\.4rem/,
    "the status pill lies across the call's Send after every message");
  assert.match(cssBlock("#status-line"), /min-height:\s*1\.9rem/, "the pill's height changed under the room kept for it");
  // The pressed Type toggle and the bar's text mode are gone with the toggle.
  assert.doesNotMatch(CSS_CODE, /#text-entry|data-mode="text"/, "the Type toggle's styles outlived it");
});

// --- the box that grows (#340 composer-grow-box) -------------------------------------------------
//
// The owner, on a desk: pasting a long message left the message box tiny while the room left of Send
// was empty. A Grow checkbox there, off by default, lets the box grow with its text up to 60% of the
// window. This fixture lays nothing out, so what it checks is the markup, the rules and where each is,
// the choice and where it is kept, the height written where the browser cannot size the box, and the
// composer kept in view; tests/composer_grow_browser.py measures the box and Send in Chromium.

const GROWING = [
  { composer: "voice-composer", box: "compose-text", send: "send-text", grow: "compose-grow", state: null },
  { composer: "channel-composer", box: "channel-compose-text", send: "channel-send", grow: "channel-compose-grow",
    state: "channel-compose-state" },
  { composer: "side-composer", box: "side-compose-text", send: "side-send", grow: "side-compose-grow",
    state: "side-compose-state" },
];

/** The markup of one composer, `<div id="...">` to its closing tag (a composer holds no other div). */
function composerMarkup(id) {
  const at = HTML_CODE.indexOf(`<div id="${id}">`);
  assert.ok(at >= 0, `web/voice.html has no #${id}`);
  return HTML_CODE.slice(at, HTML_CODE.indexOf("</div>", at));
}

/** The selector the desktop block gives a part of a composer: scoped away from a send card where one can be up. */
const growScoped = (composer, selector) => (composer === "side-composer" ? selector : `#screen-main:not([data-draft]) ${selector}`);

test("`#340 composer-grow-box`: EACH COMPOSER HOLDS A Grow CHECKBOX AFTER Send, unticked, drawn on a desk alone, on Send's row", () => {
  for (const { composer, box, send, grow, state } of GROWING) {
    const markup = composerMarkup(composer);
    const label = new RegExp(`<label class="composer-grow" title="[^"]*60%[^"]*">\\s*<input id="${grow}" type="checkbox" \\/> Grow\\s*<\\/label>`);
    assert.match(markup, label, `#${composer} has no Grow checkbox`);
    // After Send, so Tab goes from the box to Send first; before the state line.
    const order = [box, send, grow, ...(state ? [state] : [])].map((id) => markup.indexOf(`id="${id}"`));
    assert.ok(order.every((at, i) => at >= 0 && (i === 0 || at > order[i - 1])), `#${composer}'s parts are out of order: ${order}`);
    // Every other part of the composer spans both columns of the desk's row, so `dense` cannot put Send
    // beside it, above the box: whatever is added to a composer must say where it goes.
    // `#575 mobile-keyboard-compose`: the new-line key is drawn under a coarse pointer alone, with a phone's
    // keyboard up, so never in a desk's row (`(pointer: fine)`), and takes the box's own cell where it is.
    const ids = [...markup.matchAll(/\sid="([^"]+)"/g)].map((m) => m[1])
      .filter((id) => id !== composer && !/-newline$/.test(id));
    for (const id of ids.filter((one) => one !== send && one !== grow)) {
      assert.match(cssBlockIn(DESKTOP_QUERY, growScoped(composer, `#${id}`)), /grid-column:\s*1 \/ -1/,
        `#${id} does not span both columns of #${composer} on a desk`);
    }
    assert.match(cssBlockIn(DESKTOP_QUERY, growScoped(composer, `#${send}`)), /grid-column:\s*2\b/);
    const row = cssBlockIn(DESKTOP_QUERY, growScoped(composer, `#${composer}`));
    assert.match(row, /grid-template-columns:\s*minmax\(0, 1fr\) auto/);
    assert.match(row, /grid-auto-flow:\s*row dense/, `#${composer}: the label cannot come up beside Send`);
  }
  // A phone draws none of it; a desk draws it in the first column, left of Send.
  assert.match(cssBlockOutside(DESKTOP_QUERY, ".composer-grow"), /display:\s*none/, "a phone draws the Grow checkbox");
  const label = cssBlockIn(DESKTOP_QUERY, ".composer-grow");
  assert.match(label, /display:\s*inline-flex/);
  assert.match(label, /grid-column:\s*1\b/);
  assert.match(label, /min-height:\s*24px/, "the Grow checkbox is a smaller target than WCAG 2.2's 24px");
  // A send card's one-line box wins: no checkbox drawn under it.
  for (const composer of ["voice-composer", "channel-composer"]) {
    assert.match(cssBlockIn(DESKTOP_QUERY, `#screen-main[data-draft] #${composer} > .composer-grow`), /display:\s*none/,
      `#${composer} draws Grow under a send card`);
  }
});

test("`#340 composer-grow-box`: THE BOX GROWS by the browser's own field-sizing, up to 60% of the window and scrolling beyond, on a desk alone and not under a send card", () => {
  for (const selector of [
    "#screen-main[data-composer-grow]:not([data-draft]) #compose-text",
    "#screen-main[data-composer-grow]:not([data-draft]) #channel-compose-text",
    "#screen-main[data-composer-grow] #side-compose-text",
  ]) {
    const rule = cssBlockIn(DESKTOP_QUERY, selector);
    assert.match(rule, /field-sizing:\s*content/, `${selector} does not size itself to its text`);
    assert.match(rule, /height:\s*var\(--composer-grow-height, auto\)/, `${selector} ignores the height measured for it`);
    assert.match(rule, /max-height:\s*60vh/, `${selector} is not capped at 60% of the window`);
    assert.match(rule, /overflow-y:\s*auto/, `${selector} does not scroll beyond its cap`);
    assert.equal(cssRulesElsewhere(DESKTOP_QUERY, selector), 0, `${selector} grows on a phone too`);
  }
  // The box at rest is as it was: the shared rule's height, and the call's box the channel's.
  assert.match(cssBlock("#channel-compose-text"), /min-height:\s*5rem/);
  assert.equal(cssBlock("#compose-text"), cssBlock("#channel-compose-text"));
});

test("`#340 composer-grow-box`: GROW IS OFF BY DEFAULT and not written down; ticked in one box it is ticked in all three, said on #screen-main and kept across a reload; storage that throws still honours it", async () => {
  const store = new Map();
  const page = newPage(store);
  await signIn(page);
  const screen = page.el("screen-main");
  for (const { grow } of GROWING) assert.equal(page.el(grow).checked, false, `#${grow} is ticked on a fresh device`);
  assert.equal(screen.hasAttribute("data-composer-grow"), false);
  assert.equal(store.has("vibe-talk.voice.composer-grow"), false, "the default was written down as a choice");
  await page.el("channel-compose-grow").setChecked(true);
  for (const { grow } of GROWING) assert.equal(page.el(grow).checked, true, `#${grow} does not say the choice made in another box`);
  assert.equal(screen.hasAttribute("data-composer-grow"), true, "ticking Grow did not tell the stylesheet");
  assert.equal(store.get("vibe-talk.voice.composer-grow"), "1", "ticking Grow was not kept");
  const again = newPage(store);
  for (const { grow } of GROWING) assert.equal(again.el(grow).checked, true, `a reload unticked #${grow}`);
  assert.equal(again.el("screen-main").hasAttribute("data-composer-grow"), true, "a reload let the boxes stop growing");
  await again.el("side-compose-grow").setChecked(false);
  assert.equal(store.get("vibe-talk.voice.composer-grow"), "0", "unticking Grow was not kept");
  for (const { grow } of GROWING) assert.equal(again.el(grow).checked, false);
  assert.equal(again.el("screen-main").hasAttribute("data-composer-grow"), false);
  // A browser that throws on every write still grows the boxes for this visit.
  const refusing = newPage();
  refusing.storage.set = () => {
    throw new Error("QuotaExceededError: the quota has been exceeded.");
  };
  await refusing.el("compose-grow").setChecked(true);
  assert.equal(refusing.el("screen-main").hasAttribute("data-composer-grow"), true, "storage that throws lost the choice");
  assert.equal(refusing.el("side-compose-grow").checked, true);
});

test("`#340 composer-grow-box`: WHERE THE BROWSER CANNOT SIZE THE BOX its height is measured and written, only on a desk, and taken away with Grow off; where it can, nothing is written", async () => {
  const height = (page, id) => page.el(id).style.getPropertyValue("--composer-grow-height");
  /** A page on a channel, Grow ticked, the channel's checkbox drawn (a desk) or not (a phone). */
  const growPage = async (desk, arrange = null) => {
    const page = newPage(new Map(), SCRIPT, arrange);
    await signIn(page);
    await showDiscord(page, tallChannel(10));
    if (desk) page.el("channel-compose-grow").getClientRects = () => [{ top: 0, bottom: 24 }];
    await page.el("channel-compose-grow").setChecked(true);
    return page;
  };
  const desk = await growPage(true);
  const box = desk.el("channel-compose-text");
  await box.setValue("line\n".repeat(40));
  assert.match(height(desk, "channel-compose-text"), /^\d+px$/, "a desk's growing box was given no height where the browser cannot size it");
  // A send that clears the box measures it again; Grow off takes the height away.
  box.style.setProperty("--composer-grow-height", "stale");
  await desk.el("channel-send").click();
  await desk.settle();
  assert.match(height(desk, "channel-compose-text"), /^\d+px$/, "the box cleared by a send was not measured again");
  await desk.el("channel-compose-grow").setChecked(false);
  assert.equal(height(desk, "channel-compose-text"), "", "Grow off left the box at a measured height");
  // A phone draws no checkbox, and its box is never measured.
  const phone = await growPage(false);
  await phone.el("channel-compose-text").setValue("line\n".repeat(40));
  assert.equal(height(phone, "channel-compose-text"), "", "a phone's box was given a height");
  // A browser with `field-sizing: content` sizes the box itself: nothing is written.
  const sizing = await growPage(true, (p) => p.supportFieldSizing());
  await sizing.el("channel-compose-text").setValue("line\n".repeat(40));
  assert.equal(height(sizing, "channel-compose-text"), "", "a browser that sizes the box itself was given a height");
});

test("`#340 composer-grow-box`: SEND STAYS IN VIEW as the box grows under the reader's typing, and nothing moves for a box without the focus", async () => {
  const page = newPage(new Map(), SCRIPT, (p) => p.enableResizeObserver());
  await signIn(page);
  await showDiscord(page, tallChannel(30));
  page.el("channel-compose-grow").getClientRects = () => [{ top: 0, bottom: 24 }];
  await page.el("channel-compose-grow").setChecked(true);
  const area = page.el("scroll-area");
  const box = page.el("channel-compose-text");
  const composer = page.el("channel-composer");
  const inView = () => {
    const rect = composer.getBoundingClientRect();
    return rect.top >= 0 && rect.bottom <= area.clientHeight;
  };
  // Without the focus: a box grown under a reader up the list moves nothing.
  area.scrollTop = 0;
  await area.dispatch("scroll");
  assert.equal(inView(), false, "the premise: the composer is out of view at the head of the list");
  page.resized(box);
  assert.equal(area.scrollTop, 0, "a box grown without the focus moved the list");
  // With it: the whole composer, Send included, brought into the list.
  box.focus();
  page.resized(box);
  assert.ok(inView(), `the composer was left out of view as its box grew: ${JSON.stringify(composer.getBoundingClientRect())}`);
  // Grow off: the box does not grow, and nothing here moves the list for it.
  await page.el("channel-compose-grow").setChecked(false);
  area.scrollTop = 0;
  box.focus();
  page.resized(box);
  assert.equal(area.scrollTop, 0, "with Grow off a resize of the box moved the list");
});

test("`#340 composer-grow-box`: A READER AT THE FOOT WHO PASTES AND THEN TICKS Grow stays at the foot as the box grows, the focus on the checkbox; one up the list is not moved", async () => {
  const page = newPage(new Map(), SCRIPT, (p) => p.enableResizeObserver());
  await signIn(page);
  await showDiscord(page, tallChannel(30));
  const screen = page.el("screen-main");
  const area = page.el("scroll-area");
  const box = page.el("channel-compose-text");
  const grow = page.el("channel-compose-grow");
  grow.getClientRects = () => [{ top: 0, bottom: 24 }];
  // On a desk the checkbox is drawn on Send's row, so it stands where Send does. (The layout tree leaves
  // its label out: the word Grow would take a line of its own here, under Send.)
  grow.getBoundingClientRect = () => page.el("channel-send").getBoundingClientRect();
  // What the stylesheet does with a long draft: the box at rest is three lines, and ticked it grows to its cap.
  box.height = () => (screen.hasAttribute("data-composer-grow") ? 400 : 80);
  const gap = () => area.scrollHeight - area.scrollTop - area.clientHeight;
  box.focus();
  await box.setValue("line\n".repeat(40));
  page.resized(box);
  area.scrollTop = area.scrollHeight;
  await area.dispatch("scroll");
  assert.equal(gap(), 0, "the premise: the reader is at the foot of the list with the pasted draft");
  // A click or Space on the checkbox puts the focus there, and ticks it.
  grow.focus();
  await grow.setChecked(true);
  page.resized(box);
  assert.ok(atBottomOf(area), `ticking Grow after a paste left the reader ${gap()}px above the foot`);
  // From up the list, the checkbox scrolled out of view with the focus still on it, Space unticks and
  // ticks it again: nothing moves.
  await grow.setChecked(false);
  page.resized(box);
  area.scrollTop = 0;
  await area.dispatch("scroll");
  assert.ok(grow.getBoundingClientRect().top > area.clientHeight, "the premise: the checkbox is out of view below the list");
  await grow.setChecked(true);
  page.resized(box);
  assert.equal(area.scrollTop, 0, "ticking Grow moved a reader up the list");
});

test("`#340 composer-grow-box`: A READER A LITTLE ABOVE THE FOOT, past its slack, WHO TICKS Grow after a paste keeps the checkbox where it was as the box grows, and the room under it, so Send and Grow stay on screen, whether the tick focused the checkbox or, as a click in Safari or Firefox on a Mac does, nothing", async () => {
  const page = newPage(new Map(), SCRIPT, (p) => p.enableResizeObserver());
  await signIn(page);
  await showDiscord(page, tallChannel(30));
  const screen = page.el("screen-main");
  const area = page.el("scroll-area");
  const box = page.el("channel-compose-text");
  const grow = page.el("channel-compose-grow");
  grow.getClientRects = () => [{ top: 0, bottom: 24 }];
  grow.getBoundingClientRect = () => page.el("channel-send").getBoundingClientRect();
  box.height = () => (screen.hasAttribute("data-composer-grow") ? 400 : 80);
  // What a browser draws under Send's row: the composer's padding and the list's, which at 1280x800
  // let the Grow label show for a reader up to about 47px above the foot. Past the slack that counts
  // as the foot, then, Send and Grow can still be on screen, and the click lands there.
  page.el("channel-compose-state").height = () => 50;
  const gap = () => area.scrollHeight - area.scrollTop - area.clientHeight;
  box.focus();
  await box.setValue("line\n".repeat(40));
  page.resized(box);
  // Space, and a click in Chromium, focus the checkbox they tick. A click in Safari or Firefox on a Mac
  // focuses no checkbox: the focus leaves the box for nothing, which a browser reports as the body.
  const ticks = [BOTTOM_SLACK_PX + 6, BOTTOM_SLACK_PX + 16].flatMap((up) => [
    { up, focus: "the checkbox" }, { up, focus: "nothing" },
  ]);
  for (const { up, focus } of ticks) {
    const where = `${up}px above the foot with the focus on ${focus}`;
    assert.equal(grow.checked, false, `the premise: Grow is off (${where})`);
    area.scrollTop = area.scrollHeight - area.clientHeight - up;
    await area.dispatch("scroll");
    assert.ok(gap() === up && !atBottomOf(area), `the premise: the reader is ${up}px above the foot, not at it: ${gap()}`);
    const seen = grow.getBoundingClientRect();
    assert.ok(seen.top >= 0 && seen.bottom <= area.clientHeight, `the premise: the checkbox is on screen (${where})`);
    box.focus();
    if (focus === "the checkbox") grow.focus();
    else box.blur();
    assert.equal(page.document.activeElement, focus === "the checkbox" ? grow : null, `the premise: the focus is on ${focus}`);
    await grow.setChecked(true);
    page.resized(box);
    assert.equal(grow.getBoundingClientRect().top, seen.top,
      `ticking Grow ${where} moved the checkbox just ticked from ${seen.top} to ${grow.getBoundingClientRect().top}`);
    assert.equal(gap(), up, `ticking Grow ${where} left the reader ${gap()}px above the foot`);
    await grow.setChecked(false);
    page.resized(box);
  }
});

// --- `#403 composer-key-global` -------------------------------------------------------------------
//
// The owner, 2026-10-21, on a desk in two columns: "I was just trying 'Cmd-s' to select the message
// entry box... it didn't work. E.g. when a message in the right column is highlighted and focus is
// elsewhere. But if focus was in the left column it worked. It seemed selective. It should be globally
// applicable." The rule Help states: the box of the column the focus is in; with the focus in neither
// (the bar, the page itself) the box of the column holding the selection; Main's from the thread cards,
// which have none. A reply open at the foot of a column is that column's box. The thread list, like
// Settings and Help, is left first. tests/composer_key_browser.py presses the chord in real Chromium.

/** Ctrl+S, or with `fields` Cmd+S, from `target` (null for the page body): it must be refused to the browser. */
async function chordFrom(page, target, fields, where) {
  const event = await press(page, target, "s", fields);
  assert.equal(event.defaulted, false, `${where}: the chord was left to the browser, whose Save page it is`);
  assert.equal(page.el("search-field").hidden, true, `${where}: the chord opened the search`);
  return event;
}

const CHORDS = [{ ctrlKey: true }, { metaKey: true }];
const chordName = (fields) => (fields.metaKey ? "Cmd+S" : "Ctrl+S");

test("`#403 composer-key-global`: in two columns, from the right column's THREAD CARDS, Ctrl+S and Cmd+S go to Main's box", async () => {
  // Before the fix the right column, which has no box on its cards, was asked for its own, and nothing
  // moved: from a card, a highlighted card, the list, its Unread only (now its Unpack msgs, #432), or the body after the cards.
  const cases = [
    ["a card", async (page) => cardOf(page, cardIds(page)[0])],
    ["a highlighted card", async (page) => {
      await cardOf(page, cardIds(page)[0]).click();
      await page.settle();
      assert.deepStrictEqual(litCards(page), [cardIds(page)[0]], "the premise is a highlighted card");
      return cardOf(page, cardIds(page)[0]);
    }],
    ["the right column's list", async (page) => page.el("side-scroll")],
    ["Unpack msgs", async (page) => page.el("side-unpack")],
    ["the page body, the selection on a card", async (page) => {
      await press(page, page.el("side-scroll"), "ArrowDown");
      return null;
    }],
  ];
  for (const fields of CHORDS) {
    for (const [where, from] of cases) {
      const what = `${chordName(fields)} from ${where}`;
      const { page } = await twoColumnServer();
      assert.equal(page.el("side-pane").hidden, true, "the premise is the right column on its cards");
      await chordFrom(page, await from(page), fields, what);
      assert.equal(focused(page), page.el("channel-compose-text"), `${what} left the focus on ${focusName(page)}`);
      assert.ok(inTwoColumns(page), `${what} left two columns`);
    }
  }
});

test("`#403 composer-key-global`: on the THREAD LIST, Ctrl+S and Cmd+S leave it for the view it was chosen from, and its box", async () => {
  for (const fields of CHORDS) {
    for (const where of ["a card", "the list", "the page body"]) {
      const what = `${chordName(fields)} from ${where} on the thread list`;
      const page = await threadPage();
      const view = page.el("thread-select").value;
      await press(page, page.el("scroll-area"), "3", { ctrlKey: true });
      await page.settle();
      assert.equal(page.el("thread-select").value, "threads", "the premise is the thread list");
      const target = where === "a card" ? cardOf(page, cardIds(page)[0]) : where === "the list" ? page.el("side-scroll") : null;
      await chordFrom(page, target, fields, what);
      await page.settle();
      assert.equal(page.el("thread-select").value, view, `${what} did not go back to ${view}`);
      assert.equal(focused(page), page.el("channel-compose-text"), `${what} left the focus on ${focusName(page)}`);
    }
  }
});

test("`#403 composer-key-global`: with a thread open on the right, the chord goes to the box of the column the focus or the selection is in", async () => {
  const side = "side-compose-text";
  const main = "channel-compose-text";
  const cases = [
    ["the right column's selected message", side, async (page) => {
      await press(page, page.el("side-scroll"), "ArrowDown");
      assert.ok(nodeIn(page.el("side-log"), focused(page)), `the premise is a selected message on the right, not ${focusName(page)}`);
      return focused(page);
    }],
    ["the left column's selected message", main, async (page) => {
      await press(page, page.el("scroll-area"), "ArrowUp");
      assert.ok(nodeIn(page.el("discord-log"), focused(page)), `the premise is a selected message on the left, not ${focusName(page)}`);
      return focused(page);
    }],
    ["a message's Reply button on the right", side, async (page) => replyButton(sideRow(page, "202"))],
    ["a message's Reply button on the left", main, async (page) => replyButton(rowWithId(page, "200"))],
    ["the right column's X", side, async (page) => page.el("side-close")],
    ["the page body, the selection on the right", side, async (page) => {
      await press(page, page.el("side-scroll"), "ArrowDown");
      return null;
    }],
    ["the page body, the selection on the left", main, async (page) => {
      await press(page, page.el("scroll-area"), "ArrowUp");
      return null;
    }],
    ["the search glass, the selection on the right", side, async (page) => {
      await press(page, page.el("side-scroll"), "ArrowDown");
      return page.el("search-toggle");
    }],
    ["the search glass, the selection on the left", main, async (page) => {
      await press(page, page.el("scroll-area"), "ArrowUp");
      return page.el("search-toggle");
    }],
  ];
  for (const fields of CHORDS) {
    for (const [where, box, from] of cases) {
      const what = `${chordName(fields)} from ${where}`;
      const { page } = await replyColumnsPage();
      await chordFrom(page, await from(page), fields, what);
      assert.equal(focused(page), page.el(box), `${what} left the focus on ${focusName(page)}`);
    }
  }
});

test("`#403 composer-key-global`: a reply open in a column is the one box, from either column", async () => {
  // Before `#403` the reply took the chord from both columns while the right column's own box was drawn
  // beside it, which sent a reader in the right column's thread past their own box. Since `#410
  // inline-reply-editor` a reply open anywhere is the one box drawn, so it is where the chord goes.
  for (const fields of CHORDS) {
    const { page } = await replyColumnsPage();
    await replyButton(rowWithId(page, "200")).click();
    await page.settle();
    assert.equal(replyBoxAt(page), "left", "the premise is a reply open in the left column");
    // `#410 inline-reply-editor`. The right column's own box is not drawn while the reply is open: the one
    // box on the page is the reply's, from either column.
    await press(page, page.el("side-scroll"), "ArrowDown");
    await chordFrom(page, focused(page), fields, `${chordName(fields)} from the right column`);
    assert.equal(focused(page), page.el("reply-text"),
      `${chordName(fields)} from the right column's thread left the focus on ${focusName(page)}`);
    await press(page, page.el("scroll-area"), "ArrowUp");
    await chordFrom(page, focused(page), fields, `${chordName(fields)} from the left column`);
    assert.equal(focused(page), page.el("reply-text"), `${chordName(fields)} from the left column left the focus on ${focusName(page)}`);
    await chordFrom(page, null, fields, `${chordName(fields)} from the page body`);
    assert.equal(focused(page), page.el("reply-text"), `${chordName(fields)} from the body after the left left the focus on ${focusName(page)}`);
    // `#410 inline-reply-editor`. Neither column's own box is drawn beside the reply: one box.
    assert.equal(page.el("channel-composer").hidden, true, "Main's box is drawn beside the reply open in its column");
    assert.equal(page.el("side-composer").hidden, true, "the right column's box is drawn beside a reply on the left");
  }
});

test("`#403 composer-key-global`: the chord is heard before any element's own listener, so one that stops it keeps nothing from the box", async () => {
  // Before the fix the chord was the document's, and a listener on the card that stopped it left the
  // reader on the card, the Save page refused but nothing else done.
  const { page } = await twoColumnServer();
  const card = cardOf(page, cardIds(page)[0]);
  card.addEventListener("keydown", (event) => {
    event.stopPropagation();
  });
  await chordFrom(page, card, { ctrlKey: true }, "Ctrl+S on a card whose listener stops the key");
  assert.equal(focused(page), page.el("channel-compose-text"), `a stopped Ctrl+S left the focus on ${focusName(page)}`);
  // A held chord: the box once, every repeat refused.
  const repeat = await chordFrom(page, focused(page), { ctrlKey: true, repeat: true }, "a held Ctrl+S's repeat");
  assert.equal(repeat.defaulted, false);
  assert.equal(focused(page), page.el("channel-compose-text"));
});

test("`#403 composer-key-global` negative control: IN A TEXT FIELD the chord still goes to the message box and never saves the page", async () => {
  for (const platform of ["Linux x86_64", "MacIntel"]) {
    for (const fields of CHORDS) {
      const what = `${chordName(fields)} on ${platform}`;
      // The search field, the selection on the right: the right column's box; its words kept.
      const { page } = await twoColumnServer((p) => p.setPlatform(platform));
      await threadBadge(rowWithId(page, "201")).click();
      await page.settle();
      await press(page, page.el("side-scroll"), "ArrowDown");
      await page.el("search-toggle").click();
      await page.settle();
      assert.equal(focused(page), page.el("search-field"), "the premise is the focus in the search field");
      await page.el("search-field").setValue("engine");
      let event = await press(page, page.el("search-field"), "s", fields);
      assert.equal(event.defaulted, false, `${what} in the search field was left to the browser's Save page`);
      assert.equal(page.el("search-field").value, "engine", `${what} threw the search away`);
      assert.equal(focused(page), page.el("side-compose-text"), `${what} in the search field left the focus on ${focusName(page)}`);
      // In each column's box, with a draft: it stays, the draft untouched.
      for (const box of ["channel-compose-text", "side-compose-text"]) {
        await page.el(box).setValue("half a thought");
        event = await press(page, page.el(box), "s", fields);
        assert.equal(event.defaulted, false, `${what} in #${box} was left to the browser's Save page`);
        assert.equal(focused(page), page.el(box), `${what} in #${box} moved the focus to ${focusName(page)}`);
        assert.equal(page.el(box).value, "half a thought", `${what} in #${box} touched the draft`);
      }
    }
  }
});

// --- `#422 composer-key-stay` ---------------------------------------------------------------------------
//
// The owner, 2026-10-22: "when I press Cmd-S even if I have Main messages view on, it jumps to a DIFFERENT
// view such as All (Cmd 2) or Two column (Cmd 4). That's unexpected. I would want to keep my view and just
// enter the messages here." The rule in every view: the chord never changes the view when the view on
// screen has a box. The thread list has one while Main channel is on (the main channel's, as its last
// card) or a reply is open among its cards.

test("`#422 composer-key-stay`: in the Threads view with Main channel on, Ctrl+S and Cmd+S go to the box at the foot of the cards and keep the view", async () => {
  for (const fields of CHORDS) {
    for (const where of ["a card", "a main-channel message", "the list", "the page body"]) {
      const what = `${chordName(fields)} from ${where}`;
      const { page } = await mainMessagesPage(new Map([[THREADS_MAIN_STORAGE, "on"]]));
      assert.equal(page.el("channel-composer").parentNode, page.el("side-list"), "the premise is the box at the foot of the cards");
      const target = where === "a card" ? page.el("side-cards").children.find((li) => li.hasAttribute("data-card"))
        : where === "a main-channel message" ? page.el("side-cards").children.find((li) => li.hasAttribute("data-main-row"))
          : where === "the list" ? page.el("side-scroll") : null;
      await chordFrom(page, target, fields, what);
      await page.settle();
      assert.equal(page.el("screen-main").getAttribute("data-columns"), "list", `${what} left the Threads view`);
      assert.equal(page.el("thread-select").value, "threads", `${what} changed the view`);
      assert.equal(focused(page), page.el("channel-compose-text"), `${what} left the focus on ${focusName(page)}`);
      assert.equal(page.el("channel-composer").parentNode, page.el("side-list"), `${what} took the box from the foot of the cards`);
    }
  }
});

test("`#422 composer-key-stay`: a reply open among the cards is the box the chord goes to, the Threads view kept, with Main channel on or off", async () => {
  for (const main of ["on", "off"]) {
    const { page } = await mainMessagesPage(new Map([[THREADS_MAIN_STORAGE, "on"]]));
    const lone = page.el("side-cards").children.find((li) => li.getAttribute("data-id") === "200");
    await replyButton(lone).click();
    await page.settle();
    assert.deepStrictEqual(inlineAnchors(page), ["side-cards:200"], "the premise is a reply open among the cards");
    if (main === "off") {
      // Main channel let go keeps the reply only where its row stays: here it goes with the row, so the
      // reply is opened again on an unpacked row instead.
      await page.el("reply-cancel").click();
      await page.el("side-main").click();
      await page.el("side-unpack").click();
      await page.settle();
      const unpacked = page.el("side-cards").children.find((li) => !li.hasAttribute("data-card") && !li.hasAttribute("data-main-row") && !li.hidden);
      assert.ok(unpacked, "the premise is an unpacked row under a card");
      await replyButton(unpacked).click();
      await page.settle();
      assert.equal(page.el("side-reply").hidden, false, "the premise is a reply open under an unpacked row");
    }
    await chordFrom(page, page.el("side-scroll"), { metaKey: true }, `Cmd+S, Main channel ${main}`);
    await page.settle();
    assert.equal(page.el("screen-main").getAttribute("data-columns"), "list", `Main channel ${main}: Cmd+S left the Threads view`);
    assert.equal(focused(page), page.el("reply-text"), `Main channel ${main}: Cmd+S left the focus on ${focusName(page)}`);
  }
});

test("`#422 composer-key-stay` negative control: with Main channel off and no reply, the Threads view has no box, and the chord still goes back to the view it was chosen from", async () => {
  const { page } = await mainMessagesPage();
  assert.equal(page.el("side-main").getAttribute("aria-pressed"), "false", "the premise is Main channel off");
  await chordFrom(page, page.el("side-scroll"), { metaKey: true }, "Cmd+S, Main channel off");
  await page.settle();
  assert.notEqual(page.el("screen-main").getAttribute("data-columns"), "list", "with no box, Cmd+S stayed on the Threads view");
  assert.equal(focused(page), page.el("channel-compose-text"));
});

test("`#422 composer-key-stay`: in Main, All, a thread and two columns, each with its box, the chord keeps the view", async () => {
  for (const view of ["main", "flat"]) {
    const page = await threadPage(view);
    await chordFrom(page, page.el("scroll-area"), { metaKey: true }, `Cmd+S in ${view}`);
    assert.equal(page.el("thread-select").value, view, `Cmd+S changed the view from ${view}`);
    assert.equal(focused(page), page.el("channel-compose-text"));
  }
  const page = await threadPage("main");
  const thread = page.el("thread-select").children.find((one) => String(one.value).startsWith("thread:"));
  await pickThread(page, thread.value);
  await chordFrom(page, page.el("scroll-area"), { metaKey: true }, "Cmd+S in a thread");
  assert.equal(page.el("thread-select").value, thread.value, "Cmd+S left the thread");
  assert.equal(focused(page), page.el("channel-compose-text"));
  const { page: two } = await twoColumnServer();
  await chordFrom(two, two.el("side-scroll"), { metaKey: true }, "Cmd+S in two columns");
  assert.ok(inTwoColumns(two), "Cmd+S left two columns");
});
