// The app page's tests: the keyboard: shortcuts and the keymap, one selection, copy, undo, and the search key.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/keyboard.mjs.

import assert from "node:assert/strict";

import { CHANNEL, CSS_CODE, DISCORD_POLL_MS, DONE_FLASH_FADE_MS, DONE_FLASH_HOLD_MS, HTML,
  HTML_CODE, LINE_PX, PAGE_ELEMENTS, RUN_TAIL, SCRIPT, SCRIPT_CODE, SCROLL_VIEWPORT_PX,
  SHORT_MESSAGE, UNPACK_STORAGE, asksFor, atBottomOf, backlog, bid, bodyOf, bookmarkAt, bookmarkRow,
  brokenScript, brokenScriptEach, builtSince, cardIds, cardRow, channelRow, chooseReadMode, compose,
  cssBlock, cssRules, deliver, doneButton, elidedOf, enterRunThread, failedSend, fillTranscript,
  focusName, focusOn, focused, foldButton, forwardPage, forwardTimeline, heldRow, helpEntry,
  inlineAnchors, joiningChannel, json, keyOn, keyboardMedia, keyboardPage, keyboardPageAtTop,
  keyboardSaid, litCards, litIds, litRows, longMessage, markupHolds, message, mouseOnto, mouseTo,
  newPage, nodeIn, nodeWithinFixture, openReplyOn, openThree, openerOf, outgoingDismiss,
  outgoingRows, parityPage, pickThread, pollOnce, press, pressCopy, pressHere, pressOn,
  questionFarAbove, reReadChannel, readRunLineOf, readThroughButton, refreshDiscord,
  reopenedChannel, replyArrowOf, replyBoxAt, replyButton, replyInStream, rowHoldingId,
  rowMoreButton, rowMoreMenu, rowStates, rowWithId, runThread, scaleOf, scopedData, selectedId,
  serveScoped, servedBookmarkPage, serverFloor, showDiscord, shownIds, sideIds, sideRow, signIn,
  sseMessage, startTalking, statusText, summaryText, tallChannel, tapArrow, tapBookmarkClear, test,
  threadBadge, threadData, threadHistory, threadPage, throughHere, turnSummariesOn, turnTodoOn,
  twoColumnServer, twoColumns, typedFrames, unpackPage, unpackRows, unpackedRow } from "./fixture.mjs";

test("`#429 keyboard-hides-bar`: both rows go while the keyboard is up, only on a touch screen, and a way out of sound stays", () => {
  const { query, body, start, end } = keyboardMedia();
  assert.equal(query, "(pointer: coarse)", "the bars can go on a desk, whatever the script decides");
  const outside = CSS_CODE.slice(0, start) + CSS_CODE.slice(end);
  assert.doesNotMatch(outside, /data-keyboard/, "a keyboard rule outside the touch-screen block");
  // The channel view only: the call's view keeps its dock over the keyboard (`#241 voice-composer`).
  for (const rule of body.matchAll(/([^{}]+)\{[^{}]*\}/g)) {
    for (const selector of rule[1].split(/,\s*(?=:root)/)) {
      assert.match(selector.trim(), /^:root\[data-keyboard="open"\]:has\(#pane-discord:not\(\[hidden\]\)\) /,
        `a keyboard rule reaches past the channel view: ${selector.trim()}`);
    }
  }
  // Both rows: the control bar wherever it is mounted, and the dock with nothing in it to stop.
  assert.match(cssRules(body, ':root[data-keyboard="open"]:has(#pane-discord:not([hidden])) #control-bar').join("\n"), /display:\s*none/,
    "the control bar stays up over the keyboard");
  // (Read whole: `cssRules` splits a selector list at its commas, and this one has a comma inside `:has()`.)
  assert.match(body,
    /:root\[data-keyboard="open"\]:has\(#pane-discord:not\(\[hidden\]\)\) #dock:not\(:has\(#read-aloud\[data-active="true"\]:not\(\[hidden\]\), #hang-up:not\(\[hidden\]\)\)\) \{\s*display:\s*none;\s*\}/,
    "the dock stays up over the keyboard with nothing playing");
  // The header the bar was moved to goes with it, but never Settings' way back.
  assert.match(body, /#topbar:has\(#control-bar-top > #control-bar\):not\(:has\(#close-settings:not\(\[hidden\]\), #close-help:not\(\[hidden\]\)\)\)/);
  // A reading or a call keeps one slim row: its Stop or Hang up, and nothing else of the tile. A
  // read playing makes Read its Pause, and Stop (`#442 read-strip-order`, hidden otherwise) stays.
  assert.match(body,
    /:root\[data-keyboard="open"\]:has\(#pane-discord:not\(\[hidden\]\)\) #control-pane > :is\(\.control, \.popover\):not\(#hang-up, #read-aloud\[data-active="true"\], #read-aloud-stop\) \{\s*display:\s*none;\s*\}/,
    "the rest of the tile stays up beside Stop");
  assert.match(body,
    /:root\[data-keyboard="open"\]:has\(#pane-discord:not\(\[hidden\]\)\) #control-pane:has\(#read-aloud-stop:not\(\[hidden\]\)\) \{\s*grid-template-columns:\s*1fr 1fr;\s*\}/,
    "a read playing leaves the slim row without Stop beside its Pause");
  assert.match(cssRules(body, ':root[data-keyboard="open"]:has(#pane-discord:not([hidden])) #control-pane').join("\n"), /grid-template-rows:\s*2\.75rem/);
  // Nothing the box needs is in either bar: Send and Grow are the composers', in the list.
  for (const id of ["channel-send", "send-text", "reply-send"]) {
    if (!PAGE_ELEMENTS.has(id)) continue;
    assert.ok(HTML.indexOf(`id="${id}"`) < HTML.indexOf('<footer id="dock">'), `${id} is in the dock, and would go with it`);
  }
});

test("`#429 keyboard-hides-bar`: a focused box and a visual viewport a keyboard shorter is the keyboard up; blur or the keyboard going brings the bars back", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  const box = page.el("channel-compose-text");
  assert.notEqual(keyboardSaid(page), "open", "the keyboard is up before anything was focused");

  // Safari: the visual viewport shrinks, the layout does not.
  await focusOn(page, box);
  assert.notEqual(keyboardSaid(page), "open", "a focused box was taken for a keyboard before the viewport shrank");
  await page.resizeViewport({ visual: 480 });
  assert.equal(keyboardSaid(page), "open", "a keyboard taking 372px of the visual viewport went unnoticed");
  // The box losing the focus brings the bars back, whatever the viewport says yet: once the tap that took
  // the focus is over, and not inside it, where Chromium's click would land on whatever moved under the
  // finger (tests/keyboard_hides_bar_browser.py measured Stop's click landing on the tile around it).
  assert.equal([...page.timers.values()].filter((timer) => timer.ms === 0).length, 0,
    "the premise: nothing the page decides once a tap is over is waiting");
  await page.moveFocus(null);
  assert.equal(keyboardSaid(page), "open", "the bars came back inside the tap that took the focus");
  page.expireTimers(0);
  assert.equal(keyboardSaid(page), "closed", "the bars stayed away after the box lost the focus");
  await page.resizeViewport({ visual: 852 });

  // Chrome on Android, told to resize the content: the layout viewport shrinks with the visual one, so
  // comparing the two would see nothing.
  await focusOn(page, box);
  await page.resizeViewport({ visual: 520, layout: 520 });
  assert.equal(keyboardSaid(page), "open", "a keyboard that resized the layout too went unnoticed");
  // The keyboard dismissed with the box still focused (Android's Back): the bars come back.
  await page.resizeViewport({ visual: 852, layout: 852 });
  assert.equal(keyboardSaid(page), "closed", "the bars stayed away after the keyboard went down");
  assert.equal(page.document.activeElement, box, "the premise: the box still has the focus");
});

test("`#429 keyboard-hides-bar`: a focused field with nothing taken from the viewport is no keyboard — a desk, or a keyboard plugged in", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  for (const id of ["channel-compose-text", "search-field"]) {
    await focusOn(page, page.el(id));
    assert.notEqual(keyboardSaid(page), "open", `focus in ${id} alone was taken for an on-screen keyboard`);
  }
  // A browser's toolbar sliding away, a pinch zoom, and a window made shorter with nothing focused.
  await page.resizeViewport({ visual: 772 });
  assert.notEqual(keyboardSaid(page), "open", "80px of toolbar was taken for a keyboard");
  await page.resizeViewport({ visual: 426, scale: 2 });
  assert.notEqual(keyboardSaid(page), "open", "a pinch zoom was taken for a keyboard");
  await page.resizeViewport({ visual: 852, scale: 1 });
  await focusOn(page, null);
  await page.resizeViewport({ visual: 400, layout: 400 });
  assert.notEqual(keyboardSaid(page), "open", "a shorter window with nothing focused was taken for a keyboard");
  // A checkbox is no field: Grow, focused by its click, with the viewport short against the screen.
  await focusOn(page, page.el("channel-compose-grow"));
  assert.notEqual(keyboardSaid(page), "open", "a focused checkbox was taken for a keyboard");
  // The window made shorter with nothing to type in (split screen, a freeform window) is the window from
  // then on: the box focused in it, with a keyboard plugged in, is no keyboard on the screen.
  await focusOn(page, null);
  await focusOn(page, page.el("channel-compose-text"));
  assert.notEqual(keyboardSaid(page), "open", "a window made shorter before the box was focused was taken for a keyboard");
  // ...and a keyboard coming up in it still is one.
  await page.resizeViewport({ visual: 220, layout: 220 });
  assert.equal(keyboardSaid(page), "open", "a keyboard in the shorter window went unnoticed");
});

test("`#429 keyboard-hides-bar`: a keyboard is both 150px and a fifth of the screen, so neither a tall screen's toolbar nor a short one's is one", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  await focusOn(page, page.el("channel-compose-text"));
  // 160px of an 852px screen: past 150px, short of a fifth (170px).
  await page.resizeViewport({ visual: 692 });
  assert.notEqual(keyboardSaid(page), "open", "160px of a tall screen was taken for a keyboard");
  await page.resizeViewport({ visual: 680 });
  assert.equal(keyboardSaid(page), "open", "172px of the screen, a keyboard's, went unnoticed");
  await page.resizeViewport({ visual: 852 });
  // A landscape screen 393px tall: 100px is past a fifth (79px) and short of 150px.
  await focusOn(page, null);
  page.documentElement.clientWidth = 852;
  await page.resizeViewport({ visual: 393, layout: 393 });
  await focusOn(page, page.el("channel-compose-text"));
  await page.resizeViewport({ visual: 293, layout: 293 });
  assert.notEqual(keyboardSaid(page), "open", "100px of a short screen was taken for a keyboard");
  await page.resizeViewport({ visual: 213, layout: 213 });
  assert.equal(keyboardSaid(page), "open", "180px of a landscape screen, a keyboard's, went unnoticed");
});

test("`#429 keyboard-hides-bar`: the focus moving from the box to the search keeps the bars away, with no flash between", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  await focusOn(page, page.el("channel-compose-text"));
  await page.resizeViewport({ visual: 480 });
  assert.equal(keyboardSaid(page), "open");
  const said = [];
  const set = page.documentElement.setAttribute.bind(page.documentElement);
  page.documentElement.setAttribute = (name, value) => {
    if (name === "data-keyboard") said.push(value);
    return set(name, value);
  };
  await focusOn(page, page.el("search-field"));
  assert.deepEqual(said, [], "the bars came back for a moment between two fields");
  assert.equal(keyboardSaid(page), "open");
});

test("`#429 keyboard-hides-bar`: a reading playing as the keyboard comes up goes on playing, Stop still on screen", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" }), message({ id: "1000000000000000002" })]);
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(page.el("read-aloud").getAttribute("data-active"), "true", "the premise: reading is on");
  const playing = page.players.filter((player) => !player.paused).length;

  await focusOn(page, page.el("channel-compose-text"));
  await page.resizeViewport({ visual: 480 });
  assert.equal(keyboardSaid(page), "open");
  assert.equal(page.el("read-aloud").getAttribute("data-active"), "true", "the keyboard stopped the reading");
  assert.equal(page.el("read-aloud-label").text(), "Stop", "the way out no longer says it is one");
  assert.equal(page.el("read-aloud").hidden, false, "Stop went with the bars");
  assert.equal(page.players.filter((player) => !player.paused).length, playing, "the keyboard paused the audio");
  // And Stop, pressed with the keyboard up, stops it.
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(page.el("read-aloud").getAttribute("data-active"), "false", "Stop did nothing with the keyboard up");
});

test("`#429 keyboard-hides-bar`: the pace popover, whose button has gone, closes as the keyboard comes up", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  await page.el("read-speed").click();
  await page.settle();
  assert.equal(page.el("speed-popover").hidden, false, "the premise: the popover is open");
  await focusOn(page, page.el("channel-compose-text"));
  await page.resizeViewport({ visual: 480 });
  assert.equal(page.el("speed-popover").hidden, true, "a popover was left open over the keyboard");
  assert.equal(page.el("read-speed").getAttribute("aria-expanded"), "false");
});

test("`#429 keyboard-hides-bar`: where the browser reports the keyboard (the VirtualKeyboard API), that is heard too", async () => {
  const page = newPage(new Map(), SCRIPT, (p) => p.useVirtualKeyboard());
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  await focusOn(page, page.el("channel-compose-text"));
  await page.setVirtualKeyboard(320);
  assert.equal(keyboardSaid(page), "open", "a keyboard the browser reported went unheard");
  await page.setVirtualKeyboard(0);
  assert.equal(keyboardSaid(page), "closed", "the bars stayed away after the browser reported the keyboard gone");
  // Reported with nothing focused, it is not the reader writing here.
  await focusOn(page, null);
  await page.setVirtualKeyboard(320);
  assert.notEqual(keyboardSaid(page), "open");
});

test("`#429 keyboard-hides-bar`: a turned phone measures its tallest again, so the shorter landscape screen is no keyboard", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  await focusOn(page, page.el("channel-compose-text"));
  page.documentElement.clientWidth = 852;
  await page.resizeViewport({ visual: 393, layout: 393 });
  assert.notEqual(keyboardSaid(page), "open", "turning the phone was taken for a keyboard");
  await page.resizeViewport({ visual: 180, layout: 180 });
  assert.equal(keyboardSaid(page), "open", "a keyboard on the turned phone went unnoticed");
});

test("`#429 keyboard-hides-bar`: turned with the keyboard up, the bars stay away until the keyboard goes, at a width never seen without it", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  page.documentElement.clientWidth = 393;
  await page.resizeViewport({ visual: 852, layout: 852 });
  await focusOn(page, page.el("channel-compose-text"));
  await page.resizeViewport({ visual: 516, layout: 516 });
  assert.equal(keyboardSaid(page), "open");
  // Turned: 852 wide, the keyboard still up, 193px left. This width has never been seen without it.
  page.documentElement.clientWidth = 852;
  await page.resizeViewport({ visual: 193, layout: 193 });
  assert.equal(keyboardSaid(page), "open", "turning the phone with the keyboard up brought the bars back");
  // Turned back, the keyboard still up: portrait's own tallest says so.
  page.documentElement.clientWidth = 393;
  await page.resizeViewport({ visual: 516, layout: 516 });
  assert.equal(keyboardSaid(page), "open", "turning back with the keyboard up brought the bars back");
  // Landscape again, then the keyboard down: the height grows by a keyboard's, and the bars come back.
  page.documentElement.clientWidth = 852;
  await page.resizeViewport({ visual: 193, layout: 193 });
  assert.equal(keyboardSaid(page), "open");
  await page.resizeViewport({ visual: 393, layout: 393 });
  assert.equal(keyboardSaid(page), "closed", "the keyboard went down on the turned phone and the bars stayed away");
});

test("READ OWNS THE THUMB POSITION, and placement is explicit so DOM order cannot decide it", async () => {
  // All three bar controls carried `grid-row: 1 / span 2` and no column, so the grid auto-placed
  // them in DOM order: Read into column 1 — the narrow 3.5rem strip meant for Sound and Clear —
  // and Pace into column 3, the easiest place on a phone to reach. #94 read-aloud-bar-layout.
  assert.match(cssBlock("#control-pane #read-aloud"), /grid-column:\s*3/, "Read is not on the thumb");
  assert.match(cssBlock("#control-pane #read-speed"), /grid-column:\s*2/, "Pace is not beside Read");
  assert.match(cssBlock("#control-pane #todo-filter"), /grid-column:\s*1/, "the filter took a big tile");
});

test("the pace popover is anchored to something, so pressing Pace actually shows it", async () => {
  // It was `position: absolute` with no positioned ancestor, so it resolved against the initial
  // containing block: `bottom: calc(100% + 0.4rem)` put it a viewport height above the page. The
  // handler ran, the element unhid, and there was nothing to see. #94.
  assert.match(
    cssBlock("#control-pane"),
    /position:\s*relative/,
    "the popover has no positioned ancestor, so it lands off screen"
  );

  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  assert.equal(page.el("speed-popover").hidden, true, "it ships open");
  await page.el("read-speed").click();
  await page.settle();
  assert.equal(page.el("speed-popover").hidden, false, "pressing Pace did not open it");
  assert.equal(page.el("read-speed").getAttribute("aria-expanded"), "true");
});

const scrollToBottom = (area) => {
  area.scrollTop = area.scrollHeight;
};

test("EVERY LIST CAN HOLD THE FOCUS, from script and a click but never as a Tab stop, and draws no ring", () => {
  // `#281 view-keys`: and the thread list's, the right column's scroller, as in two columns.
  for (const id of ["scroll-area", "side-scroll", "screen-signin", "screen-settings", "screen-help"]) {
    const tag = new RegExp(`<[a-z]+ id="${id}"[^>]*>`).exec(HTML_CODE);
    assert.ok(tag, `#${id} is not in the markup`);
    assert.match(tag[0], /\btabindex="-1"/, `#${id} cannot hold the focus, so the scroll keys cannot reach it`);
  }
  // Not a Tab stop anywhere: a list reached by Tab would put a stop the reader has to pass on every screen.
  assert.doesNotMatch(HTML_CODE, /id="(scroll-area|side-scroll|screen-[a-z]+)"[^>]*tabindex="0"/);
  for (const selector of ["#scroll-area:focus", "#side-scroll:focus", ".screen:focus"]) {
    assert.match(cssBlock(selector), /outline:\s*none/, `${selector} draws a ring round the whole screen`);
  }
});

test("THE LIST ON SCREEN TAKES THE FOCUS: signed in, at a view switch, back from Settings and from Reply", async () => {
  const page = newPage();
  await signIn(page);
  assert.equal(focused(page), page.el("scroll-area"), `signed in, the focus is on ${focusName(page)}`);
  // THE DEFECT: the switch kept the focus from the click that pressed it, so Space pressed it again.
  page.el("view-switch").focusFrom("pointer");
  await showDiscord(page, tallChannel(30));
  assert.equal(focused(page), page.el("scroll-area"), `after the view switch the focus is on ${focusName(page)}`);
  page.el("open-settings").focusFrom("pointer");
  await page.el("open-settings").click();
  assert.equal(focused(page), page.el("screen-settings"), `on Settings the focus is on ${focusName(page)}`);
  // Back hides itself as it is pressed, which drops the focus on the floor in a browser.
  await page.el("close-settings").click();
  assert.equal(focused(page), page.el("scroll-area"), `back from Settings the focus is on ${focusName(page)}`);
  // `#410 inline-reply-editor`. Reply opens in the list, its box taking the focus; Cancel gives it back.
  await replyButton(page.el("discord-log").children[3]).click();
  assert.ok(replyInStream(page));
  assert.equal(focused(page), page.el("reply-text"), `on Reply the focus is on ${focusName(page)}`);
  await page.el("reply-cancel").click();
  assert.equal(focused(page), page.el("scroll-area"), `back from Reply the focus is on ${focusName(page)}`);
});

test("A BOX THE READER IS TYPING IN keeps the focus through a sign-in, and through a view change behind Settings", async () => {
  const page = await keyboardPage();
  page.el("channel-compose-text").focus();
  await page.el("channel-compose-text").setValue("half a thought");
  // A sign-in that answers while the reader types lands on the screen already up.
  await page.el("save-token").click();
  await page.settle();
  assert.equal(page.screen(), "main");
  assert.equal(focused(page), page.el("channel-compose-text"), `a sign-in took the focus to ${focusName(page)}`);
  // The search field, which stays on screen through a view switch.
  await press(page, page.el("scroll-area"), "/");
  await page.el("view-switch").click();
  assert.equal(page.tab(), "voice");
  assert.equal(focused(page), page.el("search-field"), `a view switch took the focus from the search to ${focusName(page)}`);
});

test("THE SCROLL KEYS are the browser's, and reach the list from wherever the focus is", async () => {
  const page = await keyboardPage();
  const area = page.el("scroll-area");
  // Over the channel the arrows and Home/End move the selection (below); the page keys and Space stay
  // the browser's. In the call's view, further down, every one of them is.
  const keys = ["PageUp", "PageDown", " "];
  // From nothing — a load, a control that hid itself — and from a control in the dock: the list takes
  // the focus on the way past, and the browser's own default, left alone, scrolls it.
  for (const from of [null, page.el("open-settings"), page.el("view-switch"), page.el("search-toggle")]) {
    for (const key of keys) {
      if (key === " " && from) continue;
      const top = area.scrollTop;
      const event = await press(page, from, key);
      const where = `${JSON.stringify(key)} from ${from ? `#${from.id}` : "nothing"}`;
      assert.equal(event.defaulted, true, `${where} was taken from the browser, which does the scrolling`);
      assert.equal(focused(page), area, `${where} left the focus on ${focusName(page)}`);
      assert.equal(area.scrollTop, top, `${where} scrolled the list itself instead of the browser`);
      assert.equal(page.screen(), "main", `${where} left the main screen`);
    }
  }
  // A focused button keeps Space: it presses it.
  for (const button of [page.el("open-settings"), page.el("view-switch")]) {
    const event = await press(page, button, " ");
    assert.equal(event.defaulted, true, `Space on #${button.id} was taken from the button`);
    assert.equal(focused(page), button, `Space on #${button.id} moved the focus to ${focusName(page)}`);
  }
  // Inside the list the browser scrolls it already: nothing moves the focus.
  const reply = replyButton(page.el("discord-log").children[5]);
  for (const key of keys) {
    const event = await press(page, reply, key);
    assert.equal(event.defaulted, true, `${JSON.stringify(key)} on a row's button was taken from the browser`);
    assert.equal(focused(page), reply, `${JSON.stringify(key)} on a row's button moved the focus`);
  }
  // Ctrl and Alt are somebody else's: the browser's tab keys, the proposal's channel switching.
  for (const fields of [{ ctrlKey: true }, { altKey: true }]) {
    const event = await press(page, null, "PageDown", fields);
    assert.equal(event.defaulted, true);
    assert.equal(focused(page), null, `${Object.keys(fields)[0]}+PageDown moved the focus`);
  }
  // The call's view has no messages to select (the proposal's question 9): every scroll key there is
  // the browser's, the list taking the focus on the way past.
  const call = newPage();
  await startTalking(call);
  assert.equal(call.tab(), "voice");
  const transcript = fillTranscript(call, 30);
  for (const key of ["PageUp", "PageDown", " ", "Home", "End", "ArrowUp", "ArrowDown", "j", "k", "e"]) {
    const top = transcript.scrollTop;
    const event = await press(call, null, key);
    assert.equal(event.defaulted, true, `${JSON.stringify(key)} in the call's view was taken from the browser`);
    if (key.length > 1 || key === " ") {
      assert.equal(focused(call), transcript, `${JSON.stringify(key)} in the call's view left the focus on ${focusName(call)}`);
    }
    assert.equal(transcript.scrollTop, top, `${JSON.stringify(key)} in the call's view scrolled the list itself`);
    assert.equal(call.tab(), "voice", `${JSON.stringify(key)} left the call's view`);
  }
});

test("ON A PICKER the arrows and Space are the picker's, and the page keys page the list", async () => {
  const page = await keyboardPage();
  const area = page.el("scroll-area");
  const picker = page.el("discord-channel");
  for (const key of ["ArrowUp", "ArrowDown", " ", "a"]) {
    const event = await press(page, picker, key);
    assert.equal(event.defaulted, true, `${JSON.stringify(key)} was taken from the channel picker`);
    assert.equal(focused(page), picker, `${JSON.stringify(key)} took the focus off the picker`);
  }
  const bottom = area.scrollTop;
  let event = await press(page, picker, "PageUp");
  assert.equal(event.defaulted, false, "PageUp on the picker was left to it, which skips channels");
  assert.equal(area.scrollTop, bottom - Math.round(SCROLL_VIEWPORT_PX * 0.875), "PageUp did not page the list");
  assert.equal(focused(page), picker, "PageUp took the focus off the picker");
  event = await press(page, picker, "Home");
  assert.equal(event.defaulted, false);
  assert.equal(area.scrollTop, 0, "Home on the picker did not take the list to its top");
  event = await press(page, picker, "End");
  assert.equal(event.defaulted, false);
  assert.ok(atBottomOf(area), "End on the picker did not take the list to its newest message");
  // A slider keeps every key, and so does the width handle, which answers them itself.
  for (const key of ["PageUp", "Home", "ArrowDown", " "]) {
    const slider = page.el("msg-scale");
    event = await press(page, slider, key);
    assert.equal(event.defaulted, true, `${JSON.stringify(key)} was taken from a slider`);
    assert.equal(focused(page), slider, `${JSON.stringify(key)} took the focus off a slider`);
  }
  const grip = page.el("width-grip");
  event = await press(page, grip, "PageDown");
  assert.equal(event.defaulted, false, "the width handle no longer answers its own keys");
  assert.equal(focused(page), grip, "a key the width handle answered moved the focus");
});

test("the text size takes no key of its own: Ctrl or Cmd with = or - is the browser's zoom, and − and + keep Enter and Space", async () => {
  // `#230 text-size-buttons`. The natural chords for "bigger text" are the browser's zoom, which
  // the reader must keep, and a single key for it would be one more to press by accident. So no
  // key: the − and + are a Tab away on Settings (`,`), and the slider between them steps on arrows.
  const page = await keyboardPage();
  const before = scaleOf(page);
  // Ctrl+0 is the voice view since `#525 cmd-0-voice-view`; the zoom chords that remain the
  // browser's are = and - (and + where a layout types it).
  for (const key of ["=", "-", "+"]) {
    for (const mod of ["ctrlKey", "metaKey"]) {
      const event = await press(page, page.el("scroll-area"), key, { [mod]: true });
      assert.equal(event.defaulted, true, `${mod} with ${JSON.stringify(key)} was taken from the browser's zoom`);
    }
  }
  assert.equal(scaleOf(page), before, "a zoom chord changed the message text size");
  const table = /const KEYMAP = \[([\s\S]*?)\n\];/.exec(SCRIPT_CODE);
  assert.ok(table, "web/voice.js has no KEYMAP table");
  assert.doesNotMatch(table[1], /msg-scale|MsgScale/, "the keymap took a key for the text size");
  await page.el("open-settings").click();
  for (const id of ["msg-scale-smaller", "msg-scale-larger"]) {
    for (const key of ["Enter", " "]) {
      const event = await press(page, page.el(id), key);
      assert.equal(event.defaulted, true, `${JSON.stringify(key)} on #${id} was taken from the button`);
      assert.equal(focused(page), page.el(id), `${JSON.stringify(key)} on #${id} moved the focus`);
      assert.equal(page.screen(), "settings", `${JSON.stringify(key)} on #${id} left Settings`);
    }
  }
});

test("IN A MESSAGE BOX the box keeps its keys, but a page key pages the list when the box has nothing to page", async () => {
  const page = await keyboardPage();
  const area = page.el("scroll-area");
  const box = page.el("channel-compose-text");
  const step = Math.round(SCROLL_VIEWPORT_PX * 0.875);
  for (const text of ["", "a reply in progress"]) {
    await box.setValue(text);
    scrollToBottom(area);
    const bottom = area.scrollTop;
    let event = await press(page, box, "PageUp");
    assert.equal(event.defaulted, false, `PageUp in the box (${JSON.stringify(text)}) was left to the box`);
    assert.equal(area.scrollTop, bottom - step, `PageUp in the box (${JSON.stringify(text)}) did not page the list`);
    assert.equal(focused(page), box, "PageUp took the reader out of the box");
    assert.equal(box.value, text, "PageUp changed what is in the box");
    event = await press(page, box, "PageDown");
    assert.equal(event.defaulted, false);
    assert.equal(area.scrollTop, bottom, "PageDown in the box did not page the list back");
    // The caret's keys, and typing. ↑ in the box while it is empty is a key of its own (below).
    for (const [key, fields] of [["Home", {}], ["End", {}], [" ", {}], ["ArrowDown", {}], ["PageUp", { shiftKey: true }],
      ["/", {}], [",", {}], ["j", {}], ["e", {}], ["?", {}], ["1", {}], ...(text ? [["ArrowUp", {}]] : [])]) {
      event = await press(page, box, key, fields);
      assert.equal(event.defaulted, true, `${JSON.stringify(key)} was taken from the box`);
      assert.equal(area.scrollTop, bottom, `${JSON.stringify(key)} in the box moved the list`);
      assert.equal(focused(page), box, `${JSON.stringify(key)} took the reader out of the box`);
      assert.equal(page.screen(), "main");
      assert.equal(page.el("search-field").hidden, true, `${JSON.stringify(key)} in the box opened the search`);
    }
    // Ctrl+Home and Ctrl+End (Cmd on a Mac) jump the list from anywhere, the box included.
    for (const mod of ["ctrlKey", "metaKey"]) {
      event = await press(page, box, "Home", { [mod]: true });
      assert.equal(event.defaulted, false, `${mod}+Home was left to the box`);
      assert.equal(area.scrollTop, 0, `${mod}+Home did not take the list to its top`);
      assert.equal(focused(page), box, `${mod}+Home took the reader out of the box`);
      event = await press(page, box, "End", { [mod]: true });
      assert.equal(event.defaulted, false, `${mod}+End was left to the box`);
      assert.ok(atBottomOf(area), `${mod}+End did not take the list to its newest message`);
    }
  }
  // A box holding more than it shows has a page of its own, and keeps its page keys.
  const longDraft = { tagName: "TEXTAREA", scrollHeight: 240, clientHeight: 60 };
  const top = area.scrollTop;
  const event = keyOn(longDraft, "PageUp");
  await page.documentEvent("keydown", event);
  assert.equal(event.defaulted, true, "a draft taller than its box lost its own PageUp");
  assert.equal(area.scrollTop, top, "PageUp in a draft taller than its box paged the list");
});

test("IN THE CALL'S BOX AND THE REPLY BOX a page key pages the list beside them", async () => {
  const page = newPage();
  await startTalking(page);
  const area = fillTranscript(page, 30);
  scrollToBottom(area);
  const bottom = area.scrollTop;
  assert.ok(bottom > 0, "the transcript is not taller than the screen");
  let event = await press(page, page.el("compose-text"), "PageUp");
  assert.equal(event.defaulted, false);
  assert.equal(area.scrollTop, bottom - Math.round(SCROLL_VIEWPORT_PX * 0.875), "PageUp in the call's box did not page the transcript");

  const reply = newPage();
  await signIn(reply);
  await openReplyOn(reply, tallChannel(30), 29);
  // `#410 inline-reply-editor`. The reply's box is in the list: the list it is in pages.
  const context = reply.el("scroll-area");
  const at = context.scrollTop;
  assert.ok(at > 0, `the list was at ${at} with the reply open, so a page up from it would prove nothing`);
  event = await press(reply, reply.el("reply-text"), "PageUp");
  assert.equal(event.defaulted, false, "PageUp in the reply box was left to the box");
  assert.equal(context.scrollTop, Math.max(0, at - Math.round(SCROLL_VIEWPORT_PX * 0.875)),
    "PageUp in the reply box did not page the list it is in");
  assert.equal(focused(reply), reply.el("reply-text"), "PageUp took the reader out of the reply box");
});

test("/ OPENS THE SEARCH with its field ready; pressed again, the field is focused with its text selected", async () => {
  const page = await keyboardPage();
  const event = await press(page, page.el("scroll-area"), "/");
  assert.equal(event.defaulted, false, "/ was left to the browser");
  assert.equal(page.el("search-field").hidden, false, "/ did not open the search");
  assert.equal(focused(page), page.el("search-field"), `/ left the focus on ${focusName(page)}`);
  await page.el("search-field").setValue("deploy");
  // Again, from the list: the field again, its text selected to be typed over.
  const selected = page.el("search-field").selectedTimes || 0;
  const again = await press(page, page.el("scroll-area"), "/");
  assert.equal(again.defaulted, false);
  assert.equal(page.el("search-field").hidden, false, "/ again closed the search");
  assert.equal(page.el("search-field").value, "deploy", "/ again lost the search text");
  assert.equal(focused(page), page.el("search-field"), `/ again left the focus on ${focusName(page)}`);
  assert.equal(page.el("search-field").selectedTimes, selected + 1, "/ again did not select the text");
  // In the field itself "/" is a character; from a message box, while typing, it is one as well.
  const typed = await press(page, page.el("search-field"), "/");
  assert.equal(typed.defaulted, true, "\"/\" typed in the search field was taken from it");
  const typing = await keyboardPage();
  await typing.el("channel-compose-text").setValue("see the ");
  const slash = await press(typing, typing.el("channel-compose-text"), "/");
  assert.equal(slash.defaulted, true, "\"/\" typed in the message box was taken from it");
  assert.equal(typing.el("search-field").hidden, true, "\"/\" typed in the message box opened the search");
  // `#248 composer-key`. Ctrl+S and Cmd+S are the message box's now, and never open the search.
  for (const fields of [{ ctrlKey: true }, { metaKey: true }]) {
    const other = await keyboardPage();
    await press(other, other.el("scroll-area"), "s", fields);
    assert.equal(other.el("search-field").hidden, true, `${JSON.stringify(fields)} s opened the search`);
  }
});

test("Ctrl+S AND Cmd+S GO TO THE MESSAGE BOX from the list, the search field and Settings, and the Save page never opens", async () => {
  // `#248 composer-key`. From the list, by the character and by the place of the key on a layout
  // that types no Latin letter there, and with Shift's capital.
  for (const [key, fields] of [["s", { ctrlKey: true }], ["s", { metaKey: true }], ["S", { ctrlKey: true }],
    ["ы", { ctrlKey: true, code: "KeyS" }]]) {
    const page = await keyboardPage();
    const what = `${Object.keys(fields).filter((f) => fields[f] === true).join("+")}+${key}`;
    const event = await press(page, page.el("scroll-area"), key, fields);
    assert.equal(event.defaulted, false, `${what} was left to the browser, whose Save page it is`);
    assert.equal(focused(page), page.el("channel-compose-text"), `${what} left the focus on ${focusName(page)}`);
    assert.equal(page.el("search-field").hidden, true, `${what} opened the search`);
  }
  // From the search field, while typing in it: the box, and the search is left as it was.
  const searching = await keyboardPage();
  await press(searching, searching.el("scroll-area"), "/");
  await searching.el("search-field").setValue("deploy");
  let event = await press(searching, searching.el("search-field"), "s", { ctrlKey: true });
  assert.equal(event.defaulted, false, "Ctrl+S in the search field was left to the browser's Save page");
  assert.equal(focused(searching), searching.el("channel-compose-text"),
    `Ctrl+S in the search field left the focus on ${focusName(searching)}`);
  assert.equal(searching.el("search-field").value, "deploy", "going to the box threw the search away");
  // From a message box it stays there, the draft untouched; a held chord's repeats are taken too.
  await searching.el("channel-compose-text").setValue("see the ");
  event = await press(searching, searching.el("channel-compose-text"), "s", { metaKey: true });
  assert.equal(event.defaulted, false);
  event = await press(searching, searching.el("channel-compose-text"), "s", { metaKey: true, repeat: true });
  assert.equal(event.defaulted, false, "a held Cmd+S reached the browser's Save page on its repeat");
  assert.equal(focused(searching), searching.el("channel-compose-text"));
  assert.equal(searching.el("channel-compose-text").value, "see the ", "Cmd+S touched the draft");
  // From Settings, and from Help: back to the list, and the box.
  for (const screen of ["settings", "help"]) {
    const page = await keyboardPage();
    await page.el("open-settings").click();
    if (screen === "help") await page.el("help-link-keyboard").click();
    assert.equal(page.screen(), screen);
    event = await press(page, page.el(`screen-${screen}`), "s", { ctrlKey: true });
    assert.equal(event.defaulted, false, `Ctrl+S on ${screen} was left to the browser's Save page`);
    assert.equal(page.screen(), "main", `Ctrl+S on ${screen} did not go back to the list`);
    assert.equal(focused(page), page.el("channel-compose-text"), `Ctrl+S on ${screen} left the focus on ${focusName(page)}`);
  }
  // With a reply open in the list, the reply's box, from anywhere on the screen (`#410
  // inline-reply-editor`); and "/" on the list is the search, the reply left open under it.
  const reply = await keyboardPage();
  await openReplyOn(reply, tallChannel(30), 2);
  event = await press(reply, reply.el("scroll-area"), "s", { ctrlKey: true });
  assert.equal(event.defaulted, false);
  assert.ok(replyInStream(reply), "Ctrl+S put the reply away");
  assert.equal(focused(reply), reply.el("reply-text"), `Ctrl+S on Reply left the focus on ${focusName(reply)}`);
  event = await press(reply, reply.el("scroll-area"), "/");
  assert.equal(reply.el("search-field").hidden, false, "\"/\" on the list beside a reply did not open the search");
  assert.ok(replyInStream(reply), "\"/\" put the reply away");
  // On the call's view, the agent's box under the transcript (`#241 voice-composer`), in a call and
  // with none: going there starts nothing.
  const call = newPage();
  await startTalking(call);
  call.el("scroll-area").focus();
  event = await press(call, call.el("scroll-area"), "s", { ctrlKey: true });
  assert.equal(event.defaulted, false, "Ctrl+S on the call's view was left to the browser's Save page");
  assert.equal(focused(call), call.el("compose-text"), `Ctrl+S on the call's view left the focus on ${focusName(call)}`);
  const idle = newPage();
  await signIn(idle);
  assert.equal(idle.tab(), "voice", "a fresh page did not open on the call's view");
  const writes = () => idle.requests.filter((request) => !request.startsWith("GET")).length;
  const before = writes();
  event = await press(idle, idle.el("scroll-area"), "s", { ctrlKey: true });
  assert.equal(event.defaulted, false, "Ctrl+S on the call's view, with no call, was left to the browser's Save page");
  assert.equal(focused(idle), idle.el("compose-text"), `Ctrl+S with no call left the focus on ${focusName(idle)}`);
  assert.equal(writes(), before, "going to the agent's box started something");
  // Not with Shift's own chord or Alt, and not a bare "s", which reads the selected message aloud.
  for (const [key, fields] of [["s", { ctrlKey: true, altKey: true }], ["s", {}]]) {
    const other = await keyboardPage();
    event = await press(other, other.el("scroll-area"), key, fields);
    assert.notEqual(focused(other), other.el("channel-compose-text"), `${JSON.stringify(fields)} ${key} went to the box`);
  }
});

test("Ctrl+Enter OUTSIDE A BOX goes to the message box; in a box it sends, as it always has", async () => {
  // `#248 composer-key`. "Or maybe Cmd-enter could jump to it if it's not highlighted (or send message
  // if it is highlighted with non-zero chars in it)."
  for (const fields of [{ ctrlKey: true }, { metaKey: true }]) {
    const page = await keyboardPage();
    const writes = () => page.requests.filter((request) => !request.startsWith("GET")).length;
    const sent = writes();
    const event = await press(page, page.el("scroll-area"), "Enter", fields);
    assert.equal(event.defaulted, false);
    assert.equal(focused(page), page.el("channel-compose-text"), `${JSON.stringify(fields)} Enter left the focus on ${focusName(page)}`);
    assert.equal(writes(), sent, "going to the box wrote something");
  }
  // From a selected message: the box, not the message's own Enter (which opens it).
  const atTop = await keyboardPageAtTop();
  await press(atTop, atTop.el("scroll-area"), "ArrowDown");
  const row = channelRow(atTop, "1");
  const collapsed = row.getAttribute("data-collapsed");
  await pressHere(atTop, "Enter", { ctrlKey: true });
  assert.equal(focused(atTop), atTop.el("channel-compose-text"), `Ctrl+Enter on a message left the focus on ${focusName(atTop)}`);
  assert.equal(channelRow(atTop, "1").getAttribute("data-collapsed"), collapsed, "Ctrl+Enter opened the message as Enter does");
  // In the reply box it sends; on the reply screen outside it, it goes to it.
  const reply = await keyboardPage();
  await openReplyOn(reply, tallChannel(30), 2);
  let event = await press(reply, reply.el("scroll-area"), "Enter", { ctrlKey: true });
  assert.equal(focused(reply), reply.el("reply-text"), `Ctrl+Enter on Reply left the focus on ${focusName(reply)}`);
  assert.equal(reply.repliesPosted.length, 0, "Ctrl+Enter outside the box sent the reply");
  await reply.el("reply-text").setValue("on it");
  event = await press(reply, reply.el("reply-text"), "Enter", { ctrlKey: true });
  assert.equal(event.defaulted, false);
  assert.equal(reply.repliesPosted.length, 1, "Ctrl+Enter in the reply box did not send");
  assert.equal(reply.repliesPosted[0].body.text, "on it");
  // In another field, the search's, it is the field's: nothing moves.
  const search = await keyboardPage();
  await press(search, search.el("scroll-area"), "/");
  event = await press(search, search.el("search-field"), "Enter", { ctrlKey: true });
  assert.equal(focused(search), search.el("search-field"), `Ctrl+Enter in the search field moved the focus to ${focusName(search)}`);
});

test("a HELD Ctrl+Enter outside a box goes to the box and sends NOTHING: its repeats land in the box", async () => {
  // `#248 composer-key`. The first keydown of a held chord puts the focus in the box; every repeat
  // after it is a keydown IN the box, where Ctrl+Enter is the send. They sent the draft the box
  // held, a quarter of a second after the reader asked only to go to it. One press is one send.
  const draft = "half-written draft";
  for (const fields of [{ ctrlKey: true }, { metaKey: true }]) {
    const chord = `${fields.ctrlKey ? "Ctrl" : "Cmd"}+Enter`;
    // From the list, with a draft waiting in the channel's box.
    const page = await keyboardPage();
    const writes = () => page.requests.filter((request) => !request.startsWith("GET")).length;
    await page.el("channel-compose-text").setValue(draft);
    const before = writes();
    let event = await press(page, page.el("scroll-area"), "Enter", fields);
    assert.equal(focused(page), page.el("channel-compose-text"), `a held ${chord} left the focus on ${focusName(page)}`);
    for (let i = 0; i < 3; i += 1) {
      event = await press(page, focused(page), "Enter", { ...fields, repeat: true });
      assert.equal(event.defaulted, false, `a held ${chord}'s repeat typed a new line in the box`);
    }
    assert.equal(page.repliesPosted.length, 0, `a held ${chord} from the list sent the channel box's draft`);
    assert.equal(writes(), before, `a held ${chord} from the list wrote something`);
    assert.equal(page.el("channel-compose-text").value, draft, `a held ${chord} from the list emptied the box`);
    // A press of its own, once the key is up and down again, still sends: nothing was taken from the box.
    event = await press(page, page.el("channel-compose-text"), "Enter", fields);
    assert.equal(page.repliesPosted.length, 1, `${chord} pressed again in the box did not send`);
    assert.equal(page.repliesPosted[0].body.text, draft);

    // From the conversation on Reply, with a draft waiting in the reply box.
    const reply = await keyboardPage();
    await openReplyOn(reply, tallChannel(30), 2);
    await reply.el("reply-text").setValue(draft);
    event = await press(reply, reply.el("scroll-area"), "Enter", fields);
    assert.equal(focused(reply), reply.el("reply-text"), `a held ${chord} on Reply left the focus on ${focusName(reply)}`);
    for (let i = 0; i < 3; i += 1) {
      event = await press(reply, focused(reply), "Enter", { ...fields, repeat: true });
      assert.equal(event.defaulted, false, `a held ${chord}'s repeat typed a new line in the reply box`);
    }
    assert.equal(reply.repliesPosted.length, 0, `a held ${chord} on Reply sent the reply's draft`);
    assert.ok(replyInStream(reply), `a held ${chord} on Reply put the reply away`);
    assert.equal(reply.el("reply-text").value, draft, `a held ${chord} on Reply emptied the reply box`);

    // On the call's view, in a call, with a draft waiting in the agent's box (`#241 voice-composer`).
    const call = newPage();
    const socket = await startTalking(call);
    await compose(call, draft);
    event = await press(call, call.el("scroll-area"), "Enter", fields);
    assert.equal(focused(call), call.el("compose-text"), `a held ${chord} on the call's view left the focus on ${focusName(call)}`);
    for (let i = 0; i < 3; i += 1) {
      event = await press(call, focused(call), "Enter", { ...fields, repeat: true });
      assert.equal(event.defaulted, false, `a held ${chord}'s repeat typed a new line in the agent's box`);
    }
    assert.equal(typedFrames(socket).length, 0, `a held ${chord} on the call's view sent the draft to the agent`);
    assert.equal(call.el("compose-text").value, draft, `a held ${chord} on the call's view emptied the agent's box`);
  }
});

test("m and Ctrl+Enter on the call's view go to the agent's box, as Ctrl+S does, and start nothing", async () => {
  // `#248 composer-key` with `#241 voice-composer`: the box m goes to on the call's view is the agent's
  // (`#538 c-key-copy`: c before).
  for (const [key, fields] of [["m", {}], ["Enter", { ctrlKey: true }], ["Enter", { metaKey: true }]]) {
    const page = newPage();
    await signIn(page);
    assert.equal(page.tab(), "voice", "a fresh page did not open on the call's view");
    const writes = () => page.requests.filter((request) => !request.startsWith("GET")).length;
    const before = writes();
    const what = `${Object.keys(fields).filter((f) => fields[f] === true).concat([key]).join("+")}`;
    const event = await press(page, page.el("scroll-area"), key, fields);
    assert.equal(event.defaulted, false, `${what} on the call's view was left to the browser`);
    assert.equal(focused(page), page.el("compose-text"), `${what} on the call's view left the focus on ${focusName(page)}`);
    assert.equal(page.el("compose-text").value, "", `${what} typed into the agent's box`);
    assert.equal(writes(), before, `${what} on the call's view started something`);
  }
});

test("ESCAPE CLOSES THE SEARCH, clears it, and gives the list back the focus", async () => {
  const page = await keyboardPage();
  await press(page, page.el("scroll-area"), "/");
  await page.el("search-field").setValue("deploy");
  // In the field: its own listener, then the document's, with the one event.
  let event = await press(page, page.el("search-field"), "Escape");
  assert.equal(event.defaulted, false);
  assert.equal(page.el("search-field").hidden, true, "Escape did not close the search");
  assert.equal(page.el("search-field").value, "", "Escape left the search text standing");
  assert.equal(focused(page), page.el("scroll-area"), `after Escape the focus is on ${focusName(page)}`);
  assert.equal(page.screen(), "main");
  // From the list, with the search open behind it.
  await press(page, page.el("scroll-area"), "/");
  event = await press(page, page.el("scroll-area"), "Escape");
  assert.equal(page.el("search-field").hidden, true, "Escape from the list did not close the search");
  assert.equal(focused(page), page.el("scroll-area"));
  // With nothing to close it does nothing — in particular it marks nothing read.
  const reads = page.dismissCalls.length;
  event = await press(page, page.el("scroll-area"), "Escape");
  assert.equal(event.defaulted, true, "an Escape with nothing to close was taken from the browser");
  assert.equal(page.screen(), "main");
  assert.equal(page.dismissCalls.length, reads, "Escape marked something read");
});

test("THE SETTINGS KEY: \",\" and Ctrl+, open Settings with its screen focused; Escape or the key again goes back", async () => {
  for (const [key, fields] of [[",", {}], [",", { ctrlKey: true }], [",", { metaKey: true }]]) {
    const page = await keyboardPage();
    const what = `${Object.keys(fields).filter((f) => fields[f] === true).join("+")}${key}`;
    const area = page.el("scroll-area");
    area.scrollTop = 1000;
    let event = await press(page, area, key, fields);
    assert.equal(event.defaulted, false, `${what} was left to the browser`);
    assert.equal(page.screen(), "settings", `${what} did not open Settings`);
    assert.equal(focused(page), page.el("screen-settings"), `on Settings the focus is on ${focusName(page)}`);
    event = await press(page, page.el("screen-settings"), "Escape");
    assert.equal(event.defaulted, false);
    assert.equal(page.screen(), "main", "Escape did not close Settings");
    assert.equal(focused(page), area, `back from Settings the focus is on ${focusName(page)}`);
    assert.equal(area.scrollTop, 1000, "back from Settings the reader was not where they were");
    await press(page, area, key, fields);
    event = await press(page, page.el("screen-settings"), key, fields);
    assert.equal(event.defaulted, false);
    assert.equal(page.screen(), "main", `${what} again did not close Settings`);
  }
  // In a field on Settings "," is a comma; Escape still leaves, and what was typed is already kept.
  const page = await keyboardPage();
  await press(page, page.el("scroll-area"), ",");
  const prompt = page.el("prompt-summary");
  let event = await press(page, prompt, ",");
  assert.equal(event.defaulted, true, "\",\" typed in a Settings field was taken from it");
  assert.equal(page.screen(), "settings", "\",\" typed in a Settings field left Settings");
  await prompt.setValue("Summarize, briefly.");
  const blurred = prompt.blurred || 0;
  event = await press(page, prompt, "Escape");
  assert.equal(event.defaulted, false);
  assert.equal(page.screen(), "main", "Escape in a Settings field did not leave Settings");
  // As a click on Back takes the focus from the field, which is what fires a field's `change`.
  assert.equal(prompt.blurred, blurred + 1, "Escape left Settings without letting go of the field first");
  assert.match(page.storage.get("vibe-talk.voice.prompts") || "", /Summarize, briefly\./,
    "what was typed in Settings was not kept before it was left");
  // Help is a page of Settings: Escape goes back to Settings, and again to the list.
  await press(page, page.el("scroll-area"), ",");
  await page.el("open-help").click();
  assert.equal(page.screen(), "help");
  assert.equal(focused(page), page.el("screen-help"), `on Help the focus is on ${focusName(page)}`);
  await press(page, page.el("screen-help"), "Escape");
  assert.equal(page.screen(), "settings", "Escape on Help did not go back to Settings");
  await press(page, page.el("screen-settings"), "Escape");
  assert.equal(page.screen(), "main");
});

test("THE SETTINGS KEY where the gear is not: from sign-in it opens and returns; beside a reply open in the list it opens and returns to it", async () => {
  const page = newPage();
  assert.equal(page.screen(), "signin");
  await press(page, null, ",");
  assert.equal(page.screen(), "settings", "\",\" on the sign-in screen did not open Settings, which the gear there does");
  await press(page, page.el("screen-settings"), "Escape");
  assert.equal(page.screen(), "signin", "Escape did not go back to the sign-in screen");
  // Typing the token: "," is a character.
  const event = await press(page, page.el("api-token"), ",");
  assert.equal(event.defaulted, true);
  assert.equal(page.screen(), "signin");

  // `#410 inline-reply-editor`. A reply is open in the list, on the main screen, where the gear is:
  // "," there is Settings, and back from it the reply is still open, its words kept.
  const reply = newPage();
  await signIn(reply);
  await openReplyOn(reply, tallChannel(5), 1);
  await reply.el("reply-text").setValue("half a thought");
  let key = await press(reply, reply.el("scroll-area"), ",");
  assert.equal(key.defaulted, false, "\",\" on the list beside a reply was left to the page's text");
  assert.equal(reply.screen(), "settings", "\",\" on the list beside a reply did not open Settings");
  await press(reply, reply.el("screen-settings"), "Escape");
  assert.ok(replyInStream(reply), "back from Settings, the reply was put away");
  assert.equal(reply.el("reply-text").value, "half a thought");
  key = await press(reply, reply.el("scroll-area"), ",", { metaKey: true });
  assert.equal(key.defaulted, false, "Cmd+, beside a reply was left to the browser, which opens its own settings");
});

test("EVERY CONTROL ON SETTINGS keeps its value the moment it changes: nothing there waits for a Save", () => {
  // `#224 keyboard-shortcuts`. Escape leaves Settings, so leaving must lose nothing. The four fields
  // below are not settings but the inputs of an act with its own button — rename the channel, add
  // one, add a rule — each a write to the server that a half-typed value must not make.
  const ACTS = {
    "channel-alias": "save-alias",
    "new-channel-id": "add-channel",
    "new-channel-label": "add-channel",
    "new-channel-writable": "add-channel",
    "noise-rule-new": "add-noise-rule",
    // `#232 upstream-mark-read`. Marking every message read in the chat service's own app is a
    // decision about the owner's attention there, so choosing a mode only shows what Save would do.
    "upstream-clear-mode": "upstream-clear-save",
    // `#272 voice-agent-instructions`. What the voice agent is told on every call: a half-typed
    // instruction must not reach it, so each box saves with its own button.
    "voice-instructions": "save-voice-instructions",
    "voice-system-prompt": "save-voice-system-prompt",
  };
  const page = newPage();
  const controls = [...PAGE_ELEMENTS].filter(([id, markup]) =>
    ["input", "select", "textarea"].includes(markup.tag) && markupHolds("screen-settings", id));
  assert.ok(controls.length >= 15, `only ${controls.length} controls were found on Settings`);
  for (const [id] of controls) {
    if (Object.prototype.hasOwnProperty.call(ACTS, id)) {
      assert.ok(markupHolds("screen-settings", ACTS[id]), `#${id}'s act has no button of its own on Settings`);
      continue;
    }
    const listens = page.el(id).listeners;
    assert.ok(listens.has("change") || listens.has("input"),
      `#${id} on Settings answers neither change nor input, so it keeps nothing until something else saves it`);
  }
});

test("NO KEY FIRES while an input method composes, from a phone's keyboard, or with Ctrl, Cmd or Alt held", async () => {
  const page = await keyboardPage();
  const area = page.el("scroll-area");
  for (const [key, fields] of [
    ["/", { isComposing: true }], [",", { isComposing: true }], ["s", { ctrlKey: true, isComposing: true }],
    // What a phone's on-screen keyboard reports for nearly every key.
    ["Unidentified", { keyCode: 229 }], ["/", { keyCode: 229 }],
    ["/", { altKey: true }], [",", { altKey: true }], ["j", { ctrlKey: true }], ["e", { metaKey: true }],
    // `#344 search-shortcut`. The search's chord too: composing, from a phone's keyboard, and a "/" typed
    // with AltGr, which Windows sends as Ctrl and Alt together.
    ["/", { ctrlKey: true, isComposing: true }], ["/", { metaKey: true, keyCode: 229 }],
    ["/", { ctrlKey: true, altKey: true }],
    // `#281 view-keys`. Ctrl+1 to Ctrl+4 choose the views now; a chord past them is still the browser's.
    ["e", { altKey: true }], ["5", { ctrlKey: true }], ["1", { ctrlKey: true, isComposing: true }],
    ["PageDown", { isComposing: true }],
  ]) {
    const event = await press(page, area, key, fields);
    const what = `${JSON.stringify(fields)} ${JSON.stringify(key)}`;
    assert.equal(event.defaulted, true, `${what} was taken`);
    assert.equal(page.screen(), "main", `${what} left the list`);
    assert.equal(page.el("search-field").hidden, true, `${what} opened the search`);
  }
  // An Escape while composing is the input method's: the search stays open.
  await press(page, area, "/");
  await press(page, page.el("search-field"), "Escape", { isComposing: true });
  assert.equal(page.el("search-field").hidden, false, "an input method's Escape closed the search");
});

test("WITH A MENU OPEN Escape closes it first, and gives its button back the focus", async () => {
  const page = await keyboardPage();
  await press(page, page.el("scroll-area"), "/");
  const row = page.el("discord-log").children[4];
  await rowMoreButton(row).click();
  const item = rowMoreMenu(row).descendants().find((node) => node.tagName === "button");
  assert.ok(item, "the ⋯ menu holds no item");
  let event = await press(page, item, "Escape");
  assert.equal(event.defaulted, false);
  assert.equal(rowMoreMenu(row).hidden, true, "Escape did not close the ⋯ menu");
  assert.equal(focused(page), rowMoreButton(row), `the focus went to ${focusName(page)}, not the ⋯ that opened the menu`);
  assert.equal(page.el("search-field").hidden, false, "one Escape closed the menu AND the search");
  // A page key from the ⋯ is the browser's: it is inside the list.
  event = await press(page, rowMoreButton(row), "PageUp");
  assert.equal(event.defaulted, true);
  assert.equal(focused(page), rowMoreButton(row));
});

test("THE SELECTION IS THE FOCUS: ↓ or j takes the first message showing, then the next; ↑ or k goes back", async () => {
  const page = await keyboardPageAtTop();
  const area = page.el("scroll-area");
  let event = await press(page, area, "ArrowDown");
  assert.equal(event.defaulted, false, "↓ over the channel was left to the browser");
  assert.equal(selectedId(page), "1", "the first ↓ did not take the first message showing");
  const first = channelRow(page, "1");
  assert.equal(first.getAttribute("tabindex"), "0", "the selected row cannot be come back to with Shift+Tab");
  assert.equal(first.focusedWith && first.focusedWith.preventScroll, true, "the focus moved the list itself");
  for (const [key, id] of [["j", "2"], ["ArrowDown", "3"], ["k", "2"], ["ArrowUp", "1"]]) {
    event = await pressHere(page, key);
    assert.equal(event.defaulted, false, `${key} was left to the browser`);
    assert.equal(selectedId(page), id, `${key} selected ${selectedId(page)}, not message ${id}`);
  }
  // One row holds the tabindex: the one selected.
  const tabbed = page.el("discord-log").children.filter((li) => li.getAttribute("tabindex") === "0");
  assert.deepStrictEqual(tabbed.map((li) => li.getAttribute("data-id")), ["1"], "more than the selected row is a Tab stop");
  // ↑ past the first scrolls on toward the older history, and the selection stays where it is.
  event = await pressHere(page, "k");
  assert.equal(selectedId(page), "1");
  // A held ↓ repeats; it is one of the keys that move.
  event = await pressHere(page, "ArrowDown", { repeat: true });
  assert.equal(selectedId(page), "2", "a held ↓ did not repeat");
  // Each selected row is brought on screen: twenty more and the list has followed.
  for (let i = 0; i < 20; i += 1) await pressHere(page, "j");
  assert.equal(selectedId(page), "22");
  const box = channelRow(page, "22").getBoundingClientRect();
  assert.ok(box.top >= 0 && box.bottom <= SCROLL_VIEWPORT_PX, `the selected row is off screen: ${JSON.stringify(box)}`);
});

test("FROM THE NEWEST LINE, ↑ takes the last message showing; a selection scrolled away starts again from the screen", async () => {
  const page = await keyboardPage();
  const area = page.el("scroll-area");
  await press(page, area, "ArrowUp");
  assert.equal(selectedId(page), "30", "↑ at the newest line did not take the last message showing");
  // Scrolled away with the wheel: the next ↓ starts from what is on screen, not from message 30.
  area.scrollTop = 0;
  await pressHere(page, "ArrowDown");
  assert.equal(selectedId(page), "1", `after scrolling to the top, ↓ selected ${selectedId(page)}`);
});

test("↓ READS THROUGH a message taller than the screen before it moves on (the proposal's question 10)", async () => {
  const huge = "a line of the agent's long report. ".repeat(320);
  const page = await keyboardPageAtTop([
    message({ id: "1", content: SHORT_MESSAGE }),
    message({ id: "2", content: huge }),
    message({ id: "3", content: SHORT_MESSAGE }),
    ...tallChannel(6).map((m, i) => ({ ...m, id: String(10 + i) })),
  ]);
  const area = page.el("scroll-area");
  await press(page, area, "j");
  await pressHere(page, "j");
  assert.equal(selectedId(page), "2");
  await pressHere(page, "Enter");
  assert.equal(channelRow(page, "2").getAttribute("data-collapsed"), "false", "Enter did not open the long message");
  assert.ok(channelRow(page, "2").getBoundingClientRect().height > 3 * SCROLL_VIEWPORT_PX, "the message is not taller than the screen");
  let steps = 0;
  while (selectedId(page) === "2" && steps < 40) {
    const before = area.scrollTop;
    await pressHere(page, "ArrowDown");
    steps += 1;
    if (selectedId(page) === "2") {
      assert.ok(area.scrollTop > before, "↓ on a tall message neither scrolled through it nor moved on");
      assert.ok(area.scrollTop - before <= SCROLL_VIEWPORT_PX / 2 + 1, "↓ scrolled more than half a screen at once");
    }
  }
  assert.ok(steps > 3, `↓ moved on from a message ${channelRow(page, "2").getBoundingClientRect().height}px tall after ${steps} presses`);
  assert.equal(selectedId(page), "3", "after reading through it ↓ did not move on to the next message");
  // And ↑ reads back up through it before it moves on.
  await pressHere(page, "ArrowUp");
  assert.equal(selectedId(page), "2");
  const top = area.scrollTop;
  await pressHere(page, "ArrowUp");
  assert.equal(selectedId(page), "2", "↑ left the tall message before reading back up through it");
  assert.ok(area.scrollTop < top, "↑ on a tall message did not scroll back up through it");
});

test("THE SELECTION OUTLIVES A REDRAW: the new row takes the focus; a row that goes hands it to the next", async () => {
  const messages = tallChannel(30);
  const page = await keyboardPageAtTop(messages);
  await press(page, page.el("scroll-area"), "ArrowDown");
  for (let i = 0; i < 9; i += 1) await pressHere(page, "j");
  assert.equal(selectedId(page), "10");
  const old = channelRow(page, "10");
  // A re-read that brings a new message redraws every row.
  await refreshDiscord(page, [...messages, message({ id: "31", content: "a new message" })]);
  assert.equal(selectedId(page), "10", `after the redraw the focus is on ${selectedId(page)}`);
  assert.equal(focused(page).parentNode, page.el("discord-log"), "the focus is on a row no longer in the list");
  assert.equal(focused(page).getAttribute("tabindex"), "0");
  if (focused(page) !== old) assert.equal(old.getAttribute("tabindex"), null, "the row that went kept its tabindex");
  // ...and is the one lit. `#239 single-selection`.
  assert.deepStrictEqual(page.el("discord-log").children.filter((li) => li.getAttribute("data-selected") === "true")
    .map((li) => li.getAttribute("data-id")), ["10"], "after the redraw the selected message is not the one lit");
  // A redraw while the reader types takes nothing from them.
  page.el("channel-compose-text").focus();
  await refreshDiscord(page, [...messages, message({ id: "31", content: "a new message" }), message({ id: "32", content: "and another" })]);
  assert.equal(focused(page), page.el("channel-compose-text"), "a redraw took the focus out of the message box");
});

test("e MARKS THE SELECTED MESSAGE DONE and moves on to the next unread below; e on a Done one puts it back", async () => {
  const page = await keyboardPageAtTop();
  await press(page, page.el("scroll-area"), "ArrowDown");
  let event = await pressHere(page, "e");
  assert.equal(event.defaulted, false);
  assert.deepStrictEqual(page.dismissCalls.map((call) => call.messages), [["1"]], "e did not mark the selected message Done");
  assert.equal(channelRow(page, "1").getAttribute("data-archived"), "true");
  assert.equal(selectedId(page), "2", "after e the selection did not move on to the next unread message");
  await pressHere(page, "e");
  assert.equal(selectedId(page), "3");
  // Back up to a Done message: e puts it back, and the selection stays on it.
  await pressHere(page, "k");
  await pressHere(page, "k");
  assert.equal(selectedId(page), "1");
  event = await pressHere(page, "e");
  assert.deepStrictEqual(page.restoreCalls.map((call) => call.messages), [["1"]], "e on a Done message did not put it back");
  assert.equal(channelRow(page, "1").getAttribute("data-archived"), "false");
  assert.equal(selectedId(page), "1", "putting a message back moved the selection off it");
  // n: the next unread below, past the Done one.
  await pressHere(page, "n");
  assert.equal(selectedId(page), "3", `n selected ${selectedId(page)}, not the next unread below`);
  // z: the last Done undone, as the Undo chip does it.
  await pressHere(page, "e");
  const restores = page.restoreCalls.length;
  await pressHere(page, "z");
  assert.equal(page.restoreCalls.length, restores + 1, "z did not undo the last Done");
  assert.equal(channelRow(page, "3").getAttribute("data-archived"), "false", "z did not put back the message e marked");
  // A held e is one Done, not one on every row it passes.
  const calls = page.dismissCalls.length;
  await pressHere(page, "e", { repeat: true });
  assert.equal(page.dismissCalls.length, calls, "a held e marked another message Done");
  // A held Shift+E likewise: one read and collapse, not one on every repeat (`#295 shift-e-read-collapse`).
  await pressHere(page, "E", { shiftKey: true, repeat: true });
  assert.equal(page.dismissCalls.length, calls, "a held Shift+E marked another message Done");
});

/** `rowStates` with message `id` left out. */
const othersOf = (states, id) => Object.fromEntries(Object.entries(states).filter(([key]) => key !== id));

/** The line in `readAndCollapse` that marks Done only a message not Done already. */
const DONE_UNLESS_DONE = "  if (!wasDone) await markRowDone(ids, row);\n";

/**
 * A channel of thirty long messages at its top, message 3 selected and opened with Enter, then Shift+E.
 * Answers the page and what was measured just before the key. `arrange` sets the page up before it
 * signs in: whether its chat service has a read marker the server can move, for one.
 */
async function readCollapseOnThree(script = SCRIPT, arrange = null) {
  const opened = await openThree(script, arrange);
  const event = await pressHere(opened.page, "E", { shiftKey: true });
  return { ...opened, event };
}

test("Shift+E MARKS THE SELECTED MESSAGE DONE AND COLLAPSES IT, and nothing else changes (#295)", async () => {
  const { page, area, before, event } = await readCollapseOnThree();
  assert.equal(event.defaulted, false, "Shift+E on a selected message was left to the browser");
  // The per-message read: exactly the request the row's Done button and e make, for this one message.
  assert.deepStrictEqual(page.dismissCalls.slice(before.dismissed), [{ messages: ["3"] }],
    "Shift+E did not mark exactly the selected message Done");
  assert.deepStrictEqual(page.restoreCalls, [], "Shift+E put something back");
  const row = channelRow(page, "3");
  assert.equal(row.getAttribute("data-archived"), "true", "Shift+E did not mark the message Done");
  assert.equal(row.getAttribute("data-collapsed"), "true", "Shift+E did not collapse the message");
  assert.ok(row.getBoundingClientRect().height < before.height, "the collapsed message takes as much room as before");
  // Nothing else: every other row as it was, the selection on the message, and the list where it was.
  assert.deepStrictEqual(othersOf(rowStates(page), "3"), othersOf(before.states, "3"), "Shift+E changed another message");
  assert.equal(selectedId(page), "3", "Shift+E moved the selection off the message");
  assert.equal(area.scrollTop, before.scrollTop, "Shift+E scrolled the list");
  assert.equal(row.getBoundingClientRect().top, before.top, "the message moved under the reader");
  // This chat service has no read marker the server can move: nothing is asked of it, and the status
  // line says the message was marked here only, as the Done's own words do.
  assert.deepStrictEqual(page.upstreamReadCalls, [], "a service with no read marker was asked to move it");
  assert.equal(page.el("status").textContent, "Marked read here, not in Discord, and collapsed.",
    "nothing said what Shift+E did, or where it marked the message");
  // The fold is the message's, by id, as the fold control's is: a redraw draws it folded.
  await refreshDiscord(page, tallChannel(30));
  assert.equal(channelRow(page, "3").getAttribute("data-collapsed"), "true", "a redraw opened the message again");
  // Pressed again, on a message already Done and folded: it is only ever a mark, and puts nothing back.
  const marked = page.dismissCalls.length;
  await pressHere(page, "E", { shiftKey: true });
  assert.deepStrictEqual(page.restoreCalls, [], "Shift+E on a Done message put it back");
  assert.equal(page.dismissCalls.length, marked, "Shift+E on a Done message wrote another Done");
  assert.equal(channelRow(page, "3").getAttribute("data-archived"), "true");
  assert.deepStrictEqual(page.upstreamReadCalls, [], "a service with no read marker was asked to move it");
  assert.equal(page.el("status").textContent, "Already read here, not in Discord, and collapsed.");
  // Opened again and Shift+E: folded, still Done.
  await pressHere(page, "Enter");
  assert.equal(channelRow(page, "3").getAttribute("data-collapsed"), "false");
  await pressHere(page, "E", { shiftKey: true });
  assert.equal(channelRow(page, "3").getAttribute("data-collapsed"), "true", "Shift+E did not fold a Done message");
  assert.deepStrictEqual(page.restoreCalls, [], "Shift+E on an open Done message put it back");
  // Its Done is the last Done, and z (the Undo chip) puts back exactly that message.
  await pressHere(page, "z");
  await page.settle();
  assert.deepStrictEqual(page.restoreCalls.map((call) => call.messages), [["3"]], "z did not undo the Shift+E's Done");
  assert.equal(channelRow(page, "3").getAttribute("data-archived"), "false");
});

test("Shift+E's negative controls: a key that does not fold, one that toggles, and the old Done through here all fail (#295)", async () => {
  // Without the fold: the message stays open.
  const unfolded = await readCollapseOnThree(brokenScript("const folded = foldForRead(pane, row);", "const folded = false;"));
  assert.equal(channelRow(unfolded.page, "3").getAttribute("data-collapsed"), "false",
    "the negative control folded the message anyway, so the check above proves nothing");
  // As a toggle, the e key's act: a second press puts the message back.
  const toggled = await readCollapseOnThree(brokenScript(DONE_UNLESS_DONE, "  await toggleArchived(ids, row);\n"));
  await pressHere(toggled.page, "E", { shiftKey: true });
  assert.notDeepStrictEqual(toggled.page.restoreCalls, [],
    "the negative control's toggle put nothing back, so the check above proves nothing");
  // The old act: every message above is marked too.
  const through = await readCollapseOnThree(brokenScript("guardQuietly(() => readAndCollapse(pane, row, ids))();",
    "guardQuietly(() => markReadThroughHere(row, ids[ids.length - 1]))();"));
  assert.notDeepStrictEqual(othersOf(rowStates(through.page), "3"), othersOf(through.before.states, "3"),
    "the negative control's Done through here changed no other message, so the check above proves nothing");
});

/**
 * Message 5 of thirty long messages opened, the list scrolled so that its head is `above` pixels over
 * the top of the screen and its foot on it, then Shift+E. Answers how far message 6, the line below
 * it, moved, and where message 5 ended up.
 */
async function readCollapseScrolledPast(script = SCRIPT) {
  const page = newPage(new Map(), script);
  await signIn(page);
  await showDiscord(page, tallChannel(30));
  const area = page.el("scroll-area");
  area.scrollTop = 0;
  await press(page, area, "ArrowDown");
  for (let i = 0; i < 4; i += 1) await pressHere(page, "j");
  assert.equal(selectedId(page), "5");
  await pressHere(page, "Enter");
  const row = channelRow(page, "5");
  const height = row.getBoundingClientRect().height;
  // Its foot 100px down the screen, so its head is above the top: read most of the way through.
  area.scrollTop += row.getBoundingClientRect().bottom - 100;
  const was = row.getBoundingClientRect();
  assert.ok(was.top < -LINE_PX && was.bottom === 100, `message 5 is not scrolled part way past: ${JSON.stringify(was)}`);
  assert.ok(height > 100 + LINE_PX * 2, "message 5 is too short for this case");
  const next = channelRow(page, "6").getBoundingClientRect().top;
  await pressHere(page, "E", { shiftKey: true });
  const now = row.getBoundingClientRect();
  return { page, row, moved: channelRow(page, "6").getBoundingClientRect().top - next, foot: now.bottom - was.bottom, now };
}

test("Shift+E ON A MESSAGE READ PART WAY PAST keeps the line below where it was, and the folded message on screen (#280, #295)", async () => {
  const { page, row, moved, foot, now } = await readCollapseScrolledPast();
  assert.equal(row.getAttribute("data-collapsed"), "true", "Shift+E did not collapse the message");
  assert.equal(row.getAttribute("data-archived"), "true", "Shift+E did not mark the message Done");
  assert.equal(moved, 0, `the message below moved ${moved}px under the reader`);
  assert.equal(foot, 0, `the collapsed message's foot moved ${foot}px`);
  assert.ok(now.bottom > 0, "the collapsed message went off the top of the screen");
  assert.equal(selectedId(page), "5");
  // The control: the fold control's own hold, on the row's head, wherever that head is. Holding a
  // head that is off the screen carries the folded message off it, and lifts the rest up past the
  // place being read.
  const control = await readCollapseScrolledPast(brokenScript("if (was.top >= head - 1 || atBottom(area)) {", "if (true) {"));
  assert.notEqual(control.moved, 0, "the negative control held the line below too, so this fixture cannot tell them apart");
  assert.ok(control.now.bottom <= 0, "the negative control kept the folded message on screen, so the check above proves nothing");
});

/** A message taller than the screen opened, read part way through with ↓, then Shift+E. */
async function readCollapseTall(script = SCRIPT) {
  const huge = "a line of the agent's long report. ".repeat(320);
  const page = newPage(new Map(), script);
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1", content: SHORT_MESSAGE }),
    message({ id: "2", content: huge }),
    ...tallChannel(6).map((m, i) => ({ ...m, id: String(10 + i) })),
  ]);
  const area = page.el("scroll-area");
  area.scrollTop = 0;
  await press(page, area, "j");
  await pressHere(page, "j");
  await pressHere(page, "Enter");
  const row = channelRow(page, "2");
  for (let i = 0; i < 4; i += 1) await pressHere(page, "ArrowDown");
  const was = row.getBoundingClientRect();
  assert.equal(selectedId(page), "2");
  assert.ok(was.top < 0 && was.bottom > SCROLL_VIEWPORT_PX, `the message does not span the screen: ${JSON.stringify(was)}`);
  await pressHere(page, "E", { shiftKey: true });
  // The top of the list is under what floats over its head, the freshness pill and the glass.
  const head = Math.max(0, ...["channel-freshness", "search-float"].map((id) => page.el(id))
    .filter((node) => !node.hidden).map((node) => node.getBoundingClientRect().bottom));
  return { page, row, head, top: row.getBoundingClientRect().top };
}

test("Shift+E ON A MESSAGE TALLER THAN THE SCREEN, read part way through, brings its head to the top (#280, #295)", async () => {
  const { page, row, head, top } = await readCollapseTall();
  assert.equal(row.getAttribute("data-collapsed"), "true");
  assert.equal(top, head, "the collapsed message's head is not at the top of the list");
  assert.equal(channelRow(page, "10").getBoundingClientRect().top, row.getBoundingClientRect().bottom,
    "the message after it is not right below it");
  // The control: its foot held, though the foot is below the screen. The list runs back up past
  // everything the reader had read, and the folded message is nowhere near the top.
  const control = await readCollapseTall(brokenScript("now.bottom - was.bottom : now.top - head;",
    "now.bottom - was.bottom : now.bottom - was.bottom;"));
  assert.notEqual(control.top, head,
    "the negative control also brought the folded message to the top, so the check above proves nothing");
});

test("Shift+E on a short message, which has no fold, marks it and says only that (#295)", async () => {
  const page = await keyboardPageAtTop(backlog(4));
  await press(page, page.el("scroll-area"), "ArrowDown");
  await pressHere(page, "j");
  const id = selectedId(page);
  await pressHere(page, "E", { shiftKey: true });
  assert.deepStrictEqual(page.dismissCalls, [{ messages: [id] }]);
  assert.equal(page.el("status").textContent, "Marked read here, not in Discord.");
  assert.equal(selectedId(page), id);
});

test("Shift+E UNDER COLLAPSE READ joins the message to the read run round it, the selection on the run's line; UNDER HIDE READ the selection goes on to the next message (#221, #295)", async () => {
  // Fold read: unread, two read, unread, two read, then unread. Shift+E on the unread one between
  // the runs joins them, as its Done button's Done does, and the selection is left on what the reader
  // sees there: the line of the run, which the message is now inside.
  const page = newPage();
  const messages = joiningChannel(page);
  await signIn(page);
  await showDiscord(page, messages);
  await chooseReadMode(page, "Fold read");
  page.el("scroll-area").scrollTop = 0;
  await press(page, page.el("scroll-area"), "ArrowDown");
  for (let i = 0; i < 3 && selectedId(page) !== messages[3].id; i += 1) await pressHere(page, "j");
  assert.equal(selectedId(page), messages[3].id, "the premise is the unread message between the runs selected");
  await pressHere(page, "E", { shiftKey: true });
  await page.settle();
  assert.deepStrictEqual(page.dismissCalls, [{ messages: [messages[3].id] }], "Shift+E marked more than its message");
  assert.equal(readRunLineOf(rowWithId(page, messages[1].id)).textContent, "… 5 read messages …",
    "the runs did not join round the message Shift+E marked");
  assert.equal(rowWithId(page, messages[3].id).hidden, true, "the message is still drawn outside the run");
  assert.equal(selectedId(page), messages[1].id, "the selection is not on the run's line");
  // A read message alone between unread ones is shown, Fold read's own rule: there the selection
  // stays on it.
  for (let i = 0; i < 6 && selectedId(page) !== messages[8].id; i += 1) await pressHere(page, "j");
  assert.equal(selectedId(page), messages[8].id);
  await pressHere(page, "E", { shiftKey: true });
  await page.settle();
  assert.equal(rowWithId(page, messages[8].id).getAttribute("data-archived"), "true");
  assert.equal(rowWithId(page, messages[8].id).hasAttribute("data-read-run"), false, "a read message alone was collapsed");
  assert.equal(selectedId(page), messages[8].id, "the selection left a read message still shown");

  // Hide read: the message goes, and the selection with it to the next message shown.
  const hide = await keyboardPageAtTop();
  await turnTodoOn(hide);
  hide.el("scroll-area").scrollTop = 0;
  await press(hide, hide.el("scroll-area"), "ArrowDown");
  await pressHere(hide, "j");
  assert.equal(selectedId(hide), "2");
  await pressHere(hide, "E", { shiftKey: true });
  await hide.settle();
  assert.deepStrictEqual(hide.dismissCalls, [{ messages: ["2"] }], "Shift+E marked more than its message");
  assert.ok(!channelRow(hide, "2") || channelRow(hide, "2").hidden, "Hide read still shows the message Shift+E marked");
  assert.equal(selectedId(hide), "3", "the selection did not go on to the next message");
});

test("Shift+E IN THE RIGHT COLUMN acts on the message selected there, and on nothing in Main (#295)", async () => {
  const page = newPage();
  const data = threadData();
  // The thread's messages long enough to fold.
  const messages = data.messages.map((m) => ({ ...m, content: longMessage(m.content) }));
  page.threadingSupported = true;
  page.threads = data.threads.map((t) => ({ ...t, root: messages.find((m) => m.id === t.root.id) }));
  page.messages = messages;
  page.upstreamReadMarkSupported = true;
  await signIn(page);
  await showDiscord(page, messages);
  await twoColumns(page);
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  const main = rowStates(page);
  await press(page, page.el("side-scroll"), "ArrowDown");
  await pressHere(page, "j");
  assert.equal(selectedId(page), "202", `the thread's answer is not selected: ${selectedId(page)}`);
  await pressHere(page, "Enter");
  assert.equal(sideRow(page, "202").getAttribute("data-collapsed"), "false", "Enter did not open the answer");
  await pressHere(page, "E", { shiftKey: true });
  assert.deepStrictEqual(page.dismissCalls, [{ messages: ["202"] }], "Shift+E in the right column marked something else");
  assert.deepStrictEqual(page.upstreamReadCalls, [{ message_id: "202", after_reply: true }],
    "Shift+E in the right column did not mark its message read in the chat service");
  assert.equal(sideRow(page, "202").getAttribute("data-archived"), "true");
  assert.equal(sideRow(page, "202").getAttribute("data-collapsed"), "true", "Shift+E did not collapse the right column's message");
  assert.deepStrictEqual(rowStates(page), main, "Shift+E in the right column changed a message of Main");
  assert.equal(selectedId(page), "202");
});

// Shift+E's read is the one a sent reply makes (`#300 reply-marks-read`): Done here, then the chat
// service's own read marker moved through the message where the server can move it, asked
// `after_reply` so the server keeps it forward and asks once. Never after a Done that failed, and never
// on a channel that is one thread of a larger space. A message already Done is not written Done again,
// and its service half is still asked, which the server's floor answers without asking the service
// where it has marked through that message already.

/** `readCollapseOnThree` on a page whose chat service has a read marker the server can move. */
const readCollapseMarked = (script = SCRIPT, arrange = null) =>
  readCollapseOnThree(script, (page) => { page.upstreamReadMarkSupported = true; if (arrange) arrange(page); });

/** The line in `readAndCollapse` that asks the chat service, taken out for a negative control. */
const ASKS_SERVICE = "  const mark = await markReadInService(channel, ids[ids.length - 1]);\n";

/**
 * Shift+E on message 3 with the server's floor, then again on it once opened, then e on message 4 and
 * Shift+E on it. Answers the page, the floor's asks of the service, and the status line after each.
 */
async function readCollapseTwice(script = SCRIPT) {
  let asked = null;
  const { page, before } = await readCollapseMarked(script, (p) => { asked = serverFloor(p); });
  const said = [page.el("status").textContent];
  await pressHere(page, "Enter");
  assert.equal(channelRow(page, "3").getAttribute("data-collapsed"), "false", "the premise is the message opened again");
  await pressHere(page, "E", { shiftKey: true });
  said.push(page.el("status").textContent);
  const again = { dismissed: page.dismissCalls.length - before.dismissed, asked: [...asked] };
  await pressHere(page, "j");
  assert.equal(selectedId(page), "4");
  await pressHere(page, "e");
  assert.equal(page.dismissCalls.at(-1).messages.join(), "4", "the premise is e marking message 4 Done");
  const afterE = page.upstreamReadCalls.length;
  await pressHere(page, "k");
  assert.equal(selectedId(page), "4");
  await pressHere(page, "E", { shiftKey: true });
  said.push(page.el("status").textContent);
  return { page, before, asked, said, again, afterE };
}

test("Shift+E MARKS THE MESSAGE READ IN THE CHAT SERVICE where the server can move its marker: asked once, after the Done, as a reply's mark (#295, #300)", async () => {
  const { page, before } = await readCollapseMarked();
  assert.deepStrictEqual(page.dismissCalls.slice(before.dismissed), [{ messages: ["3"] }], "the premise is the Done");
  assert.deepStrictEqual(page.upstreamReadCalls, [{ message_id: "3", after_reply: true }],
    "Shift+E did not ask the chat service to mark the message read, as a reply's mark");
  const dismissed = page.requests.findIndex((one) => one.endsWith("/dismiss"));
  const marked = page.requests.findIndex((one) => one.endsWith("/upstream-read"));
  assert.ok(dismissed >= 0 && marked > dismissed, "the chat service was asked before the Done was saved");
  assert.equal(page.el("status").textContent, "Marked read here and in Discord, and collapsed.",
    "the status line does not say the chat service was marked");
  assert.equal(channelRow(page, "3").getAttribute("data-collapsed"), "true");
  assert.equal(page.el("error").hidden, true, `the mark put up the error banner: ${page.el("error").textContent}`);

  // THE CONTROLS. Where the service has no marker the server can move, nothing is asked (the first
  // test above): without the provider's word checked, it is asked all the same. Without the ask,
  // Shift+E is the branch as it was before #300: Done here only.
  const { page: unmarked } = await readCollapseOnThree(brokenScript(
    "  if (!channelMarksUpstream(channel)) return none;\n", ""));
  assert.deepStrictEqual(unmarked.upstreamReadCalls, [{ message_id: "3", after_reply: true }],
    "with the provider's word unchecked nothing was asked, so the first test's check cannot see it");
  const { page: unasked } = await readCollapseMarked(brokenScript(ASKS_SERVICE,
    "  const mark = { service: \"Discord\", asked: false, marked: false, already: false, why: \"\" };\n"));
  assert.deepStrictEqual(unasked.upstreamReadCalls, [], "with the ask taken out it still asked, so this test cannot see it");
});

test("Shift+E ON A MESSAGE ALREADY DONE writes no second Done, and asks the service only what the server's floor has not marked: never again after a Shift+E, once after e (#295, #300)", async () => {
  // DECIDED: a message already Done is not marked Done again, but its chat service's half is asked,
  // `after_reply`, so the server's floor answers for a message it has marked through already and asks
  // the service nothing. A Done made by e, which leaves the service's marker, is read there by Shift+E.
  const { page, before, asked, said, again, afterE } = await readCollapseTwice();
  assert.equal(said[0], "Marked read here and in Discord, and collapsed.");
  // Pressed again on message 3, Done and marked through already.
  assert.equal(channelRow(page, "3").getAttribute("data-collapsed"), "true", "Shift+E did not fold the Done message");
  assert.equal(again.dismissed, 1, "Shift+E on a Done message wrote another Done");
  assert.deepStrictEqual(page.upstreamReadCalls.slice(0, 2), [{ message_id: "3", after_reply: true },
    { message_id: "3", after_reply: true }], "the second press was not asked as a reply's mark is, for the server's floor");
  assert.deepStrictEqual(again.asked, ["3"], "the second press on a message marked through asked the service again");
  assert.equal(said[1], "Already read here and in Discord, and collapsed.");
  assert.deepStrictEqual(page.restoreCalls, [], "Shift+E on a Done message put it back");
  // e asks the service nothing; Shift+E then on the message e marked Done reads it there, once.
  assert.equal(afterE, 2, "e asked the chat service");
  assert.equal(page.dismissCalls.length, before.dismissed + 2, "Shift+E on the message e marked Done wrote another Done");
  assert.deepStrictEqual(page.upstreamReadCalls.at(-1), { message_id: "4", after_reply: true });
  assert.deepStrictEqual(asked, ["3", "4"], "Shift+E on a message e marked Done did not read it in the service");
  assert.equal(said[2], "Already read here; now marked read in Discord too, and collapsed.");

  // THE CONTROLS. Without the already-Done check the second press writes a second Done. Without the
  // service's half for a Done message, the message e marked is never read in the service. Without
  // `after_reply`, the server's floor is not consulted, and the second press asks the service again.
  const twice = await readCollapseTwice(brokenScript(DONE_UNLESS_DONE, "  await markRowDone(ids, row);\n"));
  assert.equal(twice.again.dismissed, 2,
    "with the already-Done check taken out the second press wrote no second Done, so this test cannot see it");
  const skipped = await readCollapseTwice(brokenScript(ASKS_SERVICE, "  const mark = wasDone ? { service: \"Discord\", " +
    "asked: false, marked: false, already: false, why: \"\" } : await markReadInService(channel, ids[ids.length - 1]);\n"));
  assert.deepStrictEqual(skipped.asked, ["3"],
    "with a Done message asking nothing the message e marked was read in the service, so this test cannot see it");
  const owner = await readCollapseTwice(brokenScript("body: { message_id: String(id), after_reply: true }",
    "body: { message_id: String(id) }"));
  assert.deepStrictEqual(owner.again.asked, ["3", "3"],
    "without after_reply the second press did not reach the service, so this fake cannot tell the floor is used");
});

test("Shift+E asks the chat service nothing after a Done that failed, and the failure is said (#295, #300)", async () => {
  const refuse = (page) => {
    page.inboxWriteHook = async (kind) => (kind === "dismiss"
      ? json(502, { error: "store_error", detail: "the store is read-only" }) : null);
  };
  const { page } = await readCollapseMarked(SCRIPT, refuse);
  await page.settle();
  assert.equal(page.dismissCalls.length >= 1, true, "the premise is a Done asked for");
  assert.deepStrictEqual(page.upstreamReadCalls, [], "the chat service was asked after a Done that failed");
  assert.equal(channelRow(page, "3").getAttribute("data-archived"), "false", "a Done that failed left the row Done");
  assert.equal(page.el("error").hidden, false, "a Done that failed said nothing");
  assert.doesNotMatch(page.el("status").textContent, /^Marked read/, "a Done that failed was said as a read");
  // THE CONTROL: with the failure swallowed and no look again after its own Done (`#370
  // shift-e-undo-marker`), the service is asked all the same.
  const { page: control } = await readCollapseMarked(brokenScript(`${DONE_UNLESS_DONE}  if (!(await doneSaved(ids))) return;\n`,
    "  if (!wasDone) await markRowDone(ids, row).catch(() => {});\n"), refuse);
  await control.settle();
  assert.equal(control.upstreamReadCalls.length, 1,
    "with the failure swallowed nothing was asked, so this test cannot see it");
});

// A message is drawn Done the moment it is marked, before its write has even been sent. Shift+E pressed
// on it again meanwhile — the key twice in quick succession, or after e or the Done button — must not
// take that for a saved Done: the chat service's marker, once moved, cannot be moved back.

/** The line in `readAndCollapse` that waits for a Done still being saved, taken out for a negative control. */
const WAITS_FOR_SAVE = "  if (!(await doneSaved(ids))) return;\n";

/**
 * Message 3 opened and its Done made by `first`, Shift+E or e, held on its way to the server; Shift+E
 * pressed on it meanwhile; then the Done answered `answer`, or saved where that is null. Answers the
 * page, what was asked of the chat service while the Done was on its way, and the server's floor's asks
 * of the service.
 */
async function readCollapseWhileSaving(answer, first, script = SCRIPT) {
  let release = () => {};
  let asked = null;
  const { page, before } = await openThree(script, (p) => {
    p.upstreamReadMarkSupported = true;
    asked = serverFloor(p);
    p.inboxWriteHook = async (kind) => (kind === "dismiss"
      ? new Promise((resolve) => { release = () => resolve(answer); }) : null);
  });
  if (first === "e") {
    await pressHere(page, "e");
    await pressHere(page, "k");
  } else {
    await pressHere(page, "E", { shiftKey: true });
  }
  assert.deepStrictEqual(page.dismissCalls.slice(before.dismissed), [{ messages: ["3"] }],
    `the premise is ${first}'s Done of message 3 on its way`);
  assert.equal(selectedId(page), "3");
  assert.equal(channelRow(page, "3").getAttribute("data-archived"), "true", "the premise is the message drawn Done already");
  await pressHere(page, "E", { shiftKey: true });
  const whileSaving = [...page.upstreamReadCalls];
  release();
  await page.settle();
  await page.settle();
  return { page, before, whileSaving, asked };
}

test("SHIFT+E PRESSED WHILE A DONE IS STILL BEING SAVED asks the chat service only once it is saved, and nothing, and says no read, once it has failed (#295, #300)", async () => {
  const failing = json(502, { error: "store_error", detail: "the store is read-only" });
  for (const first of ["Shift+E", "e"]) {
    const failed = await readCollapseWhileSaving(failing, first);
    assert.deepStrictEqual(failed.whileSaving, [], `Shift+E after ${first} asked the chat service before the Done was saved`);
    assert.deepStrictEqual(failed.page.upstreamReadCalls, [], `Shift+E after ${first} asked the chat service after the Done failed`);
    assert.deepStrictEqual(failed.asked, [], "the chat service's marker was moved for a Done that failed");
    assert.equal(channelRow(failed.page, "3").getAttribute("data-archived"), "false", "a Done that failed left the row Done");
    assert.equal(failed.page.el("error").hidden, false, "a Done that failed said nothing");
    assert.doesNotMatch(failed.page.el("status").textContent, /read here/, "a Done that failed was said as a read");
    // Saved: asked once it is, as `after_reply`, and the service reached once for both presses.
    const saved = await readCollapseWhileSaving(null, first);
    assert.deepStrictEqual(saved.whileSaving, [], `Shift+E after ${first} asked the chat service before the Done was saved`);
    assert.equal(saved.page.dismissCalls.length, saved.before.dismissed + 1, "Shift+E on a message being saved wrote a second Done");
    assert.ok(saved.page.upstreamReadCalls.length >= 1 && saved.page.upstreamReadCalls.every((call) =>
      call.message_id === "3" && call.after_reply === true), `the saved Done was not marked in the chat service as a reply's mark: ${
      JSON.stringify(saved.page.upstreamReadCalls)}`);
    assert.deepStrictEqual(saved.asked, ["3"], "the chat service was not reached exactly once");
    assert.match(saved.page.el("status").textContent, /read here and in Discord, and collapsed\.$|^Already read here; now marked read in Discord too, and collapsed\.$/,
      "the saved Done's mark in the chat service was not said");
    assert.equal(channelRow(saved.page, "3").getAttribute("data-archived"), "true");
    assert.equal(saved.page.el("error").hidden, true, `the mark put up the error banner: ${saved.page.el("error").textContent}`);
  }

  // THE CONTROLS. Without the wait, the second press takes the Done being saved for a saved one and
  // asks the service at once, so its marker moves for a Done that then fails, and a read is said.
  // Waiting without looking again asks it once the Done has failed.
  for (const first of ["Shift+E", "e"]) {
    const unwaited = await readCollapseWhileSaving(failing, first, brokenScript(WAITS_FOR_SAVE, ""));
    assert.deepStrictEqual(unwaited.whileSaving, [{ message_id: "3", after_reply: true }],
      `without the wait Shift+E after ${first} did not ask before the Done was saved, so this test cannot see it`);
    assert.match(unwaited.page.el("status").textContent, /read here/,
      "without the wait no read was said, so this test cannot see it");
  }
  const unlooked = await readCollapseWhileSaving(failing, "Shift+E",
    brokenScript("if (saving.length === 0) return done();", "if (saving.length === 0) return true;"));
  assert.deepStrictEqual(unlooked.whileSaving, [], "the control waited no longer");
  assert.deepStrictEqual(unlooked.asked, ["3"],
    "waiting without looking again asked nothing after the failed Done, so this test cannot see it");
});

test("Shift+E on a channel that is one thread of a larger space marks it Done and asks the service nothing, and says why (#295, #300)", async () => {
  const run = async (script = SCRIPT) => {
    const page = newPage(new Map(), script);
    page.upstreamReadMarkSupported = true;
    const data = scopedData();
    serveScoped(page, data.summary);
    await signIn(page);
    await showDiscord(page, data.messages.slice(0, 3));
    page.el("scroll-area").scrollTop = 0;
    await press(page, page.el("scroll-area"), "ArrowDown");
    await pressHere(page, "j");
    assert.equal(selectedId(page), "302");
    await pressHere(page, "E", { shiftKey: true });
    await page.settle();
    return page;
  };
  const page = await run();
  assert.deepStrictEqual(page.dismissCalls, [{ messages: ["302"] }], "the Done did not stand");
  assert.deepStrictEqual(page.upstreamReadCalls, [],
    "Shift+E in one thread of a larger space asked to move the whole space's read marker");
  assert.equal(page.el("status").textContent,
    "Marked read here, not in Discord: this channel is one thread of a larger space there, and its read marker covers the whole space.");
  // THE CONTROL: without the page's own check it asks.
  const control = await run(brokenScript("  if (channelIsOneThread(channel)) return", "  if (false) return"));
  assert.deepStrictEqual(control.upstreamReadCalls, [{ message_id: "302", after_reply: true }],
    "with the check taken out nothing was asked, so this test cannot see it");
});

test("Shift+E says a chat service that refused or did not ask, with the reason, and the Done stands; a combined row is marked through its newest message (#295, #300)", async () => {
  // The server's answer that it asked the service nothing: the status line says not marked there, and why.
  const notAsked = (body) => json(200, { channel: CHANNEL, through: body.message_id, outcome: "scope_unknown",
    upstream_read_notice: "The source chat provider was not asked to move its read cursor." });
  const { page } = await readCollapseMarked(SCRIPT, (p) => { p.upstreamReadResponse = notAsked; });
  assert.deepStrictEqual(page.upstreamReadCalls, [{ message_id: "3", after_reply: true }], "the premise is the server asked");
  assert.equal(page.el("status").textContent,
    "Marked read here, not in Discord, and collapsed: it cannot say whether its read marker covers more than this channel.");
  // The service refusing: said on the status line, not in the error banner, and the Done stands.
  const down = (p) => { p.upstreamReadResponse = () => json(503, { error: "upstream_unavailable", detail: "Google Chat returned HTTP 429" }); };
  const { page: refused } = await readCollapseMarked(SCRIPT, down);
  assert.match(refused.el("status").textContent, /^Marked read here, not in Discord, and collapsed: /,
    "a mark the service refused was not said, or was said as a success");
  assert.equal(refused.el("error").hidden, true, `a refused mark put up the error banner: ${refused.el("error").textContent}`);
  assert.equal(channelRow(refused, "3").getAttribute("data-archived"), "true", "a refused mark took the Done back");

  // A combined row: Done for every message in it, and the service's marker through the newest.
  const combined = newPage();
  combined.upstreamReadMarkSupported = true;
  await signIn(combined);
  const rows = await showDiscord(combined, [
    message({ id: "8000000000000000100", content: "first half", timestamp: "2026-08-19T04:31:00.000Z" }),
    message({ id: "8000000000000000101", content: "second half", timestamp: "2026-08-19T04:31:01.000Z" }),
  ]);
  assert.equal(rows.length, 1, "the fixture did not produce one combined row");
  await press(combined, combined.el("scroll-area"), "ArrowDown");
  await pressHere(combined, "E", { shiftKey: true });
  await combined.settle();
  assert.deepStrictEqual(combined.dismissCalls, [{ messages: ["8000000000000000100", "8000000000000000101"] }]);
  assert.deepStrictEqual(combined.upstreamReadCalls, [{ message_id: "8000000000000000101", after_reply: true }],
    "the chat service's marker stopped before the end of the combined row");
  assert.equal(combined.el("status").textContent, "Marked read here and in Discord.", "a short row said it was collapsed");
});

test("NO KEY MARKS EVERY MESSAGE ABOVE ONE READ ANY MORE: not Shift+E, nor any key the page answers (#295)", async () => {
  // In the table the keys are answered from, nothing names the act, its ⋯ item, or the backlog's clear.
  const table = /const KEYMAP = \[([\s\S]*?)\n\];/.exec(SCRIPT_CODE);
  assert.ok(table, "web/voice.js has no KEYMAP table");
  assert.doesNotMatch(table[1], /markReadThroughHere|throughHere|row-read-through-button|clearBacklog|clear-backlog/,
    "a key still reaches Done through here or Clear the backlog");
  // Done through here's one caller is its ⋯ item's click: the definition and that call, nothing else.
  assert.equal([...SCRIPT_CODE.matchAll(/\bmarkReadThroughHere\(/g)].length, 2,
    "Done through here has a caller besides its ⋯ item");
  // Every chord of every binding in the table, pressed on the newest of six unread messages: nothing
  // marks more than that one message, and nothing marks through a boundary.
  const chords = [...table[1].matchAll(/\{\s*key:\s*"((?:[^"\\]|\\.)+)"([^}]*)\}/g)].map((m) => ({
    key: JSON.parse(`"${m[1]}"`), shiftKey: /shift:\s*true/.test(m[2]), ctrlKey: /mod:\s*true/.test(m[2]),
    altKey: /alt:\s*true/.test(m[2]),
  }));
  assert.ok(chords.length >= 40, `only ${chords.length} chords were found in KEYMAP`);
  assert.ok(chords.some((c) => c.key === "e" && c.shiftKey), "Shift+E is not among the chords swept");
  const sweep = async (script, only = null) => {
    const wide = [];
    for (const chord of only ? chords.filter(only) : chords) {
      const page = newPage(new Map(), script);
      await signIn(page);
      await showDiscord(page, backlog(6));
      await press(page, page.el("scroll-area"), "End");
      const { key, ...fields } = chord;
      await pressHere(page, key, fields);
      await page.settle();
      for (const call of page.dismissCalls) {
        if (call.through !== undefined || (call.messages || []).length > 1) wide.push({ chord, call });
      }
    }
    return wide;
  };
  assert.deepStrictEqual(await sweep(SCRIPT), [], "a key marked more than the message it was pressed on");
  // The control: Shift+E as it was. The sweep has to see it.
  const old = brokenScript("guardQuietly(() => readAndCollapse(pane, row, ids))();",
    "guardQuietly(() => markReadThroughHere(row, ids[ids.length - 1]))();");
  const caught = await sweep(old, (c) => c.key === "e");
  assert.ok(caught.some(({ chord }) => chord.shiftKey), "the sweep did not catch the old Shift+E, so it proves nothing");
});

test("DONE THROUGH HERE STILL WORKS FROM ITS ⋯ ITEM, with its Undo, and announces no key (#202, #295)", async () => {
  const page = await keyboardPage();
  const item = readThroughButton(channelRow(page, "5"));
  assert.ok(item, "the ⋯ menu has no Through here");
  assert.equal(item.getAttribute("aria-keyshortcuts"), null, "the ⋯ item still announces a key");
  assert.equal(item.textContent, "Through here");
  await item.click();
  await page.settle();
  assert.deepStrictEqual(page.dismissCalls.map((call) => call.messages), [["1", "2", "3", "4", "5"]],
    "Through here did not mark the message and every one above it");
  for (const id of ["1", "5"]) assert.equal(channelRow(page, id).getAttribute("data-archived"), "true");
  assert.equal(channelRow(page, "6").getAttribute("data-archived"), "false", "Through here marked a message below it");
  assert.equal(page.el("status").textContent, "Marked 5 messages read.");
  assert.equal(page.el("undo-dismiss").hidden, false, "Through here offered no Undo");
  assert.equal(page.el("undo-dismiss-label").textContent, "Undo Done (5)");
  await page.el("undo-dismiss").click();
  await page.settle();
  assert.deepStrictEqual(page.restoreCalls.map((call) => call.messages), [["1", "2", "3", "4", "5"]],
    "the Undo did not put back exactly what Through here marked");
  for (const id of ["1", "5"]) assert.equal(channelRow(page, id).getAttribute("data-archived"), "false");
  // And z, the Undo from the keyboard, as before.
  await readThroughButton(channelRow(page, "3")).click();
  await page.settle();
  await press(page, page.el("scroll-area"), "z");
  await page.settle();
  assert.deepStrictEqual(page.restoreCalls.at(-1).messages, ["1", "2", "3"], "z did not undo Through here");
});

// --- copying the highlighted message (#296 copy-highlighted-message) ------------------------------
//
// The owner, 2026-10-14: a keystroke that copies the highlighted message, with no text selected first.
// Cmd+C (Ctrl+C) stays the browser's wherever the browser has something to copy: text selected on the
// page, or the focus in a text field. Otherwise, with a message highlighted, it copies all of that
// message as written, a summarised or folded one too; a bare y does the same outside the fields; with
// nothing highlighted both are the browser's. The page answers the `copy` event the browser's own copy
// fires (`page.browserCopy`, modelled on Chromium 149, measured). tests/copy_message_browser.py presses
// the real keys in Chromium and reads the clipboard back.

/** What the clipboard holds before a test copies anything: a negative control's sentinel. */
const CLIPBOARD_BEFORE = "what was on the clipboard";

/**
 * A channel at its top with message `downs + 1` highlighted by the keys (↓, then j `downs` times), the
 * clipboard holding `CLIPBOARD_BEFORE`. `platform` is what `navigator.platform` says; `script` is
 * web/voice.js, or a copy broken on purpose for a negative control.
 */
async function copyPage({ messages = tallChannel(30), platform = "Linux x86_64", script = SCRIPT, downs = 1 } = {}) {
  const page = newPage(new Map(), script, (p) => p.setPlatform(platform));
  await signIn(page);
  await showDiscord(page, messages);
  page.el("scroll-area").scrollTop = 0;
  await press(page, page.el("scroll-area"), "ArrowDown");
  for (let i = 0; i < downs; i += 1) await pressHere(page, "j");
  page.clipboard = CLIPBOARD_BEFORE;
  return page;
}

/** That the browser's copy was left to the browser: the `copy` event not refused, nothing said. */
function browserKeptCopy(page, event, where) {
  assert.ok(!event.copy || !event.copy.defaultPrevented, `${where}: the page took the browser's copy`);
  assert.ok(!event.copy || !event.copy.beforeRefused, `${where}: the page refused beforecopy`);
  assert.notEqual(page.el("status").textContent, "Copied message.", `${where}: the page said it copied the message`);
}

test("`#296 copy-highlighted-message`: Ctrl+C with nothing selected copies all of the highlighted message, folded, by the browser's own copy", async () => {
  const page = await copyPage();
  const row = channelRow(page, "2");
  assert.deepStrictEqual(litIds(page), ["2"], "the fixture did not highlight message 2");
  // Folded: its opening lines, and how much the fold keeps back.
  assert.equal(row.getAttribute("data-collapsed"), "true", "message 2 is not folded; this proves nothing about a fold");
  assert.equal(elidedOf(row).hidden, false, "message 2 does not say what its fold hides");
  const event = await pressCopy(page, row);
  assert.equal(event.defaulted, true, "Ctrl+C was taken from the browser, whose copy event carries the message");
  assert.ok(event.copy, "the fixture's browser fired no copy");
  assert.equal(event.copy.defaultPrevented, true, "the page left the copy to the browser, which has nothing to copy");
  assert.equal(event.copy.beforeRefused, true, "beforecopy was not refused, which Safari needs to enable Copy");
  assert.equal(page.clipboard, longMessage("message 2"), "the copy is not all of the message as written");
  assert.doesNotMatch(page.clipboard, /more words|more characters/, "the copy holds the fold's count");
  assert.equal(page.clipboardHtml, `<div><p>${longMessage("message 2")}</p></div>`, "the rich flavour is not the body as drawn");
  assert.equal(page.el("status").textContent, "Copied message.");
  assert.equal(page.el("status-line").hidden, false, "the copy was not said");
  assert.match(HTML_CODE, /<span id="status" role="status" aria-live="polite">/, "the pill is not read out politely");
  assert.equal(page.execCopies + (page.clipboardWrites || 0), 0, "the copy event carried it, and a fallback copied again");
  assert.deepStrictEqual(litIds(page), ["2"], "the copy moved the highlight");
  assert.equal(focused(page), row, "the copy moved the focus");

  // Cmd+C on a Mac, through its Edit menu: the same.
  const mac = await copyPage({ platform: "MacIntel", downs: 3 });
  const cmd = await pressCopy(mac, channelRow(mac, "4"), { metaKey: true });
  assert.equal(cmd.defaulted, true);
  assert.equal(mac.clipboard, longMessage("message 4"), "Cmd+C on a Mac did not copy the highlighted message");
  assert.equal(mac.el("status").textContent, "Copied message.");

  // The mouse's highlight, as the keys': the message the mouse moved over.
  await mouseTo(page, channelRow(page, "5"), 100, 300);
  assert.deepStrictEqual(litIds(page), ["5"], "the mouse did not highlight message 5");
  await pressCopy(page, focused(page) || page.el("scroll-area"));
  assert.equal(page.clipboard, longMessage("message 5"), "Ctrl+C did not copy the message the mouse highlighted");
});

test("`#296 copy-highlighted-message`: a summarised message copies the message, not the summary; a rendered one its source and its rendering", async () => {
  const page = newPage();
  await signIn(page);
  const html = "<p><strong>bold</strong> &lt;b&gt;x&lt;/b&gt;</p>\n";
  const rows = await showDiscord(page, [
    message({ id: "7000000000000000020", content: longMessage("overnight") }),
    message({ id: "7000000000000000021", author: "alice", author_id: "1000000000000000002", author_is_bot: false,
      content: "**bold** <b>x</b>", content_html: html }),
    message({ id: "7000000000000000022", content: "plain <b>not markup</b> & \"quoted\"\nsecond line\n\nnext paragraph" }),
    // Two from one author, combined into one row: both, in order, as the ⋯ menu's Copy text gives them.
    message({ id: "7000000000000000023", author: "alice", author_id: "1000000000000000002", author_is_bot: false,
      content: "first half", timestamp: "2026-08-19T05:31:00.000Z" }),
    message({ id: "7000000000000000024", author: "alice", author_id: "1000000000000000002", author_is_bot: false,
      content: "second half", timestamp: "2026-08-19T05:31:01.000Z" }),
  ]);
  assert.equal(rows.length, 4, "the fixture's last two messages are not one combined row");
  await turnSummariesOn(page);
  assert.equal(rows[0].getAttribute("data-summarised"), "true", "the fixture never summarised");
  page.el("scroll-area").scrollTop = 0;
  await press(page, page.el("scroll-area"), "ArrowDown");
  assert.deepStrictEqual(litIds(page), ["7000000000000000020"]);
  page.clipboard = CLIPBOARD_BEFORE;
  await pressCopy(page, focused(page));
  assert.equal(page.clipboard, longMessage("overnight"), "a summarised row did not copy its message, whole");
  assert.notEqual(page.clipboard, summaryText(rows[0]), "a summarised row copied its summary");
  assert.doesNotMatch(page.clipboard, /a short line about/, "the copy holds the summary");
  // A body the server rendered: its source as the text, its rendering as the rich flavour.
  await pressHere(page, "j");
  await pressCopy(page, focused(page));
  assert.equal(page.clipboard, "**bold** <b>x</b>", "a rendered message did not copy its source");
  assert.equal(page.clipboardHtml, `<div>${html}</div>`, "the rich flavour is not the server's rendering");
  // A plain body: its text, and as plain paragraphs, every character markup means escaped.
  await pressHere(page, "j");
  await pressCopy(page, focused(page));
  assert.equal(page.clipboard, "plain <b>not markup</b> & \"quoted\"\nsecond line\n\nnext paragraph");
  assert.equal(page.clipboardHtml,
    "<div><p>plain &#60;b&#62;not markup&#60;/b&#62; &#38; &#34;quoted&#34;<br>second line</p><p>next paragraph</p></div>",
    "the rich flavour of a plain message is not its paragraphs, escaped");
  await pressHere(page, "j");
  await pressCopy(page, focused(page));
  assert.equal(page.clipboard, "first half\nsecond half", "a combined row did not copy both of its messages, in order");
  assert.equal(page.clipboardHtml, "<div><p>first half</p></div><div><p>second half</p></div>");
});

test("`#296 copy-highlighted-message`: NEGATIVE CONTROLS: text selected, a text field focused, nothing highlighted or a tap's silent selection leave the copy to the browser", async () => {
  const page = await copyPage();
  const row = channelRow(page, "2");
  // Words of the message selected: the browser copies those.
  await page.selectText(bodyOf(row), bodyOf(row), "a sentence about");
  let event = await pressCopy(page, row);
  browserKeptCopy(page, event, "with text selected");
  assert.equal(page.clipboard, "a sentence about", "with text selected, the copy is not the selection");
  // `#538 c-key-copy`: c and y with text selected copy the text, as the chord does, never the message.
  for (const key of ["c", "y"]) {
    page.clipboard = CLIPBOARD_BEFORE;
    event = await press(page, row, key);
    assert.equal(event.defaulted, false, `${key} with text selected was left to the browser`);
    assert.equal(page.clipboard, "a sentence about", `${key} with text selected did not copy the selection`);
  }
  // ...and let go of, the same key copies the message: it was the selection that held it back.
  await page.clearSelection();
  await pressCopy(page, row);
  assert.equal(page.clipboard, longMessage("message 2"), "with the selection gone, Ctrl+C did not copy the message");

  // In the channel's box, its own text selected: the browser copies that, and y is typed.
  page.clipboard = CLIPBOARD_BEFORE;
  page.el("status").textContent = "";
  const box = page.el("channel-compose-text");
  await box.setValue("half a thought");
  box.focus();
  box.select();
  event = await pressCopy(page, box);
  browserKeptCopy(page, event, "in the message box");
  assert.equal(page.clipboard, "half a thought", "in the box, the copy is not the box's selected text");
  box.selectedTimes = 0;
  page.clipboard = CLIPBOARD_BEFORE;
  event = await pressCopy(page, box);
  browserKeptCopy(page, event, "in the message box with nothing selected in it");
  assert.equal(page.clipboard, CLIPBOARD_BEFORE, "Ctrl+C in the box with nothing selected in it copied the message");
  event = await press(page, box, "y");
  assert.equal(event.defaulted, true, "y in the message box was taken from it");
  assert.equal(page.clipboard, CLIPBOARD_BEFORE, "y in the message box copied the message");
  // The search field too.
  await press(page, page.el("scroll-area"), "/");
  event = await pressCopy(page, page.el("search-field"));
  browserKeptCopy(page, event, "in the search field");
  assert.equal(page.clipboard, CLIPBOARD_BEFORE, "Ctrl+C in the search field copied the message");
  await press(page, page.el("search-field"), "Escape");

  // Settings, over the list: the message highlighted behind it is not on screen.
  await press(page, page.el("scroll-area"), ",");
  assert.equal(page.screen(), "settings");
  event = await pressCopy(page, page.el("screen-settings"));
  browserKeptCopy(page, event, "on Settings");
  assert.equal(page.clipboard, CLIPBOARD_BEFORE, "Ctrl+C on Settings copied the message behind it");
  await press(page, page.el("screen-settings"), "Escape");

  // Nothing highlighted: a page fresh from its load, then a tap's silent selection.
  const fresh = newPage();
  await signIn(fresh);
  await showDiscord(fresh, tallChannel(30));
  fresh.clipboard = CLIPBOARD_BEFORE;
  assert.deepStrictEqual(litIds(fresh), [], "a message is highlighted on load; this proves nothing");
  event = await pressCopy(fresh, fresh.el("scroll-area"));
  browserKeptCopy(fresh, event, "with nothing highlighted");
  assert.equal(fresh.clipboard, CLIPBOARD_BEFORE, "Ctrl+C with nothing highlighted copied something");
  // `#538 c-key-copy`: c and y with nothing to copy say so, and copy nothing.
  for (const key of ["c", "y"]) {
    fresh.el("status").textContent = "";
    event = await press(fresh, fresh.el("scroll-area"), key);
    assert.equal(fresh.execCopies, 0, `${key} with nothing highlighted ran a copy`);
    assert.equal(fresh.clipboard, CLIPBOARD_BEFORE, `${key} with nothing highlighted copied something`);
    assert.equal(fresh.el("status").textContent, "Nothing to copy: select a message, or some text, first.",
      `${key} with nothing highlighted said nothing`);
  }
  fresh.el("scroll-area").scrollTop = 0;
  await pressOn(fresh, channelRow(fresh, "3"), "touch");
  assert.deepStrictEqual(litIds(fresh), [], "a tap lit the message it selected");
  event = await pressCopy(fresh, fresh.el("scroll-area"));
  browserKeptCopy(fresh, event, "after a tap's silent selection");
  assert.equal(fresh.clipboard, CLIPBOARD_BEFORE, "Ctrl+C copied a message a tap selected without lighting it");
});

test("`#296 copy-highlighted-message`: where the browser fires no copy, the message goes by execCommand, the Clipboard API or a textarea, and a refusal is said", async () => {
  // Cmd+C off a Mac: no menu copies with it, and the browser fires no copy for the key.
  const page = await copyPage();
  const row = channelRow(page, "2");
  let event = await pressCopy(page, row, { metaKey: true }, false);
  assert.equal(event.defaulted, true);
  assert.equal(page.execCopies, 1, "the page did not ask for the copy itself once no copy event came");
  assert.equal(page.clipboard, longMessage("message 2"), "execCommand's copy event did not carry the message");
  assert.equal(page.clipboardHtml, `<div><p>${longMessage("message 2")}</p></div>`);
  // A browser whose execCommand fires nothing with nothing selected: the Clipboard API, as plain text.
  page.copyFiresNothing = true;
  page.clipboard = CLIPBOARD_BEFORE;
  page.el("status").textContent = "";
  event = await pressCopy(page, row);
  assert.equal(event.copy, null, "the fixture's browser fired a copy; this proves nothing about the fallback");
  assert.equal(page.clipboard, longMessage("message 2"), "the Clipboard API did not carry the message");
  assert.equal(page.clipboardHtml, undefined);
  assert.equal(page.el("status").textContent, "Copied message.");
  // That refusing as well: a hidden textarea, the focus given back to the message.
  page.clipboardRefuses = true;
  page.clipboard = CLIPBOARD_BEFORE;
  event = await pressCopy(page, row);
  assert.equal(page.clipboard, longMessage("message 2"), "the textarea did not carry the message");
  assert.equal(focused(page), row, `the textarea kept the focus from the message: ${focusName(page)}`);
  assert.equal(page.document.body.children.length, 0, "the textarea was left in the page");
  assert.deepStrictEqual(litIds(page), ["2"]);
  // Everything refusing: said, and the clipboard left as it was.
  page.execCopyRefuses = true;
  page.clipboard = CLIPBOARD_BEFORE;
  event = await pressCopy(page, row);
  assert.equal(page.clipboard, CLIPBOARD_BEFORE);
  assert.equal(page.el("status").textContent, "This browser would not copy to the clipboard.");
});

test("`#296 copy-highlighted-message`: y copies the highlighted message, is turned off with the single keys, and both keys are listed", async () => {
  const page = await copyPage();
  const row = channelRow(page, "2");
  let event = await press(page, row, "y");
  assert.equal(event.defaulted, false, "y on a highlighted message was left to the browser");
  assert.equal(page.execCopies, 1, "y did not ask for the browser's copy");
  assert.equal(page.clipboard, longMessage("message 2"), "y did not copy the highlighted message");
  assert.equal(page.clipboardHtml, `<div><p>${longMessage("message 2")}</p></div>`);
  assert.equal(page.el("status").textContent, "Copied message.");
  // `#538 c-key-copy`: c the same, the owner's lazier Cmd+C; it no longer goes to the message box.
  page.clipboard = CLIPBOARD_BEFORE;
  page.el("status").textContent = "";
  event = await press(page, row, "c");
  assert.equal(event.defaulted, false, "c on a highlighted message was left to the browser");
  assert.equal(page.clipboard, longMessage("message 2"), "c did not copy the highlighted message");
  assert.equal(page.clipboardHtml, `<div><p>${longMessage("message 2")}</p></div>`);
  assert.equal(page.el("status").textContent, "Copied message.");
  assert.notEqual(focused(page), page.el("channel-compose-text"), "c went to the message box");
  // Y with Shift held is another key, and nothing.
  page.clipboard = CLIPBOARD_BEFORE;
  event = await press(page, row, "Y", { shiftKey: true });
  assert.equal(event.defaulted, true, "Shift+Y was taken");
  assert.equal(page.clipboard, CLIPBOARD_BEFORE);
  // The list of keys, in the words of each keyboard, and what a screen reader hears on the row's Copy text.
  const listed = (one) => one.el("keyboard-keys").descendants().filter((node) => node.tagName === "kbd").map((node) => node.textContent);
  assert.ok(listed(page).includes("Ctrl+C, c, y"), `the list of keys does not name Ctrl+C, c and y: ${listed(page).join(" | ")}`);
  const entry = page.el("keyboard-keys").descendants().find((node) => node.tagName === "kbd" && node.textContent === "Ctrl+C, c, y");
  assert.match(entry.parentNode.text(), /highlighted message/);
  assert.match(entry.parentNode.text(), /first message/, "the list does not say what a card copies");
  const copyButton = row.descendants().find((node) => node.className === "row-copy-button");
  assert.equal(copyButton.getAttribute("aria-keyshortcuts"), "Control+C Meta+C C Y");
  const mac = await copyPage({ platform: "MacIntel" });
  assert.ok(listed(mac).includes("Cmd+C, c, y"), `on a Mac the list does not say Cmd+C: ${listed(mac).join(" | ")}`);
  // Single keys off: y is nothing, and Ctrl+C still copies.
  await page.el("single-keys").setChecked(false);
  await page.settle();
  page.clipboard = CLIPBOARD_BEFORE;
  event = await press(page, row, "y");
  assert.equal(event.defaulted, true, "y with the single keys off was taken");
  assert.equal(page.clipboard, CLIPBOARD_BEFORE, "y copied with the single keys off");
  assert.equal(channelRow(page, "2").descendants().find((node) => node.className === "row-copy-button")
    .getAttribute("aria-keyshortcuts"), "Control+C Meta+C");
  await pressCopy(page, row);
  assert.equal(page.clipboard, longMessage("message 2"), "Ctrl+C stopped copying with the single keys off");
});

test("`#296 copy-highlighted-message`: in All, in a thread, on the thread list's cards and in two columns, the highlighted message is the one copied, a card's its first message", async () => {
  const page = await threadPage();
  assert.equal(page.el("thread-select").value, "flat", "the fixture does not open in All");
  page.el("scroll-area").scrollTop = 0;
  await press(page, page.el("scroll-area"), "ArrowDown");
  await pressHere(page, "j");
  assert.deepStrictEqual(litIds(page), ["201"]);
  await pressCopy(page, focused(page));
  assert.equal(page.clipboard, "first thread root", "Ctrl+C in All did not copy the highlighted message");
  // Into its thread, onto the reply.
  await press(page, focused(page), "ArrowRight");
  await page.settle();
  assert.equal(page.el("thread-heading").hidden, false, "→ did not open the thread");
  await press(page, page.el("scroll-area"), "End");
  assert.deepStrictEqual(litIds(page), ["202"]);
  await pressCopy(page, focused(page));
  assert.equal(page.clipboard, "first thread answer", "Ctrl+C in a thread did not copy the highlighted reply");
  await press(page, focused(page), "Escape");
  await page.settle();

  // The thread list: a card copies its thread's first message, whatever the card says of it.
  await press(page, page.el("scroll-area"), "3", { ctrlKey: true });
  await page.settle();
  assert.equal(page.el("screen-main").getAttribute("data-columns"), "list");
  await press(page, page.el("side-scroll"), "ArrowDown");
  const card = focused(page);
  assert.ok(card && card.hasAttribute("data-card"), `↓ in the thread list did not select a card: ${focusName(page)}`);
  assert.deepStrictEqual(litRows(page), [`side-cards:${card.getAttribute("data-id")}`]);
  const root = threadData().threads.find((thread) => thread.id === card.getAttribute("data-id")).root.content;
  page.clipboard = CLIPBOARD_BEFORE;
  await pressCopy(page, card);
  assert.equal(page.clipboard, root, "Ctrl+C on a card did not copy its thread's first message");
  page.clipboard = CLIPBOARD_BEFORE;
  await press(page, card, "y");
  assert.equal(page.clipboard, root, "y on a card did not copy its thread's first message");

  // Two columns: a card on the right, then a message of the thread it opens.
  const desk = await threadPage();
  await twoColumns(desk);
  await press(desk, desk.el("scroll-area"), "]");
  await press(desk, focused(desk), "j");
  const first = desk.el("side-cards").children[0];
  assert.deepStrictEqual(litRows(desk), [`side-cards:${first.getAttribute("data-id")}`]);
  await pressCopy(desk, first);
  assert.equal(desk.clipboard, threadData().threads.find((thread) => thread.id === first.getAttribute("data-id")).root.content,
    "Ctrl+C on a card on the right did not copy its first message");
  await press(desk, first, "ArrowRight");
  await desk.settle();
  await press(desk, desk.el("side-scroll"), "End");
  const reply = litRows(desk);
  assert.equal(reply.length, 1);
  assert.match(reply[0], /^side-log:/, `End on the right highlighted ${reply}`);
  await pressCopy(desk, focused(desk));
  assert.equal(desk.clipboard, "first thread answer", "Ctrl+C in the right column did not copy its highlighted message");
});

test("`#296 copy-highlighted-message`: NEGATIVE CONTROLS on the page itself: a page that ignores the highlight, or the selection, fails these checks", async () => {
  /** Ctrl+C on message 2 highlighted, then with words of it selected: what each left on the clipboard. */
  const observe = async (script) => {
    const page = await copyPage({ script });
    const row = channelRow(page, "2");
    await pressCopy(page, row);
    const plain = page.clipboard;
    page.clipboard = CLIPBOARD_BEFORE;
    await page.selectText(bodyOf(row), bodyOf(row), "a sentence about");
    await pressCopy(page, row);
    return [plain, page.clipboard];
  };
  const mutant = (from, to) => {
    assert.ok(SCRIPT.includes(from), `web/voice.js no longer says ${JSON.stringify(from)}`);
    return SCRIPT.replace(from, to);
  };
  assert.deepStrictEqual(await observe(SCRIPT), [longMessage("message 2"), "a sentence about"]);
  // Highlight ignored: nothing of the message is copied, and the browser has nothing to copy.
  assert.deepStrictEqual(await observe(mutant("const row = pane ? pane.lit : null;", "const row = null;")),
    [CLIPBOARD_BEFORE, "a sentence about"], "a page that ignores the highlight passed");
  // Selection ignored: the message is copied over the words the reader selected.
  assert.deepStrictEqual(
    await observe(mutant('const browserCopies = () => isTextField(document.activeElement) || selectedText() !== "";',
      "const browserCopies = () => isTextField(document.activeElement);")),
    [longMessage("message 2"), longMessage("message 2")], "a page that copies the message over a selection passed");
});

test("m FOCUSES THE CHANNEL'S BOX; ↑ in it while empty selects the newest message, and with text is the caret's", async () => {
  // `#538 c-key-copy`: m, c being the copy now.
  const page = await keyboardPage();
  let event = await press(page, page.el("scroll-area"), "m");
  assert.equal(event.defaulted, false);
  assert.equal(focused(page), page.el("channel-compose-text"), `m left the focus on ${focusName(page)}`);
  event = await pressHere(page, "ArrowUp");
  assert.equal(event.defaulted, false, "↑ in the empty box was left to it");
  assert.equal(selectedId(page), "30", `↑ in the empty box selected ${selectedId(page)}, not the newest message`);
  await page.el("channel-compose-text").setValue("a line");
  event = await press(page, page.el("channel-compose-text"), "ArrowUp");
  assert.equal(event.defaulted, true, "↑ in a box with text was taken from the caret");
  assert.equal(focused(page), page.el("channel-compose-text"));
});

test("HOME AND END over the channel take the list to its ends and select the message there", async () => {
  const page = await keyboardPage();
  const area = page.el("scroll-area");
  let event = await press(page, area, "Home");
  assert.equal(event.defaulted, false, "Home over the channel was left to the browser");
  assert.equal(area.scrollTop, 0, "Home did not take the list to its top");
  assert.equal(selectedId(page), "1", `Home selected ${selectedId(page)}`);
  event = await pressHere(page, "End");
  assert.ok(atBottomOf(area), "End did not take the list to its newest message");
  assert.equal(selectedId(page), "30", `End selected ${selectedId(page)}`);
  assert.equal(page.el("jump-newest").hidden, true, "End left the jump to the newest standing");
});

/** The channel's row whose box takes in viewport y `y` now. */
const rowAtY = (page, y) => page.el("discord-log").children.find((li) => {
  const box = li.getBoundingClientRect();
  return box.top <= y && box.bottom > y;
});

test("A TAP SELECTS SILENTLY: no focus, nothing lit, and the next key starts from the message tapped", async () => {
  // A touch screen has no hover, and a row lit by a tap could not be put out: as before `#239 single-selection`.
  const page = await keyboardPageAtTop();
  const area = page.el("scroll-area");
  area.focus();
  const row = channelRow(page, "1");
  await pressOn(page, row, "touch");
  assert.equal(focused(page), area, "a tap on a message gave the row the focus");
  assert.equal(row.getAttribute("tabindex"), null, "a tap on a message made the row a Tab stop");
  assert.deepStrictEqual(litIds(page), [], "a tap lit the message it selected");
  await press(page, area, "ArrowDown");
  assert.equal(selectedId(page), "2", `after a tap on 1, ↓ selected ${selectedId(page)}`);
  assert.deepStrictEqual(litIds(page), ["2"]);
  // A tap elsewhere puts the keys' message out: the selection is the tapped one now, and silent.
  await pressOn(page, channelRow(page, "5"), "touch");
  assert.deepStrictEqual(litIds(page), [], "a tap left the keys' message lit");
  // A finger moving over the list is not a mouse: it selects nothing.
  await page.documentEvent("pointermove", { target: channelRow(page, "3"), pointerType: "touch", clientX: 9, clientY: 9 });
  assert.deepStrictEqual(litIds(page), []);
});

test("`#239 single-selection`: a CLICK, then ↓ — the selection moves on from the message clicked, and one message is lit", async () => {
  const page = await keyboardPageAtTop();
  const area = page.el("scroll-area");
  area.focus();
  // The mouse arrives over message 1, and it is the one lit: the selection, drawn.
  await mouseTo(page, channelRow(page, "1"), 100, 200);
  assert.deepStrictEqual(litIds(page), ["1"], "the message under the mouse is not the one lit");
  assert.equal(selectedId(page), "1", "the focus is not where the selection is");
  await pressOn(page, channelRow(page, "1"));
  assert.deepStrictEqual(litIds(page), ["1"], "a click on the lit message lit another");
  // THE DEFECT: ↓ lit 2 with the ring and left 1 lit under the mouse.
  await pressHere(page, "ArrowDown");
  assert.equal(selectedId(page), "2");
  assert.deepStrictEqual(litIds(page), ["2"], "↓ left the clicked message lit beside the keys' one");
  assert.equal(page.documentElement.getAttribute("data-input"), "keys", "the root does not say the keys are in use");
  // A click with no move before it (a touchpad's tap where the pointer already was), then j.
  area.scrollTop = 0;
  await pressOn(page, channelRow(page, "1"));
  assert.deepStrictEqual(litIds(page), ["1"], "a click did not take the selection from the keys");
  assert.equal(selectedId(page), "1");
  assert.equal(page.documentElement.getAttribute("data-input"), null, "a mouse press left the keys' look up");
  await pressHere(page, "j");
  assert.deepStrictEqual(litIds(page), ["2"]);
  const tabbed = page.el("discord-log").children.filter((li) => li.getAttribute("tabindex") === "0");
  assert.deepStrictEqual(tabbed.map((li) => li.getAttribute("data-id")), ["2"], "more than the selected row is a Tab stop");
});

test("`#239 single-selection`: ↓, then a REAL MOUSE MOVE — the message under the mouse takes the selection, the focus with it", async () => {
  const page = await keyboardPageAtTop();
  await press(page, page.el("scroll-area"), "ArrowDown");
  await pressHere(page, "j");
  assert.deepStrictEqual(litIds(page), ["2"]);
  assert.equal(page.documentElement.getAttribute("data-input"), "keys");
  await mouseTo(page, channelRow(page, "1"), 120, 150);
  assert.deepStrictEqual(litIds(page), ["1"], "the mouse did not take the selection from the keys");
  assert.equal(selectedId(page), "1", "the focus stayed on the keys' message");
  assert.equal(page.documentElement.getAttribute("data-input"), null, "the keys' look outlived a real mouse move");
  assert.equal(channelRow(page, "2").getAttribute("tabindex"), null, "the keys' message is still a Tab stop");
  // The next key moves on from where the mouse put it.
  await pressHere(page, "j");
  assert.deepStrictEqual(litIds(page), ["2"]);
  // A drag, a button held, is text being selected: it chooses no message.
  await mouseTo(page, channelRow(page, "1"), 130, 90, { buttons: 1 });
  assert.deepStrictEqual(litIds(page), ["2"], "a drag over the list moved the selection");
  // Within the row already selected, a move leaves the focus on the row's control it is on.
  await mouseTo(page, channelRow(page, "2"), 131, 300);
  const reply = replyButton(channelRow(page, "2"));
  reply.focus();
  await mouseTo(page, channelRow(page, "2"), 140, 305);
  assert.equal(focused(page), reply, "a move inside the selected row took the focus off its button");
});

test("`#239 single-selection`: A LIST SCROLLED UNDER A STILL MOUSE takes nothing — the keys keep the selection as rows slide under the pointer", async () => {
  const page = await keyboardPageAtTop();
  const area = page.el("scroll-area");
  const y = 200;
  await mouseTo(page, rowAtY(page, y), 150, y);
  assert.deepStrictEqual(litIds(page), [rowAtY(page, y).getAttribute("data-id")]);
  for (let i = 0; i < 15; i += 1) await pressHere(page, "j");
  const keyed = selectedId(page);
  assert.ok(area.scrollTop > 0, "the list did not scroll, so nothing slid under the pointer");
  // A browser that looks again at what is under a pointer that has not moved reports it from the
  // same place, over whatever row has slid there.
  const under = rowAtY(page, y);
  assert.notEqual(under.getAttribute("data-id"), keyed);
  await mouseTo(page, under, 150, y, { movementX: 0, movementY: 0 });
  assert.equal(selectedId(page), keyed, "a list scrolled under a still pointer took the selection from the keys");
  assert.deepStrictEqual(litIds(page), [keyed], "a row slid under a still pointer was lit");
  assert.equal(page.documentElement.getAttribute("data-input"), "keys", "a pointer that did not move put the keys' look away");
  // A pixel of movement is the reader reaching for the mouse.
  await mouseTo(page, under, 151, y);
  assert.deepStrictEqual(litIds(page), [under.getAttribute("data-id")]);
  // The first move after the load, once the keys are in use, counts only if it says it moved: it is
  // the one a key's scroll can make before the mouse has moved at all.
  const fresh = await keyboardPageAtTop();
  await press(fresh, fresh.el("scroll-area"), "ArrowDown");
  await mouseTo(fresh, channelRow(fresh, "2"), 50, 300, { movementX: 0, movementY: 0 });
  assert.deepStrictEqual(litIds(fresh), ["1"], "a first move that did not move took the selection from the keys");
  await mouseTo(fresh, channelRow(fresh, "2"), 52, 300, { movementX: 2, movementY: 0 });
  assert.deepStrictEqual(litIds(fresh), ["2"]);
});

test("`#239 single-selection`: Enter, e and the rest act on THE ONE SELECTION — the message the mouse moved to after the keys", async () => {
  const page = await keyboardPageAtTop();
  await press(page, page.el("scroll-area"), "ArrowDown");
  assert.equal(selectedId(page), "1");
  await mouseTo(page, channelRow(page, "2"), 100, 300);
  assert.equal(channelRow(page, "2").getAttribute("data-collapsed"), "true", "a long message did not arrive folded");
  await pressHere(page, "Enter");
  assert.equal(channelRow(page, "2").getAttribute("data-collapsed"), "false", "Enter did not open the message the mouse selected");
  assert.equal(channelRow(page, "1").getAttribute("data-collapsed"), "true", "Enter opened the message the keys had left");
  await pressHere(page, "e");
  assert.deepStrictEqual(page.dismissCalls.map((call) => call.messages), [["2"]], "e marked another message than the one lit");
  assert.deepStrictEqual(litIds(page), ["3"], "after e the selection did not move on, lit, to the next unread");
});

test("`#239 single-selection`: the mouse never takes the focus out of a message box, and moves nothing while a ⋯ menu is open", async () => {
  const page = await keyboardPageAtTop();
  const box = page.el("channel-compose-text");
  box.focus();
  await mouseTo(page, channelRow(page, "2"), 100, 40);
  assert.equal(focused(page), box, "a mouse move over the list took the focus out of the box being typed in");
  assert.deepStrictEqual(litIds(page), ["2"], "the message under the mouse is not the one lit");
  // Esc leaves the box, its draft kept, for the message lit.
  await box.setValue("half a thought");
  await press(page, box, "Escape");
  assert.equal(selectedId(page), "2", `Esc from the box left the focus on ${selectedId(page)}`);
  // A ⋯ menu open on 2 holds the focus and the arrows: the mouse over 4 moves neither.
  await pressHere(page, ".");
  const item = focused(page);
  assert.ok(nodeIn(rowMoreMenu(channelRow(page, "2")), item), "the focus is not in the menu");
  await mouseTo(page, channelRow(page, "4"), 110, 150);
  assert.deepStrictEqual(litIds(page), ["2"], "the mouse moved the selection from under an open menu");
  assert.equal(focused(page), item, "the mouse took the focus out of an open menu");
});

test("`#239 single-selection`: A CONTROL A CLICK FOCUSED gives the focus up to the message the mouse moves to, and Enter acts on that message; one the keys reached keeps it", async () => {
  // Found in review, in Chromium: a click on Summarise or Show read left the focus on the button,
  // with no ring (a click's focus is not `:focus-visible`); the mouse moved over a message, which was
  // drawn selected, ring and all; and Enter pressed the button again, the lit message untouched.
  for (const id of ["summarise", "todo-filter"]) {
    const page = await keyboardPageAtTop();
    const control = page.el(id);
    await page.documentEvent("pointerdown", { target: control, pointerType: "mouse" });
    control.focusFrom("pointer");
    await mouseTo(page, channelRow(page, "2"), 100, 300);
    assert.deepStrictEqual(litIds(page), ["2"], "the message under the mouse is not the one lit");
    assert.equal(selectedId(page), "2", `#${id}, focused by a click, kept the focus from the message lit`);
    assert.equal(channelRow(page, "2").getAttribute("data-collapsed"), "true", "a long message did not arrive folded");
    await pressHere(page, "Enter");
    assert.equal(channelRow(page, "2").getAttribute("data-collapsed"), "false", `Enter after a click on #${id} did not open the message lit`);
  }
  // A control the keys reached is a reader working by keyboard: a mouse nudged on the desk moves the
  // selection, and the focus stays where Tab put it.
  const page = await keyboardPageAtTop();
  const summarise = page.el("summarise");
  summarise.focusFrom("keyboard");
  await mouseTo(page, channelRow(page, "3"), 100, 300);
  assert.deepStrictEqual(litIds(page), ["3"]);
  assert.equal(focused(page), summarise, "the mouse took the focus from a control the keys had reached");
  await mouseTo(page, channelRow(page, "3"), 104, 302);
  assert.equal(focused(page), summarise, "a second move in the lit message took the focus from the control the keys reached");
  // Nor ever from a picker or the pace box, however it was focused: a picker's keys are its own.
  let y = 310;
  for (const id of ["discord-channel", "read-speed-box"]) {
    page.el(id).focusFrom("pointer");
    y += 4;
    await mouseTo(page, channelRow(page, "4"), 100, y);
    assert.equal(focused(page), page.el(id), `the mouse took the focus from #${id}`);
  }
});

test("`#239 single-selection`: a key a field took first still says the keys are in use; a finger says touch until the mouse moves or a key is pressed", async () => {
  const page = await keyboardPageAtTop();
  const root = page.documentElement;
  await mouseTo(page, channelRow(page, "2"), 100, 300);
  assert.equal(root.getAttribute("data-input"), null);
  // Escape out of the search is the field's own, and the page's listener hears it already taken.
  await page.el("search-toggle").click();
  await page.settle();
  const escape = await press(page, page.el("search-field"), "Escape");
  assert.equal(escape.defaultPrevented, true, "the search field no longer takes its own Escape; this proves nothing");
  assert.equal(root.getAttribute("data-input"), "keys", "Escape out of the search left a still pointer's look up");
  // A touch laptop has a mouse as well, and its pointer queries answer as a desk's: a finger is said
  // on the root, which web/voice.css gives the focus tint a touch screen has.
  await page.documentEvent("pointerdown", { target: channelRow(page, "3"), pointerType: "touch" });
  assert.equal(root.getAttribute("data-input"), "touch", "a finger did not say so");
  await mouseTo(page, channelRow(page, "3"), 120, 310);
  assert.equal(root.getAttribute("data-input"), null, "a real mouse move left the finger's look up");
  await page.documentEvent("pointerdown", { target: channelRow(page, "3"), pointerType: "pen" });
  assert.equal(root.getAttribute("data-input"), "touch", "a pen did not say so");
  await pressHere(page, "j");
  assert.equal(root.getAttribute("data-input"), "keys");
});

test("`#239 single-selection`: A COLLAPSED RUN'S LINE, under the mouse, is its row's selection, and j moves on past the run", async () => {
  const page = newPage();
  const messages = runThread(page, { tail: RUN_TAIL });
  await enterRunThread(page, messages, () => chooseReadMode(page, "Fold read"));
  const rows = page.el("discord-log").children;
  const head = rows[1];
  assert.equal(head.getAttribute("data-read-run"), "head");
  const line = readRunLineOf(head);
  await mouseTo(page, line, 100, 140);
  assert.deepStrictEqual(litIds(page), [head.getAttribute("data-id")], "the line under the mouse did not select its row");
  assert.ok(nodeIn(head, focused(page)), `the focus is not on the run's row: ${focusName(page)}`);
  // The run's hidden members are not rows the keys stop on: j goes to the first message after it.
  const after = rows.find((row, at) => at > 1 && !row.hidden && row.getAttribute("data-read-run") === null);
  await pressHere(page, "j");
  assert.equal(selectedId(page), after.getAttribute("data-id"), `j from the run's line selected ${selectedId(page)}`);
  assert.deepStrictEqual(litIds(page), [after.getAttribute("data-id")], "the run's line stayed lit beside the keys' message");
  await pressHere(page, "k");
  assert.deepStrictEqual(litIds(page), [head.getAttribute("data-id")], "k did not come back to the run's line");
});

test("`#239 single-selection`: A MESSAGE ARRIVING UNDER A STILL MOUSE takes nothing — the redraw's report from the same place selects no row", async () => {
  const messages = tallChannel(30);
  const page = await keyboardPageAtTop(messages);
  const y = 200;
  await mouseTo(page, rowAtY(page, y), 150, y);
  const pointed = litIds(page)[0];
  await pressHere(page, "j");
  await pressHere(page, "j");
  const keyed = selectedId(page);
  assert.notEqual(keyed, pointed);
  // A re-read that brings a new message redraws every row, and a browser that looks again at what is
  // under the pointer reports a move from where it already was, over the new row standing there.
  await refreshDiscord(page, [...messages, message({ id: "31", content: "a new message" })]);
  await mouseTo(page, rowAtY(page, y), 150, y, { movementX: 0, movementY: 0 });
  assert.equal(selectedId(page), keyed, "a redraw under a still pointer took the selection from the keys");
  assert.deepStrictEqual(litIds(page), [keyed], "a row redrawn under a still pointer was lit");
  assert.equal(page.documentElement.getAttribute("data-input"), "keys");
});

test("`#239 single-selection`: THE ROW A JUMP LANDS ON is the selection, and its flash goes out when the selection moves on", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, questionFarAbove());
  const question = rowWithId(page, "1000000000000000001");
  const reply = rowWithId(page, "1000000000000000099");
  await mouseTo(page, reply, 100, 300);
  assert.deepStrictEqual(litIds(page), ["1000000000000000099"]);
  await page.documentEvent("pointerdown", { target: replyArrowOf(reply), pointerType: "mouse" });
  await tapArrow(page, reply);
  assert.equal(question.getAttribute("data-landed"), "true", "nothing says which row the jump was for");
  assert.deepStrictEqual(litIds(page), ["1000000000000000001"], "the reply stayed lit beside the message the jump landed on");
  assert.equal(selectedId(page), "1000000000000000001", "the focus is not on the message the jump landed on");
  // The keys act on it.
  await pressHere(page, "e");
  assert.deepStrictEqual(page.dismissCalls.map((call) => call.messages), [["1000000000000000001"]]);
  // The mouse moves on, and the flash goes out with the selection, not two seconds later beside it.
  await tapArrow(page, reply);
  assert.equal(question.getAttribute("data-landed"), "true");
  const other = rowWithId(page, "1000000000000000010");
  await mouseTo(page, other, 101, 100);
  assert.deepStrictEqual(litIds(page), ["1000000000000000010"]);
  assert.equal(question.getAttribute("data-landed"), null, "the landing's flash stayed lit beside the new selection");
  // After a tap on a touch screen the landing is selected silently, and its flash is the only light.
  const phone = newPage();
  await signIn(phone);
  await showDiscord(phone, questionFarAbove());
  await phone.documentEvent("pointerdown", { target: replyArrowOf(rowWithId(phone, "1000000000000000099")), pointerType: "touch" });
  await tapArrow(phone, rowWithId(phone, "1000000000000000099"));
  assert.equal(rowWithId(phone, "1000000000000000001").getAttribute("data-landed"), "true");
  assert.deepStrictEqual(litIds(phone), [], "a tap's landing was drawn as a selection on a touch screen");
});

test("A KEY ABOUT ONE MESSAGE never acts on one scrolled out of sight: it brings the ring to the screen instead", async () => {
  const page = await keyboardPageAtTop();
  const area = page.el("scroll-area");
  await press(page, area, "ArrowDown");
  assert.equal(selectedId(page), "1");
  area.scrollTop = area.scrollHeight;
  const event = await pressHere(page, "e");
  assert.equal(event.defaulted, false);
  assert.deepStrictEqual(page.dismissCalls, [], "e marked Done a message the reader could not see");
  assert.equal(statusText(page), "Selected the first message showing.", "nothing said why e did not act");
  const now = channelRow(page, selectedId(page));
  assert.ok(now && selectedId(page) !== "1", `the ring did not come to the screen: the focus is on ${selectedId(page)}`);
  const box = now.getBoundingClientRect();
  assert.ok(box.bottom > 0 && box.top < SCROLL_VIEWPORT_PX, "the ring came to a message that is not on screen");
  await pressHere(page, "e");
  assert.deepStrictEqual(page.dismissCalls.map((call) => call.messages), [[now.getAttribute("data-id")]],
    "the second e did not mark the message the ring is round");
});

test("`#281 view-keys`: CTRL+1 TO CTRL+4 choose Main, All, the thread list and two columns from anywhere, a message box and the search included; h cycles the read mode; ; and : expand and collapse all", async () => {
  const page = await threadPage();
  page.wideDesk = true;
  const picker = page.el("thread-select");
  const layout = () => page.el("screen-main").getAttribute("data-columns");
  // From the list, from the message box with a draft in it, and from the search field with words typed:
  // each chord chooses its view, its default is kept from the browser, and nothing is typed.
  await press(page, page.el("scroll-area"), "/");
  await page.el("search-field").setValue("root");
  await page.el("channel-compose-text").setValue("a draft");
  for (const [target, fields] of [["scroll-area", { ctrlKey: true }], ["channel-compose-text", { ctrlKey: true }],
    ["search-field", { metaKey: true }]]) {
    for (const [key, value, columns] of [["1", "main", null], ["2", "flat", null], ["3", "threads", "list"],
      ["4", "columns", "two"], ["1", "main", null]]) {
      // The thread list covers the left column's box: the box is pressed from the list's column then.
      const at = target === "channel-compose-text" && layout() === "list" ? page.el("side-scroll") : page.el(target);
      const event = await press(page, at, key, fields);
      await page.settle();
      const what = `${JSON.stringify(fields)} ${key} in #${at.id}`;
      assert.equal(event.defaulted, false, `${what} was left to the browser's tab switch`);
      assert.equal(picker.value, value, `${what} did not choose ${value}`);
      assert.equal(layout(), columns, `${what} did not lay the page out for ${value}`);
    }
  }
  assert.equal(page.el("channel-compose-text").value, "a draft", "a view key typed into the message box");
  assert.equal(page.el("search-field").value, "root", "a view key typed into the search");
  // Esc in the search closes it, as ever, before it is Back anywhere.
  await press(page, page.el("search-field"), "Escape");
  assert.equal(page.el("search-field").hidden, true);
  // The bare digits are retired: in a field they are digits, and on the list they do nothing.
  for (const key of ["1", "2", "3"]) {
    const event = await press(page, page.el("scroll-area"), key);
    assert.equal(event.defaulted, true, `${key} on the list was taken`);
    assert.equal(picker.value, "main", `${key} on the list changed the view`);
  }
  // Inside the thread list: Enter on a card highlights it, as on the right in two columns (`#279
  // thread-card-focus`), and → opens its thread across the window; Esc is the list again, and the
  // chords leave it as they leave any view.
  await press(page, page.el("scroll-area"), "3", { ctrlKey: true });
  await page.settle();
  assert.equal(layout(), "list");
  assert.equal(focused(page), page.el("side-scroll"), `in the thread list the focus is on ${focusName(page)}`);
  await press(page, page.el("side-scroll"), "ArrowDown");
  const card = focused(page);
  assert.ok(card && card.hasAttribute("data-card"), `↓ in the thread list did not select a card: ${focusName(page)}`);
  await press(page, card, "Enter");
  await page.settle();
  assert.equal(layout(), "list", "Enter on a card left the thread list");
  // `#440 thread-card-pack-toggle`: Enter on a card of the thread list goes round its states, highlighting nothing.
  const again = cardRow(page, card.getAttribute("data-id"));
  assert.equal(again.getAttribute("data-card-state"), "expanded", "Enter on a card of the thread list did not open it whole");
  assert.equal(again.hasAttribute("data-highlighted"), false, "Enter on a card of the thread list highlighted it");
  assert.equal(focused(page), again, `after Enter on a card the focus is on ${focusName(page)}`);
  await press(page, again, "ArrowRight");
  await page.settle();
  assert.equal(layout(), null, "a card opened its thread inside the list");
  assert.equal(page.el("thread-heading").hidden, false, "→ on a card did not open its thread");
  assert.equal(picker.value, `thread:${card.getAttribute("data-id")}`);
  await press(page, focused(page) || page.el("scroll-area"), "Escape");
  await page.settle();
  assert.equal(layout(), "list", "Esc from a thread opened from the list did not go back to the list");
  const inside = await press(page, focused(page), "2", { metaKey: true });
  await page.settle();
  assert.equal(inside.defaulted, false);
  assert.equal(picker.value, "flat", "Cmd+2 inside the thread list did not choose All");
  assert.equal(layout(), null);
  const mode = () => page.el("todo-filter").getAttribute("data-read-mode");
  const modes = [mode()];
  for (let i = 0; i < 3; i += 1) {
    await press(page, page.el("scroll-area"), "h");
    modes.push(mode());
  }
  assert.deepStrictEqual(modes, ["show", "fold", "hide", "show"], "h did not cycle the read modes");
  const tall = await keyboardPage();
  const folded = () => tall.el("discord-log").children.filter((li) => li.getAttribute("data-collapsed") === "true").length;
  assert.ok(folded() > 0);
  // `#301 semicolon-expand-cycle`: the first ; in a list expands all, and : goes back round from there to
  // Collapse all. The rotation itself is tested under that issue below.
  await press(tall, tall.el("scroll-area"), ";");
  assert.equal(folded(), 0, "; did not expand every message");
  await press(tall, tall.el("scroll-area"), ":");
  assert.ok(folded() > 0, ": did not collapse them again");
  // On Dvorak ";" is typed on the key QWERTY calls Z (code KeyZ). It is ";", Expand all, and never z, Undo:
  // a key typing ASCII punctuation is read by its character, only a non-Latin letter by its place.
  const dvorak = await keyboardPage();
  const shut = () => dvorak.el("discord-log").children.filter((li) => li.getAttribute("data-collapsed") === "true").length;
  assert.ok(shut() > 0);
  await press(dvorak, dvorak.el("scroll-area"), ";", { code: "KeyZ" });
  assert.equal(shut(), 0, "; typed on Dvorak's KeyZ did not expand every message");
  assert.notEqual(dvorak.el("status").textContent, "Nothing to undo.", "; typed on Dvorak's KeyZ was read as z, Undo");
  await press(dvorak, dvorak.el("scroll-area"), ":", { code: "KeyZ", shiftKey: true });
  assert.ok(shut() > 0, ": typed on Dvorak's KeyZ did not collapse them again");
});

test("`#525 cmd-0-voice-view`: CTRL+0 (CMD+0) goes to the voice view from anywhere, and CTRL+1 comes back", async () => {
  const page = await threadPage();
  page.wideDesk = true;
  const voiceUp = () => page.el("pane-voice").hidden === false && page.el("pane-discord").hidden === true;
  assert.equal(voiceUp(), false, "the premise: the channel is up");
  assert.ok(page.el("keyboard-keys").text().includes("Voice: the call's view"), "the ? help does not list Ctrl+0");
  // From the list, from the message box with a draft, and from the search: taken, nothing typed.
  await page.el("channel-compose-text").setValue("a draft");
  for (const [target, fields] of [["scroll-area", { ctrlKey: true }], ["channel-compose-text", { metaKey: true }]]) {
    const event = await press(page, page.el(target), "0", fields);
    await page.settle();
    assert.equal(event.defaulted, false, `Ctrl+0 in #${target} was left to the browser's zoom reset`);
    assert.equal(voiceUp(), true, `Ctrl+0 in #${target} did not show the voice view`);
    // Already there: taken, and it stays (it is not the switch's toggle).
    const again = await press(page, page.el("scroll-area"), "0", { ctrlKey: true });
    await page.settle();
    assert.equal(again.defaulted, false, "Ctrl+0 on the voice view was left to the browser");
    assert.equal(voiceUp(), true, "Ctrl+0 on the voice view left it");
    // A held key's repeat is taken and chooses nothing.
    const held = await press(page, page.el("scroll-area"), "0", { ctrlKey: true, repeat: true });
    assert.equal(held.defaulted, false, "a held Ctrl+0's repeat was left to the browser");
    assert.equal(voiceUp(), true, "a held Ctrl+0's repeat left the voice view");
    // Ctrl+1 comes back to the channel, and the draft survived both ways.
    const back = await press(page, page.el("scroll-area"), "1", { ctrlKey: true });
    await page.settle();
    assert.equal(back.defaulted, false);
    assert.equal(voiceUp(), false, "Ctrl+1 from the voice view did not return to the channel");
    assert.equal(page.el("thread-select").value, "main", "Ctrl+1 from the voice view did not choose Main");
  }
  assert.equal(page.el("channel-compose-text").value, "a draft", "a view key typed into the message box");
  // From Settings over the main screen: Settings closes and the voice view is up.
  await page.el("open-settings").click();
  await page.settle();
  const fromSettings = await press(page, page.el("scroll-area"), "0", { ctrlKey: true });
  await page.settle();
  assert.equal(fromSettings.defaulted, false, "Ctrl+0 over Settings was left to the browser");
  assert.equal(voiceUp(), true, "Ctrl+0 over Settings did not show the voice view");
  // Shift+Ctrl+0 is the collapsed-lines bracket, not the voice view.
  await press(page, page.el("scroll-area"), "1", { ctrlKey: true });
  await page.settle();
  const bracket = await press(page, page.el("scroll-area"), ")", { code: "Digit0", ctrlKey: true, shiftKey: true });
  await page.settle();
  assert.equal(voiceUp(), false, "Ctrl+Shift+0 went to the voice view");
  assert.equal(bracket.defaulted, false, "the collapsed-lines chord was left alone");
  // ...and where the browser reports the digit itself while Shift is held, the same: the
  // collapsed-lines chord's place, never Ctrl+0 (the `#425` case the page already had).
  const digit = await press(page, page.el("scroll-area"), "0", { code: "Digit0", metaKey: true, shiftKey: true });
  await page.settle();
  assert.equal(voiceUp(), false, "Cmd+Shift+0 reporting a digit went to the voice view");
  assert.equal(digit.defaulted, false, "Cmd+Shift+0 reporting a digit was left alone");
});

// --- `#301 semicolon-expand-cycle` ------------------------------------------------------------------
//
// The owner, 2026-10-14: ; goes round (1) expand all, (2) expand every unread message and collapse
// every read one, (3) collapse all, and back to (1); : the other way; each press goes on from the step
// it last applied in that view. The Expand all and Collapse all chips stay.

/** Which of a list's long messages are open and which folded, by id, in the list's order. */
function foldState(page, list = "discord-log") {
  const open = [];
  const shut = [];
  for (const li of page.el(list).children) {
    const at = li.getAttribute("data-collapsed");
    if (at === "true") shut.push(li.getAttribute("data-id"));
    else if (at === "false") open.push(li.getAttribute("data-id"));
  }
  return { open, shut };
}

/** What the status line says while it shows, else null. */
const statusSaid = (page) => (page.el("status-line").hidden ? null : page.el("status").textContent);

/** A channel of `count` long messages, the odd ids read (Done on the server), in Show read. */
async function foldStepPage(count = 12, messages = tallChannel(count)) {
  const page = newPage();
  for (const one of messages) if (Number(one.id) % 2 === 1) page.dealtWith.add(String(one.id));
  await signIn(page);
  await showDiscord(page, messages);
  return page;
}

/** The fold states of `foldStepPage`'s list at each step, and what the status line says of each. */
function foldSteps(page) {
  const all = page.el("discord-log").children.map((li) => li.getAttribute("data-id"));
  const unread = all.filter((id) => Number(id) % 2 === 0);
  const read = all.filter((id) => Number(id) % 2 === 1);
  return {
    all, unread, read,
    expand: { open: all, shut: [] }, unread_open: { open: unread, shut: read }, collapse: { open: [], shut: all },
  };
}

const FOLD_SAID = { expand: "Long messages: all expanded", unread: "Long messages: unread expanded, read collapsed",
  collapse: "Long messages: all collapsed" };

test("`#301 semicolon-expand-cycle`: ; GOES ROUND expand all, the unread open and the read folded, and collapse all, and : the other way, each step said on the status line", async () => {
  const page = await foldStepPage();
  const area = page.el("scroll-area");
  const steps = foldSteps(page);
  // The fixture is what it says: half of it read by the page's own rule, and every message folded.
  assert.deepStrictEqual(steps.all.filter((id) => channelRow(page, id).getAttribute("data-archived") === "true"), steps.read);
  assert.deepStrictEqual(foldState(page), steps.collapse);
  const expected = { expand: steps.expand, unread: steps.unread_open, collapse: steps.collapse };
  const back = { shiftKey: true };
  for (const [key, fields, step] of [[";", {}, "expand"], [";", {}, "unread"], [";", {}, "collapse"], [";", {}, "expand"],
    [":", back, "collapse"], [":", back, "unread"], [":", back, "expand"], [":", back, "collapse"], [";", {}, "expand"]]) {
    const event = await press(page, area, key, fields);
    assert.equal(event.defaulted, false, `${key} was left to the browser`);
    assert.deepStrictEqual(foldState(page), expected[step], `${key} did not go to ${step}`);
    assert.equal(statusSaid(page), FOLD_SAID[step], `${key} to ${step} said ${statusSaid(page)}`);
  }
  // Said politely: the status line is the page's polite live region.
  assert.match(HTML_CODE, /<span id="status" role="status" aria-live="polite">/);
  // Read is the page's one rule, as it stands now: a message marked Done since is folded by (2) as well.
  await doneButton(channelRow(page, "2")).click();
  await page.settle();
  assert.equal(channelRow(page, "2").getAttribute("data-archived"), "true", "Done did not mark the message read");
  await press(page, area, ";");
  assert.deepStrictEqual(foldState(page).shut, ["1", "2", ...steps.read.slice(1)], "(2) opened a message read since");
  // Negative control: the rule is the read state, not the ids — with nothing read, (2) opens everything.
  const fresh = newPage();
  await signIn(fresh);
  await showDiscord(fresh, tallChannel(12));
  await press(fresh, fresh.el("scroll-area"), ";");
  await press(fresh, fresh.el("scroll-area"), ";");
  assert.equal(statusSaid(fresh), FOLD_SAID.unread);
  assert.deepStrictEqual(foldState(fresh).shut, [], "(2) folded a message nobody has read");
});

test("`#301 semicolon-expand-cycle`: THE FIRST PRESS in a list goes to the first step its way round, or past it when the list is so already; a fold by hand is no step, and the chips are steps", async () => {
  const back = { shiftKey: true };
  // ; in a list already all open: (2), not a press that changes nothing.
  let page = await foldStepPage();
  let steps = foldSteps(page);
  for (const li of page.el("discord-log").children) await foldButton(li).click();
  assert.deepStrictEqual(foldState(page), steps.expand, "the fixture did not open every message");
  await press(page, page.el("scroll-area"), ";");
  assert.deepStrictEqual(foldState(page), steps.unread_open, "the first ; in a list all open did nothing");
  assert.equal(statusSaid(page), FOLD_SAID.unread);
  // : in a list all folded: (2) as well.
  page = await foldStepPage();
  steps = foldSteps(page);
  await press(page, page.el("scroll-area"), ":", back);
  assert.deepStrictEqual(foldState(page), steps.unread_open, "the first : in a list all folded did nothing");
  // : in a list part open: Collapse all, as : always did.
  page = await foldStepPage();
  steps = foldSteps(page);
  await foldButton(channelRow(page, "4")).click();
  await press(page, page.el("scroll-area"), ":", back);
  assert.deepStrictEqual(foldState(page), steps.collapse, "the first : in a list part open did not collapse all");
  assert.equal(statusSaid(page), FOLD_SAID.collapse);
  // A message folded by hand is not a step: after Expand all the reader folds an unread message, and the
  // next ; goes on to (2), which opens it again. Started again from the list, it would have been (1).
  page = await foldStepPage();
  steps = foldSteps(page);
  await press(page, page.el("scroll-area"), ";");
  await foldButton(channelRow(page, "4")).click();
  assert.ok(foldState(page).shut.includes("4"));
  await press(page, page.el("scroll-area"), ";");
  assert.deepStrictEqual(foldState(page), steps.unread_open, "a fold by hand started the rotation again");
  // The chips are steps, and the keys go on from them: from (2), Expand all is (1) and ; goes to (2);
  // Collapse all is (3) and ; goes to (1). Negative control: had the chip not counted, each ; would have
  // gone on from (2), to (3).
  await page.el("expand-all").click();
  assert.deepStrictEqual(foldState(page), steps.expand);
  await press(page, page.el("scroll-area"), ";");
  assert.deepStrictEqual(foldState(page), steps.unread_open, "; did not go on from the Expand all chip");
  await page.el("collapse-all").click();
  assert.deepStrictEqual(foldState(page), steps.collapse);
  await press(page, page.el("scroll-area"), ";");
  assert.deepStrictEqual(foldState(page), steps.expand, "; did not go on from the Collapse all chip");
  // Neither chip says a key any more: ; and : press neither, but one time in three.
  assert.equal(page.el("expand-all").getAttribute("aria-keyshortcuts"), null, "Expand all still says ; is its key");
  assert.equal(page.el("collapse-all").getAttribute("aria-keyshortcuts"), null, "Collapse all still says : is its key");
  // A list with nothing long in it says so, takes the key, and changes nothing.
  const short = newPage();
  await signIn(short);
  await showDiscord(short, Array.from({ length: 4 }, (_unused, i) => message({ id: String(i + 1), content: SHORT_MESSAGE })));
  const event = await press(short, short.el("scroll-area"), ";");
  assert.equal(event.defaulted, false);
  assert.equal(statusSaid(short), "No long messages here to expand or collapse.");
  // On the call's view the keys are not the page's, as before.
  await short.el("view-switch").click();
  assert.equal(short.tab(), "voice");
  assert.equal((await press(short, short.el("scroll-area"), ";")).defaulted, true, "; acted on the call's view");
});

test("`#301 semicolon-expand-cycle`: EACH LIST KEEPS ITS OWN STEP — All, Main, a thread and the right column — and the keys fold the column they are in", async () => {
  const page = newPage();
  // Posts 0 to 3 on the channel, then the thread's first message and eight long replies; the posts with an
  // even number and the odd replies are read.
  threadHistory(page, { replies: 8, size: 20, top: 4, content: (i) => longMessage(`reply ${i}`),
    posted: (i) => longMessage(`post ${i}`) });
  const posts = page.posts.map((m) => String(m.id));
  const replies = page.messages.filter((m) => m.thread && !m.thread.is_root).map((m) => String(m.id));
  for (const id of [...posts.filter((_id, i) => i % 2 === 0), ...replies.filter((_id, i) => i % 2 === 0)]) page.dealtWith.add(id);
  await signIn(page);
  await showDiscord(page, page.messages);
  const picker = page.el("thread-select");
  const area = page.el("scroll-area");
  const back = { shiftKey: true };
  const choose = async (key) => {
    await press(page, area, key, { ctrlKey: true });
    await page.settle();
  };
  const unreadOf = (ids) => ids.filter((id) => !page.dealtWith.has(id));
  const readOf = (ids) => ids.filter((id) => page.dealtWith.has(id));
  assert.equal(picker.value, "flat", "the channel did not open in All");
  // All: ; and ; again, to (2).
  await press(page, area, ";");
  await press(page, area, ";");
  const longInAll = [...posts, ...replies];
  assert.deepStrictEqual(foldState(page), { open: unreadOf(longInAll), shut: readOf(longInAll) });
  // Main: its first press. It is part open — its read posts folded by All's (2) — so (1). Had the step been
  // the page's and not the list's, this would have been (3).
  await choose("1");
  assert.equal(picker.value, "main");
  await press(page, area, ";");
  assert.equal(statusSaid(page), FOLD_SAID.expand, "Main's first ; went on from All's step");
  assert.deepStrictEqual(foldState(page), { open: posts, shut: [] });
  // All again: on from its own (2), to (3). Had it gone on from Main's (1), this would have been (2).
  await choose("2");
  assert.equal(picker.value, "flat");
  // All, drawn again from what the page held, still offers its folds: before, its rows came back without
  // them in the page's list of folds, and neither the chips nor the keys found anything to fold.
  assert.equal(page.el("expand-all").hidden, false, "All, shown again, does not offer to expand its read messages");
  await press(page, area, ";");
  assert.equal(statusSaid(page), FOLD_SAID.collapse, "All did not go on from where it was left");
  assert.deepStrictEqual(foldState(page), { open: [], shut: longInAll });
  // A thread in one column: its own first press, (1); : then goes back round to (3).
  await pickThread(page, `thread:${page.threads[0].id}`);
  assert.equal(page.el("thread-heading").hidden, false, "the thread did not open");
  const inThread = page.el("discord-log").children.map((li) => li.getAttribute("data-id")).filter((id) => replies.includes(id));
  assert.ok(inThread.length >= 4, `the thread drew ${inThread.length} replies`);
  await press(page, area, ";");
  assert.equal(statusSaid(page), FOLD_SAID.expand, "the thread's first ; went on from another list's step");
  await press(page, area, ":", back);
  assert.equal(statusSaid(page), FOLD_SAID.collapse);
  // Main again: on from its (1), to (2).
  await choose("1");
  await press(page, area, ";");
  assert.equal(statusSaid(page), FOLD_SAID.unread, "Main did not go on from where it was left");
  assert.deepStrictEqual(foldState(page), { open: unreadOf(posts), shut: readOf(posts) });
  // Two columns, the thread on the right: the right column's list is its own, and the keys fold the
  // column they are pressed in, never the other.
  await twoColumns(page);
  await openerOf(cardRow(page, page.threads[0].id)).click();
  await page.settle();
  assert.equal(page.el("side-pane").hidden, false, "the thread did not open on the right");
  const mainBefore = foldState(page);
  const side = page.el("side-scroll");
  const onRight = () => page.el("side-log").children.map((li) => li.getAttribute("data-id")).filter((id) => replies.includes(id));
  assert.deepStrictEqual(foldState(page, "side-log"), { open: [], shut: onRight() }, "the right column's thread is not folded");
  await press(page, side, ";");
  assert.equal(statusSaid(page), FOLD_SAID.expand, "the right column's first ; went on from another list's step");
  assert.deepStrictEqual(foldState(page, "side-log"), { open: onRight(), shut: [] }, "; on the right did not open the right column");
  assert.deepStrictEqual(foldState(page), mainBefore, "; on the right folded Main");
  await press(page, side, ";");
  assert.deepStrictEqual(foldState(page, "side-log"), { open: unreadOf(onRight()), shut: readOf(onRight()) });
  assert.deepStrictEqual(foldState(page), mainBefore, "; on the right folded Main");
  // In Main, beside it: Main's own (2) goes on to (3), and the right column is left as it is.
  const sideBefore = foldState(page, "side-log");
  await press(page, area, ";");
  assert.equal(statusSaid(page), FOLD_SAID.collapse, "Main beside the right column did not go on from its own step");
  assert.deepStrictEqual(foldState(page), { open: [], shut: posts });
  assert.deepStrictEqual(foldState(page, "side-log"), sideBefore, "; in Main folded the right column");
  // And back on the right: on from its own (2), to (3).
  await press(page, side, ";");
  assert.deepStrictEqual(foldState(page, "side-log"), { open: [], shut: onRight() }, "the right column did not go on from its own step");
});

test("`#301 semicolon-expand-cycle` x `#280 scroll-stability`: THE READER'S ROW STAYS PUT through every step — the selected message, else the first showing — on the newest line too", async () => {
  const steps = [[";", {}], [";", {}], [";", {}], [";", {}], [":", { shiftKey: true }], [":", { shiftKey: true }], [":", { shiftKey: true }]];
  // The selected message, on a reader at the newest line.
  let page = await foldStepPage(30);
  let area = page.el("scroll-area");
  assert.ok(atBottomOf(area), "the channel did not open at its newest message");
  await press(page, area, "ArrowDown");
  await pressHere(page, "k");
  await pressHere(page, "k");
  const chosen = selectedId(page);
  assert.ok(/^\d+$/.test(chosen), `nothing was selected: ${chosen}`);
  const top = channelRow(page, chosen).getBoundingClientRect().top;
  for (const [key, fields] of steps) {
    // Pressed where the focus is, on the selected message, as a reader at the keys does.
    await pressHere(page, key, fields);
    const now = channelRow(page, chosen).getBoundingClientRect().top;
    assert.ok(Math.abs(now - top) <= 1, `${key} (${statusSaid(page)}) moved the selected message by ${now - top}px`);
    assert.equal(selectedId(page), chosen, `${key} moved the selection`);
  }
  // Negative control: the Expand all chip keeps the newest line for a reader on it, as every redraw does
  // (`preservingScroll`), and the same message moves: the check can see a move.
  page = await foldStepPage(30);
  area = page.el("scroll-area");
  await press(page, area, "ArrowDown");
  await pressHere(page, "k");
  await pressHere(page, "k");
  const lit = selectedId(page);
  const before = channelRow(page, lit).getBoundingClientRect().top;
  await page.el("expand-all").click();
  assert.ok(Math.abs(channelRow(page, lit).getBoundingClientRect().top - before) > 1,
    "the chip at the newest line left the message where it was, so this cannot tell a hold from a follow");
  // Nothing selected, the list scrolled up: the first message showing, its top at the list's top edge.
  page = await foldStepPage(30);
  area = page.el("scroll-area");
  const first = channelRow(page, "12");
  area.scrollTop += first.getBoundingClientRect().top;
  assert.equal(first.getBoundingClientRect().top, 0);
  for (const [key, fields] of steps) {
    await press(page, area, key, fields);
    assert.equal(first.getBoundingClientRect().top, 0, `${key} (${statusSaid(page)}) moved the first message showing`);
  }
});

test("`#301 semicolon-expand-cycle`: a message TALLER THAN THE SCREEN, read past its top and folded, comes back with its head showing, and the right column's reader stays put too", async () => {
  // Message 3 is a wall: open, it runs far past the screen. The reader has scrolled into the middle of it.
  const messages = tallChannel(8);
  messages[2] = { ...messages[2], content: `wall: ${"a sentence about the overnight run. ".repeat(200)}` };
  const page = await foldStepPage(8, messages);
  const area = page.el("scroll-area");
  await foldButton(channelRow(page, "3")).click();
  const wall = channelRow(page, "3");
  assert.ok(wall.getBoundingClientRect().height > 3 * SCROLL_VIEWPORT_PX, "the wall is not taller than the screen");
  area.scrollTop += wall.getBoundingClientRect().top + 1000;
  assert.ok(wall.getBoundingClientRect().top < -900 && wall.getBoundingClientRect().bottom > SCROLL_VIEWPORT_PX);
  // : in a list part open is Collapse all. Held by its top, the wall would be folded out of sight above
  // the screen; its head comes to the top of the list, under the floating line, instead.
  await press(page, area, ":", { shiftKey: true });
  assert.equal(statusSaid(page), FOLD_SAID.collapse);
  const head = page.pure("floatingClearance")(area);
  const box = wall.getBoundingClientRect();
  assert.equal(box.top, head, `the folded wall is at ${box.top}px, not with its head under the floating line at ${head}px`);
  assert.ok(box.bottom > head && box.bottom < SCROLL_VIEWPORT_PX);

  // The right column, its thread scrolled to its foot: the reader's row there stays put, and Main does not move.
  const side = newPage();
  threadHistory(side, { replies: 10, size: 20, content: (i) => longMessage(`reply ${i}`) });
  await signIn(side);
  await showDiscord(side, side.messages);
  await twoColumns(side);
  await openerOf(cardRow(side, side.threads[0].id)).click();
  await side.settle();
  const scroller = side.el("side-scroll");
  assert.ok(scroller.scrollHeight > scroller.clientHeight, "the thread on the right does not overflow");
  scroller.scrollTop = scroller.scrollHeight;
  await press(side, scroller, "ArrowDown");
  await pressHere(side, "k");
  const held = focused(side);
  assert.ok(held && held.parentNode === side.el("side-log"), `the keys did not select a row on the right: ${focusName(side)}`);
  const at = held.getBoundingClientRect().top;
  const mainAt = side.el("scroll-area").scrollTop;
  for (const [key, fields] of [[";", {}], [";", {}], [";", {}], [":", { shiftKey: true }]]) {
    await press(side, scroller, key, fields);
    const now = held.getBoundingClientRect().top;
    assert.ok(Math.abs(now - at) <= 1, `${key} on the right (${statusSaid(side)}) moved the reader's row by ${now - at}px`);
    assert.equal(side.el("scroll-area").scrollTop, mainAt, `${key} on the right scrolled Main`);
  }
});

test("`#301 semicolon-expand-cycle` x `#280 scroll-stability`: AT THE FOOT OF THE RIGHT COLUMN a step that opens only what is under the reader scrolls nothing, and the keeper of the foot does not take them down after it", async () => {
  // The thread's last three replies read and the rest unread, so at (2) everything above the first read
  // reply is open and it and the two under it are folded: : back to (1) opens only those, nothing above
  // the reader changes, and the step scrolls nothing — so no scroll tells the right column's keeper of a
  // reader at its foot (`#280`) that the reader is no longer there. Chromium showed it taking them to the
  // new foot a frame later, 679px (tests/fold_steps_browser.py). The step holds its row as a filter
  // change does, and the keeper leaves a held row alone.
  const run = async (script) => {
    const page = newPage(new Map(), script, (p) => p.enableResizeObserver());
    threadHistory(page, { replies: 10, size: 20, content: (i) => longMessage(`reply ${i}`) });
    const replies = page.messages.filter((m) => m.thread && !m.thread.is_root).map((m) => String(m.id));
    for (const id of replies.slice(-3)) page.dealtWith.add(id);
    await signIn(page);
    await showDiscord(page, page.messages);
    await twoColumns(page);
    await openerOf(cardRow(page, page.threads[0].id)).click();
    await page.settle();
    const side = page.el("side-scroll");
    const pane = page.el("side-pane");
    // The keeper's first pass, which notes what the column shows.
    page.resized(pane);
    await press(page, side, ";");
    await press(page, side, ";");
    assert.equal(statusSaid(page), FOLD_SAID.unread);
    // The reader at the foot, on the first read reply.
    await press(page, side, "End");
    await pressHere(page, "k");
    await pressHere(page, "k");
    side.scrollTop = side.scrollHeight;
    await side.dispatch("scroll");
    const row = focused(page);
    assert.equal(selectedId(page), replies[replies.length - 3], "the premise: the first read reply is selected");
    assert.ok(atBottomOf(side), "the premise: the right column is at its foot");
    const top = row.getBoundingClientRect().top;
    const scrolled = side.scrollTop;
    await pressHere(page, ":", { shiftKey: true });
    const stepScrolled = side.scrollTop !== scrolled;
    // What the browser does after the layout the step changed: tells whatever watches the column.
    page.resized(pane);
    await page.settle();
    return { said: statusSaid(page), stepScrolled, moved: row.getBoundingClientRect().top - top };
  };
  const real = await run(SCRIPT);
  assert.deepStrictEqual(real, { said: FOLD_SAID.expand, stepScrolled: false, moved: 0 });
  // THE CONTROL: the row placed but not held. The keeper takes the reader to the new foot, by the height of
  // the three replies opened under them.
  const unheld = await run(brokenScript("    placeFilterAnchor(anchor);\n",
    "    { const area = el(\"scroll-area\"); area.scrollTop += anchor.row.getBoundingClientRect().top - " +
    "area.getBoundingClientRect().top - anchor.offset; }\n"));
  assert.equal(unheld.stepScrolled, false);
  assert.ok(unheld.moved < -100, `the keeper left an unheld reader at ${unheld.moved}px, so this cannot see it`);
});

/**
 * The size of every collection `script` keeps at its top level — each `const` or `let` that starts as
 * `[]`, a Map or a Set — on `page`, by name.
 */
function keptSizes(page, script) {
  const sizes = {};
  for (const [, name] of script.matchAll(/^(?:const|let) ([A-Za-z_$][\w$]*) = (?:\[\]|new (?:Map|Set)\()/gm)) {
    const value = page.pure(name);
    sizes[name] = Array.isArray(value) ? value.length : value.size;
  }
  return sizes;
}

test("`#301 semicolon-expand-cycle` x `#226 page-energy-profile`: NOTHING THE PAGE KEEPS GROWS as a channel that does not change is polled and its views shown again — no list of folds outlives its rows", async () => {
  // Once Expand all read the folds off the rows, nothing pruned the list of folds the page kept beside
  // them, and every poll's rebuild added an entry per long message, each holding its thrown-away row:
  // 1,000 more a poll on a channel of 1,000, for as long as the page stayed open, and the row being read
  // aloud was looked for through every one of them on every draw.
  const run = async (script) => {
    const page = newPage(new Map(), script);
    threadHistory(page, { replies: 8, size: 20, top: 4, content: (i) => longMessage(`reply ${i}`),
      posted: (i) => longMessage(`post ${i}`) });
    await signIn(page);
    await showDiscord(page, page.messages);
    const area = page.el("scroll-area");
    // Main, All again from what the page held, and a poll: every way the rows are drawn again.
    const round = async () => {
      await press(page, area, "1", { ctrlKey: true });
      await page.settle();
      assert.equal(page.el("thread-select").value, "main");
      await press(page, area, "2", { ctrlKey: true });
      await page.settle();
      assert.equal(page.el("thread-select").value, "flat");
      page.expireTimers(DISCORD_POLL_MS);
      await page.settle();
    };
    // Once first, so that each view has been read and what the page keeps of it is in.
    await round();
    const long = () => page.el("discord-log").children.filter((li) => li.getAttribute("data-collapsed") !== null).length;
    const before = { long: long(), sizes: keptSizes(page, script) };
    for (let i = 0; i < 4; i += 1) await round();
    const after = keptSizes(page, script);
    assert.equal(long(), before.long, "the channel drew a different number of long messages");
    const grew = Object.keys(after).filter((name) => after[name] > before.sizes[name])
      .map((name) => `${name} ${before.sizes[name]} -> ${after[name]}`);
    return { long: before.long, grew };
  };
  const real = await run(SCRIPT);
  assert.ok(real.long >= 12, `the channel drew ${real.long} long messages`);
  assert.deepStrictEqual(real.grew, [], "something the page keeps grew with every poll of a channel that did not change");
  // Neither a fixture too small to see it nor a list this test does not read: THE CONTROL keeps a list of
  // folds beside the rows again, never pruned, and the test names it.
  const kept = brokenScript("const FOLD_LISTS = [", "const foldsKept = [];\nconst FOLD_LISTS = [")
    .replace("  meta.append(fold);\n  return fold;\n", "  meta.append(fold);\n  foldsKept.push(entry);\n  return fold;\n");
  assert.ok(kept.includes("foldsKept.push(entry)"), "the control no longer finds where a fold is made");
  const leaky = await run(kept);
  assert.equal(leaky.grew.length, 1, `the control grew ${leaky.grew.join(", ")}`);
  assert.match(leaky.grew[0], /^foldsKept \d+ -> \d+$/);
  const [from, to] = leaky.grew[0].split(" ").filter((word) => /^\d+$/.test(word)).map(Number);
  assert.ok(to - from >= 4 * leaky.long, `the control's list grew by ${to - from}, not a long message's row a round`);
});

test("`#301 semicolon-expand-cycle` x `#49 cached-summaries`: ALL, DRAWN AGAIN from what the page held, has its long messages summarised under Summaries", async () => {
  // A view held while another showed came back with its folds missing from the list of folds the page
  // kept, and Summaries asked about the rows of that list: the replies All shows were never summarised
  // until a poll drew them afresh. The rows are the record now, and Summaries reads them.
  const page = newPage();
  threadHistory(page, { replies: 8, size: 20, top: 4, content: (i) => longMessage(`reply ${i}`),
    posted: (i) => longMessage(`post ${i}`) });
  await signIn(page);
  await showDiscord(page, page.messages);
  const area = page.el("scroll-area");
  const replies = page.messages.filter((m) => m.thread && !m.thread.is_root).map((m) => String(m.id));
  assert.equal(page.el("thread-select").value, "flat", "the channel did not open in All");
  await press(page, area, "1", { ctrlKey: true });
  await page.settle();
  assert.equal(page.el("thread-select").value, "main");
  await turnSummariesOn(page);
  // Main draws none of the thread's replies, so none has been asked about.
  assert.deepStrictEqual(replies.filter((id) => asksFor(page, id) > 0), []);
  assert.ok(page.summaryAsks.length > 0, "Summaries asked about nothing in Main");
  const served = page.pagesServed.length;
  await press(page, area, "2", { ctrlKey: true });
  await page.settle();
  assert.equal(page.el("thread-select").value, "flat");
  // The premise: All is the rows the page held, not a page read for it again.
  assert.equal(page.pagesServed.length, served, "All was read again, so its rows are not the held ones");
  // Summaries asks as the list scrolls: the reader scrolls a little, as the browser's scroll event
  // after the held place is put back would.
  area.scrollTop -= 40;
  await area.dispatch("scroll");
  await page.settle();
  const asked = replies.filter((id) => asksFor(page, id) > 0);
  assert.ok(asked.length >= 2, `All, shown again, asked about ${asked.length} of the replies on screen`);
  for (const id of asked) {
    assert.equal(summaryText(channelRow(page, id)), `a short line about ${id}`, `reply ${id} does not show its summary`);
  }
});

test("`#301 semicolon-expand-cycle`: A FIRST PRESS (2) WOULD ALSO LEAVE AS IT IS goes on round to the third step — ; in a list all open with nothing read collapses all, : in one all folded with everything read expands all", async () => {
  const back = { shiftKey: true };
  const ids = (page) => page.el("discord-log").children.map((li) => li.getAttribute("data-id"));
  const openAll = async (page) => {
    for (const li of page.el("discord-log").children) await foldButton(li).click();
    assert.deepStrictEqual(foldState(page), { open: ids(page), shut: [] }, "the fixture did not open every message");
  };
  /** A channel of twelve long messages, `read` of them read. */
  const channelReading = async (read) => {
    const page = newPage();
    const messages = tallChannel(12);
    for (const one of messages) if (read(String(one.id))) page.dealtWith.add(String(one.id));
    await signIn(page);
    await showDiscord(page, messages);
    return page;
  };
  // ; in a list all open with nothing read: Expanded all, and Expanded unread, collapsed read, would both
  // leave it as it is, so Collapsed all. Before, it went to (2), and the first press did nothing.
  let page = await channelReading(() => false);
  await openAll(page);
  let event = await press(page, page.el("scroll-area"), ";");
  assert.equal(event.defaulted, false);
  assert.deepStrictEqual(foldState(page), { open: [], shut: ids(page) }, "the first ; in a list all open and unread did nothing");
  assert.equal(statusSaid(page), FOLD_SAID.collapse);
  // The rotation goes on from the step it took: the next ; is (1).
  await press(page, page.el("scroll-area"), ";");
  assert.equal(statusSaid(page), FOLD_SAID.expand, "; did not go on from the third step");
  assert.deepStrictEqual(foldState(page), { open: ids(page), shut: [] });
  // : in a list all folded with every message read: Collapsed all and (2) would both leave it, so Expanded all.
  page = await channelReading(() => true);
  assert.ok(page.el("discord-log").children.every((li) => li.getAttribute("data-archived") === "true"),
    "the fixture is not every message read");
  assert.deepStrictEqual(foldState(page), { open: [], shut: ids(page) }, "the fixture is not every message folded");
  event = await press(page, page.el("scroll-area"), ":", back);
  assert.equal(event.defaulted, false);
  assert.deepStrictEqual(foldState(page), { open: ids(page), shut: [] }, "the first : in a list all folded and read did nothing");
  assert.equal(statusSaid(page), FOLD_SAID.expand);
  await press(page, page.el("scroll-area"), ":", back);
  assert.equal(statusSaid(page), FOLD_SAID.collapse, ": did not go on from the step it took");
  // Negative controls: where (2) changes something, the first press stops there. One message read in a
  // list all open: the first ; folds that one alone…
  page = await channelReading((id) => id === "3");
  await openAll(page);
  await press(page, page.el("scroll-area"), ";");
  assert.equal(statusSaid(page), FOLD_SAID.unread, "the first ; went past (2) though (2) folded a read message");
  assert.deepStrictEqual(foldState(page).shut, ["3"]);
  // …and one unread in a list all folded and otherwise read: the first : opens that one alone.
  page = await channelReading((id) => id !== "6");
  await press(page, page.el("scroll-area"), ":", back);
  assert.equal(statusSaid(page), FOLD_SAID.unread, "the first : went past (2) though (2) opened an unread message");
  assert.deepStrictEqual(foldState(page).open, ["6"]);
  // The rule is the first press's alone: a list that has a step goes round from it, as before, even to a
  // step that changes nothing. With nothing read, : goes past Collapsed all (the list is all folded) to
  // (2), which opens everything; the next : is (1), which changes nothing, and is still (1).
  page = await channelReading(() => false);
  await press(page, page.el("scroll-area"), ":", back);
  assert.equal(statusSaid(page), FOLD_SAID.unread);
  assert.deepStrictEqual(foldState(page), { open: ids(page), shut: [] });
  await press(page, page.el("scroll-area"), ":", back);
  assert.equal(statusSaid(page), FOLD_SAID.expand, "a list with a step skipped a step");
});

test("`#301 semicolon-expand-cycle`: Ctrl+; AND Ctrl+: (Cmd on a Mac) STEP FROM ANYWHERE — the message box, the search — where the bare keys are typed, the focus and the draft left where they are", async () => {
  const back = { shiftKey: true };
  for (const [platform, mod, name] of [["Linux x86_64", "ctrlKey", "Ctrl"], ["MacIntel", "metaKey", "Cmd"]]) {
    const page = newPage(new Map(), SCRIPT, (p) => p.setPlatform(platform));
    const messages = tallChannel(12);
    for (const one of messages) if (Number(one.id) % 2 === 1) page.dealtWith.add(String(one.id));
    await signIn(page);
    await showDiscord(page, messages);
    const steps = foldSteps(page);
    const expected = { expand: steps.expand, unread: steps.unread_open, collapse: steps.collapse };
    const area = page.el("scroll-area");
    const box = page.el("channel-compose-text");
    await box.setValue("half a thought");
    // Negative control: the bare keys in the message box are the box's, typed, and step nothing.
    for (const [key, fields] of [[";", {}], [":", back]]) {
      const event = await press(page, box, key, fields);
      assert.equal(event.defaulted, true, `${platform}: ${key} in the message box was taken from it`);
      assert.deepStrictEqual(foldState(page), steps.collapse, `${platform}: ${key} typed in the message box stepped the list`);
    }
    // The chords, from the box: the steps both ways round, said, with the focus and the draft where they were.
    for (const [key, fields, step] of [[";", {}, "expand"], [";", {}, "unread"], [":", back, "expand"], [":", back, "collapse"]]) {
      const what = `${platform}: ${name}+${key} in the message box`;
      const event = await press(page, box, key, { ...fields, [mod]: true });
      assert.equal(event.defaulted, false, `${what} was left to the browser`);
      assert.deepStrictEqual(foldState(page), expected[step], `${what} did not go to ${step}`);
      assert.equal(statusSaid(page), FOLD_SAID[step], `${what} said ${statusSaid(page)}`);
      assert.equal(focused(page), box, `${what} left the focus on ${focusName(page)}`);
      assert.equal(box.value, "half a thought", `${what} touched the draft`);
    }
    // One rotation, the chord's and the bare key's: ; on the list goes on from the chord's (3).
    await press(page, area, ";");
    assert.equal(statusSaid(page), FOLD_SAID.expand, `${platform}: ; on the list did not go on from the chord's step`);
    // A held chord is one step; its repeats are taken from the browser (in Chrome on a Mac, Check Document
    // Now) and step nothing.
    await press(page, box, ";", { [mod]: true });
    let event = await press(page, box, ";", { [mod]: true, repeat: true });
    assert.equal(event.defaulted, false, `${platform}: a held ${name}+;'s repeat reached the browser`);
    assert.deepStrictEqual(foldState(page), steps.unread_open, `${platform}: a held ${name}+; stepped on its repeat`);
    // Negative control: a held bare key's repeat is still nothing, and left alone.
    event = await press(page, area, ";", { repeat: true });
    assert.equal(event.defaulted, true, `${platform}: a held ;'s repeat was taken`);
    assert.deepStrictEqual(foldState(page), steps.unread_open, `${platform}: a held ; stepped on its repeat`);
    // From the search, while it is open and holds the focus: the chord steps, the bare key is typed.
    await press(page, area, "/");
    const field = page.el("search-field");
    assert.equal(focused(page), field, `${platform}: / did not put the focus in the search`);
    event = await press(page, field, ";");
    assert.equal(event.defaulted, true, `${platform}: ; in the search was taken from it`);
    assert.deepStrictEqual(foldState(page), steps.unread_open, `${platform}: ; typed in the search stepped the list`);
    event = await press(page, field, ";", { [mod]: true });
    assert.equal(event.defaulted, false, `${platform}: ${name}+; in the search was left to the browser`);
    assert.deepStrictEqual(foldState(page), steps.collapse, `${platform}: ${name}+; in the search did not step the list`);
    assert.equal(focused(page), field, `${platform}: ${name}+; in the search took the focus out of it`);
    // The Help list names the chords in the reader's words, and says where they work.
    const kbds = page.el("keyboard-keys").descendants().filter((node) => node.tagName === "kbd");
    const entry = (words) => kbds.find((node) => node.textContent === words);
    assert.ok(entry(`; or ${name}+;`), `${platform}: the list of keys does not name ; or ${name}+;: ${kbds.map((n) => n.textContent).join(" | ")}`);
    assert.ok(entry(`: or ${name}+:`), `${platform}: the list of keys does not name : or ${name}+:`);
    assert.match(entry(`; or ${name}+;`).parentNode.text(), new RegExp(`${name}\\+; works from anywhere, a message box or the search included`));
    assert.match(entry(`: or ${name}+:`).parentNode.text(), new RegExp(`${name}\\+: \\(${name}\\+Shift\\+;\\) from anywhere`));
    assert.match(entry(`; or ${name}+;`).parentNode.text(), /goes on round to the first step that changes something/);
    const other = name === "Cmd" ? "Ctrl" : "Cmd";
    assert.ok(!page.el("keyboard-keys").text().includes(`${other}+;`), `${platform}: the list of keys names ${other}+;`);
    // Neither chip says a key, the chords included: ; and : press neither, but one time in three.
    for (const chip of ["expand-all", "collapse-all"]) {
      assert.equal(page.el(chip).getAttribute("aria-keyshortcuts"), null, `${platform}: #${chip} announces a key it is not`);
    }
  }
});

test("`#301 semicolon-expand-cycle`: THE CHORD WORKS WITH THE SINGLE KEYS OFF and on no screen without long messages to step — Settings, the call's view — where the browser keeps it", async () => {
  const page = await foldStepPage();
  const steps = foldSteps(page);
  const area = page.el("scroll-area");
  await page.el("open-settings").click();
  await page.el("single-keys").setChecked(false);
  // Settings has none, and a box of its own: the chord is the browser's there, and steps nothing.
  const prompt = page.el("prompt-summary");
  let event = await press(page, prompt, ";", { ctrlKey: true });
  assert.equal(event.defaulted, true, "Ctrl+; on Settings was taken from the browser");
  assert.equal(page.screen(), "settings", "Ctrl+; left Settings");
  await press(page, page.el("screen-settings"), "Escape");
  assert.equal(page.screen(), "main");
  // The single keys off: ; is nothing, and the chord still steps.
  event = await press(page, area, ";");
  assert.equal(event.defaulted, true, "; with the single keys off was taken");
  assert.deepStrictEqual(foldState(page), steps.collapse, "; stepped the list with the single keys off");
  event = await press(page, area, ";", { ctrlKey: true });
  assert.equal(event.defaulted, false, "Ctrl+; with the single keys off was left to the browser");
  assert.deepStrictEqual(foldState(page), steps.expand, "Ctrl+; did not step the list with the single keys off");
  event = await press(page, page.el("channel-compose-text"), ":", { metaKey: true, shiftKey: true });
  assert.deepStrictEqual(foldState(page), steps.collapse, "Cmd+: did not step the list back with the single keys off");
  // The call's view: none of the channel's lists is on screen, and the chord is left to the browser.
  await page.el("view-switch").click();
  assert.equal(page.tab(), "voice");
  event = await press(page, page.el("compose-text"), ";", { ctrlKey: true });
  assert.equal(event.defaulted, true, "Ctrl+; acted on the call's view");
  assert.equal(page.tab(), "voice");
});

test("`#301 semicolon-expand-cycle`: IN TWO COLUMNS THE CHORD IN A MESSAGE BOX STEPS THAT BOX'S COLUMN, and in the search the column the keys moved last", async () => {
  const page = newPage();
  threadHistory(page, { replies: 8, size: 20, top: 4, content: (i) => longMessage(`reply ${i}`),
    posted: (i) => longMessage(`post ${i}`) });
  await signIn(page);
  await showDiscord(page, page.messages);
  const replies = page.messages.filter((m) => m.thread && !m.thread.is_root).map((m) => String(m.id));
  await twoColumns(page);
  await openerOf(cardRow(page, page.threads[0].id)).click();
  await page.settle();
  assert.equal(page.el("side-pane").hidden, false, "the thread did not open on the right");
  const area = page.el("scroll-area");
  const side = page.el("side-scroll");
  const leftIds = () => page.el("discord-log").children.filter((li) => li.hasAttribute("data-collapsed")).map((li) => li.getAttribute("data-id"));
  const rightIds = () => page.el("side-log").children.map((li) => li.getAttribute("data-id")).filter((id) => replies.includes(id));
  const left = (open) => (open ? { open: leftIds(), shut: [] } : { open: [], shut: leftIds() });
  const right = (open) => (open ? { open: rightIds(), shut: [] } : { open: [], shut: rightIds() });
  assert.ok(leftIds().length >= 3 && rightIds().length >= 4, `too few long messages: ${leftIds()} | ${rightIds()}`);
  assert.deepStrictEqual(foldState(page), left(false), "Main is not folded");
  assert.deepStrictEqual(foldState(page, "side-log"), right(false), "the right column's thread is not folded");
  // The keys last in the right column (]); the chord in the channel's box steps Main, the box's column,
  // and leaves the right column alone.
  await press(page, area, "]");
  const channelBox = page.el("channel-compose-text");
  let event = await press(page, channelBox, ";", { ctrlKey: true });
  assert.equal(event.defaulted, false, "Ctrl+; in the channel's box was left to the browser");
  assert.deepStrictEqual(foldState(page), left(true), "Ctrl+; in the channel's box did not step Main");
  assert.deepStrictEqual(foldState(page, "side-log"), right(false), "Ctrl+; in the channel's box stepped the right column");
  assert.equal(focused(page), channelBox);
  // The chord in the right column's box steps its thread, and leaves Main alone.
  const sideBox = page.el("side-compose-text");
  await sideBox.setValue("and another");
  event = await press(page, sideBox, ";");
  assert.equal(event.defaulted, true, "; in the right column's box was taken from it");
  assert.deepStrictEqual(foldState(page, "side-log"), right(false), "; typed in the right column's box stepped its thread");
  event = await press(page, sideBox, ";", { metaKey: true });
  assert.equal(event.defaulted, false, "Cmd+; in the right column's box was left to the browser");
  assert.deepStrictEqual(foldState(page, "side-log"), right(true), "Cmd+; in the right column's box did not step its thread");
  assert.deepStrictEqual(foldState(page), left(true), "Cmd+; in the right column's box stepped Main");
  assert.equal(focused(page), sideBox);
  assert.equal(sideBox.value, "and another");
  // The search belongs to neither column: from it the chord steps the column the keys moved last. The
  // right column's first, by its keys; then Main's, by its keys.
  await press(page, side, "/");
  const field = page.el("search-field");
  assert.equal(focused(page), field, "/ did not put the focus in the search");
  await press(page, field, ":", { ctrlKey: true, shiftKey: true });
  assert.deepStrictEqual(foldState(page, "side-log"), right(false), "Ctrl+: in the search did not step the right column, where the keys were");
  assert.deepStrictEqual(foldState(page), left(true), "Ctrl+: in the search stepped Main, where the keys were not");
  await press(page, area, "k");
  await press(page, field, ":", { ctrlKey: true, shiftKey: true });
  assert.deepStrictEqual(foldState(page), left(false), "Ctrl+: in the search did not step Main, where the keys were");
  assert.deepStrictEqual(foldState(page, "side-log"), right(false), "Ctrl+: in the search stepped the right column, where the keys were not");
});

test("`#301 semicolon-expand-cycle` x `#280 scroll-stability`: A CHORD IN THE CHANNEL'S BOX AT THE LIST'S FOOT KEEPS THE BOX WHERE IT IS, its focus and draft with it, while a chord from the list keeps the reader's row", async () => {
  // From review: Ctrl+; in the channel's box held the message the reader was on, near the foot, and
  // everything opening between it and the box took the box being typed in off the screen. Pressed in a
  // box showing in the list, the step keeps the box where it is, the messages opening and folding above it.
  const run = async (script) => {
    const page = newPage(new Map(), script);
    const messages = tallChannel(30);
    for (const one of messages) if (Number(one.id) % 2 === 1) page.dealtWith.add(String(one.id));
    await signIn(page);
    await showDiscord(page, messages);
    const area = page.el("scroll-area");
    const box = page.el("channel-compose-text");
    // As tall as a browser draws its three rows: the model gives an empty field no height of its own.
    box.style.height = "80px";
    scrollToBottom(area);
    // The newest message selected by the keys, as a reader at the keys does, the list at its foot with
    // the box under that message, and then the focus put in the box with a draft in it.
    await press(page, area, "End");
    const row = focused(page);
    assert.ok(row && row.parentNode === page.el("discord-log"), `the keys did not select a message: ${focusName(page)}`);
    assert.equal(row.getAttribute("data-id"), "30", "End did not select the newest message");
    scrollToBottom(area);
    await area.dispatch("scroll");
    await box.setValue("half a thought");
    const at = box.getBoundingClientRect();
    assert.ok(atBottomOf(area), "the premise: the list is at its foot");
    assert.ok(at.top >= 0 && at.bottom <= SCROLL_VIEWPORT_PX, `the premise: the box shows whole (${at.top}px to ${at.bottom}px)`);
    const rowAt = row.getBoundingClientRect().top;
    assert.ok(rowAt >= 0 && rowAt < at.top, `the premise: the selected message shows above the box (${rowAt}px)`);
    const steps = [];
    // Ctrl+: twice (all folded, so the first goes on round to the unread open, then to every message
    // open: the list grows above the box), then Ctrl+; (from all open, the read folded: it shrinks).
    for (const [key, fields] of [[":", { ctrlKey: true, shiftKey: true }], [":", { ctrlKey: true, shiftKey: true }], [";", { ctrlKey: true }]]) {
      await press(page, box, key, fields);
      const now = box.getBoundingClientRect();
      steps.push({ said: statusSaid(page), moved: now.top - at.top, shows: now.top >= 0 && now.bottom <= SCROLL_VIEWPORT_PX,
        focus: focused(page) === box, draft: box.value, foot: atBottomOf(area) });
    }
    return { page, area, box, row, steps };
  };
  const real = await run(SCRIPT);
  const kept = { moved: 0, shows: true, focus: true, draft: "half a thought", foot: true };
  assert.deepStrictEqual(real.steps, [
    { said: FOLD_SAID.unread, ...kept }, { said: FOLD_SAID.expand, ...kept }, { said: FOLD_SAID.unread, ...kept },
  ], "a chord in the box at the list's foot did not keep the box where it was");
  // A chord from the list, the focus back on the messages and a few up from the newest, far enough from
  // the list's end for folding under it to leave room: the reader's row is the one kept, as for the bare
  // keys, and the box goes where the folds take it.
  const { page, area } = real;
  real.row.focus();
  for (let i = 0; i < 3; i += 1) await pressHere(page, "k");
  const row = focused(page);
  assert.ok(row.parentNode === page.el("discord-log") && Number(row.getAttribute("data-id")) <= 28, `k did not go up the list: ${focusName(page)}`);
  area.scrollTop += row.getBoundingClientRect().top - 100;
  await area.dispatch("scroll");
  const rowAt = row.getBoundingClientRect().top;
  assert.equal(rowAt, 100, "the premise: the selected message shows");
  await pressHere(page, ";", { ctrlKey: true });
  assert.equal(statusSaid(page), FOLD_SAID.collapse);
  assert.ok(Math.abs(row.getBoundingClientRect().top - rowAt) <= 1, `Ctrl+; on the list moved the selected message by ${row.getBoundingClientRect().top - rowAt}px`);
  // A box scrolled out of sight, the focus still in it: the reader is reading the list, and the first
  // message showing is the one kept.
  area.scrollTop = 0;
  const first = page.el("discord-log").children.find((li) => li.getBoundingClientRect().bottom > 0);
  const firstAt = first.getBoundingClientRect().top;
  await press(page, real.box, ";", { ctrlKey: true });
  assert.equal(statusSaid(page), FOLD_SAID.expand);
  assert.equal(focused(page), real.box);
  assert.ok(Math.abs(first.getBoundingClientRect().top - firstAt) <= 1, "Ctrl+; in a box out of sight did not keep the first message showing");
  // THE CONTROL: the reader's row kept for a chord in the box too. The first step opens the selected
  // message, unread, under its top, and the box goes below the screen.
  const unkept = await run(brokenScript("applyFoldStep(step, folds, foldStepField(target));", "applyFoldStep(step, folds, null);"));
  assert.equal(unkept.steps[0].said, FOLD_SAID.unread);
  assert.equal(unkept.steps[0].shows, false, `with the reader's row kept the box still showed, ${unkept.steps[0].moved}px down, so this cannot see it`);
});

test("`#281 view-keys`: ESC ON THE THREAD LIST goes back to the view it was chosen from — Main, All or a thread in one column, or two columns — and never to a thread opened from it", async () => {
  // The owner, of the Threads screen the list replaced: "rather ESC works". A view is left the way it was come to.
  const page = await threadPage();
  page.wideDesk = true;
  const picker = page.el("thread-select");
  const layout = () => page.el("screen-main").getAttribute("data-columns");
  const choose = async (key, from = focused(page) || page.el("scroll-area")) => {
    const event = await press(page, from, key, { ctrlKey: true });
    await page.settle();
    assert.equal(event.defaulted, false, `Ctrl+${key} was left to the browser`);
  };
  const escape = async (from = focused(page) || page.el("side-scroll")) => {
    const event = await press(page, from, "Escape");
    await page.settle();
    return event;
  };
  const toCard = async () => {
    await press(page, page.el("side-scroll"), "ArrowDown");
    const card = focused(page);
    assert.ok(card && card.hasAttribute("data-card"), `↓ in the thread list did not select a card: ${focusName(page)}`);
    return card;
  };
  const onTheList = (what) => {
    assert.equal(layout(), "list", `${what}: not on the thread list`);
    assert.equal(picker.value, "threads", `${what}: the picker does not say the thread list`);
  };
  const keysOnTheLeft = (what) => assert.ok(focused(page) && nodeWithinFixture(page.el("scroll-area"), focused(page)),
    `${what}: the focus is on ${focusName(page)}, not on the list on screen`);

  // From All: Esc on a card, and on the list's own scroller, is All again in one column, the keys on its list.
  await choose("2", page.el("scroll-area"));
  assert.equal(picker.value, "flat");
  for (const where of ["a card", "the scroller"]) {
    await choose("3");
    onTheList(`Ctrl+3 from All, for ${where}`);
    const from = where === "a card" ? await toCard() : page.el("side-scroll");
    const event = await escape(from);
    assert.equal(event.defaulted, false, `Esc on ${where} of the thread list was left to the browser`);
    assert.equal(layout(), null, `Esc on ${where} of the thread list did not leave it`);
    assert.equal(picker.value, "flat", `Esc on ${where} of the thread list did not go back to All`);
    keysOnTheLeft(`Esc on ${where} of the thread list`);
  }

  // ← on a card is nobody's: there is no thread on screen to leave, and the cards stay as they were. It
  // was taken, and opened Main in the list's own column, out of sight.
  await choose("3");
  const card = await toCard();
  const cards = cardIds(page);
  const reads = page.timelineCalls.length;
  const left = await press(page, card, "ArrowLeft");
  await page.settle();
  assert.equal(left.defaulted, true, "← on a card of the thread list was taken");
  onTheList("← on a card");
  assert.equal(focused(page), card, `← on a card moved the focus to ${focusName(page)}`);
  assert.deepStrictEqual(cardIds(page), cards, "← on a card changed the cards");
  assert.equal(page.el("side-list").hidden, false, "← on a card took the cards away");
  assert.equal(page.timelineCalls.length, reads, "← on a card read something");

  // The search over the list is smaller than it: the first Esc closes it, the second leaves the list.
  await press(page, page.el("side-scroll"), "/");
  assert.equal(page.el("search-field").hidden, false, "/ on the thread list did not open the search");
  await escape(page.el("search-field"));
  assert.equal(page.el("search-field").hidden, true, "Esc did not close the search first");
  onTheList("Esc in the search over the list");
  await escape();
  assert.equal(picker.value, "flat", "Esc after the search did not go back to All");

  // From Main, through a thread opened from the list: its Esc is the list, and the list's is Main —
  // not the thread again, which would go round and round.
  // `#279 thread-card-focus`, `#440 thread-card-pack-toggle`: Enter on a card of the thread list opens it
  // whole, highlighting nothing, and → goes into its thread. Back on the list, Esc leaves it at once.
  await choose("1", page.el("scroll-area"));
  await choose("3");
  const opened = await toCard();
  const openedId = opened.getAttribute("data-id");
  await press(page, opened, "Enter");
  await page.settle();
  onTheList("Enter on a card");
  assert.deepStrictEqual(litCards(page), [], "Enter on a card of the thread list highlighted it");
  await press(page, cardRow(page, openedId), "ArrowRight");
  await page.settle();
  assert.equal(picker.value, `thread:${openedId}`, "→ on a card did not open its thread");
  await escape(focused(page) || page.el("scroll-area"));
  onTheList("Esc from a thread opened from the list");
  await escape();
  assert.equal(layout(), null);
  assert.equal(picker.value, "main", "Esc on the list went back to the thread opened from it, not to Main");
  // And so with the list chosen again by Ctrl+3 from such a thread, rather than by its Esc.
  await choose("3");
  const reopened = await toCard();
  const reopenedId = reopened.getAttribute("data-id");
  await press(page, reopened, "ArrowRight");
  await page.settle();
  assert.equal(picker.value, `thread:${reopenedId}`);
  await choose("3", focused(page) || page.el("scroll-area"));
  onTheList("Ctrl+3 from a thread opened from the list");
  await escape();
  assert.equal(picker.value, "main", "Ctrl+3 from a thread opened from the list made that thread the list's way back");

  // A thread opened in one column from Main: Esc on the list is that thread again, whose own Esc is Main.
  const thread = page.threads[0];
  await pickThread(page, `thread:${thread.id}`);
  assert.equal(picker.value, `thread:${thread.id}`);
  await choose("3");
  onTheList("Ctrl+3 from a thread");
  await escape();
  assert.equal(layout(), null);
  assert.equal(picker.value, `thread:${thread.id}`, "Esc on the list did not go back to the thread it was chosen from");
  await escape(page.el("scroll-area"));
  assert.equal(picker.value, "main", "the thread Esc went back to forgot it was opened from Main");

  // From two columns: Esc is two columns again, and their button goes back to one column, as it did.
  await choose("4", page.el("scroll-area"));
  assert.equal(layout(), "two");
  assert.equal(page.el("column-toggle").title, "Back to one column");
  await choose("3");
  onTheList("Ctrl+3 from two columns");
  assert.equal(page.el("column-toggle").title, "Show Main and a thread side by side");
  await escape(await toCard());
  assert.equal(layout(), "two", "Esc on the list chosen from two columns did not go back to them");
  assert.equal(picker.value, "columns");
  assert.equal(page.el("column-toggle").title, "Back to one column", "two columns came back with another way out");

  // Back to the list by two columns' button is not a choice of it: Esc still goes to what it was chosen from.
  await choose("1", page.el("scroll-area"));
  await choose("3");
  await choose("4");
  assert.equal(page.el("column-toggle").title, "Back to the thread list");
  await press(page, page.el("scroll-area"), "\\", { ctrlKey: true });
  await page.settle();
  onTheList("Ctrl+\\ out of two columns");
  await escape();
  assert.equal(layout(), null, "Esc went back to two columns, which the list was not chosen from");
  assert.equal(picker.value, "main");

  // The list of keys says so, under Esc and under Ctrl+3, and so does Help's paragraph on the four views.
  const keys = page.el("keyboard-keys").text();
  assert.ok(keys.includes("and leaves the thread list for the view it was chosen from"), "Esc's entry does not say it leaves the list");
  assert.ok(keys.includes("Esc on the list takes the highlight off, then goes back to the view it was chosen from"),
    "Ctrl+3's entry does not say how to leave the list");
  // `#440 thread-card-pack-toggle`: the thread list highlights no card, so its Esc goes back at once.
  assert.match(HTML_CODE, /<kbd>Esc<\/kbd> on the list itself goes back to\s+the view you chose it from/);
});

test("`#281 view-keys` x `#265 window-channel-memory`: a reloaded window keeps where its thread list and two columns were chosen from", async () => {
  /** A window over the device's `store`, on a wide desk or not: a reload when handed its own `session`. */
  const deskWindow = (store, session = null, wide = true) => newPage(store, SCRIPT, (p) => {
    if (session) p.session = session;
    p.wideDesk = wide;
    p.messages = threadData().messages;
    p.threadingSupported = true;
    p.threads = threadData().threads;
  });
  const view = (page) => page.el("thread-select").value;
  const layout = (page) => page.el("screen-main").getAttribute("data-columns");
  // On a desk: All, then the list. A reload is still the list, and its Esc still All.
  const desk = deskWindow(new Map());
  await signIn(desk);
  await showDiscord(desk, threadData().messages);
  await press(desk, desk.el("scroll-area"), "2", { ctrlKey: true });
  await press(desk, desk.el("scroll-area"), "3", { ctrlKey: true });
  await desk.settle();
  assert.equal(view(desk), "threads");
  await desk.setVisibility("hidden");
  const deskAgain = deskWindow(desk.storage, desk.session);
  await reopenedChannel(deskAgain);
  await deskAgain.settle();
  assert.equal(layout(deskAgain), "list");
  await press(deskAgain, deskAgain.el("side-scroll"), "Escape");
  await deskAgain.settle();
  assert.equal(layout(deskAgain), null, "Esc on a reloaded thread list did not leave it");
  assert.equal(view(deskAgain), "flat", "Esc on a reloaded thread list forgot it was chosen from All");

  // In a window too narrow for two columns: the list, then Ctrl+4, which keeps the list on screen. A
  // reload is the list still — the layout two columns were chosen from — and not one column.
  const narrow = deskWindow(new Map(), null, false);
  await signIn(narrow);
  await showDiscord(narrow, threadData().messages);
  await press(narrow, narrow.el("scroll-area"), "1", { ctrlKey: true });
  await press(narrow, narrow.el("scroll-area"), "3", { ctrlKey: true });
  await press(narrow, narrow.el("side-scroll"), "4", { ctrlKey: true });
  await narrow.settle();
  assert.equal(layout(narrow), "list", "Ctrl+4 in a narrow window did not keep the thread list on screen");
  await narrow.setVisibility("hidden");
  const narrowAgain = deskWindow(narrow.storage, narrow.session, false);
  await reopenedChannel(narrowAgain);
  await narrowAgain.settle();
  assert.equal(layout(narrowAgain), "list", "a reload of two columns chosen from the thread list, in a narrow window, lost the list");
  // Its Esc is Main, which the list was chosen from: two columns cannot be drawn here.
  await press(narrowAgain, narrowAgain.el("side-scroll"), "Escape");
  await narrowAgain.settle();
  assert.equal(layout(narrowAgain), null);
  assert.equal(view(narrowAgain), "main", "Esc on the reloaded list did not go back to Main");
});

test("Shift+P AND Shift+L turn the Pinned and Links filters on and off, with the search bar that holds them", async () => {
  const page = await keyboardPage();
  await press(page, page.el("scroll-area"), "P", { shiftKey: true });
  assert.equal(page.el("pinned-filter").getAttribute("aria-pressed"), "true", "Shift+P did not turn Pinned on");
  assert.equal(page.el("search-field").hidden, false, "a filter came on with the bar that holds it shut");
  assert.equal(focused(page), page.el("scroll-area"), `after Shift+P the focus is on ${focusName(page)}`);
  await press(page, page.el("scroll-area"), "P", { shiftKey: true });
  assert.equal(page.el("pinned-filter").getAttribute("aria-pressed"), "false", "Shift+P did not turn Pinned off");
  await press(page, page.el("scroll-area"), "L", { shiftKey: true });
  assert.equal(page.el("links-filter").getAttribute("aria-pressed"), "true", "Shift+L did not turn Links on");
  // A plain p is the selected message's pin, not the filter.
  await press(page, page.el("scroll-area"), "L", { shiftKey: true });
  assert.equal(page.el("links-filter").getAttribute("aria-pressed"), "false");
});

test("b GOES BACK TO THE BOOKMARK, the line at the top and the message under it selected; Shift+B sets and removes it", async () => {
  // `#240 reading-bookmark`. "One to set and one to jump": Shift+B puts the line above the selected
  // message, b goes back to it. m, the old place marker's key, went there too until `#538 c-key-copy`
  // made it the message box's.
  const page = await keyboardPageAtTop();
  await press(page, page.el("scroll-area"), "ArrowDown");
  for (let i = 0; i < 2; i += 1) await pressHere(page, "j");
  assert.equal(selectedId(page), "3");
  let event = await pressHere(page, "B", { shiftKey: true });
  assert.equal(event.defaulted, false, "Shift+B was left to the browser");
  assert.deepStrictEqual(bookmarkAt(page), { id: "3", place: "above" }, "Shift+B did not draw the line above the selected message");
  assert.equal(statusText(page), "Bookmarked. b comes back here.");
  const area = page.el("scroll-area");
  for (const key of ["b"]) {
    area.scrollTop = area.scrollHeight;
    await area.dispatch("scroll");
    area.focus();
    await page.settle();
    assert.equal(page.el("jump-bookmark").hidden, false, "the chip is not offered with the line far above");
    assert.equal(page.el("jump-bookmark").getAttribute("data-direction"), "up", "the chip points away from the line");
    event = await press(page, area, key);
    assert.equal(event.defaulted, false, `${key} was left to the browser with a bookmark set`);
    assert.equal(selectedId(page), "3", `${key} selected ${selectedId(page)}, not the message under the line`);
    const top = bookmarkAt(page, "discord-log", true).getBoundingClientRect().top;
    assert.ok(top >= 0 && top < 40, `${key} did not bring the line to the top of the list: it is at ${top}px`);
    assert.equal(page.el("jump-bookmark").hidden, true, "the chip stayed with the line on screen");
  }
  // Pressed on the message right under the line, Shift+B takes it away, whatever another device did.
  await pressHere(page, "B", { shiftKey: true });
  assert.equal(bookmarkAt(page), null, "Shift+B on the message under the line did not take it away");
  const last = page.bookmarkCalls[page.bookmarkCalls.length - 1];
  assert.deepStrictEqual([last.method, last.ifRevision], ["DELETE", undefined]);
  event = await press(page, area, "b");
  assert.equal(statusText(page), "No bookmark here. Shift+B sets one.", "b with no bookmark did not say how to set one");
  assert.deepStrictEqual(page.dismissCalls, [], "the bookmark keys marked something Done");
});

test("Alt+↓ AND Alt+↑ step through the channels in the picker's order; Ctrl+K opens the picker", async () => {
  const page = newPage();
  const second = { id: "1110000000000000002", label: "second", writable: true };
  const third = { id: "1110000000000000003", label: "third", writable: true };
  page.channels = [{ ...CHANNEL }, second, third];
  await signIn(page);
  await showDiscord(page, tallChannel(5));
  const picker = page.el("discord-channel");
  assert.equal(String(picker.value), CHANNEL.id);
  let event = await press(page, page.el("scroll-area"), "ArrowDown", { altKey: true });
  await page.settle();
  assert.equal(event.defaulted, false);
  assert.equal(String(picker.value), second.id, "Alt+↓ did not move to the next channel");
  await press(page, focused(page) || page.el("scroll-area"), "ArrowDown", { altKey: true });
  await page.settle();
  assert.equal(String(picker.value), third.id);
  // At the end it stops, rather than wrapping round.
  await press(page, page.el("scroll-area"), "ArrowDown", { altKey: true });
  await page.settle();
  assert.equal(String(picker.value), third.id, "Alt+↓ at the last channel wrapped round");
  await press(page, page.el("scroll-area"), "ArrowUp", { altKey: true });
  await page.settle();
  assert.equal(String(picker.value), second.id, "Alt+↑ did not move to the previous channel");
  // On the picker itself, Alt+↓ is the picker's own (it opens it).
  event = await press(page, picker, "ArrowDown", { altKey: true });
  assert.equal(event.defaulted, true, "Alt+↓ on the focused picker was taken from it");
  assert.equal(String(picker.value), second.id);
  for (const mod of ["ctrlKey", "metaKey"]) {
    page.el("scroll-area").focus();
    event = await press(page, page.el("scroll-area"), "k", { [mod]: true });
    assert.equal(event.defaulted, false, `${mod}+K was left to the browser`);
    assert.equal(focused(page), picker, `${mod}+K left the focus on ${focusName(page)}, not the channel picker`);
  }
});

/** Whether the focus is on the channel's list: its scroller, or a message in it. */
const onChannelList = (page) => focused(page) === page.el("scroll-area") ||
  Boolean(focused(page) && focused(page).parentNode === page.el("discord-log"));

test("A CHANNEL CHOSEN FROM Ctrl+K gives the keys back to the list; Esc on a picker of the bar does too", async () => {
  const page = newPage();
  const second = { id: "1110000000000000002", label: "second", writable: true };
  const third = { id: "1110000000000000003", label: "third", writable: true };
  page.channels = [{ ...CHANNEL }, second, third];
  await signIn(page);
  await showDiscord(page, tallChannel(30));
  const picker = page.el("discord-channel");
  await press(page, page.el("scroll-area"), "k", { ctrlKey: true });
  assert.equal(focused(page), picker, "Ctrl+K did not open the channel picker");
  // THE DEFECT: the focus stayed on the picker, where j did nothing, Esc did nothing, and the ↑ meant
  // for the previous message chose the channel above instead.
  picker.value = second.id;
  await picker.dispatch("change");
  await page.settle();
  assert.equal(String(picker.value), second.id);
  assert.ok(onChannelList(page), `after a channel was chosen from Ctrl+K the focus is on ${focusName(page)}`);
  let event = await pressHere(page, "j");
  assert.equal(event.defaulted, false, "j after choosing a channel was left to the browser");
  assert.match(selectedId(page), /^\d+$/, `j after choosing a channel selected ${selectedId(page)}`);
  await pressHere(page, "ArrowUp");
  await page.settle();
  assert.equal(String(picker.value), second.id, "↑ after choosing a channel chose another channel");
  // Ctrl+K and nothing chosen: Esc on the picker is the way back to the list, the channel unchanged.
  await press(page, focused(page), "k", { ctrlKey: true });
  assert.equal(focused(page), picker);
  event = await press(page, picker, "Escape");
  assert.equal(event.defaulted, false, "Esc on the picker was left to the browser");
  assert.ok(onChannelList(page), `Esc on the picker left the focus on ${focusName(page)}`);
  assert.equal(String(picker.value), second.id, "Esc on the picker changed the channel");
  assert.equal(page.screen(), "main");
  // A picker reached by a click keeps the focus through a change: ↑ and ↓ on a closed picker step it
  // one channel at a time off a Mac, firing `change` at each step. That Esc above also forgot the Ctrl+K.
  picker.focusFrom("pointer");
  picker.value = third.id;
  await picker.dispatch("change");
  await page.settle();
  assert.equal(focused(page), picker, `a change made on a clicked picker moved the focus to ${focusName(page)}`);
  await press(page, picker, "Escape");
  assert.ok(onChannelList(page), `Esc on a clicked picker left the focus on ${focusName(page)}`);
  assert.equal(String(picker.value), third.id);
  // The thread's picker on the bar is left the same way, and the first Esc goes no further.
  const threads = page.el("thread-select");
  const view = page.tab();
  event = await press(page, threads, "Escape");
  assert.equal(event.defaulted, false, "Esc on the thread picker was left to the browser");
  assert.ok(onChannelList(page), `Esc on the thread picker left the focus on ${focusName(page)}`);
  assert.equal(page.screen(), "main");
  assert.equal(page.tab(), view, "Esc on the thread picker left the channel's view");
});

test("ON A MAC, Ctrl+K in a message box is the box's own (delete to the end of the line); Cmd+K picks the channel there", async () => {
  const page = await keyboardPage("MacIntel");
  const box = page.el("channel-compose-text");
  const picker = page.el("discord-channel");
  box.focus();
  await box.setValue("delete the rest of this line");
  // By its character, and by the place of the key on a layout that types no Latin letter there.
  for (const fields of [{ ctrlKey: true, code: "KeyK" }, { ctrlKey: true, code: "KeyK", key: "л" }]) {
    const event = await press(page, box, fields.key || "k", fields);
    assert.equal(event.defaulted, true, `Ctrl+${fields.key || "k"} in the box on a Mac was taken from the field`);
    assert.equal(focused(page), box, `Ctrl+${fields.key || "k"} in the box on a Mac moved the focus to ${focusName(page)}`);
  }
  assert.equal(box.value, "delete the rest of this line");
  let event = await press(page, box, "k", { metaKey: true, code: "KeyK" });
  assert.equal(event.defaulted, false, "Cmd+K in the box on a Mac was left to the browser");
  assert.equal(focused(page), picker, `Cmd+K in the box on a Mac left the focus on ${focusName(page)}`);
  // Off a field, Ctrl+K is the picker on a Mac too.
  event = await press(page, page.el("scroll-area"), "k", { ctrlKey: true, code: "KeyK" });
  assert.equal(event.defaulted, false);
  assert.equal(focused(page), picker, `Ctrl+K on the list on a Mac left the focus on ${focusName(page)}`);
  await press(page, picker, "Escape");
  // Ctrl+S edits nothing on a Mac, and is the message box there as everywhere: from the box, the box.
  // `#248 composer-key`.
  event = await press(page, box, "s", { ctrlKey: true, code: "KeyS" });
  assert.equal(event.defaulted, false, "Ctrl+S in the box on a Mac was left to the browser's Save page");
  assert.equal(focused(page), box, `Ctrl+S in the box on a Mac left the focus on ${focusName(page)}`);
  assert.equal(box.value, "delete the rest of this line", "Ctrl+S in the box on a Mac touched the draft");
  // Elsewhere Ctrl has no editing keys of its own in a field, and Ctrl+K picks the channel from the box.
  const linux = await keyboardPage("Linux x86_64");
  linux.el("channel-compose-text").focus();
  event = await press(linux, linux.el("channel-compose-text"), "k", { ctrlKey: true, code: "KeyK" });
  assert.equal(event.defaulted, false);
  assert.equal(focused(linux), linux.el("discord-channel"), `Ctrl+K in the box off a Mac left the focus on ${focusName(linux)}`);
});

test("HELP LEFT BY THE SEARCH OR SETTINGS KEY forgets the ?: the next Help from Settings goes back to Settings", async () => {
  for (const [key, fields] of [[",", {}], [",", { ctrlKey: true }], ["/", {}], ["/", { ctrlKey: true }],
    ["s", { ctrlKey: true }]]) {
    const said = `${fields.ctrlKey ? "Ctrl+" : ""}${key}`;
    const page = await keyboardPage();
    await press(page, page.el("scroll-area"), "?", { shiftKey: true });
    assert.equal(page.screen(), "help");
    await press(page, page.el("screen-help"), key, fields);
    assert.equal(page.screen(), "main", `${said} on Help did not leave it`);
    if (focused(page) === page.el("search-field")) await press(page, page.el("search-field"), "Escape");
    // Help from a group's ? in Settings: its Back, and Esc, go back to Settings (#85).
    await page.el("open-settings").click();
    await page.el("help-link-keyboard").click();
    assert.equal(page.screen(), "help");
    await page.el("close-help").click();
    assert.equal(page.screen(), "settings", `after ? and then ${said}, Back on Help from Settings went to ${page.screen()}`);
    await page.el("help-link-keyboard").click();
    await press(page, page.el("screen-help"), "Escape");
    assert.equal(page.screen(), "settings", `after ? and then ${said}, Esc on Help from Settings went to ${page.screen()}`);
  }
});

test("PgDn FROM THE SELECTED MESSAGE waits for ONE scroll to end, and for none at the end of the list", async () => {
  const page = await keyboardPageAtTop();
  const area = page.el("scroll-area");
  // A browser with the event: the ring follows a page key once its scroll comes to rest.
  area.onscrollend = null;
  const waiting = () => (area.listeners.get("scrollend") || []).length;
  await press(page, area, "ArrowDown");
  assert.equal(selectedId(page), "1");
  // At the top, PgUp cannot move the list, and nothing waits for a scroll that will not come.
  await pressHere(page, "PageUp");
  assert.equal(waiting(), 0, "PgUp at the top waited for a scroll");
  // A held PgDn is one wait, not one per press.
  for (let i = 0; i < 5; i += 1) await pressHere(page, "PageDown", { repeat: i > 0 });
  assert.equal(waiting(), 1, `a held PgDn left ${waiting()} listeners waiting`);
  // The browser's scroll comes to rest: the ring moves to the first message shown whole.
  area.scrollTop += Math.round(SCROLL_VIEWPORT_PX * 0.875);
  await area.dispatch("scrollend");
  const followed = selectedId(page);
  assert.notEqual(followed, "1", "the ring did not follow the page");
  const box = channelRow(page, followed).getBoundingClientRect();
  assert.ok(box.top >= 0 && box.bottom <= SCROLL_VIEWPORT_PX, `the ring followed to a row off screen: ${JSON.stringify(box)}`);
  // A later scroll of the wheel finds nothing waiting: the wheel never moves the ring.
  area.scrollTop += 2 * SCROLL_VIEWPORT_PX;
  await area.dispatch("scrollend");
  assert.equal(selectedId(page), followed, "a scroll of the wheel moved the ring");
  // At the end of the list PgDn and Space cannot move it either.
  area.scrollTop = area.scrollHeight - area.clientHeight;
  await pressHere(page, "PageDown");
  await pressHere(page, " ");
  await area.dispatch("scrollend");
  assert.equal(selectedId(page), followed, "PgDn at the end of the list moved the ring at the next scroll's end");
  assert.equal(waiting(), 1, "the pane's one listener was added again");
});

test("? AND Ctrl+? open the list of every key, and Esc goes back to where they were pressed", async () => {
  for (const [key, fields] of [["?", { shiftKey: true }], ["?", { ctrlKey: true, shiftKey: true }],
    ["?", { metaKey: true, shiftKey: true }], ["/", { metaKey: true, shiftKey: true, code: "Slash" }]]) {
    const page = await keyboardPage();
    const area = page.el("scroll-area");
    area.scrollTop = 1000;
    const scrolled = page.el("help-keyboard").scrolledIntoView;
    let event = await press(page, area, key, fields);
    assert.equal(event.defaulted, false, `${key} was left to the browser`);
    assert.equal(page.screen(), "help", `${key} did not open Help`);
    assert.equal(page.el("help-keyboard").scrolledIntoView, scrolled + 1, `${key} did not open Help at the Keyboard entry`);
    assert.equal(focused(page), page.el("screen-help"));
    event = await press(page, page.el("screen-help"), "Escape");
    assert.equal(page.screen(), "main", `Esc on the list of keys opened by ${key} did not go back to the messages`);
    assert.equal(area.scrollTop, 1000, "back from the list of keys the reader was not where they were");
    assert.equal(focused(page), area);
  }
  // From Settings, it comes back to Settings; Help's own Back still goes to Settings.
  const page = await keyboardPage();
  await press(page, page.el("scroll-area"), ",");
  await press(page, page.el("screen-settings"), "?", { shiftKey: true });
  assert.equal(page.screen(), "help");
  await page.el("close-help").click();
  assert.equal(page.screen(), "settings", "Back on Help opened from Settings did not go back to Settings");
  // `#410 inline-reply-editor`. Beside a reply open in the list, ? is the list's: Help, and back from it
  // the reply is still open.
  const reply = newPage();
  await signIn(reply);
  await openReplyOn(reply, tallChannel(5), 1);
  const event = await press(reply, reply.el("scroll-area"), "?", { shiftKey: true });
  assert.equal(event.defaulted, false);
  assert.equal(reply.screen(), "help", "? beside a reply did not open Help");
  await press(reply, reply.el("screen-help"), "Escape");
  assert.ok(replyInStream(reply), "back from Help, the reply was put away");
});

// --- the search from anywhere (#344 search-shortcut) ---------------------------------------------
//
// The owner: "want reliable cmd-/ for search ... Sometimes just pressing '/' doesn't work ... prefer it
// to just be Cmd-/. Currently that pops open help ... map it to Cmd-? (i.e. Cmd-shift-/)." A bare "/"
// is a character in a field, the picker's type-to-find and nothing with single keys off; the chord is
// none of those. tests/search_chord_browser.py presses the same chords in Chromium through CDP.

/** Whether the search is open with its field holding the focus. */
const searchReady = (page) => !page.el("search-field").hidden && focused(page) === page.el("search-field");

/** `keyboardPage`, on `script`. */
async function keyboardPageOn(script, platform = "Linux x86_64") {
  const page = newPage(new Map(), script, (p) => p.setPlatform(platform));
  await signIn(page);
  await showDiscord(page, tallChannel(30));
  return page;
}

test("`#344 search-shortcut`: Ctrl+/ AND Cmd+/ OPEN THE SEARCH FROM ANYWHERE, whatever holds the focus, and a bare / is as it was", async () => {
  for (const [platform, mod] of [["Linux x86_64", "ctrlKey"], ["MacIntel", "metaKey"]]) {
    const said = `${platform}, ${mod === "ctrlKey" ? "Ctrl" : "Cmd"}+/`;
    /** The chord on `target`: the key taken, the main screen up, the search open and its field focused. */
    const opens = async (page, target, where, extra = {}) => {
      const event = await press(page, target, "/", { [mod]: true, ...extra });
      assert.equal(event.defaulted, false, `${said} ${where} was left to the browser`);
      assert.equal(page.screen(), "main", `${said} ${where} left the screen on ${page.screen()}`);
      assert.ok(searchReady(page), `${said} ${where}: the search is ${page.el("search-field").hidden ? "shut" : "open"}, ` +
        `the focus on ${focusName(page)}`);
      return event;
    };
    // The list, and a selected message.
    let page = await keyboardPage(platform);
    await opens(page, page.el("scroll-area"), "on the list");
    page = await keyboardPage(platform);
    await press(page, page.el("scroll-area"), "ArrowDown");
    assert.ok(selectedId(page), "fixture: ↓ selected no message");
    await opens(page, focused(page), "on a selected message");
    // A message box with a draft: no slash typed and the draft as it was; a bare "/" there is still a slash.
    page = await keyboardPage(platform);
    const box = page.el("channel-compose-text");
    await box.setValue("see the ");
    const slash = await press(page, box, "/");
    assert.equal(slash.defaulted, true, "a bare \"/\" in the message box was taken from it");
    assert.equal(page.el("search-field").hidden, true, "a bare \"/\" in the message box opened the search");
    await opens(page, box, "in the message box");
    assert.equal(box.value, "see the ", `${said} touched the draft`);
    // The search field with words in it: the field again, its text selected to be typed over.
    await page.el("search-field").setValue("deploy");
    const selected = page.el("search-field").selectedTimes || 0;
    await opens(page, page.el("search-field"), "in the search field");
    assert.equal(page.el("search-field").value, "deploy", `${said} in the search field lost its words`);
    assert.equal(page.el("search-field").selectedTimes, selected + 1, `${said} in the search field did not select its text`);
    // On a layout where "/" takes Shift (Shift+7 on German), the chord with that key.
    page = await keyboardPage(platform);
    await opens(page, page.el("channel-compose-text"), "as Shift+7 on a German keyboard", { shiftKey: true, code: "Digit7" });
    // The pickers, where a bare "/" is the picker's type-to-find, and the reading-width handle.
    for (const id of ["discord-channel", "thread-select", "width-grip"]) {
      page = await keyboardPage(platform);
      if (id !== "width-grip") {
        await press(page, page.el(id), "/");
        assert.equal(page.el("search-field").hidden, true, `a bare "/" on #${id} opened the search`);
      }
      await opens(page, page.el(id), `on #${id}`);
    }
    // `#340 composer-grow-box`. The Grow checkbox beside Send, which takes no typing: a bare "/" there
    // searches as on any checkbox, and so does the chord.
    page = await keyboardPage(platform);
    await press(page, page.el("channel-compose-grow"), "/");
    assert.ok(searchReady(page), `a bare "/" on the Grow checkbox left the focus on ${focusName(page)}`);
    page = await keyboardPage(platform);
    await opens(page, page.el("channel-compose-grow"), "on the Grow checkbox");
    // An open ⋯ menu, the focus on an item: closed first.
    page = await keyboardPage(platform);
    const row = page.el("discord-log").children[4];
    await rowMoreButton(row).click();
    const item = rowMoreMenu(row).descendants().find((node) => node.tagName === "button");
    assert.ok(item && !rowMoreMenu(row).hidden, "fixture: the ⋯ menu did not open with an item in it");
    await opens(page, item, "on an item of an open ⋯ menu");
    assert.equal(rowMoreMenu(row).hidden, true, `${said} left the ⋯ menu open`);
    // The reading-pace popover, the focus on its slider: closed first.
    page = await keyboardPage(platform);
    await page.el("read-speed").click();
    await page.settle();
    assert.equal(page.el("speed-popover").hidden, false, "fixture: the pace button did not open its popover");
    await press(page, page.el("read-speed-box"), "/");
    assert.equal(page.el("search-field").hidden, true, "a bare \"/\" in the pace box opened the search");
    await opens(page, page.el("read-speed-box"), "on the pace popover's box");
    assert.equal(page.el("speed-popover").hidden, true, `${said} left the pace popover open`);
    // The prompts tray: closed first.
    page = await keyboardPage(platform);
    await page.el("prompts-open").click();
    await page.settle();
    assert.equal(page.el("prompts-tray").hidden, false, "fixture: the prompts tray did not open");
    await opens(page, page.el("prompts-open"), "with the prompts tray open");
    assert.equal(page.el("prompts-tray").hidden, true, `${said} left the prompts tray open`);
    // A message's details sheet is part of its row, not something over the list: it stays.
    page = await keyboardPage(platform);
    await press(page, page.el("scroll-area"), "ArrowDown");
    const lit = focused(page);
    await press(page, lit, "i");
    const sheet = () => lit.descendants().find((node) => node.className === "msg-details");
    assert.ok(sheet(), "fixture: i opened no details sheet");
    await opens(page, lit, "on a message with its details open");
    assert.ok(sheet(), `${said} closed the details sheet`);
    // Settings, from a text field on it: left, the field's words as they were.
    page = await keyboardPage(platform);
    await page.el("open-settings").click();
    await page.el("noise-rule-new").setValue("build bot");
    await opens(page, page.el("noise-rule-new"), "in a text field on Settings");
    assert.equal(page.el("noise-rule-new").value, "build bot", `${said} on Settings changed the field`);
    // Help.
    page = await keyboardPage(platform);
    await press(page, page.el("scroll-area"), "?", { shiftKey: true });
    assert.equal(page.screen(), "help", "fixture: ? did not open Help");
    await opens(page, page.el("screen-help"), "on Help");
    // Single keys off: the bare "/" goes with them, the chord does not.
    page = await keyboardPage(platform);
    await page.el("open-settings").click();
    await page.el("single-keys").setChecked(false);
    await page.el("close-settings").click();
    const off = await press(page, page.el("scroll-area"), "/");
    assert.equal(off.defaulted, true, "with single keys off, a bare \"/\" was taken");
    assert.equal(page.el("search-field").hidden, true, "with single keys off, a bare \"/\" opened the search");
    await opens(page, page.el("scroll-area"), "with single keys off");
    // `#410 inline-reply-editor`. A reply open in the list stays open, its words in its box: the search
    // is over the list it is in, a bare "/" in the box is a character, and the chord opens it from there.
    page = await keyboardPage(platform);
    await openReplyOn(page, tallChannel(30), 2);
    await page.el("reply-text").setValue("half a reply");
    const bare = await press(page, page.el("reply-text"), "/");
    assert.equal(bare.defaulted, true, "a bare \"/\" in the reply's box was not left to the box");
    assert.equal(page.el("search-field").hidden, true, "a bare \"/\" in the reply's box opened the search");
    await opens(page, page.el("reply-text"), "in the reply's box");
    assert.ok(replyInStream(page), `${said} in the reply's box put the reply away`);
    assert.equal(page.el("reply-text").value, "half a reply", `${said} in the reply's box lost the reply's words`);
  }
  // Two columns: the right column's box, its draft as it was; then a reply open at the foot of that
  // column stays open, its draft with it: the search is over the list beside it.
  const { page } = await twoColumnServer();
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  assert.equal(page.el("side-pane").hidden, false, "fixture: the thread did not open on the right");
  const side = page.el("side-compose-text");
  await side.setValue("in the thread");
  let event = await press(page, side, "/", { ctrlKey: true });
  assert.equal(event.defaulted, false);
  assert.ok(searchReady(page), `Ctrl+/ in the right column's box left the focus on ${focusName(page)}`);
  assert.equal(side.value, "in the thread", "Ctrl+/ in the right column's box touched its draft");
  await press(page, page.el("search-field"), "Escape");
  await replyButton(sideRow(page, "202")).click();
  await page.settle();
  assert.equal(replyBoxAt(page), "right", "fixture: Reply in the right column did not open its box at that column's foot");
  await page.el("reply-text").setValue("draft words");
  event = await press(page, page.el("reply-text"), "/", { ctrlKey: true });
  assert.equal(event.defaulted, false);
  assert.ok(searchReady(page), `Ctrl+/ in a reply open in a column left the focus on ${focusName(page)}`);
  assert.equal(replyBoxAt(page), "right", "Ctrl+/ put down the reply open in the column");
  assert.equal(page.el("reply-text").value, "draft words");
  // Sign-in, where there is nothing to search: nothing opens, and the chord is not the browser's either.
  const signin = newPage();
  assert.equal(signin.screen(), "signin", "fixture: a new page is not on sign-in");
  event = await press(signin, signin.el("api-token"), "/", { ctrlKey: true });
  assert.equal(event.defaulted, false, "Ctrl+/ on sign-in was left to the browser");
  assert.equal(signin.screen(), "signin");
  assert.equal(signin.el("search-field").hidden, true, "Ctrl+/ on sign-in opened the search");
});

test("`#344 search-shortcut`: THE SEARCH CHORD IS HEARD FIRST, at the window in the capture phase, so a listener that stops the key cannot keep it from the search", async () => {
  const page = await keyboardPage();
  assert.equal(page.windowListenersFor("keydown", true).length, 1, "not one keydown listener in the window's capture phase");
  assert.equal(page.windowListenersFor("keydown", false).length, 0, "a keydown listener in the window's bubble phase");
  /** A listener on the message box that notes whether the search is open when it runs, and stops the key there. */
  const stopper = (target) => {
    const seen = [];
    target.addEventListener("keydown", (event) => {
      seen.push(!page.el("search-field").hidden);
      event.stopPropagation();
      event.preventDefault();
    });
    return seen;
  };
  const seen = stopper(page.el("channel-compose-text"));
  await page.el("channel-compose-text").setValue("draft");
  const event = await press(page, page.el("channel-compose-text"), "/", { ctrlKey: true });
  assert.deepStrictEqual(seen, [true], "the box's own listener ran before the search was open");
  assert.equal(event.defaulted, false);
  assert.ok(searchReady(page), `with the key stopped at the box, Ctrl+/ left the focus on ${focusName(page)}`);
  // NEGATIVE CONTROL: without the window's listener the table's own walk still searches on Ctrl+/ (the
  // same binding, from the document), until something on the way stops the key.
  const deafScript = brokenScript('window.addEventListener("keydown", onEverywhereKey, { capture: true });', "");
  const deaf = await keyboardPageOn(deafScript);
  await press(deaf, deaf.el("channel-compose-text"), "/", { ctrlKey: true });
  assert.ok(searchReady(deaf), "without the window's listener Ctrl+/ did not search from the document either");
  const stopped = await keyboardPageOn(deafScript);
  stopper(stopped.el("channel-compose-text"));
  await press(stopped, stopped.el("channel-compose-text"), "/", { ctrlKey: true });
  assert.equal(stopped.el("search-field").hidden, true, "the control without the window's listener searched though the key was stopped");
});

test("`#344 search-shortcut`: A HELD Ctrl+/ OPENS THE SEARCH ONCE, its repeats refused and doing nothing", async () => {
  const page = await keyboardPage();
  await press(page, page.el("scroll-area"), "/", { ctrlKey: true });
  assert.ok(searchReady(page));
  const field = page.el("search-field");
  const selected = field.selectedTimes || 0;
  for (let i = 0; i < 3; i += 1) {
    const repeat = await press(page, field, "/", { ctrlKey: true, repeat: true });
    assert.equal(repeat.defaulted, false, "a held Ctrl+/'s repeat was left to the browser");
  }
  assert.equal(field.selectedTimes || 0, selected, "a held Ctrl+/'s repeats selected the field again");
  assert.ok(searchReady(page), "a held Ctrl+/'s repeats closed the search");
});

test("THE LIST OF KEYS IS THE KEYMAP: every binding's words and meaning, in the reader's own keyboard words", async () => {
  // Read off the source's own table, so the list cannot be checked against a second copy of it.
  const table = /const KEYMAP = \[([\s\S]*?)\n\];/.exec(SCRIPT_CODE);
  assert.ok(table, "web/voice.js has no KEYMAP table");
  const meanings = [...table[1].matchAll(/does:\s*((?:"(?:[^"\\]|\\.)*"\s*\+?\s*)+)/g)].map((m) =>
    [...m[1].matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((part) => JSON.parse(`"${part[1]}"`)).join(""));
  assert.ok(meanings.length >= 30, `only ${meanings.length} bindings were found in KEYMAP`);
  for (const [platform, mod, alt, other] of [["Linux x86_64", "Ctrl", "Alt", "Cmd"], ["MacIntel", "Cmd", "Option", "Ctrl"]]) {
    const page = newPage(new Map(), SCRIPT, (p) => p.setPlatform(platform));
    const list = page.el("keyboard-keys").text();
    for (const meaning of meanings) {
      assert.ok(list.includes(meaning), `${platform}: the list of keys leaves out "${meaning}"`);
    }
    for (const words of ["↓ or j", "↑ or k", "Enter or o", "Esc", "PgUp", "Shift+E", `${mod}+1`, `${mod}+2`, `${mod}+3`,
      `${mod}+4`, `${mod}+S`, `${mod}+,`, `${mod}+K`, `${mod}+/`, `${mod}+?`, `${mod}+Home`, `${alt}+↓`, "Shift+P",
      "Shift+L", `e or ${mod}+E`, `${mod}+E does it from a message box or the search too`]) {
      assert.ok(list.includes(words), `${platform}: the list of keys does not name ${words}`);
    }
    // `#281 view-keys`. The bare digits are gone from it, and it says so, and where a browser may keep the chords.
    assert.ok(!list.includes("1, 2, 3"), `${platform}: the list of keys still names the bare digits`);
    assert.ok(list.includes(`${mod}+1 to ${mod}+4 work from anywhere, in a message box or the search too`),
      `${platform}: the list of keys does not say the view chords work from anywhere`);
    assert.ok(list.includes("1, 2 and 3 on their own no longer change the view"),
      `${platform}: the list of keys does not say what became of the bare digits`);
    assert.ok(list.includes("a browser that keeps them for its tabs switches tabs instead"),
      `${platform}: the list of keys does not say a browser may keep the chords`);
    assert.ok(!list.includes(`${other}+`), `${platform}: the list of keys names ${other}`);
    // `#248 composer-key`. The chord left the search for the message box, and its tooltip with it;
    // `#344 search-shortcut` gave it ${mod}+/, and the list says where that chord went from Help.
    assert.equal(page.el("search-toggle").getAttribute("title"), `Search messages (/ or ${mod}+/)`, platform);
    assert.ok(list.includes(`${mod}+/ searches from anywhere`), `${platform}: the list of keys does not say ${mod}+/ searches`);
    assert.ok(list.includes(`${mod}+? is ${mod}+Shift+/; ${mod}+/, which opened this list until now, searches`),
      `${platform}: the list of keys does not say where ${mod}+/ went`);
    assert.ok(list.includes(`${mod}+S, which searched until now, goes to the message box`),
      `${platform}: the list of keys does not say where ${mod}+S went`);
    assert.equal(page.el("open-settings").getAttribute("title"), `Settings (, or ${mod}+,)`, platform);
    assert.equal(page.el("close-settings").getAttribute("title"), "Back (Esc)");
  }
  assert.ok(markupHolds("screen-help", "keyboard-keys"), "the list of keys is not on the Help screen");
  assert.match(helpEntry("keyboard"), /id="keyboard-keys"/);
});

test("aria-keyshortcuts names each control's key, from the same table, and drops a key the switch turns off", async () => {
  const page = await keyboardPage();
  const row = page.el("discord-log").children[3];
  const expected = new Map([
    [page.el("search-toggle"), "/ Control+/ Meta+/"],
    [page.el("help-link-keyboard"), "? Control+? Meta+?"],
    [page.el("channel-compose-text"), "M Control+S Meta+S"],
    [page.el("reply-text"), "M Control+S Meta+S"],
    [page.el("compose-text"), "M Control+S Meta+S"],
    [page.el("open-settings"), ", Control+, Meta+,"],
    [page.el("close-settings"), "Escape"],
    [page.el("thread-back"), "Escape ArrowLeft"],
    [page.el("todo-filter"), "H"],
    [page.el("discord-channel"), "Alt+ArrowDown Alt+ArrowUp Control+K Meta+K"],
    [doneButton(row), "E Control+E Meta+E"],
    [replyButton(row), "R"],
    [foldButton(row), "Enter O"],
    [rowMoreButton(row), "."],
  ]);
  for (const [control, shortcuts] of expected) {
    assert.equal(control.getAttribute("aria-keyshortcuts"), shortcuts, `#${control.id || control.className}`);
  }
  await page.el("open-settings").click();
  await page.el("single-keys").setChecked(false);
  assert.equal(page.el("search-toggle").getAttribute("aria-keyshortcuts"), "Control+/ Meta+/",
    "a search key turned off is still announced, or the chord the switch leaves on is not");
  assert.equal(page.el("help-link-keyboard").getAttribute("aria-keyshortcuts"), "Control+? Meta+?");
  assert.equal(page.el("search-toggle").getAttribute("title"), "Search messages (Ctrl+/)");
  assert.equal(page.el("channel-compose-text").getAttribute("aria-keyshortcuts"), "Control+S Meta+S");
  assert.equal(page.el("todo-filter").getAttribute("aria-keyshortcuts"), null, "a key turned off is still announced");
  assert.equal(doneButton(page.el("discord-log").children[3]).getAttribute("aria-keyshortcuts"), "Control+E Meta+E",
    "a row's Done still announces a key the switch turned off, or lost the chord the switch leaves on");
  assert.equal(foldButton(page.el("discord-log").children[3]).getAttribute("aria-keyshortcuts"), "Enter");
});

// --- `cmd-e-done` -------------------------------------------------------------------------------
//
// The owner, 2026-10-19: "I like the single keystroke e for marking messages as done. BUT if I have a
// text box highlighted it won't work ... I want Cmd-e to also work." Ctrl+E (Cmd+E) is e from anywhere:
// in a message box or the search it marks the message the keys act on, and the box keeps the focus and
// what is typed in it. tests/keyboard_browser.py presses it in a real Chromium.

/** A channel at its top on `platform`, `store` holding the reader's settings, with the first message selected by ↓. */
async function cmdEPage(platform = "Linux x86_64", store = new Map()) {
  const page = newPage(store, SCRIPT, (p) => p.setPlatform(platform));
  await signIn(page);
  await showDiscord(page, tallChannel(30));
  page.el("scroll-area").scrollTop = 0;
  await press(page, page.el("scroll-area"), "ArrowDown");
  assert.equal(selectedId(page), "1", "↓ did not select the first message");
  return page;
}

test("`cmd-e-done`: Ctrl+E or Cmd+E in a message box marks the selected message Done, and the box keeps the focus and the draft", async () => {
  const page = await cmdEPage();
  const box = page.el("channel-compose-text");
  box.focus();
  await box.setValue("half a thought");
  // A bare e there is typed, as it always was.
  let event = await press(page, box, "e", { code: "KeyE" });
  assert.equal(event.defaulted, true, "a bare e in the message box was taken from it");
  assert.equal(page.dismissCalls.length, 0, "a bare e in the message box marked a message Done");
  for (const [fields, id, next] of [[{ ctrlKey: true }, "1", "2"], [{ metaKey: true }, "2", "3"]]) {
    const what = fields.ctrlKey ? "Ctrl+E" : "Cmd+E";
    event = await press(page, box, "e", { code: "KeyE", ...fields });
    assert.equal(event.defaulted, false, `${what} in the box was left to the browser (Chrome's own find or address bar)`);
    assert.deepStrictEqual(page.dismissCalls.at(-1).messages, [id], `${what} did not mark message ${id} Done`);
    assert.equal(channelRow(page, id).getAttribute("data-archived"), "true");
    assert.equal(focused(page), box, `${what} took the focus out of the box, to ${focusName(page)}`);
    assert.equal(box.value, "half a thought", `${what} touched the draft`);
    assert.deepStrictEqual(litIds(page), [next], `after ${what} the selection is not on the next unread, ${next}`);
  }
  // A held chord is one Done; its repeats are taken all the same, each being the browser's own chord too.
  const calls = page.dismissCalls.length;
  event = await press(page, box, "e", { code: "KeyE", ctrlKey: true, repeat: true });
  assert.equal(page.dismissCalls.length, calls, "a held Ctrl+E marked another message Done");
  assert.equal(event.defaulted, false, "a held Ctrl+E's repeat was left to the browser");
  // Off the box it is e too, the focus going with the selection as e's does.
  await press(page, page.el("scroll-area"), "ArrowDown");
  const from = selectedId(page);
  event = await pressHere(page, "e", { code: "KeyE", ctrlKey: true });
  assert.equal(event.defaulted, false);
  assert.deepStrictEqual(page.dismissCalls.at(-1).messages, [from], "Ctrl+E on the list did not mark the selected message");
  assert.notEqual(selectedId(page), from, "Ctrl+E on the list left the focus on the message it marked");
});

test("`cmd-e-done`: with no message to act on, Ctrl+E in the box is left to the browser", async () => {
  const page = await keyboardPage();
  const box = page.el("channel-compose-text");
  const event = await press(page, box, "e", { code: "KeyE", ctrlKey: true });
  assert.equal(event.defaulted, true, "Ctrl+E with no message selected was taken anyway");
  assert.equal(page.dismissCalls.length, 0);
  assert.equal(focused(page), box);
  const held = await press(page, box, "e", { code: "KeyE", ctrlKey: true, repeat: true });
  assert.equal(held.defaulted, true, "a held Ctrl+E with no message selected was taken anyway");
});

test("`cmd-e-done`: ON A MAC, Ctrl+E in the box is the box's (the end of the line) and Cmd+E marks Done", async () => {
  const page = await cmdEPage("MacIntel");
  const box = page.el("channel-compose-text");
  box.focus();
  await box.setValue("to the end of this line");
  let event = await press(page, box, "e", { code: "KeyE", ctrlKey: true });
  assert.equal(event.defaulted, true, "Ctrl+E in the box on a Mac was taken from the field");
  assert.equal(page.dismissCalls.length, 0, "Ctrl+E in the box on a Mac marked a message Done");
  event = await press(page, box, "e", { code: "KeyE", metaKey: true });
  assert.equal(event.defaulted, false, "Cmd+E in the box on a Mac was left to the browser");
  assert.deepStrictEqual(page.dismissCalls.map((call) => call.messages), [["1"]]);
  assert.equal(focused(page), box);
  // Off a field, Ctrl+E is e on a Mac too.
  await press(page, page.el("scroll-area"), "ArrowDown");
  event = await pressHere(page, "e", { code: "KeyE", ctrlKey: true });
  assert.equal(event.defaulted, false, "Ctrl+E on the list on a Mac was left to the browser");
  assert.equal(page.dismissCalls.length, 2, "Ctrl+E on the list on a Mac marked nothing");
});

test("`cmd-e-done`: the chord works in the search field, and with the single-key shortcuts turned off", async () => {
  const page = await cmdEPage("Linux x86_64", new Map([["vibe-talk.voice.single-keys", "0"]]));
  // The switch is off: a bare e on the list does nothing, and the chord still acts.
  let event = await pressHere(page, "e", { code: "KeyE" });
  assert.equal(page.dismissCalls.length, 0, "a bare e marked Done with the single-key shortcuts off");
  const box = page.el("channel-compose-text");
  event = await press(page, box, "e", { code: "KeyE", ctrlKey: true });
  assert.equal(event.defaulted, false, "Ctrl+E did nothing with the single-key shortcuts off");
  assert.deepStrictEqual(page.dismissCalls.map((call) => call.messages), [["1"]]);
  await page.el("search-toggle").click();
  const field = page.el("search-field");
  assert.equal(field.hidden, false, "the search did not open");
  event = await press(page, field, "e", { code: "KeyE", ctrlKey: true });
  assert.equal(event.defaulted, false, "Ctrl+E in the search field was left to the browser");
  assert.deepStrictEqual(page.dismissCalls.at(-1).messages, ["2"], "Ctrl+E in the search did not mark the selected message");
  assert.equal(focused(page), field, `Ctrl+E took the focus out of the search, to ${focusName(page)}`);
});

test("`cmd-e-done`: a selected message scrolled out of sight is not marked from the box; the first showing is selected instead", async () => {
  const page = await cmdEPage();
  const area = page.el("scroll-area");
  scrollToBottom(area);
  const box = page.el("channel-compose-text");
  const event = await press(page, box, "e", { code: "KeyE", ctrlKey: true });
  assert.equal(event.defaulted, false);
  assert.equal(page.dismissCalls.length, 0, "Ctrl+E marked a message out of sight Done");
  assert.equal(focused(page), box, `Ctrl+E took the focus out of the box, to ${focusName(page)}`);
  assert.equal(litIds(page).length, 1, "nothing is selected after Ctrl+E on a message out of sight");
  assert.notEqual(litIds(page)[0], "1", "the selection stayed on the message out of sight");
});

test("THE SINGLE-KEY SWITCH turns off every key that types a character, and keeps the arrows, Esc, Enter and the chords", async () => {
  const store = new Map();
  const page = newPage(store);
  await signIn(page);
  await showDiscord(page, tallChannel(30));
  assert.equal(page.el("single-keys").checked, true, "single-key shortcuts are not on by default");
  assert.equal(store.has("vibe-talk.voice.single-keys"), false, "the default was written down as a choice");
  await page.el("open-settings").click();
  await page.el("single-keys").setChecked(false);
  assert.equal(store.get("vibe-talk.voice.single-keys"), "0", "turning them off was not kept");
  assert.match(page.el("single-keys-hint").text(), /Off: Esc, the arrows, Enter and the Ctrl chords still work/);
  await press(page, page.el("screen-settings"), "Escape");
  assert.equal(page.screen(), "main", "with single keys off, Esc no longer leaves Settings");
  const area = page.el("scroll-area");
  area.scrollTop = 0;
  for (const [key, fields] of [["j", {}], ["k", {}], ["e", {}], ["/", {}], [",", {}], ["?", { shiftKey: true }],
    ["1", {}], ["h", {}], ["E", { shiftKey: true }], [";", {}], ["c", {}]]) {
    const event = await press(page, area, key, fields);
    assert.equal(event.defaulted, true, `with single keys off, ${key} was taken`);
    assert.equal(focused(page), area, `with single keys off, ${key} moved the focus`);
    assert.equal(page.screen(), "main");
    assert.equal(page.el("search-field").hidden, true);
  }
  assert.deepStrictEqual(page.dismissCalls, []);
  let event = await press(page, area, "ArrowDown");
  assert.equal(event.defaulted, false, "with single keys off, ↓ stopped selecting");
  assert.equal(selectedId(page), "1");
  event = await pressHere(page, "Enter");
  assert.equal(channelRow(page, "1").getAttribute("data-collapsed"), "false", "with single keys off, Enter stopped opening");
  event = await pressHere(page, "s", { ctrlKey: true });
  assert.equal(focused(page), page.el("channel-compose-text"), "with single keys off, Ctrl+S stopped going to the message box");
  // Kept across a reload; and absent means on.
  const again = newPage(store);
  assert.equal(again.el("single-keys").checked, false, "a reload turned single keys back on");
  assert.equal(newPage(new Map()).el("single-keys").checked, true);
});

test("NO SINGLE KEY FIRES WHILE TYPING, in any field on any screen, and the field keeps every character", async () => {
  const page = await keyboardPage();
  const singles = [["j", {}], ["k", {}], ["n", {}], ["o", {}], ["e", {}], ["E", { shiftKey: true }], ["z", {}], ["r", {}],
    ["p", {}], ["s", {}], [".", {}], ["i", {}], ["h", {}], ["P", { shiftKey: true }], ["L", { shiftKey: true }],
    [";", {}], [":", { shiftKey: true }], ["c", {}], ["/", {}], [",", {}], ["?", { shiftKey: true }], ["1", {}], ["2", {}],
    ["3", {}], ["m", {}], [" ", {}], ["ArrowDown", {}], ["ArrowLeft", {}], ["ArrowRight", {}], ["Home", {}], ["End", {}]];
  const fields = ["search-field", "channel-compose-text", "reply-text", "compose-text", "api-token", "prompt-summary",
    "prompt-blockers", "channel-alias", "new-channel-id", "new-channel-label", "channel-directory-search", "noise-rule-new"];
  for (const id of fields) {
    const field = page.el(id);
    for (const [key, extra] of singles) {
      const screen = page.screen();
      const event = await press(page, field, key, extra);
      const what = `${JSON.stringify(key)} in #${id}`;
      assert.equal(event.defaulted, true, `${what} was taken from the field`);
      assert.equal(focused(page), field, `${what} took the focus out of the field`);
      assert.equal(page.screen(), screen, `${what} changed the screen`);
    }
  }
  assert.deepStrictEqual(page.dismissCalls, [], "a key typed in a field marked something Done");
  assert.deepStrictEqual(page.pinCalls, [], "a key typed in a field pinned something");
});

// --- `#443 undo-key-general` ---------------------------------------------------------------------
//
// The owner: an undo keystroke should be a general undo, for anything that can be undone and
// anything that pops up an undo chip. Three chips exist — Done's, a dismissed send's, a removed
// bookmark's — and z (and Ctrl+Z / Cmd+Z outside a text field) takes back whichever was done last,
// then the next, exactly as that chip's click does.

test("`#443 undo-key-general`: z undoes the newest undoable act first, then the next, chip and key agreeing", async () => {
  const page = await servedBookmarkPage(backlog(6));
  await bookmarkRow(page, bid(3));
  // Done first, the bookmark's removal second: z takes the removal back, then the Done.
  const row = rowHoldingId(page, bid(2));
  await doneButton(row).click();
  await page.settle();
  assert.deepStrictEqual(page.dismissCalls.map((call) => call.messages), [[bid(2)]], "the premise: nothing was marked Done");
  assert.equal(page.el("undo-dismiss").hidden, false, "Done offered no Undo");
  await tapBookmarkClear(page);
  assert.equal(page.el("undo-bookmark").hidden, false, "the removal offered no Undo");
  await press(page, page.el("scroll-area"), "z");
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(3), place: "above" }, "z did not take the newest act (the removal) back first");
  assert.equal(page.el("undo-bookmark").hidden, true, "the bookmark chip stayed up after z used it");
  assert.equal(page.el("undo-dismiss").hidden, false, "the Done chip went with the bookmark's undo");
  assert.deepStrictEqual(page.restoreCalls, [], "z undid the Done under the removal");
  await press(page, page.el("scroll-area"), "z");
  await page.settle();
  assert.deepStrictEqual(page.restoreCalls.map((call) => call.messages), [[bid(2)]], "a second z did not undo the Done");
  assert.equal(page.el("undo-dismiss").hidden, true, "the Done chip stayed up after z used it");
  await press(page, page.el("scroll-area"), "z");
  await page.settle();
  assert.equal(statusText(page), "Nothing to undo.", "a third z, with every chip down, did not say so");

  // The other way round: removal first, Done second, and z takes the Done back first.
  await tapBookmarkClear(page);
  await doneButton(rowHoldingId(page, bid(4))).click();
  await page.settle();
  await press(page, page.el("scroll-area"), "z");
  await page.settle();
  assert.deepStrictEqual(page.restoreCalls.at(-1)?.messages, [bid(4)], "z did not take the newest act (the Done) back first");
  assert.equal(bookmarkAt(page), null, "the bookmark came back under its own undo");
  await press(page, page.el("scroll-area"), "z");
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(3), place: "above" }, "a second z did not put the bookmark back");
});

test("`#443 undo-key-general`: a dismissed send is in the same newest-first order as a Done", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "44301", content: "read this first" })]);
  const failed = await failedSend(page, "not yet, on second thoughts");
  await outgoingDismiss(failed).click();
  await page.settle();
  assert.equal(page.el("undo-send-dismiss").hidden, false, "the premise: the dismiss offered no Undo");
  await doneButton(channelRow(page, "44301")).click();
  await page.settle();
  // The Done is the newest: z restores it and leaves the send dismissed; a second z puts the send back.
  await press(page, page.el("scroll-area"), "z");
  await page.settle();
  assert.equal(channelRow(page, "44301").getAttribute("data-archived"), "false", "z did not undo the newest act (the Done)");
  assert.equal(outgoingRows(page).length, 0, "the send came back under the Done's undo");
  await press(page, page.el("scroll-area"), "z");
  await page.settle();
  assert.equal(outgoingRows(page).length, 1, "a second z did not put the dismissed send back");
  assert.equal(page.el("undo-send-dismiss").hidden, true, "the send chip stayed up after z used it");
});

test("`#443 undo-key-general`: Ctrl+Z and Cmd+Z undo as z, never in a text field, and Help lists both", async () => {
  const page = await servedBookmarkPage(backlog(6));
  await bookmarkRow(page, bid(3));
  await tapBookmarkClear(page);
  assert.equal(page.el("undo-bookmark").hidden, false, "the premise: no removal to undo");
  let event = await press(page, page.el("scroll-area"), "z", { ctrlKey: true });
  await page.settle();
  assert.equal(event.defaulted, false, "Ctrl+Z on the list was left to the browser");
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(3), place: "above" }, "Ctrl+Z did not undo the removal");
  await tapBookmarkClear(page);
  event = await press(page, page.el("scroll-area"), "z", { metaKey: true });
  await page.settle();
  assert.equal(event.defaulted, false, "Cmd+Z on the list was left to the browser");
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(3), place: "above" }, "Cmd+Z did not undo the removal");
  // In the message box and the search the chord is the browser's undo of typing: not taken.
  await tapBookmarkClear(page);
  assert.equal(page.el("undo-bookmark").hidden, false, "the premise: no removal to undo from a field");
  await page.el("channel-compose-text").setValue("typed words");
  event = await press(page, page.el("channel-compose-text"), "z", { ctrlKey: true });
  await page.settle();
  assert.equal(event.defaulted, true, "Ctrl+Z in the message box was taken from the box");
  assert.equal(bookmarkAt(page), null, "Ctrl+Z in the message box put the bookmark back");
  assert.equal(page.el("channel-compose-text").value, "typed words", "the chord touched the draft");
  await press(page, page.el("scroll-area"), "/");
  event = await press(page, page.el("search-field"), "z", { metaKey: true });
  await page.settle();
  assert.equal(event.defaulted, true, "Cmd+Z in the search was taken from the search");
  assert.equal(bookmarkAt(page), null, "Cmd+Z in the search put the bookmark back");

  const listed = page.el("keyboard-keys").descendants()
    .filter((node) => node.tagName === "kbd").map((node) => node.textContent);
  assert.ok(listed.includes("z"), `z is not in the list of keys: ${listed.join(", ")}`);
  assert.ok(listed.includes("Ctrl+Z"), "Ctrl+Z is not in the list of keys, next to z");
  const said = page.el("keyboard-keys").text();
  assert.match(said, /Undo the newest thing that can still be taken back/, "the list does not say undo is general");
  assert.match(said, /stays the browser's/, "the list does not say the chord leaves a text field's undo alone");
  for (const chip of ["undo-dismiss", "undo-send-dismiss", "undo-bookmark"]) {
    assert.equal(page.el(chip).getAttribute("aria-keyshortcuts"), "Z Control+Z Meta+Z",
      `#${chip} does not say both undo keys to a screen reader`);
  }
});

test("`#240 reading-bookmark`: reply, and b goes back to where you were reading", async () => {
  const page = await keyboardPageAtTop(tallChannel(30));
  await bookmarkRow(page, "4");
  // A message further down than the line: its Done (`#256 reply-marks-read`) leaves the line alone.
  // `#410 inline-reply-editor`. The reply is written in the list, and sending it moves nothing: the
  // reader is where they were reading, and b still goes back to the bookmark.
  const area = page.el("scroll-area");
  await replyButton(channelRow(page, "8")).click();
  await page.settle();
  const reading = area.scrollTop;
  page.el("reply-text").value = "on it";
  await page.el("reply-send").click();
  await page.settle();
  assert.equal(page.screen(), "main");
  assert.equal(area.scrollTop, reading, "sending moved the list");
  assert.equal(channelRow(page, "8").getAttribute("data-archived"), "true", "the message answered is not Done");
  assert.deepStrictEqual(bookmarkAt(page), { id: "4", place: "above" }, "sending moved the bookmark");
  await press(page, page.el("scroll-area"), "b");
  assert.equal(selectedId(page), "4", "b did not go back to the bookmark after the reply");
  // The message right under the line: the reply's Done slides the line on past it, as that Done from
  // its button does, and b goes on to the next message still to read.
  await replyButton(channelRow(page, "4")).click();
  await page.settle();
  page.el("reply-text").value = "and on this";
  await page.el("reply-send").click();
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: "5", place: "above" }, "the reply's Done did not slide the line as a Done does");
  await press(page, page.el("scroll-area"), "b");
  assert.equal(selectedId(page), "5", "b did not go to where the line slid");
});

test("`#240 reading-bookmark`: a long Done through here is ONE request, and past the server's ceiling it and its undo go in batches it accepts", async () => {
  const page = newPage();
  await signIn(page);
  const many = Array.from({ length: 150 }, (_unused, i) => message({ id: String(10000 + i), content: `message ${i}` }));
  await showDiscord(page, many);
  await throughHere(page, String(10000 + 149));
  assert.equal(page.dismissCalls.length, 1, `${page.dismissCalls.length} requests for one Done through here`);
  assert.equal(page.dismissCalls[0].messages.length, 150);
  // Over a thousand: two requests each way, none over the ceiling, and the undo puts back every one.
  const huge = newPage();
  await signIn(huge);
  const lots = Array.from({ length: 1100 }, (_unused, i) => message({ id: String(100000 + i), content: `m ${i}` }));
  await showDiscord(huge, lots);
  await throughHere(huge, String(100000 + 1099));
  assert.deepStrictEqual(huge.dismissCalls.map((call) => call.messages.length), [1000, 100]);
  await huge.el("undo-dismiss").click();
  await huge.settle();
  assert.deepStrictEqual(huge.restoreCalls.map((call) => call.messages.length), [1000, 100],
    "the undo of a long Done through here went out in one request the server refuses");
  assert.equal(huge.dealtWith.size, 0, "the undo did not put every message back");
});

/** A page of 1,100 messages, more than one batch, with the bookmark above the first. */
async function overOneBatch() {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, Array.from({ length: 1100 }, (_unused, i) => message({ id: String(100000 + i), content: `m ${i}` })));
  await bookmarkRow(page, "100000");
  return page;
}

/** Fail the `nth` write of `kind` ("dismiss" or "restore") from now on, and let every other through. */
function failNthWrite(page, kind, nth) {
  let seen = 0;
  page.inboxWriteHook = async (asked) => {
    if (asked !== kind) return undefined;
    seen += 1;
    return seen === nth ? json(502, { error: "store_error", detail: "the store is read-only" }) : undefined;
  };
}

test("`#240 reading-bookmark`: a Done through here whose second batch fails keeps the first Done, takes back only the rest, and Undo puts back what landed", async () => {
  const page = await overOneBatch();
  const calls = page.bookmarkCalls.length;
  failNthWrite(page, "dismiss", 2);
  await throughHere(page, "101099");
  page.inboxWriteHook = null;
  assert.deepStrictEqual(page.dismissCalls.map((call) => call.messages.length), [1000, 100]);
  assert.equal(page.dealtWith.size, 1000, "the fake server did not take the first batch");
  assert.equal(channelRow(page, "100999").getAttribute("data-archived"), "true",
    "a message the server has Done was shown to read again");
  assert.equal(channelRow(page, "101000").getAttribute("data-archived"), "false",
    "a message the server never took stayed Done");
  assert.match(page.el("error").text(), /read-only/, "the failure was not said");
  // The line moves only once the whole Done is saved, so it stays above the first, now Done.
  assert.deepStrictEqual(bookmarkAt(page), { id: "100000", place: "above" }, "the line moved for a Done that did not all land");
  assert.equal(page.bookmarkCalls.length, calls, "the slide of a Done that did not all land was sent");
  assert.equal(page.el("undo-dismiss").hidden, false, "what the server did take cannot be undone");
  await page.el("undo-dismiss").click();
  await page.settle();
  assert.deepStrictEqual(page.restoreCalls.map((call) => call.messages.length), [1000],
    "the undo did not put back exactly the batch the server took");
  assert.equal(page.dealtWith.size, 0);
  assert.equal(channelRow(page, "100999").getAttribute("data-archived"), "false");
});

test("`#240 reading-bookmark`: an Undo whose second batch fails leaves what was put back to read, and the line above it again", async () => {
  const page = await overOneBatch();
  await throughHere(page, "101099");
  assert.deepStrictEqual(bookmarkAt(page), { id: "101099", place: "below" }, "Done through here did not move the line");
  failNthWrite(page, "restore", 2);
  await page.el("undo-dismiss").click();
  await page.settle();
  page.inboxWriteHook = null;
  assert.deepStrictEqual(page.restoreCalls.map((call) => call.messages.length), [1000, 100]);
  assert.equal(page.dealtWith.size, 100, "the fake server did not put back the first batch");
  assert.equal(channelRow(page, "100999").getAttribute("data-archived"), "false",
    "a message the server put back was shown Done again");
  assert.equal(channelRow(page, "101000").getAttribute("data-archived"), "true",
    "a message the server never put back was shown to read");
  // Messages to read again are above where the Done moved the line, so it goes back above them.
  assert.deepStrictEqual(bookmarkAt(page), { id: "100000", place: "above" }, "the line stayed below messages put back to read");
  assert.equal(page.bookmark.anchor.message_id, "100000", "the server's bookmark stayed below messages put back to read");
  assert.match(page.el("error").text(), /read-only/, "the failure was not said");
});

test("`#240 reading-bookmark`: THE SWEEP ORACLE — Done through here marks exactly the rows above, not yet Done, under random Done, put-back and arrivals", async () => {
  // The page's sweep, against a from-scratch scan of the list after every random step; and the line,
  // against a from-scratch placement of the bookmark the fake server now holds.
  let seed = 240;
  const next = (n) => {
    seed = (seed * 1103515245 + 12345) % 2147483648;
    return seed % n;
  };
  const page = newPage();
  await signIn(page);
  await showDiscord(page, Array.from({ length: 12 }, (_unused, i) => message({ id: String(20000 + i), content: `message ${i}` })));
  let arrivals = 0;
  for (let step = 0; step < 40; step += 1) {
    const rows = page.el("discord-log").children;
    const at = next(rows.length);
    const id = rows[at].getAttribute("data-id");
    const choice = next(5);
    if (choice === 0) {
      await doneButton(rows[at]).click();
    } else if (choice === 1) {
      await bookmarkRow(page, id);
    } else if (choice === 2) {
      arrivals += 1;
      page.messages = [...page.messages, message({ id: String(30000 + arrivals), content: `arrival ${arrivals}` })];
      await reReadChannel(page);
    } else {
      const expected = rows.slice(0, at + 1).flatMap((li) => li.getAttribute("data-ids").split(" "))
        .filter((one) => !page.dealtWith.has(one));
      const before = page.dismissCalls.length;
      await throughHere(page, id);
      const sent = page.dismissCalls.slice(before).flatMap((call) => call.messages || []);
      assert.deepStrictEqual(sent, expected, `step ${step}: Done through ${id} marked the wrong set`);
    }
    await page.settle();
    const anchor = page.bookmark.anchor;
    const shown = page.el("discord-log").children.map((li) => li.getAttribute("data-id"));
    if (!anchor) {
      assert.equal(bookmarkAt(page), null, `step ${step}: a line with no bookmark`);
      continue;
    }
    const home = shown.indexOf(anchor.message_id);
    const start = home + (anchor.place === "below" ? 1 : 0);
    const want = start < shown.length ? { id: shown[start], place: "above" } : { id: shown[shown.length - 1], place: "below" };
    assert.deepStrictEqual(bookmarkAt(page), want, `step ${step}: the line is not where the bookmark puts it`);
  }
});

test("`#240 reading-bookmark`: under Hide read without threads, a Done right under the line, and Done through here, draw the line where it slid", async () => {
  // Hide read without threads takes a Done row out of the list itself, with no redraw of the rows:
  // the line went with the row it was drawn on, and its new place was drawn only at the next read.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(5));
  await chooseReadMode(page, "Hide read");
  await bookmarkRow(page, bid(1));
  await doneButton(channelRow(page, bid(1))).click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), [0, 2, 3, 4].map(bid));
  assert.equal(page.bookmark.anchor.message_id, bid(2), "the Done did not slide the bookmark");
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(2), place: "above" }, "the slid line was not drawn");
  await throughHere(page, bid(2));
  assert.deepStrictEqual(shownIds(page), [3, 4].map(bid));
  assert.equal(page.bookmark.anchor.message_id, bid(3));
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(3), place: "above" }, "the line Done through here slid was not drawn");
});

/**
 * `#240 reading-bookmark`. A channel of 12 messages, from 20000; with `threads`, every third one a
 * root with a reply posted after the next Main message, so Main skips the replies and the line has
 * to be placed among Main's rows by the channel's order.
 */
function oracleChannel(page, threads) {
  const messages = [];
  const summaries = [];
  for (let i = 0; i < 12; i += 1) {
    const id = String(20000 + i);
    if (threads && i % 3 === 0) {
      const thread = { id: `spaces/O/threads/${i}`, root_message_id: id, is_root: true, reply_count: 1, reply_count_exact: true };
      messages.push(message({ id, content: `root ${i}`, thread }));
      summaries.push({ thread, at: messages.length - 1 });
    } else {
      messages.push(message({ id, content: `message ${i}` }));
    }
    const owed = summaries.find((one) => one.reply === undefined && one.at < messages.length - 1);
    if (owed) {
      owed.reply = String(29000 + i);
      messages.push(message({ id: owed.reply, content: `reply in ${owed.thread.id}`, thread: { ...owed.thread, is_root: false } }));
    }
  }
  if (threads) {
    page.threadingSupported = true;
    page.threads = summaries.map(({ thread, at }) => ({ id: thread.id, root: messages[at], title: `Thread ${at}`,
      reply_count: 1, reply_count_exact: true, updated_at: messages[messages.length - 1].timestamp }));
  }
  return messages;
}

for (const [threads, mode] of [[false, "Fold read"], [false, "Hide read"], [true, "Show read"], [true, "Hide read"]]) {
  const where = threads ? `a threaded Main under ${mode}` : `${mode} without threads`;
  test(`\`#240 reading-bookmark\`: THE SWEEP ORACLE in ${where} — the sweep marks exactly the rows above not yet Done, and the line sits where a fresh placement puts it`, async () => {
    // As above, in the read modes that leave rows out, and in a Main that leaves the thread replies
    // out. The placement is worked out afresh from the fake server's own record: the channel's order,
    // the Done marks and the bookmark it holds, against only which rows the list shows.
    let seed = threads ? 2401 : mode === "Hide read" ? 2402 : 2403;
    // The high bits: this generator's low bits repeat with a short period.
    const next = (n) => {
      seed = (seed * 1103515245 + 12345) % 2147483648;
      return Math.floor((seed / 2147483648) * n);
    };
    const page = newPage();
    // Threading is the deployment's, said at sign-in, so the channel is arranged before it.
    const channel = oracleChannel(page, threads);
    await signIn(page);
    await showDiscord(page, channel);
    if (threads) {
      await pickThread(page, "main");
      assert.equal(shownIds(page).some((id) => id.startsWith("29")), false, "Main shows thread replies: the fixture has no threads");
    }
    await chooseReadMode(page, mode);
    let arrivals = 0;
    const acts = [];
    for (let step = 0; step < 60; step += 1) {
      const all = page.el("discord-log").children;
      // A collapsed run's line stands for the run: no act about one message is offered on it.
      const pickable = all.filter((li) => !li.hidden && li.getAttribute("data-read-run") !== "head");
      // Most of a reader's acts are Done, often on the message right under the line: the slide.
      const under = pickable.find((li) => li.getAttribute("data-bookmark") === "above");
      const roll = next(20);
      const act = pickable.length === 0 || roll < 4 ? "arrive"
        : roll < 9 ? "done-under" : roll < 12 ? "done" : roll < 15 ? "bookmark"
          : roll < 17 && !page.el("undo-dismiss").hidden ? "undo" : "through";
      const li = act === "done-under" && under ? under : pickable[next(Math.max(1, pickable.length))];
      acts.push(act);
      if (act === "arrive") {
        arrivals += 1;
        page.messages = [...page.messages, message({ id: String(30000 + arrivals), content: `arrival ${arrivals}` })];
        await reReadChannel(page);
      } else if (act === "done" || act === "done-under") {
        await doneButton(li).click();
      } else if (act === "bookmark") {
        await bookmarkRow(page, li.getAttribute("data-id"));
      } else if (act === "undo") {
        await page.el("undo-dismiss").click();
      } else {
        // Done through here, from the upper half of what is shown, so the list is not emptied at once.
        const near = pickable[next(Math.max(1, Math.ceil(pickable.length / 2)))];
        const at = all.indexOf(near);
        const expected = all.slice(0, at + 1).flatMap((one) => one.getAttribute("data-ids").split(" "))
          .filter((one) => !page.dealtWith.has(one));
        const before = page.dismissCalls.length;
        await throughHere(page, near.getAttribute("data-id"));
        const sent = page.dismissCalls.slice(before).flatMap((call) => call.messages || []);
        assert.deepStrictEqual(sent, expected, `step ${step}: Done through ${near.getAttribute("data-id")} marked the wrong set`);
      }
      await page.settle();
      const anchor = page.bookmark.anchor;
      const rows = page.el("discord-log").children;
      const order = new Map(page.messages.map((one, i) => [String(one.id), i]));
      let want = null;
      if (anchor && rows.length > 0) {
        // Above the first row shown holding a message at or after the bookmark (after it, for one
        // below its message), else below the last row shown.
        const from = order.get(anchor.message_id) + (anchor.place === "below" ? 0.5 : 0);
        const newest = (li) => Math.max(...li.getAttribute("data-ids").split(" ").map((one) => order.get(one)));
        const shown = rows.filter((li) => !li.hidden);
        const home = shown.find((li) => newest(li) >= from);
        want = home ? { id: home.getAttribute("data-id"), place: "above" }
          : shown.length ? { id: shown[shown.length - 1].getAttribute("data-id"), place: "below" } : null;
      }
      assert.deepStrictEqual(bookmarkAt(page), want, `step ${step}, after ${acts.slice(-3).join(", ")}: the line is not where the bookmark puts it`);
    }
  });
}

test("`#240 reading-bookmark`: in a channel with threads, a delta read carries a move made on the other device", async () => {
  const { page, server } = await forwardPage({}, (p) => {
    p.serveBookmark = true;
  });
  assert.equal(bookmarkAt(page), null);
  page.moveBookmarkElsewhere({ message_id: "203", place: "above", timestamp: page.messages[3].timestamp,
    thread_id: "spaces/A/threads/another" });
  await pollOnce(page);
  assert.notEqual(server.reads.at(-1).after, null, "the refresh was not a delta read");
  assert.deepStrictEqual(bookmarkAt(page), { id: "203", place: "above" }, "a delta carrying the other device's move drew nothing");
});

test("`#240 reading-bookmark`: a Done in the thread on the right slides the line there, keeping its thread", async () => {
  const { page } = await twoColumnServer();
  await threadBadge(rowWithId(page, "203")).click();
  await page.settle();
  await bookmarkRow(page, "203", "side-log");
  assert.deepStrictEqual(bookmarkAt(page, "side-log"), { id: "203", place: "above" });
  await doneButton(sideRow(page, "203")).click();
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page, "side-log"), { id: "204", place: "above" }, "the Done in the thread did not slide the line");
  assert.equal(page.bookmark.anchor.message_id, "204");
  assert.equal(page.bookmark.anchor.thread_id, "spaces/A/threads/another", "the slide lost the thread");
  // Nothing of Main's lies between the root and its reply, so Main on the left goes past both, by time.
  assert.deepStrictEqual(bookmarkAt(page), { id: "203", place: "below" }, "the left column did not follow the slide");
});

/**
 * `#240 reading-bookmark`. A channel whose thread T, rooted at 301, has its reply 304 posted after
 * two messages to the channel itself, 302 and 303: Main is 300, 301, 302, 303, 305, and T is 301,
 * 304. `extra(t)`, given T's record, makes messages to go on the end, later than those.
 */
function lateReplyThread(extra = () => []) {
  const t = { id: "spaces/A/threads/late", root_message_id: "301", is_root: true, reply_count: 1, reply_count_exact: true };
  const messages = [
    message({ id: "300", content: "main a" }),
    message({ id: "301", content: "the root", thread: t }),
    message({ id: "302", content: "main b, between the root and its reply" }),
    message({ id: "303", content: "main c" }),
    message({ id: "304", content: "the late reply", thread: { ...t, is_root: false } }),
    message({ id: "305", content: "main d" }),
  ];
  messages.push(...extra(t));
  const threads = [{ id: t.id, root: messages[1], title: "Late", reply_count: 1, reply_count_exact: true,
    updated_at: messages[4].timestamp }];
  return { messages, threads, thread: t };
}

/** A one-column page on `lateReplyThread`, in Main. */
async function lateReplyPage(extra = () => []) {
  const page = newPage();
  const data = lateReplyThread(extra);
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  await pickThread(page, "main");
  assert.deepStrictEqual(shownIds(page), ["300", "301", "302", "303", "305"]);
  return { page, data };
}

test("`#240 reading-bookmark`: a Done on a root in its own thread stops the line at Main's next unread message, not the thread's late reply", async () => {
  const { page } = await lateReplyPage();
  await bookmarkRow(page, "301");
  await threadBadge(rowWithId(page, "301")).click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["301", "304"]);
  assert.deepStrictEqual(bookmarkAt(page), { id: "301", place: "above" }, "the root's thread drew no line");
  await doneButton(rowWithId(page, "301")).click();
  await page.settle();
  assert.equal(page.bookmark.anchor.message_id, "302",
    "a Done in the thread carried the bookmark past Main's unread messages to the thread's late reply");
  assert.equal(page.bookmark.anchor.thread_id, undefined, "a message of Main's took a thread");
  assert.equal(bookmarkAt(page), null, "the thread draws a line that is now in Main");
  await press(page, page.el("thread-back"), "Escape");
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["300", "301", "302", "303", "305"]);
  assert.deepStrictEqual(bookmarkAt(page), { id: "302", place: "above" },
    "Main's line is not where the same Done in Main would have put it");
});

test("`#240 reading-bookmark`: ...Main's messages already Done are stepped over, and the line stays in the thread", async () => {
  const { page } = await lateReplyPage();
  page.dealtWith.add("302");
  page.dealtWith.add("303");
  await reReadChannel(page);
  await bookmarkRow(page, "301");
  await threadBadge(rowWithId(page, "301")).click();
  await page.settle();
  await doneButton(rowWithId(page, "301")).click();
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: "304", place: "above" }, "the line did not slide on in the thread past read messages of Main's");
  assert.equal(page.bookmark.anchor.thread_id, "spaces/A/threads/late");
  await press(page, page.el("thread-back"), "Escape");
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: "305", place: "above" }, "Main did not place the reply's line by time");
});

test("`#240 reading-bookmark`: ...nor does a Done on a reply in the thread, or Done through here there, carry Main's line past Main's unread", async () => {
  // T is now 301, 304, 306: Main's 305 lies between its two replies.
  const { page } = await lateReplyPage((t) => [message({ id: "306", content: "a later reply", thread: { ...t, is_root: false } })]);
  await threadBadge(rowWithId(page, "301")).click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["301", "304", "306"]);
  await bookmarkRow(page, "304");
  await doneButton(rowWithId(page, "304")).click();
  await page.settle();
  assert.equal(page.bookmark.anchor.message_id, "305", "a Done on a reply carried the line past Main's unread message");
  assert.equal(bookmarkAt(page), null);
  // Done through here, from the root at the line, is held back the same way: at Main's first unread.
  await bookmarkRow(page, "301");
  await throughHere(page, "306");
  assert.equal(page.bookmark.anchor.message_id, "302", "Done through here in the thread carried the line past Main's unread");
  await press(page, page.el("thread-back"), "Escape");
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: "302", place: "above" });
  assert.equal(page.dealtWith.has("302"), false, "Done through here in a thread marked a message of Main's");
});

test("`#240 reading-bookmark`: in two columns, a Done on the root in the thread on the right leaves the left's line at Main's next unread", async () => {
  const { page } = await twoColumnServer((p, data) => {
    const late = lateReplyThread();
    p.threads = late.threads;
    p.messages = late.messages;
    data.threads = late.threads;
    data.messages = late.messages;
  });
  assert.deepStrictEqual(shownIds(page), ["300", "301", "302", "303", "305"], "the left column is not Main");
  await bookmarkRow(page, "301");
  await threadBadge(rowWithId(page, "301")).click();
  await page.settle();
  assert.deepStrictEqual(sideIds(page), ["301", "304"]);
  assert.deepStrictEqual(bookmarkAt(page, "side-log"), { id: "301", place: "above" });
  await doneButton(sideRow(page, "301")).click();
  await page.settle();
  assert.equal(page.bookmark.anchor.message_id, "302", "the Done on the right carried the bookmark past Main's unread");
  assert.deepStrictEqual(bookmarkAt(page), { id: "302", place: "above" }, "the left column's line skipped Main's unread messages");
  assert.equal(bookmarkAt(page, "side-log"), null, "the right column kept a line that is now in Main");
});

test("`#240 reading-bookmark`: a screen cleared under a bookmark write still on its way keeps it, and the read that follows does not take it back", async () => {
  const page = await servedBookmarkPage(backlog(5));
  page.holdBookmarkWrites = true;
  await bookmarkRow(page, bid(1));
  // A read the server answers "no such channel" clears the screen (`clearChannelScreen`), with the
  // write still held: then the channel reads again, from before the write settles.
  const serve = page.channelPage;
  page.channelPage = async () => {
    page.channelPage = serve;
    return json(404, { error: "unknown_channel", detail: "no such channel" });
  };
  await reReadChannel(page);
  assert.deepStrictEqual(shownIds(page), [], "the fixture's refused read left the rows standing");
  await page.el("view-switch").click();
  await page.el("view-switch").click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), [bid(0), bid(1), bid(2), bid(3), bid(4)]);
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(1), place: "above" },
    "a read sent before the bookmark write settled took it back, once the screen had been cleared");
  page.holdBookmarkWrites = false;
  for (const release of page.pendingBookmarkWrites.splice(0)) release();
  await page.settle();
  assert.deepStrictEqual(bookmarkAt(page), { id: bid(1), place: "above" }, "the settled write was not shown");
  assert.equal(page.bookmark.anchor.message_id, bid(1));
});

// --- `#423 back-key-consistency` ----------------------------------------------------------------------
//
// The owner, 2026-10-22: "if I press `Cmd-[` on the thread list view it jumps back to the Main view but it
// does NOT have kind of navigation (or back?) effect on other views." Cmd+[ was the browser's Back, which
// `#304` gives entries only for what is open over a view. Now Ctrl/Cmd+[ and ] go back and forward through
// every place visited, the thread list's and a thread's Back being the special cases where the place before
// is the one under them. tests/threads_polish_browser.py walks it in Chromium.

/** Where the reader is: the screen, and on the channel the picker's choice and the layout. */
const whereNow = (page) => {
  const screen = page.el("frame-body").getAttribute("data-screen");
  if (screen !== "main") return screen;
  const layout = page.el("screen-main").getAttribute("data-columns");
  return layout === "two" ? (page.el("side-pane").hidden ? "two" : "two+thread") : layout === "list" ? "threads" : page.el("thread-select").value;
};

/** Ctrl/Cmd+[ (`back`) or ] from `target`: the chord is the page's, never the browser's Back or Forward. */
async function backKey(page, back = true, fields = { metaKey: true }, target = null) {
  const event = await press(page, target || page.el("scroll-area"), back ? "[" : "]", fields);
  assert.equal(event.defaulted, false, `${back ? "[" : "]"} was left to the browser, whose Back or Forward it is`);
  await page.settle();
  return event;
}

test("`#423 back-key-consistency`: Ctrl/Cmd+[ walks back through Main, All, the thread list, a thread opened from it and Settings, and ] forward again, in every view", async () => {
  for (const fields of [{ metaKey: true }, { ctrlKey: true }]) {
    const page = await threadPage("main");
    const thread = threadData().threads[0].id;
    const visits = ["main"];
    await press(page, page.el("scroll-area"), "2", { ctrlKey: true });
    visits.push("flat");
    await press(page, page.el("scroll-area"), "3", { ctrlKey: true });
    visits.push("threads");
    await pickThread(page, `thread:${thread}`);
    visits.push(`thread:${thread}`);
    await press(page, page.el("scroll-area"), ",");
    visits.push("settings");
    assert.equal(whereNow(page), "settings", "the premise is Settings, last of the places");
    for (let at = visits.length - 2; at >= 0; at -= 1) {
      await backKey(page, true, fields);
      assert.equal(whereNow(page), visits[at], `Back from ${visits[at + 1]} did not go to ${visits[at]}`);
    }
    // On to the start (the view the channel opened in came before Main): there it says so and stays
    // (negative control: never out of the app), then Forward comes back to Main.
    for (let press = 0; press < 3 && page.el("status").textContent !== "Nothing to go back to."; press += 1) {
      await backKey(page, true, fields);
    }
    assert.equal(page.el("status").textContent, "Nothing to go back to.", "Back never reached the start");
    assert.equal(page.el("frame-body").getAttribute("data-screen"), "main", "Back at the start left the channel");
    const start = whereNow(page);
    if (start !== "main") {
      await backKey(page, false, fields);
      assert.equal(whereNow(page), "main", `Forward from ${start}, where the channel opened, did not come back to Main`);
    }
    for (let at = 1; at < visits.length; at += 1) {
      await backKey(page, false, fields);
      assert.equal(whereNow(page), visits[at], `Forward from ${visits[at - 1]} did not go to ${visits[at]}`);
    }
    await backKey(page, false, fields);
    assert.equal(whereNow(page), "settings");
    assert.equal(page.el("status").textContent, "Nothing to go forward to.");
  }
});

test("`#423 back-key-consistency`: the thread list's Back is the view it was chosen from, as Esc's is, and a thread's from the list is the list — the special cases of the rule", async () => {
  for (const from of ["main", "flat"]) {
    const page = await threadPage(from);
    await press(page, page.el("scroll-area"), "3", { ctrlKey: true });
    await backKey(page);
    assert.equal(whereNow(page), from, `Back on the thread list did not go back to ${from}, where it was chosen from`);
  }
  // Esc from the same places goes to the same places (the rule's special case, not an exception).
  const page = await threadPage("flat");
  await press(page, page.el("scroll-area"), "3", { ctrlKey: true });
  const thread = threadData().threads[1].id;
  await pickThread(page, `thread:${thread}`);
  await backKey(page);
  assert.equal(whereNow(page), "threads", "Back from a thread opened from the list did not go back to the list");
  await press(page, page.el("side-scroll"), "Escape");
  await page.settle();
  assert.equal(whereNow(page), "flat", "the premise: Esc on the list goes back to All");
});

test("`#423 back-key-consistency`: in two columns, the thread on the right and the cards are places; Help is a place over the view it was opened from", async () => {
  const { page, data } = await twoColumnServer();
  assert.equal(whereNow(page), "two");
  await openerOf(cardRow(page, data.threads[0].id)).click();
  await page.settle();
  assert.equal(whereNow(page), "two+thread", "the premise is a thread on the right");
  await press(page, page.el("side-scroll"), "?");
  assert.equal(whereNow(page), "help");
  await backKey(page, true, { metaKey: true }, page.el("screen-help"));
  assert.equal(whereNow(page), "two+thread", "Back from Help did not go back to the thread on the right");
  await backKey(page);
  assert.equal(whereNow(page), "two", "Back from the thread on the right did not go back to its cards");
  await backKey(page);
  // The fixture's channel opens in All, and the layout button was pressed there.
  assert.equal(whereNow(page), "flat", "Back from two columns did not go back to the view before them");
  await backKey(page, false);
  await backKey(page, false);
  assert.equal(whereNow(page), "two+thread", "Forward did not open the thread on the right again");
  await backKey(page, false);
  assert.equal(whereNow(page), "help", "Forward did not open Help again");
});

test("`#423 back-key-consistency` negative controls: a new place drops the ones ahead; the bare [ and ] are still the columns' keys; a menu closes first; the call's view is no place", async () => {
  const page = await threadPage("main");
  await press(page, page.el("scroll-area"), "2", { ctrlKey: true });
  await backKey(page);
  assert.equal(whereNow(page), "main");
  await press(page, page.el("scroll-area"), "3", { ctrlKey: true });
  await backKey(page, false);
  assert.equal(whereNow(page), "threads", "Forward after a new place went to a place it dropped");
  assert.equal(page.el("status").textContent, "Nothing to go forward to.");
  // The bare [ and ] are not Back and Forward.
  await press(page, page.el("side-scroll"), "[");
  await press(page, page.el("side-scroll"), "]");
  assert.equal(whereNow(page), "threads", "the bare [ or ] moved through the places");
  await backKey(page);
  assert.equal(whereNow(page), "main");
  // A menu open: Back closes it, and goes nowhere.
  const row = page.el("discord-log").children.find((li) => li.getAttribute("data-id") === "200");
  await rowMoreButton(row).click();
  await page.settle();
  const menuOpen = () => rowMoreMenu(row).hidden === false;
  assert.ok(menuOpen(), "the premise is a menu open");
  await backKey(page, false);
  assert.ok(!menuOpen(), "Forward with a menu open did not close it");
  assert.equal(whereNow(page), "main", "Forward with a menu open went on to a place");
  // The call's view: the keys say so, and nothing moves.
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.el("pane-discord").hidden, true, "the premise is the call's view");
  await backKey(page);
  assert.equal(page.el("pane-discord").hidden, true, "Back on the call's view left it");
  assert.match(page.el("status").textContent, /the call has none/);
  // The ? help lists both chords.
  assert.ok(page.el("keyboard-keys").text().includes("+[ goes back to where you were before"), "the ? help does not list Back");
});

test("`#423 back-key-consistency`: Esc, the X or a Back button closing onto the place before is a step back — Cmd+[ then goes where Esc would have, and Forward opens it again", async () => {
  const page = await threadPage("flat");
  await press(page, page.el("scroll-area"), "3", { ctrlKey: true });
  const thread = threadData().threads[0].id;
  await pickThread(page, `thread:${thread}`);
  await press(page, page.el("scroll-area"), "Escape");
  await page.settle();
  assert.equal(whereNow(page), "threads", "the premise: Esc from the thread goes back to the list");
  await backKey(page);
  assert.equal(whereNow(page), "flat", "Back after Esc did not go where Esc on the list goes");
  await backKey(page, false);
  await backKey(page, false);
  assert.equal(whereNow(page), `thread:${thread}`, "Forward did not open again what Esc closed");
  // Settings and Help: Esc from Help opened in Settings is a step back too.
  await press(page, page.el("scroll-area"), ",");
  await page.el("help-link-keyboard").click();
  await page.settle();
  assert.equal(whereNow(page), "help", "the premise is Help opened from Settings");
  await press(page, page.el("screen-help"), "Escape");
  await page.settle();
  assert.equal(whereNow(page), "settings");
  await backKey(page, true, { metaKey: true }, page.el("screen-settings"));
  assert.equal(whereNow(page), `thread:${thread}`, "Back after Esc from Help reopened Help instead of leaving Settings");
});

test("`#423 back-key-consistency`: with nothing noted behind — a page opened on the thread list — Back closes it as Esc does, rather than saying there is nowhere to go", async () => {
  const page = newPage(new Map([["vibe-talk.voice.columns", "list"]]));
  const data = threadData();
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  await page.settle();
  assert.equal(whereNow(page), "threads", "the premise is a page opened on the thread list");
  await backKey(page);
  assert.notEqual(whereNow(page), "threads", "Back on a thread list the page opened on did nothing");
  assert.notEqual(page.el("status").textContent, "Nothing to go back to.");
  // Negative control: with nothing open over the view either, it says so.
  const plain = await threadPage();
  await backKey(plain);
  assert.equal(plain.el("status").textContent, "Nothing to go back to.");
});

test("`#423 back-key-consistency`: a place noted twice — two columns lost to a narrow window and back — is passed over, never a Back that changes nothing", async () => {
  const { page } = await twoColumnServer();
  await press(page, page.el("scroll-area"), "3", { ctrlKey: true });
  await press(page, page.el("scroll-area"), "4", { ctrlKey: true });
  assert.equal(whereNow(page), "two");
  // The window narrows: two columns give way to the layout they came from, the thread list.
  await page.setDesk(false);
  const narrowed = whereNow(page);
  assert.notEqual(narrowed, "two", "the premise: a narrow window takes two columns away");
  await backKey(page);
  assert.notEqual(whereNow(page), narrowed, "Back stayed on the place it was on");
});

// --- `#532 shift-o-latency` -----------------------------------------------------------------------
//
// The owner, 2026-10-26: Shift+O "does take a couple seconds. If I am folding up 4 messages in a row in a
// thread I thought I could spam the key four times and see them responsively fold up", and of e: "one of
// those things that I most want to be instantly responsive locally". Each press drew every thread
// unpacked in the Threads view again, three times over. Now a fold or a Done is drawn on the rows already
// there: the thread's card counts again in place, its rows are dressed again and its runs placed again,
// and every other thread is left alone.

/** `SHIFT_O_THREADS` threads of a root and `SHIFT_O_REPLIES` long replies, every message unread. */
const SHIFT_O_THREADS = 12;
const SHIFT_O_REPLIES = 5;
function shiftOData() {
  const messages = [];
  const threads = [];
  for (let t = 0; t < SHIFT_O_THREADS; t += 1) {
    const id = `spaces/A/threads/s${t}`;
    const rootId = String(9532000 + t * 10);
    const record = { id, root_message_id: rootId, is_root: true, reply_count: SHIFT_O_REPLIES, reply_count_exact: true };
    const root = message({ id: rootId, content: `thread ${t} root`, thread: record });
    messages.push(root);
    for (let r = 1; r <= SHIFT_O_REPLIES; r += 1) {
      messages.push(message({ id: String(9532000 + t * 10 + r), content: longMessage(`thread ${t} reply ${r}`),
        thread: { ...record, is_root: false } }));
    }
    threads.push({ id, root, title: "Thread", reply_count: SHIFT_O_REPLIES, reply_count_exact: true,
      updated_at: messages.at(-1).timestamp });
  }
  return { messages, threads };
}

async function shiftOPage(script = SCRIPT) {
  const data = shiftOData();
  const page = newPage(new Map([[UNPACK_STORAGE, "on"]]), script);
  page.threadingSupported = true;
  page.threads = data.threads;
  page.messages = data.messages;
  forwardTimeline(page);
  await signIn(page);
  await showDiscord(page, data.messages);
  await pickThread(page, "threads");
  await page.settle();
  return page;
}

/** Per-press budget in this fixture, which lays nothing out: a press that draws more than its own rows misses it. */
const SHIFT_O_BUDGET_MS = 50;

/**
 * Press `key` `times` times on whatever holds the focus, each press timed and the elements it built
 * counted, and the Threads view's rows of every other thread compared before and after.
 */
async function pressesInPlace(page, key, fields, times, own) {
  const others = () => page.el("side-cards").children.filter((li) => li.getAttribute("data-unpack-thread") !== own &&
    !(li.hasAttribute("data-card") && li.getAttribute("data-id") === own));
  const before = others();
  const took = [];
  let built = 0;
  for (let at = 0; at < times; at += 1) {
    const made = page.createdTags.length;
    const start = performance.now();
    await pressHere(page, key, fields);
    took.push(performance.now() - start);
    built += builtSince(page, made, "li");
  }
  const after = others();
  const kept = after.length === before.length && after.every((li, at) => li === before[at]);
  return { took, built, kept };
}

test("`#532 shift-o-latency`: four Shift+O and four e in an unpacked Threads view each draw in place, within budget, and the folds, Dones and Undo are right", async () => {
  const page = await shiftOPage();
  const own = "spaces/A/threads/s11";
  const rows = () => unpackRows(page, own);
  assert.equal(rows().length, SHIFT_O_REPLIES, "the premise is the last thread's replies unpacked under its card");
  const drawn = rows();
  await mouseOnto(page, unpackedRow(page, "9532115"));
  const folds = await pressesInPlace(page, "O", { shiftKey: true }, 4, own);
  assert.equal(folds.built, 0, `Shift+O built ${folds.built} rows: it drew the list again rather than fold in place`);
  assert.ok(folds.kept, "Shift+O drew the other threads again");
  assert.ok(rows().every((li, at) => li === drawn[at]), "Shift+O drew its own thread's rows again");
  assert.deepStrictEqual(rows().map((li) => li.getAttribute("data-read-run")), [null, "head", "member", "member", "member"],
    "four presses did not fold the four last replies into one line");
  assert.equal(readRunLineOf(rows()[1]).textContent, "… 4 read messages …");
  assert.equal(selectedId(page), "9532111", "the selection did not step above the folds");
  assert.deepStrictEqual(page.dismissCalls.map((call) => call.messages),
    [["9532115"], ["9532114"], ["9532113"], ["9532112"]], "each fold did not mark its message read");
  for (const li of rows().slice(1)) assert.equal(li.getAttribute("data-archived"), "true", "a folded row is not drawn read");
  assert.ok(folds.took.every((ms) => ms < SHIFT_O_BUDGET_MS),
    `a Shift+O took over ${SHIFT_O_BUDGET_MS} ms: ${folds.took.map((ms) => ms.toFixed(1)).join(", ")}`);

  // e down another thread: each Done drawn on its row, the card's unread counted again in place.
  const other = "spaces/A/threads/s10";
  const card = cardRow(page, other);
  const theirs = unpackRows(page, other);
  await mouseOnto(page, unpackedRow(page, "9532101"));
  const dones = await pressesInPlace(page, "e", {}, 4, other);
  assert.equal(dones.built, 0, `e built ${dones.built} rows: it drew the list again rather than the Done in place`);
  assert.ok(unpackRows(page, other).every((li, at) => li === theirs[at]), "e drew its thread's rows again");
  assert.equal(cardRow(page, other), card, "e drew the thread's card again rather than count it in place");
  assert.deepStrictEqual(page.dismissCalls.slice(4).map((call) => call.messages),
    [["9532101"], ["9532102"], ["9532103"], ["9532104"]], "e did not mark four messages Done, stepping down");
  assert.deepStrictEqual(unpackRows(page, other).map((li) => li.getAttribute("data-archived")),
    ["true", "true", "true", "true", "false"], "the Done rows are not drawn read");
  const counted = () => card.children.find((node) => node.className === "side-card-open").getAttribute("aria-label");
  assert.match(counted(), /· 1 unread$/, `the card did not count the Dones: ${counted()}`);
  assert.ok(dones.took.every((ms) => ms < SHIFT_O_BUDGET_MS),
    `an e took over ${SHIFT_O_BUDGET_MS} ms: ${dones.took.map((ms) => ms.toFixed(1)).join(", ")}`);
  // z puts back the last, on the row drawn, and the card counts it.
  await pressHere(page, "z");
  assert.deepStrictEqual(page.restoreCalls.at(-1), { messages: ["9532104"] }, "z did not undo the last e");
  assert.equal(unpackedRow(page, "9532104").getAttribute("data-archived"), "false", "z did not draw the row unread");
  assert.match(counted(), /· 2 unread$/, `the card did not count the put back: ${counted()}`);

  // THE CONTROL: the list drawn anew on every pass, as before: the same presses build rows, so the
  // checks above can see it.
  const control = await shiftOPage(brokenScriptEach(
    ['const keep = sideCardsDrawn !== "" && sideCardsKept.whole === whole;', "const keep = false;"]));
  await mouseOnto(control, unpackedRow(control, "9532115"));
  const redrawn = await pressesInPlace(control, "O", { shiftKey: true }, 1, own);
  assert.ok(redrawn.built > 0 && !redrawn.kept, "the negative control did not draw the list again");
});

test("`#532 shift-o-latency`: a kept block is drawn again for what its rows show: a Done of a message read by a rule, an edit, a reaction", async () => {
  // From the adversarial review: a block kept from the last pass is drawn again only for what its parts
  // say. A noise message is read by a rule whether Done or not, so "still to do" alone did not change.
  const data = shiftOData();
  const noisy = "9532113";
  const page = newPage(new Map([[UNPACK_STORAGE, "on"]]));
  page.threadingSupported = true;
  data.messages = data.messages.map((one) => (one.id === noisy ? { ...one, noise: true } : one));
  page.threads = data.threads;
  page.messages = data.messages;
  forwardTimeline(page);
  await signIn(page);
  await showDiscord(page, data.messages);
  await pickThread(page, "threads");
  await page.settle();
  const row = unpackedRow(page, noisy);
  assert.equal(row.getAttribute("data-noise"), "true", "the premise: a message read by a rule");
  await mouseOnto(page, row);
  await pressHere(page, "e");
  assert.deepStrictEqual(page.dismissCalls.at(-1), { messages: [noisy] }, "the premise: e marked it Done");
  assert.equal(unpackedRow(page, noisy).getAttribute("data-archived"), "true", "the Done of a noise message was not drawn");
  assert.equal(unpackedRow(page, noisy).getAttribute("data-noise"), "false", "a Done noise row still says it is noise");

  // An edit and a reaction are in what a kept row is drawn again for, though its id and read state are
  // the same; and the thread's reported count and its cover, which decide its loading line.
  const look = page.pure("messageLook");
  const base = data.messages.find((one) => one.id === "9532112");
  assert.notEqual(look({ ...base, content: `${base.content} and more`, edited_timestamp: "2026-10-26T12:00:00Z" }), look(base),
    "an edit is not in a row's signature");
  assert.notEqual(look({ ...base, reactions: [{ emoji: "👍", count: 1, me: false }] }), look(base),
    "a reaction is not in a row's signature");
  const choice = (count) => ({ ...page.pure("threadChoices")().find((one) => one.id === "spaces/A/threads/s11"), count });
  const held = page.pure("unpackHeldMessages")(choice(SHIFT_O_REPLIES));
  assert.notEqual(page.pure("unpackSignature")(choice(SHIFT_O_REPLIES + 1), held),
    page.pure("unpackSignature")(choice(SHIFT_O_REPLIES), held), "a thread grown by its count keeps its block, unread");
});

test("`#407 unpack-row-parity`: ; and : step the unpacked rows' folds as they step a thread's, each step said", async () => {
  const { page } = await parityPage();
  const area = page.el("side-scroll");
  area.focus();
  const rows = () => foldState(page, "side-cards");
  // `#528 thread-card-markdown-and-cap`: the raw thread's root is one long paragraph, so its card is
  // clamped and the root goes inward whole, folded like any long row.
  const all = ["700", "701", "702", "703", "704", "705", "706", "707"];
  assert.deepStrictEqual(rows(), { open: [], shut: all }, "the fixture's unpacked rows are not all folded");
  await press(page, area, ";");
  assert.equal(statusSaid(page), FOLD_SAID.expand, `; said ${statusSaid(page)}`);
  assert.deepStrictEqual(rows(), { open: all, shut: [] }, "; did not expand every unpacked row");
  await press(page, area, ":", { shiftKey: true });
  assert.equal(statusSaid(page), FOLD_SAID.collapse);
  assert.deepStrictEqual(rows(), { open: [], shut: all }, ": did not collapse every unpacked row");
  await press(page, area, ":", { shiftKey: true });
  assert.equal(statusSaid(page), FOLD_SAID.unread);
  assert.deepStrictEqual(rows(), { open: ["703", "707"], shut: ["700", "701", "702", "704", "705", "706"] },
    ": did not open the unread rows and fold the read ones");
});

test("`#407 unpack-row-parity`: Fold read draws a block's run as every list draws one, and Enter on it opens it", async () => {
  const { page } = await parityPage({ long: false });
  await page.el("todo-filter").click(); // Show -> Collapse
  await page.settle();
  const block = unpackRows(page, "spaces/A/threads/raw");
  const head = block.find((li) => li.getAttribute("data-read-run") === "head");
  assert.ok(head, "the raw block's read run is not the shared run line");
  assert.deepStrictEqual((head.getAttribute("data-ids") || "").split(" "), ["705"], "the run line is not on its first row");
  assert.equal(unpackedRow(page, "706").getAttribute("data-read-run"), "member");
  assert.equal(unpackedRow(page, "706").hidden, true, "the run's second row shows");
  assert.ok(!block.some((li) => li.hasClass("unpack-read-run")), "a second kind of run line is still drawn");
  assert.match(head.descendants().find((n) => n.className === "read-run").textContent, /2 read messages/);
  // The summarized root stays out of every run, as an open thread's root does.
  assert.equal(unpackedRow(page, "700").getAttribute("data-read-run"), null, "the summarized root joined a run");
  // Selected and opened by its key, as a run in Main is.
  const area = page.el("side-scroll");
  area.focus();
  await mouseOnto(page, head);
  assert.deepStrictEqual(litRows(page), ["side-cards:705"], "the run line is not selectable");
  await pressHere(page, "Enter");
  assert.equal(unpackedRow(page, "706").hidden, false, "Enter on the run line did not open it");
  assert.equal(unpackedRow(page, "705").getAttribute("data-read-run"), null);
  // And stays open as the block is drawn again, as a run opened in a thread stays open.
  await page.el("side-unpack").click();
  await page.el("side-unpack").click();
  await page.settle();
  assert.equal(unpackedRow(page, "706").hidden, false, "the opened run shut again when its block was drawn again");
});

test("`#407 unpack-row-parity`: Through here on an unpacked row covers the rows above it in its own thread, as in the thread", async () => {
  const { page } = await parityPage({ long: false, read: [] });
  const row = unpackedRow(page, "706");
  const through = row.descendants().find((n) => n.className === "row-read-through-button");
  await through.click();
  await page.settle();
  const marked = page.dismissCalls.at(-1).messages;
  assert.deepStrictEqual(marked, ["705", "706"], "Through here did not cover exactly its own thread's rows above it");
  assert.ok(!marked.some((id) => ["700", "701", "702", "703"].includes(id)),
    `Through here marked another thread's rows above it: ${marked}`);
});

test("`#407 unpack-row-parity` negative controls: Main, a thread, and the cards packed answer the same keys as before", async () => {
  /** The mouse onto `row` of the list `scroller` scrolls, its top at the top of the screen; then ↓. */
  const pointThenDown = async (page, scroller, row) => {
    const area = page.el(scroller);
    area.focus();
    area.scrollTop += row.getBoundingClientRect().top;
    await mouseTo(page, row, 300, row.getBoundingClientRect().top + 5);
    await pressHere(page, "ArrowDown");
  };
  // Main: ↓ from a message to the next, Enter opens it, ; expands.
  const main = await foldStepPage(6);
  await pointThenDown(main, "scroll-area", channelRow(main, "1"));
  assert.equal(selectedId(main), "2", "↓ in Main did not go to the next message");
  await pressHere(main, "Enter");
  assert.equal(channelRow(main, "2").getAttribute("data-collapsed"), "false", "Enter in Main did not open the message");
  await press(main, main.el("scroll-area"), ";");
  assert.equal(statusSaid(main), FOLD_SAID.expand);
  // A thread: the same keys over its rows.
  const { page } = await parityPage();
  await pickThread(page, "thread:spaces/A/threads/raw");
  await page.settle();
  const rows = page.el("discord-log").children.filter((li) => li.hasAttribute("data-id"));
  assert.deepStrictEqual(rows.map((li) => li.getAttribute("data-id")), ["704", "705", "706", "707"], "the thread did not open");
  await pointThenDown(page, "scroll-area", rows[1]);
  assert.equal(selectedId(page), "706", "↓ in a thread did not go on");
  await pressHere(page, "Enter");
  assert.equal(rows[2].getAttribute("data-collapsed"), "false", "Enter in a thread did not open the message");
  await press(page, page.el("scroll-area"), ";");
  assert.equal(statusSaid(page), FOLD_SAID.expand, "; in a thread said otherwise");
  // The cards with Unpack all off: ↓ goes card to card, and ; has nothing to fold.
  const { page: cards } = await unpackPage();
  await pointThenDown(cards, "side-scroll", cardRow(cards, "spaces/A/threads/sum"));
  assert.equal(heldRow(cards), "card:spaces/A/threads/raw", "↓ over packed cards did not go card to card");
  await press(cards, cards.el("side-scroll"), ";");
  assert.equal(statusSaid(cards), "No long messages here to expand or collapse.", "; over packed cards found folds");
});

test("`#407 unpack-row-parity`: an unpacked row's Done flash runs its course, its pin is drawn at once, and Enter on a run line leaves the keys on its row", async () => {
  const { page } = await parityPage({ long: false });
  await doneButton(unpackedRow(page, "703")).click();
  await page.settle();
  assert.equal(unpackedRow(page, "703").getAttribute("data-done-flash"), "marking", "the Done drew no moment of marking");
  page.expireTimers(DONE_FLASH_HOLD_MS);
  assert.equal(unpackedRow(page, "703").getAttribute("data-done-flash"), "fading");
  page.expireTimers(DONE_FLASH_FADE_MS);
  assert.equal(unpackedRow(page, "703").hasAttribute("data-done-flash"), false, "the moment outlived its timers on an unpacked row");
  // p pins the selected unpacked row, and the row says so at once, as a row of Main does.
  const area = page.el("side-scroll");
  area.focus();
  await mouseOnto(page, unpackedRow(page, "707"));
  await pressHere(page, "p");
  await page.settle();
  assert.equal(page.pinCalls.at(-1)?.id, "707", "p did not pin the selected unpacked row");
  assert.equal(unpackedRow(page, "707").getAttribute("data-pinned"), "true", "the pinned unpacked row is not drawn pinned");
  // Enter on a block's run line: the run opens and the keys stay on its first row, focused.
  await page.el("todo-filter").click(); // Show -> Collapse
  await page.settle();
  const head = unpackRows(page, "spaces/A/threads/raw").find((li) => li.getAttribute("data-read-run") === "head");
  area.focus();
  await mouseOnto(page, head);
  await pressHere(page, "Enter");
  assert.equal(unpackedRow(page, "706").hidden, false, "Enter did not open the run");
  assert.equal(focused(page), unpackedRow(page, "705"), `after Enter on the run line the focus is on ${focusName(page)}`);
  assert.equal(unpackedRow(page, "705").getAttribute("tabindex"), "0", "the run's first row is not the Tab stop");
  await pressHere(page, "ArrowDown");
  assert.equal(heldRow(page), "706", "↓ after opening the run did not go on from its first row");
});

test("`#407 unpack-row-parity`: Reply from an unpacked row opens under it among the cards, and Cancel leaves the cards where they were", async () => {
  const { page } = await parityPage({ long: false });
  const area = page.el("side-scroll");
  area.focus();
  await mouseOnto(page, unpackedRow(page, "703"));
  const was = area.scrollTop;
  await pressHere(page, "r");
  assert.ok(replyInStream(page), "r did not open the reply");
  // `#410 inline-reply-editor`. In the cards' list, under the row, as it opens under a row anywhere.
  assert.equal(page.el("side-reply").parentNode, page.el("side-list"), "the reply's box is not among the cards");
  assert.deepStrictEqual(inlineAnchors(page), ["side-cards:703"], "the box is not under the unpacked row");
  await page.el("reply-cancel").click();
  await page.settle();
  assert.equal(page.screen(), "main");
  assert.equal(area.scrollTop, was, "the cards moved under a reply from an unpacked row");
  assert.ok(page.el("side-cards").children.some((li) => li.hasAttribute("data-unpack-thread")), "the cards came back packed");
});

test("`#491 threads-view-sent-receipt`: a reply sent from among the cards, once drawn there, leaves no Sent row at the foot", async () => {
  // The owner, 2026-10-23: "After it sent there was still a card on the bottom AND it rendered as a card
  // under the thread. The same message is on the screen twice."
  const { page, data } = await parityPage({ long: false });
  const answered = data.messages.find((m) => m.id === "703");
  const mine = message({ id: "7990", content: "Answered from the cards.", reply_to: "703",
    thread: { ...answered.thread, is_root: false, reply_count: null, reply_count_exact: false } });
  page.replyResponse = async () => {
    page.messages.push(mine);
    return json(200, { posted: mine });
  };
  const area = page.el("side-scroll");
  area.focus();
  await mouseOnto(page, unpackedRow(page, "703"));
  await pressHere(page, "r");
  assert.ok(replyInStream(page), "r did not open the reply");
  await page.el("reply-text").setValue("Answered from the cards.");
  await page.el("reply-send").click();
  await page.settle();
  await deliver(page, page.stream(), sseMessage(mine, { self_posted: true }));
  await page.settle();
  const drawn = () => page.el("side-cards").children.filter((li) => li.hasAttribute("data-unpack-thread") &&
    li.text().includes("Answered from the cards."));
  const receipts = () => page.el("outgoing-log").children.filter((li) => li.text().includes("Answered from the cards."));
  assert.equal(drawn().length, 1, "the premise: the reply is not drawn among the cards");
  assert.deepEqual(receipts().map((li) => li.getAttribute("data-send-state")), [],
    "the reply is drawn among the cards and again as a receipt at the foot");
});
